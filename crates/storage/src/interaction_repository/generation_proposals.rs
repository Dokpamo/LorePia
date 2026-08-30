use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    CoreError, CoreResult, GenerationId, InteractionProposalRecordId, InteractionState,
    Sha256Digest,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::{
    GenerationApprovalEvidence, GenerationAttemptDerivedClosure, InteractionEvaluationSeal,
    Storage, database::storage_db_error, generation_approval_evidence_sha256,
    generation_attempt_derived_closure_sha256,
};

use super::event_transactions::{
    validate_event_collections, validate_new_event_collections, validate_policy_shape,
};
use super::generation_proposal_persistence::{
    prepare_generation_attempt_proposal_decision, write_generation_attempt_proposal_decision,
};
use super::generation_proposal_queries::{
    read_generation_attempt_interaction_aggregate, read_generation_attempt_proposal,
};
use super::generation_review_authority::generation_attempt_proposal_storage_id;
use super::proposal_records::interaction_proposal_record_id;
use super::state::{validate_knowledge_bindings, validate_nonempty_id, validate_state};
use super::types::{
    GenerationAttemptProposalDecision, GenerationAttemptProposalDecisionCommit,
    GenerationAttemptProposalDecisionReceipt, InteractionDerivedEventCommit,
    InteractionPolicySnapshot, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES,
    interaction_proposal_review_sha256,
};
use super::{
    decode_json, encode_json, not_found, revision_conflict, sha256_hex, storage_corrupted,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GenerationAttemptProposalDecisionMaterialization {
    pub(super) schema_version: u32,
    pub(super) decision: GenerationAttemptProposalDecision,
    pub(super) decision_state: InteractionState,
    pub(super) current_policy: Option<InteractionPolicySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) evaluation_seal: Option<InteractionEvaluationSeal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) derived_closure: Option<GenerationAttemptDerivedClosure>,
    pub(super) derived: Option<InteractionDerivedEventCommit>,
}

#[derive(Serialize)]
struct GenerationAttemptProposalDecisionFingerprint<'a> {
    schema_version: u32,
    proposal_record_id: &'a InteractionProposalRecordId,
    expected_proposal_revision: u64,
    expected_aggregate_revision: u64,
    decision: GenerationAttemptProposalDecision,
    decision_idempotency_key: &'a str,
    decided_at_epoch_seconds: i64,
    materialization_sha256: &'a str,
}

struct NamespacedGenerationAttemptProposalDecision {
    commit: GenerationAttemptProposalDecisionCommit,
    domain_review_sha256_by_record_id: BTreeMap<String, String>,
}

impl Storage {
    /// Atomically decides one attempt-owned proposal and advances its isolated
    /// aggregate. Exact idempotency replay is resolved before any current-state
    /// CAS check.
    pub fn decide_generation_attempt_proposal(
        &self,
        commit: &GenerationAttemptProposalDecisionCommit,
    ) -> CoreResult<GenerationAttemptProposalDecisionReceipt> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let namespaced = namespace_generation_attempt_proposal_decision(&transaction, commit)?;
        let commit = &namespaced.commit;
        validate_generation_attempt_proposal_decision_commit(commit)?;
        if let Some(replay) =
            read_generation_attempt_proposal_decision_replay(&transaction, commit)?
        {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(replay);
        }
        let prepared = prepare_generation_attempt_proposal_decision(
            &transaction,
            commit,
            &namespaced.domain_review_sha256_by_record_id,
        )?;
        write_generation_attempt_proposal_decision(&transaction, commit, &prepared)?;
        let proposal = read_generation_attempt_proposal(&transaction, &commit.proposal_record_id)?
            .ok_or_else(|| storage_corrupted("generation proposal vanished after its decision"))?;
        let aggregate =
            read_generation_attempt_interaction_aggregate(&transaction, &proposal.generation_id)?;
        let (approval_evidence, approval_evidence_sha256) =
            read_generation_attempt_approval_evidence(&transaction, &proposal.generation_id)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(GenerationAttemptProposalDecisionReceipt {
            proposal,
            aggregate,
            approval_evidence,
            approval_evidence_sha256,
            exact_replay: false,
        })
    }
}

