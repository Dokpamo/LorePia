use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use rusqlite::{TransactionBehavior, params};

use crate::Storage;

use super::super::event_transactions::{
    InteractionEventTransitionWrite, event_commit_sha256, stored_event_payload,
    validate_event_commit, write_event_transition,
};
use super::super::projections::read_event_by_occurrence;
use super::super::proposal_records::validate_existing_proposals_unchanged;
use super::super::state::{require_state_for_key, require_state_revision, validate_nonempty_id};
use super::super::types::{
    InteractionEventCommit, InteractionEventOccurrenceLookup, MAX_AUDIT_JSON_BYTES,
    MAX_EVENT_JSON_BYTES, StoredInteractionEvent,
};
use super::super::{
    DerivedChainParent, encode_json, i64_from_u64, interaction_policy_sha256, not_found,
    revision_conflict, sha256_hex, storage_corrupted, storage_db_error,
};
use super::quarantine::read_derived_event_quarantine;
use super::row_mapping::{decode_derived_outbox_row, read_derived_outbox_row};
use super::{InteractionDerivedOccurrenceCommit, StoredInteractionDerivedEvent};

impl Storage {
    /// Atomically commits a claimed derived event, enqueues its children, and
    /// acknowledges the source occurrence. Repeating the exact commit after a
    /// lost response returns the already committed event as an exact replay.
    pub fn commit_interaction_derived_occurrence(
        &self,
        commit: &InteractionDerivedOccurrenceCommit,
    ) -> CoreResult<StoredInteractionEvent> {
        validate_nonempty_id("derived interaction occurrence", &commit.occurrence_id)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let raw = read_derived_outbox_row(&transaction, &commit.occurrence_id)?
            .ok_or_else(|| not_found("derived interaction occurrence"))?;
        if read_derived_event_quarantine(&transaction, &commit.occurrence_id)?.is_some() {
            return Err(revision_conflict(
                "derived interaction occurrence is terminally quarantined",
            ));
        }
        let status = raw.status.clone();
        let occurrence = decode_derived_outbox_row(&transaction, raw)?;
        let (event_id, idempotency_key) = derived_occurrence_event_identity(&occurrence)?;
        if occurrence.delivery_attempts != commit.expected_delivery_attempts {
            return Err(revision_conflict(
                "derived interaction occurrence delivery token is stale",
            ));
        }
        if commit.key.conversation_id != occurrence.conversation_id
            || commit.key.branch_id != occurrence.branch_id
        {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "derived interaction occurrence was not found in this branch",
                false,
            ));
        }
        let ordinary = InteractionEventCommit {
            event_id: event_id.clone(),
            idempotency_key: idempotency_key.clone(),
            key: commit.key.clone(),
            expected_state_revision: commit.expected_state_revision,
            event: occurrence.event.clone(),
            generation_attempt_id: None,
            owner_message_id: None,
            policy: occurrence.policy.clone(),
            evaluation_seal: Some(occurrence.evaluation_seal.clone()),
            deterministic_seed: Some(occurrence.deterministic_seed),
            next_state: commit.next_state.clone(),
            knowledge: commit.knowledge.clone(),
            action_results: commit.action_results.clone(),
            effects: commit.effects.clone(),
            derived_events: commit.derived_events.clone(),
            proposals: commit.proposals.clone(),
            created_at: occurrence.occurred_at,
        };
        validate_event_commit(&ordinary)?;
        let fingerprint = event_commit_sha256(&ordinary)?;
        let event_payload = stored_event_payload(&ordinary, fingerprint)?;
        if status == "acknowledged" {
            let replay = read_event_by_occurrence(
                &transaction,
                &InteractionEventOccurrenceLookup {
                    event_id,
                    idempotency_key,
                    conversation_id: occurrence.conversation_id,
                    branch_id: occurrence.branch_id,
                    event: occurrence.event,
                    generation_attempt_id: None,
                    owner_message_id: None,
                    occurred_at: occurrence.occurred_at,
                },
            )?
            .ok_or_else(|| {
                storage_corrupted("acknowledged derived occurrence has no committed event")
            })?;
            if replay.interaction_state_id != ordinary.key.state_id
                || replay.expected_state_revision != ordinary.expected_state_revision
                || replay.resulting_state_revision != ordinary.next_state.revision
                || replay.commit_sha256 != event_payload.commit_sha256
            {
                return Err(revision_conflict(
                    "derived interaction exact replay materialization changed",
                ));
            }
            transaction.commit().map_err(storage_db_error)?;
            return Ok(StoredInteractionEvent {
                exact_replay: true,
                ..replay
            });
        }
        if status != "claimed" {
            return Err(revision_conflict(
                "derived interaction occurrence delivery token is stale",
            ));
        }
        let current = require_state_for_key(&transaction, &commit.key)?;
        require_state_revision(&current, commit.expected_state_revision)?;
        validate_existing_proposals_unchanged(
            &transaction,
            &current.id,
            &current.state,
            &commit.next_state,
            &commit.proposals,
        )?;
        let payload_json = encode_json(
            "derived interaction event payload",
            &event_payload,
            MAX_EVENT_JSON_BYTES,
        )?;
        write_event_transition(
            &transaction,
            InteractionEventTransitionWrite {
                key: &ordinary.key,
                expected_state_revision: ordinary.expected_state_revision,
                event: &ordinary.event,
                generation_attempt_id: None,
                proposal_namespace_generation_id: None,
                owner_message_id: None,
                policy: &ordinary.policy,
                evaluation_seal: ordinary.evaluation_seal.as_ref(),
                deterministic_seed: ordinary.deterministic_seed,
                next_state: &ordinary.next_state,
                knowledge: &ordinary.knowledge,
                action_results: &ordinary.action_results,
                effects: &ordinary.effects,
                derived_events: &ordinary.derived_events,
                proposals: &ordinary.proposals,
                event_id: &ordinary.event_id,
                idempotency_key: &ordinary.idempotency_key,
                payload_json: &payload_json,
                created_at: ordinary.created_at,
                generation_append_materialization: false,
                derived_chain_parent: Some(DerivedChainParent {
                    occurrence: &occurrence,
                }),
            },
        )?;
        let changed = transaction
            .execute(
                "UPDATE interaction_derived_event_outbox
                 SET status = 'acknowledged', lease_until = NULL,
                     acknowledged_at = ?3
                 WHERE occurrence_id = ?1 AND status = 'claimed'
                   AND delivery_attempts = ?2",
                params![
                    commit.occurrence_id,
                    i64_from_u64(
                        "derived interaction delivery attempts",
                        commit.expected_delivery_attempts,
                    )?,
                    commit.committed_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(revision_conflict(
                "derived interaction occurrence acknowledgement raced",
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(StoredInteractionEvent {
            event_id,
            idempotency_key,
            interaction_state_id: commit.key.state_id.clone(),
            expected_state_revision: commit.expected_state_revision,
            resulting_state_revision: commit.next_state.revision,
            exact_replay: false,
            generation_attempt_id: None,
            owner_message_id: None,
            commit_sha256: event_payload.commit_sha256,
            resulting_state_snapshot_sha256: event_payload.resulting_state_snapshot_sha256,
            proposal_review_sha256s: event_payload.proposal_review_sha256s,
            policy: occurrence.policy.clone(),
            policy_sha256: interaction_policy_sha256(&occurrence.policy)?,
            created_at: occurrence.occurred_at,
        })
    }
}

fn derived_occurrence_event_identity(
    occurrence: &StoredInteractionDerivedEvent,
) -> CoreResult<(String, String)> {
    let digest = sha256_hex(
        encode_json(
            "derived interaction materialization identity",
            &(
                "lorepia.interaction-derived-materialization.v1",
                occurrence.occurrence_id.as_str(),
                &occurrence.event_sha256,
                occurrence.chain_ordinal,
            ),
            MAX_AUDIT_JSON_BYTES,
        )?
        .as_bytes(),
    );
    Ok((
        format!("interaction-event-{digest}"),
        format!("interaction-derived-event:v1:{digest}"),
    ))
}
