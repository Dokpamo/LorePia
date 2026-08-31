use lorepia_domain::{
    CapabilityKey, CapabilityValue, ConversationBranchId, ConversationId, ConversationMode,
    CoreError, CoreErrorCode, CoreResult, ModelRouteId, ModuleScope, SupportStatus, VariableMap,
};
use lorepia_storage::{PromptPresetBinding, built_in_prompt_presets};

use super::{
    ResolvedPromptRuntimePolicy, RuntimeTransformRevision, module_runtime::ResolvedModuleRuntime,
};
use crate::Core;

impl Core {
    pub(super) fn supported_capabilities_for_route(
        &self,
        route_id: &ModelRouteId,
    ) -> CoreResult<Vec<CapabilityKey>> {
        const KEYS: [CapabilityKey; 16] = [
            CapabilityKey::Streaming,
            CapabilityKey::Reasoning,
            CapabilityKey::PromptCaching,
            CapabilityKey::ToolCalling,
            CapabilityKey::ParallelToolCalling,
            CapabilityKey::StructuredOutput,
            CapabilityKey::JsonMode,
            CapabilityKey::ImageInput,
            CapabilityKey::AudioInput,
            CapabilityKey::AudioOutput,
            CapabilityKey::Logprobs,
            CapabilityKey::Seed,
            CapabilityKey::Batch,
            CapabilityKey::Background,
            CapabilityKey::ContextWindow,
            CapabilityKey::MaxOutputTokens,
        ];
        let mut supported = Vec::new();
        for key in KEYS {
            let Some(capability) = self.effective_capability(route_id, key)? else {
                continue;
            };
            if capability.has_conflict
                || capability.selected_is_stale
                || matches!(
                    capability.selected.status,
                    SupportStatus::Unsupported | SupportStatus::Unknown
                )
                || matches!(capability.selected.value, CapabilityValue::Boolean(false))
            {
                continue;
            }
            supported.push(key);
        }
        Ok(supported)
    }

    pub(super) fn runtime_selected_capabilities(&self) -> CoreResult<Vec<CapabilityKey>> {
        let settings = self.storage().load_settings()?;
        settings.selected_model_route_id.as_ref().map_or_else(
            || Ok(Vec::new()),
            |route_id| self.supported_capabilities_for_route(route_id),
        )
    }

    fn select_memory_prompt_binding(
        &self,
        scopes: &[(ModuleScope, Option<&str>)],
    ) -> CoreResult<Option<PromptPresetBinding>> {
        for &(scope, target_id) in scopes {
            if scope == ModuleScope::Persona && target_id.is_none() {
                continue;
            }
            let mut enabled = self
                .storage()
                .list_prompt_preset_bindings(scope, target_id)?
                .into_iter()
                .filter(|stored| stored.deleted_at.is_none() && stored.value.enabled)
                .collect::<Vec<_>>();
            enabled.sort_by(|left, right| {
                right
                    .value
                    .priority
                    .cmp(&left.value.priority)
                    .then_with(|| left.value.id.cmp(&right.value.id))
            });
            if enabled.len() > 1 && enabled[0].value.priority == enabled[1].value.priority {
                return Err(CoreError::invalid(
                    "multiple prompt bindings with equal priority apply to memory runtime",
                ));
            }
            if let Some(stored) = enabled.into_iter().next() {
                if !stored.value.memory_enabled {
                    return Err(CoreError::new(
                        CoreErrorCode::PermissionDenied,
                        "memory is disabled by the active prompt binding",
                        false,
                    ));
                }
                return Ok(Some(stored.value));
            }
        }
        Ok(None)
    }

