use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    AssetDescriptor, AssetId, ConversationBranchId, ConversationId, CoreError, CoreErrorCode,
    CoreResult, InteractionRuleSet, ModuleComponentRef, ModuleScope, Provenance, SourceKind,
    TransformSet, VariableMap,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, ModuleMergeReview, ModuleResolutionContext, ResolvedModuleComponent,
};
use lorepia_storage::{ModuleRevisionComponentSnapshot, ObjectRevision};

use crate::Core;

#[derive(Debug, Clone, Default)]
pub(super) struct ResolvedModuleRuntime {
    pub(super) plan_sha256: Option<String>,
    pub(super) variables: VariableMap,
    pub(super) transform_sets: Vec<ObjectRevision<TransformSet>>,
    pub(super) interaction_rule_sets: Vec<ObjectRevision<InteractionRuleSet>>,
    pub(super) knowledge_books: Vec<ObjectRevision<lorepia_domain::KnowledgeBook>>,
    pub(super) assets: BTreeMap<AssetId, ApprovedRuntimeAsset>,
    pub(super) approved_import_source_ids: BTreeSet<String>,
    pub(super) approved_module_sources: BTreeSet<(String, String, String)>,
}

#[derive(Debug, Clone)]
pub(super) struct ApprovedRuntimeAsset {
    pub(super) descriptor: AssetDescriptor,
    pub(super) module_id: String,
    pub(super) module_revision_id: String,
    pub(super) component_sha256: String,
}

impl Core {
    pub(super) fn resolve_runtime_modules(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ResolvedModuleRuntime> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let branch = self.storage().get_conversation_branch(branch_id)?;
        if branch.conversation_id != *conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        let persona_id = self
            .storage()
            .get_conversation_persona_selection(conversation_id)?
            .map(|selection| selection.value.persona_id);
        let context = ModuleResolutionContext {
            local_user_id: self.storage().load_settings()?.local_user_id,
            persona_id,
            character_id: Some(conversation.character_id.clone()),
            conversation_id: Some(conversation_id.0.clone()),
            branch_id: Some(branch_id.0.clone()),
            supported_capabilities: crate::module_orchestration::SUPPORTED_CONTENT_CAPABILITIES
                .to_vec(),
        };
        let bindings = self.storage().list_all_module_bindings()?;
        let has_applicable_approved_binding = bindings.iter().any(|stored| {
            stored.deleted_at.is_none()
                && stored.value.enabled
                && stored.value.approved
                && module_binding_applies_to_runtime(&stored.value, &context)
        });
        if !has_applicable_approved_binding {
            return Ok(ResolvedModuleRuntime::default());
        }

        // Exactly one full-context applied plan is authoritative. Replaying
        // each binding's historical activation plan independently would
        // resurrect components that lost a later composition conflict.
        let approved = self.resolve_applied_content_module_runtime_plan(&context)?;
        self.materialize_resolved_module_runtime(&approved)
    }

