use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use lorepia_domain::{
    ConversationBranch, ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult,
    InteractionState, MessageId, Sha256Digest,
};
use lorepia_orchestration::{InteractionLimits, InteractionOutcome};
use lorepia_storage::{
    InteractionEvaluationAssetDiagnostic, InteractionEvaluationKnowledgeRevision,
    InteractionEvaluationLimits, InteractionEvaluationSeal, InteractionKnowledgeBinding,
    InteractionStateKey, interaction_evaluation_seal_sha256, interaction_policy_sha256,
};

use crate::Core;

use super::policy::{
    ResolvedInteractionPolicy, executable_interaction_policy_sha256, interaction_policy_snapshot,
    runtime_interaction_template_values,
};

impl Core {
    pub(crate) fn validate_runtime_branch_identity(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ConversationBranch> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let branch = self.storage().get_conversation_branch(branch_id)?;
        if branch.conversation_id != *conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        // Reading the conversation above is intentional: it verifies that the
        // caller cannot pair an otherwise valid branch with a deleted or
        // unrelated conversation identity.
        let _ = conversation;
        Ok(branch)
    }

    pub(in crate::orchestration_runtime) fn validate_runtime_branch_head(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
    ) -> CoreResult<ConversationBranch> {
        let branch = self.validate_runtime_branch_identity(conversation_id, branch_id)?;
        if branch.head_message_id.as_ref() != expected_head {
            return Err(CoreError::invalid(
                "conversation branch head changed before orchestration runtime review",
            ));
        }
        Ok(branch)
    }
}

pub(in crate::orchestration_runtime) fn initial_interaction_state(
    policy: &ResolvedInteractionPolicy,
) -> InteractionState {
    InteractionState {
        variables: policy.variables.clone(),
        manually_active_knowledge: Vec::new(),
        proposals: Vec::new(),
        revision: 0,
    }
}

pub(in crate::orchestration_runtime) fn interaction_evaluation_limits(
    limits: InteractionLimits,
) -> InteractionEvaluationLimits {
    InteractionEvaluationLimits {
        max_rule_sets: limits.max_rule_sets,
        max_rules: limits.max_rules,
        max_actions_per_event: limits.max_actions_per_event,
        max_actions_per_rule: limits.max_actions_per_rule,
        max_condition_depth: limits.max_condition_depth,
        max_condition_nodes: limits.max_condition_nodes,
        max_template_depth: limits.max_template_depth,
        max_template_parts: limits.max_template_parts,
        max_variables: limits.max_variables,
        max_proposals: limits.max_proposals,
        max_pending_proposals: limits.max_pending_proposals,
        max_effects: limits.max_effects,
        max_choices: limits.max_choices,
        max_dice_count: limits.max_dice_count,
        max_dice_sides: limits.max_dice_sides,
        max_text_chars: limits.max_text_chars,
        max_identifier_bytes: limits.max_identifier_bytes,
    }
}

pub(super) fn interaction_limits_from_evaluation(
    limits: &InteractionEvaluationLimits,
) -> InteractionLimits {
    InteractionLimits {
        max_rule_sets: limits.max_rule_sets,
        max_rules: limits.max_rules,
        max_actions_per_event: limits.max_actions_per_event,
        max_actions_per_rule: limits.max_actions_per_rule,
        max_condition_depth: limits.max_condition_depth,
        max_condition_nodes: limits.max_condition_nodes,
        max_template_depth: limits.max_template_depth,
        max_template_parts: limits.max_template_parts,
        max_variables: limits.max_variables,
        max_proposals: limits.max_proposals,
        max_pending_proposals: limits.max_pending_proposals,
        max_effects: limits.max_effects,
        max_choices: limits.max_choices,
        max_dice_count: limits.max_dice_count,
        max_dice_sides: limits.max_dice_sides,
        max_text_chars: limits.max_text_chars,
        max_identifier_bytes: limits.max_identifier_bytes,
    }
}

