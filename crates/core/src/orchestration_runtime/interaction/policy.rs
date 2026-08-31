use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use lorepia_domain::{
    AssetId, CapabilityKey, ConversationBranchId, ConversationId, CoreError, CoreErrorCode,
    CoreResult, InteractionAction, InteractionRuleSet, KnowledgeEntryId, Sha256Digest, UiRegion,
    VariableMap, VersionedJson,
};
use lorepia_orchestration::{AppliedModuleRuntimePlan, InteractionTemplateValues};
use lorepia_storage::{
    InteractionEvaluationAssetDiagnostic, InteractionEvaluationSeal,
    InteractionEvaluationTemplateValues, InteractionPolicyRuleSetRevision,
    InteractionPolicySnapshot, interaction_policy_sha256,
};
use serde::Serialize;

use super::{review::InteractionRuleSetRevision, state::validate_interaction_evaluation_seal};
use crate::{
    Core,
    orchestration_runtime::{
        module_runtime::{ApprovedRuntimeAsset, ResolvedModuleRuntime, module_plan_error},
        versioned_digest,
    },
};

#[derive(Debug, Clone)]
pub(in crate::orchestration_runtime) struct ResolvedInteractionPolicy {
    pub(in crate::orchestration_runtime) module_plan_sha256: Option<String>,
    pub(in crate::orchestration_runtime) rule_sets: Vec<InteractionRuleSet>,
    pub(in crate::orchestration_runtime) rule_set_revisions: Vec<InteractionRuleSetRevision>,
    pub(in crate::orchestration_runtime) knowledge_revisions: BTreeMap<KnowledgeEntryId, String>,
    pub(in crate::orchestration_runtime) asset_action_diagnostics:
        BTreeMap<(String, u32), VersionedJson>,
    pub(in crate::orchestration_runtime) approved_import_source_ids: BTreeSet<String>,
    pub(in crate::orchestration_runtime) variables: VariableMap,
    pub(in crate::orchestration_runtime) supported_capabilities: Vec<CapabilityKey>,
    pub(in crate::orchestration_runtime) character_name: String,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeInteractionKnowledgeRevision<'a> {
    entry_id: &'a KnowledgeEntryId,
    book_revision_id: &'a str,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeInteractionAssetDiagnostic<'a> {
    rule_id: &'a str,
    action_ordinal: u32,
    diagnostic: &'a VersionedJson,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeExecutableInteractionPolicy<'a> {
    schema_version: u32,
    rule_sets: &'a [InteractionRuleSet],
    rule_set_revisions: &'a [InteractionRuleSetRevision],
    knowledge_revisions: Vec<RuntimeInteractionKnowledgeRevision<'a>>,
    asset_action_diagnostics: Vec<RuntimeInteractionAssetDiagnostic<'a>>,
    approved_import_source_ids: &'a BTreeSet<String>,
    variables: &'a VariableMap,
    supported_capabilities: &'a [CapabilityKey],
    character_name: &'a str,
}

impl Core {
    pub(in crate::orchestration_runtime) fn resolve_interaction_policy(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ResolvedInteractionPolicy> {
        let modules = self.resolve_runtime_modules(conversation_id, branch_id)?;
        self.resolve_interaction_policy_from_modules(conversation_id, modules)
    }

    pub(in crate::orchestration_runtime) fn resolve_sealed_interaction_policy(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        sealed: &InteractionPolicySnapshot,
        evaluation_seal: &InteractionEvaluationSeal,
    ) -> CoreResult<ResolvedInteractionPolicy> {
        if interaction_policy_sha256(sealed)? != evaluation_seal.policy_sha256.as_str() {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "sealed derived interaction policy hash is inconsistent",
                false,
            ));
        }
        if let Ok(modules) = self.resolve_runtime_modules(conversation_id, branch_id)
            && let Ok(current) = Self::resolve_interaction_policy_from_modules_with_evaluation_seal(
                &modules,
                evaluation_seal,
            )
            && interaction_policy_snapshot(&current) == *sealed
        {
            validate_interaction_evaluation_seal(
                &current,
                chrono::DateTime::from_timestamp(evaluation_seal.event_epoch_seconds, 0)
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "sealed derived interaction timestamp is invalid",
                            false,
                        )
                    })?,
                evaluation_seal,
            )?;
            return Ok(current);
        }
        let modules = if let Some(applied_plan_sha256) = sealed.module_plan_sha256.as_deref() {
            let applied_plan_sha256 =
                Sha256Digest::parse(applied_plan_sha256.to_owned()).map_err(CoreError::invalid)?;
            let applied = self
                .storage()
                .get_historical_applied_module_runtime_plan(&applied_plan_sha256)?;
            if applied.review.context.conversation_id.as_deref() != Some(conversation_id.0.as_str())
                || applied.review.context.branch_id.as_deref() != Some(branch_id.0.as_str())
            {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "sealed derived interaction module plan belongs to another branch",
                    false,
                ));
            }
            self.materialize_resolved_module_runtime(&applied)?
        } else {
            ResolvedModuleRuntime::default()
        };
        let resolved = Self::resolve_interaction_policy_from_modules_with_evaluation_seal(
            &modules,
            evaluation_seal,
        )?;
        if interaction_policy_snapshot(&resolved) != *sealed {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "sealed derived interaction policy cannot be reconstructed exactly",
                false,
            ));
        }
        validate_interaction_evaluation_seal(
            &resolved,
            chrono::DateTime::from_timestamp(evaluation_seal.event_epoch_seconds, 0).ok_or_else(
                || {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "sealed derived interaction timestamp is invalid",
                        false,
                    )
                },
            )?,
            evaluation_seal,
        )?;
        Ok(resolved)
    }

    pub(super) fn resolve_interaction_policy_for_proposed_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        applied_plan: Option<&AppliedModuleRuntimePlan>,
    ) -> CoreResult<ResolvedInteractionPolicy> {
        let modules = if let Some(applied_plan) = applied_plan {
            let expected_context =
                self.content_module_context_for_proposed_branch(conversation_id, branch_id)?;
            applied_plan.verify().map_err(module_plan_error)?;
            if applied_plan.review.context != expected_context {
                return Err(CoreError::invalid(
                    "applied module plan does not match the proposed interaction branch",
                ));
            }
            self.materialize_resolved_module_runtime(applied_plan)?
        } else {
            ResolvedModuleRuntime::default()
        };
        self.resolve_interaction_policy_from_modules(conversation_id, modules)
    }

    fn resolve_interaction_policy_from_modules(
        &self,
        conversation_id: &ConversationId,
        modules: ResolvedModuleRuntime,
    ) -> CoreResult<ResolvedInteractionPolicy> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let character = self.storage().get_character(&conversation.character_id)?;
        let variables = modules.variables.clone();
        let mut rule_sets = Vec::with_capacity(modules.interaction_rule_sets.len());
        let mut rule_set_revisions = Vec::with_capacity(modules.interaction_rule_sets.len());
        for stored in &modules.interaction_rule_sets {
            rule_set_revisions.push(InteractionRuleSetRevision {
                rule_set_id: stored.value.id.clone(),
                revision: stored.revision,
                revision_id: stored.revision_id.clone(),
                sha256: stored.sha256.clone(),
            });
            rule_sets.push(stored.value.clone());
        }
        let asset_action_diagnostics =
            self.validate_interaction_asset_actions(&mut rule_sets, &modules.assets);
        let mut knowledge_revisions = BTreeMap::new();
        for stored in &modules.knowledge_books {
            for entry in &stored.value.entries {
                if knowledge_revisions
                    .insert(entry.id.clone(), stored.revision_id.clone())
                    .is_some()
                {
                    return Err(CoreError::invalid(
                        "active interaction knowledge entry IDs are ambiguous",
                    ));
                }
            }
        }

        Ok(ResolvedInteractionPolicy {
            module_plan_sha256: modules.plan_sha256,
            rule_sets,
            rule_set_revisions,
            knowledge_revisions,
            asset_action_diagnostics,
            approved_import_source_ids: modules.approved_import_source_ids,
            variables,
            supported_capabilities: self.runtime_selected_capabilities()?,
            character_name: character.name,
        })
    }

    pub(in crate::orchestration_runtime) fn resolve_interaction_policy_from_modules_with_evaluation_seal(
        modules: &ResolvedModuleRuntime,
        sealed: &InteractionEvaluationSeal,
    ) -> CoreResult<ResolvedInteractionPolicy> {
        if modules.variables != sealed.policy_variables
            || modules
                .approved_import_source_ids
                .iter()
                .ne(sealed.approved_import_source_ids.iter())
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "sealed interaction module variables or import approvals changed",
                false,
            ));
        }
        let mut rule_sets = Vec::with_capacity(modules.interaction_rule_sets.len());
        let mut rule_set_revisions = Vec::with_capacity(modules.interaction_rule_sets.len());
        for stored in &modules.interaction_rule_sets {
            rule_set_revisions.push(InteractionRuleSetRevision {
                rule_set_id: stored.value.id.clone(),
                revision: stored.revision,
                revision_id: stored.revision_id.clone(),
                sha256: stored.sha256.clone(),
            });
            rule_sets.push(stored.value.clone());
        }
        let mut knowledge_revisions = BTreeMap::new();
        for stored in &modules.knowledge_books {
            for entry in &stored.value.entries {
                if knowledge_revisions
                    .insert(entry.id.clone(), stored.revision_id.clone())
                    .is_some()
                {
                    return Err(CoreError::invalid(
                        "sealed interaction knowledge entry IDs are ambiguous",
                    ));
                }
            }
        }
        let sealed_knowledge = sealed
            .knowledge_revisions
            .iter()
            .map(|revision| (revision.entry_id.clone(), revision.book_revision_id.clone()))
            .collect::<BTreeMap<_, _>>();
        if knowledge_revisions != sealed_knowledge {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "sealed interaction knowledge revisions changed",
                false,
            ));
        }
        let asset_action_diagnostics = apply_sealed_interaction_asset_diagnostics(
            &mut rule_sets,
            &sealed.asset_action_diagnostics,
        )?;
        let character_name = sealed
            .template_values
            .character_name
            .clone()
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "sealed interaction character template value is missing",
                    false,
                )
            })?;
        let policy = ResolvedInteractionPolicy {
            module_plan_sha256: modules.plan_sha256.clone(),
            rule_sets,
            rule_set_revisions,
            knowledge_revisions,
            asset_action_diagnostics,
            approved_import_source_ids: modules.approved_import_source_ids.clone(),
            variables: modules.variables.clone(),
            supported_capabilities: sealed.supported_capabilities.clone(),
            character_name,
        };
        if executable_interaction_policy_sha256(&policy)? != sealed.executable_rule_sets_sha256 {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "sealed executable interaction policy changed",
                false,
            ));
        }
        Ok(policy)
    }

    fn validate_interaction_asset_actions(
        &self,
        rule_sets: &mut [InteractionRuleSet],
        assets: &BTreeMap<AssetId, ApprovedRuntimeAsset>,
    ) -> BTreeMap<(String, u32), VersionedJson> {
        let mut diagnostics = BTreeMap::new();
        for rule_set in rule_sets {
            for rule in &mut rule_set.rules {
                for (ordinal, action) in rule.actions.iter().enumerate() {
                    let validation = match action {
                        InteractionAction::ShowAsset { asset_id, region } => assets
                            .get(asset_id)
                            .ok_or_else(|| {
                                CoreError::new(
                                    CoreErrorCode::PermissionDenied,
                                    "interaction asset is not selected by the approved module plan",
                                    false,
                                )
                            })
                            .and_then(|asset| {
                                self.validate_approved_runtime_asset(asset, Some(*region))
                            }),
                        InteractionAction::PlayAudio { asset_id } => assets
                            .get(asset_id)
                            .ok_or_else(|| {
                                CoreError::new(
                                    CoreErrorCode::PermissionDenied,
                                    "interaction audio is not selected by the approved module plan",
                                    false,
                                )
                            })
                            .and_then(|asset| self.validate_approved_runtime_asset(asset, None)),
                        _ => continue,
                    };
                    if let Err(error) = validation {
                        // Disable the whole rule before evaluation. This is
                        // deliberately fail-closed: a sibling mutation must
                        // not commit after its asset side effect was rejected.
                        rule.enabled = false;
                        let ordinal = u32::try_from(ordinal).unwrap_or(u32::MAX);
                        diagnostics.insert(
                            (rule.id.as_str().to_owned(), ordinal),
                            VersionedJson {
                                schema_version: 1,
                                value: serde_json::json!({
                                    "diagnostic": "approved_asset_validation_failed",
                                    "error_code": format!("{:?}", error.code),
                                    "message": error.message,
                                }),
                            },
                        );
                    }
                }
            }
        }
        diagnostics
    }

    fn validate_approved_runtime_asset(
        &self,
        asset: &ApprovedRuntimeAsset,
        region: Option<UiRegion>,
    ) -> CoreResult<()> {
        let expected = crate::AssetDeliveryDescriptor::try_from(asset.descriptor.clone())?;
        let actual = self.resolve_asset_delivery_by_sha256(&asset.descriptor.sha256)?;
        if actual != expected {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "approved module asset differs from its verified CAS descriptor",
                false,
            ));
        }
        let compatible = match region {
            None | Some(UiRegion::Audio) => actual.kind == crate::AssetDeliveryKind::Audio,
            Some(
                UiRegion::Message
                | UiRegion::Background
                | UiRegion::CharacterPortrait
                | UiRegion::StatusPanel,
            ) => matches!(
                actual.kind,
                crate::AssetDeliveryKind::Image | crate::AssetDeliveryKind::Video
            ),
        };
        if !compatible {
            return Err(CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "approved module asset is incompatible with the requested renderer region",
                false,
            ));
        }
        if asset.module_id.is_empty()
            || asset.module_revision_id.is_empty()
            || asset.component_sha256.is_empty()
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "approved module asset is missing plan-bound evidence",
                false,
            ));
        }
        Ok(())
    }
}

