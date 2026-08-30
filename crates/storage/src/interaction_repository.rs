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

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    InteractionAction, InteractionEffect, InteractionEvent, InteractionProposalRecord,
    InteractionProposalRecordId, InteractionState, Sha256Digest,
};
#[cfg(test)]
use lorepia_domain::{
    InteractionProposalStatus, InteractionRuleId, KnowledgeEntryId, VariableValue, VersionedJson,
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
    StoredEventPayload, decode_stored_event_payload, encode_interaction_evaluation_authority,
    encode_interaction_policy, event_requires_argument, interaction_event_argument_json,
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
pub use derived_outbox::{
    InteractionDerivedEventCommit, InteractionDerivedEventSupervisorStatus,
    InteractionDerivedEventWrite, InteractionDerivedOccurrenceCommit,
    MAX_INTERACTION_DERIVED_CHAIN_DEPTH, MAX_INTERACTION_DERIVED_CHAIN_EVENTS,
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
    interaction_event_sha256,
};

#[derive(Clone, Copy)]
struct DerivedChainParent<'a> {
    occurrence: &'a StoredInteractionDerivedEvent,
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
