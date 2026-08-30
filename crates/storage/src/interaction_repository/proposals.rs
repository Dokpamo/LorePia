use lorepia_domain::{
    CoreError, CoreResult, InteractionEvent, InteractionProposalRecordId,
    InteractionProposalStatus, InteractionState,
};
use lorepia_orchestration::expire_pending_proposals;
use rusqlite::TransactionBehavior;

use super::event_transactions::{
    InteractionEventTransitionWrite, event_commit_sha256, event_id_or_idempotency_exists,
    stored_event_payload, validate_derived_event_commit, write_event_transition,
};
use super::projections::read_proposal;
use super::proposal_records::{
    mark_proposal_dispatched, require_pending_proposal, transition_proposal_status,
    validate_existing_proposals_unchanged,
};
use super::state::{
    bump_normalized_state_revisions, read_state_by_id, read_state_row, require_state_revision,
    validate_normalized_state, validate_state, write_state_document_only,
};
use super::{
    InteractionEventCommit, InteractionProposalApprovalCommit, InteractionProposalApprovalReceipt,
    InteractionProposalExpiryCommit, InteractionProposalExpiryReceipt,
    InteractionProposalRejectionCommit, InteractionStateKey, MAX_EVENT_JSON_BYTES, Storage,
    StoredInteractionEvent, StoredInteractionProposal, encode_json, interaction_policy_sha256,
    not_found, revision_conflict, storage_corrupted, storage_db_error,
    validate_interaction_policy_revisions,
};

impl Storage {
    /// Rejects one exact pending proposal with proposal and state CAS.
    ///
    /// Decision replays are intentionally errors, even if every byte matches.
    pub fn reject_interaction_proposal(
        &self,
        commit: &InteractionProposalRejectionCommit,
    ) -> CoreResult<StoredInteractionProposal> {
        if commit.decided_at_epoch_seconds < 0 {
            return Err(CoreError::invalid(
                "proposal decision timestamp must be non-negative",
            ));
        }
        validate_state(&commit.decision_state)?;

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let proposal = require_pending_proposal(
            &transaction,
            &commit.proposal_record_id,
            commit.expected_proposal_revision,
            commit.decided_at_epoch_seconds,
        )?;
        let current = read_state_by_id(&transaction, &proposal.interaction_state_id)?
            .ok_or_else(|| storage_corrupted("proposal interaction state is missing"))?;
        validate_normalized_state(&transaction, &current)?;
        require_state_revision(&current, commit.expected_state_revision)?;

        let expected_state = derive_decision_state(
            &current.state,
            &proposal.record.id,
            InteractionProposalStatus::Rejected,
            commit.decided_at_epoch_seconds,
        )?;
        if expected_state != commit.decision_state {
            return Err(CoreError::invalid(
                "proposal rejection state does not match the durable pending proposal",
            ));
        }

        write_state_document_only(
            &transaction,
            &current.id,
            commit.expected_state_revision,
            &commit.decision_state,
            commit.updated_at,
        )?;
        bump_normalized_state_revisions(&transaction, &current.id, commit.decision_state.revision)?;
        transition_proposal_status(
            &transaction,
            &proposal,
            InteractionProposalStatus::Rejected,
            commit.decided_at_epoch_seconds,
            commit.decision_state.revision,
        )?;

        transaction.commit().map_err(storage_db_error)?;
        read_proposal(&connection, &commit.proposal_record_id)?
            .ok_or_else(|| storage_corrupted("rejected interaction proposal is missing"))
    }

