use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

use lorepia_chat::{
    ChatEvent, ChatEventKind, MAX_GENERATED_OUTPUT_BYTES, MAX_GENERATED_OUTPUT_CHARS,
};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    GenerationRecord, MessageId, ProviderConnectionId,
};
use tokio::sync::{broadcast, watch};

use super::{
    MAX_ACTIVE_GENERATIONS_PER_CONVERSATION, MAX_ACTIVE_GENERATIONS_PER_PROCESS,
    MAX_ACTIVE_GENERATIONS_PER_PROVIDER, MAX_LIVE_DISPLAY_PREFIX_BYTES,
    MAX_LIVE_DISPLAY_PREFIX_CHARS,
};

/// Atomic process-local subscription state for one running generation.
///
/// The receiver, sequence watermark, and bounded display/reasoning prefixes are
/// captured under the same delivery mutex used by generation publishers.
/// Callers therefore either observe a durable terminal status or can rebuild
/// the exact live presentation through the returned watermark before receiving
/// every later event. This process-local snapshot exists only while the
/// generation is registered as live; terminal recovery reads the durable
/// message/projection instead of subscribing again.
pub struct GenerationEventSubscription {
    pub(super) receiver: broadcast::Receiver<ChatEvent>,
    pub(super) assistant_message_id: MessageId,
    pub(super) sequence_watermark: u64,
    pub(super) display_prefix: String,
    pub(super) reasoning_prefix: String,
}