fn apply_sealed_interaction_asset_diagnostics(
    rule_sets: &mut [InteractionRuleSet],
    diagnostics: &[InteractionEvaluationAssetDiagnostic],
) -> CoreResult<BTreeMap<(String, u32), VersionedJson>> {
    let mut sealed = BTreeMap::new();
    for diagnostic in diagnostics {
        let key = (diagnostic.rule_id.clone(), diagnostic.action_ordinal);
        if sealed.insert(key, diagnostic.diagnostic.clone()).is_some() {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "sealed interaction asset diagnostic is duplicated",
                false,
            ));
        }
        let mut matched = false;
        for rule in rule_sets.iter_mut().flat_map(|set| set.rules.iter_mut()) {
            if rule.id.as_str() != diagnostic.rule_id {
                continue;
            }
            if matched {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "sealed interaction asset diagnostic rule is ambiguous",
                    false,
                ));
            }
            let action_index = usize::try_from(diagnostic.action_ordinal)
                .map_err(|_| CoreError::invalid("sealed asset action ordinal overflowed"))?;
            let action = rule.actions.get(action_index).ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "sealed interaction asset diagnostic action is missing",
                    false,
                )
            })?;
            if !matches!(
                action,
                InteractionAction::ShowAsset { .. } | InteractionAction::PlayAudio { .. }
            ) {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "sealed interaction asset diagnostic targets a non-asset action",
                    false,
                ));
            }
            rule.enabled = false;
            matched = true;
        }
        if !matched {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "sealed interaction asset diagnostic rule is missing",
                false,
            ));
        }
    }
    Ok(sealed)
}

