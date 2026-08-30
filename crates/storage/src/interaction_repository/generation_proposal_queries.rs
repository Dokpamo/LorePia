use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId,
    InteractionProposalRecord, InteractionProposalRecordId, InteractionProposalStatus,
    InteractionState, Sha256Digest,
};
use rusqlite::{Connection, OptionalExtension, Row, params};

use crate::{
    GenerationAttemptDerivedClosure, InteractionEvaluationSeal, Storage,
    database::storage_db_error, generation_attempt_derived_closure_sha256,
    interaction_evaluation_seal_sha256,
};

use super::effect_history::validate_proposal_list_limit;
use super::generation_proposals::GenerationAttemptProposalDecisionMaterialization;
use super::proposal_records::{proposal_status_from_wire, proposal_status_wire};
use super::state::{validate_knowledge_bindings, validate_nonempty_id, validate_state};
use super::types::{
    InteractionKnowledgeBinding, InteractionPolicySnapshot, MAX_EVENT_JSON_BYTES,
    MAX_STATE_JSON_BYTES, StoredGenerationAttemptInteractionAggregate,
    StoredGenerationAttemptProposal, interaction_policy_sha256, interaction_proposal_review_sha256,
    interaction_state_snapshot_sha256,
};
use super::{
    decode_json, i64_from_u64, not_found, parse_datetime, sha256_hex, storage_corrupted,
    u64_from_i64, validate_generation_attempt_proposal_storage_identity,
};

impl Storage {
    /// Loads and verifies the current isolated interaction aggregate.
    pub fn get_generation_attempt_interaction_aggregate(
        &self,
        generation_id: &GenerationId,
    ) -> CoreResult<StoredGenerationAttemptInteractionAggregate> {
        validate_nonempty_id("generation attempt id", &generation_id.0)?;
        let connection = self.connection()?;
        read_generation_attempt_interaction_aggregate(&connection, generation_id)
    }

    /// Loads one exact attempt-owned proposal.
    pub fn get_generation_attempt_proposal(
        &self,
        proposal_record_id: &InteractionProposalRecordId,
    ) -> CoreResult<StoredGenerationAttemptProposal> {
        validate_nonempty_id(
            "generation attempt proposal record id",
            proposal_record_id.as_str(),
        )?;
        let connection = self.connection()?;
        read_generation_attempt_proposal(&connection, proposal_record_id)?
            .ok_or_else(|| not_found("generation attempt proposal"))
    }

    /// Loads the exact immutable closure transition authority that created an
    /// attempt-owned proposal, including proposals emitted by prior approval
    /// closures rather than only the initial `BeforeGeneration` closure.
    pub fn get_generation_attempt_proposal_origin_closure(
        &self,
        proposal_record_id: &InteractionProposalRecordId,
    ) -> CoreResult<GenerationAttemptDerivedClosure> {
        validate_nonempty_id(
            "generation attempt proposal record id",
            proposal_record_id.as_str(),
        )?;
        let connection = self.connection()?;
        let stored = read_generation_attempt_proposal(&connection, proposal_record_id)?
            .ok_or_else(|| not_found("generation attempt proposal"))?;
        read_generation_attempt_proposal_origin_closure(&connection, &stored)
    }

    /// Lists one attempt's proposals in their immutable review order.
    pub fn list_generation_attempt_proposals(
        &self,
        generation_id: &GenerationId,
        status: InteractionProposalStatus,
        limit: u32,
    ) -> CoreResult<Vec<StoredGenerationAttemptProposal>> {
        validate_nonempty_id("generation attempt id", &generation_id.0)?;
        validate_proposal_list_limit(limit)?;
        let connection = self.connection()?;
        list_generation_attempt_proposals_query(
            &connection,
            Some(generation_id),
            None,
            None,
            status,
            limit,
        )
    }

