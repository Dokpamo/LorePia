use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use lorepia_domain::{CoreError, CoreResult, InteractionEffect, InteractionEvent, Sha256Digest};
use rusqlite::{Transaction, params};
use serde::Serialize;

use crate::InteractionEvaluationSeal;

use super::super::event_transactions::{
    StoredEventPayload, encode_interaction_evaluation_authority, encode_interaction_policy,
    interaction_event_argument_json, interaction_event_kind,
};
use super::super::types::{
    InteractionPolicySnapshot, InteractionStateKey, MAX_AUDIT_JSON_BYTES, MAX_EVENT_JSON_BYTES,
    interaction_event_sha256,
};
use super::super::{
    decode_json, encode_json, encode_u64_hex, i64_from_u64, sha256_hex, storage_corrupted,
    storage_db_error,
};
use super::{
    InteractionDerivedEventWrite, MAX_INTERACTION_DERIVED_CHAIN_DEPTH,
    MAX_INTERACTION_DERIVED_CHAIN_EVENTS, StoredInteractionDerivedEvent,
};

#[derive(Clone, Copy)]
pub(in crate::interaction_repository) struct DerivedChainParent<'a> {
    pub(in crate::interaction_repository) occurrence: &'a StoredInteractionDerivedEvent,
}

pub(in crate::interaction_repository) struct DerivedEventOutboxWrite<'a> {
    pub(in crate::interaction_repository) key: &'a InteractionStateKey,
    pub(in crate::interaction_repository) event: &'a InteractionEvent,
    pub(in crate::interaction_repository) policy: &'a InteractionPolicySnapshot,
    pub(in crate::interaction_repository) evaluation_seal: Option<&'a InteractionEvaluationSeal>,
    pub(in crate::interaction_repository) deterministic_seed: Option<u64>,
    pub(in crate::interaction_repository) effects: &'a [InteractionEffect],
    pub(in crate::interaction_repository) derived_events: &'a [InteractionDerivedEventWrite],
    pub(in crate::interaction_repository) event_id: &'a str,
    pub(in crate::interaction_repository) parent_resulting_state_revision: u64,
    pub(in crate::interaction_repository) payload_json: &'a str,
    pub(in crate::interaction_repository) created_at: DateTime<Utc>,
    pub(in crate::interaction_repository) chain_parent: Option<DerivedChainParent<'a>>,
}

pub(in crate::interaction_repository) fn write_derived_event_outbox(
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
