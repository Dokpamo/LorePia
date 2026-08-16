use std::fmt;

use lorepia_core::{CHAT_EVENT_VERSION, ChatEvent};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use crate::dto::ChatEventDto;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconcileReason {
    BroadcastLagged,
    UnsupportedEventVersion,
    RouteMismatch,
    DuplicateOrDecreasingSequence,
    SequenceGap,
    /// Initial process-local presentation snapshot for an atomic live
    /// reattachment. This is a catch-up control item, not a lossy condition.
    LiveSnapshot,
    EventAfterTerminal,
}

/// Control-plane signal produced by the shell adapter.
///
/// This is intentionally not a `ChatEventKind`: Core did not emit it. The
/// frontend must reload persisted conversation state before trusting further
/// deltas for this generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationRequiredDto {
    pub reason: ReconcileReason,
    pub generation_id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub last_sequence: Option<u64>,
    pub observed_sequence: Option<u64>,
    pub dropped_events: Option<u64>,
    pub supported_event_version: u32,
    /// Exact process-local display text emitted through `observed_sequence`.
    /// Present only on the initial atomic same-process reattachment marker.
    #[serde(default)]
    pub display_prefix: Option<String>,
    /// Exact process-local reasoning text emitted through `observed_sequence`.
    /// Present only on the initial atomic same-process reattachment marker.
    #[serde(default)]
    pub reasoning_prefix: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ChatStreamItem {
    Event(ChatEventDto),
    ReconciliationRequired(ReconciliationRequiredDto),
    Closed,
}

/// One filtered subscription over Core's process-local chat broadcast.
///
/// The receiver is subscribed before a generation is started so the initial
/// sequence cannot race the command response. Events from other generations
/// are ignored inside Rust. Any loss or protocol inconsistency becomes a
/// separate reconciliation control item.
pub struct ChatEventStream {
    receiver: broadcast::Receiver<ChatEvent>,
    generation_id: String,
    conversation_id: String,
    branch_id: String,
    assistant_message_id: Option<String>,
    last_sequence: Option<u64>,
    terminal_seen: bool,
    initial_control: Option<ReconciliationRequiredDto>,
    reconciliation_required: bool,
}

pub(crate) struct GenerationReattachment {
    pub assistant_message_id: String,
    pub sequence_baseline: u64,
    pub authoritative_watermark: u64,
    pub display_prefix: String,
    pub reasoning_prefix: String,
}

impl ChatEventStream {
    pub(crate) fn new(
        receiver: broadcast::Receiver<ChatEvent>,
        generation_id: String,
        conversation_id: String,
        branch_id: String,
    ) -> Self {
        Self::new_with_sequence_baseline(receiver, generation_id, conversation_id, branch_id, 0)
    }

    pub(crate) fn new_with_sequence_baseline(
        receiver: broadcast::Receiver<ChatEvent>,
        generation_id: String,
        conversation_id: String,
        branch_id: String,
        sequence_baseline: u64,
    ) -> Self {
        Self {
            receiver,
            generation_id,
            conversation_id,
            branch_id,
            assistant_message_id: None,
            last_sequence: Some(sequence_baseline),
            terminal_seen: false,
            initial_control: None,
            reconciliation_required: false,
        }
    }

    pub(crate) fn reattached(
        receiver: broadcast::Receiver<ChatEvent>,
        generation_id: String,
        conversation_id: String,
        branch_id: String,
        reattachment: GenerationReattachment,
    ) -> Self {
        let GenerationReattachment {
            assistant_message_id,
            sequence_baseline,
            authoritative_watermark,
            display_prefix,
            reasoning_prefix,
        } = reattachment;
        let initial_control = Some(ReconciliationRequiredDto {
            reason: ReconcileReason::LiveSnapshot,
            generation_id: generation_id.clone(),
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            last_sequence: Some(sequence_baseline),
            observed_sequence: Some(authoritative_watermark),
            dropped_events: None,
            supported_event_version: CHAT_EVENT_VERSION,
            display_prefix: Some(display_prefix),
            reasoning_prefix: Some(reasoning_prefix),
        });
        Self {
            receiver,
            generation_id,
            conversation_id,
            branch_id,
            assistant_message_id: Some(assistant_message_id),
            last_sequence: Some(authoritative_watermark),
            terminal_seen: false,
            initial_control,
            reconciliation_required: false,
        }
    }

    pub async fn recv(&mut self) -> ChatStreamItem {
        if let Some(control) = self.initial_control.take() {
            return ChatStreamItem::ReconciliationRequired(control);
        }
        if self.reconciliation_required {
            return ChatStreamItem::Closed;
        }
        loop {
            let event = match self.receiver.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(dropped_events)) => {
                    let item = self.require_reconciliation(
                        ReconcileReason::BroadcastLagged,
                        None,
                        Some(dropped_events),
                    );
                    return item;
                }
                Err(broadcast::error::RecvError::Closed) => return ChatStreamItem::Closed,
            };

