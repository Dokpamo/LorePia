use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CoreError, CoreResult, GenerationId, InteractionEffect, InteractionEvent, InteractionState,
    ResolvedPromptPlan, Sha256Digest, ValidateOrchestration, prompt_local_user_id_sha256,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, ModuleMergeReview, no_applied_module_runtime_plan_sha256,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::{
    MemoryRecordsAtHeadSnapshot, Storage, StoredGenerationAttempt, database::storage_db_error,
    memory_records_at_head_snapshot_sha256,
};

use super::checkpoints::clone_interaction_checkpoint_for_branch_transaction;
use super::event_transactions::{validate_event_collections, validate_policy_shape};
use super::generation_proposal_queries::read_generation_attempt_interaction_aggregate;
use super::generation_review_authority::{
    generation_attempt_before_review_storage_sha256, read_generation_attempt_before_review,
};
use super::replay::{
    materialize_generation_attempt_closed_closure, read_generation_attempt_append_decisions,
    replay_generation_attempt_append_decision,
};
use super::state::{
    read_knowledge_bindings, require_state_for_key, validate_knowledge_bindings, validate_state,
};
use super::types::{
    GenerationAttemptInteractionMaterializationReceipt, InteractionActionResultWrite,
    InteractionDerivedEventWrite, InteractionKnowledgeBinding, InteractionPolicySnapshot,
    InteractionProposalWrite, InteractionStateKey, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES,
    interaction_policy_sha256, interaction_state_snapshot_sha256,
};
use super::{
    decode_json, encode_json, not_found, parse_datetime,
    require_no_pending_derived_predecessor_through, revision_conflict, sha256_hex,
    storage_corrupted, u64_from_i64, validate_derived_event_writes,
    validate_generation_attempt_append_proposal_identities,
};

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
