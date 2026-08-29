use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, InteractionEffect,
};
use rusqlite::{Connection, OptionalExtension, Row, params};
use sha2::{Digest, Sha256};

use super::{
    InteractionChoiceEffectStatus, InteractionEffectHistoryCursor, MAX_EVENT_JSON_BYTES,
    StoredInteractionEffect, StoredInteractionEffectHistory, decode_interaction_policy,
    decode_json, i64_from_u64, parse_datetime, revision_conflict, storage_corrupted, u64_from_i64,
    validate_nonempty_id, validate_stored_interaction_policy_rule_sets,
};
use crate::database::storage_db_error;

pub(super) fn effect_outbox_kind(effect: &InteractionEffect) -> Option<&'static str> {
    match effect {
        InteractionEffect::AssetShown { .. } => Some("asset_shown"),
        InteractionEffect::AudioRequested { .. } => Some("audio_requested"),
        InteractionEffect::ChoicesPresented { .. } => Some("choices_presented"),
        InteractionEffect::VisibleSystemEvent { .. } => Some("visible_system_event"),
        InteractionEffect::DiceRolled { .. } => Some("dice_rolled"),
        InteractionEffect::ApprovalRequested { .. } => Some("approval_requested"),
        InteractionEffect::VariableSet { .. } | InteractionEffect::KnowledgeActivated { .. } => {
            None
        }
    }
}

#[derive(Debug)]
struct RawEffectHistoryRow {
    effect_id: String,
    event_id: String,
    sequence: i64,
    effect_kind: String,
    effect_json: String,
    available_at: String,
    delivery_attempts: i64,
    delivered_at: Option<String>,
    choice_status: Option<String>,
    selected_choice_id: Option<String>,
    choice_decided_at_epoch_seconds: Option<i64>,
    interaction_state_id: String,
    conversation_id: String,
    branch_id: String,
    resulting_state_revision: i64,
    event_created_at: String,
    module_plan_sha256: String,
    policy_json: String,
    policy_sha256: String,
}

fn raw_effect_history_row(row: &Row<'_>) -> rusqlite::Result<RawEffectHistoryRow> {
    Ok(RawEffectHistoryRow {
        effect_id: row.get(0)?,
        event_id: row.get(1)?,
        sequence: row.get(2)?,
        effect_kind: row.get(3)?,
        effect_json: row.get(4)?,
        available_at: row.get(5)?,
        delivery_attempts: row.get(6)?,
        delivered_at: row.get(7)?,
        choice_status: row.get(8)?,
        selected_choice_id: row.get(9)?,
        choice_decided_at_epoch_seconds: row.get(10)?,
        interaction_state_id: row.get(11)?,
        conversation_id: row.get(12)?,
        branch_id: row.get(13)?,
        resulting_state_revision: row.get(14)?,
        event_created_at: row.get(15)?,
        module_plan_sha256: row.get(16)?,
        policy_json: row.get(17)?,
        policy_sha256: row.get(18)?,
    })
}