    /// Resolves one not-yet-materialized branch against the exact runtime
    /// module context that the later atomic branch append will promote.
    ///
    /// `None` is authoritative only when no approved binding applies. It is
    /// distinct from a failed or ambiguous materialization, both of which fail
    /// closed before any generation attempt can advance.
    pub(crate) fn preview_module_runtime_authority_for_proposed_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<(ModuleMergeReview, Option<AppliedModuleRuntimePlan>)> {
        let context =
            self.content_module_context_for_proposed_branch(conversation_id, branch_id)?;
        let review = self.review_current_content_module_runtime(&context)?;
        if review.ordered_bindings.is_empty() {
            return Ok((review, None));
        }
        let approved = self
            .storage()
            .preview_applied_module_runtime_plan(&review)?;
        approved.verify().map_err(module_plan_error)?;
        if approved.review.context != context {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "previewed module plan differs from its proposed branch context",
                false,
            ));
        }
        Ok((review, Some(approved)))
    }

    pub(super) fn materialize_resolved_module_runtime(
        &self,
        approved: &AppliedModuleRuntimePlan,
    ) -> CoreResult<ResolvedModuleRuntime> {
        approved.verify().map_err(module_plan_error)?;
        let ordered_binding_ids = approved
            .plan
            .ordered_binding_ids
            .iter()
            .map(lorepia_domain::ModuleBindingId::as_str)
            .collect::<BTreeSet<_>>();
        let mut approved_module_sources = BTreeSet::new();
        for source in approved.plan.components.iter().flat_map(|component| {
            std::iter::once(&component.selected_source).chain(component.coalesced_sources.iter())
        }) {
            if !ordered_binding_ids.contains(source.binding_id.as_str()) {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "approved module component names a source outside the approved binding order",
                    false,
                ));
            }
            approved_module_sources.insert((
                source.module_id.as_str().to_owned(),
                source.revision_id.as_str().to_owned(),
                source.revision_source_sha256.as_str().to_owned(),
            ));
        }

        let mut runtime = ResolvedModuleRuntime {
            plan_sha256: Some(approved.applied_plan_sha256.as_str().to_owned()),
            variables: approved.plan.effective_variable_overrides.clone(),
            approved_module_sources,
            ..ResolvedModuleRuntime::default()
        };
        for component in &approved.plan.components {
            self.materialize_runtime_component(&mut runtime, component)?;
        }
        runtime.variables.validate().map_err(|error| {
            CoreError::invalid(format!("module variables are invalid: {error}"))
        })?;
        runtime
            .transform_sets
            .sort_by(|left, right| left.value.id.cmp(&right.value.id));
        runtime
            .knowledge_books
            .sort_by(|left, right| left.value.id.cmp(&right.value.id));
        Ok(runtime)
    }

    fn materialize_runtime_component(
        &self,
        runtime: &mut ResolvedModuleRuntime,
        component: &ResolvedModuleComponent,
    ) -> CoreResult<()> {
        let snapshot = self.load_approved_content_module_component(
            &crate::module_orchestration::ApprovedContentModuleComponent {
                component: component.component.clone(),
                component_sha256: component.sha256.clone(),
                selected_source: component.selected_source.clone(),
                runtime_enabled: component.runtime_enabled,
            },
        )?;
        match (&component.component, snapshot) {
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
                        &mut runtime.approved_import_source_ids,
                    )?;
                }
                runtime.transform_sets.push(transform_set);
            }
            (
                ModuleComponentRef::InteractionRuleSet { .. },
                ModuleRevisionComponentSnapshot::InteractionRuleSet(mut rule_set),
            ) => {
                apply_exact_interaction_runtime_overlay(
                    &mut rule_set.value,
                    component.runtime_enabled,
                );
                if component.runtime_enabled {
                    collect_exact_component_import_approvals(
                        &rule_set.value.provenance,
                        rule_set.value.rules.iter().map(|rule| &rule.provenance),
                        &mut runtime.approved_import_source_ids,
                    )?;
                }
                runtime.interaction_rule_sets.push(rule_set);
            }
            (
                ModuleComponentRef::KnowledgeBook { .. },
                ModuleRevisionComponentSnapshot::KnowledgeBook(book),
            ) => runtime.knowledge_books.push(book),
            (
                ModuleComponentRef::Asset { id },
                ModuleRevisionComponentSnapshot::Asset(descriptor),
            ) => Self::materialize_runtime_asset(runtime, component, id, descriptor)?,
            (
                ModuleComponentRef::PromptBlock { .. },
                ModuleRevisionComponentSnapshot::PromptBlock(_),
            )
            | (ModuleComponentRef::Control { .. }, ModuleRevisionComponentSnapshot::Control(_)) => {
            }
            _ => {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "approved module component resolved to the wrong immutable type",
                    false,
                ));
            }
        }
        Ok(())
    }

    fn materialize_runtime_asset(
        runtime: &mut ResolvedModuleRuntime,
        component: &ResolvedModuleComponent,
        id: &AssetId,
        descriptor: AssetDescriptor,
    ) -> CoreResult<()> {
        if descriptor.id != *id {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "approved module asset identity differs from its component",
                false,
            ));
        }
        let evidence = ApprovedRuntimeAsset {
            descriptor,
            module_id: component.selected_source.module_id.as_str().to_owned(),
            module_revision_id: component.selected_source.revision_id.as_str().to_owned(),
            component_sha256: component.sha256.as_str().to_owned(),
        };
        if runtime.assets.insert(id.clone(), evidence).is_some() {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "approved module plan contains duplicate asset identities",
                false,
            ));
        }
        Ok(())
    }
}