    pub(super) fn resolve_runtime_prompt_policy(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ResolvedPromptRuntimePolicy> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let state = self.storage().get_conversation_state(conversation_id)?;
        let persona_target = self
            .storage()
            .get_conversation_persona_selection(conversation_id)?
            .map(|selection| selection.value.persona_id.as_str().to_owned());
        let scopes = [
            (ModuleScope::Branch, Some(branch_id.0.as_str())),
            (ModuleScope::Conversation, Some(conversation_id.0.as_str())),
            (
                ModuleScope::Character,
                Some(conversation.character_id.as_str()),
            ),
            (ModuleScope::Persona, persona_target.as_deref()),
            (ModuleScope::User, None),
            (ModuleScope::App, None),
        ];
        let selected_binding = self.select_memory_prompt_binding(&scopes)?;
        let preset_id = selected_binding.as_ref().map_or_else(
            || match state.selected_mode {
                ConversationMode::Chat => built_in_prompt_presets()[0].id.clone(),
                ConversationMode::Story => built_in_prompt_presets()[1].id.clone(),
            },
            |binding| binding.prompt_preset_id.clone(),
        );
        let stored_preset = self.storage().get_prompt_preset(&preset_id)?;
        let preset_revision_id = stored_preset.revision_id.clone().ok_or_else(|| {
            CoreError::internal("prompt preset is missing immutable revision identity")
        })?;
        if let Some(binding) = &selected_binding
            && let Some(pinned) = &binding.pinned_revision_id
            && pinned != &preset_revision_id
        {
            return Err(CoreError::invalid(
                "active prompt binding no longer matches its pinned revision",
            ));
        }

        let modules = self.resolve_runtime_modules(conversation_id, branch_id)?;
        self.validate_prompt_preset_module_dependencies(&preset_revision_id, &modules)?;
        let mut variables = stored_preset.value.default_values.clone();
        if let Some(binding) = &selected_binding {
            merge_variables(&mut variables, &binding.variable_overrides);
        }
        merge_variables(&mut variables, &modules.variables);
        let approved_import_source_ids = modules.approved_import_source_ids.clone();
        let exact_preset_transform_sets = self
            .storage()
            .get_prompt_preset_transform_set_revisions(&preset_revision_id)?;
        variables.validate().map_err(|error| {
            CoreError::invalid(format!("memory runtime variables are invalid: {error}"))
        })?;

        let mut transform_sets =
            Vec::with_capacity(exact_preset_transform_sets.len() + modules.transform_sets.len());
        let mut transform_revisions =
            Vec::with_capacity(exact_preset_transform_sets.len() + modules.transform_sets.len());
        for exact in exact_preset_transform_sets {
            transform_revisions.push(RuntimeTransformRevision {
                transform_set_id: exact.value.id.clone(),
                revision: exact.revision,
                revision_id: exact.revision_id,
                sha256: exact.sha256,
            });
            transform_sets.push(exact.value);
        }
        for stored in &modules.transform_sets {
            if transform_sets
                .iter()
                .any(|transform_set| transform_set.id == stored.value.id)
            {
                return Err(CoreError::invalid(
                    "prompt preset and approved module select the same transform set ambiguously",
                ));
            }
            transform_revisions.push(RuntimeTransformRevision {
                transform_set_id: stored.value.id.clone(),
                revision: stored.revision,
                revision_id: stored.revision_id.clone(),
                sha256: stored.sha256.clone(),
            });
            transform_sets.push(stored.value.clone());
        }
        transform_revisions
            .sort_by(|left, right| left.transform_set_id.cmp(&right.transform_set_id));
        transform_sets.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(ResolvedPromptRuntimePolicy {
            preset: stored_preset.value,
            preset_revision_id,
            module_plan_sha256: modules.plan_sha256,
            variables,
            transform_sets,
            transform_revisions,
            approved_import_source_ids,
        })
    }

    fn validate_prompt_preset_module_dependencies(
        &self,
        prompt_preset_revision_id: &str,
        modules: &ResolvedModuleRuntime,
    ) -> CoreResult<()> {
        let dependencies = self
            .storage()
            .get_prompt_preset_module_dependencies(prompt_preset_revision_id)?;
        for dependency in dependencies {
            let identity = (
                dependency.module_id.as_str().to_owned(),
                dependency.module_revision_id.as_str().to_owned(),
                dependency.source_sha256.as_str().to_owned(),
            );
            if !modules.approved_module_sources.contains(&identity) {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    format!(
                        "prompt preset module dependency {} is not present at its exact approved revision",
                        dependency.module_id.as_str()
                    ),
                    false,
                ));
            }
        }
        Ok(())
    }
}

fn merge_variables(target: &mut VariableMap, source: &VariableMap) {
    for binding in &source.values {
        target.insert(binding.variable.clone(), binding.value.clone());
    }
}