fn decode_effect_history_row(
    raw: RawEffectHistoryRow,
) -> CoreResult<StoredInteractionEffectHistory> {
    validate_stored_effect_identity(&raw.effect_id, &raw.event_id, raw.sequence)?;
    let effect: InteractionEffect = decode_json(
        "stored interaction effect",
        &raw.effect_json,
        MAX_EVENT_JSON_BYTES,
    )?;
    if effect_outbox_kind(&effect) != Some(raw.effect_kind.as_str()) {
        return Err(storage_corrupted(
            "interaction effect kind differs from its stored payload",
        ));
    }
    let choice_status = decode_choice_effect_lifecycle(
        &effect,
        raw.choice_status.as_deref(),
        raw.selected_choice_id.as_deref(),
        raw.choice_decided_at_epoch_seconds,
    )?;
    let policy = decode_interaction_policy(
        &raw.module_plan_sha256,
        &raw.policy_json,
        &raw.policy_sha256,
    )?;
    let replay_on_reopen = !matches!(&effect, InteractionEffect::AudioRequested { .. });
    Ok(StoredInteractionEffectHistory {
        stored: StoredInteractionEffect {
            effect_id: raw.effect_id,
            event_id: raw.event_id,
            sequence: u64_from_i64("interaction effect sequence", raw.sequence)?,
            interaction_state_id: raw.interaction_state_id,
            conversation_id: ConversationId(raw.conversation_id),
            branch_id: ConversationBranchId(raw.branch_id),
            resulting_state_revision: u64_from_i64(
                "interaction effect resulting state revision",
                raw.resulting_state_revision,
            )?,
            event_created_at: parse_datetime(
                "interaction effect event created_at",
                &raw.event_created_at,
            )?,
            policy,
            policy_sha256: raw.policy_sha256,
            effect,
            available_at: parse_datetime("interaction effect available_at", &raw.available_at)?,
            delivery_attempts: u64_from_i64(
                "interaction effect delivery attempts",
                raw.delivery_attempts,
            )?,
            delivered_at: raw
                .delivered_at
                .map(|value| parse_datetime("interaction effect delivered_at", &value))
                .transpose()?,
            choice_status,
            selected_choice_id: raw.selected_choice_id,
            choice_decided_at_epoch_seconds: raw.choice_decided_at_epoch_seconds,
        },
        replay_on_reopen,
    })
}

fn decode_and_validate_effect_history_row(
    connection: &Connection,
    raw: RawEffectHistoryRow,
) -> CoreResult<StoredInteractionEffectHistory> {
    let decoded = decode_effect_history_row(raw)?;
    validate_stored_interaction_policy_rule_sets(
        connection,
        &decoded.stored.event_id,
        &decoded.stored.policy,
    )?;
    Ok(decoded)
}

pub(super) fn read_effect_history(
    connection: &Connection,
    effect_id: &str,
) -> CoreResult<Option<StoredInteractionEffectHistory>> {
    connection
        .query_row(
            "SELECT effect.effect_id, effect.event_id, effect.sequence,
                    effect.effect_kind, effect.effect_json,
                    effect.available_at, effect.delivery_attempts,
                    effect.delivered_at, effect.choice_status,
                    effect.choice_id, effect.choice_decided_at_epoch_seconds,
                    event.interaction_state_id, event.conversation_id,
                    event.branch_id, event.resulting_state_revision,
                    event.created_at, event.module_plan_sha256,
                    event.policy_json, event.policy_sha256
             FROM interaction_effect_outbox AS effect
             JOIN interaction_events AS event ON event.id = effect.event_id
             WHERE effect.effect_id = ?1",
            [effect_id],
            raw_effect_history_row,
        )
        .optional()
        .map_err(storage_db_error)?
        .map(|raw| decode_and_validate_effect_history_row(connection, raw))
        .transpose()
}

pub(super) fn read_effect_history_page(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    after: Option<InteractionEffectHistoryCursor>,
    limit: u32,
    reopen_only: bool,
) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
    let after_revision = after
        .map(|cursor| {
            i64_from_u64(
                "interaction effect history cursor revision",
                cursor.resulting_state_revision,
            )
        })
        .transpose()?;
    let after_sequence = after.map_or(Ok(0_i64), |cursor| {
        i64_from_u64(
            "interaction effect history cursor sequence",
            cursor.sequence,
        )
    })?;
    let mut statement = connection
        .prepare(
            "SELECT effect.effect_id, effect.event_id, effect.sequence,
                    effect.effect_kind, effect.effect_json,
                    effect.available_at, effect.delivery_attempts,
                    effect.delivered_at, effect.choice_status,
                    effect.choice_id, effect.choice_decided_at_epoch_seconds,
                    event.interaction_state_id, event.conversation_id,
                    event.branch_id, event.resulting_state_revision,
                    event.created_at, event.module_plan_sha256,
                    event.policy_json, event.policy_sha256
             FROM interaction_effect_outbox AS effect
             JOIN interaction_events AS event ON event.id = effect.event_id
             WHERE event.conversation_id = ?1 AND event.branch_id = ?2
               AND (
                    ?3 IS NULL
                    OR event.resulting_state_revision > ?3
                    OR (
                        event.resulting_state_revision = ?3
                        AND effect.sequence > ?4
                    )
               )
               AND (?5 = 0 OR effect.effect_kind != 'audio_requested')
             ORDER BY event.resulting_state_revision ASC, effect.sequence ASC
             LIMIT ?6",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                after_revision,
                after_sequence,
                i64::from(reopen_only),
                i64::from(limit),
            ],
            raw_effect_history_row,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|raw| decode_and_validate_effect_history_row(connection, raw))
        .collect()
}

