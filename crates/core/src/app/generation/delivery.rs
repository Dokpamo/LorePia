use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;

use lorepia_chat::ChatEvent;
use lorepia_domain::{
    CapabilityKey, ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId,
    GenerationRequest, GenerationStatus, Message, MessageId, TransformSet, VariableMap,
};
use lorepia_providers::Provider;
use lorepia_storage::Storage;
use tokio::sync::{broadcast, watch};

use super::{GenerationCredential, GenerationLaunchPermit};
use crate::app::{
    Core,
    generation_events::{
        GenerationDeliveryPhase, GenerationEventSubscription, GenerationRegistry,
        generation_subscription_unavailable,
    },
    generation_workflow::execute_generation_task,
};

pub(in crate::app) struct GenerationTask {
    pub(in crate::app) storage: Arc<Storage>,
    pub(in crate::app) active_generations: Arc<GenerationRegistry>,
    pub(in crate::app) event_bus: broadcast::Sender<ChatEvent>,
    pub(in crate::app) branch_id: ConversationBranchId,
    pub(in crate::app) request: GenerationRequest,
    pub(in crate::app) assistant: Message,
    pub(in crate::app) provider: Arc<dyn Provider>,
    pub(in crate::app) credential: GenerationCredential,
    pub(in crate::app) cancel_receiver: watch::Receiver<bool>,
    pub(in crate::app) preserve_partial: bool,
    pub(in crate::app) transforms: GenerationTransformContext,
}

#[derive(Clone)]
pub(in crate::app) struct GenerationTransformContext {
    pub(in crate::app) sets: Vec<TransformSet>,
    pub(in crate::app) variables: VariableMap,
    pub(in crate::app) supported_capabilities: Vec<CapabilityKey>,
    pub(in crate::app) approved_import_source_ids: std::collections::BTreeSet<String>,
    pub(in crate::app) display_context: Option<lorepia_domain::PromptResolutionContext>,
}

impl From<crate::orchestration::PreparedGenerationPlan> for GenerationTransformContext {
    fn from(prepared: crate::orchestration::PreparedGenerationPlan) -> Self {
        Self {
            sets: prepared.transform_sets,
            variables: prepared.variables,
            supported_capabilities: prepared.supported_capabilities,
            approved_import_source_ids: prepared.approved_import_source_ids,
            display_context: Some(prepared.display_context),
        }
    }
}

pub(in crate::app) struct TerminalPersistenceContext<'a> {
    pub(in crate::app) storage: &'a Storage,
    pub(in crate::app) generation_id: &'a GenerationId,
}

pub(in crate::app) struct GenerationCompletionContext {
    pub(in crate::app) storage: Arc<Storage>,
    pub(in crate::app) active_generations: Arc<GenerationRegistry>,
    pub(in crate::app) event_bus: broadcast::Sender<ChatEvent>,
    pub(in crate::app) branch_id: ConversationBranchId,
    pub(in crate::app) conversation_id: ConversationId,
    pub(in crate::app) generation_id: GenerationId,
    pub(in crate::app) assistant_message_id: MessageId,
    pub(in crate::app) preserve_partial: bool,
    pub(in crate::app) transforms: GenerationTransformContext,
}

pub(in crate::app) struct GenerationEventForwardingContext {
    pub(in crate::app) active_generations: Arc<GenerationRegistry>,
    pub(in crate::app) event_bus: broadcast::Sender<ChatEvent>,
    pub(in crate::app) storage: Arc<Storage>,
    pub(in crate::app) checkpoint: Message,
    pub(in crate::app) branch_id: ConversationBranchId,
    pub(in crate::app) assistant_message_id: MessageId,
    pub(in crate::app) preserve_partial: bool,
    pub(in crate::app) defer_text_events: bool,
}

pub(in crate::app) struct ActiveGenerationGuard {
    pub(in crate::app) generation_id: GenerationId,
    pub(in crate::app) active_generations: Arc<GenerationRegistry>,
}

impl Drop for ActiveGenerationGuard {
    fn drop(&mut self) {
        self.active_generations.remove(&self.generation_id);
    }
}

impl Core {
    pub fn cancel_generation(&self, generation_id: &GenerationId) -> CoreResult<()> {
        self.inner.active_generations.cancel(generation_id)
    }

    /// Atomically validates a live generation route and subscribes at its
    /// authoritative event watermark.
    pub fn subscribe_generation_events(
        &self,
        generation_id: &GenerationId,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<GenerationEventSubscription> {
        let entry = self.inner.active_generations.entry(generation_id)?;
        let delivery = entry
            .delivery
            .lock()
            .map_err(|_| CoreError::internal("generation delivery lock was poisoned"))?;
        if delivery.phase == GenerationDeliveryPhase::Terminal
            || entry.route.conversation != *conversation_id
            || entry.route.branch != *branch_id
        {
            return Err(generation_subscription_unavailable());
        }

        let generation = self.inner.storage.get_generation(generation_id)?;
        if generation.status != GenerationStatus::Running
            || generation.conversation_id != *conversation_id
            || generation.branch_id != *branch_id
            || generation.assistant_message_id.as_ref() != Some(&entry.route.assistant_message)
        {
            return Err(generation_subscription_unavailable());
        }

        #[cfg(test)]
        if let Some(pause) = entry
            .subscription_pause
            .lock()
            .map_err(|_| CoreError::internal("generation subscription test lock was poisoned"))?
            .take()
        {
            pause
                .entered
                .send(())
                .map_err(|_| CoreError::internal("generation subscription test did not start"))?;
            pause
                .release
                .recv_timeout(Duration::from_secs(2))
                .map_err(|_| CoreError::internal("generation subscription test timed out"))?;
        }

        let receiver = self.inner.event_bus.subscribe();
        let live_prefix = delivery.live_prefix.as_ref().ok_or_else(|| {
            CoreError::internal("live generation catch-up prefix exceeded its bounded contract")
        })?;
        Ok(GenerationEventSubscription {
            receiver,
            assistant_message_id: entry.route.assistant_message.clone(),
            sequence_watermark: delivery.sequence_watermark,
            display_prefix: live_prefix.display.clone(),
            reasoning_prefix: live_prefix.reasoning.clone(),
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_generation_task(
        &self,
        launch: GenerationLaunchPermit,
        branch_id: ConversationBranchId,
        request: GenerationRequest,
        assistant_message: Message,
        provider: Arc<dyn Provider>,
        credential: GenerationCredential,
        transforms: GenerationTransformContext,
    ) -> CoreResult<GenerationId> {
        let generation_id = request.generation_id.clone();
        let task = launch.into_task(
            Arc::clone(&self.inner.storage),
            self.inner.event_bus.clone(),
            branch_id,
            request,
            assistant_message,
            provider,
            credential,
            transforms,
        )?;
        self.inner.runtime.spawn(execute_generation_task(task));
        Ok(generation_id)
    }
}