pub(in crate::orchestration_runtime) fn interaction_policy_snapshot(
    policy: &ResolvedInteractionPolicy,
) -> InteractionPolicySnapshot {
    InteractionPolicySnapshot {
        module_plan_sha256: policy.module_plan_sha256.clone(),
        rule_sets: policy
            .rule_set_revisions
            .iter()
            .map(|revision| InteractionPolicyRuleSetRevision {
                rule_set_id: revision.rule_set_id.clone(),
                revision_id: revision.revision_id.clone(),
                sha256: revision.sha256.clone(),
            })
            .collect(),
    }
}

pub(super) fn runtime_interaction_template_values(
    policy: &ResolvedInteractionPolicy,
    event_at: chrono::DateTime<Utc>,
) -> InteractionEvaluationTemplateValues {
    InteractionEvaluationTemplateValues {
        character_name: Some(policy.character_name.clone()),
        user_name: Some("User".to_owned()),
        persona_name: None,
        persona_description: None,
        current_date: Some(event_at.format("%Y-%m-%d").to_string()),
        current_time: Some(event_at.format("%H:%M:%S%:z").to_string()),
    }
}

pub(super) fn interaction_engine_template_values(
    sealed: &InteractionEvaluationTemplateValues,
) -> InteractionTemplateValues {
    InteractionTemplateValues {
        character_name: sealed.character_name.clone(),
        user_name: sealed.user_name.clone(),
        persona_name: sealed.persona_name.clone(),
        persona_description: sealed.persona_description.clone(),
        current_date: sealed.current_date.clone(),
        current_time: sealed.current_time.clone(),
    }
}

pub(super) fn executable_interaction_policy_sha256(
    policy: &ResolvedInteractionPolicy,
) -> CoreResult<Sha256Digest> {
    let knowledge_revisions = policy
        .knowledge_revisions
        .iter()
        .map(
            |(entry_id, book_revision_id)| RuntimeInteractionKnowledgeRevision {
                entry_id,
                book_revision_id,
            },
        )
        .collect();
    let asset_action_diagnostics = policy
        .asset_action_diagnostics
        .iter()
        .map(
            |((rule_id, action_ordinal), diagnostic)| RuntimeInteractionAssetDiagnostic {
                rule_id,
                action_ordinal: *action_ordinal,
                diagnostic,
            },
        )
        .collect();
    Sha256Digest::parse(versioned_digest(&RuntimeExecutableInteractionPolicy {
        schema_version: 1,
        rule_sets: &policy.rule_sets,
        rule_set_revisions: &policy.rule_set_revisions,
        knowledge_revisions,
        asset_action_diagnostics,
        approved_import_source_ids: &policy.approved_import_source_ids,
        variables: &policy.variables,
        supported_capabilities: &policy.supported_capabilities,
        character_name: &policy.character_name,
    })?)
    .map_err(CoreError::invalid)
}
