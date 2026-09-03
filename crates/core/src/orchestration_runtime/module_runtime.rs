use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};

use lorepia_domain::{
    ActivationRule, AssetDescriptor, AssetId, CharacterContentV1, CharacterKnowledgeBookRef,
    CharacterRuntimeProfile, ConversationBranchId, ConversationId, CoreError, CoreErrorCode,
    CoreResult, InteractionRuleSet, KnowledgeBook, KnowledgePlacement, ModuleComponentRef,
    ModuleScope, PortableKnowledgeBook, PortableKnowledgeEntry, PortableKnowledgePlacement,
    Provenance, SourceKind, TransformSet, VariableMap,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, ModuleMergeReview, ModuleResolutionContext, ResolvedModuleComponent,
};
use lorepia_storage::{ModuleRevisionComponentSnapshot, ObjectRevision};
use sha2::{Digest, Sha256};

use crate::{Core, Revisioned};

#[derive(Debug, Clone, Default)]
pub(crate) struct ResolvedModuleRuntime {
    pub(super) plan_sha256: Option<String>,
    pub(super) variables: VariableMap,
    pub(super) transform_sets: Vec<ObjectRevision<TransformSet>>,
    pub(super) interaction_rule_sets: Vec<ObjectRevision<InteractionRuleSet>>,
    pub(super) knowledge_books: Vec<ObjectRevision<lorepia_domain::KnowledgeBook>>,
    pub(super) assets: BTreeMap<AssetId, ApprovedRuntimeAsset>,
    pub(super) approved_import_source_ids: BTreeSet<String>,
    pub(super) approved_module_sources: BTreeSet<(String, String, String)>,
    pub(crate) portable_runtimes: Vec<CharacterRuntimeProfile>,
}

#[derive(Debug, Clone)]
pub(super) struct ApprovedRuntimeAsset {
    pub(super) descriptor: AssetDescriptor,
    pub(super) module_id: String,
    pub(super) module_revision_id: String,
    pub(super) component_sha256: String,
}

impl Core {
    /// Returns the exact character content visible to one approved room,
    /// including portable runtime and assets from its current module plan.
    pub fn get_effective_character_content(
        &self,
        character_id: &str,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Revisioned<CharacterContentV1>> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let branch = self.storage().get_conversation_branch(branch_id)?;
        if conversation.character_id != character_id || branch.conversation_id != *conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "character runtime room scope does not match the requested character",
                false,
            ));
        }
        let mut content = self.get_character_content(character_id)?;
        let runtime = self.resolve_runtime_modules(conversation_id, branch_id)?;
        merge_portable_module_runtime(&mut content.value, &runtime)?;
        if let Some(plan_sha256) = runtime.plan_sha256.as_deref() {
            let mut digest = Sha256::new();
            digest.update(b"effective-character-runtime-v1\0");
            digest.update(
                content
                    .revision_id
                    .as_deref()
                    .unwrap_or("legacy")
                    .as_bytes(),
            );
            digest.update([0]);
            digest.update(plan_sha256.as_bytes());
            let mut revision_id = String::with_capacity(64);
            for byte in digest.finalize() {
                write!(&mut revision_id, "{byte:02x}")
                    .expect("writing a digest into a String cannot fail");
            }
            content.revision_id = Some(revision_id);
        }
        Ok(content)
    }

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
            supported_capabilities: crate::module_orchestration::CAPABILITIES.to_vec(),
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
        for binding in &approved.review.ordered_bindings {
            let stored = self
                .storage()
                .get_content_module_revision(&binding.module_id, &binding.revision_id)?;
            let expected_source = approved_module_source_hash(approved, binding);
            if stored.object.value.portable_runtime.is_some() && expected_source.is_none() {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "approved portable runtime module has no immutable source authority",
                    false,
                ));
            }
            if expected_source
                .is_some_and(|expected| stored.module_revision.source_hash != expected)
            {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "approved portable runtime module source changed after review",
                    false,
                ));
            }
            if let Some(profile) = stored.object.value.portable_runtime {
                runtime.portable_runtimes.push(profile);
            }
        }
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

fn merge_portable_module_runtime(
    content: &mut CharacterContentV1,
    resolved: &ResolvedModuleRuntime,
) -> CoreResult<()> {
    for incoming in &resolved.portable_runtimes {
        merge_portable_runtime_profile(&mut content.runtime, incoming);
    }
    for descriptor in resolved.assets.values() {
        if let Some(existing) = content
            .assets
            .iter()
            .find(|asset| asset.id == descriptor.descriptor.id)
        {
            if existing != &descriptor.descriptor {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "approved module asset conflicts with the character asset identity",
                    false,
                ));
            }
        } else {
            content.assets.push(descriptor.descriptor.clone());
        }
    }
    content.assets.sort_by(|left, right| left.id.cmp(&right.id));
    merge_portable_runtime_knowledge(content, &resolved.knowledge_books);
    if let Some(plan_sha256) = resolved.plan_sha256.as_deref() {
        content.runtime.source_id = Some(format!("approved-module-plan:{plan_sha256}"));
    }
    Ok(())
}

