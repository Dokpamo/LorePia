use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    CoreError, CoreResult, GenerationId, InteractionEvent, InteractionProposalRecordId,
    Sha256Digest,
};
use lorepia_orchestration::{AppliedModuleRuntimePlan, no_applied_module_runtime_plan_sha256};
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::{
    GenerationApprovalEvidence, GenerationAttemptDerivedClosure, GenerationBeforeEventEvidence,
    InteractionEvaluationSeal, MemoryRecordsAtHeadSnapshot, Storage, database::storage_db_error,
    generation_approval_evidence_sha256, generation_attempt_derived_closure_sha256,
    generation_before_event_evidence_sha256, interaction_evaluation_seal_sha256,
    memory_records_at_head_snapshot_sha256,
};

use super::checkpoints::read_generation_attempt_review_boundary;
use super::event_transactions::{validate_new_event_collections, validate_policy_shape};
use super::generation_review::{
    NamespacedGenerationAttemptBeforeReview, PreparedGenerationAttemptBeforeReview,
};
use super::proposal_records::{
    interaction_proposal_record_id, validate_existing_proposals_unchanged, validate_proposal_writes,
};
use super::state::{
    validate_knowledge_bindings, validate_nonempty_id, validate_review_hash, validate_state,
};
use super::types::{
    GenerationAttemptBeforeReviewCommit, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES,
    StoredGenerationAttemptBeforeReview, interaction_proposal_review_sha256,
    interaction_state_snapshot_sha256,
};
use super::{
    decode_json, encode_json, is_sha256, not_found, parse_datetime,
    require_no_pending_derived_predecessor_through, revision_conflict, sha256_hex,
    storage_corrupted, u64_from_i64, validate_action_result_sources,
    validate_action_results_belong_to_policy, validate_derived_event_writes,
    validate_interaction_policy_rule_set_revisions,
};

pub(super) fn generation_attempt_proposal_storage_id(
    generation_id: &GenerationId,
    domain_record_id: &InteractionProposalRecordId,
    domain_review_sha256: &str,
    before_review_sha256: &str,
) -> CoreResult<InteractionProposalRecordId> {
    let identity_json = encode_json(
        "generation attempt proposal storage identity",
        &(
            "lorepia.generation-attempt-proposal-record.v1",
            generation_id,
            domain_record_id,
            domain_review_sha256,
            before_review_sha256,
        ),
        MAX_EVENT_JSON_BYTES,
    )?;
    Ok(InteractionProposalRecordId::from(format!(
        "attempt-proposal-{}",
        sha256_hex(identity_json.as_bytes())
    )))
}

pub(super) fn generation_attempt_before_review_storage_sha256(
    generation_id: &GenerationId,
    domain_review_sha256: &str,
) -> CoreResult<String> {
    if !is_sha256(domain_review_sha256) {
        return Err(CoreError::invalid(
            "generation BeforeGeneration domain review hash is invalid",
        ));
    }
    let identity_json = encode_json(
        "generation attempt BeforeGeneration storage identity",
        &(
            "lorepia.generation-attempt-before-review.v2",
            generation_id,
            domain_review_sha256,
        ),
        MAX_EVENT_JSON_BYTES,
    )?;
    Ok(sha256_hex(identity_json.as_bytes()))
}
pub(super) fn validate_generation_attempt_before_review_commit(
    commit: &GenerationAttemptBeforeReviewCommit,
) -> CoreResult<()> {
    validate_generation_attempt_before_review_shape(commit)?;
    validate_generation_attempt_domain_proposal_identities(commit)
}