fn remap_generation_attempt_proposal_records(
    state: &mut InteractionState,
    identities: &BTreeMap<InteractionProposalRecordId, InteractionProposalRecordId>,
) {
    for record in &mut state.proposals {
        if let Some(namespaced) = identities.get(&record.id) {
            record.id = namespaced.clone();
        }
    }
}

fn namespace_generation_attempt_proposal_decision(
    transaction: &Transaction<'_>,
    commit: &GenerationAttemptProposalDecisionCommit,
) -> CoreResult<NamespacedGenerationAttemptProposalDecision> {
    let mut namespaced = commit.clone();
    let mut domain_review_sha256_by_record_id = BTreeMap::new();
    let Some(closure) = namespaced.derived_closure.as_mut() else {
        return Ok(NamespacedGenerationAttemptProposalDecision {
            commit: namespaced,
            domain_review_sha256_by_record_id,
        });
    };
    let stored = read_generation_attempt_proposal(transaction, &commit.proposal_record_id)?
        .ok_or_else(|| not_found("generation attempt proposal"))?;
    let before_review_sha256 = transaction
        .query_row(
            "SELECT review_sha256
             FROM generation_attempt_before_event_snapshots
             WHERE generation_id = ?1",
            [stored.generation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(storage_db_error)?;
    let mut identities = {
        let mut statement = transaction
            .prepare(
                "SELECT domain_proposal_record_id, proposal_record_id
                 FROM generation_attempt_proposals
                 WHERE generation_id = ?1
                 ORDER BY ordinal, proposal_record_id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([stored.generation_id.0.as_str()], |row| {
                Ok((
                    InteractionProposalRecordId::from(row.get::<_, String>(0)?),
                    InteractionProposalRecordId::from(row.get::<_, String>(1)?),
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(storage_db_error)?
    };
    let mut new_domain_ids = BTreeSet::new();
    for proposal in closure
        .transitions
        .iter()
        .flat_map(|transition| transition.proposals.iter())
    {
        let domain_record_id = interaction_proposal_record_id(
            &proposal.record.rule_set_id,
            &proposal.record.rule_id,
            &proposal.record.proposal_id,
            proposal.record.source_interaction_state_revision,
        )?;
        if proposal.record.id != domain_record_id
            || interaction_proposal_review_sha256(&proposal.record)?
                != proposal.review_payload_sha256
            || identities.contains_key(&domain_record_id)
            || !new_domain_ids.insert(domain_record_id.clone())
        {
            return Err(CoreError::invalid(
                "generation approval closure has an invalid new proposal identity",
            ));
        }
        let namespaced_id = generation_attempt_proposal_storage_id(
            &stored.generation_id,
            &domain_record_id,
            &proposal.review_payload_sha256,
            &before_review_sha256,
        )?;
        if identities
            .values()
            .any(|existing| existing == &namespaced_id)
        {
            return Err(revision_conflict(
                "generation approval proposal identity is already in use",
            ));
        }
        domain_review_sha256_by_record_id.insert(
            namespaced_id.as_str().to_owned(),
            proposal.review_payload_sha256.clone(),
        );
        identities.insert(domain_record_id, namespaced_id);
    }

    for transition in &mut closure.transitions {
        remap_generation_attempt_proposal_records(&mut transition.next_state, &identities);
        for proposal in &mut transition.proposals {
            proposal.record.id = identities
                .get(&proposal.record.id)
                .cloned()
                .ok_or_else(|| {
                    CoreError::invalid("generation approval proposal lost its namespace identity")
                })?;
            proposal.review_payload_sha256 = interaction_proposal_review_sha256(&proposal.record)?;
        }
        transition.commit_sha256 = crate::generation_attempt_derived_transition_commit_sha256(
            &stored.generation_id,
            transition,
        )?;
    }
    remap_generation_attempt_proposal_records(&mut closure.final_state, &identities);
    closure.chain_sha256 = crate::generation_attempt_derived_chain_sha256(closure)?;

    if let Some(derived) = namespaced.derived.as_mut() {
        remap_generation_attempt_proposal_records(&mut derived.next_state, &identities);
        for proposal in &mut derived.proposals {
            if let Some(namespaced_id) = identities.get(&proposal.record.id) {
                proposal.record.id = namespaced_id.clone();
                proposal.review_payload_sha256 =
                    interaction_proposal_review_sha256(&proposal.record)?;
            }
        }
    }
    Ok(NamespacedGenerationAttemptProposalDecision {
        commit: namespaced,
        domain_review_sha256_by_record_id,
    })
}

pub(super) fn validate_generation_attempt_proposal_decision_commit(
    commit: &GenerationAttemptProposalDecisionCommit,
) -> CoreResult<()> {
    validate_nonempty_id(
        "generation proposal record id",
        commit.proposal_record_id.as_str(),
    )?;
    validate_nonempty_id(
        "generation proposal decision idempotency key",
        &commit.decision_idempotency_key,
    )?;
    if commit.expected_proposal_revision == 0
        || commit.expected_aggregate_revision == 0
        || commit.decided_at_epoch_seconds < 0
    {
        return Err(CoreError::invalid(
            "generation proposal decision CAS or timestamp is invalid",
        ));
    }
    validate_state(&commit.decision_state)?;
    match commit.decision {
        GenerationAttemptProposalDecision::Approve => {
            if commit.current_policy.is_none()
                || commit.evaluation_seal.is_none()
                || commit.derived_closure.is_none()
                || commit.derived.is_none()
            {
                return Err(CoreError::invalid(
                    "generation proposal approval requires an exact sealed UserAction closure",
                ));
            }
        }
        GenerationAttemptProposalDecision::Reject | GenerationAttemptProposalDecision::Expire => {
            if commit.current_policy.is_some()
                || commit.evaluation_seal.is_some()
                || commit.derived_closure.is_some()
                || commit.derived.is_some()
            {
                return Err(CoreError::invalid(
                    "generation proposal rejection or expiry cannot dispatch an event",
                ));
            }
        }
    }
    if let Some(derived) = commit.derived.as_ref() {
        validate_nonempty_id("generation proposal decision event id", &derived.event_id)?;
        validate_nonempty_id(
            "generation proposal decision event idempotency key",
            &derived.idempotency_key,
        )?;
        validate_policy_shape(&derived.policy)?;
        validate_state(&derived.next_state)?;
        validate_knowledge_bindings(&derived.next_state, &derived.knowledge)?;
        validate_event_collections(
            &derived.action_results,
            &derived.effects,
            &derived.proposals,
        )?;
        let evaluation_seal = commit.evaluation_seal.as_ref().ok_or_else(|| {
            CoreError::invalid("generation proposal approval evaluation seal is missing")
        })?;
        let closure = commit
            .derived_closure
            .as_ref()
            .ok_or_else(|| CoreError::invalid("generation proposal approval closure is missing"))?;
        generation_attempt_derived_closure_sha256(closure)?;
        for transition in &closure.transitions {
            validate_new_event_collections(
                &transition.action_results,
                &transition.effects,
                &transition.proposals,
            )?;
        }
        let root = closure.transitions.first().ok_or_else(|| {
            CoreError::invalid("generation proposal approval closure has no root")
        })?;
        if root.event_id != derived.event_id
            || root.policy != derived.policy
            || &root.evaluation_seal != evaluation_seal
            || root.next_state != derived.next_state
            || root.knowledge != derived.knowledge
            || root.action_results != derived.action_results
            || root.effects != derived.effects
            || root.derived_events != derived.derived_events
            || root.proposals != derived.proposals
        {
            return Err(CoreError::invalid(
                "generation proposal UserAction differs from its derived closure root",
            ));
        }
    }
    Ok(())
}

fn read_generation_attempt_approval_evidence(
    connection: &Connection,
    generation_id: &GenerationId,
) -> CoreResult<(Option<GenerationApprovalEvidence>, Option<Sha256Digest>)> {
    let raw = connection
        .query_row(
            "SELECT approval_evidence_json, approval_evidence_sha256
             FROM generation_attempt_intents
             WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("generation attempt"))?;
    match (raw.0, raw.1) {
        (None, None) => Ok((None, None)),
        (Some(json), Some(expected_sha256)) => {
            let evidence: GenerationApprovalEvidence =
                decode_json("generation approval evidence", &json, MAX_EVENT_JSON_BYTES)?;
            let sha256 = generation_approval_evidence_sha256(&evidence)?;
            if sha256.as_str() != expected_sha256 {
                return Err(storage_corrupted(
                    "generation approval evidence fingerprint is invalid",
                ));
            }
            Ok((Some(evidence), Some(sha256)))
        }
        _ => Err(storage_corrupted(
            "generation approval evidence columns are incomplete",
        )),
    }
}

pub(super) fn generation_attempt_decision_materialization(
    commit: &GenerationAttemptProposalDecisionCommit,
) -> CoreResult<(String, String)> {
    let materialization = GenerationAttemptProposalDecisionMaterialization {
        schema_version: 1,
        decision: commit.decision,
        decision_state: commit.decision_state.clone(),
        current_policy: commit.current_policy.clone(),
        evaluation_seal: commit.evaluation_seal.clone(),
        derived_closure: commit.derived_closure.clone(),
        derived: commit.derived.clone(),
    };
    let json = encode_json(
        "generation proposal decision materialization",
        &materialization,
        MAX_STATE_JSON_BYTES,
    )?;
    let sha256 = sha256_hex(json.as_bytes());
    Ok((json, sha256))
}

pub(super) fn generation_attempt_decision_evidence(
    commit: &GenerationAttemptProposalDecisionCommit,
    materialization_sha256: &str,
) -> CoreResult<(String, String)> {
    let json = encode_json(
        "generation proposal decision evidence",
        &GenerationAttemptProposalDecisionFingerprint {
            schema_version: 1,
            proposal_record_id: &commit.proposal_record_id,
            expected_proposal_revision: commit.expected_proposal_revision,
            expected_aggregate_revision: commit.expected_aggregate_revision,
            decision: commit.decision,
            decision_idempotency_key: &commit.decision_idempotency_key,
            decided_at_epoch_seconds: commit.decided_at_epoch_seconds,
            materialization_sha256,
        },
        MAX_EVENT_JSON_BYTES,
    )?;
    let sha256 = sha256_hex(json.as_bytes());
    Ok((json, sha256))
}

fn read_generation_attempt_proposal_decision_replay(
    connection: &Connection,
    commit: &GenerationAttemptProposalDecisionCommit,
) -> CoreResult<Option<GenerationAttemptProposalDecisionReceipt>> {
    let existing = connection
        .query_row(
            "SELECT proposal_record_id, decision_evidence_sha256
             FROM generation_attempt_proposals
             WHERE decision_idempotency_key = ?1",
            [commit.decision_idempotency_key.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some((proposal_record_id, stored_evidence_sha256)) = existing else {
        return Ok(None);
    };
    let (_, materialization_sha256) = generation_attempt_decision_materialization(commit)?;
    let (_, evidence_sha256) =
        generation_attempt_decision_evidence(commit, &materialization_sha256)?;
    if proposal_record_id != commit.proposal_record_id.as_str()
        || stored_evidence_sha256 != evidence_sha256
    {
        return Err(revision_conflict(
            "generation proposal decision idempotency key was reused",
        ));
    }
    let proposal = read_generation_attempt_proposal(connection, &commit.proposal_record_id)?
        .ok_or_else(|| storage_corrupted("generation proposal replay row is missing"))?;
    let aggregate =
        read_generation_attempt_interaction_aggregate(connection, &proposal.generation_id)?;
    let (approval_evidence, approval_evidence_sha256) =
        read_generation_attempt_approval_evidence(connection, &proposal.generation_id)?;
    Ok(Some(GenerationAttemptProposalDecisionReceipt {
        proposal,
        aggregate,
        approval_evidence,
        approval_evidence_sha256,
        exact_replay: true,
    }))
}
