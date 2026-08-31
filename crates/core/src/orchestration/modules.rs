use crate::{
    Core, Revisioned,
    orchestration_runtime::{
        apply_exact_transform_runtime_overlay, collect_exact_component_import_approvals,
    },
    revision::{project_revision, project_revisions},
};
use lorepia_domain::{
    Character, ContentCapability, ContentModule, ContentModuleId, ControlSpec,
    ConversationBranchId, ConversationId, ConversationMode, CoreError, CoreResult, GenerationId,
    InteractionRuleSet, InteractionRuleSetId, KnowledgeBook, KnowledgeEntryId, ModuleBinding,
    ModuleBindingId, ModuleComponentRef, ModuleScope, PersonaId, PromptPreset, PromptPresetId,
    SourceKind, TransformSet, VariableMap, prompt_local_user_id_sha256,
};
use lorepia_orchestration::{AppliedModuleRuntimePlan, ModuleResolutionContext};
use lorepia_storage::{
    ContentModuleRevisionDiff, InteractionKnowledgeBinding, ModuleRevisionComponentSnapshot,
    ObjectRevision,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContentShareGate {
    pub module_id: ContentModuleId,
    pub local_use_allowed: bool,
    pub sharing_allowed: bool,
    pub reasons: Vec<String>,
}

#[derive(Default)]
pub(super) struct PromptModuleOverlay {
    pub(super) plan_sha256: Option<String>,
    pub(super) prompt_blocks: Vec<lorepia_domain::PromptBlock>,
    pub(super) prompt_block_source_revisions: BTreeMap<lorepia_domain::PromptBlockId, String>,
    pub(super) controls: Vec<ControlSpec>,
    pub(super) knowledge_books: Vec<ObjectRevision<KnowledgeBook>>,
    pub(super) transform_sets: Vec<ObjectRevision<TransformSet>>,
    pub(super) variables: VariableMap,
    pub(super) approved_import_source_ids: BTreeSet<String>,
    pub(super) warnings: Vec<String>,
}

pub(super) struct PromptModuleOverlayInput<'a> {
    pub(super) character: &'a Character,
    pub(super) conversation_id: &'a ConversationId,
    pub(super) branch_id: &'a ConversationBranchId,
    pub(super) persona_id: Option<&'a PersonaId>,
    pub(super) applied_plan_override: Option<&'a AppliedModuleRuntimePlan>,
    pub(super) sealed_local_user_id_sha256: Option<&'a str>,
    pub(super) generation_attempt_id: Option<&'a GenerationId>,
}

impl Core {
    /// Resolves the immutable module-composition authority that must be bound
    /// into a generation attempt before `BeforeGeneration` is delivered.
    ///
    /// Interaction rules may change variables, knowledge, or approval state,
    /// but they cannot replace the exact content-module composition admitted
    /// for this room. The final prompt plan independently carries the same
    /// hash and storage rechecks it at the dispatch-ready append boundary.
    pub(crate) fn resolve_generation_module_plan_sha256(
        &self,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        prompt_preset_id: Option<&PromptPresetId>,
    ) -> CoreResult<lorepia_domain::Sha256Digest> {
        let (preset, _revision, prompt_preset_revision_id, _binding, persona_selection) = self
            .resolve_prompt_preset_selection(
                character,
                conversation_id,
                branch_id,
                mode,
                prompt_preset_id,
            )?;
        let module_overlay = self.resolve_prompt_module_overlay(
            &preset,
            &prompt_preset_revision_id,
            PromptModuleOverlayInput {
                character,
                conversation_id,
                branch_id,
                persona_id: persona_selection
                    .as_ref()
                    .map(|selection| &selection.value.persona_id),
                applied_plan_override: None,
                sealed_local_user_id_sha256: None,
                generation_attempt_id: None,
            },
        )?;
        module_overlay.plan_sha256.map_or_else(
            || Ok(lorepia_orchestration::no_applied_module_runtime_plan_sha256()),
            |sha256| lorepia_domain::Sha256Digest::parse(sha256).map_err(CoreError::invalid),
        )
    }