pub(super) fn validate_generation_attempt_before_review_shape(
    commit: &GenerationAttemptBeforeReviewCommit,
) -> CoreResult<()> {
    validate_nonempty_id("generation attempt id", &commit.generation_id.0)?;
    validate_nonempty_id("generation BeforeGeneration event id", &commit.event_id)?;
    if commit.expected_attempt_revision == 0
        || !is_sha256(&commit.context_checkpoint_sha256)
        || !is_sha256(&commit.review_sha256)
    {
        return Err(CoreError::invalid(
            "generation BeforeGeneration review authority is invalid",
        ));
    }
    validate_state(&commit.previous_state)?;
    validate_state(&commit.next_state)?;
    validate_knowledge_bindings(&commit.previous_state, &commit.previous_knowledge)?;
    validate_knowledge_bindings(&commit.next_state, &commit.knowledge)?;
    validate_policy_shape(&commit.policy)?;
    generation_attempt_derived_closure_sha256(&commit.derived_closure)?;
    let root =
        commit.derived_closure.transitions.first().ok_or_else(|| {
            CoreError::invalid("generation derived closure has no root transition")
        })?;
    for transition in &commit.derived_closure.transitions {
        validate_new_event_collections(
            &transition.action_results,
            &transition.effects,
            &transition.proposals,
        )?;
    }
    if root.event_id != commit.event_id
        || root.event != InteractionEvent::BeforeGeneration
        || root.policy != commit.policy
        || root.evaluation_seal != commit.evaluation_seal
        || root.next_state != commit.next_state
        || root.knowledge != commit.knowledge
        || root.action_results != commit.action_results
        || root.effects != commit.effects
        || root.derived_events != commit.derived_events
    {
        return Err(CoreError::invalid(
            "generation BeforeGeneration root differs from its derived closure",
        ));
    }
    let flattened_proposals = commit
        .derived_closure
        .transitions
        .iter()
        .flat_map(|transition| transition.proposals.iter())
        .map(|proposal| (proposal.record.id.as_str(), proposal))
        .collect::<BTreeMap<_, _>>();
    let commit_proposals = commit
        .proposals
        .iter()
        .map(|proposal| (proposal.record.id.as_str(), proposal))
        .collect::<BTreeMap<_, _>>();
    if flattened_proposals.len() != commit.proposals.len()
        || commit_proposals.len() != commit.proposals.len()
        || flattened_proposals != commit_proposals
    {
        return Err(CoreError::invalid(
            "generation proposal flattening differs from its closure origins",
        ));
    }
    if commit.next_state.revision
        != commit
            .previous_state
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("interaction state revision overflowed"))?
    {
        return Err(CoreError::invalid(
            "generation BeforeGeneration next-state revision is invalid",
        ));
    }
    commit.module_runtime_review.verify().map_err(|error| {
        CoreError::invalid(format!("module runtime review is invalid: {error}"))
    })?;
    if let Some(plan) = &commit.applied_runtime_plan {
        plan.verify().map_err(|error| {
            CoreError::invalid(format!("applied module runtime plan is invalid: {error}"))
        })?;
        if plan.review != commit.module_runtime_review {
            return Err(CoreError::invalid(
                "applied module runtime plan differs from its exact target review",
            ));
        }
    }
    Ok(())
}

fn validate_generation_attempt_domain_proposal_identities(
    commit: &GenerationAttemptBeforeReviewCommit,
) -> CoreResult<()> {
    for proposal in &commit.proposals {
        let record = &proposal.record;
        if record.id
            != interaction_proposal_record_id(
                &record.rule_set_id,
                &record.rule_id,
                &record.proposal_id,
                record.source_interaction_state_revision,
            )?
        {
            return Err(CoreError::invalid(
                "generation proposal domain record id does not match its deterministic binding",
            ));
        }
        validate_review_hash(proposal)?;
    }
    Ok(())
}

