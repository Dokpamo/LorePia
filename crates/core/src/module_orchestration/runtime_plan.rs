use std::collections::BTreeMap;

use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, ModuleBinding,
    ModuleRevisionResolutionMode, SourceKind,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, ModuleActivationReview, ModuleResolutionContext,
    ModuleRevisionSnapshot, review_module_merge,
};
use lorepia_storage::{
    ActiveContentModuleRevision, ModuleRevisionComponentSnapshot, StoredRevision,
};

use super::{
    ApprovedContentModuleComponent, ContentModuleRuntimeBindingDisposition,
    ContentModuleRuntimeBindingSummary, SUPPORTED_CONTENT_CAPABILITIES, insert_revision_snapshot,
    module_import_approval_evidence, module_merge_error, module_snapshot,
    validate_module_binding_variables,
};
use crate::Core;

pub(super) struct PreparedModuleRuntimeReview {
    pub(super) review: ModuleActivationReview,
    pub(super) bindings: Vec<(StoredRevision<ModuleBinding>, ModuleBinding)>,
}

impl Core {
    /// Re-derives the current effective module stack for trusted runtime code
    /// and loads only an exact-context applied plan.
    ///
    /// A missing, stale, or differently resolved plan fails closed.
    pub(crate) fn resolve_applied_content_module_runtime_plan(
        &self,
        context: &ModuleResolutionContext,
    ) -> CoreResult<AppliedModuleRuntimePlan> {
        let current_review = self.review_current_content_module_runtime(context)?;
        self.storage()
            .get_applied_module_runtime_plan(&current_review)
    }

    /// Builds the exact module context for a proposed branch without requiring
    /// that branch row to exist yet.
    ///
    /// The generation-action caller remains responsible for validating the
    /// source conversation/branch and deterministic proposed branch identity
    /// before using this context in an atomic branch append.
    pub(crate) fn content_module_context_for_proposed_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ModuleResolutionContext> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let persona_id = self
            .storage()
            .get_conversation_persona_selection(conversation_id)?
            .map(|selection| selection.value.persona_id);
        Ok(ModuleResolutionContext {
            local_user_id: self.storage().load_settings()?.local_user_id,
            persona_id,
            character_id: Some(conversation.character_id),
            conversation_id: Some(conversation_id.0.clone()),
            branch_id: Some(branch_id.0.clone()),
            supported_capabilities: SUPPORTED_CONTENT_CAPABILITIES.to_vec(),
        })
    }

    /// Loads one immutable child revision named by an already verified applied
    /// plan. The parent revision source hash and component hash are rechecked
    /// by storage before any runtime overlay is returned.
    pub(crate) fn load_approved_content_module_component(
        &self,
        approved: &ApprovedContentModuleComponent,
    ) -> CoreResult<ModuleRevisionComponentSnapshot> {
        self.storage().get_module_revision_component(
            &approved.selected_source,
            &approved.component,
            &approved.component_sha256,
        )
    }

    pub(crate) fn review_current_content_module_runtime(
        &self,
        context: &ModuleResolutionContext,
    ) -> CoreResult<ModuleActivationReview> {
        self.prepare_current_content_module_runtime(context)
            .map(|prepared| prepared.review)
    }

    pub(super) fn prepare_current_content_module_runtime(
        &self,
        context: &ModuleResolutionContext,
    ) -> CoreResult<PreparedModuleRuntimeReview> {
        let stored_bindings = self.storage().list_all_module_bindings()?;
        let mut snapshots = BTreeMap::new();
        let mut bindings = Vec::with_capacity(stored_bindings.len());
        let mut resolved_bindings = Vec::with_capacity(stored_bindings.len());
        for stored in stored_bindings {
            let (binding, snapshot) = self.resolve_module_binding(&stored.value)?;
            insert_revision_snapshot(&mut snapshots, snapshot)?;
            bindings.push(binding.clone());
            resolved_bindings.push((stored, binding));
        }
        // Runtime review has no single mutable binding CAS target. Zero is a
        // stable sentinel; the full binding/revision payload remains committed
        // by the review hash and storage independently re-derives it.
        let review = review_module_merge(
            0,
            context,
            &bindings,
            &snapshots.into_values().collect::<Vec<_>>(),
        )
        .map_err(module_merge_error)?;
        Ok(PreparedModuleRuntimeReview {
            review,
            bindings: resolved_bindings,
        })
    }

    pub(super) fn resolve_module_binding(
        &self,
        binding: &ModuleBinding,
    ) -> CoreResult<(ModuleBinding, ModuleRevisionSnapshot)> {
        validate_module_binding_variables(binding)?;
        let stored = match binding.resolution_mode {
            ModuleRevisionResolutionMode::Active => self
                .storage()
                .get_active_content_module_revision(&binding.module_id)?,
            ModuleRevisionResolutionMode::Pinned => {
                let pinned = binding.pinned_revision_id.as_ref().ok_or_else(|| {
                    CoreError::invalid("stored pinned module binding has no revision id")
                })?;
                self.storage()
                    .get_content_module_revision(&binding.module_id, pinned)?
            }
        };
        let revision_advanced = binding.revision_id != stored.module_revision.id;
        let mut resolved = binding.clone();
        resolved.revision_id = stored.module_revision.id.clone();
        let snapshot = if revision_advanced {
            // An active-resolution binding is approved for one immutable
            // revision only. Keep the durable approval tuple untouched, but
            // make this in-memory projection awaiting approval so the newer
            // revision cannot enter a runtime plan before an explicit review.
            resolved.approved = false;
            resolved.activation_approval_id = None;
            resolved.activation_review_sha256 = None;
            resolved.activation_plan_sha256 = None;
            module_snapshot(stored)
        } else {
            self.module_snapshot_for_binding(stored, binding.package_import_approval_id.as_deref())?
        };
        Ok((resolved, snapshot))
    }

    pub(super) fn module_snapshot_for_binding(
        &self,
        stored: ActiveContentModuleRevision,
        package_import_approval_id: Option<&str>,
    ) -> CoreResult<ModuleRevisionSnapshot> {
        let imported_package =
            stored.object.value.metadata.provenance.source_kind == SourceKind::ImportedPackage;
        let import_approval = match (imported_package, package_import_approval_id) {
            (false, None) => None,
            (false, Some(_)) => {
                return Err(CoreError::invalid(
                    "non-package content module cannot carry a package import approval",
                ));
            }
            (true, None) => {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "imported package module requires a completed package approval",
                    false,
                ));
            }
            (true, Some(approval_id)) => {
                let authority = self.get_completed_content_package_authority(approval_id)?;
                Some(module_import_approval_evidence(&stored, &authority)?)
            }
        };
        Ok(ModuleRevisionSnapshot {
            module: stored.object.value,
            revision: stored.module_revision,
            import_approval,
        })
    }
}

pub(super) fn content_module_runtime_binding_summary(
    stored: StoredRevision<ModuleBinding>,
    binding: ModuleBinding,
    disposition: ContentModuleRuntimeBindingDisposition,
) -> ContentModuleRuntimeBindingSummary {
    let approved_revision_id = stored.value.revision_id.clone();
    ContentModuleRuntimeBindingSummary {
        binding,
        approved_revision_id,
        state_revision: stored.revision,
        updated_at: stored.updated_at,
        disposition,
    }
}