    /// Lists attempt-owned proposals discoverable from one source room.
    ///
    /// This is the restart-safe UI discovery seam: no transient generation ID
    /// is required to restore a pending proposal after a blocked send.
    pub fn list_generation_attempt_proposals_for_source_room(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        status: InteractionProposalStatus,
        limit: u32,
    ) -> CoreResult<Vec<StoredGenerationAttemptProposal>> {
        validate_nonempty_id("generation proposal conversation id", &conversation_id.0)?;
        validate_nonempty_id("generation proposal source branch id", &source_branch_id.0)?;
        validate_proposal_list_limit(limit)?;
        let connection = self.connection()?;
        list_generation_attempt_proposals_query(
            &connection,
            None,
            Some(conversation_id),
            Some(source_branch_id),
            status,
            limit,
        )
    }
}

pub(crate) fn read_generation_attempt_interaction_aggregate(
    connection: &Connection,
    generation_id: &GenerationId,
) -> CoreResult<StoredGenerationAttemptInteractionAggregate> {
    let raw = connection
        .query_row(
            "SELECT aggregate_revision, interaction_state_revision,
                    state_json, state_document_sha256, state_snapshot_sha256,
                    knowledge_json, knowledge_sha256,
                    pending_proposal_count, terminal_decision_count,
                    decision_event_ids_json, decision_event_ids_sha256,
                    decision_event_sha256s_json,
                    decision_event_sha256s_sha256, evaluation_seal_sha256,
                    derived_chain_sha256, derived_event_count,
                    derived_guard_count, closure_authority_version,
                    created_at, updated_at
             FROM generation_attempt_interaction_aggregates
             WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, i64>(15)?,
                    row.get::<_, i64>(16)?,
                    row.get::<_, i64>(17)?,
                    row.get::<_, String>(18)?,
                    row.get::<_, String>(19)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("generation attempt interaction aggregate"))?;
    let state: InteractionState = decode_json(
        "generation attempt aggregate state",
        &raw.2,
        MAX_STATE_JSON_BYTES,
    )?;
    let knowledge: Vec<InteractionKnowledgeBinding> = decode_json(
        "generation attempt aggregate knowledge",
        &raw.5,
        MAX_STATE_JSON_BYTES,
    )?;
    validate_state(&state)?;
    validate_knowledge_bindings(&state, &knowledge)?;
    let state_revision = u64_from_i64("generation attempt aggregate state revision", raw.1)?;
    if state.revision != state_revision
        || sha256_hex(raw.2.as_bytes()) != raw.3
        || interaction_state_snapshot_sha256(&state, &knowledge)? != raw.4
        || sha256_hex(raw.5.as_bytes()) != raw.6
    {
        return Err(storage_corrupted(
            "generation attempt aggregate state fingerprint is invalid",
        ));
    }
    let decision_event_ids: Vec<String> = decode_json(
        "generation attempt decision event ids",
        &raw.9,
        MAX_EVENT_JSON_BYTES,
    )?;
    let decision_event_sha256s_raw: Vec<String> = decode_json(
        "generation attempt decision event hashes",
        &raw.11,
        MAX_EVENT_JSON_BYTES,
    )?;
    if sha256_hex(raw.9.as_bytes()) != raw.10
        || sha256_hex(raw.11.as_bytes()) != raw.12
        || decision_event_ids.len() != decision_event_sha256s_raw.len()
        || decision_event_ids.iter().any(|id| id.trim().is_empty())
    {
        return Err(storage_corrupted(
            "generation attempt decision event evidence is invalid",
        ));
    }
    let decision_event_sha256s = decision_event_sha256s_raw
        .into_iter()
        .map(|sha256| Sha256Digest::parse(sha256).map_err(CoreError::invalid))
        .collect::<CoreResult<Vec<_>>>()?;
    let pending_proposal_count = u32::try_from(raw.7)
        .map_err(|_| storage_corrupted("generation pending proposal count is invalid"))?;
    let terminal_decision_count = u32::try_from(raw.8)
        .map_err(|_| storage_corrupted("generation terminal decision count is invalid"))?;
    let actual_counts = connection
        .query_row(
            "SELECT
                 SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END),
                 SUM(CASE WHEN status != 'pending' THEN 1 ELSE 0 END)
             FROM generation_attempt_proposals
             WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                ))
            },
        )
        .map_err(storage_db_error)?;
    if actual_counts.0 != i64::from(pending_proposal_count)
        || actual_counts.1 != i64::from(terminal_decision_count)
        || decision_event_ids.len() > terminal_decision_count as usize
    {
        return Err(storage_corrupted(
            "generation attempt aggregate proposal counts are inconsistent",
        ));
    }
    let closure_authority_version = u32::try_from(raw.17)
        .map_err(|_| storage_corrupted("generation closure authority version is invalid"))?;
    if closure_authority_version != 1 {
        return Err(storage_corrupted(
            "generation aggregate has no immutable derived closure authority",
        ));
    }
    let evaluation_seal_sha256 = raw
        .13
        .ok_or_else(|| storage_corrupted("generation aggregate evaluation seal is missing"))?;
    let derived_chain_sha256 = raw
        .14
        .ok_or_else(|| storage_corrupted("generation aggregate derived chain is missing"))?;
    Ok(StoredGenerationAttemptInteractionAggregate {
        generation_id: generation_id.clone(),
        aggregate_revision: u64_from_i64("generation attempt aggregate revision", raw.0)?,
        state,
        knowledge,
        state_snapshot_sha256: Sha256Digest::parse(raw.4).map_err(CoreError::invalid)?,
        evaluation_seal_sha256: Sha256Digest::parse(evaluation_seal_sha256)
            .map_err(CoreError::invalid)?,
        derived_chain_sha256: Sha256Digest::parse(derived_chain_sha256)
            .map_err(CoreError::invalid)?,
        derived_event_count: u32::try_from(raw.15)
            .map_err(|_| storage_corrupted("generation derived event count is invalid"))?,
        derived_guard_count: u32::try_from(raw.16)
            .map_err(|_| storage_corrupted("generation derived guard count is invalid"))?,
        closure_authority_version,
        pending_proposal_count,
        terminal_decision_count,
        decision_event_ids,
        decision_event_sha256s,
        created_at: parse_datetime("generation aggregate created at", &raw.18)?,
        updated_at: parse_datetime("generation aggregate updated at", &raw.19)?,
    })
}

