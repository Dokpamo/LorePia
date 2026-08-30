use chrono::{DateTime, Utc};
use lorepia_domain::{
    CoreError, CoreResult, GenerationId, InteractionEvent, InteractionProposalRecordId,
    InteractionProposalStatus, InteractionState, Sha256Digest,
};
use rusqlite::{Connection, Transaction};

use crate::{GenerationAttemptDerivedClosure, generation_attempt_derived_closure_sha256};

use super::event_transactions::{
    InteractionEventTransitionWrite, event_commit_sha256, event_id_or_idempotency_exists,
    stored_event_payload, validate_derived_event_commit, validate_event_commit,
    write_event_transition,
};
use super::generation_proposals::{
    GenerationAttemptProposalDecisionMaterialization, generation_attempt_decision_evidence,
    generation_attempt_decision_materialization,
    validate_generation_attempt_proposal_decision_commit,
};
use super::proposal_records::{
    mark_proposal_dispatched, proposal_status_wire, require_pending_proposal,
    transition_proposal_status, validate_existing_proposals_unchanged,
};
use super::proposals::derive_decision_state;
use super::state::{
    bump_normalized_state_revisions, read_knowledge_bindings, require_state_for_key,
    write_state_document_only,
};
use super::types::{
    GenerationAttemptProposalDecision, GenerationAttemptProposalDecisionCommit,
    InteractionEventCommit, InteractionKnowledgeBinding, InteractionStateKey, MAX_EVENT_JSON_BYTES,
    MAX_STATE_JSON_BYTES,
};
use super::{
    decode_json, encode_json, parse_datetime, revision_conflict, sha256_hex, storage_corrupted,
    storage_db_error, u64_from_i64,
};

#[derive(Debug)]
pub(super) struct GenerationAttemptAppendDecision {
    proposal_record_id: InteractionProposalRecordId,
    expected_proposal_revision: u64,
    pub(super) decision_event_id: Option<String>,
    pub(super) decision_event_sha256: Option<Sha256Digest>,
    decided_at_epoch_seconds: i64,
    updated_at: DateTime<Utc>,
    pub(super) materialization: GenerationAttemptProposalDecisionMaterialization,
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

#[allow(clippy::too_many_arguments)]
pub(super) fn materialize_generation_attempt_closed_closure(
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

pub(super) fn read_generation_attempt_append_decisions(
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

pub(super) fn replay_generation_attempt_append_decision(
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
