use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    CoreError, CoreResult, GenerationId, InteractionEvent, MessageId, Sha256Digest,
};
use lorepia_orchestration::no_applied_module_runtime_plan_sha256;
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;

use crate::{
    GenerationBeforeEventEvidence, Storage, database::storage_db_error,
    generation_attempt_derived_closure_sha256, generation_before_event_evidence_sha256,
    interaction_evaluation_seal_sha256, memory_records_at_head_snapshot_sha256,
};

use super::checkpoints::{GenerationAttemptAuthority, read_generation_attempt_authority};
use super::generation_review_authority::{
    namespace_generation_attempt_proposal_records, read_generation_attempt_before_review,
    validate_generation_attempt_before_review_commit,
    validate_generation_attempt_before_review_shape,
    validate_prepared_generation_attempt_before_review,
};
use super::proposal_records::interaction_proposal_record_id;
use super::types::{
    GenerationAttemptBeforeReviewCommit, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES,
    StoredGenerationAttemptBeforeReview, interaction_policy_sha256,
    interaction_state_snapshot_sha256,
};
use super::{
    encode_json, i64_from_u64, revision_conflict, sha256_hex, storage_corrupted,
    validate_nonempty_id,
};

#[derive(Debug)]
struct PreparedGenerationAttemptProposal {
    ordinal: u32,
    record_json: String,
    record_sha256: String,
    domain_record_id: String,
    domain_review_sha256: String,
    action_payload_json: String,
    action_payload_sha256: String,
    origin_event_id: String,
    origin_chain_ordinal: u32,
    origin_evaluation_seal_json: String,
    origin_evaluation_seal_sha256: String,
}

pub(super) struct NamespacedGenerationAttemptBeforeReview {
    pub(super) commit: GenerationAttemptBeforeReviewCommit,
    pub(super) domain_review_sha256: String,
    pub(super) domain_review_sha256_by_record_id: BTreeMap<String, String>,
}

#[derive(Debug)]
pub(super) struct PreparedGenerationAttemptBeforeReview {
    pub(super) authority: GenerationAttemptAuthority,
    event_json: String,
    pub(super) event_sha256: String,
    previous_state_json: String,
    previous_state_document_sha256: String,
    previous_state_snapshot_sha256: String,
    previous_knowledge_json: String,
    previous_knowledge_sha256: String,
    pub(super) applied_runtime_plan_sha256: String,
    module_runtime_review_json: String,
    module_runtime_review_sha256: String,
    memory_head_snapshot_json: String,
    memory_head_snapshot_sha256: String,
    source_runtime_plan_sha256: Option<String>,
    source_activation_plan_sha256: Option<String>,
    applied_runtime_plan_json: Option<String>,
    policy_json: String,
    policy_sha256: String,
    evaluation_seal_json: String,
    evaluation_seal_sha256: String,
    derived_closure_json: String,
    derived_closure_sha256: String,
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
    aggregate_state_json: String,
    aggregate_state_document_sha256: String,
    aggregate_state_snapshot_sha256: String,
    aggregate_knowledge_json: String,
    aggregate_knowledge_sha256: String,
    domain_review_sha256: String,
    proposals: Vec<PreparedGenerationAttemptProposal>,
    evidence_json: String,
    evidence_sha256: String,
}