#[allow(clippy::type_complexity)]
fn generation_attempt_proposal_row(
    row: &Row<'_>,
) -> rusqlite::Result<(
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    i64,
    i64,
    String,
    String,
)> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        row.get(16)?,
        row.get(17)?,
        row.get(18)?,
        row.get(19)?,
        row.get(20)?,
        row.get(21)?,
        row.get(22)?,
        row.get(23)?,
        row.get(24)?,
        row.get(25)?,
        row.get(26)?,
        row.get(27)?,
        row.get(28)?,
        row.get(29)?,
        row.get(30)?,
        row.get(31)?,
        row.get(32)?,
    ))
}

type RawGenerationAttemptProposal = (
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    i64,
    i64,
    String,
    String,
);

fn decode_generation_attempt_proposal(
    raw: RawGenerationAttemptProposal,
) -> CoreResult<StoredGenerationAttemptProposal> {
    let mut record: InteractionProposalRecord = decode_json(
        "generation attempt proposal record",
        &raw.5,
        MAX_EVENT_JSON_BYTES,
    )?;
    if record.id.as_str() != raw.27
        || sha256_hex(raw.5.as_bytes()) != raw.6
        || raw.6 != raw.7
        || interaction_proposal_review_sha256(&record)? != raw.7
    {
        return Err(storage_corrupted(
            "generation attempt proposal record fingerprint is invalid",
        ));
    }
    validate_generation_attempt_proposal_storage_identity(
        &GenerationId(raw.0.clone()),
        &record,
        &InteractionProposalRecordId::from(raw.25.clone()),
        &raw.7,
        &raw.24,
        &raw.8,
        u32::try_from(raw.26)
            .map_err(|_| storage_corrupted("generation proposal identity version is invalid"))?,
    )?;
    let origin_policy: InteractionPolicySnapshot = decode_json(
        "generation attempt proposal origin policy",
        &raw.9,
        MAX_EVENT_JSON_BYTES,
    )?;
    if interaction_policy_sha256(&origin_policy)? != raw.10
        || sha256_hex(raw.14.as_bytes()) != raw.15
    {
        return Err(storage_corrupted(
            "generation attempt proposal authority fingerprint is invalid",
        ));
    }
    let origin_evaluation_seal: InteractionEvaluationSeal = decode_json(
        "generation proposal origin evaluation seal",
        &raw.31,
        MAX_STATE_JSON_BYTES,
    )?;
    let origin_evaluation_seal_sha256 =
        interaction_evaluation_seal_sha256(&origin_evaluation_seal)?;
    if origin_evaluation_seal_sha256.as_str() != raw.32
        || origin_evaluation_seal.policy_sha256.as_str() != raw.10
    {
        return Err(storage_corrupted(
            "generation proposal origin evaluation authority is invalid",
        ));
    }
    let status = proposal_status_from_wire(&raw.12)?;
    // The SQL row index above is deliberately kept compact; status is read
    // separately below by the query decoder and encoded into the record.
    record.status = status;
    record.decided_at_epoch_seconds = raw.21;
    let decision_event_sha256 = match (&raw.18, &raw.19) {
        (None, None) => None,
        (Some(_), Some(sha256)) => {
            Some(Sha256Digest::parse(sha256.clone()).map_err(CoreError::invalid)?)
        }
        _ => {
            return Err(storage_corrupted(
                "generation attempt proposal decision event is incomplete",
            ));
        }
    };
    Ok(StoredGenerationAttemptProposal {
        generation_id: GenerationId(raw.0),
        conversation_id: ConversationId(raw.1),
        source_branch_id: ConversationBranchId(raw.2),
        proposed_branch_id: ConversationBranchId(raw.3),
        ordinal: u32::try_from(raw.4)
            .map_err(|_| storage_corrupted("generation proposal ordinal is invalid"))?,
        record,
        domain_proposal_record_id: InteractionProposalRecordId::from(raw.25),
        before_event_snapshot_sha256: Sha256Digest::parse(raw.8).map_err(CoreError::invalid)?,
        origin_policy,
        origin_policy_sha256: Sha256Digest::parse(raw.10).map_err(CoreError::invalid)?,
        origin_event_id: raw.28,
        origin_chain_ordinal: u32::try_from(raw.29)
            .map_err(|_| storage_corrupted("generation proposal origin ordinal is invalid"))?,
        origin_aggregate_revision: u64_from_i64(
            "generation proposal origin aggregate revision",
            raw.30,
        )?,
        origin_evaluation_seal,
        origin_evaluation_seal_sha256,
        rule_set_revision_id: raw.11,
        action_ordinal: u32::try_from(raw.13)
            .map_err(|_| storage_corrupted("generation action ordinal is invalid"))?,
        action_payload_sha256: Sha256Digest::parse(raw.15).map_err(CoreError::invalid)?,
        proposal_revision: u64_from_i64("generation proposal revision", raw.16)?,
        proposal_review_sha256: Sha256Digest::parse(raw.7).map_err(CoreError::invalid)?,
        domain_proposal_review_sha256: Sha256Digest::parse(raw.24).map_err(CoreError::invalid)?,
        storage_identity_version: u32::try_from(raw.26)
            .map_err(|_| storage_corrupted("generation proposal identity version is invalid"))?,
        decision_idempotency_key: raw.17,
        decision_event_id: raw.18,
        decision_event_sha256,
        resulting_aggregate_revision: raw
            .20
            .map(|revision| {
                u64_from_i64("generation proposal resulting aggregate revision", revision)
            })
            .transpose()?,
        decided_at_epoch_seconds: raw.21,
        created_at: parse_datetime("generation proposal created at", &raw.22)?,
        updated_at: parse_datetime("generation proposal updated at", &raw.23)?,
    })
}

