use std::collections::BTreeSet;

use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, InteractionAction,
    InteractionEffect, InteractionEvent,
};
use rusqlite::{Connection, OptionalExtension, params};

use super::super::types::{
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionPolicySnapshot,
    InteractionStateKey, MAX_EFFECTS_PER_EVENT, MAX_EVENT_JSON_BYTES, interaction_action_sha256,
};
use super::super::{decode_json, i64_from_u64, storage_db_error};
use super::InteractionDerivedEventWrite;

pub(in crate::interaction_repository) fn require_no_pending_derived_predecessor(
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

pub(in crate::interaction_repository) fn require_no_pending_derived_predecessor_through(
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
pub(in crate::interaction_repository) fn validate_derived_event_writes(
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
