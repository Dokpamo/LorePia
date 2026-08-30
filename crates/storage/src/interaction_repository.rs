//! Durable interaction state, event, effect, and approval persistence.
//!
//! The repository deliberately accepts already-evaluated domain outcomes, but
//! it does not accept an event when approving a proposal. Approval dispatch is
//! always derived from the exact durable proposal record, which prevents a
//! caller from substituting an arbitrary user action at the persistence seam.

#![allow(clippy::too_many_lines)]

mod checkpoints;
mod effect_history;
mod effects;
mod event_transactions;
mod generation_proposal_persistence;
mod generation_proposal_queries;
mod generation_proposals;
mod generation_review;
mod generation_review_authority;
mod projections;
mod proposal_records;
mod proposals;
mod state;
mod types;

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    InteractionAction, InteractionEffect, InteractionEvent, InteractionProposalRecord,
    InteractionProposalRecordId, InteractionProposalStatus, InteractionRuleId, InteractionState,
    ResolvedPromptPlan, Sha256Digest, ValidateOrchestration, prompt_local_user_id_sha256,
};
#[cfg(test)]
use lorepia_domain::{KnowledgeEntryId, VariableValue, VersionedJson};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, ModuleMergeReview, no_applied_module_runtime_plan_sha256,
};
#[cfg(test)]
use lorepia_orchestration::{approve_pending, expire_pending_proposal, reject_pending};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    GenerationAttemptDerivedClosure, InteractionEvaluationSeal, MemoryRecordsAtHeadSnapshot,
    Storage, StoredGenerationAttempt, database::storage_db_error,
    generation_attempt_derived_closure_sha256, interaction_evaluation_seal_sha256,
    memory_records_at_head_snapshot_sha256,
};

use effect_history::{
    decode_choice_effect_lifecycle, effect_outbox_kind, interaction_effect_id, read_effect_history,
    read_effect_history_page, read_latest_region_effects, read_older_reopen_effect_history,
    read_pending_choice_effect_history, read_pending_effects, read_recent_reopen_effect_history,
    require_pending_choice, require_pending_choice_effect, validate_effect_delivery_token,
    validate_effect_poll_limit, validate_proposal_list_limit, validate_stored_effect_identity,
};
use effects::validate_stored_interaction_policy_rule_sets;
use event_transactions::{
    InteractionEventTransitionWrite, StoredEventPayload, decode_stored_event_payload,
    encode_interaction_evaluation_authority, encode_interaction_policy, event_commit_sha256,
    event_id_or_idempotency_exists, event_requires_argument, interaction_event_argument_json,
    interaction_event_kind, stored_event_payload, stored_module_plan_sha256,
    validate_derived_event_commit, validate_event_collections, validate_event_commit,
    validate_event_generation_attempt_shape, validate_event_owner_message_shape,
    validate_policy_shape, write_event_transition,
};
pub(crate) use generation_proposal_queries::read_generation_attempt_interaction_aggregate;
use generation_proposals::{
    GenerationAttemptProposalDecisionMaterialization, generation_attempt_decision_evidence,
    generation_attempt_decision_materialization,
    validate_generation_attempt_proposal_decision_commit,
};
#[cfg(test)]
use generation_review::{
    prepare_generation_attempt_before_review, write_generation_attempt_before_review,
};
use generation_review_authority::{
    generation_attempt_before_review_storage_sha256, generation_attempt_proposal_storage_id,
    read_generation_attempt_before_review,
};
pub(crate) use proposal_records::validate_generation_attempt_identity_migration_legacy_rows;
use proposal_records::{
    interaction_proposal_record_id, mark_proposal_dispatched, proposal_status,
    proposal_status_wire, require_pending_proposal, transition_proposal_status,
    validate_existing_proposals_unchanged, validate_proposal_writes, write_new_proposals,
};
use proposals::derive_decision_state;

pub(crate) use checkpoints::clone_interaction_checkpoint_for_branch_transaction;
use checkpoints::write_interaction_state_checkpoint;
use projections::{
    decode_interaction_policy, decode_stored_interaction_event, read_event_by_occurrence,
    read_proposal, validate_stored_event_checkpoint_evidence,
    validate_stored_event_evaluation_authority, validate_stored_event_proposal_evidence,
};
use state::{
    bump_normalized_state_revisions, read_knowledge_bindings, read_state_by_id, read_state_row,
    replace_normalized_state, require_state_for_key, require_state_revision, validate_key,
    validate_knowledge_bindings, validate_nonempty_id, validate_normalized_state, validate_state,
    write_state_document_only,
};
pub(crate) use types::{
    ClonedInteractionCheckpoint, GenerationAttemptInteractionMaterializationReceipt,
};
pub use types::{
    GenerationAttemptBeforeReviewCommit, GenerationAttemptProposalDecision,
    GenerationAttemptProposalDecisionCommit, GenerationAttemptProposalDecisionReceipt,
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionChoiceEffectStatus,
    InteractionChoiceExpirationCommit, InteractionChoiceSelectionCommit,
    InteractionChoiceSelectionReceipt, InteractionDerivedEventCommit,
    InteractionDerivedEventSupervisorStatus, InteractionDerivedEventWrite,
    InteractionDerivedOccurrenceCommit, InteractionEffectHistoryCursor, InteractionEventCommit,
    InteractionEventOccurrenceLookup, InteractionKnowledgeBinding,
    InteractionPolicyRuleSetRevision, InteractionPolicySnapshot, InteractionProposalApprovalCommit,
    InteractionProposalApprovalReceipt, InteractionProposalExpiryCommit,
    InteractionProposalExpiryReceipt, InteractionProposalRejectionCommit, InteractionProposalWrite,
    InteractionStateKey, MAX_INTERACTION_DERIVED_CHAIN_DEPTH, MAX_INTERACTION_DERIVED_CHAIN_EVENTS,
    StoredGenerationAttemptBeforeReview, StoredGenerationAttemptInteractionAggregate,
    StoredGenerationAttemptInteractionBoundary, StoredGenerationAttemptProposal,
    StoredInteractionDerivedEvent, StoredInteractionDerivedEventQuarantine,
    StoredInteractionEffect, StoredInteractionEffectHistory, StoredInteractionEvent,
    StoredInteractionProposal, StoredInteractionState, StoredInteractionStateCheckpoint,
    interaction_action_sha256, interaction_policy_sha256, interaction_proposal_review_sha256,
    interaction_state_key_for_branch, interaction_state_snapshot_sha256,
};
use types::{
    MAX_ACTION_RESULTS_PER_EVENT, MAX_AUDIT_JSON_BYTES, MAX_EFFECTS_PER_EVENT,
    MAX_EVENT_JSON_BYTES, MAX_INTERACTION_DERIVED_CLAIM, MAX_JSON_DEPTH, MAX_JSON_NODES,
    MAX_STATE_JSON_BYTES, interaction_event_sha256,
};

#[derive(Clone, Copy)]
struct DerivedChainParent<'a> {
    occurrence: &'a StoredInteractionDerivedEvent,
}

#[derive(Debug)]
struct GenerationAttemptAppendSnapshot {
    event_id: String,
    event_sha256: Sha256Digest,
    occurred_at: DateTime<Utc>,
    context_checkpoint_sha256: Sha256Digest,
    previous_state: InteractionState,
    previous_knowledge: Vec<InteractionKnowledgeBinding>,
    module_runtime_review: ModuleMergeReview,
    memory_head_snapshot: MemoryRecordsAtHeadSnapshot,
    source_runtime_plan_sha256: Option<Sha256Digest>,
    source_activation_plan_sha256: Option<Sha256Digest>,
    applied_runtime_plan: Option<AppliedModuleRuntimePlan>,
    policy: InteractionPolicySnapshot,
    next_state: InteractionState,
    knowledge: Vec<InteractionKnowledgeBinding>,
    action_results: Vec<InteractionActionResultWrite>,
    effects: Vec<InteractionEffect>,
    derived_events: Vec<InteractionDerivedEventWrite>,
    review_sha256: Sha256Digest,
}

#[derive(Debug)]
struct GenerationAttemptAppendDecision {
    proposal_record_id: InteractionProposalRecordId,
    expected_proposal_revision: u64,
    decision_event_id: Option<String>,
    decision_event_sha256: Option<Sha256Digest>,
    decided_at_epoch_seconds: i64,
    updated_at: DateTime<Utc>,
    materialization: GenerationAttemptProposalDecisionMaterialization,
}

#[derive(Debug)]
struct RawGenerationAttemptAppendSnapshot {
    event_id: String,
    event_sha256: String,
    occurred_at: String,
    context_checkpoint_sha256: String,
    previous_state_revision: i64,
    previous_state_json: String,
    previous_state_document_sha256: String,
    previous_state_snapshot_sha256: String,
    previous_knowledge_json: String,
    previous_knowledge_sha256: String,
    applied_runtime_plan_sha256: String,
    module_runtime_review_json: String,
    module_runtime_review_sha256: String,
    memory_head_snapshot_json: String,
    memory_head_snapshot_sha256: String,
    source_runtime_plan_sha256: Option<String>,
    source_activation_plan_sha256: Option<String>,
    applied_runtime_plan_json: Option<String>,
    policy_json: String,
    policy_sha256: String,
    next_state_json: String,
    next_state_document_sha256: String,
    next_state_snapshot_sha256: String,
    knowledge_json: String,
    knowledge_sha256: String,
    action_results_json: String,
    action_results_sha256: String,
    effects_json: String,
    effects_sha256: String,
    derived_events_json: String,
    derived_events_sha256: String,
    proposal_writes_json: String,
    proposal_writes_sha256: String,
    review_sha256: String,
    domain_review_sha256: String,
    storage_identity_version: i64,
}

#[derive(Debug)]
struct RawGenerationAttemptAppendDecision {
    proposal_record_id: String,
    status: String,
    proposal_revision: i64,
    decision_idempotency_key: String,
    decision_event_id: Option<String>,
    decision_event_sha256: Option<String>,
    decision_evidence_json: String,
    decision_evidence_sha256: String,
    resulting_aggregate_revision: i64,
    materialization_json: String,
    materialization_sha256: String,
    decided_at_epoch_seconds: i64,
    updated_at: String,
}

