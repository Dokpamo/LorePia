use std::collections::BTreeMap;

use lorepia_domain::{
    CoreError, CoreResult, InteractionEvent, InteractionProposalRecordId,
    InteractionProposalStatus, Sha256Digest,
};
use lorepia_orchestration::{approve_pending, expire_pending_proposal, reject_pending};
use rusqlite::{OptionalExtension, Transaction, params};

use crate::{
    GenerationApprovalEvidence, database::storage_db_error, generation_approval_evidence_sha256,
    interaction_evaluation_seal_sha256,
};

use super::generation_proposal_queries::{
    read_generation_attempt_interaction_aggregate, read_generation_attempt_proposal,
};
use super::generation_proposals::{
    generation_attempt_decision_evidence, generation_attempt_decision_materialization,
};
use super::proposal_records::interaction_proposal_record_id;
use super::state::{validate_knowledge_bindings, validate_state};
use super::types::{
    GenerationAttemptProposalDecision, GenerationAttemptProposalDecisionCommit,
    InteractionProposalWrite, MAX_EVENT_JSON_BYTES, MAX_INTERACTION_DERIVED_CHAIN_EVENTS,
    MAX_STATE_JSON_BYTES, StoredGenerationAttemptInteractionAggregate,
    StoredGenerationAttemptProposal, interaction_policy_sha256, interaction_state_snapshot_sha256,
};
use super::{
    decode_json, encode_json, i64_from_u64, not_found, remap_generation_attempt_state_proposal_ids,
    revision_conflict, sha256_hex, storage_corrupted, validate_action_result_sources,
    validate_action_results_belong_to_policy, validate_interaction_policy_rule_set_revisions,
};

pub(super) struct PreparedGenerationAttemptProposalDecision {
    stored: StoredGenerationAttemptProposal,
    aggregate: StoredGenerationAttemptInteractionAggregate,
    materialization_json: String,
    materialization_sha256: String,
    decision_evidence_json: String,
    decision_evidence_sha256: String,
    decision_event_id: Option<String>,
    decision_event_sha256: Option<String>,
    next_state_revision: u64,
    next_state_json: String,
    next_state_document_sha256: String,
    next_state_snapshot_sha256: String,
    next_knowledge_json: String,
    next_knowledge_sha256: String,
    next_decision_event_ids_json: String,
    next_decision_event_ids_sha256: String,
    next_decision_event_sha256s_json: String,
    next_decision_event_sha256s_sha256: String,
    next_derived_chain_sha256: String,
    next_derived_event_count: u32,
    next_derived_guard_count: u32,
    next_pending_proposal_count: u32,
    new_proposals: Vec<PreparedGenerationAttemptDecisionProposal>,
}

struct PreparedGenerationAttemptDecisionProposal {
    ordinal: u32,
    write: InteractionProposalWrite,
    domain_record_id: InteractionProposalRecordId,
    domain_review_sha256: String,
    record_json: String,
    record_sha256: String,
    action_payload_json: String,
    action_payload_sha256: String,
    origin_policy_json: String,
    origin_policy_sha256: String,
    origin_event_id: String,
    origin_chain_ordinal: u32,
    origin_evaluation_seal_json: String,
    origin_evaluation_seal_sha256: String,
}