fn merge_portable_runtime_profile(
    target: &mut CharacterRuntimeProfile,
    incoming: &CharacterRuntimeProfile,
) {
    target
        .transforms
        .extend(incoming.transforms.iter().cloned());
    target.scripts.extend(incoming.scripts.iter().cloned());
    append_portable_markup(&mut target.background_markup, &incoming.background_markup);
    append_portable_markup(&mut target.additional_text, &incoming.additional_text);
    append_portable_markup(&mut target.toggle_schema, &incoming.toggle_schema);
    target
        .initial_variables
        .extend(incoming.initial_variables.clone());
    target.metadata.extend(incoming.metadata.clone());
    let mut capabilities = target.required_capabilities.take().unwrap_or_default();
    capabilities.extend(
        incoming
            .required_capabilities
            .as_deref()
            .unwrap_or_default()
            .iter()
            .copied(),
    );
    capabilities.sort_unstable();
    capabilities.dedup();
    target.required_capabilities = (!capabilities.is_empty()).then_some(capabilities);
}

fn append_portable_markup(target: &mut String, incoming: &str) {
    if incoming.trim().is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push('\n');
    }
    target.push_str(incoming);
}

fn merge_portable_runtime_knowledge(
    content: &mut CharacterContentV1,
    books: &[ObjectRevision<KnowledgeBook>],
) {
    if books.is_empty() {
        return;
    }
    let book = content
        .knowledge_book
        .get_or_insert_with(|| CharacterKnowledgeBookRef {
            id: None,
            name: Some("Effective runtime knowledge".to_owned()),
            source_sha256: None,
            embedded: Some(PortableKnowledgeBook {
                id: lorepia_domain::KnowledgeBookId::from("effective-runtime-knowledge"),
                name: "Effective runtime knowledge".to_owned(),
                entries: Vec::new(),
                scan_depth: 0,
                token_budget: 0,
                recursive: false,
                max_recursion_depth: 0,
                metadata: BTreeMap::new(),
            }),
        })
        .embedded
        .get_or_insert_with(|| PortableKnowledgeBook {
            id: lorepia_domain::KnowledgeBookId::from("effective-runtime-knowledge"),
            name: "Effective runtime knowledge".to_owned(),
            entries: Vec::new(),
            scan_depth: 0,
            token_budget: 0,
            recursive: false,
            max_recursion_depth: 0,
            metadata: BTreeMap::new(),
        });
    for stored in books {
        book.scan_depth = book.scan_depth.max(stored.value.scan_depth);
        book.token_budget = book.token_budget.max(stored.value.token_budget.max_tokens);
        book.recursive |= stored.value.recursive;
        book.max_recursion_depth = book
            .max_recursion_depth
            .max(stored.value.max_recursion_depth);
        book.entries.extend(
            stored
                .value
                .entries
                .iter()
                .map(|entry| portable_runtime_knowledge_entry(stored.value.id.as_str(), entry)),
        );
    }
}

fn portable_runtime_knowledge_entry(
    book_id: &str,
    entry: &lorepia_domain::KnowledgeEntry,
) -> PortableKnowledgeEntry {
    let mut portable = PortableKnowledgeEntry {
        id: format!("module:{book_id}:{}", entry.id.as_str()),
        name: entry.name.clone(),
        content: entry.content.clone(),
        enabled: entry.enabled,
        priority: entry.priority,
        placement: match entry.placement {
            KnowledgePlacement::RetrievedContext => PortableKnowledgePlacement::RetrievedContext,
            KnowledgePlacement::BeforeOlderHistory => {
                PortableKnowledgePlacement::BeforeOlderHistory
            }
            KnowledgePlacement::BeforeRecentHistory => {
                PortableKnowledgePlacement::BeforeRecentHistory
            }
            KnowledgePlacement::PostHistory => PortableKnowledgePlacement::PostHistory,
        },
        parent_id: entry.parent_id.as_ref().map(|id| id.as_str().to_owned()),
        probability_basis_points: entry.activation_probability_basis_points,
        ..PortableKnowledgeEntry::default()
    };
    match &entry.activation {
        ActivationRule::Always => portable.constant = true,
        ActivationRule::Keyword {
            primary,
            secondary,
            selective,
            case_sensitive,
            whole_word,
        } => {
            portable.primary_keys.clone_from(primary);
            portable.secondary_keys.clone_from(secondary);
            portable.selective = *selective;
            portable.case_sensitive = *case_sensitive;
            portable.whole_word = *whole_word;
        }
        ActivationRule::Regex { patterns } => {
            portable.primary_keys = patterns
                .iter()
                .map(|pattern| pattern.pattern.clone())
                .collect();
            portable.case_sensitive = patterns.iter().all(|pattern| !pattern.case_insensitive);
            portable.use_regex = true;
        }
        ActivationRule::Manual
        | ActivationRule::Semantic { .. }
        | ActivationRule::Condition { .. }
        | ActivationRule::Any { .. }
        | ActivationRule::All { .. } => portable.enabled = false,
    }
    portable
}

fn approved_module_source_hash(
    approved: &AppliedModuleRuntimePlan,
    binding: &lorepia_domain::ModuleBinding,
) -> Option<lorepia_domain::Sha256Digest> {
    approved
        .plan
        .components
        .iter()
        .flat_map(|component| {
            std::iter::once(&component.selected_source).chain(component.coalesced_sources.iter())
        })
        .find(|source| {
            source.binding_id == binding.id
                && source.module_id == binding.module_id
                && source.revision_id == binding.revision_id
        })
        .map(|source| source.revision_source_sha256.clone())
        .or_else(|| {
            approved
                .review
                .import_approvals
                .iter()
                .find(|reviewed| reviewed.binding_id == binding.id)
                .map(|reviewed| reviewed.evidence.module_revision_source_sha256.clone())
        })
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