pub(super) fn read_recent_reopen_effect_history(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    limit: u32,
) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
    let mut statement = connection
        .prepare(
            "SELECT effect.effect_id, effect.event_id, effect.sequence,
                    effect.effect_kind, effect.effect_json,
                    effect.available_at, effect.delivery_attempts,
                    effect.delivered_at, effect.choice_status,
                    effect.choice_id, effect.choice_decided_at_epoch_seconds,
                    event.interaction_state_id, event.conversation_id,
                    event.branch_id, event.resulting_state_revision,
                    event.created_at, event.module_plan_sha256,
                    event.policy_json, event.policy_sha256
             FROM interaction_effect_outbox AS effect
             JOIN interaction_events AS event ON event.id = effect.event_id
             WHERE event.conversation_id = ?1 AND event.branch_id = ?2
               AND effect.effect_kind != 'audio_requested'
             ORDER BY event.resulting_state_revision DESC, effect.sequence DESC
             LIMIT ?3",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                i64::from(limit),
            ],
            raw_effect_history_row,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    let mut decoded = rows
        .into_iter()
        .map(|raw| decode_and_validate_effect_history_row(connection, raw))
        .collect::<CoreResult<Vec<_>>>()?;
    decoded.reverse();
    Ok(decoded)
}

pub(super) fn read_older_reopen_effect_history(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    before: InteractionEffectHistoryCursor,
    limit: u32,
) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
    let before_revision = i64_from_u64(
        "interaction effect history cursor revision",
        before.resulting_state_revision,
    )?;
    let before_sequence = i64_from_u64(
        "interaction effect history cursor sequence",
        before.sequence,
    )?;
    let mut statement = connection
        .prepare(
            "SELECT effect.effect_id, effect.event_id, effect.sequence,
                    effect.effect_kind, effect.effect_json,
                    effect.available_at, effect.delivery_attempts,
                    effect.delivered_at, effect.choice_status,
                    effect.choice_id, effect.choice_decided_at_epoch_seconds,
                    event.interaction_state_id, event.conversation_id,
                    event.branch_id, event.resulting_state_revision,
                    event.created_at, event.module_plan_sha256,
                    event.policy_json, event.policy_sha256
             FROM interaction_effect_outbox AS effect
             JOIN interaction_events AS event ON event.id = effect.event_id
             WHERE event.conversation_id = ?1 AND event.branch_id = ?2
               AND effect.effect_kind != 'audio_requested'
               AND (
                    event.resulting_state_revision < ?3
                    OR (
                        event.resulting_state_revision = ?3
                        AND effect.sequence < ?4
                    )
               )
             ORDER BY event.resulting_state_revision DESC, effect.sequence DESC
             LIMIT ?5",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                before_revision,
                before_sequence,
                i64::from(limit),
            ],
            raw_effect_history_row,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    let mut decoded = rows
        .into_iter()
        .map(|raw| decode_and_validate_effect_history_row(connection, raw))
        .collect::<CoreResult<Vec<_>>>()?;
    decoded.reverse();
    Ok(decoded)
}