fn module_binding_applies_to_runtime(
    binding: &lorepia_domain::ModuleBinding,
    context: &ModuleResolutionContext,
) -> bool {
    match binding.scope {
        ModuleScope::App | ModuleScope::User => {
            binding.target_id.is_none() && binding.conversation_id.is_none()
        }
        ModuleScope::Persona => {
            binding.target_id.as_deref()
                == context
                    .persona_id
                    .as_ref()
                    .map(lorepia_domain::PersonaId::as_str)
        }
        ModuleScope::Character => binding.target_id == context.character_id,
        ModuleScope::Conversation => binding.target_id == context.conversation_id,
        ModuleScope::Branch => {
            binding.target_id == context.branch_id
                && binding
                    .conversation_id
                    .as_ref()
                    .map(|conversation_id| conversation_id.0.as_str())
                    == context.conversation_id.as_deref()
        }
    }
}

pub(crate) fn apply_exact_transform_runtime_overlay(
    transform_set: &mut TransformSet,
    runtime_enabled: bool,
) {
    if !runtime_enabled {
        transform_set.enabled = false;
        for rule in &mut transform_set.rules {
            rule.enabled = false;
            rule.imported_enabled = false;
        }
        return;
    }
    if is_imported_runtime_provenance(&transform_set.provenance) {
        transform_set.enabled = transform_set.imported_author_enabled;
    }
    for rule in &mut transform_set.rules {
        if is_imported_runtime_provenance(&rule.provenance) {
            rule.enabled = rule.imported_author_enabled;
            rule.imported_enabled = rule.imported_author_enabled;
        }
    }
}

fn apply_exact_interaction_runtime_overlay(
    rule_set: &mut InteractionRuleSet,
    runtime_enabled: bool,
) {
    for rule in &mut rule_set.rules {
        if !runtime_enabled {
            rule.enabled = false;
        } else if is_imported_runtime_provenance(&rule.provenance) {
            rule.enabled = rule.imported_author_enabled;
        }
    }
}

fn is_imported_runtime_provenance(provenance: &Provenance) -> bool {
    matches!(
        provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    )
}

pub(crate) fn collect_exact_component_import_approvals<'a>(
    component_provenance: &Provenance,
    child_provenance: impl IntoIterator<Item = &'a Provenance>,
    approvals: &mut BTreeSet<String>,
) -> CoreResult<()> {
    let component_source = imported_runtime_source_id(component_provenance)?;
    if let Some(source_id) = component_source {
        approvals.insert(source_id.to_owned());
    }
    for provenance in child_provenance {
        let Some(source_id) = imported_runtime_source_id(provenance)? else {
            continue;
        };
        if component_source.is_some_and(|component| component != source_id) {
            return Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "an imported approved component contains a child from a different source",
                false,
            ));
        }
        approvals.insert(source_id.to_owned());
    }
    Ok(())
}

fn imported_runtime_source_id(provenance: &Provenance) -> CoreResult<Option<&str>> {
    if matches!(
        provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    ) {
        return provenance
            .source_id
            .as_deref()
            .filter(|source_id| !source_id.is_empty())
            .map(Some)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "approved imported runtime content has no source identity",
                    false,
                )
            });
    }
    Ok(None)
}

pub(super) fn module_plan_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!("invalid approved module runtime plan: {error}"))
}