const GENERATION_ATTEMPT_PROPOSAL_SELECT: &str =
    "SELECT proposal.generation_id, attempt.conversation_id,
            attempt.source_branch_id, attempt.proposed_branch_id,
            proposal.ordinal, proposal.proposal_record_json,
            proposal.proposal_record_sha256, proposal.proposal_review_sha256,
            proposal.before_event_snapshot_sha256, proposal.origin_policy_json,
            proposal.origin_policy_sha256, proposal.rule_set_revision_id,
            proposal.status, proposal.action_ordinal,
            proposal.action_payload_json, proposal.action_payload_sha256,
            proposal.proposal_revision, proposal.decision_idempotency_key,
            proposal.decision_event_id, proposal.decision_event_sha256,
            proposal.resulting_aggregate_revision,
            proposal.decided_at_epoch_seconds, proposal.created_at,
            proposal.updated_at, proposal.domain_proposal_review_sha256,
            proposal.domain_proposal_record_id,
            proposal.storage_identity_version,
            proposal.proposal_record_id, proposal.origin_event_id,
            proposal.origin_chain_ordinal, proposal.origin_aggregate_revision,
            proposal.origin_evaluation_seal_json,
            proposal.origin_evaluation_seal_sha256
     FROM generation_attempt_proposals AS proposal
     JOIN generation_attempt_intents AS attempt
       ON attempt.generation_id = proposal.generation_id
     JOIN generation_attempt_before_event_snapshots AS snapshot
       ON snapshot.generation_id = proposal.generation_id";