impl GenerationEventSubscription {
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        broadcast::Receiver<ChatEvent>,
        MessageId,
        u64,
        String,
        String,
    ) {
        (
            self.receiver,
            self.assistant_message_id,
            self.sequence_watermark,
            self.display_prefix,
            self.reasoning_prefix,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GenerationDeliveryPhase {
    Preparing,
    Running,
    Terminal,
}

pub(super) struct GenerationRoute {
    pub(super) conversation: ConversationId,
    pub(super) branch: ConversationBranchId,
    pub(super) assistant_message: MessageId,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) enum GenerationProviderAdmissionKey {
    Connection(ProviderConnectionId),
    ProviderProfile(String),
    #[cfg(test)]
    DirectModel(String),
}

pub(super) struct GenerationDeliveryState {
    pub(super) phase: GenerationDeliveryPhase,
    pub(super) sequence_watermark: u64,
    pub(super) live_prefix: Option<GenerationLivePrefix>,
}

#[derive(Default)]
pub(super) struct GenerationLivePrefix {
    pub(super) display: String,
    pub(super) reasoning: String,
    pub(super) display_chars: usize,
    pub(super) reasoning_chars: usize,
}

impl GenerationLivePrefix {
    pub(super) fn append(&mut self, kind: &ChatEventKind) -> bool {
        let (target, chars, max_bytes, max_chars, delta) = match kind {
            ChatEventKind::TextDelta(delta) => (
                &mut self.display,
                &mut self.display_chars,
                MAX_LIVE_DISPLAY_PREFIX_BYTES,
                MAX_LIVE_DISPLAY_PREFIX_CHARS,
                delta,
            ),
            ChatEventKind::ReasoningDelta(delta) => (
                &mut self.reasoning,
                &mut self.reasoning_chars,
                MAX_GENERATED_OUTPUT_BYTES,
                MAX_GENERATED_OUTPUT_CHARS,
                delta,
            ),
            _ => return true,
        };
        let Some(next_bytes) = target.len().checked_add(delta.len()) else {
            return false;
        };
        let Some(next_chars) = chars.checked_add(delta.chars().count()) else {
            return false;
        };
        if next_bytes > max_bytes || next_chars > max_chars {
            return false;
        }
        target.push_str(delta);
        *chars = next_chars;
        true
    }
}

pub(super) struct ActiveGeneration {
    cancel: watch::Sender<bool>,
    pub(super) route: GenerationRoute,
    provider_admission_key: GenerationProviderAdmissionKey,
    pub(super) delivery: Mutex<GenerationDeliveryState>,
    #[cfg(test)]
    pub(super) subscription_pause: Mutex<Option<GenerationSubscriptionPause>>,
}

#[cfg(test)]
pub(super) struct GenerationSubscriptionPause {
    pub(super) entered: std::sync::mpsc::Sender<()>,
    pub(super) release: std::sync::mpsc::Receiver<()>,
}

#[derive(Default)]
pub(super) struct GenerationRegistry {
    pub(super) active: Mutex<HashMap<GenerationId, Arc<ActiveGeneration>>>,
    drained: Condvar,
}

impl GenerationRegistry {
    pub(super) fn register(
        &self,
        generation: &GenerationRecord,
        provider_admission_key: GenerationProviderAdmissionKey,
        cancel: watch::Sender<bool>,
    ) -> CoreResult<()> {
        let assistant_message_id = generation.assistant_message_id.clone().ok_or_else(|| {
            CoreError::internal("running generation is missing its assistant message route")
        })?;
        let mut active = self
            .active
            .lock()
            .map_err(|_| CoreError::internal("generation registry lock was poisoned"))?;
        if active.contains_key(&generation.id) {
            return Err(CoreError::internal(
                "generation is already registered for delivery",
            ));
        }
        if active.len() >= MAX_ACTIVE_GENERATIONS_PER_PROCESS {
            return Err(generation_admission_limit_reached("process"));
        }
        if active
            .values()
            .filter(|entry| entry.route.conversation == generation.conversation_id)
            .count()
            >= MAX_ACTIVE_GENERATIONS_PER_CONVERSATION
        {
            return Err(generation_admission_limit_reached("conversation"));
        }
        if active
            .values()
            .filter(|entry| entry.provider_admission_key == provider_admission_key)
            .count()
            >= MAX_ACTIVE_GENERATIONS_PER_PROVIDER
        {
            return Err(generation_admission_limit_reached("provider"));
        }
        active.insert(
            generation.id.clone(),
            Arc::new(ActiveGeneration {
                cancel,
                route: GenerationRoute {
                    conversation: generation.conversation_id.clone(),
                    branch: generation.branch_id.clone(),
                    assistant_message: assistant_message_id,
                },
                provider_admission_key,
                delivery: Mutex::new(GenerationDeliveryState {
                    phase: GenerationDeliveryPhase::Preparing,
                    sequence_watermark: 0,
                    live_prefix: Some(GenerationLivePrefix::default()),
                }),
                #[cfg(test)]
                subscription_pause: Mutex::new(None),
            }),
        );
        Ok(())
    }

    pub(super) fn activate(&self, generation_id: &GenerationId) -> CoreResult<()> {
        let entry = self.entry(generation_id)?;
        let mut delivery = entry
            .delivery
            .lock()
            .map_err(|_| CoreError::internal("generation delivery lock was poisoned"))?;
        if delivery.phase != GenerationDeliveryPhase::Preparing {
            return Err(CoreError::internal(
                "generation delivery phase cannot be activated",
            ));
        }
        delivery.phase = GenerationDeliveryPhase::Running;
        Ok(())
    }

    pub(super) fn entry(&self, generation_id: &GenerationId) -> CoreResult<Arc<ActiveGeneration>> {
        self.active
            .lock()
            .map_err(|_| CoreError::internal("generation registry lock was poisoned"))?
            .get(generation_id)
            .cloned()
            .ok_or_else(generation_subscription_unavailable)
    }

    pub(super) fn publish(
        &self,
        event_bus: &broadcast::Sender<ChatEvent>,
        event: ChatEvent,
    ) -> CoreResult<()> {
        let entry = self.entry(&event.generation_id)?;
        let mut delivery = entry
            .delivery
            .lock()
            .map_err(|_| CoreError::internal("generation delivery lock was poisoned"))?;
        if delivery.phase != GenerationDeliveryPhase::Running {
            return Err(CoreError::internal(
                "generation event was published outside the running phase",
            ));
        }
        if event.conversation_id != entry.route.conversation
            || event.branch_id.as_ref() != Some(&entry.route.branch)
            || event.assistant_message_id.as_ref() != Some(&entry.route.assistant_message)
        {
            return Err(CoreError::internal(
                "generation event route does not match the registered route",
            ));
        }
        if event.sequence <= delivery.sequence_watermark {
            return Err(CoreError::internal(
                "generation event sequence is not strictly increasing",
            ));
        }
        let is_terminal = matches!(
            &event.kind,
            ChatEventKind::GenerationCancelled
                | ChatEventKind::GenerationFailed { .. }
                | ChatEventKind::GenerationFinished
        );
        if delivery
            .live_prefix
            .as_mut()
            .is_some_and(|prefix| !prefix.append(&event.kind))
        {
            // The normal provider stream is already bounded by these same
            // cumulative output limits. A larger post-commit display
            // projection may still be delivered to an existing receiver, but
            // cannot be used as a process-local reattachment snapshot.
            delivery.live_prefix = None;
        }
        let sequence = event.sequence;
        let _ = event_bus.send(event);
        delivery.sequence_watermark = sequence;
        if is_terminal {
            delivery.phase = GenerationDeliveryPhase::Terminal;
        }
        Ok(())
    }

    pub(super) fn cancel(&self, generation_id: &GenerationId) -> CoreResult<()> {
        let entry = self.entry(generation_id)?;
        entry.cancel.send(true).map_err(|_| {
            CoreError::new(CoreErrorCode::Cancelled, "generation already stopped", true)
        })
    }

    pub(super) fn remove(&self, generation_id: &GenerationId) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(generation_id);
            if active.is_empty() {
                self.drained.notify_all();
            }
        }
    }

    pub(super) fn len(&self) -> usize {
        self.active.lock().map_or(0, |active| active.len())
    }

    pub(super) fn cancel_all_and_wait(&self, timeout: Duration) {
        let Ok(mut active) = self.active.lock() else {
            return;
        };
        for entry in active.values() {
            let _ = entry.cancel.send(true);
        }
        let deadline = Instant::now() + timeout;
        while !active.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.drained.wait_timeout(active, remaining) {
                Ok((next, result)) => {
                    active = next;
                    if result.timed_out() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    #[cfg(test)]
    pub(super) fn sequence_watermark_for_test(&self, generation_id: &GenerationId) -> Option<u64> {
        let entry = self.active.lock().ok()?.get(generation_id).cloned()?;
        entry
            .delivery
            .lock()
            .ok()
            .map(|delivery| delivery.sequence_watermark)
    }

    #[cfg(test)]
    pub(super) fn phase_for_test(
        &self,
        generation_id: &GenerationId,
    ) -> Option<GenerationDeliveryPhase> {
        let entry = self.active.lock().ok()?.get(generation_id).cloned()?;
        entry.delivery.lock().ok().map(|delivery| delivery.phase)
    }

    #[cfg(test)]
    pub(super) fn pause_next_subscription_for_test(
        &self,
        generation_id: &GenerationId,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> CoreResult<()> {
        let entry = self.entry(generation_id)?;
        let mut pause = entry
            .subscription_pause
            .lock()
            .map_err(|_| CoreError::internal("generation subscription test lock was poisoned"))?;
        if pause.is_some() {
            return Err(CoreError::internal(
                "generation subscription test pause is already installed",
            ));
        }
        *pause = Some(GenerationSubscriptionPause { entered, release });
        Ok(())
    }
}

pub(super) fn generation_subscription_unavailable() -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        "generation subscription is unavailable",
        false,
    )
}

fn generation_admission_limit_reached(scope: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::ProviderRateLimited,
        format!("active generation {scope} concurrency limit reached"),
        true,
    )
}