            if event.generation_id.0 != self.generation_id {
                continue;
            }

            let event = ChatEventDto::from(event);
            if !event.is_supported_version() {
                return self.require_reconciliation(
                    ReconcileReason::UnsupportedEventVersion,
                    Some(event.sequence),
                    None,
                );
            }
            if event.conversation_id != self.conversation_id
                || event.branch_id.as_deref() != Some(self.branch_id.as_str())
            {
                return self.require_reconciliation(
                    ReconcileReason::RouteMismatch,
                    Some(event.sequence),
                    None,
                );
            }
            let Some(observed_assistant_message_id) = event.assistant_message_id.as_deref() else {
                return self.require_reconciliation(
                    ReconcileReason::RouteMismatch,
                    Some(event.sequence),
                    None,
                );
            };
            if self
                .assistant_message_id
                .as_deref()
                .is_some_and(|expected| expected != observed_assistant_message_id)
            {
                return self.require_reconciliation(
                    ReconcileReason::RouteMismatch,
                    Some(event.sequence),
                    None,
                );
            }
            if self.terminal_seen {
                return self.require_reconciliation(
                    ReconcileReason::EventAfterTerminal,
                    Some(event.sequence),
                    None,
                );
            }

            let sequence_problem = match self.last_sequence {
                Some(last) if event.sequence <= last => {
                    Some(ReconcileReason::DuplicateOrDecreasingSequence)
                }
                Some(_) | None => None,
            };
            if let Some(reason) = sequence_problem {
                let observed = event.sequence;
                return self.require_reconciliation(reason, Some(observed), None);
            }

            if self.assistant_message_id.is_none() {
                self.assistant_message_id = Some(observed_assistant_message_id.to_owned());
            }
            self.last_sequence = Some(event.sequence);
            self.terminal_seen = event.kind.is_terminal();
            return ChatStreamItem::Event(event);
        }
    }

    fn require_reconciliation(
        &mut self,
        reason: ReconcileReason,
        observed_sequence: Option<u64>,
        dropped_events: Option<u64>,
    ) -> ChatStreamItem {
        self.reconciliation_required = true;
        ChatStreamItem::ReconciliationRequired(ReconciliationRequiredDto {
            reason,
            generation_id: self.generation_id.clone(),
            conversation_id: self.conversation_id.clone(),
            branch_id: self.branch_id.clone(),
            last_sequence: self.last_sequence,
            observed_sequence,
            dropped_events,
            supported_event_version: CHAT_EVENT_VERSION,
            display_prefix: None,
            reasoning_prefix: None,
        })
    }
}

