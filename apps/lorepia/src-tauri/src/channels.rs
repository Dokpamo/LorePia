use lorepia_shell_api::{ChatEventKindDto, ChatEventStream, ChatStreamItem, ReconcileReason};
use tauri::ipc::Channel;

use crate::state::ChatStreamRegistration;

pub fn forward_chat_stream(
    mut stream: ChatEventStream,
    channel: Channel<ChatStreamItem>,
    mut registration: ChatStreamRegistration,
) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = registration.disposed() => {
                    // Receiver disposal never implies Core generation cancellation.
                    break;
                }
                item = stream.recv() => {
                    let should_close = closes_forwarder(&item);
                    if channel.send(item).is_err() || should_close {
                        // Closing a renderer Channel stops only this forwarding task.
                        // Generation cancellation requires the explicit cancel command.
                        break;
                    }
                }
            }
        }
    });
}

fn closes_forwarder(item: &ChatStreamItem) -> bool {
    match item {
        ChatStreamItem::Event(event) => matches!(
            event.kind,
            ChatEventKindDto::GenerationCancelled
                | ChatEventKindDto::GenerationFailed { .. }
                | ChatEventKindDto::GenerationFinished
        ),
        ChatStreamItem::ReconciliationRequired(required) => {
            required.reason != ReconcileReason::LiveSnapshot
                || required.dropped_events.is_some()
                || required.display_prefix.is_none()
                || required.reasoning_prefix.is_none()
                || !matches!(
                    (required.last_sequence, required.observed_sequence),
                    (Some(last), Some(observed)) if observed >= last
                )
        }
        ChatStreamItem::Closed => true,
    }
}

#[cfg(test)]
mod tests {
    use lorepia_shell_api::{ChatStreamItem, ReconcileReason, ReconciliationRequiredDto};

    use super::closes_forwarder;

    fn reconciliation(reason: ReconcileReason) -> ReconciliationRequiredDto {
        ReconciliationRequiredDto {
            reason,
            generation_id: "generation".to_owned(),
            conversation_id: "conversation".to_owned(),
            branch_id: "branch".to_owned(),
            last_sequence: Some(3),
            observed_sequence: Some(7),
            dropped_events: None,
            supported_event_version: 4,
            display_prefix: None,
            reasoning_prefix: None,
        }
    }

    #[test]
    fn atomic_catchup_marker_keeps_the_forwarder_alive() {
        let mut snapshot = reconciliation(ReconcileReason::LiveSnapshot);
        snapshot.display_prefix = Some("display".to_owned());
        snapshot.reasoning_prefix = Some("reasoning".to_owned());
        let catchup = ChatStreamItem::ReconciliationRequired(snapshot);
        assert!(!closes_forwarder(&catchup));
    }

    #[test]
    fn lossy_or_fatal_reconciliation_still_closes_the_forwarder() {
        let mut lagged = reconciliation(ReconcileReason::BroadcastLagged);
        lagged.dropped_events = Some(4);
        let lagged = ChatStreamItem::ReconciliationRequired(lagged);
        let route_mismatch =
            ChatStreamItem::ReconciliationRequired(reconciliation(ReconcileReason::RouteMismatch));
        let sequence_gap =
            ChatStreamItem::ReconciliationRequired(reconciliation(ReconcileReason::SequenceGap));
        assert!(closes_forwarder(&lagged));
        assert!(closes_forwarder(&route_mismatch));
        assert!(closes_forwarder(&sequence_gap));
    }
}