pub(super) fn prepare_generation_attempt_proposal_decision(
    transaction: &Transaction<'_>,
    commit: &GenerationAttemptProposalDecisionCommit,
    domain_review_sha256_by_record_id: &BTreeMap<String, String>,
) -> CoreResult<PreparedGenerationAttemptProposalDecision> {
    let stored = read_generation_attempt_proposal(transaction, &commit.proposal_record_id)?
        .ok_or_else(|| not_found("generation attempt proposal"))?;
    let aggregate =
        read_generation_attempt_interaction_aggregate(transaction, &stored.generation_id)?;
    if stored.record.status != InteractionProposalStatus::Pending
        || stored.proposal_revision != commit.expected_proposal_revision
        || aggregate.aggregate_revision != commit.expected_aggregate_revision
        || aggregate.pending_proposal_count == 0
    {
        return Err(revision_conflict(
            "generation proposal decision compare-and-swap failed",
        ));
    }
    let attempt_status = transaction
        .query_row(
            "SELECT status
             FROM generation_attempt_intents
             WHERE generation_id = ?1",
            [stored.generation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(storage_db_error)?;
    if attempt_status != "awaiting_approval" {
        return Err(revision_conflict(
            "generation attempt is not awaiting proposal approval",
        ));
    }
    if stored.origin_aggregate_revision > aggregate.aggregate_revision {
        return Err(storage_corrupted(
            "generation proposal origin aggregate revision is ahead of its aggregate",
        ));
    }
    if commit.decision == GenerationAttemptProposalDecision::Approve {
        let seal = commit.evaluation_seal.as_ref().ok_or_else(|| {
            CoreError::invalid("generation proposal approval evaluation seal is missing")
        })?;
        let closure = commit
            .derived_closure
            .as_ref()
            .ok_or_else(|| CoreError::invalid("generation proposal approval closure is missing"))?;
        let root = closure.transitions.first().ok_or_else(|| {
            CoreError::invalid("generation proposal approval closure has no root")
        })?;
        if seal != &stored.origin_evaluation_seal
            || root.evaluation_seal != stored.origin_evaluation_seal
            || root.event
                != (InteractionEvent::UserAction {
                    action_id: stored.record.proposal_id.clone(),
                })
        {
            return Err(revision_conflict(
                "generation proposal approval differs from its sealed origin authority",
            ));
        }
    }
    let domain_aggregate_state = remap_generation_attempt_state_proposal_ids(
        transaction,
        &stored.generation_id,
        &aggregate.state,
        true,
    )?;
    let expected_domain_decision_state = match commit.decision {
        GenerationAttemptProposalDecision::Approve => {
            approve_pending(
                &domain_aggregate_state,
                &stored.record.proposal_id,
                domain_aggregate_state.revision,
                commit.decided_at_epoch_seconds,
            )
            .map_err(|error| CoreError::invalid(error.to_string()))?
            .state
        }
        GenerationAttemptProposalDecision::Reject => {
            reject_pending(
                &domain_aggregate_state,
                &stored.record.proposal_id,
                domain_aggregate_state.revision,
                commit.decided_at_epoch_seconds,
            )
            .map_err(|error| CoreError::invalid(error.to_string()))?
            .state
        }
        GenerationAttemptProposalDecision::Expire => {
            expire_pending_proposal(
                &domain_aggregate_state,
                &stored.record.proposal_id,
                domain_aggregate_state.revision,
                commit.decided_at_epoch_seconds,
            )
            .map_err(|error| CoreError::invalid(error.to_string()))?
            .state
        }
    };
    let expected_decision_state = remap_generation_attempt_state_proposal_ids(
        transaction,
        &stored.generation_id,
        &expected_domain_decision_state,
        false,
    )?;
    if expected_decision_state != commit.decision_state {
        return Err(CoreError::invalid(
            "generation proposal decision state differs from the stored proposal",
        ));
    }

    let (next_state, next_knowledge, decision_event_id, decision_event_sha256) = match commit
        .decision
    {
        GenerationAttemptProposalDecision::Approve => {
            let current_policy = commit.current_policy.as_ref().ok_or_else(|| {
                CoreError::invalid("generation proposal approval policy is missing")
            })?;
            let derived = commit.derived.as_ref().ok_or_else(|| {
                CoreError::invalid("generation proposal approval event is missing")
            })?;
            if current_policy != &stored.origin_policy || &derived.policy != current_policy {
                return Err(revision_conflict(
                    "generation proposal policy changed after review",
                ));
            }
            // The attempt-owned module plan may not be published to the
            // ordinary historical-plan table until append. Rule revisions and
            // the immutable evaluation seal remain independently durable.
            validate_interaction_policy_rule_set_revisions(transaction, current_policy)?;
            validate_action_results_belong_to_policy(&derived.action_results, current_policy)?;
            let user_action = InteractionEvent::UserAction {
                action_id: stored.record.proposal_id.clone(),
            };
            validate_action_result_sources(transaction, &user_action, &derived.action_results)?;
            if derived.next_state.revision
                != commit
                    .decision_state
                    .revision
                    .checked_add(1)
                    .ok_or_else(|| CoreError::invalid("interaction state revision overflowed"))?
            {
                return Err(CoreError::invalid(
                    "generation approval UserAction revision is invalid",
                ));
            }
            let decision_state_by_id = commit
                .decision_state
                .proposals
                .iter()
                .map(|proposal| (proposal.id.as_str(), proposal))
                .collect::<BTreeMap<_, _>>();
            let next_state_by_id = derived
                .next_state
                .proposals
                .iter()
                .map(|proposal| (proposal.id.as_str(), proposal))
                .collect::<BTreeMap<_, _>>();
            if decision_state_by_id != next_state_by_id {
                return Err(CoreError::invalid(
                    "generation approval UserAction cannot mutate proposal audit records",
                ));
            }
            let event_fingerprint = encode_json(
                "generation proposal decision event",
                &(
                    "lorepia.generation-proposal-decision-event.v1",
                    &stored.generation_id,
                    &commit.proposal_record_id,
                    &user_action,
                    derived,
                ),
                MAX_STATE_JSON_BYTES,
            )?;
            let event_sha256 = sha256_hex(event_fingerprint.as_bytes());
            let event_id_in_use = transaction
                .query_row(
                    "SELECT EXISTS(
                             SELECT 1 FROM interaction_events WHERE id = ?1
                             UNION ALL
                             SELECT 1 FROM generation_attempt_proposals
                             WHERE decision_event_id = ?1
                         )",
                    [derived.event_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if event_id_in_use {
                return Err(revision_conflict(
                    "generation proposal decision event id is already in use",
                ));
            }
            let closure = commit.derived_closure.as_ref().ok_or_else(|| {
                CoreError::invalid("generation proposal approval closure is missing")
            })?;
            (
                closure.final_state.clone(),
                closure.final_knowledge.clone(),
                Some(derived.event_id.clone()),
                Some(event_sha256),
            )
        }
        GenerationAttemptProposalDecision::Reject | GenerationAttemptProposalDecision::Expire => (
            commit.decision_state.clone(),
            aggregate.knowledge.clone(),
            None,
            None,
        ),
    };
    validate_state(&next_state)?;
    validate_knowledge_bindings(&next_state, &next_knowledge)?;
    let next_state_json = encode_json(
        "generation proposal resulting state",
        &next_state,
        MAX_STATE_JSON_BYTES,
    )?;
    let next_state_document_sha256 = sha256_hex(next_state_json.as_bytes());
    let next_state_snapshot_sha256 =
        interaction_state_snapshot_sha256(&next_state, &next_knowledge)?;
    let next_knowledge_json = encode_json(
        "generation proposal resulting knowledge",
        &next_knowledge,
        MAX_STATE_JSON_BYTES,
    )?;
    let next_knowledge_sha256 = sha256_hex(next_knowledge_json.as_bytes());
    let mut decision_event_ids = aggregate.decision_event_ids.clone();
    let mut decision_event_sha256s = aggregate.decision_event_sha256s.clone();
    if let (Some(event_id), Some(event_sha256)) =
        (decision_event_id.as_ref(), decision_event_sha256.as_ref())
    {
        decision_event_ids.push(event_id.clone());
        decision_event_sha256s
            .push(Sha256Digest::parse(event_sha256.clone()).map_err(CoreError::invalid)?);
    }
    let next_decision_event_ids_json = encode_json(
        "generation decision event ids",
        &decision_event_ids,
        MAX_EVENT_JSON_BYTES,
    )?;
    let next_decision_event_ids_sha256 = sha256_hex(next_decision_event_ids_json.as_bytes());
    let next_decision_event_sha256s_json = encode_json(
        "generation decision event hashes",
        &decision_event_sha256s,
        MAX_EVENT_JSON_BYTES,
    )?;
    let next_decision_event_sha256s_sha256 =
        sha256_hex(next_decision_event_sha256s_json.as_bytes());
    let (next_derived_chain_sha256, next_derived_event_count, next_derived_guard_count) =
        if let Some(closure) = commit.derived_closure.as_ref() {
            let fingerprint = encode_json(
                "generation cumulative derived chain",
                &(
                    "lorepia.generation-attempt-cumulative-derived-chain.v1",
                    &aggregate.derived_chain_sha256,
                    &closure.chain_sha256,
                    aggregate.derived_event_count,
                    closure.event_count,
                    aggregate.derived_guard_count,
                    closure.guard_count,
                ),
                MAX_EVENT_JSON_BYTES,
            )?;
            (
                sha256_hex(fingerprint.as_bytes()),
                aggregate
                    .derived_event_count
                    .checked_add(closure.event_count)
                    .ok_or_else(|| {
                        CoreError::invalid("generation derived event count overflowed")
                    })?,
                aggregate
                    .derived_guard_count
                    .checked_add(closure.guard_count)
                    .ok_or_else(|| {
                        CoreError::invalid("generation derived guard count overflowed")
                    })?,
            )
        } else {
            (
                aggregate.derived_chain_sha256.as_str().to_owned(),
                aggregate.derived_event_count,
                aggregate.derived_guard_count,
            )
        };
    if next_derived_event_count > MAX_INTERACTION_DERIVED_CHAIN_EVENTS
        || next_derived_guard_count > 1_024
    {
        return Err(CoreError::invalid(
            "generation attempt cumulative derived closure limit was exceeded",
        ));
    }
    let first_new_ordinal = transaction
        .query_row(
            "SELECT COALESCE(MAX(ordinal) + 1, 0)
             FROM generation_attempt_proposals
             WHERE generation_id = ?1",
            [stored.generation_id.0.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)?;
    let mut new_proposals = Vec::new();
    if let Some(closure) = commit.derived_closure.as_ref() {
        for transition in &closure.transitions {
            let origin_policy_json = encode_json(
                "generation proposal origin policy",
                &transition.policy,
                MAX_EVENT_JSON_BYTES,
            )?;
            let origin_policy_sha256 = interaction_policy_sha256(&transition.policy)?;
            let origin_evaluation_seal_json = encode_json(
                "generation proposal origin evaluation seal",
                &transition.evaluation_seal,
                MAX_STATE_JSON_BYTES,
            )?;
            let origin_evaluation_seal_sha256 =
                interaction_evaluation_seal_sha256(&transition.evaluation_seal)?
                    .as_str()
                    .to_owned();
            for write in &transition.proposals {
                let domain_review_sha256 = domain_review_sha256_by_record_id
                    .get(write.record.id.as_str())
                    .cloned()
                    .ok_or_else(|| {
                        storage_corrupted(
                            "generation approval proposal lost its domain fingerprint",
                        )
                    })?;
                let domain_record_id = interaction_proposal_record_id(
                    &write.record.rule_set_id,
                    &write.record.rule_id,
                    &write.record.proposal_id,
                    write.record.source_interaction_state_revision,
                )?;
                let record_json = encode_json(
                    "generation approval proposal record",
                    &write.record,
                    MAX_EVENT_JSON_BYTES,
                )?;
                let record_sha256 = sha256_hex(record_json.as_bytes());
                if record_sha256 != write.review_payload_sha256 {
                    return Err(CoreError::invalid(
                        "generation approval proposal review hash changed",
                    ));
                }
                let action_payload_json = transaction
                    .query_row(
                        "SELECT payload_json
                         FROM interaction_actions
                         WHERE set_revision_id = ?1
                           AND rule_id = ?2
                           AND ordinal = ?3",
                        params![
                            write.rule_set_revision_id,
                            write.record.rule_id.as_str(),
                            i64::from(write.action_ordinal),
                        ],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(storage_db_error)?
                    .ok_or_else(|| {
                        CoreError::invalid("generation approval proposal source action is missing")
                    })?;
                let ordinal = first_new_ordinal
                    .checked_add(i64::try_from(new_proposals.len()).map_err(|_| {
                        CoreError::invalid("too many generation approval proposals")
                    })?)
                    .ok_or_else(|| CoreError::invalid("generation proposal ordinal overflowed"))?;
                new_proposals.push(PreparedGenerationAttemptDecisionProposal {
                    ordinal: u32::try_from(ordinal).map_err(|_| {
                        CoreError::invalid("generation proposal ordinal overflowed")
                    })?,
                    write: write.clone(),
                    domain_record_id,
                    domain_review_sha256,
                    record_json,
                    record_sha256,
                    action_payload_sha256: sha256_hex(action_payload_json.as_bytes()),
                    action_payload_json,
                    origin_policy_json: origin_policy_json.clone(),
                    origin_policy_sha256: origin_policy_sha256.clone(),
                    origin_event_id: transition.event_id.clone(),
                    origin_chain_ordinal: transition.ordinal,
                    origin_evaluation_seal_json: origin_evaluation_seal_json.clone(),
                    origin_evaluation_seal_sha256: origin_evaluation_seal_sha256.clone(),
                });
            }
        }
    }
    let next_pending_proposal_count = aggregate
        .pending_proposal_count
        .checked_sub(1)
        .and_then(|count| count.checked_add(u32::try_from(new_proposals.len()).ok()?))
        .ok_or_else(|| CoreError::invalid("generation pending proposal count overflowed"))?;
    let (materialization_json, materialization_sha256) =
        generation_attempt_decision_materialization(commit)?;
    let (decision_evidence_json, decision_evidence_sha256) =
        generation_attempt_decision_evidence(commit, &materialization_sha256)?;
    Ok(PreparedGenerationAttemptProposalDecision {
        stored,
        aggregate,
        materialization_json,
        materialization_sha256,
        decision_evidence_json,
        decision_evidence_sha256,
        decision_event_id,
        decision_event_sha256,
        next_state_revision: next_state.revision,
        next_state_json,
        next_state_document_sha256,
        next_state_snapshot_sha256,
        next_knowledge_json,
        next_knowledge_sha256,
        next_decision_event_ids_json,
        next_decision_event_ids_sha256,
        next_decision_event_sha256s_json,
        next_decision_event_sha256s_sha256,
        next_derived_chain_sha256,
        next_derived_event_count,
        next_derived_guard_count,
        next_pending_proposal_count,
        new_proposals,
    })
}

pub(super) fn write_generation_attempt_proposal_decision(
    transaction: &Transaction<'_>,
    commit: &GenerationAttemptProposalDecisionCommit,
    prepared: &PreparedGenerationAttemptProposalDecision,
) -> CoreResult<()> {
    let next_aggregate_revision = prepared
        .aggregate
        .aggregate_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("generation aggregate revision overflowed"))?;
    let next_pending = prepared.next_pending_proposal_count;
    let next_terminal = prepared
        .aggregate
        .terminal_decision_count
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("generation terminal decision count overflowed"))?;
    let (status, decision_kind) = match commit.decision {
        GenerationAttemptProposalDecision::Approve => ("approved", "approved"),
        GenerationAttemptProposalDecision::Reject => ("rejected", "rejected"),
        GenerationAttemptProposalDecision::Expire => ("expired", "expired"),
    };
    let changed = transaction
        .execute(
            "UPDATE generation_attempt_proposals
             SET status = ?2, proposal_revision = proposal_revision + 1,
                 decision_kind = ?3, decision_idempotency_key = ?4,
                 decision_event_id = ?5, decision_event_sha256 = ?6,
                 decision_evidence_json = ?7,
                 decision_evidence_sha256 = ?8,
                 resulting_aggregate_revision = ?9,
                 resulting_state_revision = ?10,
                 resulting_state_json = ?11,
                 resulting_state_snapshot_sha256 = ?12,
                 materialization_json = ?13, materialization_sha256 = ?14,
                 decided_at_epoch_seconds = ?15, updated_at = ?16,
                 resulting_derived_chain_sha256 = ?18,
                 resulting_derived_event_count = ?19,
                 resulting_derived_guard_count = ?20,
                 resulting_pending_proposal_count = ?21
             WHERE proposal_record_id = ?1
               AND proposal_revision = ?17
               AND status = 'pending'",
            params![
                commit.proposal_record_id.as_str(),
                status,
                decision_kind,
                commit.decision_idempotency_key,
                prepared.decision_event_id,
                prepared.decision_event_sha256,
                prepared.decision_evidence_json,
                prepared.decision_evidence_sha256,
                i64_from_u64(
                    "generation resulting aggregate revision",
                    next_aggregate_revision
                )?,
                i64_from_u64(
                    "generation proposal resulting state revision",
                    prepared.next_state_revision,
                )?,
                prepared.next_state_json,
                prepared.next_state_snapshot_sha256,
                prepared.materialization_json,
                prepared.materialization_sha256,
                commit.decided_at_epoch_seconds,
                commit.updated_at.to_rfc3339(),
                i64_from_u64(
                    "generation proposal expected revision",
                    commit.expected_proposal_revision
                )?,
                prepared.next_derived_chain_sha256,
                i64::from(prepared.next_derived_event_count),
                i64::from(prepared.next_derived_guard_count),
                i64::from(prepared.next_pending_proposal_count),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "generation proposal decision compare-and-swap failed",
        ));
    }
    let before_review_sha256 = transaction
        .query_row(
            "SELECT review_sha256
             FROM generation_attempt_before_event_snapshots
             WHERE generation_id = ?1",
            [prepared.stored.generation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(storage_db_error)?;
    for proposal in &prepared.new_proposals {
        transaction
            .execute(
                "INSERT INTO generation_attempt_proposals
                 (proposal_record_id, generation_id, ordinal,
                  before_event_snapshot_sha256, proposal_id,
                  proposal_record_json, proposal_record_sha256,
                  proposal_review_sha256, domain_proposal_review_sha256,
                  origin_policy_json, origin_policy_sha256,
                  rule_set_revision_id, rule_id, action_ordinal,
                  action_payload_json, action_payload_sha256,
                  source_interaction_state_revision, status, proposal_revision,
                  requested_at_epoch_seconds, expires_at_epoch_seconds,
                  domain_proposal_record_id, storage_identity_version,
                  origin_event_id, origin_chain_ordinal,
                  origin_aggregate_revision, origin_evaluation_seal_json,
                  origin_evaluation_seal_sha256, created_at, updated_at)
                 VALUES
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                  ?13, ?14, ?15, ?16, ?17, 'pending', 1, ?18, ?19,
                  ?20, 2, ?21, ?22, ?23, ?24, ?25, ?26, ?26)",
                params![
                    proposal.write.record.id.as_str(),
                    prepared.stored.generation_id.0.as_str(),
                    i64::from(proposal.ordinal),
                    before_review_sha256,
                    proposal.write.record.proposal_id,
                    proposal.record_json,
                    proposal.record_sha256,
                    proposal.write.review_payload_sha256,
                    proposal.domain_review_sha256,
                    proposal.origin_policy_json,
                    proposal.origin_policy_sha256,
                    proposal.write.rule_set_revision_id,
                    proposal.write.record.rule_id.as_str(),
                    i64::from(proposal.write.action_ordinal),
                    proposal.action_payload_json,
                    proposal.action_payload_sha256,
                    i64_from_u64(
                        "generation proposal source state revision",
                        proposal.write.record.source_interaction_state_revision,
                    )?,
                    proposal.write.record.requested_at_epoch_seconds,
                    proposal.write.record.expires_at_epoch_seconds,
                    proposal.domain_record_id.as_str(),
                    proposal.origin_event_id,
                    i64::from(proposal.origin_chain_ordinal),
                    i64_from_u64(
                        "generation proposal origin aggregate revision",
                        next_aggregate_revision,
                    )?,
                    proposal.origin_evaluation_seal_json,
                    proposal.origin_evaluation_seal_sha256,
                    commit.updated_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
    }
    let aggregate_changed = transaction
        .execute(
            "UPDATE generation_attempt_interaction_aggregates
             SET aggregate_revision = aggregate_revision + 1,
                 interaction_state_revision = ?2,
                 state_json = ?3, state_document_sha256 = ?4,
                 state_snapshot_sha256 = ?5, knowledge_json = ?6,
                 knowledge_sha256 = ?7, pending_proposal_count = ?8,
                 terminal_decision_count = ?9,
                 decision_event_ids_json = ?10,
                 decision_event_ids_sha256 = ?11,
                 decision_event_sha256s_json = ?12,
                 decision_event_sha256s_sha256 = ?13,
                 updated_at = ?14,
                 derived_chain_sha256 = ?17,
                 derived_event_count = ?18,
                 derived_guard_count = ?19
             WHERE generation_id = ?1
               AND aggregate_revision = ?15
               AND pending_proposal_count = ?16",
            params![
                prepared.stored.generation_id.0.as_str(),
                i64_from_u64(
                    "generation aggregate state revision",
                    prepared.next_state_revision,
                )?,
                prepared.next_state_json,
                prepared.next_state_document_sha256,
                prepared.next_state_snapshot_sha256,
                prepared.next_knowledge_json,
                prepared.next_knowledge_sha256,
                i64::from(next_pending),
                i64::from(next_terminal),
                prepared.next_decision_event_ids_json,
                prepared.next_decision_event_ids_sha256,
                prepared.next_decision_event_sha256s_json,
                prepared.next_decision_event_sha256s_sha256,
                commit.updated_at.to_rfc3339(),
                i64_from_u64(
                    "generation aggregate expected revision",
                    commit.expected_aggregate_revision
                )?,
                i64::from(prepared.aggregate.pending_proposal_count),
                prepared.next_derived_chain_sha256,
                i64::from(prepared.next_derived_event_count),
                i64::from(prepared.next_derived_guard_count),
            ],
        )
        .map_err(storage_db_error)?;
    if aggregate_changed != 1 {
        return Err(revision_conflict(
            "generation interaction aggregate compare-and-swap failed",
        ));
    }

    if next_pending == 0 {
        let before_evidence_sha256 = transaction
            .query_row(
                "SELECT before_generation_evidence_sha256
                 FROM generation_attempt_intents
                 WHERE generation_id = ?1
                   AND status = 'awaiting_approval'",
                [prepared.stored.generation_id.0.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                revision_conflict("generation attempt is no longer awaiting approval")
            })?;
        let decision_event_ids: Vec<String> = decode_json(
            "generation approval decision event ids",
            &prepared.next_decision_event_ids_json,
            MAX_EVENT_JSON_BYTES,
        )?;
        let decision_event_sha256s: Vec<Sha256Digest> = decode_json(
            "generation approval decision event hashes",
            &prepared.next_decision_event_sha256s_json,
            MAX_EVENT_JSON_BYTES,
        )?;
        let evidence = GenerationApprovalEvidence {
            before_event_sha256: Sha256Digest::parse(before_evidence_sha256)
                .map_err(CoreError::invalid)?,
            decision_event_ids,
            decision_event_sha256s,
            resulting_state_revision: prepared.next_state_revision,
            resulting_state_sha256: Sha256Digest::parse(
                prepared.next_state_snapshot_sha256.clone(),
            )
            .map_err(CoreError::invalid)?,
        };
        let evidence_sha256 = generation_approval_evidence_sha256(&evidence)?;
        let evidence_json = encode_json(
            "generation approval evidence",
            &evidence,
            MAX_EVENT_JSON_BYTES,
        )?;
        let attempt_changed = transaction
            .execute(
                "UPDATE generation_attempt_intents
                 SET status = 'before_generation_applied',
                     revision = revision + 1,
                     approval_evidence_json = ?2,
                     approval_evidence_sha256 = ?3,
                     updated_at = ?4
                 WHERE generation_id = ?1
                   AND status = 'awaiting_approval'
                   AND approval_evidence_sha256 IS NULL",
                params![
                    prepared.stored.generation_id.0.as_str(),
                    evidence_json,
                    evidence_sha256.as_str(),
                    commit.updated_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        if attempt_changed != 1 {
            return Err(revision_conflict(
                "generation attempt approval resolution compare-and-swap failed",
            ));
        }
    }
    Ok(())
}