    pub fn upsert_interaction_rule_set(
        &self,
        rule_set: &InteractionRuleSet,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<InteractionRuleSet>> {
        self.storage()
            .save_interaction_rule_set(rule_set, expected_revision)
            .map(project_revision)
    }

    pub fn get_interaction_rule_set(
        &self,
        id: &InteractionRuleSetId,
    ) -> CoreResult<Revisioned<InteractionRuleSet>> {
        self.storage()
            .get_interaction_rule_set(id)
            .map(project_revision)
    }

    pub fn list_interaction_rule_sets(&self) -> CoreResult<Vec<Revisioned<InteractionRuleSet>>> {
        self.storage()
            .list_interaction_rule_sets()
            .map(project_revisions)
    }

    pub fn delete_interaction_rule_set(
        &self,
        id: &InteractionRuleSetId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<InteractionRuleSet>> {
        self.storage()
            .soft_delete_interaction_rule_set(id, expected_revision)
            .map(project_revision)
    }

    pub fn upsert_content_module(
        &self,
        module: &ContentModule,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<ContentModule>> {
        self.storage()
            .save_content_module(module, expected_revision)
            .map(project_revision)
    }

    pub fn get_content_module(
        &self,
        id: &ContentModuleId,
    ) -> CoreResult<Revisioned<ContentModule>> {
        self.storage().get_content_module(id).map(project_revision)
    }

    pub fn list_content_modules(&self) -> CoreResult<Vec<Revisioned<ContentModule>>> {
        self.storage().list_content_modules().map(project_revisions)
    }

    pub fn delete_content_module(
        &self,
        id: &ContentModuleId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<ContentModule>> {
        self.storage()
            .soft_delete_content_module(id, expected_revision)
            .map(project_revision)
    }

    pub fn list_content_module_bindings(
        &self,
        module_id: &ContentModuleId,
    ) -> CoreResult<Vec<Revisioned<ModuleBinding>>> {
        self.storage()
            .list_module_bindings(module_id)
            .map(project_revisions)
    }

    pub fn unbind_content_module(
        &self,
        binding_id: &ModuleBindingId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<ModuleBinding>> {
        self.storage()
            .soft_delete_module_binding(binding_id, expected_revision)
            .map(project_revision)
    }

    pub fn list_content_module_revisions(
        &self,
        id: &ContentModuleId,
    ) -> CoreResult<Vec<ObjectRevision<ContentModule>>> {
        self.storage().list_content_module_revisions(id)
    }

    pub fn diff_content_module_revisions(
        &self,
        id: &ContentModuleId,
        from_revision: u64,
        to_revision: u64,
    ) -> CoreResult<ContentModuleRevisionDiff> {
        self.storage()
            .diff_content_module_revisions(id, from_revision, to_revision)
    }

    /// Evaluates the non-networked share gate for a module.
    ///
    /// The decision does not upload or publish anything. Unknown licenses,
    /// missing imported-source hashes, high-risk assets, and explicit
    /// redistribution denial fail closed while local use remains available.
    pub fn evaluate_content_module_share_gate(
        &self,
        id: &ContentModuleId,
    ) -> CoreResult<ContentShareGate> {
        let module = self.get_content_module(id)?.value;
        let mut reasons = Vec::new();
        let license = module.metadata.license.trim();
        if license.is_empty()
            || license.eq_ignore_ascii_case("unknown")
            || license.eq_ignore_ascii_case("LicenseRef-Unknown")
        {
            reasons.push("content license is unknown".to_owned());
        }
        if !module.metadata.redistribution_allowed {
            reasons.push("content metadata does not allow redistribution".to_owned());
        }
        if module
            .required_capabilities
            .contains(&ContentCapability::HighRiskAssets)
        {
            reasons.push("module contains high-risk assets".to_owned());
        }
        if module.metadata.provenance.source_kind == SourceKind::ImportedPackage
            && module.metadata.provenance.source_hash.is_none()
        {
            reasons.push("imported module has no immutable source hash".to_owned());
        }
        Ok(ContentShareGate {
            module_id: module.id,
            local_use_allowed: true,
            sharing_allowed: reasons.is_empty(),
            reasons,
        })
    }

    pub(super) fn resolve_prompt_module_overlay(
        &self,
        preset: &PromptPreset,
        prompt_preset_revision_id: &str,
        input: PromptModuleOverlayInput<'_>,
    ) -> CoreResult<PromptModuleOverlay> {
        let preset_dependencies = if preset.module_ids.is_empty() {
            Vec::new()
        } else {
            self.storage()
                .get_prompt_preset_module_dependencies(prompt_preset_revision_id)?
        };
        if input.applied_plan_override.is_none()
            && let Some(generation_id) = input.generation_attempt_id
        {
            let attempt = self.storage().get_generation_attempt(generation_id)?;
            if input.sealed_local_user_id_sha256.is_none()
                || attempt.input.module_plan_sha256
                    != lorepia_orchestration::no_applied_module_runtime_plan_sha256()
            {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "attempt no-module authority is incomplete",
                    false,
                ));
            }
            return missing_preset_module_overlay(preset, &preset_dependencies);
        }
        let local_user_id = if let Some(applied) = input.applied_plan_override {
            applied.verify().map_err(module_plan_error)?;
            if input.sealed_local_user_id_sha256.is_some_and(|expected| {
                prompt_local_user_id_sha256(&applied.review.context.local_user_id) != expected
            }) {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "attempt module plan local user differs from sealed prompt authority",
                    false,
                ));
            }
            applied.review.context.local_user_id.clone()
        } else {
            let local_user_id = self.storage().load_settings()?.local_user_id;
            if input
                .sealed_local_user_id_sha256
                .is_some_and(|expected| prompt_local_user_id_sha256(&local_user_id) != expected)
            {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "current local user differs from sealed prompt authority",
                    false,
                ));
            }
            local_user_id
        };
        let context = ModuleResolutionContext {
            local_user_id,
            persona_id: input.persona_id.cloned(),
            character_id: Some(input.character.id.clone()),
            conversation_id: Some(input.conversation_id.0.clone()),
            branch_id: Some(input.branch_id.0.clone()),
            supported_capabilities: crate::module_orchestration::SUPPORTED_CONTENT_CAPABILITIES
                .to_vec(),
        };
        if let Some(applied) = input.applied_plan_override {
            if applied.review.context != context {
                return Err(CoreError::invalid(
                    "applied module plan override does not match the prompt context",
                ));
            }
            return self.materialize_prompt_module_overlay(preset, &preset_dependencies, applied);
        }
        let bindings = self.storage().list_all_module_bindings()?;
        let has_applicable_binding = bindings.iter().any(|stored| {
            stored.deleted_at.is_none()
                && stored.value.enabled
                && stored.value.approved
                && module_binding_applies_to_prompt(
                    &stored.value,
                    input.conversation_id,
                    input.branch_id,
                    &input.character.id,
                    input.persona_id.map(PersonaId::as_str),
                )
        });
        let approved = match self.resolve_applied_content_module_runtime_plan(&context) {
            Ok(approved) => approved,
            Err(error)
                if error.code == lorepia_domain::CoreErrorCode::NotFound
                    && !has_applicable_binding =>
            {
                return missing_preset_module_overlay(preset, &preset_dependencies);
            }
            Err(error) => return Err(error),
        };
        approved.verify().map_err(module_plan_error)?;
        self.materialize_prompt_module_overlay(preset, &preset_dependencies, &approved)
    }

    fn materialize_prompt_module_overlay(
        &self,
        preset: &PromptPreset,
        preset_dependencies: &[lorepia_storage::PromptPresetModuleDependency],
        approved: &AppliedModuleRuntimePlan,
    ) -> CoreResult<PromptModuleOverlay> {
        let mut overlay = initialize_prompt_module_overlay(preset, preset_dependencies, approved)?;

        for component in &approved.plan.components {
            let snapshot = self.storage().get_module_revision_component(
                &component.selected_source,
                &component.component,
                &component.sha256,
            )?;
            match (&component.component, snapshot) {
                (
                    ModuleComponentRef::PromptBlock { .. },
                    ModuleRevisionComponentSnapshot::PromptBlock(mut block),
                ) => {
                    if block.authority == lorepia_domain::InstructionAuthority::Application
                        || block.placement_zone == lorepia_domain::PlacementZone::ApplicationPolicy
                        || block.provenance.source_kind == SourceKind::ApplicationBuiltIn
                    {
                        return Err(CoreError::new(
                            lorepia_domain::CoreErrorCode::PermissionDenied,
                            "approved module attempted to replace application prompt policy",
                            false,
                        ));
                    }
                    // The immutable component digest is stronger runtime
                    // provenance than an optional package-level source hash.
                    block.provenance.source_hash = Some(component.sha256.as_str().to_owned());
                    overlay.prompt_block_source_revisions.insert(
                        block.id.clone(),
                        component.selected_source.revision_id.as_str().to_owned(),
                    );
                    overlay.prompt_blocks.push(block);
                }
                (
                    ModuleComponentRef::Control { .. },
                    ModuleRevisionComponentSnapshot::Control(control),
                ) => overlay.controls.push(control),
                (
                    ModuleComponentRef::KnowledgeBook { .. },
                    ModuleRevisionComponentSnapshot::KnowledgeBook(book),
                ) => overlay.knowledge_books.push(book),
                (
                    ModuleComponentRef::TransformSet { .. },
                    ModuleRevisionComponentSnapshot::TransformSet(mut transform_set),
                ) => {
                    apply_exact_transform_runtime_overlay(
                        &mut transform_set.value,
                        component.runtime_enabled,
                    );
                    if component.runtime_enabled {
                        collect_exact_component_import_approvals(
                            &transform_set.value.provenance,
                            transform_set
                                .value
                                .rules
                                .iter()
                                .map(|rule| &rule.provenance),
                            &mut overlay.approved_import_source_ids,
                        )?;
                    }
                    overlay.transform_sets.push(transform_set);
                }
                (
                    ModuleComponentRef::InteractionRuleSet { .. },
                    ModuleRevisionComponentSnapshot::InteractionRuleSet(_),
                )
                | (ModuleComponentRef::Asset { .. }, ModuleRevisionComponentSnapshot::Asset(_)) => {
                }
                _ => {
                    return Err(CoreError::new(
                        lorepia_domain::CoreErrorCode::StorageCorrupted,
                        "approved module component resolved to the wrong immutable type",
                        false,
                    ));
                }
            }
        }
        Ok(overlay)
    }
}

