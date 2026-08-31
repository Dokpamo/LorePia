use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use lorepia_domain::{
    ControlKind, ControlSpec, ConversationBranchId, ConversationId, CoreError, CoreResult,
    GenerationPresetId, GenerationReasoningEffort, GenerationTarget, KnowledgeEntryId, ModuleScope,
    PromptPreset, PromptPresetId, TemplateSlot, VariableId, VariableMap, VariableRef,
    VariableScope, VariableType, VariableValue,
};
use lorepia_storage::{PromptPresetBinding, PromptResponseLength};
use uuid::Uuid;

use crate::{
    Core, Revisioned,
    revision::{project_revision, project_revisions},
};

use super::{
    GenerationPlanInput, PromptModuleOverlay, exact_prompt_manual_knowledge,
    prompt_module_knowledge_revisions,
};

/// JSON-safe creator-control value. Core converts this high-level value only
/// through the selected preset's declared `ControlSpec -> VariableRef` binding;
/// callers never submit arbitrary variable references for room settings.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum CreatorControlValue {
    Bool(bool),
    Integer(i64),
    Decimal(f64),
    Text(String),
    StringList(Vec<String>),
}

/// Full desired room-scoped orchestration settings used by the CAS save.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOrchestrationConfigPatch {
    pub prompt_preset_id: Option<PromptPresetId>,
    pub generation_preset_id: Option<GenerationPresetId>,
    #[serde(default)]
    pub creator_values: BTreeMap<String, CreatorControlValue>,
    pub response_length: PromptResponseLength,
    pub creativity: u8,
    pub reasoning_effort: Option<GenerationReasoningEffort>,
    pub memory_enabled: bool,
    pub knowledge_enabled: bool,
    #[serde(default)]
    pub user_name_override: Option<String>,
    #[serde(default)]
    pub author_note: Option<String>,
    #[serde(default)]
    pub group_context: Option<String>,
    #[serde(default)]
    pub template_slots: Vec<TemplateSlot>,
}

/// Effective room settings. `binding_revision` is present only when this exact
/// branch owns a binding; inherited conversation/character/user/app settings
/// remain visible but save as a new branch binding with expected `None`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOrchestrationConfig {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub prompt_preset_id: PromptPresetId,
    pub generation_preset_id: Option<GenerationPresetId>,
    /// Credential-free target that generation must use for this exact room.
    ///
    /// A prompt binding/default generation preset wins over the installation
    /// default. The route is read from the stored preset in Core so renderers
    /// never reproduce route/preset resolution rules.
    pub generation_target: Option<GenerationTarget>,
    pub creator_values: BTreeMap<String, CreatorControlValue>,
    /// Exact non-sensitive variable overrides represented by the effective
    /// binding. The renderer may review this value but cannot author arbitrary
    /// variable references through the room quick-settings API.
    pub variable_overrides: VariableMap,
    pub response_length: PromptResponseLength,
    pub creativity: u8,
    pub reasoning_effort: Option<GenerationReasoningEffort>,
    pub memory_enabled: bool,
    pub knowledge_enabled: bool,
    pub user_name_override: Option<String>,
    pub author_note: Option<String>,
    pub group_context: Option<String>,
    pub template_slots: Vec<TemplateSlot>,
    pub binding_revision: Option<u64>,
}

pub(super) struct PromptQuickSettings {
    pub(super) temperature: Option<f64>,
    pub(super) max_output_tokens: Option<u32>,
    pub(super) response_length: PromptResponseLength,
    pub(super) creativity: u8,
    pub(super) reasoning_effort: Option<GenerationReasoningEffort>,
    pub(super) memory_enabled: bool,
    pub(super) knowledge_enabled: bool,
    pub(super) warnings: Vec<String>,
}

pub(super) struct PromptVariableState {
    pub(super) variables: VariableMap,
    pub(super) manually_active_knowledge: BTreeSet<KnowledgeEntryId>,
}