pub(super) fn read_latest_region_effects(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
    let mut statement = connection
        .prepare(
            "WITH ranked AS (
                 SELECT effect.effect_id, effect.event_id, effect.sequence,
                        effect.effect_kind, effect.effect_json,
                        effect.available_at, effect.delivery_attempts,
                        effect.delivered_at, effect.choice_status,
                        effect.choice_id,
                        effect.choice_decided_at_epoch_seconds,
                        event.interaction_state_id, event.conversation_id,
                        event.branch_id, event.resulting_state_revision,
                        event.created_at, event.module_plan_sha256,
                        event.policy_json, event.policy_sha256,
                        ROW_NUMBER() OVER (
                            PARTITION BY json_extract(effect.effect_json, '$.region')
                            ORDER BY event.resulting_state_revision DESC,
                                     effect.sequence DESC
                        ) AS region_rank
                 FROM interaction_effect_outbox AS effect
                 JOIN interaction_events AS event ON event.id = effect.event_id
                 WHERE event.conversation_id = ?1 AND event.branch_id = ?2
                   AND effect.effect_kind = 'asset_shown'
             )
             SELECT effect_id, event_id, sequence, effect_kind, effect_json,
                    available_at, delivery_attempts, delivered_at,
                    choice_status, choice_id,
                    choice_decided_at_epoch_seconds, interaction_state_id,
                    conversation_id, branch_id, resulting_state_revision,
                    created_at, module_plan_sha256, policy_json, policy_sha256
             FROM ranked
             WHERE region_rank = 1
             ORDER BY resulting_state_revision ASC, sequence ASC",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![conversation_id.0.as_str(), branch_id.0.as_str()],
            raw_effect_history_row,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|raw| decode_and_validate_effect_history_row(connection, raw))
        .collect()
}

pub(super) fn read_pending_choice_effect_history(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    limit: u32,
) -> CoreResult<Vec<StoredInteractionEffectHistory>> {
    let mut statement = connection
        .prepare(
            "SELECT effect.effect_id, effect.event_id, effect.sequence,
                    effect.effect_kind, effect.effect_json,
                    effect.available_at, effect.delivery_attempts,
                    effect.delivered_at, effect.choice_status,
                    effect.choice_id, effect.choice_decided_at_epoch_seconds,
                    event.interaction_state_id, event.conversation_id,
                    event.branch_id, event.resulting_state_revision,
                    event.created_at, event.module_plan_sha256,
                    event.policy_json, event.policy_sha256
             FROM interaction_effect_outbox AS effect
             JOIN interaction_events AS event ON event.id = effect.event_id
             WHERE event.conversation_id = ?1 AND event.branch_id = ?2
               AND effect.effect_kind = 'choices_presented'
               AND effect.choice_status = 'pending'
             ORDER BY event.resulting_state_revision ASC, effect.sequence ASC
             LIMIT ?3",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![
                conversation_id.0.as_str(),
                branch_id.0.as_str(),
                i64::from(limit),
            ],
            raw_effect_history_row,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|raw| decode_and_validate_effect_history_row(connection, raw))
        .collect()
}

pub(super) fn require_pending_choice_effect(
    choice_effect: &StoredInteractionEffectHistory,
    decided_at_epoch_seconds: i64,
) -> CoreResult<()> {
    if !matches!(
        &choice_effect.stored.effect,
        InteractionEffect::ChoicesPresented { .. }
    ) {
        return Err(CoreError::invalid(
            "interaction effect is not a choice presentation",
        ));
    }
    if choice_effect.stored.choice_status != Some(InteractionChoiceEffectStatus::Pending) {
        return Err(revision_conflict(
            "interaction choice was already consumed or expired",
        ));
    }
    if decided_at_epoch_seconds < choice_effect.stored.event_created_at.timestamp() {
        return Err(CoreError::invalid(
            "interaction choice decision timestamp precedes its presentation",
        ));
    }
    Ok(())
}