pub(super) fn namespace_generation_attempt_proposal_records(
    commit: &GenerationAttemptBeforeReviewCommit,
) -> CoreResult<NamespacedGenerationAttemptBeforeReview> {
    let mut namespaced = commit.clone();
    let domain_review_sha256 = commit.review_sha256.clone();
    namespaced.review_sha256 = generation_attempt_before_review_storage_sha256(
        &commit.generation_id,
        &domain_review_sha256,
    )?;
    let mut original_ids = BTreeSet::new();
    let mut namespaced_ids = BTreeMap::new();
    let mut domain_review_sha256_by_record_id = BTreeMap::new();
    for proposal in &mut namespaced.proposals {
        let original_id = proposal.record.id.clone();
        let domain_review_sha256 = proposal.review_payload_sha256.clone();
        if !original_ids.insert(original_id.as_str().to_owned()) {
            return Err(CoreError::invalid(
                "generation attempt review contains duplicate proposal record ids",
            ));
        }
        let namespaced_id = generation_attempt_proposal_storage_id(
            &commit.generation_id,
            &original_id,
            &domain_review_sha256,
            &namespaced.review_sha256,
        )?;
        let matching_origins = commit
            .derived_closure
            .transitions
            .iter()
            .flat_map(|transition| transition.proposals.iter())
            .filter(|origin| origin.record.id == original_id)
            .collect::<Vec<_>>();
        if matching_origins.len() != 1 || matching_origins[0] != proposal {
            return Err(CoreError::invalid(
                "generation proposal write differs from its exact closure origin",
            ));
        }
        proposal.record.id = namespaced_id.clone();
        proposal.review_payload_sha256 = interaction_proposal_review_sha256(&proposal.record)?;
        namespaced_ids.insert(original_id, namespaced_id);
        domain_review_sha256_by_record_id
            .insert(proposal.record.id.as_str().to_owned(), domain_review_sha256);
    }
    for transition in &mut namespaced.derived_closure.transitions {
        for proposal in &mut transition.proposals {
            if let Some(namespaced_id) = namespaced_ids.get(&proposal.record.id) {
                proposal.record.id = namespaced_id.clone();
                proposal.review_payload_sha256 =
                    interaction_proposal_review_sha256(&proposal.record)?;
            }
        }
        for record in &mut transition.next_state.proposals {
            if let Some(namespaced_id) = namespaced_ids.get(&record.id) {
                record.id = namespaced_id.clone();
            }
        }
    }
    for record in &mut namespaced.next_state.proposals {
        if let Some(namespaced_id) = namespaced_ids.get(&record.id) {
            record.id = namespaced_id.clone();
        }
    }
    for record in &mut namespaced.derived_closure.final_state.proposals {
        if let Some(namespaced_id) = namespaced_ids.get(&record.id) {
            record.id = namespaced_id.clone();
        }
    }
    for transition in &mut namespaced.derived_closure.transitions {
        transition.commit_sha256 = crate::generation_attempt_derived_transition_commit_sha256(
            &namespaced.generation_id,
            transition,
        )?;
    }
    namespaced.derived_closure.chain_sha256 =
        crate::generation_attempt_derived_chain_sha256(&namespaced.derived_closure)?;
    Ok(NamespacedGenerationAttemptBeforeReview {
        commit: namespaced,
        domain_review_sha256,
        domain_review_sha256_by_record_id,
    })
}
pub(super) fn validate_prepared_generation_attempt_before_review(
    _storage: &Storage,
    transaction: &Transaction<'_>,
    commit: &GenerationAttemptBeforeReviewCommit,
    prepared: &PreparedGenerationAttemptBeforeReview,
) -> CoreResult<()> {
    let authority = &prepared.authority;
    let attempt = crate::generation_attempt::read_attempt(transaction, &commit.generation_id)?;
    require_no_pending_derived_predecessor_through(
        transaction,
        &authority.conversation_id,
        &authority.source_branch_id,
        commit.previous_state.revision,
    )?;
    if authority.revision != commit.expected_attempt_revision
        || authority.status != "prepared"
        || authority.context_head_message_id != commit.context_head_message_id
        || authority.module_plan_sha256 != prepared.applied_runtime_plan_sha256
    {
        return Err(revision_conflict(
            "generation attempt changed before BeforeGeneration review",
        ));
    }
    if attempt.input.module_runtime_review_authority.as_ref() != Some(&commit.module_runtime_review)
        || attempt.input.applied_runtime_plan_authority.as_ref()
            != commit.applied_runtime_plan.as_ref()
    {
        return Err(CoreError::invalid(
            "generation BeforeGeneration module authority differs from its prepared attempt",
        ));
    }
    if commit.memory_head_snapshot.conversation_id != authority.conversation_id
        || commit.memory_head_snapshot.source_branch_id != authority.source_branch_id
        || commit.memory_head_snapshot.context_head_message_id != authority.context_head_message_id
        || commit.memory_head_snapshot.include_invalidated
    {
        return Err(CoreError::invalid(
            "generation attempt memory snapshot differs from its immutable source authority",
        ));
    }
    crate::orchestration::require_memory_records_at_head_snapshot_transaction(
        transaction,
        &commit.memory_head_snapshot,
    )?;

    let context = &commit.module_runtime_review.context;
    if context.conversation_id.as_deref() != Some(authority.conversation_id.0.as_str())
        || context.branch_id.as_deref() != Some(authority.proposed_branch_id.0.as_str())
        || !commit
            .module_runtime_review
            .activation_binding_ids
            .is_empty()
    {
        return Err(CoreError::invalid(
            "generation attempt module review differs from its target context",
        ));
    }
    match &commit.applied_runtime_plan {
        Some(plan) => {
            if plan.review != commit.module_runtime_review
                || plan.applied_plan_sha256.as_str() != authority.module_plan_sha256
                || commit.policy.module_plan_sha256.as_deref()
                    != Some(plan.applied_plan_sha256.as_str())
            {
                return Err(CoreError::invalid(
                    "generation attempt applied module authority is inconsistent",
                ));
            }
            let source_applied = transaction
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1
                         FROM module_activation_plans
                         WHERE plan_sha256 = ?1
                           AND approval_sha256 = ?2
                           AND state = 'applied'
                     )",
                    params![
                        plan.source_approval.plan.plan_sha256.as_str(),
                        plan.source_approval.approval_sha256.as_str(),
                    ],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if !source_applied {
                return Err(revision_conflict(
                    "generation attempt module activation is no longer applied",
                ));
            }
            if let Some(parent) = plan.derived_from_plan_sha256.as_ref() {
                let parent_applied = transaction
                    .query_row(
                        "SELECT EXISTS(
                             SELECT 1
                             FROM applied_module_runtime_plans
                             WHERE applied_plan_sha256 = ?1
                               AND state = 'applied'
                         )",
                        [parent.as_str()],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(storage_db_error)?;
                if !parent_applied {
                    return Err(revision_conflict(
                        "generation attempt parent module runtime plan is stale",
                    ));
                }
            }
        }
        None => {
            if authority.module_plan_sha256 != no_applied_module_runtime_plan_sha256().as_str()
                || !commit.module_runtime_review.ordered_bindings.is_empty()
                || commit.policy.module_plan_sha256.is_some()
            {
                return Err(CoreError::invalid(
                    "generation attempt no-module sentinel is inconsistent",
                ));
            }
        }
    }

    // The attempt's applied runtime plan is intentionally not persisted until
    // the atomic generation append. Its embedded, freshly reviewed authority
    // was validated above, so only the immutable rule-set revisions can be
    // resolved through ordinary durable policy tables at this staging seam.
    for (index, transition) in commit.derived_closure.transitions.iter().enumerate() {
        let previous_state = if index == 0 {
            &commit.previous_state
        } else {
            &commit.derived_closure.transitions[index - 1].next_state
        };
        let previous_revision = previous_state.revision;
        if transition.expected_state_revision != previous_revision
            || transition.commit_sha256
                != crate::generation_attempt_derived_transition_commit_sha256(
                    &commit.generation_id,
                    transition,
                )?
        {
            return Err(CoreError::invalid(
                "generation derived transition state or commit authority is invalid",
            ));
        }
        let previous_proposals = previous_state
            .proposals
            .iter()
            .map(|record| (record.id.as_str(), record))
            .collect::<BTreeMap<_, _>>();
        let next_proposals = transition
            .next_state
            .proposals
            .iter()
            .map(|record| (record.id.as_str(), record))
            .collect::<BTreeMap<_, _>>();
        if previous_proposals
            .iter()
            .any(|(id, record)| next_proposals.get(id).copied() != Some(*record))
        {
            return Err(CoreError::invalid(
                "generation derived transition mutated prior proposal audit state",
            ));
        }
        let new_state_proposal_ids = next_proposals
            .keys()
            .copied()
            .filter(|id| !previous_proposals.contains_key(id))
            .collect::<BTreeSet<_>>();
        let transition_proposal_ids = transition
            .proposals
            .iter()
            .map(|proposal| proposal.record.id.as_str())
            .collect::<BTreeSet<_>>();
        if new_state_proposal_ids != transition_proposal_ids
            || transition_proposal_ids.len() != transition.proposals.len()
        {
            return Err(CoreError::invalid(
                "generation derived transition proposal state is not bijective",
            ));
        }
        validate_interaction_policy_rule_set_revisions(transaction, &transition.policy)?;
        validate_action_results_belong_to_policy(&transition.action_results, &transition.policy)?;
        validate_action_result_sources(transaction, &transition.event, &transition.action_results)?;
        validate_proposal_writes(
            transaction,
            previous_revision,
            &transition.next_state,
            &transition.effects,
            &transition.action_results,
            &transition.proposals,
            Some(&commit.generation_id),
            Some(&commit.review_sha256),
        )?;
        validate_derived_event_writes(
            transaction,
            &transition.policy,
            &transition.action_results,
            &transition.effects,
            &transition.derived_events,
        )?;
    }
    if authority.proposed_branch_id == authority.source_branch_id {
        let state_id = transaction
            .query_row(
                "SELECT id
                 FROM interaction_state
                 WHERE conversation_id = ?1 AND branch_id = ?2",
                params![
                    authority.conversation_id.0.as_str(),
                    authority.source_branch_id.0.as_str(),
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation attempt interaction state"))?;
        validate_existing_proposals_unchanged(
            transaction,
            &state_id,
            &commit.previous_state,
            &commit.derived_closure.final_state,
            &commit.proposals,
        )?;
    } else if !commit.previous_state.proposals.is_empty() {
        return Err(revision_conflict(
            "fork generation boundary cannot retain proposal records",
        ));
    }

    let (boundary_state, boundary_knowledge, boundary_sha256) =
        read_generation_attempt_review_boundary(transaction, authority)?;
    if boundary_state != commit.previous_state
        || boundary_knowledge != commit.previous_knowledge
        || boundary_sha256 != commit.context_checkpoint_sha256
    {
        return Err(revision_conflict(
            "generation attempt interaction boundary changed before review",
        ));
    }
    let conflicting_event = transaction
        .query_row(
            "SELECT generation_id, event_sha256
             FROM generation_attempt_before_event_snapshots
             WHERE event_id = ?1",
            [commit.event_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?;
    if conflicting_event
        .is_some_and(|row| row.0 != commit.generation_id.0 || row.1 != prepared.event_sha256)
    {
        return Err(revision_conflict(
            "generation BeforeGeneration event id is already in use",
        ));
    }
    Ok(())
}

pub(super) fn read_generation_attempt_before_review(
    connection: &Connection,
    generation_id: &GenerationId,
    expected_event_sha256: Option<&str>,
) -> CoreResult<Option<StoredGenerationAttemptBeforeReview>> {
    let raw = connection
        .query_row(
            "SELECT snapshot.event_id, snapshot.event_sha256,
                    snapshot.review_sha256,
                    snapshot.previous_state_revision,
                    snapshot.previous_state_snapshot_sha256,
                    snapshot.reviewed_next_state_snapshot_sha256,
                    aggregate.interaction_state_revision,
                    aggregate.state_snapshot_sha256,
                    aggregate.pending_proposal_count,
                    snapshot.created_at,
                    attempt.before_generation_evidence_json,
                    attempt.before_generation_evidence_sha256,
                    attempt.approval_evidence_json,
                    attempt.approval_evidence_sha256,
                    snapshot.domain_review_sha256,
                    snapshot.storage_identity_version,
                    snapshot.evaluation_seal_json,
                    snapshot.evaluation_seal_sha256,
                    snapshot.derived_closure_json,
                    snapshot.derived_closure_sha256,
                    snapshot.closure_authority_version,
                    aggregate.evaluation_seal_sha256,
                    aggregate.derived_chain_sha256,
                    snapshot.applied_runtime_plan_sha256,
                    snapshot.applied_runtime_plan_json,
                    attempt.prompt_selection_authority_json,
                    attempt.prompt_selection_authority_sha256,
                    attempt.prompt_selection_authority_version,
                    snapshot.memory_head_snapshot_json,
                    snapshot.memory_head_snapshot_sha256
             FROM generation_attempt_before_event_snapshots AS snapshot
             JOIN generation_attempt_interaction_aggregates AS aggregate
               ON aggregate.generation_id = snapshot.generation_id
             JOIN generation_attempt_intents AS attempt
               ON attempt.generation_id = snapshot.generation_id
             WHERE snapshot.generation_id = ?1",
            [generation_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, Option<String>>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, String>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, Option<String>>(16)?,
                    row.get::<_, Option<String>>(17)?,
                    row.get::<_, Option<String>>(18)?,
                    row.get::<_, Option<String>>(19)?,
                    row.get::<_, i64>(20)?,
                    row.get::<_, Option<String>>(21)?,
                    row.get::<_, Option<String>>(22)?,
                    row.get::<_, String>(23)?,
                    row.get::<_, Option<String>>(24)?,
                    row.get::<_, Option<String>>(25)?,
                    row.get::<_, Option<String>>(26)?,
                    row.get::<_, i64>(27)?,
                    row.get::<_, String>(28)?,
                    row.get::<_, String>(29)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some(raw) = raw else {
        return Ok(None);
    };
    if expected_event_sha256.is_some_and(|expected| expected != raw.1) {
        return Err(revision_conflict(
            "generation attempt BeforeGeneration input conflicts with its immutable snapshot",
        ));
    }
    let storage_identity_version = u32::try_from(raw.15)
        .map_err(|_| storage_corrupted("generation review identity version is invalid"))?;
    let expected_review_sha256 = match storage_identity_version {
        1 => raw.14.clone(),
        2 => generation_attempt_before_review_storage_sha256(generation_id, &raw.14)?,
        _ => {
            return Err(storage_corrupted(
                "generation review identity version is invalid",
            ));
        }
    };
    if raw.2 != expected_review_sha256 {
        return Err(storage_corrupted(
            "generation review storage identity is invalid",
        ));
    }
    let evidence_json = raw.10.as_deref().ok_or_else(|| {
        storage_corrupted("generation BeforeGeneration snapshot has no attempt evidence")
    })?;
    let evidence_sha256 = raw.11.as_deref().ok_or_else(|| {
        storage_corrupted("generation BeforeGeneration snapshot has no evidence hash")
    })?;
    let evidence: GenerationBeforeEventEvidence = decode_json(
        "generation BeforeGeneration evidence",
        evidence_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    let verified_evidence_sha256 = generation_before_event_evidence_sha256(&evidence)?;
    if verified_evidence_sha256.as_str() != evidence_sha256
        || evidence.event_id != raw.0
        || evidence.event_sha256.as_str() != raw.1
    {
        return Err(storage_corrupted(
            "generation BeforeGeneration snapshot evidence is inconsistent",
        ));
    }
    let mut proposal_review_sha256s = {
        let mut statement = connection
            .prepare(
                "SELECT proposal_review_sha256
                 FROM generation_attempt_proposals
                 WHERE generation_id = ?1
                   AND origin_aggregate_revision = 1
                 ORDER BY ordinal, proposal_record_id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([generation_id.0.as_str()], |row| row.get::<_, String>(0))
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
            .into_iter()
            .map(|sha256| Sha256Digest::parse(sha256).map_err(CoreError::invalid))
            .collect::<CoreResult<Vec<_>>>()?
    };
    proposal_review_sha256s.sort();
    if proposal_review_sha256s != evidence.proposal_review_sha256s
        || evidence.awaiting_approval == proposal_review_sha256s.is_empty()
    {
        return Err(storage_corrupted(
            "generation proposal rows differ from BeforeGeneration evidence",
        ));
    }
    let approval = match (raw.12.as_deref(), raw.13.as_deref()) {
        (None, None) => None,
        (Some(json), Some(expected_sha256)) => {
            let evidence: GenerationApprovalEvidence =
                decode_json("generation approval evidence", json, MAX_EVENT_JSON_BYTES)?;
            let sha256 = generation_approval_evidence_sha256(&evidence)?;
            if sha256.as_str() != expected_sha256 {
                return Err(storage_corrupted(
                    "generation approval evidence fingerprint is invalid",
                ));
            }
            Some((evidence, sha256))
        }
        _ => {
            return Err(storage_corrupted(
                "generation approval evidence columns are incomplete",
            ));
        }
    };
    let closure_authority_version = u32::try_from(raw.20)
        .map_err(|_| storage_corrupted("generation closure authority version is invalid"))?;
    if closure_authority_version != 1 {
        return Err(storage_corrupted(
            "generation attempt has no immutable derived closure authority",
        ));
    }
    let evaluation_seal_json = raw
        .16
        .as_deref()
        .ok_or_else(|| storage_corrupted("generation attempt evaluation seal is missing"))?;
    let evaluation_seal_sha256 = raw
        .17
        .as_deref()
        .ok_or_else(|| storage_corrupted("generation attempt evaluation seal hash is missing"))?;
    let evaluation_seal: InteractionEvaluationSeal = decode_json(
        "generation attempt evaluation seal",
        evaluation_seal_json,
        MAX_STATE_JSON_BYTES,
    )?;
    let verified_evaluation_seal_sha256 = interaction_evaluation_seal_sha256(&evaluation_seal)?;
    if verified_evaluation_seal_sha256.as_str() != evaluation_seal_sha256
        || raw.21.as_deref() != Some(evaluation_seal_sha256)
    {
        return Err(storage_corrupted(
            "generation attempt evaluation seal fingerprint is invalid",
        ));
    }
    let derived_closure_json = raw
        .18
        .as_deref()
        .ok_or_else(|| storage_corrupted("generation attempt derived closure is missing"))?;
    let derived_closure_sha256 = raw
        .19
        .as_deref()
        .ok_or_else(|| storage_corrupted("generation attempt derived closure hash is missing"))?;
    let derived_closure: GenerationAttemptDerivedClosure = decode_json(
        "generation attempt derived closure",
        derived_closure_json,
        16 * 1_024 * 1_024,
    )?;
    let verified_derived_closure_sha256 =
        generation_attempt_derived_closure_sha256(&derived_closure)?;
    if verified_derived_closure_sha256.as_str() != derived_closure_sha256
        || evidence.context_state_revision != derived_closure.final_state.revision
        || evidence.context_state_sha256.as_str()
            != interaction_state_snapshot_sha256(
                &derived_closure.final_state,
                &derived_closure.final_knowledge,
            )?
    {
        return Err(storage_corrupted(
            "generation attempt derived closure fingerprint is invalid",
        ));
    }
    let applied_runtime_plan = raw
        .24
        .as_deref()
        .map(|json| {
            let plan: AppliedModuleRuntimePlan = decode_json(
                "generation attempt applied runtime plan",
                json,
                MAX_STATE_JSON_BYTES,
            )?;
            plan.verify().map_err(|error| {
                storage_corrupted(format!(
                    "generation attempt applied runtime plan is invalid: {error}"
                ))
            })?;
            if plan.applied_plan_sha256.as_str() != raw.23 {
                return Err(storage_corrupted(
                    "generation attempt applied runtime plan fingerprint is invalid",
                ));
            }
            Ok(plan)
        })
        .transpose()?;
    if applied_runtime_plan.is_none() && raw.23 != no_applied_module_runtime_plan_sha256().as_str()
    {
        return Err(storage_corrupted(
            "generation attempt missing applied runtime plan authority",
        ));
    }
    let memory_head_snapshot: MemoryRecordsAtHeadSnapshot = decode_json(
        "generation attempt memory head snapshot",
        &raw.28,
        MAX_STATE_JSON_BYTES,
    )?;
    if encode_json(
        "generation attempt memory head snapshot",
        &memory_head_snapshot,
        MAX_STATE_JSON_BYTES,
    )? != raw.28
        || memory_records_at_head_snapshot_sha256(&memory_head_snapshot)? != raw.29
        || memory_head_snapshot.snapshot_sha256 != raw.29
    {
        return Err(storage_corrupted(
            "generation attempt memory head snapshot fingerprint is invalid",
        ));
    }
    let prompt_selection_authority = match (raw.25.as_deref(), raw.26.as_deref(), raw.27) {
        (Some(json), Some(expected_sha256), 1) => {
            let authority: crate::GenerationPromptSelectionAuthority = decode_json(
                "generation prompt selection authority",
                json,
                MAX_STATE_JSON_BYTES,
            )?;
            let actual_sha256 = crate::generation_prompt_selection_authority_sha256(&authority)?;
            if actual_sha256.as_str() != expected_sha256
                || encode_json(
                    "generation prompt selection authority",
                    &authority,
                    MAX_STATE_JSON_BYTES,
                )? != json
            {
                return Err(storage_corrupted(
                    "generation prompt selection authority fingerprint is invalid",
                ));
            }
            authority
        }
        _ => {
            return Err(storage_corrupted(
                "generation prompt selection authority is incomplete",
            ));
        }
    };
    Ok(Some(StoredGenerationAttemptBeforeReview {
        generation_id: generation_id.clone(),
        event_id: raw.0,
        event_sha256: Sha256Digest::parse(raw.1).map_err(CoreError::invalid)?,
        review_sha256: Sha256Digest::parse(raw.2).map_err(CoreError::invalid)?,
        domain_review_sha256: Sha256Digest::parse(raw.14).map_err(CoreError::invalid)?,
        storage_identity_version,
        closure_authority_version,
        evaluation_seal,
        evaluation_seal_sha256: verified_evaluation_seal_sha256,
        derived_closure,
        derived_closure_sha256: verified_derived_closure_sha256,
        applied_runtime_plan,
        memory_head_snapshot,
        prompt_selection_authority,
        previous_state_revision: u64_from_i64("generation previous state revision", raw.3)?,
        previous_state_snapshot_sha256: Sha256Digest::parse(raw.4).map_err(CoreError::invalid)?,
        resulting_state_revision: u64_from_i64("generation aggregate state revision", raw.6)?,
        resulting_state_snapshot_sha256: Sha256Digest::parse(raw.7).map_err(CoreError::invalid)?,
        proposal_review_sha256s,
        pending_proposal_count: u32::try_from(raw.8)
            .map_err(|_| storage_corrupted("generation pending proposal count is invalid"))?,
        evidence,
        evidence_sha256: verified_evidence_sha256,
        approval_evidence: approval.as_ref().map(|value| value.0.clone()),
        approval_evidence_sha256: approval.map(|value| value.1),
        exact_replay: true,
        created_at: parse_datetime("generation BeforeGeneration created at", &raw.9)?,
    }))
}