impl Core {
    pub(super) fn resolve_prompt_quick_settings(
        &self,
        binding: Option<&PromptPresetBinding>,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<PromptQuickSettings> {
        if let (Some(binding), Some(target)) = (binding, input.generation_target)
            && binding
                .generation_preset_override_id
                .as_ref()
                .is_some_and(|id| id != &target.generation_preset_id)
        {
            return Err(CoreError::invalid(
                "prompt binding generation override does not match the selected target",
            ));
        }
        if let Some(authority) = input.prompt_selection_authority {
            let quick = &authority.quick_settings;
            let mut warnings = Vec::new();
            if binding.is_some()
                && input.temperature.is_none()
                && quick.resolved_temperature.is_none()
                && !quick.supports_temperature
            {
                warnings.push(
                    "creativity quick setting was ignored because the selected route does not expose temperature"
                        .to_owned(),
                );
            }
            return Ok(PromptQuickSettings {
                temperature: quick.resolved_temperature,
                max_output_tokens: quick.resolved_max_output_tokens,
                response_length: quick.response_length,
                creativity: quick.creativity,
                reasoning_effort: quick.reasoning_effort,
                memory_enabled: quick.memory_enabled,
                knowledge_enabled: quick.knowledge_enabled,
                warnings,
            });
        }
        let mut settings = PromptQuickSettings {
            temperature: input.temperature,
            max_output_tokens: input.max_output_tokens,
            response_length: PromptResponseLength::Balanced,
            creativity: 50,
            reasoning_effort: None,
            memory_enabled: true,
            knowledge_enabled: true,
            warnings: Vec::new(),
        };
        let Some(binding) = binding else {
            return Ok(settings);
        };
        settings.response_length = binding.response_length;
        settings.creativity = binding.creativity;
        settings.reasoning_effort = binding.reasoning_effort;
        settings.memory_enabled = binding.memory_enabled;
        settings.knowledge_enabled = binding.knowledge_enabled;
        if settings.temperature.is_none() {
            let supports_temperature = if let Some(contract) = input.prompt_wire_contract {
                contract.supports_temperature
            } else {
                input.generation_target.map_or(Ok(false), |target| {
                    crate::app::prompt_route_supports_temperature(self, target)
                })?
            };
            if supports_temperature {
                settings.temperature = Some(prompt_creativity_temperature(binding.creativity));
            } else {
                settings.warnings.push(
                    "creativity quick setting was ignored because the selected route does not expose temperature"
                        .to_owned(),
                );
            }
        }
        if settings.max_output_tokens.is_none() {
            settings.max_output_tokens = Some(match binding.response_length {
                PromptResponseLength::Short => 512,
                PromptResponseLength::Balanced => 2_048,
                PromptResponseLength::Long => 4_096,
            });
        }
        Ok(settings)
    }

    pub(super) fn resolve_prompt_variable_state(
        &self,
        preset: &PromptPreset,
        binding: Option<&PromptPresetBinding>,
        module_overlay: &PromptModuleOverlay,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<PromptVariableState> {
        let mut variables = self.character_runtime_initial_variables(input)?;
        merge_variable_map(&mut variables, &preset.default_values);
        if let Some(binding) = binding {
            merge_variable_map(&mut variables, &binding.variable_overrides);
        }
        merge_variable_map(&mut variables, &module_overlay.variables);
        let state_branch = input.interaction_state_branch_id.unwrap_or(input.branch_id);
        let current_module_knowledge =
            prompt_module_knowledge_revisions(&module_overlay.knowledge_books)?;
        let manually_active_knowledge = if let Some(snapshot) = input.interaction_state_override {
            if snapshot.key.conversation_id != *input.conversation_id
                || snapshot.key.branch_id != *state_branch
            {
                return Err(CoreError::invalid(
                    "historical interaction state does not match the prompt lineage",
                ));
            }
            merge_variable_map(&mut variables, &snapshot.state.variables);
            exact_prompt_manual_knowledge(
                &snapshot.state.manually_active_knowledge,
                &snapshot.knowledge,
                &current_module_knowledge,
            )?
        } else {
            match self
                .storage()
                .get_interaction_state_snapshot(input.conversation_id, state_branch)
            {
                Ok(snapshot) => {
                    merge_variable_map(&mut variables, &snapshot.state.variables);
                    exact_prompt_manual_knowledge(
                        &snapshot.state.manually_active_knowledge,
                        &snapshot.knowledge,
                        &current_module_knowledge,
                    )?
                }
                Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => {
                    BTreeSet::new()
                }
                Err(error) => return Err(error),
            }
        };
        merge_variable_map(&mut variables, input.variable_overrides);
        Ok(PromptVariableState {
            variables,
            manually_active_knowledge,
        })
    }

    fn character_runtime_initial_variables(
        &self,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<VariableMap> {
        let values = if let Some(authority) = input.prompt_selection_authority {
            authority
                .character_content
                .as_ref()
                .map(|content| content.value.runtime.initial_variables.clone())
                .unwrap_or_default()
        } else {
            match self.storage().get_character_content(&input.character.id) {
                Ok(content) => content.value.runtime.initial_variables,
                Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => {
                    BTreeMap::new()
                }
                Err(error) => return Err(error),
            }
        };
        let mut variables = VariableMap::default();
        for (name, value) in values {
            let value = match value.trim().to_ascii_lowercase().as_str() {
                "true" => VariableValue::Bool(true),
                "false" => VariableValue::Bool(false),
                _ => value
                    .parse::<i64>()
                    .map_or_else(|_| VariableValue::Text(value), VariableValue::Integer),
            };
            variables.insert(
                VariableRef {
                    scope: VariableScope::Character,
                    namespace: None,
                    id: VariableId::from(name),
                },
                value,
            );
        }
        Ok(variables)
    }

    pub fn get_room_orchestration_config(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<RoomOrchestrationConfig> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let branch = self.storage().get_conversation_branch(branch_id)?;
        if branch.conversation_id != *conversation_id {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        let character = self.storage().get_character(&conversation.character_id)?;
        let state = self.storage().get_conversation_state(conversation_id)?;
        let (preset, _, _, effective_binding, _) = self.resolve_prompt_preset_selection(
            &character,
            conversation_id,
            branch_id,
            state.selected_mode,
            None,
        )?;
        let branch_bindings = self
            .list_prompt_preset_bindings(ModuleScope::Branch, Some(branch_id.0.as_str()))?
            .into_iter()
            .filter(|stored| stored.deleted_at.is_none() && stored.value.enabled)
            .collect::<Vec<_>>();
        if branch_bindings.len() > 1 {
            return Err(CoreError::invalid(
                "multiple enabled prompt bindings apply to this room",
            ));
        }
        let binding_revision = branch_bindings.first().map(|stored| stored.revision);
        let binding = effective_binding.as_ref().map(|stored| &stored.value);
        let creator_values = creator_values_from_binding(&preset, binding)?;
        let generation_preset_id = binding
            .and_then(|binding| binding.generation_preset_override_id.clone())
            .or_else(|| preset.default_generation_preset_id.clone());
        let generation_target = if let Some(generation_preset_id) = &generation_preset_id {
            let generation_preset = self.storage().get_generation_preset(generation_preset_id)?;
            Some(GenerationTarget {
                model_route_id: generation_preset.model_route_id,
                generation_preset_id: generation_preset.id,
            })
        } else {
            let settings = self.get_settings()?;
            match (
                settings.selected_model_route_id,
                settings.selected_generation_preset_id,
            ) {
                (Some(model_route_id), Some(generation_preset_id)) => Some(GenerationTarget {
                    model_route_id,
                    generation_preset_id,
                }),
                (None, None) => None,
                _ => {
                    return Err(CoreError::new(
                        lorepia_domain::CoreErrorCode::StorageCorrupted,
                        "stored generation target is incomplete",
                        false,
                    ));
                }
            }
        };
        if let Some(target) = &generation_target {
            self.validate_generation_preset(&target.model_route_id, &target.generation_preset_id)?;
        }
        Ok(RoomOrchestrationConfig {
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            prompt_preset_id: preset.id.clone(),
            generation_preset_id,
            generation_target,
            creator_values,
            variable_overrides: binding
                .map(|binding| binding.variable_overrides.clone())
                .unwrap_or_default(),
            response_length: binding.map_or(PromptResponseLength::Balanced, |binding| {
                binding.response_length
            }),
            creativity: binding.map_or(50, |binding| binding.creativity),
            reasoning_effort: binding.and_then(|binding| binding.reasoning_effort),
            memory_enabled: binding.is_none_or(|binding| binding.memory_enabled),
            knowledge_enabled: binding.is_none_or(|binding| binding.knowledge_enabled),
            user_name_override: binding.and_then(|binding| binding.user_name_override.clone()),
            author_note: binding.and_then(|binding| binding.author_note.clone()),
            group_context: binding.and_then(|binding| binding.group_context.clone()),
            template_slots: binding
                .map(|binding| binding.template_slots.clone())
                .unwrap_or_default(),
            binding_revision,
        })
    }

    pub fn save_room_orchestration_config(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_revision: Option<u64>,
        patch: &RoomOrchestrationConfigPatch,
    ) -> CoreResult<RoomOrchestrationConfig> {
        if patch.creativity > 100 {
            return Err(CoreError::invalid(
                "room creativity must be between 0 and 100",
            ));
        }
        let current = self.get_room_orchestration_config(conversation_id, branch_id)?;
        if current.binding_revision != expected_revision {
            return Err(CoreError::invalid(
                "room orchestration settings changed before save",
            ));
        }
        let preset_id = patch
            .prompt_preset_id
            .clone()
            .unwrap_or(current.prompt_preset_id);
        let stored_preset = self.get_prompt_preset(&preset_id)?;
        let preset = stored_preset.value;
        let variable_overrides =
            canonical_creator_variable_overrides(&preset, &patch.creator_values)?;
        if let Some(generation_preset_id) = &patch.generation_preset_id {
            let generation_preset = self.storage().get_generation_preset(generation_preset_id)?;
            if generation_preset.id != *generation_preset_id {
                return Err(CoreError::internal(
                    "generation preset identity changed during room save",
                ));
            }
        }
        let now = Utc::now();
        let binding = PromptPresetBinding {
            id: deterministic_room_prompt_binding_id(conversation_id, branch_id),
            prompt_preset_id: preset_id,
            scope: ModuleScope::Branch,
            target_id: Some(branch_id.0.clone()),
            conversation_id: Some(conversation_id.clone()),
            pinned_revision_id: None,
            priority: 0,
            enabled: true,
            response_length: patch.response_length,
            creativity: patch.creativity,
            reasoning_effort: patch.reasoning_effort,
            memory_enabled: patch.memory_enabled,
            knowledge_enabled: patch.knowledge_enabled,
            variable_overrides,
            generation_preset_override_id: patch.generation_preset_id.clone(),
            user_name_override: patch.user_name_override.clone(),
            author_note: patch.author_note.clone(),
            group_context: patch.group_context.clone(),
            template_slots: patch.template_slots.clone(),
            created_at: now,
            updated_at: now,
        };
        self.bind_prompt_preset(&binding, expected_revision)?;
        self.get_room_orchestration_config(conversation_id, branch_id)
    }

    pub fn list_prompt_preset_bindings(
        &self,
        scope: ModuleScope,
        target_id: Option<&str>,
    ) -> CoreResult<Vec<Revisioned<PromptPresetBinding>>> {
        self.storage()
            .list_prompt_preset_bindings(scope, target_id)
            .map(project_revisions)
    }

    pub fn unbind_prompt_preset(
        &self,
        binding_id: &str,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<PromptPresetBinding>> {
        self.storage()
            .soft_delete_prompt_preset_binding(binding_id, expected_revision)
            .map(project_revision)
    }
}

fn deterministic_room_prompt_binding_id(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> String {
    let identity = format!(
        "lorepia:room-prompt-binding:v1\u{0}{}\u{0}{}",
        conversation_id.0, branch_id.0
    );
    Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string()
}

fn canonical_creator_variable_overrides(
    preset: &PromptPreset,
    creator_values: &BTreeMap<String, CreatorControlValue>,
) -> CoreResult<VariableMap> {
    if creator_values.len() > preset.controls.len() {
        return Err(CoreError::invalid(
            "creator values contain more controls than the selected preset",
        ));
    }
    let mut variables = VariableMap::default();
    for (control_id, supplied) in creator_values {
        let control = preset
            .controls
            .iter()
            .find(|control| control.id.as_str() == control_id)
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "creator value references unknown control `{control_id}`"
                ))
            })?;
        if control.sensitive {
            return Err(CoreError::invalid(
                "sensitive creator controls cannot cross the frontend boundary",
            ));
        }
        let variable = control.variable.as_ref().ok_or_else(|| {
            CoreError::invalid("presentation-only controls cannot receive creator values")
        })?;
        if variables.get(variable).is_some() {
            return Err(CoreError::invalid(
                "multiple creator controls cannot override the same variable",
            ));
        }
        let value = canonical_creator_control_value(control, supplied)?;
        variables.insert(variable.clone(), value);
    }
    Ok(variables)
}

fn canonical_creator_control_value(
    control: &ControlSpec,
    supplied: &CreatorControlValue,
) -> CoreResult<VariableValue> {
    let value_type = control
        .value_type
        .ok_or_else(|| CoreError::invalid("creator control has no declared value type"))?;
    let value = match (value_type, supplied) {
        (VariableType::Bool, CreatorControlValue::Bool(value)) => VariableValue::Bool(*value),
        (VariableType::Integer, CreatorControlValue::Integer(value)) => {
            VariableValue::Integer(*value)
        }
        (VariableType::Integer, CreatorControlValue::Decimal(value)) => {
            VariableValue::Integer(exact_i64_from_f64(*value).ok_or_else(|| {
                CoreError::invalid("creator value type does not match the selected preset control")
            })?)
        }
        (VariableType::Decimal, CreatorControlValue::Integer(value)) => {
            VariableValue::Decimal(i64_as_f64(*value)?)
        }
        (VariableType::Decimal, CreatorControlValue::Decimal(value)) if value.is_finite() => {
            VariableValue::Decimal(*value)
        }
        (VariableType::Text, CreatorControlValue::Text(value)) => {
            validate_creator_text(value)?;
            VariableValue::Text(value.clone())
        }
        (VariableType::Enum, CreatorControlValue::Text(value)) => {
            validate_creator_text(value)?;
            VariableValue::Enum(value.clone())
        }
        (VariableType::StringList, CreatorControlValue::StringList(values)) => {
            if values.len() > 1_024 {
                return Err(CoreError::invalid(
                    "creator multi-select contains too many values",
                ));
            }
            let mut unique = std::collections::BTreeSet::new();
            for value in values {
                validate_creator_text(value)?;
                if !unique.insert(value.as_str()) {
                    return Err(CoreError::invalid(
                        "creator multi-select contains duplicate values",
                    ));
                }
            }
            let allowed = control
                .options
                .iter()
                .filter_map(|option| match &option.value {
                    VariableValue::Text(value) | VariableValue::Enum(value) => Some(value),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if values.iter().any(|value| !allowed.contains(&value)) {
                return Err(CoreError::invalid(
                    "creator multi-select value is not a declared option",
                ));
            }
            let canonical = allowed
                .into_iter()
                .filter(|value| unique.contains(value.as_str()))
                .cloned()
                .collect();
            VariableValue::StringList(canonical)
        }
        _ => {
            return Err(CoreError::invalid(
                "creator value type does not match the selected preset control",
            ));
        }
    };
    if control.kind == ControlKind::Select
        && !control.options.iter().any(|option| option.value == value)
    {
        return Err(CoreError::invalid(
            "creator select value is not a declared option",
        ));
    }
    validate_creator_numeric_control(control, &value)?;
    Ok(value)
}

fn validate_creator_text(value: &str) -> CoreResult<()> {
    if value.len() > 262_144 || value.chars().count() > 65_536 || value.contains('\0') {
        return Err(CoreError::invalid(
            "creator text exceeds its safe size or contains a null character",
        ));
    }
    Ok(())
}

fn validate_creator_numeric_control(
    control: &ControlSpec,
    value: &VariableValue,
) -> CoreResult<()> {
    let numeric = match value {
        VariableValue::Integer(value) => i64_as_f64(*value)?,
        VariableValue::Decimal(value) => *value,
        _ => return Ok(()),
    };
    if !numeric.is_finite()
        || control.minimum.is_some_and(|minimum| numeric < minimum)
        || control.maximum.is_some_and(|maximum| numeric > maximum)
    {
        return Err(CoreError::invalid(
            "creator numeric value is outside the declared bounds",
        ));
    }
    if let Some(step) = control.step {
        let origin = control.minimum.unwrap_or(0.0);
        let steps = (numeric - origin) / step;
        let tolerance = f64::EPSILON * steps.abs().max(1.0) * 16.0;
        if (steps - steps.round()).abs() > tolerance {
            return Err(CoreError::invalid(
                "creator numeric value does not match the declared step",
            ));
        }
    }
    Ok(())
}

fn exact_i64_from_f64(value: f64) -> Option<i64> {
    (value.is_finite() && value.fract() == 0.0)
        .then(|| value.to_string().parse::<i64>().ok())
        .flatten()
}

fn i64_as_f64(value: i64) -> CoreResult<f64> {
    value
        .to_string()
        .parse::<f64>()
        .map_err(|_| CoreError::internal("integer creator value could not be converted"))
}

fn creator_values_from_binding(
    preset: &PromptPreset,
    binding: Option<&PromptPresetBinding>,
) -> CoreResult<BTreeMap<String, CreatorControlValue>> {
    let Some(binding) = binding else {
        return Ok(BTreeMap::new());
    };
    let mut values = BTreeMap::new();
    for control in &preset.controls {
        if control.sensitive {
            continue;
        }
        let Some(variable) = &control.variable else {
            continue;
        };
        let Some(value) = binding.variable_overrides.get(variable) else {
            continue;
        };
        let value = match value {
            VariableValue::Bool(value) => CreatorControlValue::Bool(*value),
            VariableValue::Integer(value) => CreatorControlValue::Integer(*value),
            VariableValue::Decimal(value) if value.is_finite() => {
                CreatorControlValue::Decimal(*value)
            }
            VariableValue::Text(value) | VariableValue::Enum(value) => {
                CreatorControlValue::Text(value.clone())
            }
            VariableValue::StringList(values) => CreatorControlValue::StringList(values.clone()),
            VariableValue::Decimal(_) => {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "stored creator value is not finite",
                    false,
                ));
            }
        };
        values.insert(control.id.as_str().to_owned(), value);
    }
    Ok(values)
}

fn merge_variable_map(target: &mut VariableMap, source: &VariableMap) {
    for binding in &source.values {
        target.insert(binding.variable.clone(), binding.value.clone());
    }
}
pub(super) fn prompt_creativity_temperature(creativity: u8) -> f64 {
    // Preserve the product's 0.015 step through a JSON round trip. Multiplying
    // directly by a binary floating-point literal can serialize values such as
    // 90 as 1.3499999999999999 and then normalize to 1.35 when decoded.
    f64::from(u16::from(creativity) * 15) / 1_000.0
}