impl Storage {
    /// Resets every abandoned derived-event claim while `Storage::open` still
    /// holds the process-exclusive data-root lock.
    pub(crate) fn recover_all_interaction_derived_event_leases(
        &self,
        available_at: DateTime<Utc>,
    ) -> CoreResult<u64> {
        let changed = self
            .connection()?
            .execute(
                "UPDATE interaction_derived_event_outbox
                 SET status = 'pending', lease_until = NULL, available_at = ?1
                 WHERE status = 'claimed'
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id =
                             interaction_derived_event_outbox.occurrence_id
                   )",
                [available_at.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        u64::try_from(changed)
            .map_err(|_| CoreError::internal("derived-event recovery count overflowed"))
    }

    pub fn interaction_derived_event_supervisor_status(
        &self,
    ) -> CoreResult<InteractionDerivedEventSupervisorStatus> {
        let connection = self.connection()?;
        let (pending_count, next_available_at) = connection
            .query_row(
                "WITH live AS (
                     SELECT occurrence.*
                     FROM interaction_derived_event_outbox AS occurrence
                     WHERE occurrence.status != 'acknowledged'
                       AND NOT EXISTS (
                           SELECT 1
                           FROM interaction_derived_event_quarantines AS quarantine
                           WHERE quarantine.occurrence_id = occurrence.occurrence_id
                       )
                 ), branch_heads AS (
                     SELECT candidate.*
                     FROM live AS candidate
                     WHERE NOT EXISTS (
                         SELECT 1
                         FROM live AS predecessor
                         WHERE predecessor.conversation_id = candidate.conversation_id
                           AND predecessor.branch_id = candidate.branch_id
                           AND (
                               predecessor.parent_resulting_state_revision
                                   < candidate.parent_resulting_state_revision
                               OR (
                                   predecessor.parent_resulting_state_revision
                                       = candidate.parent_resulting_state_revision
                                   AND predecessor.chain_id < candidate.chain_id
                               )
                               OR (
                                   predecessor.parent_resulting_state_revision
                                       = candidate.parent_resulting_state_revision
                                   AND predecessor.chain_id = candidate.chain_id
                                   AND predecessor.chain_ordinal < candidate.chain_ordinal
                               )
                           )
                     )
                 )
                 SELECT (SELECT COUNT(*) FROM live),
                        (SELECT MIN(CASE
                            WHEN branch_heads.status = 'pending'
                                THEN branch_heads.available_at
                            ELSE branch_heads.lease_until
                        END) FROM branch_heads)",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .map_err(storage_db_error)?;
        Ok(InteractionDerivedEventSupervisorStatus {
            pending_count: u64_from_i64("pending derived interaction count", pending_count)?,
            next_available_at: next_available_at
                .as_deref()
                .map(|value| parse_datetime("next derived interaction availability", value))
                .transpose()?,
        })
    }

    /// Claims the earliest derived events without allowing two transitions on
    /// the same branch to race. Expired leases are reclaimed at least once;
    /// exact event/idempotency identities make materialization idempotent.
    pub fn claim_interaction_derived_events(
        &self,
        now: DateTime<Utc>,
        lease_until: DateTime<Utc>,
        limit: u32,
    ) -> CoreResult<Vec<StoredInteractionDerivedEvent>> {
        if limit == 0 || limit > MAX_INTERACTION_DERIVED_CLAIM {
            return Err(CoreError::invalid(
                "derived interaction claim limit must be between 1 and 64",
            ));
        }
        if lease_until <= now {
            return Err(CoreError::invalid(
                "derived interaction lease must end after its claim time",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        quarantine_legacy_derived_outbox_rows(&transaction, now, limit)?;
        let ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT candidate.occurrence_id
                     FROM interaction_derived_event_outbox AS candidate
                     WHERE (
                         (candidate.status = 'pending' AND candidate.available_at <= ?1)
                         OR (candidate.status = 'claimed' AND candidate.lease_until <= ?1)
                     )
                     AND NOT EXISTS (
                         SELECT 1
                         FROM interaction_derived_event_quarantines AS quarantine
                         WHERE quarantine.occurrence_id = candidate.occurrence_id
                     )
                     AND NOT EXISTS (
                         SELECT 1
                         FROM interaction_derived_event_outbox AS predecessor
                         WHERE predecessor.conversation_id = candidate.conversation_id
                           AND predecessor.branch_id = candidate.branch_id
                           AND predecessor.status != 'acknowledged'
                           AND NOT EXISTS (
                               SELECT 1
                               FROM interaction_derived_event_quarantines AS quarantine
                               WHERE quarantine.occurrence_id = predecessor.occurrence_id
                           )
                           AND (
                               predecessor.parent_resulting_state_revision
                                   < candidate.parent_resulting_state_revision
                               OR (
                                   predecessor.parent_resulting_state_revision
                                       = candidate.parent_resulting_state_revision
                                   AND predecessor.chain_id < candidate.chain_id
                               )
                               OR (
                                   predecessor.parent_resulting_state_revision
                                       = candidate.parent_resulting_state_revision
                                   AND predecessor.chain_id = candidate.chain_id
                                   AND predecessor.chain_ordinal < candidate.chain_ordinal
                               )
                           )
                     )
                     ORDER BY candidate.parent_resulting_state_revision,
                              candidate.chain_id,
                              candidate.chain_ordinal
                     LIMIT ?2",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(params![now.to_rfc3339(), i64::from(limit)], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        let mut claimed = Vec::with_capacity(ids.len());
        for occurrence_id in ids {
            let changed = transaction
                .execute(
                    "UPDATE interaction_derived_event_outbox
                     SET status = 'claimed', delivery_attempts = delivery_attempts + 1,
                         lease_until = ?2, available_at = ?3
                     WHERE occurrence_id = ?1
                       AND ((status = 'pending' AND available_at <= ?3)
                            OR (status = 'claimed' AND lease_until <= ?3))",
                    params![occurrence_id, lease_until.to_rfc3339(), now.to_rfc3339()],
                )
                .map_err(storage_db_error)?;
            if changed != 1 {
                return Err(storage_corrupted(
                    "derived interaction occurrence changed during claim",
                ));
            }
            let row = read_derived_outbox_row(&transaction, &occurrence_id)?
                .ok_or_else(|| storage_corrupted("claimed derived occurrence disappeared"))?;
            claimed.push(decode_claimed_derived_outbox_row(&transaction, row)?);
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(claimed)
    }

    /// Defers one failed derived occurrence under its exact delivery token.
    pub fn retry_interaction_derived_event_after(
        &self,
        occurrence_id: &str,
        expected_delivery_attempts: u64,
        available_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        validate_nonempty_id("derived interaction occurrence", occurrence_id)?;
        let changed = self
            .connection()?
            .execute(
                "UPDATE interaction_derived_event_outbox
                 SET status = 'pending', lease_until = NULL, available_at = ?3
                 WHERE occurrence_id = ?1 AND status = 'claimed'
                   AND delivery_attempts = ?2
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id =
                             interaction_derived_event_outbox.occurrence_id
                   )",
                params![
                    occurrence_id,
                    i64_from_u64(
                        "derived interaction delivery attempts",
                        expected_delivery_attempts
                    )?,
                    available_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(revision_conflict(
                "derived interaction occurrence delivery token is stale",
            ))
        }
    }

    /// Atomically records a terminal, non-successful outcome when Core cannot
    /// reconstruct the occurrence's sealed evaluation authority. Repeating
    /// the same evidence after response loss returns an exact replay.
    pub fn quarantine_interaction_derived_event_authority_failure(
        &self,
        occurrence_id: &str,
        expected_delivery_attempts: u64,
        active_policy: Option<&InteractionPolicySnapshot>,
        quarantined_at: DateTime<Utc>,
    ) -> CoreResult<StoredInteractionDerivedEventQuarantine> {
        validate_nonempty_id("derived interaction occurrence", occurrence_id)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let raw = read_derived_outbox_row(&transaction, occurrence_id)?
            .ok_or_else(|| not_found("derived interaction occurrence"))?;
        let status = raw.status.clone();
        let occurrence = decode_derived_outbox_row(&transaction, raw)?;
        let active_policy_sha256 = active_policy
            .map(interaction_policy_sha256)
            .transpose()?
            .map(Sha256Digest::parse)
            .transpose()
            .map_err(CoreError::invalid)?;
        let evidence = DerivedQuarantineEvidence {
            schema_version: 1,
            occurrence_id,
            delivery_attempts: expected_delivery_attempts,
            sealed_policy_sha256: &occurrence.policy_sha256,
            active_policy_sha256: active_policy_sha256.as_ref(),
            source_effect_sha256: &occurrence.source_effect_sha256,
            source_action_sha256: &occurrence.source_action_sha256,
            reason_kind: "sealed_policy_recovery_failed",
        };
        let evidence_json = encode_json(
            "derived interaction quarantine evidence",
            &evidence,
            MAX_AUDIT_JSON_BYTES,
        )?;
        let evidence_sha256 = Sha256Digest::parse(sha256_hex(evidence_json.as_bytes()))
            .map_err(CoreError::invalid)?;
        if let Some(stored) = read_derived_event_quarantine(&transaction, occurrence_id)? {
            if stored.delivery_attempts != expected_delivery_attempts
                || stored.sealed_policy_sha256 != occurrence.policy_sha256
                || stored.active_policy_sha256 != active_policy_sha256
                || stored.source_effect_sha256 != occurrence.source_effect_sha256
                || stored.source_action_sha256 != occurrence.source_action_sha256
                || stored.evidence_sha256 != evidence_sha256
            {
                return Err(revision_conflict(
                    "derived interaction quarantine evidence changed",
                ));
            }
            transaction.commit().map_err(storage_db_error)?;
            return Ok(StoredInteractionDerivedEventQuarantine {
                exact_replay: true,
                ..stored
            });
        }
        if status != "claimed" || occurrence.delivery_attempts != expected_delivery_attempts {
            return Err(revision_conflict(
                "derived interaction occurrence delivery token is stale",
            ));
        }
        transaction
            .execute(
                "INSERT INTO interaction_derived_event_quarantines
                 (occurrence_id, reason_kind, delivery_attempts,
                  sealed_policy_sha256, active_policy_sha256,
                  source_effect_sha256, source_action_sha256,
                  evidence_json, evidence_sha256, quarantined_at)
                 VALUES (?1, 'sealed_policy_recovery_failed', ?2, ?3, ?4,
                         ?5, ?6, ?7, ?8, ?9)",
                params![
                    occurrence_id,
                    i64_from_u64(
                        "derived interaction delivery attempts",
                        expected_delivery_attempts,
                    )?,
                    occurrence.policy_sha256.as_str(),
                    active_policy_sha256.as_ref().map(Sha256Digest::as_str),
                    occurrence.source_effect_sha256.as_str(),
                    occurrence.source_action_sha256.as_str(),
                    evidence_json,
                    evidence_sha256.as_str(),
                    quarantined_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(StoredInteractionDerivedEventQuarantine {
            occurrence_id: occurrence_id.to_owned(),
            delivery_attempts: expected_delivery_attempts,
            sealed_policy_sha256: occurrence.policy_sha256,
            active_policy_sha256,
            source_effect_sha256: occurrence.source_effect_sha256,
            source_action_sha256: occurrence.source_action_sha256,
            evidence_sha256,
            quarantined_at,
            exact_replay: false,
        })
    }

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

#[derive(Debug)]
struct RawDerivedOutboxRow {
    occurrence_id: String,
    chain_id: String,
    root_event_id: String,
    parent_event_id: String,
    parent_occurrence_id: Option<String>,
    conversation_id: String,
    branch_id: String,
    depth: i64,
    chain_ordinal: i64,
    source_effect_ordinal: i64,
    parent_event_commit_sha256: String,
    parent_resulting_state_revision: i64,
    source_effect_sha256: String,
    source_action_sha256: String,
    source_set_revision_id: String,
    source_rule_id: String,
    source_action_ordinal: i64,
    event_kind: String,
    event_argument_json: String,
    event_sha256: String,
    visited_event_sha256s_json: String,
    policy_json: String,
    policy_sha256: String,
    evaluation_seal_json: Option<String>,
    evaluation_seal_sha256: Option<String>,
    evaluation_seal_version: i64,
    deterministic_seed_hex: Option<String>,
    occurred_at: String,
    available_at: String,
    status: String,
    delivery_attempts: i64,
    lease_until: Option<String>,
}

#[derive(Serialize)]
struct DerivedQuarantineEvidence<'a> {
    schema_version: u32,
    occurrence_id: &'a str,
    delivery_attempts: u64,
    sealed_policy_sha256: &'a Sha256Digest,
    active_policy_sha256: Option<&'a Sha256Digest>,
    source_effect_sha256: &'a Sha256Digest,
    source_action_sha256: &'a Sha256Digest,
    reason_kind: &'a str,
}

fn quarantine_legacy_derived_outbox_rows(
    transaction: &Transaction<'_>,
    quarantined_at: DateTime<Utc>,
    limit: u32,
) -> CoreResult<()> {
    let legacy_rows = {
        let mut statement = transaction
            .prepare(
                "SELECT occurrence.occurrence_id, occurrence.delivery_attempts,
                        occurrence.policy_sha256, occurrence.source_effect_sha256,
                        occurrence.source_action_sha256
                 FROM interaction_derived_event_outbox AS occurrence
                 WHERE occurrence.status != 'acknowledged'
                   AND (
                       occurrence.evaluation_seal_version != 1
                       OR occurrence.evaluation_seal_json IS NULL
                       OR occurrence.evaluation_seal_sha256 IS NULL
                       OR occurrence.deterministic_seed_hex IS NULL
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id = occurrence.occurrence_id
                   )
                 ORDER BY occurrence.occurrence_id
                 LIMIT ?1",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([i64::from(limit)], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    for (occurrence_id, delivery_attempts, policy_sha256, effect_sha256, action_sha256) in
        legacy_rows
    {
        let prior_delivery_attempts =
            u64_from_i64("legacy derived delivery attempts", delivery_attempts)?;
        let delivery_attempts = prior_delivery_attempts
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("legacy derived delivery attempts overflowed"))?;
        let claimed = transaction
            .execute(
                "UPDATE interaction_derived_event_outbox
                 SET status = 'claimed',
                     delivery_attempts = delivery_attempts + 1,
                     lease_until = ?3,
                     available_at = ?3
                 WHERE occurrence_id = ?1
                   AND status != 'acknowledged'
                   AND delivery_attempts = ?2
                   AND (
                       evaluation_seal_version != 1
                       OR evaluation_seal_json IS NULL
                       OR evaluation_seal_sha256 IS NULL
                       OR deterministic_seed_hex IS NULL
                   )",
                params![
                    occurrence_id,
                    i64_from_u64(
                        "legacy derived prior delivery attempts",
                        prior_delivery_attempts,
                    )?,
                    quarantined_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if claimed != 1 {
            return Err(storage_corrupted(
                "legacy derived interaction claim was not exact",
            ));
        }
        let sealed_policy_sha256 =
            Sha256Digest::parse(policy_sha256).map_err(CoreError::invalid)?;
        let source_effect_sha256 =
            Sha256Digest::parse(effect_sha256).map_err(CoreError::invalid)?;
        let source_action_sha256 =
            Sha256Digest::parse(action_sha256).map_err(CoreError::invalid)?;
        let evidence = DerivedQuarantineEvidence {
            schema_version: 1,
            occurrence_id: &occurrence_id,
            delivery_attempts,
            sealed_policy_sha256: &sealed_policy_sha256,
            active_policy_sha256: None,
            source_effect_sha256: &source_effect_sha256,
            source_action_sha256: &source_action_sha256,
            reason_kind: "sealed_policy_recovery_failed",
        };
        let evidence_json = encode_json(
            "legacy derived interaction quarantine evidence",
            &evidence,
            MAX_AUDIT_JSON_BYTES,
        )?;
        let evidence_sha256 = sha256_hex(evidence_json.as_bytes());
        let inserted = transaction
            .execute(
                "INSERT INTO interaction_derived_event_quarantines
                 (occurrence_id, reason_kind, delivery_attempts,
                  sealed_policy_sha256, active_policy_sha256,
                  source_effect_sha256, source_action_sha256,
                  evidence_json, evidence_sha256, quarantined_at)
                 VALUES (?1, 'sealed_policy_recovery_failed', ?2, ?3, NULL,
                         ?4, ?5, ?6, ?7, ?8)",
                params![
                    occurrence_id,
                    i64_from_u64("legacy derived delivery attempts", delivery_attempts)?,
                    sealed_policy_sha256.as_str(),
                    source_effect_sha256.as_str(),
                    source_action_sha256.as_str(),
                    evidence_json,
                    evidence_sha256,
                    quarantined_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if inserted != 1 {
            return Err(storage_corrupted(
                "legacy derived interaction quarantine insert was not exact",
            ));
        }
    }
    Ok(())
}

fn read_derived_event_quarantine(
    connection: &Connection,
    occurrence_id: &str,
) -> CoreResult<Option<StoredInteractionDerivedEventQuarantine>> {
    connection
        .query_row(
            "SELECT delivery_attempts, sealed_policy_sha256,
                    active_policy_sha256, source_effect_sha256,
                    source_action_sha256, reason_kind, evidence_json,
                    evidence_sha256, quarantined_at
             FROM interaction_derived_event_quarantines
             WHERE occurrence_id = ?1",
            [occurrence_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .map(|row| {
            let delivery_attempts = u64_from_i64("derived quarantine delivery attempts", row.0)?;
            let sealed_policy_sha256 = Sha256Digest::parse(row.1).map_err(CoreError::invalid)?;
            let active_policy_sha256 = row
                .2
                .map(Sha256Digest::parse)
                .transpose()
                .map_err(CoreError::invalid)?;
            let source_effect_sha256 = Sha256Digest::parse(row.3).map_err(CoreError::invalid)?;
            let source_action_sha256 = Sha256Digest::parse(row.4).map_err(CoreError::invalid)?;
            if row.5 != "sealed_policy_recovery_failed" {
                return Err(storage_corrupted(
                    "derived quarantine reason kind is invalid",
                ));
            }
            let canonical_evidence = encode_json(
                "derived interaction quarantine evidence",
                &DerivedQuarantineEvidence {
                    schema_version: 1,
                    occurrence_id,
                    delivery_attempts,
                    sealed_policy_sha256: &sealed_policy_sha256,
                    active_policy_sha256: active_policy_sha256.as_ref(),
                    source_effect_sha256: &source_effect_sha256,
                    source_action_sha256: &source_action_sha256,
                    reason_kind: "sealed_policy_recovery_failed",
                },
                MAX_AUDIT_JSON_BYTES,
            )?;
            if canonical_evidence != row.6 || sha256_hex(row.6.as_bytes()) != row.7 {
                return Err(storage_corrupted(
                    "derived quarantine evidence hash is inconsistent",
                ));
            }
            Ok(StoredInteractionDerivedEventQuarantine {
                occurrence_id: occurrence_id.to_owned(),
                delivery_attempts,
                sealed_policy_sha256,
                active_policy_sha256,
                source_effect_sha256,
                source_action_sha256,
                evidence_sha256: Sha256Digest::parse(row.7).map_err(CoreError::invalid)?,
                quarantined_at: parse_datetime("derived quarantine timestamp", &row.8)?,
                exact_replay: false,
            })
        })
        .transpose()
}

fn read_derived_outbox_row(
    connection: &Connection,
    occurrence_id: &str,
) -> CoreResult<Option<RawDerivedOutboxRow>> {
    connection
        .query_row(
            "SELECT occurrence_id, chain_id, root_event_id, parent_event_id,
                    parent_occurrence_id, conversation_id, branch_id, depth,
                    chain_ordinal, source_effect_ordinal,
                    parent_event_commit_sha256, parent_resulting_state_revision,
                    source_effect_sha256,
                    source_action_sha256, source_set_revision_id, source_rule_id,
                    source_action_ordinal, event_kind, event_argument_json,
                    event_sha256, visited_event_sha256s_json, policy_json,
                    policy_sha256, evaluation_seal_json,
                    evaluation_seal_sha256, evaluation_seal_version,
                    deterministic_seed_hex, occurred_at, available_at, status,
                    delivery_attempts, lease_until
             FROM interaction_derived_event_outbox WHERE occurrence_id = ?1",
            [occurrence_id],
            |row| {
                Ok(RawDerivedOutboxRow {
                    occurrence_id: row.get(0)?,
                    chain_id: row.get(1)?,
                    root_event_id: row.get(2)?,
                    parent_event_id: row.get(3)?,
                    parent_occurrence_id: row.get(4)?,
                    conversation_id: row.get(5)?,
                    branch_id: row.get(6)?,
                    depth: row.get(7)?,
                    chain_ordinal: row.get(8)?,
                    source_effect_ordinal: row.get(9)?,
                    parent_event_commit_sha256: row.get(10)?,
                    parent_resulting_state_revision: row.get(11)?,
                    source_effect_sha256: row.get(12)?,
                    source_action_sha256: row.get(13)?,
                    source_set_revision_id: row.get(14)?,
                    source_rule_id: row.get(15)?,
                    source_action_ordinal: row.get(16)?,
                    event_kind: row.get(17)?,
                    event_argument_json: row.get(18)?,
                    event_sha256: row.get(19)?,
                    visited_event_sha256s_json: row.get(20)?,
                    policy_json: row.get(21)?,
                    policy_sha256: row.get(22)?,
                    evaluation_seal_json: row.get(23)?,
                    evaluation_seal_sha256: row.get(24)?,
                    evaluation_seal_version: row.get(25)?,
                    deterministic_seed_hex: row.get(26)?,
                    occurred_at: row.get(27)?,
                    available_at: row.get(28)?,
                    status: row.get(29)?,
                    delivery_attempts: row.get(30)?,
                    lease_until: row.get(31)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)
}

fn decode_claimed_derived_outbox_row(
    connection: &Connection,
    raw: RawDerivedOutboxRow,
) -> CoreResult<StoredInteractionDerivedEvent> {
    if raw.status != "claimed" || raw.lease_until.is_none() {
        return Err(storage_corrupted(
            "derived interaction occurrence is not durably claimed",
        ));
    }
    decode_derived_outbox_row(connection, raw)
}

fn decode_derived_outbox_row(
    connection: &Connection,
    raw: RawDerivedOutboxRow,
) -> CoreResult<StoredInteractionDerivedEvent> {
    if !matches!(raw.status.as_str(), "pending" | "claimed" | "acknowledged") {
        return Err(storage_corrupted(
            "derived interaction occurrence status is invalid",
        ));
    }
    let depth = u32::try_from(raw.depth)
        .map_err(|_| storage_corrupted("derived interaction depth is invalid"))?;
    let chain_ordinal = u32::try_from(raw.chain_ordinal)
        .map_err(|_| storage_corrupted("derived interaction ordinal is invalid"))?;
    let source_effect_ordinal = u32::try_from(raw.source_effect_ordinal)
        .map_err(|_| storage_corrupted("derived source effect ordinal is invalid"))?;
    let source_action_ordinal = u32::try_from(raw.source_action_ordinal)
        .map_err(|_| storage_corrupted("derived source action ordinal is invalid"))?;
    let event =
        decode_stored_interaction_event(&raw.event_kind, Some(raw.event_argument_json.as_str()))?;
    if !matches!(
        event,
        InteractionEvent::VariableChanged { .. } | InteractionEvent::KnowledgeActivated { .. }
    ) {
        return Err(storage_corrupted(
            "derived interaction occurrence has a forbidden event kind",
        ));
    }
    let event_sha256 = Sha256Digest::parse(raw.event_sha256).map_err(CoreError::invalid)?;
    if interaction_event_sha256(&event)? != event_sha256 {
        return Err(storage_corrupted(
            "derived interaction event digest is invalid",
        ));
    }
    let policy = decode_interaction_policy(
        &stored_module_plan_sha256_from_json(&raw.policy_json)?,
        &raw.policy_json,
        &raw.policy_sha256,
    )?;
    let policy_sha256 = Sha256Digest::parse(raw.policy_sha256).map_err(CoreError::invalid)?;
    if raw.evaluation_seal_version != 1 {
        return Err(storage_corrupted(
            "derived interaction occurrence has no v1 evaluation seal",
        ));
    }
    let evaluation_seal_json = raw.evaluation_seal_json.ok_or_else(|| {
        storage_corrupted("derived interaction occurrence evaluation seal is missing")
    })?;
    let stored_evaluation_seal_sha256 = raw.evaluation_seal_sha256.ok_or_else(|| {
        storage_corrupted("derived interaction occurrence evaluation seal hash is missing")
    })?;
    let evaluation_seal: InteractionEvaluationSeal = decode_json(
        "derived interaction evaluation seal",
        &evaluation_seal_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let canonical_evaluation_seal_json = encode_json(
        "derived interaction evaluation seal",
        &evaluation_seal,
        MAX_STATE_JSON_BYTES,
    )?;
    let evaluation_seal_sha256 = interaction_evaluation_seal_sha256(&evaluation_seal)?;
    if canonical_evaluation_seal_json != evaluation_seal_json
        || evaluation_seal_sha256.as_str() != stored_evaluation_seal_sha256
        || evaluation_seal.policy_sha256 != policy_sha256
    {
        return Err(storage_corrupted(
            "derived interaction occurrence evaluation seal is invalid",
        ));
    }
    let deterministic_seed_hex = raw.deterministic_seed_hex.ok_or_else(|| {
        storage_corrupted("derived interaction occurrence deterministic seed is missing")
    })?;
    let deterministic_seed = decode_u64_hex(
        "derived interaction deterministic seed",
        &deterministic_seed_hex,
    )?;
    let source_action_sha256 =
        Sha256Digest::parse(&raw.source_action_sha256).map_err(CoreError::invalid)?;
    let expected_occurrence_hash = sha256_hex(
        encode_json(
            "derived interaction occurrence identity",
            &(
                "lorepia.interaction-derived-occurrence.v1",
                &raw.chain_id,
                &raw.parent_event_id,
                source_effect_ordinal,
                &event_sha256,
                &source_action_sha256,
                evaluation_seal_sha256.as_str(),
                deterministic_seed,
            ),
            MAX_AUDIT_JSON_BYTES,
        )?
        .as_bytes(),
    );
    if raw.occurrence_id != format!("interaction-derived-{expected_occurrence_hash}") {
        return Err(storage_corrupted(
            "derived interaction occurrence identity fingerprint is invalid",
        ));
    }
    let visited_event_sha256s: Vec<Sha256Digest> = decode_json(
        "derived interaction visited events",
        &raw.visited_event_sha256s_json,
        MAX_AUDIT_JSON_BYTES,
    )?;
    if visited_event_sha256s.len() != usize::try_from(depth).unwrap_or(usize::MAX)
        || visited_event_sha256s.contains(&event_sha256)
    {
        return Err(storage_corrupted(
            "derived interaction visited-set evidence is invalid",
        ));
    }
    let (
        parent_payload_json,
        parent_resulting_state_revision,
        parent_evaluation_seal_json,
        parent_evaluation_seal_sha256,
        parent_evaluation_seal_version,
        parent_policy_sha256,
    ) = connection
        .query_row(
            "SELECT payload_json, resulting_state_revision,
                    evaluation_seal_json, evaluation_seal_sha256,
                    evaluation_seal_version, policy_sha256
             FROM interaction_events WHERE id = ?1",
            [&raw.parent_event_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("derived interaction parent event is missing"))?;
    let parent_payload: StoredEventPayload = decode_json(
        "derived interaction parent payload",
        &parent_payload_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    validate_stored_event_evaluation_authority(
        &parent_policy_sha256,
        parent_evaluation_seal_json.as_deref(),
        parent_evaluation_seal_sha256.as_deref(),
        parent_evaluation_seal_version,
        &parent_payload,
    )?;
    if parent_payload.commit_sha256 != raw.parent_event_commit_sha256
        || parent_resulting_state_revision != raw.parent_resulting_state_revision
        || parent_payload.evaluation_seal_sha256.as_ref() != Some(&evaluation_seal_sha256)
    {
        return Err(storage_corrupted(
            "derived interaction parent event evidence is invalid",
        ));
    }
    Ok(StoredInteractionDerivedEvent {
        occurrence_id: raw.occurrence_id,
        chain_id: raw.chain_id,
        root_event_id: raw.root_event_id,
        parent_event_id: raw.parent_event_id,
        parent_occurrence_id: raw.parent_occurrence_id,
        conversation_id: ConversationId(raw.conversation_id),
        branch_id: ConversationBranchId(raw.branch_id),
        depth,
        chain_ordinal,
        source_effect_ordinal,
        parent_event_commit_sha256: Sha256Digest::parse(raw.parent_event_commit_sha256)
            .map_err(CoreError::invalid)?,
        parent_resulting_state_revision: u64_from_i64(
            "derived parent resulting state revision",
            raw.parent_resulting_state_revision,
        )?,
        source_effect_sha256: Sha256Digest::parse(raw.source_effect_sha256)
            .map_err(CoreError::invalid)?,
        source_action_sha256,
        source_set_revision_id: raw.source_set_revision_id,
        source_rule_id: InteractionRuleId::from(raw.source_rule_id),
        source_action_ordinal,
        event,
        event_sha256,
        visited_event_sha256s,
        policy,
        policy_sha256,
        evaluation_seal,
        evaluation_seal_sha256,
        deterministic_seed,
        occurred_at: parse_datetime("derived interaction occurred_at", &raw.occurred_at)?,
        available_at: parse_datetime("derived interaction available_at", &raw.available_at)?,
        delivery_attempts: u64_from_i64(
            "derived interaction delivery attempts",
            raw.delivery_attempts,
        )?,
        lease_until: raw
            .lease_until
            .as_deref()
            .map(|value| parse_datetime("derived interaction lease_until", value))
            .transpose()?,
    })
}

fn stored_module_plan_sha256_from_json(policy_json: &str) -> CoreResult<String> {
    let policy: InteractionPolicySnapshot = decode_json(
        "derived interaction policy",
        policy_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    Ok(stored_module_plan_sha256(&policy))
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

#[allow(clippy::too_many_arguments)]
fn materialize_generation_attempt_closed_closure(
    transaction: &Transaction<'_>,
    generation_id: &GenerationId,
    key: &InteractionStateKey,
    closure: &GenerationAttemptDerivedClosure,
    previous_state: &InteractionState,
    previous_knowledge: &[InteractionKnowledgeBinding],
    root_idempotency_key: &str,
    bind_root_to_attempt: bool,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    generation_attempt_derived_closure_sha256(closure)?;
    let root_seal = closure
        .transitions
        .first()
        .ok_or_else(|| storage_corrupted("generation append closure has no root"))?
        .evaluation_seal
        .clone();
    let mut expected_state = previous_state.clone();
    let mut expected_knowledge = previous_knowledge.to_vec();
    for transition in &closure.transitions {
        if transition.expected_state_revision != expected_state.revision
            || transition.evaluation_seal != root_seal
            || transition.commit_sha256
                != crate::generation_attempt_derived_transition_commit_sha256(
                    generation_id,
                    transition,
                )?
        {
            return Err(storage_corrupted(
                "generation append transition authority is inconsistent",
            ));
        }
        let current = require_state_for_key(transaction, key)?;
        let current_knowledge = read_knowledge_bindings(transaction, &current.id)?;
        if current.state != expected_state || current_knowledge != expected_knowledge {
            return Err(revision_conflict(
                "generation append transition predecessor changed",
            ));
        }
        validate_existing_proposals_unchanged(
            transaction,
            &current.id,
            &expected_state,
            &transition.next_state,
            &transition.proposals,
        )?;
        let idempotency_key = if transition.ordinal == 0 {
            root_idempotency_key.to_owned()
        } else {
            format!(
                "generation-attempt-closed:v1:{}:{}:{}",
                generation_id.0,
                transition.ordinal,
                transition.commit_sha256.as_str(),
            )
        };
        if event_id_or_idempotency_exists(transaction, &transition.event_id, &idempotency_key)? {
            return Err(revision_conflict(
                "generation closed transition materialization already exists",
            ));
        }
        let commit = InteractionEventCommit {
            event_id: transition.event_id.clone(),
            idempotency_key,
            key: key.clone(),
            expected_state_revision: transition.expected_state_revision,
            event: transition.event.clone(),
            generation_attempt_id: (bind_root_to_attempt && transition.ordinal == 0)
                .then(|| generation_id.clone()),
            owner_message_id: None,
            policy: transition.policy.clone(),
            evaluation_seal: Some(transition.evaluation_seal.clone()),
            deterministic_seed: Some(transition.deterministic_seed),
            next_state: transition.next_state.clone(),
            knowledge: transition.knowledge.clone(),
            action_results: transition.action_results.clone(),
            effects: transition.effects.clone(),
            derived_events: transition.derived_events.clone(),
            proposals: transition.proposals.clone(),
            created_at,
        };
        validate_event_commit(&commit)?;
        let fingerprint = event_commit_sha256(&commit)?;
        let payload = stored_event_payload(&commit, fingerprint)?;
        let payload_json = encode_json(
            "generation closed transition payload",
            &payload,
            MAX_EVENT_JSON_BYTES,
        )?;
        write_event_transition(
            transaction,
            InteractionEventTransitionWrite {
                key,
                expected_state_revision: commit.expected_state_revision,
                event: &commit.event,
                generation_attempt_id: commit.generation_attempt_id.as_ref(),
                proposal_namespace_generation_id: Some(generation_id),
                owner_message_id: None,
                policy: &commit.policy,
                evaluation_seal: commit.evaluation_seal.as_ref(),
                deterministic_seed: commit.deterministic_seed,
                next_state: &commit.next_state,
                knowledge: &commit.knowledge,
                action_results: &commit.action_results,
                effects: &commit.effects,
                derived_events: &commit.derived_events,
                proposals: &commit.proposals,
                event_id: &commit.event_id,
                idempotency_key: &commit.idempotency_key,
                payload_json: &payload_json,
                created_at: commit.created_at,
                generation_append_materialization: true,
                derived_chain_parent: None,
            },
        )?;
        expected_state = transition.next_state.clone();
        expected_knowledge.clone_from(&transition.knowledge);
    }
    if expected_state != closure.final_state || expected_knowledge != closure.final_knowledge {
        return Err(storage_corrupted(
            "generation append closure final snapshot is inconsistent",
        ));
    }
    Ok(())
}

/// Consumes the immutable attempt-owned interaction review inside the same
/// transaction that makes its generation visible.
///
/// The staged review and proposal decisions deliberately do not mutate a live
/// branch. This function is the sole bridge from that isolated authority to
/// ordinary interaction rows, and therefore must only be called after the
/// exact dispatch-ready attempt and generation append have been validated.
#[allow(clippy::too_many_lines)]
pub(crate) fn materialize_generation_attempt_interaction_for_append(
    _storage: &Storage,
    transaction: &Transaction<'_>,
    attempt: &StoredGenerationAttempt,
    target_key: &InteractionStateKey,
    prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
    materialized_at: DateTime<Utc>,
) -> CoreResult<GenerationAttemptInteractionMaterializationReceipt> {
    if attempt.status != crate::GenerationAttemptStatus::DispatchReady
        || target_key.conversation_id != attempt.input.conversation_id
        || target_key.branch_id != attempt.input.proposed_branch_id
    {
        return Err(revision_conflict(
            "generation interaction materialization lacks dispatch-ready target authority",
        ));
    }
    let seal = attempt.dispatch_seal.as_ref().ok_or_else(|| {
        storage_corrupted("dispatch-ready generation attempt is missing its seal")
    })?;
    let prompt_authority = attempt
        .input
        .prompt_selection_authority
        .as_ref()
        .ok_or_else(|| storage_corrupted("generation attempt prompt authority is missing"))?;
    let before = read_generation_attempt_before_review(transaction, &attempt.generation_id, None)?
        .ok_or_else(|| {
            storage_corrupted("generation attempt is missing its BeforeGeneration review")
        })?;
    let snapshot = read_generation_attempt_append_snapshot(transaction, &attempt.generation_id)?;
    let aggregate =
        read_generation_attempt_interaction_aggregate(transaction, &attempt.generation_id)?;
    let decisions = read_generation_attempt_append_decisions(transaction, &attempt.generation_id)?;
    let decision_event_ids = decisions
        .iter()
        .filter_map(|decision| decision.decision_event_id.clone())
        .collect::<Vec<_>>();
    let decision_event_sha256s = decisions
        .iter()
        .filter_map(|decision| decision.decision_event_sha256.clone())
        .collect::<Vec<_>>();

    if attempt.before_generation_evidence.as_ref() != Some(&before.evidence)
        || attempt.before_generation_evidence_sha256.as_ref() != Some(&before.evidence_sha256)
        || before.event_id != snapshot.event_id
        || before.event_sha256 != snapshot.event_sha256
        || before.review_sha256 != snapshot.review_sha256
        || aggregate.pending_proposal_count != 0
        || aggregate.terminal_decision_count as usize != decisions.len()
        || seal.final_interaction_state_revision != aggregate.state.revision
        || seal.final_interaction_state_sha256 != aggregate.state_snapshot_sha256
        || seal.before_generation_evidence_sha256 != before.evidence_sha256
        || seal.approval_evidence_sha256.as_ref() != attempt.approval_evidence_sha256.as_ref()
        || seal.derived_chain_sha256.as_ref() != Some(&aggregate.derived_chain_sha256)
        || seal.derived_event_count != Some(aggregate.derived_event_count)
        || seal.derived_guard_count != Some(aggregate.derived_guard_count)
        || seal.applied_module_plan_sha256 != attempt.input.module_plan_sha256
        || prompt_plan.generation_id != attempt.generation_id
        || prompt_plan.conversation_id != attempt.input.conversation_id
        || prompt_plan.branch_id != attempt.input.proposed_branch_id
        || prompt_plan.plan_sha256 != seal.final_prompt_plan_sha256.as_str()
        || prompt_plan.input_fingerprint_sha256
            != seal.final_prompt_input_fingerprint_sha256.as_str()
        || aggregate.decision_event_ids != decision_event_ids
        || aggregate.decision_event_sha256s != decision_event_sha256s
    {
        return Err(storage_corrupted(
            "generation attempt append evidence is internally inconsistent",
        ));
    }
    match (&attempt.approval_evidence, &before.approval_evidence) {
        (None, None) if decisions.is_empty() => {}
        (Some(expected), Some(stored))
            if expected == stored
                && expected.resulting_state_revision == aggregate.state.revision
                && expected.resulting_state_sha256 == aggregate.state_snapshot_sha256
                && expected.decision_event_ids == decision_event_ids
                && expected.decision_event_sha256s == decision_event_sha256s => {}
        _ => {
            return Err(storage_corrupted(
                "generation attempt approval evidence does not match its terminal decisions",
            ));
        }
    }

    if snapshot.memory_head_snapshot.conversation_id != attempt.input.conversation_id
        || snapshot.memory_head_snapshot.source_branch_id != attempt.input.source_branch_id
        || snapshot.memory_head_snapshot.context_head_message_id
            != attempt.input.context_head_message_id
        || snapshot.memory_head_snapshot.include_invalidated
    {
        return Err(storage_corrupted(
            "generation memory snapshot differs from its immutable attempt authority",
        ));
    }
    validate_generation_prompt_memory_snapshot(prompt_plan, &snapshot.memory_head_snapshot)?;

    if snapshot
        .module_runtime_review
        .context
        .conversation_id
        .as_deref()
        != Some(attempt.input.conversation_id.0.as_str())
        || snapshot.module_runtime_review.context.branch_id.as_deref()
            != Some(attempt.input.proposed_branch_id.0.as_str())
        || snapshot
            .module_runtime_review
            .context
            .character_id
            .as_deref()
            != Some(prompt_authority.character.id.as_str())
        || snapshot.module_runtime_review.context.persona_id.as_ref()
            != prompt_authority
                .persona_selection
                .as_ref()
                .map(|selection| &selection.value.persona_id)
        || prompt_local_user_id_sha256(&snapshot.module_runtime_review.context.local_user_id)
            != prompt_authority.local_user_id_sha256
        || !snapshot
            .module_runtime_review
            .activation_binding_ids
            .is_empty()
    {
        return Err(storage_corrupted(
            "generation module review differs from its immutable attempt authority",
        ));
    }
    // Freshness was checked before the immutable review was staged. Append
    // intentionally verifies that sealed review and applied-plan authority
    // below instead of re-reading mutable settings, persona, or bindings.
    require_no_pending_derived_predecessor_through(
        transaction,
        &attempt.input.conversation_id,
        &attempt.input.source_branch_id,
        snapshot.previous_state.revision,
    )?;

    let source_and_target_match =
        attempt.input.source_branch_id == attempt.input.proposed_branch_id;
    if source_and_target_match {
        let current = require_state_for_key(transaction, target_key)?;
        let current_knowledge = read_knowledge_bindings(transaction, &current.id)?;
        if current.state != snapshot.previous_state
            || current_knowledge != snapshot.previous_knowledge
            || interaction_state_snapshot_sha256(&current.state, &current_knowledge)?
                != snapshot.context_checkpoint_sha256.as_str()
        {
            return Err(revision_conflict(
                "same-branch interaction state changed before generation append",
            ));
        }
    } else {
        let cloned = clone_interaction_checkpoint_for_branch_transaction(
            transaction,
            &attempt.input.conversation_id,
            &attempt.input.source_branch_id,
            attempt.input.context_head_message_id.as_ref(),
            target_key,
            materialized_at,
        )?;
        if cloned.checkpoint_sha256 != snapshot.context_checkpoint_sha256.as_str()
            || cloned.cloned.state != snapshot.previous_state
            || cloned.cloned.knowledge != snapshot.previous_knowledge
        {
            return Err(revision_conflict(
                "fork interaction checkpoint differs from the reviewed generation boundary",
            ));
        }
    }

    match snapshot.applied_runtime_plan.as_ref() {
        Some(runtime) => {
            if runtime.applied_plan_sha256 != seal.applied_module_plan_sha256
                || runtime.applied_plan_sha256 != attempt.input.module_plan_sha256
                || runtime.review != snapshot.module_runtime_review
                || runtime.derived_from_plan_sha256 != snapshot.source_runtime_plan_sha256
                || Some(runtime.source_approval.plan.plan_sha256.clone())
                    != snapshot.source_activation_plan_sha256
                || snapshot.policy.module_plan_sha256.as_deref()
                    != Some(runtime.applied_plan_sha256.as_str())
            {
                return Err(storage_corrupted(
                    "generation runtime plan differs from its immutable append authority",
                ));
            }
            crate::orchestration::persist_applied_module_runtime_plan_transaction(
                transaction,
                runtime,
                materialized_at,
            )?;
        }
        None => {
            if seal.applied_module_plan_sha256 != no_applied_module_runtime_plan_sha256()
                || snapshot.source_runtime_plan_sha256.is_some()
                || snapshot.source_activation_plan_sha256.is_some()
                || !snapshot.module_runtime_review.ordered_bindings.is_empty()
                || snapshot.policy.module_plan_sha256.is_some()
            {
                return Err(storage_corrupted(
                    "generation no-module snapshot differs from its immutable append authority",
                ));
            }
        }
    }

    let before_idempotency_key = format!(
        "generation-attempt-before:v1:{}",
        snapshot.event_sha256.as_str()
    );
    let root = before
        .derived_closure
        .transitions
        .first()
        .ok_or_else(|| storage_corrupted("generation BeforeGeneration closure has no root"))?;
    if root.event_id != snapshot.event_id
        || root.event != InteractionEvent::BeforeGeneration
        || root.policy != snapshot.policy
        || root.next_state != snapshot.next_state
        || root.knowledge != snapshot.knowledge
        || root.action_results != snapshot.action_results
        || root.effects != snapshot.effects
        || root.derived_events != snapshot.derived_events
        || before.derived_closure.final_state != aggregate.state && decisions.is_empty()
    {
        return Err(storage_corrupted(
            "generation BeforeGeneration closure differs from its append snapshot",
        ));
    }
    materialize_generation_attempt_closed_closure(
        transaction,
        &attempt.generation_id,
        target_key,
        &before.derived_closure,
        &snapshot.previous_state,
        &snapshot.previous_knowledge,
        &before_idempotency_key,
        true,
        snapshot.occurred_at,
    )?;

    for decision in &decisions {
        replay_generation_attempt_append_decision(
            transaction,
            &attempt.generation_id,
            target_key,
            decision,
        )?;
    }

    let mut closed_closures = vec![&before.derived_closure];
    closed_closures.extend(
        decisions
            .iter()
            .filter_map(|decision| decision.materialization.derived_closure.as_ref()),
    );
    for closure in closed_closures {
        for transition in &closure.transitions {
            if !transition.proposals.is_empty() {
                let suppressed = transaction
                    .execute(
                        "UPDATE interaction_effect_outbox
                         SET delivery_attempts = CASE
                               WHEN delivery_attempts = 0 THEN 1
                               ELSE delivery_attempts
                             END,
                             delivered_at = ?2
                         WHERE event_id = ?1
                           AND effect_kind = 'approval_requested'
                           AND delivered_at IS NULL",
                        params![transition.event_id, materialized_at.to_rfc3339()],
                    )
                    .map_err(storage_db_error)?;
                if suppressed != transition.proposals.len() {
                    return Err(storage_corrupted(
                        "generation approval-request effects do not match terminal proposals",
                    ));
                }
            }
            let live_derived = transaction
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM interaction_derived_event_outbox
                         WHERE parent_event_id = ?1
                           AND status != 'acknowledged'
                     )",
                    [transition.event_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if live_derived {
                return Err(storage_corrupted(
                    "generation closed materialization left a live derived occurrence",
                ));
            }
        }
    }

    let final_state = require_state_for_key(transaction, target_key)?;
    let final_knowledge = read_knowledge_bindings(transaction, &final_state.id)?;
    let final_sha256 = interaction_state_snapshot_sha256(&final_state.state, &final_knowledge)?;
    if final_state.state != aggregate.state
        || final_knowledge != aggregate.knowledge
        || final_state.state.revision != seal.final_interaction_state_revision
        || final_sha256 != seal.final_interaction_state_sha256.as_str()
    {
        return Err(storage_corrupted(
            "materialized generation interaction state differs from its dispatch seal",
        ));
    }

    Ok(GenerationAttemptInteractionMaterializationReceipt {
        final_state_revision: final_state.state.revision,
        final_state_snapshot_sha256: seal.final_interaction_state_sha256.clone(),
    })
}

fn validate_generation_prompt_memory_snapshot(
    prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
    snapshot: &MemoryRecordsAtHeadSnapshot,
) -> CoreResult<()> {
    let resolved: ResolvedPromptPlan = serde_json::from_value(prompt_plan.plan.value.clone())
        .map_err(|error| {
            CoreError::invalid(format!(
                "generation prompt plan cannot be decoded for memory verification: {error}"
            ))
        })?;
    resolved.validate().map_err(|error| {
        CoreError::invalid(format!(
            "generation prompt plan is invalid during memory verification: {error}"
        ))
    })?;
    let visible_ids = snapshot
        .records
        .iter()
        .map(|record| record.record_id.as_str())
        .collect::<BTreeSet<_>>();
    let evidence_is_visible = resolved.trace.blocks.iter().all(|trace| {
        trace
            .memory_record_ids
            .iter()
            .all(|record_id| visible_ids.contains(record_id.as_str()))
            && trace
                .memory_evidence
                .iter()
                .all(|evidence| visible_ids.contains(evidence.record_id.as_str()))
    });
    if !evidence_is_visible {
        return Err(revision_conflict(
            "generation prompt memory evidence differs from its immutable head snapshot",
        ));
    }
    Ok(())
}

pub(crate) fn require_generation_attempt_prompt_context_authority_transaction(
    transaction: &Transaction<'_>,
    attempt: &StoredGenerationAttempt,
    prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
) -> CoreResult<()> {
    let snapshot = read_generation_attempt_append_snapshot(transaction, &attempt.generation_id)?;
    let authority = attempt
        .input
        .prompt_selection_authority
        .as_ref()
        .ok_or_else(|| {
            storage_corrupted("generation attempt has no sealed prompt selection authority")
        })?;
    crate::orchestration::require_sealed_generation_prompt_context_snapshot_transaction(
        transaction,
        prompt_plan,
        crate::orchestration::SealedGenerationPromptContext {
            conversation_id: &attempt.input.conversation_id,
            target_branch_id: &attempt.input.proposed_branch_id,
            source_branch_id: &attempt.input.source_branch_id,
            context_head_message_id: attempt.input.context_head_message_id.as_ref(),
            authority,
            memory_snapshot: &snapshot.memory_head_snapshot,
        },
    )?;
    validate_generation_prompt_memory_snapshot(prompt_plan, &snapshot.memory_head_snapshot)
}

#[allow(clippy::too_many_lines)]
fn read_generation_attempt_append_snapshot(
    connection: &Connection,
    generation_id: &GenerationId,
) -> CoreResult<GenerationAttemptAppendSnapshot> {
    let raw = connection
        .query_row(
            "SELECT event_id, event_sha256, occurred_at,
                    context_checkpoint_sha256, previous_state_revision,
                    previous_state_json, previous_state_document_sha256,
                    previous_state_snapshot_sha256, previous_knowledge_json,
                    previous_knowledge_sha256, applied_runtime_plan_sha256,
                    module_runtime_review_json, module_runtime_review_sha256,
                    memory_head_snapshot_json, memory_head_snapshot_sha256,
                    source_runtime_plan_sha256, source_activation_plan_sha256,
                    applied_runtime_plan_json, policy_json, policy_sha256,
                    reviewed_next_state_json,
                    reviewed_next_state_document_sha256,
                    reviewed_next_state_snapshot_sha256, knowledge_json,
                    knowledge_sha256, action_results_json,
                    action_results_sha256, effects_json, effects_sha256,
                    derived_events_json, derived_events_sha256,
                    proposal_writes_json, proposal_writes_sha256,
                    review_sha256, domain_review_sha256,
                    storage_identity_version
             FROM generation_attempt_before_event_snapshots
             WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| {
                Ok(RawGenerationAttemptAppendSnapshot {
                    event_id: row.get(0)?,
                    event_sha256: row.get(1)?,
                    occurred_at: row.get(2)?,
                    context_checkpoint_sha256: row.get(3)?,
                    previous_state_revision: row.get(4)?,
                    previous_state_json: row.get(5)?,
                    previous_state_document_sha256: row.get(6)?,
                    previous_state_snapshot_sha256: row.get(7)?,
                    previous_knowledge_json: row.get(8)?,
                    previous_knowledge_sha256: row.get(9)?,
                    applied_runtime_plan_sha256: row.get(10)?,
                    module_runtime_review_json: row.get(11)?,
                    module_runtime_review_sha256: row.get(12)?,
                    memory_head_snapshot_json: row.get(13)?,
                    memory_head_snapshot_sha256: row.get(14)?,
                    source_runtime_plan_sha256: row.get(15)?,
                    source_activation_plan_sha256: row.get(16)?,
                    applied_runtime_plan_json: row.get(17)?,
                    policy_json: row.get(18)?,
                    policy_sha256: row.get(19)?,
                    next_state_json: row.get(20)?,
                    next_state_document_sha256: row.get(21)?,
                    next_state_snapshot_sha256: row.get(22)?,
                    knowledge_json: row.get(23)?,
                    knowledge_sha256: row.get(24)?,
                    action_results_json: row.get(25)?,
                    action_results_sha256: row.get(26)?,
                    effects_json: row.get(27)?,
                    effects_sha256: row.get(28)?,
                    derived_events_json: row.get(29)?,
                    derived_events_sha256: row.get(30)?,
                    proposal_writes_json: row.get(31)?,
                    proposal_writes_sha256: row.get(32)?,
                    review_sha256: row.get(33)?,
                    domain_review_sha256: row.get(34)?,
                    storage_identity_version: row.get(35)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("generation attempt BeforeGeneration snapshot"))?;

    let previous_state: InteractionState = decode_json(
        "generation append previous interaction state",
        &raw.previous_state_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let previous_knowledge: Vec<InteractionKnowledgeBinding> = decode_json(
        "generation append previous interaction knowledge",
        &raw.previous_knowledge_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let module_runtime_review: ModuleMergeReview = decode_json(
        "generation append module runtime review",
        &raw.module_runtime_review_json,
        MAX_STATE_JSON_BYTES,
    )?;
    module_runtime_review.verify().map_err(|error| {
        storage_corrupted(format!(
            "generation append module runtime review is invalid: {error}"
        ))
    })?;
    let memory_head_snapshot: MemoryRecordsAtHeadSnapshot = decode_json(
        "generation append memory head snapshot",
        &raw.memory_head_snapshot_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let policy: InteractionPolicySnapshot = decode_json(
        "generation append interaction policy",
        &raw.policy_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    let next_state: InteractionState = decode_json(
        "generation append reviewed interaction state",
        &raw.next_state_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let knowledge: Vec<InteractionKnowledgeBinding> = decode_json(
        "generation append reviewed interaction knowledge",
        &raw.knowledge_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let action_results: Vec<InteractionActionResultWrite> = decode_json(
        "generation append interaction action results",
        &raw.action_results_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let effects: Vec<InteractionEffect> = decode_json(
        "generation append interaction effects",
        &raw.effects_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let derived_events: Vec<InteractionDerivedEventWrite> = decode_json(
        "generation append interaction derived events",
        &raw.derived_events_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let proposals: Vec<InteractionProposalWrite> = decode_json(
        "generation append interaction proposals",
        &raw.proposal_writes_json,
        MAX_STATE_JSON_BYTES,
    )?;
    validate_state(&previous_state)?;
    validate_knowledge_bindings(&previous_state, &previous_knowledge)?;
    validate_state(&next_state)?;
    validate_knowledge_bindings(&next_state, &knowledge)?;
    validate_policy_shape(&policy)?;
    validate_event_collections(&action_results, &effects, &proposals)?;
    validate_derived_event_writes(
        connection,
        &policy,
        &action_results,
        &effects,
        &derived_events,
    )?;

    let previous_revision = u64_from_i64(
        "generation append previous interaction revision",
        raw.previous_state_revision,
    )?;
    if previous_state.revision != previous_revision
        || encode_json(
            "generation append previous interaction state",
            &previous_state,
            MAX_STATE_JSON_BYTES,
        )? != raw.previous_state_json
        || sha256_hex(raw.previous_state_json.as_bytes()) != raw.previous_state_document_sha256
        || interaction_state_snapshot_sha256(&previous_state, &previous_knowledge)?
            != raw.previous_state_snapshot_sha256
        || encode_json(
            "generation append previous interaction knowledge",
            &previous_knowledge,
            MAX_STATE_JSON_BYTES,
        )? != raw.previous_knowledge_json
        || sha256_hex(raw.previous_knowledge_json.as_bytes()) != raw.previous_knowledge_sha256
        || encode_json(
            "generation append module runtime review",
            &module_runtime_review,
            MAX_STATE_JSON_BYTES,
        )? != raw.module_runtime_review_json
        || sha256_hex(raw.module_runtime_review_json.as_bytes()) != raw.module_runtime_review_sha256
        || encode_json(
            "generation append memory head snapshot",
            &memory_head_snapshot,
            MAX_STATE_JSON_BYTES,
        )? != raw.memory_head_snapshot_json
        || memory_records_at_head_snapshot_sha256(&memory_head_snapshot)?
            != raw.memory_head_snapshot_sha256
        || memory_head_snapshot.snapshot_sha256 != raw.memory_head_snapshot_sha256
        || encode_json(
            "generation append interaction policy",
            &policy,
            MAX_EVENT_JSON_BYTES,
        )? != raw.policy_json
        || interaction_policy_sha256(&policy)? != raw.policy_sha256
        || encode_json(
            "generation append reviewed interaction state",
            &next_state,
            MAX_STATE_JSON_BYTES,
        )? != raw.next_state_json
        || sha256_hex(raw.next_state_json.as_bytes()) != raw.next_state_document_sha256
        || interaction_state_snapshot_sha256(&next_state, &knowledge)?
            != raw.next_state_snapshot_sha256
        || encode_json(
            "generation append reviewed interaction knowledge",
            &knowledge,
            MAX_STATE_JSON_BYTES,
        )? != raw.knowledge_json
        || sha256_hex(raw.knowledge_json.as_bytes()) != raw.knowledge_sha256
        || encode_json(
            "generation append interaction action results",
            &action_results,
            MAX_STATE_JSON_BYTES,
        )? != raw.action_results_json
        || sha256_hex(raw.action_results_json.as_bytes()) != raw.action_results_sha256
        || encode_json(
            "generation append interaction effects",
            &effects,
            MAX_STATE_JSON_BYTES,
        )? != raw.effects_json
        || sha256_hex(raw.effects_json.as_bytes()) != raw.effects_sha256
        || encode_json(
            "generation append interaction derived events",
            &derived_events,
            MAX_STATE_JSON_BYTES,
        )? != raw.derived_events_json
        || sha256_hex(raw.derived_events_json.as_bytes()) != raw.derived_events_sha256
        || encode_json(
            "generation append interaction proposals",
            &proposals,
            MAX_STATE_JSON_BYTES,
        )? != raw.proposal_writes_json
        || sha256_hex(raw.proposal_writes_json.as_bytes()) != raw.proposal_writes_sha256
    {
        return Err(storage_corrupted(
            "generation attempt append snapshot fingerprint is invalid",
        ));
    }

    let source_runtime_plan_sha256 = raw
        .source_runtime_plan_sha256
        .as_deref()
        .map(Sha256Digest::parse)
        .transpose()
        .map_err(CoreError::invalid)?;
    let source_activation_plan_sha256 = raw
        .source_activation_plan_sha256
        .as_deref()
        .map(Sha256Digest::parse)
        .transpose()
        .map_err(CoreError::invalid)?;

    let applied_runtime_plan = raw
        .applied_runtime_plan_json
        .as_deref()
        .map(|json| {
            let runtime: AppliedModuleRuntimePlan = decode_json(
                "generation append applied module runtime plan",
                json,
                MAX_STATE_JSON_BYTES,
            )?;
            runtime.verify().map_err(|error| {
                CoreError::invalid(format!(
                    "generation append runtime plan is invalid: {error}"
                ))
            })?;
            if encode_json(
                "generation append applied module runtime plan",
                &runtime,
                MAX_STATE_JSON_BYTES,
            )? != json
                || runtime.applied_plan_sha256.as_str() != raw.applied_runtime_plan_sha256
                || runtime.review != module_runtime_review
                || runtime.derived_from_plan_sha256 != source_runtime_plan_sha256
                || Some(runtime.source_approval.plan.plan_sha256.clone())
                    != source_activation_plan_sha256
            {
                return Err(storage_corrupted(
                    "generation append runtime plan fingerprint is invalid",
                ));
            }
            Ok(runtime)
        })
        .transpose()?;
    if applied_runtime_plan.is_none()
        && raw.applied_runtime_plan_sha256 != no_applied_module_runtime_plan_sha256().as_str()
    {
        return Err(storage_corrupted(
            "generation append no-module sentinel is invalid",
        ));
    }
    let storage_identity_version = u32::try_from(raw.storage_identity_version)
        .map_err(|_| storage_corrupted("generation append review identity version is invalid"))?;
    let expected_review_sha256 = match storage_identity_version {
        1 => raw.domain_review_sha256.clone(),
        2 => generation_attempt_before_review_storage_sha256(
            generation_id,
            &raw.domain_review_sha256,
        )?,
        _ => {
            return Err(storage_corrupted(
                "generation append review identity version is invalid",
            ));
        }
    };
    if raw.review_sha256 != expected_review_sha256 {
        return Err(storage_corrupted(
            "generation append review storage identity is invalid",
        ));
    }
    validate_generation_attempt_append_proposal_identities(connection, generation_id, &proposals)?;

    Ok(GenerationAttemptAppendSnapshot {
        event_id: raw.event_id,
        event_sha256: Sha256Digest::parse(raw.event_sha256).map_err(CoreError::invalid)?,
        occurred_at: parse_datetime(
            "generation append BeforeGeneration timestamp",
            &raw.occurred_at,
        )?,
        context_checkpoint_sha256: Sha256Digest::parse(raw.context_checkpoint_sha256)
            .map_err(CoreError::invalid)?,
        previous_state,
        previous_knowledge,
        module_runtime_review,
        memory_head_snapshot,
        source_runtime_plan_sha256,
        source_activation_plan_sha256,
        applied_runtime_plan,
        policy,
        next_state,
        knowledge,
        action_results,
        effects,
        derived_events,
        review_sha256: Sha256Digest::parse(raw.review_sha256).map_err(CoreError::invalid)?,
    })
}

fn read_generation_attempt_append_decisions(
    connection: &Connection,
    generation_id: &GenerationId,
) -> CoreResult<Vec<GenerationAttemptAppendDecision>> {
    let raw = {
        let mut statement = connection
            .prepare(
                "SELECT proposal_record_id, status, proposal_revision,
                        decision_idempotency_key, decision_event_id,
                        decision_event_sha256, decision_evidence_json,
                        decision_evidence_sha256, resulting_aggregate_revision,
                        materialization_json, materialization_sha256,
                        decided_at_epoch_seconds, updated_at
                 FROM generation_attempt_proposals
                 WHERE generation_id = ?1 AND status != 'pending'
                 ORDER BY resulting_aggregate_revision, proposal_record_id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([generation_id.0.as_str()], |row| {
                Ok(RawGenerationAttemptAppendDecision {
                    proposal_record_id: row.get(0)?,
                    status: row.get(1)?,
                    proposal_revision: row.get(2)?,
                    decision_idempotency_key: row.get(3)?,
                    decision_event_id: row.get(4)?,
                    decision_event_sha256: row.get(5)?,
                    decision_evidence_json: row.get(6)?,
                    decision_evidence_sha256: row.get(7)?,
                    resulting_aggregate_revision: row.get(8)?,
                    materialization_json: row.get(9)?,
                    materialization_sha256: row.get(10)?,
                    decided_at_epoch_seconds: row.get(11)?,
                    updated_at: row.get(12)?,
                })
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };

    raw.into_iter()
        .enumerate()
        .map(|(ordinal, raw)| {
            let proposal_record_id = InteractionProposalRecordId::from(raw.proposal_record_id);
            let proposal_revision =
                u64_from_i64("generation append proposal revision", raw.proposal_revision)?;
            let resulting_aggregate_revision = u64_from_i64(
                "generation append resulting aggregate revision",
                raw.resulting_aggregate_revision,
            )?;
            let expected_aggregate_revision = resulting_aggregate_revision
                .checked_sub(1)
                .ok_or_else(|| storage_corrupted("generation decision aggregate underflowed"))?;
            let expected_resulting_revision = u64::try_from(ordinal)
                .map_err(|_| CoreError::invalid("too many generation proposal decisions"))?
                .checked_add(2)
                .ok_or_else(|| CoreError::invalid("generation decision revision overflowed"))?;
            let materialization: GenerationAttemptProposalDecisionMaterialization = decode_json(
                "generation append proposal materialization",
                &raw.materialization_json,
                MAX_STATE_JSON_BYTES,
            )?;
            if materialization.schema_version != 1
                || proposal_revision != 2
                || resulting_aggregate_revision != expected_resulting_revision
                || raw.status
                    != proposal_status_wire(match materialization.decision {
                        GenerationAttemptProposalDecision::Approve => {
                            InteractionProposalStatus::Approved
                        }
                        GenerationAttemptProposalDecision::Reject => {
                            InteractionProposalStatus::Rejected
                        }
                        GenerationAttemptProposalDecision::Expire => {
                            InteractionProposalStatus::Expired
                        }
                    })
                || encode_json(
                    "generation append proposal materialization",
                    &materialization,
                    MAX_STATE_JSON_BYTES,
                )? != raw.materialization_json
                || sha256_hex(raw.materialization_json.as_bytes()) != raw.materialization_sha256
            {
                return Err(storage_corrupted(
                    "generation proposal materialization row is invalid",
                ));
            }
            let commit = GenerationAttemptProposalDecisionCommit {
                proposal_record_id: proposal_record_id.clone(),
                expected_proposal_revision: 1,
                expected_aggregate_revision,
                decision: materialization.decision,
                decision_idempotency_key: raw.decision_idempotency_key.clone(),
                decided_at_epoch_seconds: raw.decided_at_epoch_seconds,
                decision_state: materialization.decision_state.clone(),
                current_policy: materialization.current_policy.clone(),
                evaluation_seal: materialization.evaluation_seal.clone(),
                derived_closure: materialization.derived_closure.clone(),
                derived: materialization.derived.clone(),
                updated_at: parse_datetime(
                    "generation append proposal update timestamp",
                    &raw.updated_at,
                )?,
            };
            validate_generation_attempt_proposal_decision_commit(&commit)?;
            let (expected_materialization_json, expected_materialization_sha256) =
                generation_attempt_decision_materialization(&commit)?;
            let (expected_evidence_json, expected_evidence_sha256) =
                generation_attempt_decision_evidence(&commit, &expected_materialization_sha256)?;
            if expected_materialization_json != raw.materialization_json
                || expected_materialization_sha256 != raw.materialization_sha256
                || expected_evidence_json != raw.decision_evidence_json
                || expected_evidence_sha256 != raw.decision_evidence_sha256
            {
                return Err(storage_corrupted(
                    "generation proposal decision evidence is invalid",
                ));
            }
            let (decision_event_id, decision_event_sha256) =
                if let Some(derived) = materialization.derived.as_ref() {
                    let user_action = InteractionEvent::UserAction {
                        action_id: materialization
                            .decision_state
                            .proposals
                            .iter()
                            .find(|record| record.id == proposal_record_id)
                            .map(|record| record.proposal_id.clone())
                            .ok_or_else(|| {
                                storage_corrupted(
                                    "generation approval decision lost its proposal identity",
                                )
                            })?,
                    };
                    let event_fingerprint = encode_json(
                        "generation proposal decision event",
                        &(
                            "lorepia.generation-proposal-decision-event.v1",
                            generation_id,
                            &proposal_record_id,
                            &user_action,
                            derived,
                        ),
                        MAX_STATE_JSON_BYTES,
                    )?;
                    (
                        Some(derived.event_id.clone()),
                        Some(
                            Sha256Digest::parse(sha256_hex(event_fingerprint.as_bytes()))
                                .map_err(CoreError::invalid)?,
                        ),
                    )
                } else {
                    (None, None)
                };
            if raw.decision_event_id != decision_event_id
                || raw.decision_event_sha256.as_deref()
                    != decision_event_sha256.as_ref().map(Sha256Digest::as_str)
            {
                return Err(storage_corrupted(
                    "generation proposal decision event evidence is invalid",
                ));
            }
            Ok(GenerationAttemptAppendDecision {
                proposal_record_id,
                expected_proposal_revision: 1,
                decision_event_id,
                decision_event_sha256,
                decided_at_epoch_seconds: raw.decided_at_epoch_seconds,
                updated_at: commit.updated_at,
                materialization,
            })
        })
        .collect()
}

fn replay_generation_attempt_append_decision(
    transaction: &Transaction<'_>,
    generation_id: &GenerationId,
    key: &InteractionStateKey,
    decision: &GenerationAttemptAppendDecision,
) -> CoreResult<()> {
    let proposal = require_pending_proposal(
        transaction,
        &decision.proposal_record_id,
        decision.expected_proposal_revision,
        decision.decided_at_epoch_seconds,
    )?;
    let current = require_state_for_key(transaction, key)?;
    if proposal.interaction_state_id != current.id {
        return Err(storage_corrupted(
            "generation proposal materialized into another interaction state",
        ));
    }
    let terminal_status = match decision.materialization.decision {
        GenerationAttemptProposalDecision::Approve => InteractionProposalStatus::Approved,
        GenerationAttemptProposalDecision::Reject => InteractionProposalStatus::Rejected,
        GenerationAttemptProposalDecision::Expire => InteractionProposalStatus::Expired,
    };
    let expected_decision_state = derive_decision_state(
        &current.state,
        &proposal.record.id,
        terminal_status,
        decision.decided_at_epoch_seconds,
    )?;
    if expected_decision_state != decision.materialization.decision_state {
        return Err(storage_corrupted(
            "generation proposal decision state cannot be replayed from its pending record",
        ));
    }
    write_state_document_only(
        transaction,
        &current.id,
        current.revision,
        &decision.materialization.decision_state,
        decision.updated_at,
    )?;
    bump_normalized_state_revisions(
        transaction,
        &current.id,
        decision.materialization.decision_state.revision,
    )?;
    let terminal = transition_proposal_status(
        transaction,
        &proposal,
        terminal_status,
        decision.decided_at_epoch_seconds,
        decision.materialization.decision_state.revision,
    )?;

    match decision.materialization.decision {
        GenerationAttemptProposalDecision::Approve => {
            let current_policy = decision
                .materialization
                .current_policy
                .as_ref()
                .ok_or_else(|| {
                    storage_corrupted("generation approval materialization is missing its policy")
                })?;
            let derived = decision.materialization.derived.as_ref().ok_or_else(|| {
                storage_corrupted("generation approval materialization is missing its UserAction")
            })?;
            if current_policy != &proposal.origin_policy || &derived.policy != current_policy {
                return Err(storage_corrupted(
                    "generation approval materialization policy changed",
                ));
            }
            validate_derived_event_commit(&decision.materialization.decision_state, derived)?;
            let closure = decision
                .materialization
                .derived_closure
                .as_ref()
                .ok_or_else(|| {
                    storage_corrupted("generation approval materialization is missing its closure")
                })?;
            let root = closure
                .transitions
                .first()
                .ok_or_else(|| storage_corrupted("generation approval closure has no root"))?;
            if root.event_id != derived.event_id
                || root.event
                    != (InteractionEvent::UserAction {
                        action_id: proposal.record.proposal_id.clone(),
                    })
                || root.policy != derived.policy
                || root.next_state != derived.next_state
                || root.knowledge != derived.knowledge
                || root.action_results != derived.action_results
                || root.effects != derived.effects
                || root.derived_events != derived.derived_events
                || root.proposals != derived.proposals
            {
                return Err(storage_corrupted(
                    "generation approval root differs from its closed materialization",
                ));
            }
            let decision_knowledge = read_knowledge_bindings(transaction, &current.id)?;
            materialize_generation_attempt_closed_closure(
                transaction,
                generation_id,
                key,
                closure,
                &decision.materialization.decision_state,
                &decision_knowledge,
                &derived.idempotency_key,
                false,
                derived.created_at,
            )?;
            mark_proposal_dispatched(
                transaction,
                &terminal,
                decision.decided_at_epoch_seconds,
                closure.final_state.revision,
            )?;
        }
        GenerationAttemptProposalDecision::Reject | GenerationAttemptProposalDecision::Expire => {
            if decision.materialization.current_policy.is_some()
                || decision.materialization.derived.is_some()
            {
                return Err(storage_corrupted(
                    "generation rejection or expiry unexpectedly dispatches an event",
                ));
            }
        }
    }
    Ok(())
}

fn require_no_pending_derived_predecessor(
    connection: &Connection,
    key: &InteractionStateKey,
) -> CoreResult<()> {
    let blocked = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM interaction_derived_event_outbox AS occurrence
                 WHERE occurrence.conversation_id = ?1
                   AND occurrence.branch_id = ?2
                   AND occurrence.status != 'acknowledged'
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id = occurrence.occurrence_id
                   )
             )",
            params![key.conversation_id.0.as_str(), key.branch_id.0.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if blocked {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "a pending derived interaction occurrence must be drained first",
            true,
        ));
    }
    Ok(())
}

fn require_no_pending_derived_predecessor_through(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    boundary_revision: u64,
) -> CoreResult<()> {
    let blocked = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM interaction_derived_event_outbox AS occurrence
                 WHERE occurrence.conversation_id = ?1
                   AND occurrence.branch_id = ?2
                   AND occurrence.parent_resulting_state_revision <= ?3
                   AND occurrence.status != 'acknowledged'
                   AND NOT EXISTS (
                       SELECT 1
                       FROM interaction_derived_event_quarantines AS quarantine
                       WHERE quarantine.occurrence_id = occurrence.occurrence_id
                   )
             )",
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                i64_from_u64(
                    "generation interaction boundary revision",
                    boundary_revision
                )?,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if blocked {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "a predecessor derived interaction occurrence must be drained first",
            true,
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_derived_event_writes(
    connection: &Connection,
    policy: &InteractionPolicySnapshot,
    action_results: &[InteractionActionResultWrite],
    effects: &[InteractionEffect],
    derived_events: &[InteractionDerivedEventWrite],
) -> CoreResult<()> {
    if derived_events.len() > MAX_EFFECTS_PER_EVENT {
        return Err(CoreError::invalid(
            "interaction derived-event count exceeds the per-event limit",
        ));
    }
    let policy_revisions = policy
        .rule_sets
        .iter()
        .map(|revision| revision.revision_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut represented_effects = BTreeSet::new();
    for derived in derived_events {
        if !policy_revisions.contains(derived.source_set_revision_id.as_str()) {
            return Err(CoreError::invalid(
                "interaction derived event references a rule set outside the event policy",
            ));
        }
        if !action_results.iter().any(|result| {
            result.set_revision_id == derived.source_set_revision_id
                && result.rule_id == derived.source_rule_id
                && result.action_ordinal == derived.source_action_ordinal
                && result.status == InteractionActionResultStatus::Applied
        }) {
            return Err(CoreError::invalid(
                "interaction derived event has no exact applied source action",
            ));
        }
        let action_json = connection
            .query_row(
                "SELECT payload_json
                 FROM interaction_actions
                 WHERE set_revision_id = ?1 AND rule_id = ?2 AND ordinal = ?3",
                params![
                    derived.source_set_revision_id,
                    derived.source_rule_id.as_str(),
                    i64::from(derived.source_action_ordinal),
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::invalid("interaction derived event source action does not exist")
            })?;
        let action: InteractionAction = decode_json(
            "interaction derived event source action",
            &action_json,
            MAX_EVENT_JSON_BYTES,
        )?;
        if interaction_action_sha256(&action)? != derived.source_action_sha256 {
            return Err(CoreError::invalid(
                "interaction derived event source action digest is stale or invalid",
            ));
        }
        let effect_index = usize::try_from(derived.source_effect_ordinal)
            .map_err(|_| CoreError::invalid("derived effect ordinal overflowed"))?;
        let effect = effects.get(effect_index).ok_or_else(|| {
            CoreError::invalid("interaction derived event source effect does not exist")
        })?;
        let matches = match (&derived.event, effect) {
            (
                InteractionEvent::VariableChanged { variable },
                InteractionEffect::VariableSet { target, .. },
            ) => variable == target,
            (
                InteractionEvent::KnowledgeActivated {
                    entry_id: event_entry,
                },
                InteractionEffect::KnowledgeActivated {
                    entry_id: effect_entry,
                },
            ) => event_entry == effect_entry,
            _ => false,
        };
        let action_matches = match (&derived.event, &action) {
            (
                InteractionEvent::VariableChanged { variable },
                InteractionAction::SetVariable { target, .. }
                | InteractionAction::IncrementVariable { target, .. }
                | InteractionAction::RollDice {
                    target: Some(target),
                    ..
                },
            ) => variable == target,
            (
                InteractionEvent::KnowledgeActivated {
                    entry_id: event_entry,
                },
                InteractionAction::ActivateKnowledge {
                    entry_id: action_entry,
                },
            ) => event_entry == action_entry,
            _ => false,
        };
        if !matches || !action_matches || !represented_effects.insert(derived.source_effect_ordinal)
        {
            return Err(CoreError::invalid(
                "interaction derived event does not uniquely match its source effect",
            ));
        }
    }
    let required = effects
        .iter()
        .enumerate()
        .filter_map(|(ordinal, effect)| {
            matches!(
                effect,
                InteractionEffect::VariableSet { .. }
                    | InteractionEffect::KnowledgeActivated { .. }
            )
            .then(|| u32::try_from(ordinal).ok())
            .flatten()
        })
        .collect::<BTreeSet<_>>();
    if represented_effects != required {
        return Err(CoreError::invalid(
            "every state-changing interaction effect requires exact derived-event evidence",
        ));
    }
    Ok(())
}

struct DerivedEventOutboxWrite<'a> {
    key: &'a InteractionStateKey,
    event: &'a InteractionEvent,
    policy: &'a InteractionPolicySnapshot,
    evaluation_seal: Option<&'a InteractionEvaluationSeal>,
    deterministic_seed: Option<u64>,
    effects: &'a [InteractionEffect],
    derived_events: &'a [InteractionDerivedEventWrite],
    event_id: &'a str,
    parent_resulting_state_revision: u64,
    payload_json: &'a str,
    created_at: DateTime<Utc>,
    chain_parent: Option<DerivedChainParent<'a>>,
}

fn write_derived_event_outbox(
    transaction: &Transaction<'_>,
    write: &DerivedEventOutboxWrite<'_>,
) -> CoreResult<()> {
    if write.derived_events.is_empty() {
        return Ok(());
    }
    let evaluation_seal = write.evaluation_seal.ok_or_else(|| {
        CoreError::invalid("derived interaction outbox requires an evaluation seal")
    })?;
    let parent_deterministic_seed = write.deterministic_seed.ok_or_else(|| {
        CoreError::invalid("derived interaction outbox requires a parent deterministic seed")
    })?;
    let (Some(evaluation_seal_json), Some(evaluation_seal_sha256), 1) =
        encode_interaction_evaluation_authority(
            write.policy,
            Some(evaluation_seal),
            Some(parent_deterministic_seed),
        )?
    else {
        return Err(CoreError::internal(
            "sealed derived interaction authority encoded as legacy v0",
        ));
    };
    let payload: StoredEventPayload = decode_json(
        "interaction parent event payload",
        write.payload_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    if payload
        .evaluation_seal_sha256
        .as_ref()
        .map(Sha256Digest::as_str)
        != Some(evaluation_seal_sha256.as_str())
        || payload.deterministic_seed != Some(parent_deterministic_seed)
    {
        return Err(storage_corrupted(
            "derived interaction parent event payload has different evaluation authority",
        ));
    }
    let parent_event_commit_sha256 =
        Sha256Digest::parse(payload.commit_sha256).map_err(CoreError::invalid)?;
    let current_event_sha256 = interaction_event_sha256(write.event)?;
    let (chain_id, root_event_id, parent_occurrence_id, depth, mut visited) =
        if let Some(parent) = write.chain_parent.as_ref() {
            let occurrence = parent.occurrence;
            if occurrence.event != *write.event
                || occurrence.parent_event_id == write.event_id
                || occurrence.conversation_id != write.key.conversation_id
                || occurrence.branch_id != write.key.branch_id
                || occurrence.policy != *write.policy
                || occurrence.evaluation_seal != *evaluation_seal
                || occurrence.deterministic_seed != parent_deterministic_seed
                || occurrence.event_sha256 != current_event_sha256
            {
                return Err(storage_corrupted(
                    "derived interaction parent authority is inconsistent",
                ));
            }
            let mut visited = occurrence.visited_event_sha256s.clone();
            if visited.contains(&current_event_sha256) {
                return Err(storage_corrupted(
                    "a cycle-suppressed derived occurrence was materialized",
                ));
            }
            visited.push(current_event_sha256.clone());
            (
                occurrence.chain_id.clone(),
                occurrence.root_event_id.clone(),
                Some(occurrence.occurrence_id.clone()),
                occurrence.depth.checked_add(1).ok_or_else(|| {
                    CoreError::invalid("derived interaction chain depth overflowed")
                })?,
                visited,
            )
        } else {
            let chain_hash = sha256_hex(
                encode_json(
                    "derived interaction chain identity",
                    &("lorepia.interaction-derived-chain.v1", write.event_id),
                    MAX_AUDIT_JSON_BYTES,
                )?
                .as_bytes(),
            );
            (
                format!("interaction-derived-chain-{chain_hash}"),
                write.event_id.to_owned(),
                None,
                1,
                vec![current_event_sha256],
            )
        };
    let expected_visited_len = usize::try_from(depth)
        .map_err(|_| CoreError::invalid("derived interaction depth overflowed"))?;
    if visited.len() != expected_visited_len {
        return Err(storage_corrupted(
            "derived interaction visited-set differs from its child depth",
        ));
    }
    visited.sort();
    visited.dedup();
    if visited.len() != expected_visited_len {
        return Err(storage_corrupted(
            "derived interaction visited-set contains duplicate ancestry",
        ));
    }
    if depth > MAX_INTERACTION_DERIVED_CHAIN_DEPTH {
        let mut cycle_limited = BTreeMap::new();
        let mut depth_limited = BTreeMap::new();
        for derived in write.derived_events {
            let event_sha256 = interaction_event_sha256(&derived.event)?;
            let target = if visited.contains(&event_sha256) {
                &mut cycle_limited
            } else {
                &mut depth_limited
            };
            increment_derived_guard_count(target, event_sha256)?;
        }
        for (guard_kind, guards) in [("cycle", cycle_limited), ("depth_limit", depth_limited)] {
            for (_, (candidate_event_sha256, suppressed_count)) in guards {
                write_derived_guard_audit(
                    transaction,
                    DerivedGuardAuditWrite {
                        chain_id: &chain_id,
                        root_event_id: &root_event_id,
                        parent_event_id: write.event_id,
                        parent_occurrence_id: parent_occurrence_id.as_deref(),
                        guard_kind,
                        candidate_event_sha256: Some(&candidate_event_sha256),
                        suppressed_count,
                        created_at: write.created_at,
                    },
                )?;
            }
        }
        return Ok(());
    }
    let visited_json = encode_json(
        "derived interaction visited events",
        &visited,
        MAX_AUDIT_JSON_BYTES,
    )?;
    let (_, policy_json, policy_sha256) = encode_interaction_policy(write.policy)?;
    let mut next_ordinal = transaction
        .query_row(
            "SELECT COALESCE(MAX(chain_ordinal), 0)
             FROM interaction_derived_event_outbox WHERE chain_id = ?1",
            [&chain_id],
            |row| row.get::<_, u32>(0),
        )
        .map_err(storage_db_error)?;
    let mut cycle_limited = BTreeMap::new();
    let mut count_limited = 0_u32;
    for derived in write.derived_events {
        let event_sha256 = interaction_event_sha256(&derived.event)?;
        if visited.contains(&event_sha256) {
            increment_derived_guard_count(&mut cycle_limited, event_sha256)?;
            continue;
        }
        if next_ordinal >= MAX_INTERACTION_DERIVED_CHAIN_EVENTS {
            count_limited = count_limited.checked_add(1).ok_or_else(|| {
                CoreError::invalid("derived interaction suppressed count overflowed")
            })?;
            continue;
        }
        next_ordinal = next_ordinal
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("derived interaction chain ordinal overflowed"))?;
        let effect_index = usize::try_from(derived.source_effect_ordinal)
            .map_err(|_| CoreError::invalid("derived interaction effect ordinal overflowed"))?;
        let effect = write
            .effects
            .get(effect_index)
            .ok_or_else(|| CoreError::invalid("derived interaction source effect disappeared"))?;
        let source_effect_json = encode_json(
            "derived interaction source effect",
            effect,
            MAX_EVENT_JSON_BYTES,
        )?;
        let source_effect_sha256 = sha256_hex(source_effect_json.as_bytes());
        let event_argument_json =
            interaction_event_argument_json(&derived.event)?.ok_or_else(|| {
                CoreError::internal("derived interaction event has no canonical argument")
            })?;
        let occurrence_hash = sha256_hex(
            encode_json(
                "derived interaction occurrence identity",
                &(
                    "lorepia.interaction-derived-occurrence.v1",
                    &chain_id,
                    write.event_id,
                    derived.source_effect_ordinal,
                    &event_sha256,
                    &derived.source_action_sha256,
                    &evaluation_seal_sha256,
                    derived.deterministic_seed,
                ),
                MAX_AUDIT_JSON_BYTES,
            )?
            .as_bytes(),
        );
        let occurrence_id = format!("interaction-derived-{occurrence_hash}");
        transaction
            .execute(
                "INSERT INTO interaction_derived_event_outbox
                 (occurrence_id, chain_id, root_event_id, parent_event_id,
                  parent_occurrence_id, conversation_id, branch_id, depth,
                  chain_ordinal, source_effect_ordinal,
                  parent_event_commit_sha256, parent_resulting_state_revision,
                  source_effect_sha256,
                  source_action_sha256, source_set_revision_id, source_rule_id,
                  source_action_ordinal, event_kind, event_argument_json,
                  event_sha256, visited_event_sha256s_json, policy_json,
                  policy_sha256, evaluation_seal_json,
                  evaluation_seal_sha256, evaluation_seal_version,
                  deterministic_seed_hex, occurred_at, available_at, status,
                  delivery_attempts, lease_until, acknowledged_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                         ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21,
                         ?22, ?23, ?24, ?25, 1, ?26, ?27, ?27, 'pending',
                         0, NULL, NULL, ?27)",
                params![
                    occurrence_id,
                    chain_id,
                    root_event_id,
                    write.event_id,
                    parent_occurrence_id,
                    write.key.conversation_id.0.as_str(),
                    write.key.branch_id.0.as_str(),
                    i64::from(depth),
                    i64::from(next_ordinal),
                    i64::from(derived.source_effect_ordinal),
                    parent_event_commit_sha256.as_str(),
                    i64_from_u64(
                        "derived parent resulting state revision",
                        write.parent_resulting_state_revision,
                    )?,
                    source_effect_sha256,
                    derived.source_action_sha256.as_str(),
                    derived.source_set_revision_id,
                    derived.source_rule_id.as_str(),
                    i64::from(derived.source_action_ordinal),
                    interaction_event_kind(&derived.event),
                    event_argument_json,
                    event_sha256.as_str(),
                    visited_json,
                    policy_json,
                    policy_sha256,
                    evaluation_seal_json,
                    evaluation_seal_sha256,
                    encode_u64_hex(derived.deterministic_seed),
                    write.created_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
    }
    for (_, (candidate_event_sha256, suppressed_count)) in cycle_limited {
        write_derived_guard_audit(
            transaction,
            DerivedGuardAuditWrite {
                chain_id: &chain_id,
                root_event_id: &root_event_id,
                parent_event_id: write.event_id,
                parent_occurrence_id: parent_occurrence_id.as_deref(),
                guard_kind: "cycle",
                candidate_event_sha256: Some(&candidate_event_sha256),
                suppressed_count,
                created_at: write.created_at,
            },
        )?;
    }
    if count_limited > 0 {
        write_derived_guard_audit(
            transaction,
            DerivedGuardAuditWrite {
                chain_id: &chain_id,
                root_event_id: &root_event_id,
                parent_event_id: write.event_id,
                parent_occurrence_id: parent_occurrence_id.as_deref(),
                guard_kind: "count_limit",
                candidate_event_sha256: None,
                suppressed_count: count_limited,
                created_at: write.created_at,
            },
        )?;
    }
    Ok(())
}

struct DerivedGuardAuditWrite<'a> {
    chain_id: &'a str,
    root_event_id: &'a str,
    parent_event_id: &'a str,
    parent_occurrence_id: Option<&'a str>,
    guard_kind: &'a str,
    candidate_event_sha256: Option<&'a Sha256Digest>,
    suppressed_count: u32,
    created_at: DateTime<Utc>,
}

fn increment_derived_guard_count(
    counts: &mut BTreeMap<String, (Sha256Digest, u32)>,
    candidate: Sha256Digest,
) -> CoreResult<()> {
    let key = candidate.as_str().to_owned();
    let entry = counts.entry(key).or_insert((candidate, 0));
    entry.1 = entry
        .1
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("derived interaction guard count overflowed"))?;
    Ok(())
}

#[derive(Serialize)]
struct DerivedGuardEvidence<'a> {
    schema_version: u32,
    chain_id: &'a str,
    root_event_id: &'a str,
    parent_event_id: &'a str,
    parent_occurrence_id: Option<&'a str>,
    guard_kind: &'a str,
    candidate_event_sha256: Option<&'a Sha256Digest>,
    suppressed_count: u32,
}

fn write_derived_guard_audit(
    transaction: &Transaction<'_>,
    write: DerivedGuardAuditWrite<'_>,
) -> CoreResult<()> {
    let evidence_json = encode_json(
        "derived interaction guard evidence",
        &DerivedGuardEvidence {
            schema_version: 1,
            chain_id: write.chain_id,
            root_event_id: write.root_event_id,
            parent_event_id: write.parent_event_id,
            parent_occurrence_id: write.parent_occurrence_id,
            guard_kind: write.guard_kind,
            candidate_event_sha256: write.candidate_event_sha256,
            suppressed_count: write.suppressed_count,
        },
        MAX_AUDIT_JSON_BYTES,
    )?;
    let evidence_sha256 = sha256_hex(evidence_json.as_bytes());
    let audit_id = format!("interaction-derived-guard-{evidence_sha256}");
    transaction
        .execute(
            "INSERT INTO interaction_derived_event_guard_audit
             (id, chain_id, root_event_id, parent_event_id,
              parent_occurrence_id, guard_kind, candidate_event_sha256,
              suppressed_count, evidence_json, evidence_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                audit_id,
                write.chain_id,
                write.root_event_id,
                write.parent_event_id,
                write.parent_occurrence_id,
                write.guard_kind,
                write.candidate_event_sha256.map(Sha256Digest::as_str),
                i64::from(write.suppressed_count),
                evidence_json,
                evidence_sha256,
                write.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_interaction_policy_revisions(
    connection: &Connection,
    policy: &InteractionPolicySnapshot,
) -> CoreResult<()> {
    validate_policy_shape(policy)?;
    if let Some(module_plan_sha256) = policy.module_plan_sha256.as_deref() {
        let exists = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM module_activation_plans
                     WHERE plan_sha256 = ?1
                     UNION ALL
                     SELECT 1 FROM applied_module_runtime_plans
                     WHERE applied_plan_sha256 = ?1
                 )",
                [module_plan_sha256],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !exists {
            return Err(CoreError::invalid(
                "interaction policy module plan does not exist",
            ));
        }
    }
    validate_interaction_policy_rule_set_revisions(connection, policy)
}

fn validate_interaction_policy_rule_set_revisions(
    connection: &Connection,
    policy: &InteractionPolicySnapshot,
) -> CoreResult<()> {
    validate_policy_shape(policy)?;
    for revision in &policy.rule_sets {
        let stored = connection
            .query_row(
                "SELECT revision.interaction_rule_set_id,
                        content.document_sha256
                 FROM interaction_rule_set_revisions AS revision
                 JOIN content_revisions AS content
                   ON content.id = revision.revision_id
                  AND content.object_id = revision.interaction_rule_set_id
                 WHERE revision.revision_id = ?1",
                [&revision.revision_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::invalid("interaction policy rule-set revision does not exist")
            })?;
        if stored.0 != revision.rule_set_id.as_str() || stored.1 != revision.sha256 {
            return Err(CoreError::invalid(
                "interaction policy rule-set revision fingerprint changed",
            ));
        }
    }
    Ok(())
}

fn validate_generation_attempt_binding(
    connection: &Connection,
    key: &InteractionStateKey,
    event: &InteractionEvent,
    generation_attempt_id: Option<&GenerationId>,
    generation_append_materialization: bool,
) -> CoreResult<()> {
    let Some(generation_attempt_id) = generation_attempt_id else {
        if matches!(
            event,
            InteractionEvent::BeforeGeneration | InteractionEvent::AfterGeneration
        ) {
            return Err(CoreError::invalid(
                "generation interaction event is missing its attempt",
            ));
        }
        return Ok(());
    };
    let raw = connection
        .query_row(
            "SELECT conversation_id, proposed_branch_id, status
             FROM generation_attempt_intents
             WHERE generation_id = ?1",
            [generation_attempt_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| CoreError::invalid("generation attempt does not exist"))?;
    let valid_status_and_authority = match event {
        InteractionEvent::BeforeGeneration => {
            generation_append_materialization && raw.2 == "dispatch_ready"
        }
        InteractionEvent::AfterGeneration => {
            matches!(raw.2.as_str(), "running" | "completed")
                && terminal_after_generation_authority_exists(
                    connection,
                    generation_attempt_id,
                    key,
                )?
        }
        _ => false,
    };
    if raw.0 != key.conversation_id.0 || raw.1 != key.branch_id.0 || !valid_status_and_authority {
        return Err(revision_conflict(
            "generation attempt does not match the interaction event room or status",
        ));
    }
    Ok(())
}

fn terminal_after_generation_authority_exists(
    connection: &Connection,
    generation_attempt_id: &GenerationId,
    key: &InteractionStateKey,
) -> CoreResult<bool> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM generations AS generation
                 JOIN core_lifecycle_outbox AS occurrence
                   ON occurrence.occurrence_id = ?1
                  AND occurrence.event_kind = 'after_generation'
                  AND occurrence.generation_id = generation.id
                  AND occurrence.conversation_id = generation.conversation_id
                  AND occurrence.branch_id = generation.branch_id
                 WHERE generation.id = ?2
                   AND generation.conversation_id = ?3
                   AND generation.branch_id = ?4
                   AND generation.status != 'running'
                   AND generation.finished_at IS NOT NULL
             )",
            params![
                format!("after-generation:{}", generation_attempt_id.0),
                generation_attempt_id.0.as_str(),
                key.conversation_id.0.as_str(),
                key.branch_id.0.as_str(),
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)
}

fn validate_action_results_belong_to_policy(
    action_results: &[InteractionActionResultWrite],
    policy: &InteractionPolicySnapshot,
) -> CoreResult<()> {
    for result in action_results {
        if !policy
            .rule_sets
            .iter()
            .any(|revision| revision.revision_id == result.set_revision_id)
        {
            return Err(CoreError::invalid(
                "interaction action result is absent from the committed policy",
            ));
        }
    }
    Ok(())
}

fn write_interaction_policy_rule_sets(
    transaction: &Transaction<'_>,
    event_id: &str,
    policy: &InteractionPolicySnapshot,
) -> CoreResult<()> {
    for (ordinal, revision) in policy.rule_sets.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO interaction_event_policy_rule_sets
                 (event_id, ordinal, rule_set_id, rule_set_revision_id,
                  revision_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event_id,
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many policy rule sets"))?,
                    revision.rule_set_id.as_str(),
                    revision.revision_id,
                    revision.sha256,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_action_results(
    transaction: &Transaction<'_>,
    event_id: &str,
    results: &[InteractionActionResultWrite],
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    for (result_ordinal, result) in results.iter().enumerate() {
        let result_json = encode_json(
            "interaction action result",
            &result.result,
            MAX_EVENT_JSON_BYTES,
        )?;
        transaction
            .execute(
                "INSERT INTO interaction_action_results
                 (event_id, set_revision_id, rule_id, action_ordinal,
                  result_ordinal, status, result_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    event_id,
                    result.set_revision_id,
                    result.rule_id.as_str(),
                    i64::from(result.action_ordinal),
                    i64::try_from(result_ordinal)
                        .map_err(|_| CoreError::invalid("too many interaction action results"))?,
                    action_result_status(result.status),
                    result_json,
                    created_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn validate_action_result_sources(
    transaction: &Transaction<'_>,
    event: &InteractionEvent,
    action_results: &[InteractionActionResultWrite],
) -> CoreResult<()> {
    let expected_kind = interaction_event_kind(event);
    for result in action_results {
        let source = transaction
            .query_row(
                "SELECT rule.event_kind, rule.event_argument_json
                 FROM interaction_actions AS action
                 JOIN interaction_rules AS rule
                   ON rule.set_revision_id = action.set_revision_id
                  AND rule.rule_id = action.rule_id
                 WHERE action.set_revision_id = ?1
                   AND action.rule_id = ?2
                   AND action.ordinal = ?3",
                params![
                    result.set_revision_id,
                    result.rule_id.as_str(),
                    i64::from(result.action_ordinal),
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| CoreError::invalid("interaction action result source does not exist"))?;
        if source.0 != expected_kind {
            return Err(CoreError::invalid(
                "interaction action result source does not match the committed event",
            ));
        }
        match source.1 {
            Some(argument_json) => {
                let source_event: InteractionEvent = decode_json(
                    "interaction rule event argument",
                    &argument_json,
                    MAX_AUDIT_JSON_BYTES,
                )?;
                if source_event != *event {
                    return Err(CoreError::invalid(
                        "interaction action result source argument does not match the committed event",
                    ));
                }
            }
            None if event_requires_argument(event) => {
                return Err(storage_corrupted(
                    "argument-bearing interaction rule is missing its event argument",
                ));
            }
            None => {}
        }
    }
    Ok(())
}

fn validate_generation_attempt_proposal_storage_identity(
    generation_id: &GenerationId,
    record: &InteractionProposalRecord,
    domain_proposal_record_id: &InteractionProposalRecordId,
    proposal_review_sha256: &str,
    domain_proposal_review_sha256: &str,
    before_review_sha256: &str,
    storage_identity_version: u32,
) -> CoreResult<()> {
    let expected_domain_record_id = interaction_proposal_record_id(
        &record.rule_set_id,
        &record.rule_id,
        &record.proposal_id,
        record.source_interaction_state_revision,
    )?;
    if domain_proposal_record_id != &expected_domain_record_id {
        return Err(storage_corrupted(
            "generation proposal domain identity is invalid",
        ));
    }
    let expected_storage_id = match storage_identity_version {
        1 => domain_proposal_record_id.clone(),
        2 => generation_attempt_proposal_storage_id(
            generation_id,
            domain_proposal_record_id,
            domain_proposal_review_sha256,
            before_review_sha256,
        )?,
        _ => {
            return Err(storage_corrupted(
                "generation proposal storage identity version is invalid",
            ));
        }
    };
    if record.id != expected_storage_id
        || interaction_proposal_review_sha256(record)? != proposal_review_sha256
    {
        return Err(storage_corrupted(
            "generation proposal storage identity is invalid",
        ));
    }
    let mut domain_record = record.clone();
    domain_record.id = domain_proposal_record_id.clone();
    if interaction_proposal_review_sha256(&domain_record)? != domain_proposal_review_sha256
        || (storage_identity_version == 1
            && proposal_review_sha256 != domain_proposal_review_sha256)
    {
        return Err(storage_corrupted(
            "generation proposal domain review fingerprint is invalid",
        ));
    }
    Ok(())
}

fn validate_generation_attempt_append_proposal_identities(
    connection: &Connection,
    generation_id: &GenerationId,
    proposals: &[InteractionProposalWrite],
) -> CoreResult<()> {
    let durable_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM generation_attempt_proposals
             WHERE generation_id = ?1
               AND origin_aggregate_revision = 1",
            [generation_id.0.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)?;
    if durable_count
        != i64::try_from(proposals.len())
            .map_err(|_| CoreError::invalid("too many generation proposals"))?
    {
        return Err(storage_corrupted(
            "generation append proposal count differs from its reviewed snapshot",
        ));
    }
    for proposal in proposals {
        let durable = connection
            .query_row(
                "SELECT proposal.proposal_review_sha256,
                        proposal.domain_proposal_review_sha256,
                        proposal.domain_proposal_record_id,
                        snapshot.review_sha256,
                        proposal.storage_identity_version
                 FROM generation_attempt_proposals AS proposal
                 JOIN generation_attempt_before_event_snapshots AS snapshot
                   ON snapshot.generation_id = proposal.generation_id
                 WHERE proposal.generation_id = ?1
                   AND proposal.proposal_record_id = ?2
                   AND proposal.origin_aggregate_revision = 1",
                params![generation_id.0.as_str(), proposal.record.id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, u32>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| storage_corrupted("generation append proposal authority is missing"))?;
        if durable.0 != proposal.review_payload_sha256 {
            return Err(storage_corrupted(
                "generation append proposal review differs from its durable authority",
            ));
        }
        validate_generation_attempt_proposal_storage_identity(
            generation_id,
            &proposal.record,
            &InteractionProposalRecordId::from(durable.2),
            &proposal.review_payload_sha256,
            &durable.1,
            &durable.3,
            durable.4,
        )?;
    }
    Ok(())
}

fn generation_attempt_proposal_identity_pairs(
    connection: &Connection,
    generation_id: &GenerationId,
) -> CoreResult<Vec<(InteractionProposalRecordId, InteractionProposalRecordId)>> {
    let raw = {
        let mut statement = connection
            .prepare(
                "SELECT proposal.proposal_record_id,
                        proposal.domain_proposal_record_id,
                        proposal.proposal_record_json,
                        proposal.proposal_record_sha256,
                        proposal.proposal_review_sha256,
                        proposal.domain_proposal_review_sha256,
                        snapshot.review_sha256,
                        proposal.storage_identity_version
                 FROM generation_attempt_proposals AS proposal
                 JOIN generation_attempt_before_event_snapshots AS snapshot
                   ON snapshot.generation_id = proposal.generation_id
                 WHERE proposal.generation_id = ?1
                 ORDER BY proposal.ordinal, proposal.proposal_record_id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([generation_id.0.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, u32>(7)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let mut pairs = Vec::with_capacity(raw.len());
    for (
        storage_id,
        domain_id,
        record_json,
        record_sha256,
        review_sha256,
        domain_sha256,
        before_review_sha256,
        storage_identity_version,
    ) in raw
    {
        let record: InteractionProposalRecord = decode_json(
            "generation proposal identity record",
            &record_json,
            MAX_EVENT_JSON_BYTES,
        )?;
        if sha256_hex(record_json.as_bytes()) != record_sha256 || record_sha256 != review_sha256 {
            return Err(storage_corrupted(
                "generation proposal identity record fingerprint is invalid",
            ));
        }
        let storage_id = InteractionProposalRecordId::from(storage_id);
        let domain_id = InteractionProposalRecordId::from(domain_id);
        if record.id != storage_id {
            return Err(storage_corrupted(
                "generation proposal identity row differs from its record",
            ));
        }
        validate_generation_attempt_proposal_storage_identity(
            generation_id,
            &record,
            &domain_id,
            &review_sha256,
            &domain_sha256,
            &before_review_sha256,
            storage_identity_version,
        )?;
        pairs.push((storage_id, domain_id));
    }
    let storage_ids = pairs
        .iter()
        .map(|(storage_id, _)| storage_id.as_str())
        .collect::<BTreeSet<_>>();
    let domain_ids = pairs
        .iter()
        .map(|(_, domain_id)| domain_id.as_str())
        .collect::<BTreeSet<_>>();
    if storage_ids.len() != pairs.len() || domain_ids.len() != pairs.len() {
        return Err(storage_corrupted(
            "generation proposal identity mapping is not one-to-one",
        ));
    }
    Ok(pairs)
}

fn remap_generation_attempt_state_proposal_ids(
    connection: &Connection,
    generation_id: &GenerationId,
    state: &InteractionState,
    to_domain: bool,
) -> CoreResult<InteractionState> {
    let pairs = generation_attempt_proposal_identity_pairs(connection, generation_id)?;
    let mut remapped = state.clone();
    for (storage_id, domain_id) in pairs {
        let (source_id, target_id) = if to_domain {
            (storage_id, domain_id)
        } else {
            (domain_id, storage_id)
        };
        let mut matches = remapped
            .proposals
            .iter_mut()
            .filter(|proposal| proposal.id == source_id)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(storage_corrupted(
                "generation proposal identity mapping is not total over its aggregate state",
            ));
        }
        matches[0].id = target_id;
    }
    validate_state(&remapped)?;
    Ok(remapped)
}

fn action_result_status(status: InteractionActionResultStatus) -> &'static str {
    match status {
        InteractionActionResultStatus::Proposed => "proposed",
        InteractionActionResultStatus::Applied => "applied",
        InteractionActionResultStatus::Skipped => "skipped",
        InteractionActionResultStatus::Failed => "failed",
    }
}

fn encode_json<T: Serialize>(label: &str, value: &T, max_bytes: usize) -> CoreResult<String> {
    let json = serde_json::to_string(value)
        .map_err(|error| CoreError::invalid(format!("{label} cannot be serialized: {error}")))?;
    validate_json(label, &json, max_bytes)?;
    Ok(json)
}

fn decode_json<T: for<'de> Deserialize<'de>>(
    label: &str,
    json: &str,
    max_bytes: usize,
) -> CoreResult<T> {
    validate_json(label, json, max_bytes).map_err(|error| {
        storage_corrupted(format!(
            "{label} violates storage bounds: {}",
            error.message
        ))
    })?;
    serde_json::from_str(json)
        .map_err(|error| storage_corrupted(format!("{label} is invalid: {error}")))
}

fn validate_json(label: &str, json: &str, max_bytes: usize) -> CoreResult<()> {
    if json.len() > max_bytes {
        return Err(CoreError::invalid(format!(
            "{label} exceeds its {max_bytes}-byte storage limit"
        )));
    }
    let root: Value = serde_json::from_str(json)
        .map_err(|error| CoreError::invalid(format!("{label} is invalid JSON: {error}")))?;
    let mut pending = vec![(&root, 0_usize)];
    let mut nodes = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_JSON_NODES || depth > MAX_JSON_DEPTH {
            return Err(CoreError::invalid(format!(
                "{label} exceeds JSON depth or node limits"
            )));
        }
        match node {
            Value::Object(object) => {
                for (key, child) in object {
                    if is_forbidden_secret_key(key) {
                        return Err(CoreError::invalid(format!(
                            "{label} contains a raw credential field"
                        )));
                    }
                    pending.push((child, depth.saturating_add(1)));
                }
            }
            Value::Array(array) => {
                pending.extend(array.iter().map(|child| (child, depth.saturating_add(1))));
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_forbidden_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "api_key"
            | "authorization"
            | "password"
            | "private_key"
            | "client_secret"
            | "access_token"
            | "refresh_token"
            | "credential"
    )
}

fn parse_datetime(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| storage_corrupted(format!("{label} is invalid: {error}")))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn i64_from_u64(label: &str, value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid(format!("{label} exceeds SQLite range")))
}

fn u64_from_i64(label: &str, value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| storage_corrupted(format!("{label} is negative")))
}

fn encode_u64_hex(value: u64) -> String {
    format!("{value:016x}")
}

fn decode_u64_hex(label: &str, value: &str) -> CoreResult<u64> {
    if value.len() != 16
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(storage_corrupted(format!(
            "{label} is not canonical lowercase u64 hexadecimal"
        )));
    }
    u64::from_str_radix(value, 16)
        .map_err(|error| storage_corrupted(format!("{label} is invalid: {error}")))
}

fn u32_from_i64(label: &str, value: i64) -> CoreResult<u32> {
    u32::try_from(value).map_err(|_| storage_corrupted(format!("{label} is outside u32 range")))
}

fn not_found(kind: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{kind} was not found"),
        false,
    )
}

fn revision_conflict(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::InvalidInput, message, true)
}

fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests;
