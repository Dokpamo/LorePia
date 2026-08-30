//! Durable interaction state, event, effect, and approval persistence.
//!
//! The repository deliberately accepts already-evaluated domain outcomes, but
//! it does not accept an event when approving a proposal. Approval dispatch is
//! always derived from the exact durable proposal record, which prevents a
//! caller from substituting an arbitrary user action at the persistence seam.

#![allow(clippy::too_many_lines)]

mod checkpoints;
mod derived_outbox;
mod effect_history;
mod effects;
mod event_transactions;
mod generation_materialization;
mod generation_proposal_persistence;
mod generation_proposal_queries;
mod generation_proposals;
mod generation_review;
mod generation_review_authority;
mod projections;
mod proposal_records;
mod proposals;
mod replay;
mod state;
mod types;

#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
#[cfg(test)]
use lorepia_domain::{
    ConversationBranchId, ConversationId, InteractionEffect, InteractionProposalStatus,
    InteractionRuleId, KnowledgeEntryId, Sha256Digest, VariableValue, VersionedJson,
};
use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, GenerationId, InteractionEvent,
    InteractionProposalRecord, InteractionProposalRecordId, InteractionState,
};
#[cfg(test)]
use lorepia_orchestration::{
    approve_pending, expire_pending_proposal, no_applied_module_runtime_plan_sha256, reject_pending,
};
#[cfg(test)]
use rusqlite::TransactionBehavior;
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::{GenerationAttemptDerivedClosure, StoredGenerationAttempt};
use crate::{
    InteractionEvaluationSeal, Storage, database::storage_db_error,
    interaction_evaluation_seal_sha256,
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
    StoredEventPayload, decode_stored_event_payload, event_requires_argument,
    interaction_event_kind, stored_module_plan_sha256, validate_event_generation_attempt_shape,
    validate_event_owner_message_shape, validate_policy_shape,
};
pub(crate) use generation_materialization::{
    materialize_generation_attempt_interaction_for_append,
    require_generation_attempt_prompt_context_authority_transaction,
};
pub(crate) use generation_proposal_queries::read_generation_attempt_interaction_aggregate;
#[cfg(test)]
use generation_review::{
    prepare_generation_attempt_before_review, write_generation_attempt_before_review,
};
use generation_review_authority::generation_attempt_proposal_storage_id;
pub(crate) use proposal_records::validate_generation_attempt_identity_migration_legacy_rows;
use proposal_records::{
    interaction_proposal_record_id, proposal_status, proposal_status_wire,
    validate_existing_proposals_unchanged, validate_proposal_writes, write_new_proposals,
};

pub(crate) use checkpoints::clone_interaction_checkpoint_for_branch_transaction;
use checkpoints::write_interaction_state_checkpoint;
use derived_outbox::{
    DerivedChainParent, DerivedEventOutboxWrite, require_no_pending_derived_predecessor,
    require_no_pending_derived_predecessor_through, validate_derived_event_writes,
    write_derived_event_outbox,
};
pub use derived_outbox::{
    InteractionDerivedEventCommit, InteractionDerivedEventSupervisorStatus,
    InteractionDerivedEventWrite, InteractionDerivedOccurrenceCommit,
    StoredInteractionDerivedEvent, StoredInteractionDerivedEventQuarantine,
};
use projections::{
    decode_interaction_policy, read_proposal, validate_stored_event_checkpoint_evidence,
    validate_stored_event_evaluation_authority, validate_stored_event_proposal_evidence,
};
use state::{
    read_knowledge_bindings, read_state_by_id, read_state_row, replace_normalized_state,
    require_state_for_key, require_state_revision, validate_key, validate_knowledge_bindings,
    validate_nonempty_id, validate_normalized_state, validate_state, write_state_document_only,
};
pub(crate) use types::ClonedInteractionCheckpoint;
pub use types::{
    GenerationAttemptBeforeReviewCommit, GenerationAttemptProposalDecision,
    GenerationAttemptProposalDecisionCommit, GenerationAttemptProposalDecisionReceipt,
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionChoiceEffectStatus,
    InteractionChoiceExpirationCommit, InteractionChoiceSelectionCommit,
    InteractionChoiceSelectionReceipt, InteractionEffectHistoryCursor, InteractionEventCommit,
    InteractionEventOccurrenceLookup, InteractionKnowledgeBinding,
    InteractionPolicyRuleSetRevision, InteractionPolicySnapshot, InteractionProposalApprovalCommit,
    InteractionProposalApprovalReceipt, InteractionProposalExpiryCommit,
    InteractionProposalExpiryReceipt, InteractionProposalRejectionCommit, InteractionProposalWrite,
    InteractionStateKey, StoredGenerationAttemptBeforeReview,
    StoredGenerationAttemptInteractionAggregate, StoredGenerationAttemptInteractionBoundary,
    StoredGenerationAttemptProposal, StoredInteractionEffect, StoredInteractionEffectHistory,
    StoredInteractionEvent, StoredInteractionProposal, StoredInteractionState,
    StoredInteractionStateCheckpoint, interaction_action_sha256, interaction_policy_sha256,
    interaction_proposal_review_sha256, interaction_state_key_for_branch,
    interaction_state_snapshot_sha256,
};
use types::{
    MAX_ACTION_RESULTS_PER_EVENT, MAX_AUDIT_JSON_BYTES, MAX_EFFECTS_PER_EVENT,
    MAX_EVENT_JSON_BYTES, MAX_JSON_DEPTH, MAX_JSON_NODES, MAX_STATE_JSON_BYTES,
};

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