pub(super) fn interaction_evaluation_seal(
    policy: &ResolvedInteractionPolicy,
    event_at: chrono::DateTime<Utc>,
) -> CoreResult<InteractionEvaluationSeal> {
    let policy_snapshot = interaction_policy_snapshot(policy);
    let policy_sha256 = Sha256Digest::parse(interaction_policy_sha256(&policy_snapshot)?)
        .map_err(CoreError::invalid)?;
    let knowledge_revisions = policy
        .knowledge_revisions
        .iter()
        .map(
            |(entry_id, book_revision_id)| InteractionEvaluationKnowledgeRevision {
                entry_id: entry_id.clone(),
                book_revision_id: book_revision_id.clone(),
            },
        )
        .collect();
    let asset_action_diagnostics = policy
        .asset_action_diagnostics
        .iter()
        .map(
            |((rule_id, action_ordinal), diagnostic)| InteractionEvaluationAssetDiagnostic {
                rule_id: rule_id.clone(),
                action_ordinal: *action_ordinal,
                diagnostic: diagnostic.clone(),
            },
        )
        .collect();
    Ok(InteractionEvaluationSeal {
        schema_version: 1,
        engine_contract_version: 1,
        policy_sha256,
        executable_rule_sets_sha256: executable_interaction_policy_sha256(policy)?,
        knowledge_revisions,
        asset_action_diagnostics,
        approved_import_source_ids: policy.approved_import_source_ids.iter().cloned().collect(),
        policy_variables: policy.variables.clone(),
        supported_capabilities: policy.supported_capabilities.clone(),
        template_values: runtime_interaction_template_values(policy, event_at),
        event_epoch_seconds: event_at.timestamp(),
        limits: interaction_evaluation_limits(InteractionLimits::default()),
        seed_contract_version: 1,
    })
}

pub(in crate::orchestration_runtime) fn validate_interaction_evaluation_seal(
    policy: &ResolvedInteractionPolicy,
    event_at: chrono::DateTime<Utc>,
    sealed: &InteractionEvaluationSeal,
) -> CoreResult<()> {
    interaction_evaluation_seal_sha256(sealed)?;
    let expected = interaction_evaluation_seal(policy, event_at)?;
    if &expected != sealed {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "sealed interaction evaluation context cannot be reconstructed exactly",
            false,
        ));
    }
    Ok(())
}

pub(crate) fn interaction_state_key(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> CoreResult<InteractionStateKey> {
    lorepia_storage::interaction_state_key_for_branch(conversation_id, branch_id)
}

pub(in crate::orchestration_runtime) fn interaction_knowledge_bindings(
    state: &InteractionState,
    policy: &ResolvedInteractionPolicy,
    existing: &[InteractionKnowledgeBinding],
) -> CoreResult<Vec<InteractionKnowledgeBinding>> {
    let existing = existing
        .iter()
        .map(|binding| (binding.entry_id.clone(), binding))
        .collect::<BTreeMap<_, _>>();
    let mut bindings = state
        .manually_active_knowledge
        .iter()
        .map(|entry_id| {
            if let Some(binding) = existing.get(entry_id) {
                let current_revision = policy.knowledge_revisions.get(entry_id).ok_or_else(|| {
                    CoreError::invalid(format!(
                        "stale interaction knowledge entry {} is absent from the approved module plan",
                        entry_id.as_str()
                    ))
                })?;
                if binding.book_revision_id != *current_revision {
                    return Err(CoreError::invalid(format!(
                        "stale interaction knowledge entry {} is bound to a different book revision",
                        entry_id.as_str()
                    )));
                }
                return Ok((*binding).clone());
            }
            let book_revision_id = policy.knowledge_revisions.get(entry_id).ok_or_else(|| {
                CoreError::invalid(format!(
                    "interaction knowledge entry {} has no approved exact book revision",
                    entry_id.as_str()
                ))
            })?;
            Ok(InteractionKnowledgeBinding {
                book_revision_id: book_revision_id.clone(),
                entry_id: entry_id.clone(),
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    bindings.sort();
    Ok(bindings)
}

pub(in crate::orchestration_runtime) fn reconcile_interaction_knowledge_state(
    mut state: InteractionState,
    policy: &ResolvedInteractionPolicy,
    existing: &[InteractionKnowledgeBinding],
) -> CoreResult<(InteractionState, Vec<InteractionKnowledgeBinding>)> {
    let state_entries = state
        .manually_active_knowledge
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut existing_by_entry = BTreeMap::new();
    for binding in existing {
        if existing_by_entry
            .insert(binding.entry_id.clone(), binding)
            .is_some()
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "durable interaction knowledge contains a duplicate entry binding",
                false,
            ));
        }
    }
    if state_entries.len() != state.manually_active_knowledge.len()
        || state_entries.len() != existing_by_entry.len()
        || state_entries
            .iter()
            .any(|entry_id| !existing_by_entry.contains_key(entry_id))
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "durable interaction knowledge does not match the active state",
            false,
        ));
    }

    let mut bindings = existing
        .iter()
        .filter(|binding| {
            policy.knowledge_revisions.get(&binding.entry_id) == Some(&binding.book_revision_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    bindings.sort();
    state.manually_active_knowledge = bindings
        .iter()
        .map(|binding| binding.entry_id.clone())
        .collect();
    Ok((state, bindings))
}

pub(super) fn normalize_interaction_event_revision(
    previous: &InteractionState,
    outcome: &mut InteractionOutcome,
) -> CoreResult<()> {
    if !outcome.state_changed {
        outcome.state.revision = previous
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("interaction state revision overflowed"))?;
        outcome.state_changed = true;
    }
    Ok(())
}