pub(super) fn prompt_module_knowledge_revisions(
    books: &[ObjectRevision<KnowledgeBook>],
) -> CoreResult<BTreeMap<KnowledgeEntryId, String>> {
    let mut revisions = BTreeMap::new();
    for book in books {
        for entry in &book.value.entries {
            if revisions
                .insert(entry.id.clone(), book.revision_id.clone())
                .is_some()
            {
                return Err(CoreError::invalid(
                    "approved module knowledge entry IDs are ambiguous",
                ));
            }
        }
    }
    Ok(revisions)
}

pub(super) fn exact_prompt_manual_knowledge(
    manually_active: &[KnowledgeEntryId],
    bindings: &[InteractionKnowledgeBinding],
    current_revisions: &BTreeMap<KnowledgeEntryId, String>,
) -> CoreResult<BTreeSet<KnowledgeEntryId>> {
    let mut bindings_by_entry = BTreeMap::new();
    for binding in bindings {
        if bindings_by_entry
            .insert(binding.entry_id.clone(), binding)
            .is_some()
        {
            return Err(CoreError::invalid(
                "manual knowledge activation has duplicate revision bindings",
            ));
        }
    }
    let mut exact = BTreeSet::new();
    for entry_id in manually_active {
        let binding = bindings_by_entry.get(entry_id).ok_or_else(|| {
            CoreError::invalid(format!(
                "manual knowledge entry {} has no revision binding",
                entry_id.as_str()
            ))
        })?;
        if current_revisions
            .get(entry_id)
            .is_some_and(|revision| revision.as_str() == binding.book_revision_id.as_str())
        {
            exact.insert(entry_id.clone());
        }
    }
    Ok(exact)
}