#[derive(Serialize)]
struct GenerationAttemptBeforeCommitFingerprint<'a> {
    schema_version: u32,
    generation_id: &'a GenerationId,
    expected_attempt_revision: u64,
    event_id: &'a str,
    occurred_at: DateTime<Utc>,
    context_head_message_id: Option<&'a MessageId>,
    context_checkpoint_sha256: &'a str,
    previous_state_document_sha256: &'a str,
    previous_state_snapshot_sha256: &'a str,
    previous_knowledge_sha256: &'a str,
    applied_runtime_plan_sha256: &'a str,
    module_runtime_review_sha256: &'a str,
    memory_head_snapshot_sha256: &'a str,
    source_runtime_plan_sha256: Option<&'a str>,
    source_activation_plan_sha256: Option<&'a str>,
    policy_sha256: &'a str,
    evaluation_seal_sha256: &'a str,
    derived_closure_sha256: &'a str,
    next_state_document_sha256: &'a str,
    next_state_snapshot_sha256: &'a str,
    knowledge_sha256: &'a str,
    action_results_sha256: &'a str,
    effects_sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    derived_events_sha256: Option<&'a str>,
    proposal_writes_sha256: &'a str,
    review_sha256: &'a str,
}
impl Storage {
    /// Atomically stages one generation-owned `BeforeGeneration` review and
    /// advances the generation attempt to its exact reviewed state.
    ///
    /// No live interaction state, ordinary proposal, or UI-effect row is
    /// changed. Repeating byte-identical input returns an exact replay;
    /// conflicting input for the same attempt or event ID is rejected.
    pub fn commit_generation_attempt_before_review(
        &self,
        commit: &GenerationAttemptBeforeReviewCommit,
    ) -> CoreResult<StoredGenerationAttemptBeforeReview> {
        validate_generation_attempt_before_review_commit(commit)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let existing_identity_version = transaction
            .query_row(
                "SELECT storage_identity_version
                 FROM generation_attempt_before_event_snapshots
                 WHERE generation_id = ?1",
                [commit.generation_id.0.as_str()],
                |row| row.get::<_, u32>(0),
            )
            .optional()
            .map_err(storage_db_error)?;
        if existing_identity_version == Some(1) {
            let domain_review_sha256_by_record_id = commit
                .proposals
                .iter()
                .map(|proposal| {
                    (
                        proposal.record.id.as_str().to_owned(),
                        proposal.review_payload_sha256.clone(),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let prepared = prepare_generation_attempt_before_review(
                &transaction,
                commit,
                &commit.review_sha256,
                &domain_review_sha256_by_record_id,
            )?;
            let replay = read_generation_attempt_before_review(
                &transaction,
                &commit.generation_id,
                Some(&prepared.event_sha256),
            )?
            .ok_or_else(|| {
                storage_corrupted("legacy generation review identity vanished during replay")
            })?;
            transaction.commit().map_err(storage_db_error)?;
            return Ok(replay);
        }
        let namespaced = namespace_generation_attempt_proposal_records(commit)?;
        let commit = &namespaced.commit;
        validate_generation_attempt_before_review_shape(commit)?;
        let prepared = prepare_generation_attempt_before_review(
            &transaction,
            commit,
            &namespaced.domain_review_sha256,
            &namespaced.domain_review_sha256_by_record_id,
        )?;
        if let Some(replay) = read_generation_attempt_before_review(
            &transaction,
            &commit.generation_id,
            Some(&prepared.event_sha256),
        )? {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(replay);
        }
        validate_prepared_generation_attempt_before_review(self, &transaction, commit, &prepared)?;
        write_generation_attempt_before_review(&transaction, commit, &prepared)?;
        let stored = read_generation_attempt_before_review(
            &transaction,
            &commit.generation_id,
            Some(&prepared.event_sha256),
        )?
        .ok_or_else(|| {
            storage_corrupted("generation attempt BeforeGeneration snapshot vanished after commit")
        })?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(StoredGenerationAttemptBeforeReview {
            exact_replay: false,
            ..stored
        })
    }

    /// Reads immutable generation-owned `BeforeGeneration` evidence before
    /// any policy, module, memory, or interaction-state reevaluation.
    pub fn get_generation_attempt_before_review(
        &self,
        generation_id: &GenerationId,
    ) -> CoreResult<Option<StoredGenerationAttemptBeforeReview>> {
        validate_nonempty_id("generation attempt id", &generation_id.0)?;
        let connection = self.connection()?;
        read_generation_attempt_before_review(&connection, generation_id, None)
    }
}

pub(super) fn prepare_generation_attempt_before_review(
    transaction: &Transaction<'_>,
    commit: &GenerationAttemptBeforeReviewCommit,
    domain_review_sha256: &str,
    domain_review_sha256_by_record_id: &BTreeMap<String, String>,
) -> CoreResult<PreparedGenerationAttemptBeforeReview> {
    let authority = read_generation_attempt_authority(transaction, &commit.generation_id)?;
    let event_json = encode_json(
        "generation attempt BeforeGeneration event",
        &InteractionEvent::BeforeGeneration,
        MAX_EVENT_JSON_BYTES,
    )?;
    let previous_state_json = encode_json(
        "generation attempt previous interaction state",
        &commit.previous_state,
        MAX_STATE_JSON_BYTES,
    )?;
    let previous_state_document_sha256 = sha256_hex(previous_state_json.as_bytes());
    let previous_state_snapshot_sha256 =
        interaction_state_snapshot_sha256(&commit.previous_state, &commit.previous_knowledge)?;
    let previous_knowledge_json = encode_json(
        "generation attempt previous interaction knowledge",
        &commit.previous_knowledge,
        MAX_STATE_JSON_BYTES,
    )?;
    let previous_knowledge_sha256 = sha256_hex(previous_knowledge_json.as_bytes());

    let module_runtime_review_json = encode_json(
        "generation attempt module runtime review",
        &commit.module_runtime_review,
        MAX_STATE_JSON_BYTES,
    )?;
    let module_runtime_review_sha256 = sha256_hex(module_runtime_review_json.as_bytes());
    let memory_head_snapshot_json = encode_json(
        "generation attempt memory head snapshot",
        &commit.memory_head_snapshot,
        MAX_STATE_JSON_BYTES,
    )?;
    let memory_head_snapshot_sha256 =
        memory_records_at_head_snapshot_sha256(&commit.memory_head_snapshot)?;
    if memory_head_snapshot_sha256 != commit.memory_head_snapshot.snapshot_sha256 {
        return Err(CoreError::invalid(
            "generation attempt memory snapshot fingerprint is invalid",
        ));
    }

    let (
        applied_runtime_plan_sha256,
        source_runtime_plan_sha256,
        source_activation_plan_sha256,
        applied_runtime_plan_json,
    ) = match &commit.applied_runtime_plan {
        Some(plan) => (
            plan.applied_plan_sha256.as_str().to_owned(),
            plan.derived_from_plan_sha256
                .as_ref()
                .map(|sha256| sha256.as_str().to_owned()),
            Some(plan.source_approval.plan.plan_sha256.as_str().to_owned()),
            Some(encode_json(
                "generation attempt applied module runtime plan",
                plan,
                MAX_STATE_JSON_BYTES,
            )?),
        ),
        None => (
            no_applied_module_runtime_plan_sha256().as_str().to_owned(),
            None,
            None,
            None,
        ),
    };
    let policy_json = encode_json(
        "generation attempt interaction policy",
        &commit.policy,
        MAX_EVENT_JSON_BYTES,
    )?;
    let policy_sha256 = interaction_policy_sha256(&commit.policy)?;
    let evaluation_seal_json = encode_json(
        "generation attempt interaction evaluation seal",
        &commit.evaluation_seal,
        MAX_STATE_JSON_BYTES,
    )?;
    let evaluation_seal_sha256 = interaction_evaluation_seal_sha256(&commit.evaluation_seal)?
        .as_str()
        .to_owned();
    if commit.evaluation_seal.policy_sha256.as_str() != policy_sha256 {
        return Err(CoreError::invalid(
            "generation attempt evaluation seal differs from its policy",
        ));
    }
    let derived_closure_json = encode_json(
        "generation attempt derived closure",
        &commit.derived_closure,
        16 * 1_024 * 1_024,
    )?;
    let derived_closure_sha256 =
        generation_attempt_derived_closure_sha256(&commit.derived_closure)?
            .as_str()
            .to_owned();
    let next_state_json = encode_json(
        "generation attempt reviewed interaction state",
        &commit.next_state,
        MAX_STATE_JSON_BYTES,
    )?;
    let next_state_document_sha256 = sha256_hex(next_state_json.as_bytes());
    let next_state_snapshot_sha256 =
        interaction_state_snapshot_sha256(&commit.next_state, &commit.knowledge)?;
    let knowledge_json = encode_json(
        "generation attempt reviewed interaction knowledge",
        &commit.knowledge,
        MAX_STATE_JSON_BYTES,
    )?;
    let knowledge_sha256 = sha256_hex(knowledge_json.as_bytes());
    let action_results_json = encode_json(
        "generation attempt action results",
        &commit.action_results,
        MAX_STATE_JSON_BYTES,
    )?;
    let action_results_sha256 = sha256_hex(action_results_json.as_bytes());
    let effects_json = encode_json(
        "generation attempt effects",
        &commit.effects,
        MAX_STATE_JSON_BYTES,
    )?;
    let effects_sha256 = sha256_hex(effects_json.as_bytes());
    let derived_events_json = encode_json(
        "generation attempt derived events",
        &commit.derived_events,
        MAX_STATE_JSON_BYTES,
    )?;
    let derived_events_sha256 = sha256_hex(derived_events_json.as_bytes());
    let proposal_writes_json = encode_json(
        "generation attempt proposal writes",
        &commit.proposals,
        MAX_STATE_JSON_BYTES,
    )?;
    let proposal_writes_sha256 = sha256_hex(proposal_writes_json.as_bytes());
    let aggregate_state_json = encode_json(
        "generation attempt closure final state",
        &commit.derived_closure.final_state,
        MAX_STATE_JSON_BYTES,
    )?;
    let aggregate_state_document_sha256 = sha256_hex(aggregate_state_json.as_bytes());
    let aggregate_state_snapshot_sha256 = interaction_state_snapshot_sha256(
        &commit.derived_closure.final_state,
        &commit.derived_closure.final_knowledge,
    )?;
    let aggregate_knowledge_json = encode_json(
        "generation attempt closure final knowledge",
        &commit.derived_closure.final_knowledge,
        MAX_STATE_JSON_BYTES,
    )?;
    let aggregate_knowledge_sha256 = sha256_hex(aggregate_knowledge_json.as_bytes());

    let mut proposals = Vec::with_capacity(commit.proposals.len());
    for (ordinal, proposal) in commit.proposals.iter().enumerate() {
        let origin = commit
            .derived_closure
            .transitions
            .iter()
            .find(|transition| {
                transition
                    .proposals
                    .iter()
                    .any(|origin| origin.record.id == proposal.record.id)
            })
            .ok_or_else(|| {
                CoreError::invalid("generation proposal is missing from its derived closure")
            })?;
        let origin_evaluation_seal_json = encode_json(
            "generation proposal origin evaluation seal",
            &origin.evaluation_seal,
            MAX_STATE_JSON_BYTES,
        )?;
        let origin_evaluation_seal_sha256 =
            interaction_evaluation_seal_sha256(&origin.evaluation_seal)?
                .as_str()
                .to_owned();
        let record_json = encode_json(
            "generation attempt proposal record",
            &proposal.record,
            MAX_EVENT_JSON_BYTES,
        )?;
        let record_sha256 = sha256_hex(record_json.as_bytes());
        if record_sha256 != proposal.review_payload_sha256 {
            return Err(CoreError::invalid(
                "generation attempt proposal review hash changed",
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
                    proposal.rule_set_revision_id,
                    proposal.record.rule_id.as_str(),
                    i64::from(proposal.action_ordinal),
                ],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| CoreError::invalid("generation proposal source action is missing"))?;
        proposals.push(PreparedGenerationAttemptProposal {
            ordinal: u32::try_from(ordinal)
                .map_err(|_| CoreError::invalid("too many generation proposals"))?,
            record_json,
            record_sha256,
            domain_record_id: interaction_proposal_record_id(
                &proposal.record.rule_set_id,
                &proposal.record.rule_id,
                &proposal.record.proposal_id,
                proposal.record.source_interaction_state_revision,
            )?
            .as_str()
            .to_owned(),
            domain_review_sha256: domain_review_sha256_by_record_id
                .get(proposal.record.id.as_str())
                .cloned()
                .ok_or_else(|| {
                    storage_corrupted(
                        "generation proposal lost its original domain review fingerprint",
                    )
                })?,
            action_payload_sha256: sha256_hex(action_payload_json.as_bytes()),
            action_payload_json,
            origin_event_id: origin.event_id.clone(),
            origin_chain_ordinal: origin.ordinal,
            origin_evaluation_seal_json,
            origin_evaluation_seal_sha256,
        });
    }

    let fingerprint_json = encode_json(
        "generation attempt BeforeGeneration commit fingerprint",
        &GenerationAttemptBeforeCommitFingerprint {
            schema_version: 3,
            generation_id: &commit.generation_id,
            expected_attempt_revision: commit.expected_attempt_revision,
            event_id: &commit.event_id,
            occurred_at: commit.occurred_at,
            context_head_message_id: commit.context_head_message_id.as_ref(),
            context_checkpoint_sha256: &commit.context_checkpoint_sha256,
            previous_state_document_sha256: &previous_state_document_sha256,
            previous_state_snapshot_sha256: &previous_state_snapshot_sha256,
            previous_knowledge_sha256: &previous_knowledge_sha256,
            applied_runtime_plan_sha256: &applied_runtime_plan_sha256,
            module_runtime_review_sha256: &module_runtime_review_sha256,
            memory_head_snapshot_sha256: &memory_head_snapshot_sha256,
            source_runtime_plan_sha256: source_runtime_plan_sha256.as_deref(),
            source_activation_plan_sha256: source_activation_plan_sha256.as_deref(),
            policy_sha256: &policy_sha256,
            evaluation_seal_sha256: &evaluation_seal_sha256,
            derived_closure_sha256: &derived_closure_sha256,
            next_state_document_sha256: &next_state_document_sha256,
            next_state_snapshot_sha256: &next_state_snapshot_sha256,
            knowledge_sha256: &knowledge_sha256,
            action_results_sha256: &action_results_sha256,
            effects_sha256: &effects_sha256,
            derived_events_sha256: (!commit.derived_events.is_empty())
                .then_some(derived_events_sha256.as_str()),
            proposal_writes_sha256: &proposal_writes_sha256,
            review_sha256: &commit.review_sha256,
        },
        MAX_STATE_JSON_BYTES,
    )?;
    let event_sha256 = sha256_hex(fingerprint_json.as_bytes());
    let mut proposal_review_sha256s = commit
        .proposals
        .iter()
        .map(|proposal| {
            Sha256Digest::parse(proposal.review_payload_sha256.clone()).map_err(CoreError::invalid)
        })
        .collect::<CoreResult<Vec<_>>>()?;
    proposal_review_sha256s.sort();
    let evidence = GenerationBeforeEventEvidence {
        event_id: commit.event_id.clone(),
        event_sha256: Sha256Digest::parse(event_sha256.clone()).map_err(CoreError::invalid)?,
        context_state_revision: commit.derived_closure.final_state.revision,
        context_state_sha256: Sha256Digest::parse(aggregate_state_snapshot_sha256.clone())
            .map_err(CoreError::invalid)?,
        awaiting_approval: !commit.proposals.is_empty(),
        proposal_review_sha256s,
    };
    let evidence_json = encode_json(
        "generation attempt BeforeGeneration evidence",
        &evidence,
        MAX_EVENT_JSON_BYTES,
    )?;
    let evidence_sha256 = generation_before_event_evidence_sha256(&evidence)?
        .as_str()
        .to_owned();

    Ok(PreparedGenerationAttemptBeforeReview {
        authority,
        event_json,
        event_sha256,
        previous_state_json,
        previous_state_document_sha256,
        previous_state_snapshot_sha256,
        previous_knowledge_json,
        previous_knowledge_sha256,
        applied_runtime_plan_sha256,
        module_runtime_review_json,
        module_runtime_review_sha256,
        memory_head_snapshot_json,
        memory_head_snapshot_sha256,
        source_runtime_plan_sha256,
        source_activation_plan_sha256,
        applied_runtime_plan_json,
        policy_json,
        policy_sha256,
        evaluation_seal_json,
        evaluation_seal_sha256,
        derived_closure_json,
        derived_closure_sha256,
        next_state_json,
        next_state_document_sha256,
        next_state_snapshot_sha256,
        knowledge_json,
        knowledge_sha256,
        action_results_json,
        action_results_sha256,
        effects_json,
        effects_sha256,
        derived_events_json,
        derived_events_sha256,
        proposal_writes_json,
        proposal_writes_sha256,
        aggregate_state_json,
        aggregate_state_document_sha256,
        aggregate_state_snapshot_sha256,
        aggregate_knowledge_json,
        aggregate_knowledge_sha256,
        domain_review_sha256: domain_review_sha256.to_owned(),
        proposals,
        evidence_json,
        evidence_sha256,
    })
}
pub(super) fn write_generation_attempt_before_review(
    transaction: &Transaction<'_>,
    commit: &GenerationAttemptBeforeReviewCommit,
    prepared: &PreparedGenerationAttemptBeforeReview,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO generation_attempt_before_event_snapshots
             (generation_id, event_id, event_kind, event_json, event_sha256,
              occurred_at, context_head_message_id, context_checkpoint_sha256,
              previous_state_revision, previous_state_json,
              previous_state_document_sha256, previous_state_snapshot_sha256,
              previous_knowledge_json, previous_knowledge_sha256,
              applied_runtime_plan_sha256, module_runtime_review_json,
              module_runtime_review_sha256, memory_head_snapshot_json,
              memory_head_snapshot_sha256, source_runtime_plan_sha256,
              source_activation_plan_sha256, applied_runtime_plan_json,
              policy_json, policy_sha256, reviewed_next_state_json,
              reviewed_next_state_document_sha256,
              reviewed_next_state_snapshot_sha256, knowledge_json,
              knowledge_sha256, action_results_json, action_results_sha256,
              effects_json, effects_sha256, derived_events_json,
              derived_events_sha256, proposal_writes_json,
              proposal_writes_sha256, review_sha256, domain_review_sha256,
              storage_identity_version, evaluation_seal_json,
              evaluation_seal_sha256, derived_closure_json,
              derived_closure_sha256, closure_authority_version, created_at)
             VALUES
             (?1, ?2, 'before_generation', ?3, ?4, ?5, ?6, ?7, ?8, ?9,
              ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
              ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31,
              ?32, ?33, ?34, ?35, ?36, ?37, ?38, 2, ?39, ?40, ?41,
              ?42, 1, ?43)",
            params![
                commit.generation_id.0.as_str(),
                commit.event_id,
                prepared.event_json,
                prepared.event_sha256,
                commit.occurred_at.to_rfc3339(),
                commit
                    .context_head_message_id
                    .as_ref()
                    .map(|message_id| message_id.0.as_str()),
                commit.context_checkpoint_sha256,
                i64_from_u64(
                    "generation previous state revision",
                    commit.previous_state.revision
                )?,
                prepared.previous_state_json,
                prepared.previous_state_document_sha256,
                prepared.previous_state_snapshot_sha256,
                prepared.previous_knowledge_json,
                prepared.previous_knowledge_sha256,
                prepared.applied_runtime_plan_sha256,
                prepared.module_runtime_review_json,
                prepared.module_runtime_review_sha256,
                prepared.memory_head_snapshot_json,
                prepared.memory_head_snapshot_sha256,
                prepared.source_runtime_plan_sha256,
                prepared.source_activation_plan_sha256,
                prepared.applied_runtime_plan_json,
                prepared.policy_json,
                prepared.policy_sha256,
                prepared.next_state_json,
                prepared.next_state_document_sha256,
                prepared.next_state_snapshot_sha256,
                prepared.knowledge_json,
                prepared.knowledge_sha256,
                prepared.action_results_json,
                prepared.action_results_sha256,
                prepared.effects_json,
                prepared.effects_sha256,
                prepared.derived_events_json,
                prepared.derived_events_sha256,
                prepared.proposal_writes_json,
                prepared.proposal_writes_sha256,
                commit.review_sha256,
                prepared.domain_review_sha256,
                prepared.evaluation_seal_json,
                prepared.evaluation_seal_sha256,
                prepared.derived_closure_json,
                prepared.derived_closure_sha256,
                commit.occurred_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;

    let empty_decisions_json = "[]";
    let empty_decisions_sha256 = sha256_hex(empty_decisions_json.as_bytes());
    transaction
        .execute(
            "INSERT INTO generation_attempt_interaction_aggregates
             (generation_id, before_review_sha256, aggregate_revision,
              interaction_state_revision, state_json, state_document_sha256,
              state_snapshot_sha256, knowledge_json, knowledge_sha256,
              pending_proposal_count, terminal_decision_count,
              decision_event_ids_json, decision_event_ids_sha256,
              decision_event_sha256s_json, decision_event_sha256s_sha256,
              evaluation_seal_sha256, derived_chain_sha256,
              derived_event_count, derived_guard_count,
              closure_authority_version, created_at, updated_at)
             VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0,
                     ?10, ?11, ?10, ?11, ?12, ?13, ?14, ?15, 1,
                     ?16, ?16)",
            params![
                commit.generation_id.0.as_str(),
                commit.review_sha256,
                i64_from_u64(
                    "generation aggregate state revision",
                    commit.derived_closure.final_state.revision
                )?,
                prepared.aggregate_state_json,
                prepared.aggregate_state_document_sha256,
                prepared.aggregate_state_snapshot_sha256,
                prepared.aggregate_knowledge_json,
                prepared.aggregate_knowledge_sha256,
                i64::try_from(prepared.proposals.len())
                    .map_err(|_| CoreError::invalid("too many generation proposals"))?,
                empty_decisions_json,
                empty_decisions_sha256,
                prepared.evaluation_seal_sha256,
                commit.derived_closure.chain_sha256.as_str(),
                i64::from(commit.derived_closure.event_count),
                i64::from(commit.derived_closure.guard_count),
                commit.occurred_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;

    for (proposal, prepared_proposal) in commit.proposals.iter().zip(&prepared.proposals) {
        transaction
            .execute(
                "INSERT INTO generation_attempt_proposals
                 (proposal_record_id, generation_id, ordinal,
                  before_event_snapshot_sha256, proposal_id,
                  proposal_record_json, proposal_record_sha256,
                  proposal_review_sha256, domain_proposal_review_sha256,
                  origin_policy_json,
                  origin_policy_sha256, rule_set_revision_id, rule_id,
                  action_ordinal, action_payload_json, action_payload_sha256,
                  source_interaction_state_revision, status, proposal_revision,
                  requested_at_epoch_seconds, expires_at_epoch_seconds,
                  decision_kind, decision_idempotency_key, decision_event_id,
                  decision_event_sha256, decision_evidence_json,
                  decision_evidence_sha256, resulting_aggregate_revision,
                  resulting_state_revision, resulting_state_json,
                  resulting_state_snapshot_sha256, materialization_json,
                  materialization_sha256, decided_at_epoch_seconds,
                  domain_proposal_record_id, storage_identity_version,
                  origin_event_id, origin_chain_ordinal,
                  origin_aggregate_revision, origin_evaluation_seal_json,
                  origin_evaluation_seal_sha256, created_at, updated_at)
                 VALUES
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                  ?13, ?14, ?15, ?16, ?17, 'pending', 1, ?18, ?19,
                  NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                  NULL, NULL, NULL, ?20, 2, ?21, ?22, 1, ?23, ?24,
                  ?25, ?25)",
                params![
                    proposal.record.id.as_str(),
                    commit.generation_id.0.as_str(),
                    i64::from(prepared_proposal.ordinal),
                    commit.review_sha256,
                    proposal.record.proposal_id,
                    prepared_proposal.record_json,
                    prepared_proposal.record_sha256,
                    proposal.review_payload_sha256,
                    prepared_proposal.domain_review_sha256,
                    prepared.policy_json,
                    prepared.policy_sha256,
                    proposal.rule_set_revision_id,
                    proposal.record.rule_id.as_str(),
                    i64::from(proposal.action_ordinal),
                    prepared_proposal.action_payload_json,
                    prepared_proposal.action_payload_sha256,
                    i64_from_u64(
                        "generation proposal source state revision",
                        proposal.record.source_interaction_state_revision
                    )?,
                    proposal.record.requested_at_epoch_seconds,
                    proposal.record.expires_at_epoch_seconds,
                    prepared_proposal.domain_record_id,
                    prepared_proposal.origin_event_id,
                    i64::from(prepared_proposal.origin_chain_ordinal),
                    prepared_proposal.origin_evaluation_seal_json,
                    prepared_proposal.origin_evaluation_seal_sha256,
                    commit.occurred_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
    }

    let next_status = if prepared.proposals.is_empty() {
        "before_generation_applied"
    } else {
        "awaiting_approval"
    };
    let changed = transaction
        .execute(
            "UPDATE generation_attempt_intents
             SET status = ?2, revision = revision + 1,
                 before_generation_evidence_json = ?3,
                 before_generation_evidence_sha256 = ?4,
                 updated_at = ?5
             WHERE generation_id = ?1
               AND revision = ?6
               AND status = 'prepared'
               AND before_generation_evidence_sha256 IS NULL",
            params![
                commit.generation_id.0.as_str(),
                next_status,
                prepared.evidence_json,
                prepared.evidence_sha256,
                commit.occurred_at.to_rfc3339(),
                i64_from_u64(
                    "generation attempt expected revision",
                    commit.expected_attempt_revision
                )?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "generation attempt changed before BeforeGeneration snapshot commit",
        ));
    }
    Ok(())
}