pub(super) fn require_pending_choice(
    choice_effect: &StoredInteractionEffectHistory,
    selected_choice_id: &str,
    selected_at_epoch_seconds: i64,
) -> CoreResult<()> {
    require_pending_choice_effect(choice_effect, selected_at_epoch_seconds)?;
    let InteractionEffect::ChoicesPresented { choices } = &choice_effect.stored.effect else {
        unreachable!("pending choice validation checked the effect kind")
    };
    if !choices.iter().any(|choice| choice.id == selected_choice_id) {
        return Err(CoreError::invalid(
            "selected interaction choice is absent from the durable effect",
        ));
    }
    Ok(())
}

pub(super) fn read_pending_effects(
    connection: &Connection,
    now: DateTime<Utc>,
    limit: u32,
) -> CoreResult<Vec<StoredInteractionEffect>> {
    let mut statement = connection
        .prepare(
            "SELECT effect.effect_id, effect.event_id, effect.sequence,
                    effect.effect_kind, effect.effect_json,
                    effect.available_at, effect.delivery_attempts,
                    effect.delivered_at, effect.choice_status,
                    effect.choice_id, effect.choice_decided_at_epoch_seconds,
                    event.interaction_state_id, event.conversation_id,
                    event.branch_id, event.resulting_state_revision,
                    event.created_at, event.module_plan_sha256,
                    event.policy_json, event.policy_sha256
             FROM interaction_effect_outbox AS effect
             JOIN interaction_events AS event ON event.id = effect.event_id
             WHERE effect.delivered_at IS NULL
               AND effect.available_at <= ?1
             ORDER BY effect.available_at, effect.event_id, effect.sequence
             LIMIT ?2",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(params![now.to_rfc3339(), i64::from(limit)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, String>(15)?,
                row.get::<_, String>(16)?,
                row.get::<_, String>(17)?,
                row.get::<_, String>(18)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(
            |(
                effect_id,
                event_id,
                sequence,
                effect_kind,
                effect_json,
                available_at,
                attempts,
                delivered_at,
                choice_status,
                selected_choice_id,
                choice_decided_at,
                interaction_state_id,
                conversation_id,
                branch_id,
                resulting_state_revision,
                event_created_at,
                module_plan_sha256,
                policy_json,
                policy_sha256,
            )| {
                let effect: InteractionEffect = decode_json(
                    "stored interaction effect",
                    &effect_json,
                    MAX_EVENT_JSON_BYTES,
                )?;
                if effect_outbox_kind(&effect) != Some(effect_kind.as_str()) {
                    return Err(storage_corrupted(
                        "interaction effect kind differs from its stored payload",
                    ));
                }
                validate_stored_effect_identity(&effect_id, &event_id, sequence)?;
                let choice_status = decode_choice_effect_lifecycle(
                    &effect,
                    choice_status.as_deref(),
                    selected_choice_id.as_deref(),
                    choice_decided_at,
                )?;
                let policy =
                    decode_interaction_policy(&module_plan_sha256, &policy_json, &policy_sha256)?;
                validate_stored_interaction_policy_rule_sets(connection, &event_id, &policy)?;
                Ok(StoredInteractionEffect {
                    effect_id,
                    event_id,
                    sequence: u64_from_i64("interaction effect sequence", sequence)?,
                    interaction_state_id,
                    conversation_id: ConversationId(conversation_id),
                    branch_id: ConversationBranchId(branch_id),
                    resulting_state_revision: u64_from_i64(
                        "interaction effect resulting state revision",
                        resulting_state_revision,
                    )?,
                    event_created_at: parse_datetime(
                        "interaction effect event created_at",
                        &event_created_at,
                    )?,
                    policy,
                    policy_sha256,
                    effect,
                    available_at: parse_datetime("interaction effect available_at", &available_at)?,
                    delivery_attempts: u64_from_i64(
                        "interaction effect delivery attempts",
                        attempts,
                    )?,
                    delivered_at: delivered_at
                        .map(|value| parse_datetime("interaction effect delivered_at", &value))
                        .transpose()?,
                    choice_status,
                    selected_choice_id,
                    choice_decided_at_epoch_seconds: choice_decided_at,
                })
            },
        )
        .collect()
}

pub(super) fn validate_effect_poll_limit(limit: u32) -> CoreResult<()> {
    if limit == 0 || limit > 1_024 {
        return Err(CoreError::invalid(
            "interaction effect poll limit must be between 1 and 1024",
        ));
    }
    Ok(())
}

pub(super) fn validate_proposal_list_limit(limit: u32) -> CoreResult<()> {
    if limit == 0 || limit > 1_024 {
        return Err(CoreError::invalid(
            "interaction proposal list limit must be between 1 and 1024",
        ));
    }
    Ok(())
}

pub(super) fn validate_stored_effect_identity(
    effect_id: &str,
    event_id: &str,
    sequence: i64,
) -> CoreResult<()> {
    if interaction_effect_id(event_id, sequence) != effect_id {
        return Err(storage_corrupted(
            "interaction effect ID differs from its event and sequence",
        ));
    }
    Ok(())
}

pub(super) fn decode_choice_effect_lifecycle(
    effect: &InteractionEffect,
    status: Option<&str>,
    selected_choice_id: Option<&str>,
    decided_at_epoch_seconds: Option<i64>,
) -> CoreResult<Option<InteractionChoiceEffectStatus>> {
    let InteractionEffect::ChoicesPresented { choices } = effect else {
        if status.is_some() || selected_choice_id.is_some() || decided_at_epoch_seconds.is_some() {
            return Err(storage_corrupted(
                "non-choice interaction effect has choice lifecycle metadata",
            ));
        }
        return Ok(None);
    };
    if choices.is_empty() {
        return match (status, selected_choice_id, decided_at_epoch_seconds) {
            (Some("pending"), None, None) => Ok(Some(InteractionChoiceEffectStatus::Pending)),
            (Some("expired"), None, Some(decided_at)) if decided_at >= 0 => {
                Ok(Some(InteractionChoiceEffectStatus::Expired))
            }
            _ => Err(storage_corrupted(
                "empty legacy choice effect has inconsistent lifecycle metadata",
            )),
        };
    }
    let mut choice_ids = BTreeSet::new();
    if choices
        .iter()
        .any(|choice| choice.id.trim().is_empty() || !choice_ids.insert(choice.id.as_str()))
    {
        return Err(storage_corrupted(
            "stored choice interaction effect has invalid or duplicate choice IDs",
        ));
    }
    match (status, selected_choice_id, decided_at_epoch_seconds) {
        (Some("pending"), None, None) => Ok(Some(InteractionChoiceEffectStatus::Pending)),
        (Some("consumed"), Some(choice_id), Some(decided_at))
            if decided_at >= 0 && choice_ids.contains(choice_id) =>
        {
            Ok(Some(InteractionChoiceEffectStatus::Consumed))
        }
        (Some("expired"), None, Some(decided_at)) if decided_at >= 0 => {
            Ok(Some(InteractionChoiceEffectStatus::Expired))
        }
        (Some("pending" | "consumed" | "expired"), _, _) => Err(storage_corrupted(
            "choice interaction effect lifecycle metadata is inconsistent",
        )),
        (Some(other), _, _) => Err(storage_corrupted(format!(
            "stored interaction choice status `{other}` is invalid"
        ))),
        (None, _, _) => Err(storage_corrupted(
            "choice interaction effect is missing lifecycle metadata",
        )),
    }
}

pub(super) fn validate_effect_delivery_token(
    event_id: &str,
    sequence: u64,
    delivery_attempts: u64,
) -> CoreResult<()> {
    validate_nonempty_id("interaction effect event id", event_id)?;
    if sequence == 0 || delivery_attempts == 0 {
        return Err(CoreError::invalid(
            "interaction effect acknowledgement requires a claimed effect token",
        ));
    }
    Ok(())
}

pub(super) fn interaction_effect_id(event_id: &str, sequence: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"lorepia.interaction-effect.v1");
    hasher.update(
        u64::try_from(event_id.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    hasher.update(event_id.as_bytes());
    hasher.update(sequence.to_be_bytes());
    hex::encode(hasher.finalize())
}