fn read_generation_attempt_proposal_origin_closure(
    connection: &Connection,
    stored: &StoredGenerationAttemptProposal,
) -> CoreResult<GenerationAttemptDerivedClosure> {
    let closure = if stored.origin_aggregate_revision == 1 {
        let (json, expected_sha256) = connection
            .query_row(
                "SELECT derived_closure_json, derived_closure_sha256
                 FROM generation_attempt_before_event_snapshots
                 WHERE generation_id = ?1",
                [stored.generation_id.0.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .map_err(storage_db_error)?;
        let closure: GenerationAttemptDerivedClosure = decode_json(
            "generation proposal origin derived closure",
            &json,
            16 * 1_024 * 1_024,
        )?;
        if generation_attempt_derived_closure_sha256(&closure)?.as_str() != expected_sha256 {
            return Err(storage_corrupted(
                "generation proposal origin closure fingerprint is invalid",
            ));
        }
        closure
    } else {
        let (materialization_json, materialization_sha256) = connection
            .query_row(
                "SELECT materialization_json, materialization_sha256
                 FROM generation_attempt_proposals
                 WHERE generation_id = ?1
                   AND resulting_aggregate_revision = ?2
                   AND status = 'approved'",
                params![
                    stored.generation_id.0.as_str(),
                    i64_from_u64(
                        "generation proposal origin aggregate revision",
                        stored.origin_aggregate_revision,
                    )?,
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                storage_corrupted("generation proposal origin decision closure is missing")
            })?;
        if sha256_hex(materialization_json.as_bytes()) != materialization_sha256 {
            return Err(storage_corrupted(
                "generation proposal origin decision materialization is invalid",
            ));
        }
        let materialization: GenerationAttemptProposalDecisionMaterialization = decode_json(
            "generation proposal origin decision materialization",
            &materialization_json,
            MAX_STATE_JSON_BYTES,
        )?;
        materialization.derived_closure.ok_or_else(|| {
            storage_corrupted("generation proposal origin decision has no derived closure")
        })?
    };
    let origin = closure
        .transitions
        .get(
            usize::try_from(stored.origin_chain_ordinal)
                .map_err(|_| storage_corrupted("generation proposal origin ordinal overflowed"))?,
        )
        .filter(|transition| {
            transition.ordinal == stored.origin_chain_ordinal
                && transition.event_id == stored.origin_event_id
        })
        .ok_or_else(|| {
            storage_corrupted("generation proposal exact origin transition is missing")
        })?;
    let origin_write = origin
        .proposals
        .iter()
        .find(|write| write.record.id == stored.record.id)
        .ok_or_else(|| {
            storage_corrupted("generation proposal is absent from its origin transition")
        })?;
    let mut reviewed_record = stored.record.clone();
    reviewed_record.status = InteractionProposalStatus::Pending;
    reviewed_record.decided_at_epoch_seconds = None;
    if origin.evaluation_seal != stored.origin_evaluation_seal
        || origin.policy != stored.origin_policy
        || origin_write.record != reviewed_record
        || origin_write.rule_set_revision_id != stored.rule_set_revision_id
        || origin_write.action_ordinal != stored.action_ordinal
        || origin_write.review_payload_sha256 != stored.proposal_review_sha256.as_str()
    {
        return Err(storage_corrupted(
            "generation proposal differs from its exact immutable origin",
        ));
    }
    Ok(closure)
}

fn validate_generation_attempt_proposal_origin_lineage(
    connection: &Connection,
    stored: &StoredGenerationAttemptProposal,
) -> CoreResult<()> {
    read_generation_attempt_proposal_origin_closure(connection, stored).map(drop)
}

pub(super) fn read_generation_attempt_proposal(
    connection: &Connection,
    proposal_record_id: &InteractionProposalRecordId,
) -> CoreResult<Option<StoredGenerationAttemptProposal>> {
    let sql = format!(
        "{GENERATION_ATTEMPT_PROPOSAL_SELECT}
         WHERE proposal.proposal_record_id = ?1"
    );
    let stored = connection
        .query_row(
            &sql,
            [proposal_record_id.as_str()],
            generation_attempt_proposal_row,
        )
        .optional()
        .map_err(storage_db_error)?
        .map(decode_generation_attempt_proposal)
        .transpose()?;
    if let Some(stored) = stored.as_ref() {
        validate_generation_attempt_proposal_origin_lineage(connection, stored)?;
    }
    Ok(stored)
}

fn list_generation_attempt_proposals_query(
    connection: &Connection,
    generation_id: Option<&GenerationId>,
    conversation_id: Option<&ConversationId>,
    source_branch_id: Option<&ConversationBranchId>,
    status: InteractionProposalStatus,
    limit: u32,
) -> CoreResult<Vec<StoredGenerationAttemptProposal>> {
    let sql = format!(
        "{GENERATION_ATTEMPT_PROPOSAL_SELECT}
         WHERE ((?1 IS NOT NULL AND proposal.generation_id = ?1)
                OR (?1 IS NULL
                    AND attempt.conversation_id = ?2
                    AND attempt.source_branch_id = ?3))
           AND proposal.status = ?4
         ORDER BY
           CASE attempt.status
             WHEN 'awaiting_approval' THEN 0
             WHEN 'before_generation_applied' THEN 1
             WHEN 'dispatch_ready' THEN 2
             WHEN 'running' THEN 3
             ELSE 4
           END,
           snapshot.created_at DESC, proposal.ordinal,
           proposal.proposal_record_id
         LIMIT ?5"
    );
    let mut statement = connection.prepare(&sql).map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![
                generation_id.map(|id| id.0.as_str()),
                conversation_id.map(|id| id.0.as_str()),
                source_branch_id.map(|id| id.0.as_str()),
                proposal_status_wire(status),
                i64::from(limit),
            ],
            generation_attempt_proposal_row,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    let stored = rows
        .into_iter()
        .map(decode_generation_attempt_proposal)
        .collect::<CoreResult<Vec<_>>>()?;
    for proposal in &stored {
        validate_generation_attempt_proposal_origin_lineage(connection, proposal)?;
    }
    Ok(stored)
}
