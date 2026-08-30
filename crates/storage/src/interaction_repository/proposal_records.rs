use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, GenerationId, InteractionEffect,
    InteractionProposalRecord, InteractionProposalRecordId, InteractionProposalStatus,
    InteractionRuleId, InteractionRuleSetId, InteractionState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::projections::read_proposal;
use super::state::validate_review_hash;
use super::{
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionProposalWrite,
    MAX_AUDIT_JSON_BYTES, MAX_EVENT_JSON_BYTES, MAX_STATE_JSON_BYTES, StoredInteractionProposal,
    decode_json, encode_json, generation_attempt_proposal_storage_id, i64_from_u64,
    interaction_proposal_review_sha256, not_found, revision_conflict, sha256_hex,
    storage_corrupted, storage_db_error,
};

#[derive(Debug, Serialize)]
struct ProposalAuditPayload<'a> {
    schema_version: u32,
    proposal_record_id: &'a str,
    status: &'a str,
    state_revision: u64,
}

pub(super) fn write_new_proposals(
    transaction: &Transaction<'_>,
    state_id: &str,
    proposals: &[InteractionProposalWrite],
    resulting_state_revision: u64,
) -> CoreResult<()> {
    for proposal in proposals {
        let payload_json = encode_json(
            "interaction proposal",
            &proposal.record,
            MAX_EVENT_JSON_BYTES,
        )?;
        let payload_sha256 = sha256_hex(payload_json.as_bytes());
        if payload_sha256 != proposal.review_payload_sha256 {
            return Err(CoreError::invalid(format!(
                "interaction proposal review hash mismatch for {}",
                proposal.record.id.as_str()
            )));
        }
        transaction
            .execute(
                "INSERT INTO interaction_proposals
                 (id, interaction_state_id, rule_set_revision_id, rule_id,
                  action_ordinal, proposal_id, title, body, status,
                  source_interaction_state_revision, proposal_revision,
                  payload_json, payload_sha256, requested_at_epoch_seconds,
                  expires_at_epoch_seconds, decided_at_epoch_seconds,
                  dispatched_at_epoch_seconds)
                 VALUES (
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending',
                     ?9, 1, ?10, ?11, ?12, ?13, NULL, NULL
                 )",
                params![
                    proposal.record.id.as_str(),
                    state_id,
                    proposal.rule_set_revision_id,
                    proposal.record.rule_id.as_str(),
                    i64::from(proposal.action_ordinal),
                    proposal.record.proposal_id,
                    proposal.record.title,
                    proposal.record.body,
                    i64_from_u64(
                        "proposal source interaction state revision",
                        proposal.record.source_interaction_state_revision,
                    )?,
                    payload_json,
                    payload_sha256,
                    proposal.record.requested_at_epoch_seconds,
                    proposal.record.expires_at_epoch_seconds,
                ],
            )
            .map_err(storage_db_error)?;
        append_proposal_audit(
            transaction,
            proposal.record.id.as_str(),
            1,
            1,
            "requested",
            resulting_state_revision,
            proposal.record.requested_at_epoch_seconds,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn validate_proposal_writes(
    transaction: &Transaction<'_>,
    expected_state_revision: u64,
    next_state: &InteractionState,
    effects: &[InteractionEffect],
    action_results: &[InteractionActionResultWrite],
    proposals: &[InteractionProposalWrite],
    generation_attempt_id: Option<&GenerationId>,
    staged_before_review_sha256: Option<&str>,
) -> CoreResult<()> {
    let proposal_by_id = proposals
        .iter()
        .map(|proposal| (proposal.record.id.as_str(), proposal))
        .collect::<BTreeMap<_, _>>();
    if proposal_by_id.len() != proposals.len() {
        return Err(CoreError::invalid(
            "interaction proposal writes contain duplicate record ids",
        ));
    }
    let state_pending = next_state
        .proposals
        .iter()
        .filter(|record| record.status == InteractionProposalStatus::Pending)
        .map(|record| (record.id.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    for proposal in proposals {
        let record = &proposal.record;
        if record.status != InteractionProposalStatus::Pending
            || record.decided_at_epoch_seconds.is_some()
            || record.source_interaction_state_revision != expected_state_revision
        {
            return Err(CoreError::invalid(
                "new interaction proposals must be pending and bound to the expected state revision",
            ));
        }
        let domain_record_id = interaction_proposal_record_id(
            &record.rule_set_id,
            &record.rule_id,
            &record.proposal_id,
            record.source_interaction_state_revision,
        )?;
        let expected_record_id = match generation_attempt_id {
            Some(generation_id) => {
                let mut domain_record = record.clone();
                domain_record.id = domain_record_id.clone();
                let domain_review_sha256 = interaction_proposal_review_sha256(&domain_record)?;
                let (before_review_sha256, storage_identity_version) =
                    match staged_before_review_sha256 {
                        Some(review_sha256) => (review_sha256.to_owned(), 2_u32),
                        None => transaction
                            .query_row(
                                "SELECT snapshot.review_sha256,
                                        proposal.storage_identity_version
                                 FROM generation_attempt_before_event_snapshots AS snapshot
                                 JOIN generation_attempt_proposals AS proposal
                                   ON proposal.generation_id = snapshot.generation_id
                                  AND proposal.proposal_record_id = ?2
                                 WHERE snapshot.generation_id = ?1",
                                params![generation_id.0.as_str(), record.id.as_str()],
                                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?)),
                            )
                            .optional()
                            .map_err(storage_db_error)?
                            .ok_or_else(|| {
                                storage_corrupted("generation proposal storage binding is missing")
                            })?,
                    };
                match storage_identity_version {
                    1 => domain_record_id,
                    2 => generation_attempt_proposal_storage_id(
                        generation_id,
                        &domain_record_id,
                        &domain_review_sha256,
                        &before_review_sha256,
                    )?,
                    _ => {
                        return Err(storage_corrupted(
                            "generation proposal storage identity version is invalid",
                        ));
                    }
                }
            }
            None => domain_record_id,
        };
        if record.id != expected_record_id {
            return Err(CoreError::invalid(
                "interaction proposal record id does not match its deterministic storage binding",
            ));
        }
        if state_pending.get(record.id.as_str()).copied() != Some(record) {
            return Err(CoreError::invalid(
                "new interaction proposal is absent from the next interaction state",
            ));
        }
        validate_review_hash(proposal)?;
        validate_proposal_action_source(transaction, proposal)?;
        if !action_results.iter().any(|result| {
            result.set_revision_id == proposal.rule_set_revision_id
                && result.rule_id == proposal.record.rule_id
                && result.action_ordinal == proposal.action_ordinal
                && matches!(
                    result.status,
                    InteractionActionResultStatus::Proposed
                        | InteractionActionResultStatus::Applied
                )
        }) {
            return Err(CoreError::invalid(
                "new proposal is missing its exact durable action result",
            ));
        }
        let effect_matches = effects.iter().any(|effect| {
            matches!(
                effect,
                InteractionEffect::ApprovalRequested {
                    rule_set_id,
                    rule_id,
                    proposal_id,
                    title,
                    body,
                    expires_after_seconds,
                } if rule_set_id == &record.rule_set_id
                    && rule_id == &record.rule_id
                    && proposal_id == &record.proposal_id
                    && title == &record.title
                    && body == &record.body
                    && record.expires_at_epoch_seconds
                        == expires_after_seconds
                            .map(i64::from)
                            .and_then(|seconds| {
                                record.requested_at_epoch_seconds.checked_add(seconds)
                            })
            )
        });
        if !effect_matches {
            return Err(CoreError::invalid(
                "new proposal does not have an exact approval-requested effect",
            ));
        }
    }
    for effect in effects {
        if let InteractionEffect::ApprovalRequested {
            rule_set_id,
            rule_id,
            proposal_id,
            title,
            body,
            expires_after_seconds,
        } = effect
        {
            let matching = proposals.iter().filter(|proposal| {
                let record = &proposal.record;
                record.rule_set_id == *rule_set_id
                    && record.rule_id == *rule_id
                    && record.proposal_id == *proposal_id
                    && record.title == *title
                    && record.body == *body
                    && record.expires_at_epoch_seconds
                        == expires_after_seconds.map(i64::from).and_then(|seconds| {
                            record.requested_at_epoch_seconds.checked_add(seconds)
                        })
            });
            if matching.count() != 1 {
                return Err(CoreError::invalid(
                    "approval-requested effect must have exactly one durable proposal write",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn interaction_proposal_record_id(
    rule_set_id: &InteractionRuleSetId,
    rule_id: &InteractionRuleId,
    proposal_id: &str,
    source_revision: u64,
) -> CoreResult<InteractionProposalRecordId> {
    let mut hasher = Sha256::new();
    hash_interaction_proposal_field(&mut hasher, b"lorepia.interaction-proposal.v1")?;
    hash_interaction_proposal_field(&mut hasher, rule_set_id.as_str().as_bytes())?;
    hash_interaction_proposal_field(&mut hasher, rule_id.as_str().as_bytes())?;
    hash_interaction_proposal_field(&mut hasher, proposal_id.as_bytes())?;
    hasher.update(source_revision.to_be_bytes());
    Ok(InteractionProposalRecordId::from(hex::encode(
        hasher.finalize(),
    )))
}

fn hash_interaction_proposal_field(hasher: &mut Sha256, value: &[u8]) -> CoreResult<()> {
    let length = u64::try_from(value.len())
        .map_err(|_| CoreError::invalid("interaction proposal hash field length overflowed"))?;
    hasher.update(length.to_be_bytes());
    hasher.update(value);
    Ok(())
}

fn validate_proposal_action_source(
    transaction: &Transaction<'_>,
    proposal: &InteractionProposalWrite,
) -> CoreResult<()> {
    let raw = transaction
        .query_row(
            "SELECT revision.interaction_rule_set_id, action.action_kind,
                    action.payload_json
             FROM interaction_actions AS action
             JOIN interaction_rule_set_revisions AS revision
               ON revision.revision_id = action.set_revision_id
             WHERE action.set_revision_id = ?1
               AND action.rule_id = ?2
               AND action.ordinal = ?3",
            params![
                proposal.rule_set_revision_id,
                proposal.record.rule_id.as_str(),
                i64::from(proposal.action_ordinal),
            ],
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
        .ok_or_else(|| CoreError::invalid("proposal source action does not exist"))?;
    if raw.0 != proposal.record.rule_set_id.as_str() || raw.1 != "request_user_approval" {
        return Err(CoreError::invalid(
            "proposal source is not the exact request-user-approval action",
        ));
    }
    let action: lorepia_domain::InteractionAction =
        decode_json("proposal source action", &raw.2, MAX_EVENT_JSON_BYTES)?;
    let lorepia_domain::InteractionAction::RequestUserApproval { proposal: spec } = action else {
        return Err(storage_corrupted(
            "request-user-approval action payload has the wrong shape",
        ));
    };
    let expected_expiration = spec
        .expires_after_seconds
        .map(i64::from)
        .and_then(|seconds| {
            proposal
                .record
                .requested_at_epoch_seconds
                .checked_add(seconds)
        });
    if spec.id != proposal.record.proposal_id
        || spec.title != proposal.record.title
        || expected_expiration != proposal.record.expires_at_epoch_seconds
    {
        return Err(CoreError::invalid(
            "proposal record does not match its reviewed source action",
        ));
    }
    Ok(())
}

pub(super) fn require_pending_proposal(
    transaction: &Transaction<'_>,
    proposal_record_id: &InteractionProposalRecordId,
    expected_proposal_revision: u64,
    now_epoch_seconds: i64,
) -> CoreResult<StoredInteractionProposal> {
    let proposal = read_proposal(transaction, proposal_record_id)?
        .ok_or_else(|| not_found("interaction proposal"))?;
    if proposal.proposal_revision != expected_proposal_revision {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            format!(
                "interaction proposal revision conflict: expected {expected_proposal_revision}, current {}",
                proposal.proposal_revision
            ),
            true,
        ));
    }
    if proposal.record.status != InteractionProposalStatus::Pending {
        return Err(revision_conflict(
            "interaction proposal is no longer pending",
        ));
    }
    if now_epoch_seconds < proposal.record.requested_at_epoch_seconds {
        return Err(CoreError::invalid(
            "proposal decision timestamp precedes its request",
        ));
    }
    if proposal
        .record
        .expires_at_epoch_seconds
        .is_some_and(|expires_at| now_epoch_seconds >= expires_at)
    {
        return Err(revision_conflict("interaction proposal has expired"));
    }
    Ok(proposal)
}

pub(super) fn transition_proposal_status(
    transaction: &Transaction<'_>,
    current: &StoredInteractionProposal,
    status: InteractionProposalStatus,
    decided_at_epoch_seconds: i64,
    state_revision: u64,
) -> CoreResult<StoredInteractionProposal> {
    let audit_event_kind = match status {
        InteractionProposalStatus::Approved => "approved",
        InteractionProposalStatus::Rejected => "rejected",
        InteractionProposalStatus::Expired => "expired",
        InteractionProposalStatus::Pending => {
            return Err(CoreError::invalid(
                "pending is not a proposal decision status",
            ));
        }
    };
    let next_revision = current
        .proposal_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("interaction proposal revision overflowed"))?;
    let status_wire = proposal_status_wire(status);
    let changed = transaction
        .execute(
            "UPDATE interaction_proposals
             SET status = ?1, proposal_revision = ?2,
                 decided_at_epoch_seconds = ?3,
                 dispatched_at_epoch_seconds = ?4
             WHERE id = ?5 AND proposal_revision = ?6 AND status = 'pending'",
            params![
                status_wire,
                i64_from_u64("interaction proposal revision", next_revision)?,
                decided_at_epoch_seconds,
                Option::<i64>::None,
                current.record.id.as_str(),
                i64_from_u64(
                    "expected interaction proposal revision",
                    current.proposal_revision,
                )?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "interaction proposal compare-and-swap failed",
        ));
    }
    append_proposal_audit(
        transaction,
        current.record.id.as_str(),
        2,
        next_revision,
        audit_event_kind,
        state_revision,
        decided_at_epoch_seconds,
    )?;
    read_proposal(transaction, &current.record.id)?
        .ok_or_else(|| storage_corrupted("updated interaction proposal is missing"))
}

pub(super) fn mark_proposal_dispatched(
    transaction: &Transaction<'_>,
    approved: &StoredInteractionProposal,
    dispatched_at_epoch_seconds: i64,
    state_revision: u64,
) -> CoreResult<StoredInteractionProposal> {
    if approved.record.status != InteractionProposalStatus::Approved
        || approved.dispatched_at_epoch_seconds.is_some()
    {
        return Err(revision_conflict(
            "interaction proposal cannot be dispatched from its current state",
        ));
    }
    let next_revision = approved
        .proposal_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("interaction proposal revision overflowed"))?;
    let changed = transaction
        .execute(
            "UPDATE interaction_proposals
             SET proposal_revision = ?1, dispatched_at_epoch_seconds = ?2
             WHERE id = ?3 AND proposal_revision = ?4
               AND status = 'approved' AND dispatched_at_epoch_seconds IS NULL",
            params![
                i64_from_u64("interaction proposal revision", next_revision)?,
                dispatched_at_epoch_seconds,
                approved.record.id.as_str(),
                i64_from_u64(
                    "expected interaction proposal revision",
                    approved.proposal_revision,
                )?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "interaction proposal dispatch compare-and-swap failed",
        ));
    }
    append_proposal_audit(
        transaction,
        approved.record.id.as_str(),
        3,
        next_revision,
        "dispatched",
        state_revision,
        dispatched_at_epoch_seconds,
    )?;
    read_proposal(transaction, &approved.record.id)?
        .ok_or_else(|| storage_corrupted("dispatched interaction proposal is missing"))
}

fn append_proposal_audit(
    transaction: &Transaction<'_>,
    proposal_record_id: &str,
    sequence: u64,
    proposal_revision: u64,
    event_kind: &str,
    state_revision: u64,
    created_at_epoch_seconds: i64,
) -> CoreResult<()> {
    let payload_json = encode_json(
        "interaction proposal audit",
        &ProposalAuditPayload {
            schema_version: 1,
            proposal_record_id,
            status: event_kind,
            state_revision,
        },
        MAX_AUDIT_JSON_BYTES,
    )?;
    transaction
        .execute(
            "INSERT INTO interaction_proposal_audit
             (proposal_id, sequence, proposal_revision, event_kind,
              payload_json, created_at_epoch_seconds)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                proposal_record_id,
                i64_from_u64("proposal audit sequence", sequence)?,
                i64_from_u64("proposal audit revision", proposal_revision)?,
                event_kind,
                payload_json,
                created_at_epoch_seconds,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(super) fn validate_existing_proposals_unchanged(
    transaction: &Transaction<'_>,
    state_id: &str,
    current_state: &InteractionState,
    next_state: &InteractionState,
    proposal_writes: &[InteractionProposalWrite],
) -> CoreResult<()> {
    let durable_document = transaction
        .query_row(
            "SELECT document_json FROM interaction_state WHERE id = ?1",
            [state_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("current interaction state row is missing"))?;
    let current_document = encode_json("interaction state", current_state, MAX_STATE_JSON_BYTES)?;
    if current_document != durable_document {
        return Err(storage_corrupted(
            "current interaction state document is not the durable state",
        ));
    }
    let proposal_ids = {
        let mut statement = transaction
            .prepare(
                "SELECT id
                 FROM interaction_proposals
                 WHERE interaction_state_id = ?1
                 ORDER BY id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([state_id], |row| row.get::<_, String>(0))
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let current_by_id = current_state
        .proposals
        .iter()
        .map(|proposal| (proposal.id.as_str(), proposal))
        .collect::<BTreeMap<_, _>>();
    let next_by_id = next_state
        .proposals
        .iter()
        .map(|proposal| (proposal.id.as_str(), proposal))
        .collect::<BTreeMap<_, _>>();
    if current_by_id.len() != current_state.proposals.len()
        || next_by_id.len() != next_state.proposals.len()
    {
        return Err(CoreError::invalid(
            "interaction state contains duplicate proposal record ids",
        ));
    }

    let existing_ids = proposal_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if current_by_id.keys().copied().collect::<BTreeSet<_>>() != existing_ids {
        return Err(storage_corrupted(
            "interaction proposal rows differ from the state document",
        ));
    }
    for id in &proposal_ids {
        let durable = read_proposal(transaction, &InteractionProposalRecordId::from(id.clone()))?
            .ok_or_else(|| {
            storage_corrupted("interaction proposal vanished while validating state")
        })?;
        if current_by_id.get(id.as_str()).copied() != Some(&durable.record) {
            return Err(storage_corrupted(
                "interaction proposal row differs from the state document",
            ));
        }
        if next_by_id.get(id.as_str()).copied() != Some(&durable.record) {
            return Err(CoreError::invalid(
                "ordinary interaction events cannot mutate or remove existing proposal records",
            ));
        }
    }

    let new_state_ids = next_by_id
        .keys()
        .copied()
        .filter(|id| !existing_ids.contains(id))
        .collect::<BTreeSet<_>>();
    let write_ids = proposal_writes
        .iter()
        .map(|proposal| proposal.record.id.as_str())
        .collect::<BTreeSet<_>>();
    if new_state_ids != write_ids || write_ids.len() != proposal_writes.len() {
        return Err(CoreError::invalid(
            "new state proposal records must exactly match proposal writes",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) fn validate_generation_attempt_identity_migration_legacy_rows(
    connection: &Connection,
) -> CoreResult<()> {
    let malformed_snapshot_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM generation_attempt_before_event_snapshots AS snapshot
             LEFT JOIN generation_attempt_interaction_aggregates AS aggregate
               ON aggregate.generation_id = snapshot.generation_id
             WHERE aggregate.generation_id IS NULL
                OR length(snapshot.review_sha256) != 64
                OR snapshot.review_sha256 GLOB '*[^0-9a-f]*'
                OR aggregate.before_review_sha256 != snapshot.review_sha256",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)?;
    if malformed_snapshot_count != 0 {
        return Err(storage_corrupted(
            "legacy generation review identity is malformed",
        ));
    }

    let mut expected_writes_by_generation = {
        let mut statement = connection
            .prepare(
                "SELECT generation_id, proposal_writes_json
                 FROM generation_attempt_before_event_snapshots
                 ORDER BY generation_id",
            )
            .map_err(storage_db_error)?;
        let snapshots = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        let mut expected = BTreeMap::new();
        for (generation_id, writes_json) in snapshots {
            let writes: Vec<InteractionProposalWrite> = decode_json(
                "legacy generation proposal writes",
                &writes_json,
                MAX_STATE_JSON_BYTES,
            )?;
            let mut writes_by_id = BTreeMap::new();
            for write in writes {
                let expected_domain_id = interaction_proposal_record_id(
                    &write.record.rule_set_id,
                    &write.record.rule_id,
                    &write.record.proposal_id,
                    write.record.source_interaction_state_revision,
                )?;
                if expected_domain_id != write.record.id
                    || interaction_proposal_review_sha256(&write.record)?
                        != write.review_payload_sha256
                    || writes_by_id
                        .insert(write.record.id.as_str().to_owned(), write)
                        .is_some()
                {
                    return Err(storage_corrupted(
                        "legacy generation proposal identity is malformed",
                    ));
                }
            }
            if expected.insert(generation_id, writes_by_id).is_some() {
                return Err(storage_corrupted(
                    "legacy generation review identity is malformed",
                ));
            }
        }
        expected
    };

    let raw = {
        let mut statement = connection
            .prepare(
                "SELECT proposal.generation_id, proposal.proposal_record_id,
                        proposal.proposal_record_json,
                        proposal.proposal_record_sha256,
                        proposal.proposal_review_sha256,
                        proposal.before_event_snapshot_sha256,
                        snapshot.review_sha256
                 FROM generation_attempt_proposals AS proposal
                 JOIN generation_attempt_before_event_snapshots AS snapshot
                   ON snapshot.generation_id = proposal.generation_id
                 ORDER BY proposal.generation_id, proposal.ordinal",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    for (
        generation_id,
        proposal_record_id,
        record_json,
        record_sha256,
        review_sha256,
        before_review_sha256,
        snapshot_review_sha256,
    ) in raw
    {
        let record: InteractionProposalRecord = decode_json(
            "legacy generation proposal record",
            &record_json,
            MAX_EVENT_JSON_BYTES,
        )?;
        let matching_write = expected_writes_by_generation
            .get_mut(&generation_id)
            .and_then(|writes| writes.remove(&proposal_record_id));
        let expected_domain_id = interaction_proposal_record_id(
            &record.rule_set_id,
            &record.rule_id,
            &record.proposal_id,
            record.source_interaction_state_revision,
        )?;
        if record.id.as_str() != proposal_record_id
            || expected_domain_id != record.id
            || before_review_sha256 != snapshot_review_sha256
            || sha256_hex(record_json.as_bytes()) != record_sha256
            || record_sha256 != review_sha256
            || interaction_proposal_review_sha256(&record)? != review_sha256
            || matching_write.is_none_or(|write| {
                write.record != record || write.review_payload_sha256 != review_sha256
            })
        {
            return Err(storage_corrupted(
                "legacy generation proposal identity is malformed",
            ));
        }
    }
    if expected_writes_by_generation
        .values()
        .any(|writes| !writes.is_empty())
    {
        return Err(storage_corrupted(
            "legacy generation proposal identity is malformed",
        ));
    }
    Ok(())
}

pub(super) fn proposal_status(status: &str) -> CoreResult<InteractionProposalStatus> {
    match status {
        "pending" => Ok(InteractionProposalStatus::Pending),
        "approved" => Ok(InteractionProposalStatus::Approved),
        "rejected" => Ok(InteractionProposalStatus::Rejected),
        "expired" => Ok(InteractionProposalStatus::Expired),
        _ => Err(storage_corrupted(format!(
            "stored interaction proposal status `{status}` is invalid"
        ))),
    }
}

pub(super) fn proposal_status_wire(status: InteractionProposalStatus) -> &'static str {
    match status {
        InteractionProposalStatus::Pending => "pending",
        InteractionProposalStatus::Approved => "approved",
        InteractionProposalStatus::Rejected => "rejected",
        InteractionProposalStatus::Expired => "expired",
    }
}

pub(super) fn proposal_status_from_wire(value: &str) -> CoreResult<InteractionProposalStatus> {
    match value {
        "pending" => Ok(InteractionProposalStatus::Pending),
        "approved" => Ok(InteractionProposalStatus::Approved),
        "rejected" => Ok(InteractionProposalStatus::Rejected),
        "expired" => Ok(InteractionProposalStatus::Expired),
        _ => Err(storage_corrupted(
            "stored interaction proposal status is invalid",
        )),
    }
}