    /// Atomically expires every due pending proposal in one room.
    ///
    /// A no-op pass leaves the state revision unchanged. A non-empty pass
    /// advances it exactly once, terminalizes every due proposal, appends
    /// audit rows, and never creates or dispatches a `UserAction`.
    pub fn expire_due_interaction_proposals(
        &self,
        commit: &InteractionProposalExpiryCommit,
    ) -> CoreResult<InteractionProposalExpiryReceipt> {
        if commit.now_epoch_seconds < 0 {
            return Err(CoreError::invalid(
                "proposal expiry timestamp must be non-negative",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_state_row(&transaction, &commit.conversation_id, &commit.branch_id)?
            .ok_or_else(|| not_found("interaction state"))?;
        validate_normalized_state(&transaction, &current)?;
        require_state_revision(&current, commit.expected_state_revision)?;
        let outcome = expire_pending_proposals(
            &current.state,
            commit.expected_state_revision,
            commit.now_epoch_seconds,
        )
        .map_err(|error| CoreError::invalid(format!("cannot expire proposals: {error}")))?;
        if outcome.expired_proposals.is_empty() {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(InteractionProposalExpiryReceipt {
                state: outcome.state,
                expired_proposals: Vec::new(),
            });
        }

        let mut durable_due = Vec::with_capacity(outcome.expired_proposals.len());
        for expired in &outcome.expired_proposals {
            if expired.status != InteractionProposalStatus::Expired
                || expired.decided_at_epoch_seconds != Some(commit.now_epoch_seconds)
            {
                return Err(storage_corrupted(
                    "proposal expiry outcome has an invalid terminal record",
                ));
            }
            let durable = read_proposal(&transaction, &expired.id)?
                .ok_or_else(|| storage_corrupted("due interaction proposal is missing"))?;
            if durable.record.status != InteractionProposalStatus::Pending
                || durable
                    .record
                    .expires_at_epoch_seconds
                    .is_none_or(|expires_at| commit.now_epoch_seconds < expires_at)
            {
                return Err(revision_conflict(
                    "interaction proposal is no longer due and pending",
                ));
            }
            durable_due.push(durable);
        }

        write_state_document_only(
            &transaction,
            &current.id,
            commit.expected_state_revision,
            &outcome.state,
            commit.updated_at,
        )?;
        bump_normalized_state_revisions(&transaction, &current.id, outcome.state.revision)?;
        for proposal in &durable_due {
            transition_proposal_status(
                &transaction,
                proposal,
                InteractionProposalStatus::Expired,
                commit.now_epoch_seconds,
                outcome.state.revision,
            )?;
        }
        transaction.commit().map_err(storage_db_error)?;

        let mut expired_proposals = Vec::with_capacity(durable_due.len());
        for proposal in durable_due {
            expired_proposals.push(
                read_proposal(&connection, &proposal.record.id)?
                    .ok_or_else(|| storage_corrupted("expired interaction proposal is missing"))?,
            );
        }
        Ok(InteractionProposalExpiryReceipt {
            state: outcome.state,
            expired_proposals,
        })
    }

    /// Approves one pending proposal and atomically persists its derived,
    /// storage-controlled `UserAction` outcome when it is non-empty.
    ///
    /// Approval replays are intentionally errors. The derived event has no
    /// caller-controlled event field; its action ID always comes from the
    /// durable proposal record.
    pub fn approve_interaction_proposal(
        &self,
        commit: &InteractionProposalApprovalCommit,
    ) -> CoreResult<InteractionProposalApprovalReceipt> {
        if commit.decided_at_epoch_seconds < 0 {
            return Err(CoreError::invalid(
                "proposal decision timestamp must be non-negative",
            ));
        }
        validate_state(&commit.decision_state)?;

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let proposal = require_pending_proposal(
            &transaction,
            &commit.proposal_record_id,
            commit.expected_proposal_revision,
            commit.decided_at_epoch_seconds,
        )?;
        let current = read_state_by_id(&transaction, &proposal.interaction_state_id)?
            .ok_or_else(|| storage_corrupted("proposal interaction state is missing"))?;
        validate_normalized_state(&transaction, &current)?;
        require_state_revision(&current, commit.expected_state_revision)?;
        validate_interaction_policy_revisions(&transaction, &commit.current_policy)?;
        if proposal.origin_policy != commit.current_policy
            || commit
                .derived
                .as_ref()
                .is_some_and(|derived| derived.policy != commit.current_policy)
        {
            return Err(revision_conflict(
                "interaction proposal policy changed after presentation",
            ));
        }

        let expected_decision_state = derive_decision_state(
            &current.state,
            &proposal.record.id,
            InteractionProposalStatus::Approved,
            commit.decided_at_epoch_seconds,
        )?;
        if expected_decision_state != commit.decision_state {
            return Err(CoreError::invalid(
                "proposal approval state does not match the durable pending proposal",
            ));
        }

        write_state_document_only(
            &transaction,
            &current.id,
            commit.expected_state_revision,
            &commit.decision_state,
            commit.updated_at,
        )?;
        bump_normalized_state_revisions(&transaction, &current.id, commit.decision_state.revision)?;
        let approved = transition_proposal_status(
            &transaction,
            &proposal,
            InteractionProposalStatus::Approved,
            commit.decided_at_epoch_seconds,
            commit.decision_state.revision,
        )?;

        let event = if let Some(derived) = &commit.derived {
            validate_derived_event_commit(&commit.decision_state, derived)?;
            validate_existing_proposals_unchanged(
                &transaction,
                &current.id,
                &commit.decision_state,
                &derived.next_state,
                &derived.proposals,
            )?;
            let key = InteractionStateKey {
                state_id: current.id.clone(),
                conversation_id: current.conversation_id.clone(),
                branch_id: current.branch_id.clone(),
            };
            let event = InteractionEvent::UserAction {
                action_id: proposal.record.proposal_id.clone(),
            };
            let ordinary = InteractionEventCommit {
                event_id: derived.event_id.clone(),
                idempotency_key: derived.idempotency_key.clone(),
                key: key.clone(),
                expected_state_revision: commit.decision_state.revision,
                event: event.clone(),
                generation_attempt_id: None,
                owner_message_id: None,
                policy: derived.policy.clone(),
                evaluation_seal: derived.evaluation_seal.clone(),
                deterministic_seed: derived.deterministic_seed,
                next_state: derived.next_state.clone(),
                knowledge: derived.knowledge.clone(),
                action_results: derived.action_results.clone(),
                effects: derived.effects.clone(),
                derived_events: derived.derived_events.clone(),
                proposals: derived.proposals.clone(),
                created_at: derived.created_at,
            };
            let fingerprint = event_commit_sha256(&ordinary)?;
            let event_payload = stored_event_payload(&ordinary, fingerprint)?;
            let payload_json = encode_json(
                "interaction event payload",
                &event_payload,
                MAX_EVENT_JSON_BYTES,
            )?;
            if event_id_or_idempotency_exists(
                &transaction,
                &derived.event_id,
                &derived.idempotency_key,
            )? {
                return Err(revision_conflict(
                    "proposal approval derived event was already committed",
                ));
            }
            write_event_transition(
                &transaction,
                InteractionEventTransitionWrite {
                    key: &key,
                    expected_state_revision: commit.decision_state.revision,
                    event: &event,
                    generation_attempt_id: None,
                    proposal_namespace_generation_id: None,
                    owner_message_id: None,
                    policy: &derived.policy,
                    evaluation_seal: derived.evaluation_seal.as_ref(),
                    deterministic_seed: derived.deterministic_seed,
                    next_state: &derived.next_state,
                    knowledge: &derived.knowledge,
                    action_results: &derived.action_results,
                    effects: &derived.effects,
                    derived_events: &derived.derived_events,
                    proposals: &derived.proposals,
                    event_id: &derived.event_id,
                    idempotency_key: &derived.idempotency_key,
                    payload_json: &payload_json,
                    created_at: derived.created_at,
                    generation_append_materialization: false,
                    derived_chain_parent: None,
                },
            )?;
            Some(StoredInteractionEvent {
                event_id: derived.event_id.clone(),
                idempotency_key: derived.idempotency_key.clone(),
                interaction_state_id: current.id.clone(),
                expected_state_revision: commit.decision_state.revision,
                resulting_state_revision: derived.next_state.revision,
                exact_replay: false,
                generation_attempt_id: None,
                owner_message_id: None,
                commit_sha256: event_payload.commit_sha256,
                resulting_state_snapshot_sha256: event_payload.resulting_state_snapshot_sha256,
                proposal_review_sha256s: event_payload.proposal_review_sha256s,
                policy: derived.policy.clone(),
                policy_sha256: interaction_policy_sha256(&derived.policy)?,
                created_at: derived.created_at,
            })
        } else {
            None
        };

        let final_state_revision = event
            .as_ref()
            .map_or(commit.decision_state.revision, |event| {
                event.resulting_state_revision
            });
        let dispatched = mark_proposal_dispatched(
            &transaction,
            &approved,
            commit.decided_at_epoch_seconds,
            final_state_revision,
        )?;
        transaction.commit().map_err(storage_db_error)?;

        Ok(InteractionProposalApprovalReceipt {
            proposal: dispatched,
            event,
            resulting_state_revision: final_state_revision,
        })
    }
}

pub(super) fn derive_decision_state(
    current: &InteractionState,
    proposal_record_id: &InteractionProposalRecordId,
    status: InteractionProposalStatus,
    decided_at_epoch_seconds: i64,
) -> CoreResult<InteractionState> {
    let mut next = current.clone();
    let proposal = next
        .proposals
        .iter_mut()
        .find(|proposal| &proposal.id == proposal_record_id)
        .ok_or_else(|| {
            storage_corrupted("durable proposal row is absent from interaction state document")
        })?;
    if proposal.status != InteractionProposalStatus::Pending {
        return Err(revision_conflict(
            "interaction state proposal is no longer pending",
        ));
    }
    proposal.status = status;
    proposal.decided_at_epoch_seconds = Some(decided_at_epoch_seconds);
    next.revision = current
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("interaction state revision overflowed"))?;
    validate_state(&next)?;
    Ok(next)
}