impl fmt::Debug for ChatEventStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatEventStream")
            .field("generation_id", &self.generation_id)
            .field("conversation_id", &self.conversation_id)
            .field("branch_id", &self.branch_id)
            .field("assistant_message_id", &self.assistant_message_id)
            .field("last_sequence", &self.last_sequence)
            .field("terminal_seen", &self.terminal_seen)
            .field("has_initial_control", &self.initial_control.is_some())
            .field("reconciliation_required", &self.reconciliation_required)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use crate::ChatEventKindDto;
    use lorepia_core::{
        ChatEvent, ChatEventKind, ConversationBranchId, ConversationId, GenerationId, MessageId,
    };
    use tokio::sync::broadcast;

    use super::{ChatEventStream, ChatStreamItem, GenerationReattachment, ReconcileReason};

    fn reattachment(
        sequence_baseline: u64,
        authoritative_watermark: u64,
        display_prefix: &str,
        reasoning_prefix: &str,
    ) -> GenerationReattachment {
        GenerationReattachment {
            assistant_message_id: "assistant".to_owned(),
            sequence_baseline,
            authoritative_watermark,
            display_prefix: display_prefix.to_owned(),
            reasoning_prefix: reasoning_prefix.to_owned(),
        }
    }

    fn routed_event(sequence: u64, kind: ChatEventKind) -> ChatEvent {
        routed_event_for_assistant(sequence, "assistant", kind)
    }

    fn routed_event_for_assistant(
        sequence: u64,
        assistant_message_id: &str,
        kind: ChatEventKind,
    ) -> ChatEvent {
        ChatEvent::new(
            GenerationId("generation".to_owned()),
            ConversationId("conversation".to_owned()),
            sequence,
            kind,
        )
        .with_route(
            ConversationBranchId("branch".to_owned()),
            MessageId(assistant_message_id.to_owned()),
        )
    }

    #[tokio::test]
    async fn stream_accepts_source_sequence_gaps_from_filtered_events() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );

        sender
            .send(routed_event(1, ChatEventKind::GenerationStarted))
            .expect("send start");
        sender
            .send(routed_event(3, ChatEventKind::TextDelta("missed".into())))
            .expect("send gap");

        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::Event(event) if event.sequence == 1
        ));
        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::Event(event) if event.sequence == 3
        ));
    }

    #[tokio::test]
    async fn reattached_stream_emits_a_live_snapshot_at_an_equal_watermark() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::reattached(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
            reattachment(7, 7, "", ""),
        );
        sender
            .send(routed_event(8, ChatEventKind::TextDelta("next".into())))
            .expect("send next event");

        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::LiveSnapshot
                    && required.last_sequence == Some(7)
                    && required.observed_sequence == Some(7)
                    && required.display_prefix.as_deref() == Some("")
                    && required.reasoning_prefix.as_deref() == Some("")
        ));
        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::Event(event) if event.sequence == 8
        ));
    }

    #[tokio::test]
    async fn reattached_stream_emits_a_live_snapshot_when_the_baseline_is_behind() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::reattached(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
            reattachment(3, 7, "display prefix", "reasoning prefix"),
        );

        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::LiveSnapshot
                    && required.last_sequence == Some(3)
                    && required.observed_sequence == Some(7)
                    && required.display_prefix.as_deref() == Some("display prefix")
                    && required.reasoning_prefix.as_deref() == Some("reasoning prefix")
        ));
        sender
            .send(routed_event(8, ChatEventKind::TextDelta("next".into())))
            .expect("send post-watermark event");
        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::Event(event) if event.sequence == 8
        ));
    }

    #[tokio::test]
    async fn reattached_stream_accepts_a_visible_gap_after_a_live_snapshot() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::reattached(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
            reattachment(1, 1, "", ""),
        );

        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::LiveSnapshot
                    && required.observed_sequence == Some(1)
        ));
        sender
            .send(routed_event(
                7,
                ChatEventKind::TextDelta("transformed display".into()),
            ))
            .expect("send display after filtered provider deltas");
        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::Event(event)
                if event.sequence == 7
                    && matches!(event.kind, ChatEventKindDto::TextDelta(ref text) if text == "transformed display")
        ));
    }

    #[tokio::test]
    async fn broadcast_lag_is_not_forged_into_a_core_event() {
        let (sender, receiver) = broadcast::channel(1);
        let mut stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );

        sender
            .send(routed_event(1, ChatEventKind::GenerationStarted))
            .expect("send first");
        sender
            .send(routed_event(2, ChatEventKind::TextDelta("next".into())))
            .expect("send second");

        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::BroadcastLagged
                    && required.dropped_events == Some(1)
        ));
    }

    #[tokio::test]
    async fn forwards_4096_ordered_events_without_reconciliation() {
        const EVENT_COUNT: u64 = 4_096;
        let (sender, receiver) = broadcast::channel(8_192);
        let mut stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );

        for sequence in 1..=EVENT_COUNT {
            let kind = if sequence == 1 {
                ChatEventKind::GenerationStarted
            } else if sequence == EVENT_COUNT {
                ChatEventKind::GenerationFinished
            } else {
                ChatEventKind::TextDelta("x".into())
            };
            sender
                .send(routed_event(sequence, kind))
                .expect("send ordered event");
        }

        for expected_sequence in 1..=EVENT_COUNT {
            assert!(matches!(
                stream.recv().await,
                ChatStreamItem::Event(event) if event.sequence == expected_sequence
            ));
        }
    }

    #[tokio::test]
    async fn ignores_other_generations_but_reconciles_same_generation_wrong_route() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );
        let wrong_generation = ChatEvent::new(
            GenerationId("other-generation".to_owned()),
            ConversationId("conversation".to_owned()),
            1,
            ChatEventKind::GenerationStarted,
        )
        .with_route(
            ConversationBranchId("branch".to_owned()),
            MessageId("other-assistant".to_owned()),
        );
        sender.send(wrong_generation).expect("send unrelated");
        sender
            .send(routed_event(1, ChatEventKind::GenerationStarted))
            .expect("send expected");

        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::Event(event) if event.generation_id == "generation"
        ));

        let wrong_route = ChatEvent::new(
            GenerationId("generation".to_owned()),
            ConversationId("wrong-conversation".to_owned()),
            2,
            ChatEventKind::TextDelta("wrong".into()),
        )
        .with_route(
            ConversationBranchId("branch".to_owned()),
            MessageId("assistant".to_owned()),
        );
        sender.send(wrong_route).expect("send wrong route");
        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::RouteMismatch
        ));
    }

    #[tokio::test]
    async fn null_assistant_message_id_requires_route_reconciliation() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );
        let mut event = ChatEvent::new(
            GenerationId("generation".to_owned()),
            ConversationId("conversation".to_owned()),
            1,
            ChatEventKind::GenerationStarted,
        );
        event.branch_id = Some(ConversationBranchId("branch".to_owned()));
        sender.send(event).expect("send event without assistant");

        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::RouteMismatch
                    && required.last_sequence == Some(0)
                    && required.observed_sequence == Some(1)
        ));
    }

    #[tokio::test]
    async fn assistant_message_id_is_bound_by_first_event_and_must_remain_stable() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );
        sender
            .send(routed_event(1, ChatEventKind::GenerationStarted))
            .expect("send first route");
        sender
            .send(routed_event_for_assistant(
                2,
                "different-assistant",
                ChatEventKind::TextDelta("wrong route".to_owned()),
            ))
            .expect("send changed assistant route");

        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::Event(event)
                if event.assistant_message_id.as_deref() == Some("assistant")
        ));
        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::RouteMismatch
                    && required.last_sequence == Some(1)
                    && required.observed_sequence == Some(2)
        ));
    }

    #[tokio::test]
    async fn nonzero_sequence_baseline_accepts_strictly_higher_source_sequences() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::new_with_sequence_baseline(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
            7,
        );
        sender
            .send(routed_event(
                8,
                ChatEventKind::TextDelta("resumed".to_owned()),
            ))
            .expect("send event after baseline");
        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::Event(event) if event.sequence == 8
        ));

        let receiver = sender.subscribe();
        let mut replayed_stream = ChatEventStream::new_with_sequence_baseline(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
            7,
        );
        sender
            .send(routed_event(
                7,
                ChatEventKind::TextDelta("replayed".to_owned()),
            ))
            .expect("send replayed event");
        assert!(matches!(
            replayed_stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::DuplicateOrDecreasingSequence
                    && required.last_sequence == Some(7)
                    && required.observed_sequence == Some(7)
        ));

        let receiver = sender.subscribe();
        let mut skipped_stream = ChatEventStream::new_with_sequence_baseline(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
            7,
        );
        sender
            .send(routed_event(
                9,
                ChatEventKind::TextDelta("skipped".to_owned()),
            ))
            .expect("send event after skipped sequence");
        assert!(matches!(
            skipped_stream.recv().await,
            ChatStreamItem::Event(event) if event.sequence == 9
        ));
    }

    #[tokio::test]
    async fn duplicate_and_post_terminal_events_require_reconciliation() {
        let (sender, receiver) = broadcast::channel(8);
        let mut duplicate_stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );
        sender
            .send(routed_event(1, ChatEventKind::GenerationStarted))
            .expect("send start");
        sender
            .send(routed_event(
                1,
                ChatEventKind::TextDelta("duplicate".into()),
            ))
            .expect("send duplicate");
        assert!(matches!(
            duplicate_stream.recv().await,
            ChatStreamItem::Event(_)
        ));
        assert!(matches!(
            duplicate_stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::DuplicateOrDecreasingSequence
        ));

        let receiver = sender.subscribe();
        let mut terminal_stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );
        sender
            .send(routed_event(1, ChatEventKind::GenerationStarted))
            .expect("send second start");
        sender
            .send(routed_event(2, ChatEventKind::GenerationFinished))
            .expect("send terminal");
        sender
            .send(routed_event(3, ChatEventKind::TextDelta("late".into())))
            .expect("send post terminal");
        assert!(matches!(
            terminal_stream.recv().await,
            ChatStreamItem::Event(_)
        ));
        assert!(matches!(
            terminal_stream.recv().await,
            ChatStreamItem::Event(_)
        ));
        assert!(matches!(
            terminal_stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::EventAfterTerminal
        ));
    }

    #[tokio::test]
    async fn unsupported_version_and_closed_receiver_are_explicit() {
        let (sender, receiver) = broadcast::channel(8);
        let mut stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );
        let mut unsupported = routed_event(1, ChatEventKind::GenerationStarted);
        unsupported.event_version = 99;
        sender.send(unsupported).expect("send unsupported event");
        assert!(matches!(
            stream.recv().await,
            ChatStreamItem::ReconciliationRequired(required)
                if required.reason == ReconcileReason::UnsupportedEventVersion
                    && required.supported_event_version == 4
        ));

        let receiver = sender.subscribe();
        let mut closed_stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );
        drop(sender);
        assert_eq!(closed_stream.recv().await, ChatStreamItem::Closed);
    }

    #[test]
    fn dropping_stream_disposes_its_broadcast_receiver() {
        let (sender, receiver) = broadcast::channel(8);
        let stream = ChatEventStream::new(
            receiver,
            "generation".to_owned(),
            "conversation".to_owned(),
            "branch".to_owned(),
        );
        assert_eq!(sender.receiver_count(), 1);
        drop(stream);
        assert_eq!(sender.receiver_count(), 0);
    }
}