fn missing_preset_module_overlay(
    preset: &PromptPreset,
    dependencies: &[lorepia_storage::PromptPresetModuleDependency],
) -> CoreResult<PromptModuleOverlay> {
    if dependencies.is_empty() {
        return Ok(PromptModuleOverlay::default());
    }
    if matches!(
        preset.metadata.provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    ) {
        return Err(CoreError::new(
            lorepia_domain::CoreErrorCode::PermissionDenied,
            "imported prompt preset requires an exact approved module plan",
            false,
        ));
    }
    Ok(PromptModuleOverlay {
        warnings: vec![format!(
            "{} local preset module dependencies were omitted because no exact approved module plan exists",
            dependencies.len()
        )],
        ..PromptModuleOverlay::default()
    })
}

fn initialize_prompt_module_overlay(
    preset: &PromptPreset,
    preset_dependencies: &[lorepia_storage::PromptPresetModuleDependency],
    approved: &AppliedModuleRuntimePlan,
) -> CoreResult<PromptModuleOverlay> {
    let selected_sources = approved
        .plan
        .components
        .iter()
        .flat_map(|component| {
            std::iter::once(&component.selected_source).chain(component.coalesced_sources.iter())
        })
        .collect::<BTreeSet<_>>();
    let missing_dependencies = preset_dependencies
        .iter()
        .filter(|dependency| {
            !selected_sources.iter().any(|source| {
                source.module_id == dependency.module_id
                    && source.revision_id == dependency.module_revision_id
                    && source.revision_source_sha256 == dependency.source_sha256
            })
        })
        .count();
    let mut overlay = PromptModuleOverlay {
        plan_sha256: Some(approved.applied_plan_sha256.as_str().to_owned()),
        variables: approved.plan.effective_variable_overrides.clone(),
        ..PromptModuleOverlay::default()
    };
    if missing_dependencies == 0 {
        return Ok(overlay);
    }
    if matches!(
        preset.metadata.provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    ) {
        return Err(CoreError::new(
            lorepia_domain::CoreErrorCode::PermissionDenied,
            "imported prompt preset has an unapproved or stale module dependency",
            false,
        ));
    }
    overlay.warnings.push(format!(
        "{missing_dependencies} local preset module dependencies were omitted because the exact approved revision is unavailable"
    ));
    Ok(overlay)
}

fn module_binding_applies_to_prompt(
    binding: &ModuleBinding,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    character_id: &str,
    persona_id: Option<&str>,
) -> bool {
    match binding.scope {
        ModuleScope::App | ModuleScope::User => binding.target_id.is_none(),
        ModuleScope::Persona => binding.target_id.as_deref() == persona_id,
        ModuleScope::Character => binding.target_id.as_deref() == Some(character_id),
        ModuleScope::Conversation => {
            binding.target_id.as_deref() == Some(conversation_id.0.as_str())
        }
        ModuleScope::Branch => {
            binding.target_id.as_deref() == Some(branch_id.0.as_str())
                && binding.conversation_id.as_ref() == Some(conversation_id)
        }
    }
}

fn module_plan_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!("approved module plan is invalid: {error}"))
}
