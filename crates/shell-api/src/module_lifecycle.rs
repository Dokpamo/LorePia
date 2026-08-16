//! Hash-bound content-module activation and rollback for the webview.
//!
//! The caller may submit only an inert binding draft, exact review/plan hash
//! echoes, explicit conflict choices, and a caller-stable approval id. Core
//! recreates every review and performs the durable compare-and-swap. This
//! module rejects oversized review surfaces instead of truncating authoritative
//! candidate sets.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_core::{
    ApprovedContentModuleComponent, ContentModuleActivationReceipt,
    ContentModuleActivationReceiptPreflight, ContentModuleActivationRequest,
    ContentModuleActivationReviewPresentation, ContentModuleActivationRevisionReview,
    ContentModuleDeactivationReceipt, ContentModuleDeactivationRequest,
    ContentModuleDeactivationReview, ContentModuleImportApprovalCandidate,
    ContentModuleRollbackApplyRequest, ContentModuleRollbackPlan,
    ContentModuleRollbackResolutionRequest, ContentModuleRollbackReviewPresentation,
    ContentModuleRuntimeBindingDisposition, ContentModuleRuntimeBindingSummary,
    ContentModuleRuntimeTarget, CoreError, CoreErrorCode, ModuleActivationApproval,
    ModuleActivationPlan, ModuleBinding, ModuleBindingId, ModuleMergeResolutionSet,
    ModuleResolutionContext, ModuleRevisionId, ModuleRevisionResolutionMode, ModuleScope,
    Sha256Digest, SourceKind,
};
use serde::{Deserialize, Serialize};

use crate::{ShellApi, ShellError, ShellResult, api::validate_identifier};

const MAX_LIFECYCLE_DOCUMENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_LIFECYCLE_BINDINGS: usize = 256;
const MAX_LIFECYCLE_COMPONENTS: usize = 512;
const MAX_LIFECYCLE_CANDIDATES_PER_COMPONENT: usize = 64;
const MAX_LIFECYCLE_SOURCES_PER_CANDIDATE: usize = 256;
const MAX_LIFECYCLE_IMPORT_APPROVALS: usize = 64;
const MAX_LIFECYCLE_COMPONENT_AUTHORITIES: usize = 512;
const MAX_LIFECYCLE_VARIABLE_OVERRIDES: usize = 512;
const MAX_ACTIVATION_APPROVAL_ID_BYTES: usize = 256;
const MAX_LIFECYCLE_MODULES: usize = 100;
const MAX_LIFECYCLE_REVISIONS: usize = 100;
const MAX_JAVASCRIPT_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_UTC_RFC3339_TIMESTAMP: &str = "9999-12-31T23:59:59.999999999Z";

#[derive(Serialize)]
struct PreflightBindingProjection<'a> {
    binding: &'a ModuleBinding,
    state_revision: u64,
    updated_at: &'static str,
}

#[derive(Serialize)]
struct PreflightDeactivationReceipt<'a> {
    verified: bool,
    review: &'a ContentModuleDeactivationReview,
    binding: PreflightBindingProjection<'a>,
    deleted_at: &'static str,
}

#[derive(Serialize)]
struct PreflightReceiptProjection<'a> {
    verified: bool,
    binding: PreflightBindingProjection<'a>,
    approval_id: &'a str,
    approval_sha256: &'a str,
    review_sha256: &'a str,
    plan_sha256: &'a str,
    approved_plan: &'a ModuleActivationPlan,
    approved_components: &'a [ApprovedContentModuleComponent],
}

#[derive(Serialize)]
struct PreflightStoredBinding<'a> {
    value: &'a ModuleBinding,
    revision: u64,
    revision_id: Option<&'a str>,
    created_at: &'static str,
    updated_at: &'static str,
    deleted_at: Option<&'a str>,
}

#[derive(Serialize)]
struct PreflightCoreReceipt<'a> {
    binding: PreflightStoredBinding<'a>,
    approved_plan: &'a lorepia_core::ApprovedModuleActivationPlan,
    approved_components: &'a [ApprovedContentModuleComponent],
}

pub type ContentModuleActivationReviewDto = ContentModuleActivationReviewPresentation;
pub type ContentModuleActivationPlanDto = ModuleActivationPlan;
pub type ContentModuleRollbackReviewDto = ContentModuleRollbackReviewPresentation;
pub type ContentModuleRollbackPlanDto = ContentModuleRollbackPlan;
pub type ContentModuleDeactivationReviewDto = ContentModuleDeactivationReview;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewContentModuleActivationInput {
    pub activation: ContentModuleActivationRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolveContentModuleActivationInput {
    pub activation: ContentModuleActivationRequest,
    pub resolutions: ModuleMergeResolutionSet,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivateContentModuleInput {
    pub activation: ContentModuleActivationRequest,
    pub resolutions: ModuleMergeResolutionSet,
    pub approval: ModuleActivationApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewContentModuleDeactivationInput {
    pub deactivation: ContentModuleDeactivationRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeactivateContentModuleInput {
    pub deactivation: ContentModuleDeactivationRequest,
    pub expected_review_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewContentModuleRollbackInput {
    pub binding_id: ModuleBindingId,
    pub target_revision_id: ModuleRevisionId,
    pub target_package_import_approval_id: Option<String>,
    pub runtime_target: lorepia_core::ContentModuleRuntimeTarget,
}

pub type ResolveContentModuleRollbackInput = ContentModuleRollbackResolutionRequest;
pub type ApplyContentModuleRollbackInput = ContentModuleRollbackApplyRequest;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListContentModuleLifecycleCandidatesInput {
    pub runtime_target: ContentModuleRuntimeTarget,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListContentModuleLifecycleBindingsInput {
    pub runtime_target: ContentModuleRuntimeTarget,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleScopeTargetDto {
    pub scope: ModuleScope,
    pub target_id: Option<String>,
    pub conversation_id: Option<String>,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleImportApprovalCandidateDto {
    pub approval_id: String,
    pub approval_sha256: String,
    pub import_id: String,
    pub import_revision: u64,
    pub package_id: String,
    pub package_source_sha256: String,
    pub selection_sha256: String,
    pub capability_review_sha256: String,
    pub module_id: String,
    pub module_revision_id: String,
    pub module_revision_source_sha256: String,
}

impl TryFrom<ContentModuleImportApprovalCandidate> for ContentModuleImportApprovalCandidateDto {
    type Error = ShellError;

    fn try_from(value: ContentModuleImportApprovalCandidate) -> Result<Self, Self::Error> {
        validate_javascript_safe_integer("package import revision", value.import_revision)?;
        Ok(Self {
            approval_id: value.package_import_approval_id,
            approval_sha256: value.approval_sha256.into_inner(),
            import_id: value.import_id,
            import_revision: value.import_revision,
            package_id: value.package_id.0,
            package_source_sha256: value.package_source_sha256.into_inner(),
            selection_sha256: value.selection_sha256.into_inner(),
            capability_review_sha256: value.capability_review_sha256.into_inner(),
            module_id: value.module_id.0,
            module_revision_id: value.module_revision_id.0,
            module_revision_source_sha256: value.module_revision_source_sha256.into_inner(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleLifecycleCandidateDto {
    pub module_id: String,
    pub revision_id: String,
    pub revision_source_sha256: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub license: String,
    pub redistribution_allowed: bool,
    pub required_capabilities: Vec<lorepia_core::ContentCapability>,
    pub source_kind: SourceKind,
    pub local_use_allowed: bool,
    pub sharing_allowed: bool,
    pub share_reasons: Vec<String>,
    pub component_count: u32,
    pub completed_package_approvals: Vec<ContentModuleImportApprovalCandidateDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleLifecycleCandidatesDto {
    pub items: Vec<ContentModuleLifecycleCandidateDto>,
    pub truncated: bool,
    pub scope_targets: Vec<ContentModuleScopeTargetDto>,
}

/// Reader-safe durable binding state used after app restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleLifecycleBindingDto {
    pub binding: ModuleBinding,
    /// Compare-and-swap revision of the durable binding row.
    pub state_revision: u64,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleLifecycleRevisionDto {
    pub revision_id: String,
    pub name: String,
    pub version: String,
    pub source_sha256: String,
    pub source_kind: SourceKind,
    pub previous_revision_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub active: bool,
    pub rollback_allowed: bool,
    pub completed_package_approvals: Vec<ContentModuleImportApprovalCandidateDto>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleLifecycleBindingItemDto {
    pub binding: ContentModuleLifecycleBindingDto,
    pub approved_revision_id: String,
    pub disposition: ContentModuleRuntimeBindingDisposition,
    pub module_name: String,
    pub revision_source_sha256: String,
    pub revisions: Vec<ContentModuleLifecycleRevisionDto>,
    pub revisions_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleLifecycleBindingsDto {
    pub items: Vec<ContentModuleLifecycleBindingItemDto>,
    pub truncated: bool,
    pub workspace_review_sha256: String,
    pub workspace_state_revision: u64,
}

/// Receipt-only success projection. `verified` is emitted only after the
/// independently verifiable Core receipt has passed `verify()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationReceiptDto {
    pub verified: bool,
    pub binding: ContentModuleLifecycleBindingDto,
    pub approval_id: String,
    pub approval_sha256: String,
    pub review_sha256: String,
    pub plan_sha256: String,
    pub approved_plan: ModuleActivationPlan,
    pub approved_components: Vec<ApprovedContentModuleComponent>,
}

/// Receipt-only deactivation success. `verified` is emitted only after Core's
/// review hash and deletion CAS receipt have both passed independent checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleDeactivationReceiptDto {
    pub verified: bool,
    pub review: ContentModuleDeactivationReviewDto,
    pub binding: ContentModuleLifecycleBindingDto,
    pub deleted_at: DateTime<Utc>,
}

impl ShellApi {
    pub fn review_content_module_activation(
        &self,
        input: ReviewContentModuleActivationInput,
    ) -> ShellResult<ContentModuleActivationReviewDto> {
        validate_activation_request(&input.activation)?;
        let presentation = self
            .core
            .review_content_module_activation_presentation(&input.activation)
            .map_err(ShellError::from)?;
        presentation.review.verify().map_err(|error| {
            invalid_lifecycle(format!("Core returned an invalid review: {error}"))
        })?;
        validate_activation_review(&presentation)?;
        Ok(presentation)
    }

    pub fn resolve_content_module_activation(
        &self,
        input: ResolveContentModuleActivationInput,
    ) -> ShellResult<ContentModuleActivationPlanDto> {
        validate_activation_request(&input.activation)?;
        validate_resolution_set(&input.resolutions)?;
        let plan = self
            .core
            .resolve_content_module_activation(&input.activation, &input.resolutions)
            .map_err(ShellError::from)?;
        plan.verify().map_err(|error| {
            invalid_lifecycle(format!("Core returned an invalid plan: {error}"))
        })?;
        validate_activation_plan(&plan)?;
        Ok(plan)
    }

    pub fn activate_content_module(
        &self,
        input: ActivateContentModuleInput,
    ) -> ShellResult<ContentModuleActivationReceiptDto> {
        validate_activation_request(&input.activation)?;
        validate_resolution_set(&input.resolutions)?;
        validate_approval(&input.approval)?;
        let preflight = self
            .core
            .preflight_content_module_activation(
                &input.activation,
                &input.resolutions,
                &input.approval,
            )
            .map_err(ShellError::from)?;
        validate_activation_receipt_preflight(&preflight)?;
        let receipt = self
            .core
            .activate_content_module(&input.activation, &input.resolutions, &input.approval)
            .map_err(ShellError::from)?;
        receipt.verify().map_err(|error| {
            invalid_lifecycle(format!(
                "Core returned an invalid activation receipt: {error}"
            ))
        })?;
        project_activation_receipt(receipt)
    }

    pub fn review_content_module_deactivation(
        &self,
        input: ReviewContentModuleDeactivationInput,
    ) -> ShellResult<ContentModuleDeactivationReviewDto> {
        validate_deactivation_request(&input.deactivation)?;
        let review = self
            .core
            .review_content_module_deactivation(&input.deactivation)
            .map_err(ShellError::from)?;
        review.verify().map_err(|error| {
            invalid_lifecycle(format!(
                "Core returned an invalid content module deactivation review: {error}"
            ))
        })?;
        validate_deactivation_review(&review)?;
        Ok(review)
    }

    pub fn deactivate_content_module(
        &self,
        input: DeactivateContentModuleInput,
    ) -> ShellResult<ContentModuleDeactivationReceiptDto> {
        validate_deactivation_request(&input.deactivation)?;
        validate_serialized("content module deactivation request", &input)?;
        let preflight = self
            .core
            .review_content_module_deactivation(&input.deactivation)
            .map_err(ShellError::from)?;
        preflight.verify().map_err(|error| {
            invalid_lifecycle(format!(
                "Core returned an invalid content module deactivation preflight: {error}"
            ))
        })?;
        if preflight.review_sha256 != input.expected_review_sha256 {
            return Err(invalid_lifecycle(
                "content module deactivation review is stale",
            ));
        }
        let durable_binding = self
            .core
            .list_content_module_bindings(&preflight.binding.module_id)
            .map_err(ShellError::from)?
            .into_iter()
            .find(|stored| stored.value.id == preflight.binding.id)
            .ok_or_else(|| {
                storage_corrupted(
                    "Core deactivation preflight binding has no durable binding record",
                )
            })?;
        if durable_binding.revision != preflight.expected_binding_revision
            || durable_binding.deleted_at.is_some()
        {
            return Err(invalid_lifecycle(
                "content module deactivation binding changed during preflight",
            ));
        }
        validate_deactivation_receipt_preflight(&preflight, &durable_binding.value)?;
        let receipt = self
            .core
            .deactivate_content_module(&input.deactivation, &input.expected_review_sha256)
            .map_err(ShellError::from)?;
        receipt.verify().map_err(|error| {
            invalid_lifecycle(format!(
                "Core returned an invalid content module deactivation receipt: {error}"
            ))
        })?;
        project_deactivation_receipt(receipt)
    }

    pub fn review_content_module_rollback(
        &self,
        input: ReviewContentModuleRollbackInput,
    ) -> ShellResult<ContentModuleRollbackReviewDto> {
        validate_identifier("binding_id", input.binding_id.as_str())?;
        validate_identifier("target_revision_id", input.target_revision_id.as_str())?;
        if let Some(approval_id) = input.target_package_import_approval_id.as_deref() {
            validate_identifier("target_package_import_approval_id", approval_id)?;
        }
        validate_runtime_target(&input.runtime_target)?;
        let presentation = self
            .core
            .review_content_module_rollback_presentation(
                &input.binding_id,
                &input.target_revision_id,
                input.target_package_import_approval_id.as_deref(),
                &input.runtime_target,
            )
            .map_err(ShellError::from)?;
        presentation.review.rollback.verify().map_err(|error| {
            invalid_lifecycle(format!("Core returned an invalid rollback review: {error}"))
        })?;
        presentation.review.activation.verify().map_err(|error| {
            invalid_lifecycle(format!(
                "Core returned an invalid rollback activation review: {error}"
            ))
        })?;
        validate_rollback_review(&presentation)?;
        Ok(presentation)
    }

    pub fn resolve_content_module_rollback(
        &self,
        input: ResolveContentModuleRollbackInput,
    ) -> ShellResult<ContentModuleRollbackPlanDto> {
        validate_rollback_resolution(&input)?;
        let plan = self
            .core
            .resolve_content_module_rollback(&input)
            .map_err(ShellError::from)?;
        plan.verify().map_err(|error| {
            invalid_lifecycle(format!("Core returned an invalid rollback plan: {error}"))
        })?;
        validate_rollback_plan(&plan)?;
        Ok(plan)
    }

    pub fn apply_content_module_rollback(
        &self,
        input: ApplyContentModuleRollbackInput,
    ) -> ShellResult<ContentModuleActivationReceiptDto> {
        validate_rollback_resolution(&input.resolution)?;
        validate_approval(&input.activation_approval)?;
        validate_serialized("rollback approval", &input)?;
        let preflight = self
            .core
            .preflight_content_module_rollback(&input)
            .map_err(ShellError::from)?;
        validate_activation_receipt_preflight(&preflight)?;
        let receipt = self
            .core
            .apply_content_module_rollback(&input)
            .map_err(ShellError::from)?;
        receipt.verify().map_err(|error| {
            invalid_lifecycle(format!(
                "Core returned an invalid rollback receipt: {error}"
            ))
        })?;
        project_activation_receipt(receipt)
    }

    pub fn list_content_module_lifecycle_candidates(
        &self,
        input: ListContentModuleLifecycleCandidatesInput,
    ) -> ShellResult<ContentModuleLifecycleCandidatesDto> {
        validate_runtime_target(&input.runtime_target)?;
        let limit = validate_list_limit(input.limit, MAX_LIFECYCLE_MODULES, "module candidate")?;
        let workspace = self
            .core
            .review_content_module_runtime_workspace(&input.runtime_target)
            .map_err(ShellError::from)?;
        validate_javascript_safe_integer(
            "module workspace state revision",
            workspace.state_revision,
        )?;
        let scope_targets = scope_targets(&workspace.context)?;
        let modules = self.core.list_content_modules().map_err(ShellError::from)?;
        let mut modules = modules
            .into_iter()
            .filter(|stored| {
                stored.value.metadata.provenance.source_kind != SourceKind::ApplicationBuiltIn
            })
            .collect::<Vec<_>>();
        modules.sort_by(|left, right| left.value.id.as_str().cmp(right.value.id.as_str()));
        let truncated = modules.len() > limit;
        let mut items = Vec::with_capacity(modules.len().min(limit));
        for stored in modules.into_iter().take(limit) {
            let module_id = stored.value.id.clone();
            let revision = self
                .core
                .list_content_module_revision_summaries(&module_id, 1)
                .map_err(ShellError::from)?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    storage_corrupted("active content module has no immutable revision")
                })?;
            if !revision.active
                || stored.revision_id.as_deref() != Some(revision.revision_id.as_str())
            {
                return Err(storage_corrupted(
                    "content module state and active immutable revision disagree",
                ));
            }
            let share = self
                .core
                .evaluate_content_module_share_gate(&module_id)
                .map_err(ShellError::from)?;
            let completed_package_approvals =
                if stored.value.metadata.provenance.source_kind == SourceKind::ImportedPackage {
                    self.core
                        .list_content_module_import_approval_candidates(
                            &module_id,
                            ModuleRevisionResolutionMode::Active,
                            None,
                            MAX_LIFECYCLE_IMPORT_APPROVALS,
                        )
                        .map_err(ShellError::from)?
                        .into_iter()
                        .map(ContentModuleImportApprovalCandidateDto::try_from)
                        .collect::<ShellResult<Vec<_>>>()?
                } else {
                    Vec::new()
                };
            let component_count = module_component_count(&stored.value)?;
            items.push(ContentModuleLifecycleCandidateDto {
                module_id: module_id.0,
                revision_id: revision.revision_id.0,
                revision_source_sha256: revision.source_sha256.into_inner(),
                name: stored.value.name,
                version: stored.value.version,
                author: stored.value.metadata.author,
                license: stored.value.metadata.license,
                redistribution_allowed: stored.value.metadata.redistribution_allowed,
                required_capabilities: stored.value.required_capabilities,
                source_kind: stored.value.metadata.provenance.source_kind,
                local_use_allowed: share.local_use_allowed,
                sharing_allowed: share.sharing_allowed,
                share_reasons: share.reasons,
                component_count,
                completed_package_approvals,
            });
        }
        let result = ContentModuleLifecycleCandidatesDto {
            items,
            truncated,
            scope_targets,
        };
        validate_serialized("content module candidate workspace", &result)?;
        Ok(result)
    }

    pub fn list_content_module_lifecycle_bindings(
        &self,
        input: ListContentModuleLifecycleBindingsInput,
    ) -> ShellResult<ContentModuleLifecycleBindingsDto> {
        validate_runtime_target(&input.runtime_target)?;
        let limit = validate_list_limit(input.limit, MAX_LIFECYCLE_BINDINGS, "module binding")?;
        let workspace = self
            .core
            .review_content_module_runtime_workspace(&input.runtime_target)
            .map_err(ShellError::from)?;
        validate_javascript_safe_integer(
            "module workspace state revision",
            workspace.state_revision,
        )?;
        let truncated = workspace.bindings.len() > limit;
        let mut items = Vec::with_capacity(workspace.bindings.len().min(limit));
        for summary in workspace.bindings.into_iter().take(limit) {
            items.push(self.project_content_module_lifecycle_binding(summary)?);
        }
        let result = ContentModuleLifecycleBindingsDto {
            items,
            truncated,
            workspace_review_sha256: workspace.review_sha256.into_inner(),
            workspace_state_revision: workspace.state_revision,
        };
        validate_serialized("content module binding workspace", &result)?;
        Ok(result)
    }

    fn project_content_module_lifecycle_binding(
        &self,
        summary: ContentModuleRuntimeBindingSummary,
    ) -> ShellResult<ContentModuleLifecycleBindingItemDto> {
        validate_javascript_safe_integer("module binding state revision", summary.state_revision)?;
        let module_id = summary.binding.module_id.clone();
        let module = self
            .core
            .get_content_module(&module_id)
            .map_err(ShellError::from)?;
        let all_revision_count = self
            .core
            .list_content_module_revisions(&module_id)
            .map_err(ShellError::from)?
            .len();
        let mut revisions = self
            .core
            .list_content_module_revision_summaries(&module_id, MAX_LIFECYCLE_REVISIONS)
            .map_err(ShellError::from)?;
        include_binding_revisions(self, &module_id, &summary, &mut revisions)?;
        let by_id = revisions
            .iter()
            .map(|revision| (revision.revision_id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        let resolved = by_id.get(&summary.binding.revision_id).ok_or_else(|| {
            storage_corrupted(
                "resolved module binding revision is outside the bounded revision history",
            )
        })?;
        let resolved_source_sha256 = resolved.source_sha256.to_string();
        let approved_module_name = by_id.get(&summary.approved_revision_id).map_or_else(
            || module.value.name.clone(),
            |revision| revision.name.clone(),
        );
        let rollback_ancestors =
            if summary.disposition == ContentModuleRuntimeBindingDisposition::NeedsReapproval {
                BTreeSet::new()
            } else {
                revision_ancestors(&by_id, &summary.approved_revision_id)?
            };
        let projected_revisions = revisions
            .into_iter()
            .map(|revision| {
                self.project_content_module_lifecycle_revision(
                    &module_id,
                    revision,
                    &rollback_ancestors,
                )
            })
            .collect::<ShellResult<Vec<_>>>()?;
        Ok(ContentModuleLifecycleBindingItemDto {
            binding: ContentModuleLifecycleBindingDto {
                binding: summary.binding,
                state_revision: summary.state_revision,
                updated_at: summary.updated_at,
            },
            approved_revision_id: summary.approved_revision_id.0,
            disposition: summary.disposition,
            module_name: approved_module_name,
            revision_source_sha256: resolved_source_sha256,
            revisions_truncated: all_revision_count > projected_revisions.len(),
            revisions: projected_revisions,
        })
    }

    fn project_content_module_lifecycle_revision(
        &self,
        module_id: &lorepia_core::ContentModuleId,
        revision: lorepia_core::ContentModuleRevisionSummary,
        rollback_ancestors: &BTreeSet<ModuleRevisionId>,
    ) -> ShellResult<ContentModuleLifecycleRevisionDto> {
        let completed_package_approvals = if revision.source_kind == SourceKind::ImportedPackage {
            self.core
                .list_content_module_import_approval_candidates(
                    module_id,
                    ModuleRevisionResolutionMode::Pinned,
                    Some(&revision.revision_id),
                    MAX_LIFECYCLE_IMPORT_APPROVALS,
                )
                .map_err(ShellError::from)?
                .into_iter()
                .map(ContentModuleImportApprovalCandidateDto::try_from)
                .collect::<ShellResult<Vec<_>>>()?
        } else {
            Vec::new()
        };
        Ok(ContentModuleLifecycleRevisionDto {
            rollback_allowed: rollback_ancestors.contains(&revision.revision_id),
            revision_id: revision.revision_id.0,
            name: revision.name,
            version: revision.version,
            source_sha256: revision.source_sha256.into_inner(),
            source_kind: revision.source_kind,
            previous_revision_id: revision.previous_revision_id.map(|value| value.0),
            created_at: revision.created_at,
            active: revision.active,
            completed_package_approvals,
        })
    }
}

fn include_binding_revisions(
    shell: &ShellApi,
    module_id: &lorepia_core::ContentModuleId,
    summary: &ContentModuleRuntimeBindingSummary,
    revisions: &mut Vec<lorepia_core::ContentModuleRevisionSummary>,
) -> ShellResult<()> {
    for exact_revision_id in [&summary.binding.revision_id, &summary.approved_revision_id] {
        if !revisions
            .iter()
            .any(|revision| &revision.revision_id == exact_revision_id)
        {
            revisions.push(
                shell
                    .core
                    .get_content_module_revision_summary(module_id, exact_revision_id)
                    .map_err(ShellError::from)?,
            );
        }
    }
    Ok(())
}

fn validate_list_limit(limit: u32, maximum: usize, kind: &str) -> ShellResult<usize> {
    let limit = usize::try_from(limit)
        .map_err(|_| invalid_lifecycle(format!("{kind} list limit is invalid")))?;
    if limit == 0 || limit > maximum {
        return Err(invalid_lifecycle(format!(
            "{kind} list limit must be between 1 and {maximum}"
        )));
    }
    Ok(limit)
}

fn validate_javascript_safe_integer(kind: &str, value: u64) -> ShellResult<()> {
    if value > MAX_JAVASCRIPT_SAFE_INTEGER {
        return Err(invalid_lifecycle(format!(
            "{kind} exceeds the exact JavaScript integer boundary"
        )));
    }
    Ok(())
}

fn scope_targets(
    context: &ModuleResolutionContext,
) -> ShellResult<Vec<ContentModuleScopeTargetDto>> {
    let conversation_id = context
        .conversation_id
        .as_deref()
        .ok_or_else(|| storage_corrupted("module runtime context has no conversation"))?;
    let branch_id = context
        .branch_id
        .as_deref()
        .ok_or_else(|| storage_corrupted("module runtime context has no branch"))?;
    validate_identifier("context_conversation_id", conversation_id)?;
    validate_identifier("context_branch_id", branch_id)?;

    let mut targets = vec![
        ContentModuleScopeTargetDto {
            scope: ModuleScope::App,
            target_id: None,
            conversation_id: None,
            label: "All chats on this device".to_owned(),
        },
        ContentModuleScopeTargetDto {
            scope: ModuleScope::User,
            target_id: None,
            conversation_id: None,
            label: "Current local user".to_owned(),
        },
    ];
    if let Some(persona_id) = context.persona_id.as_ref() {
        validate_identifier("context_persona_id", persona_id.as_str())?;
        targets.push(ContentModuleScopeTargetDto {
            scope: ModuleScope::Persona,
            target_id: Some(persona_id.as_str().to_owned()),
            conversation_id: None,
            label: "Selected persona".to_owned(),
        });
    }
    if let Some(character_id) = context.character_id.as_deref() {
        validate_identifier("context_character_id", character_id)?;
        targets.push(ContentModuleScopeTargetDto {
            scope: ModuleScope::Character,
            target_id: Some(character_id.to_owned()),
            conversation_id: None,
            label: "Current character".to_owned(),
        });
    }
    targets.push(ContentModuleScopeTargetDto {
        scope: ModuleScope::Conversation,
        target_id: Some(conversation_id.to_owned()),
        conversation_id: None,
        label: "Current conversation".to_owned(),
    });
    targets.push(ContentModuleScopeTargetDto {
        scope: ModuleScope::Branch,
        target_id: Some(branch_id.to_owned()),
        conversation_id: Some(conversation_id.to_owned()),
        label: "Current branch".to_owned(),
    });
    Ok(targets)
}

fn module_component_count(module: &lorepia_core::ContentModule) -> ShellResult<u32> {
    let count = module
        .prompt_fragments
        .len()
        .checked_add(module.knowledge_book_ids.len())
        .and_then(|value| value.checked_add(module.control_specs.len()))
        .and_then(|value| value.checked_add(module.transform_set_ids.len()))
        .and_then(|value| value.checked_add(module.interaction_rule_set_ids.len()))
        .and_then(|value| value.checked_add(module.asset_ids.len()))
        .ok_or_else(|| invalid_lifecycle("content module component count overflowed"))?;
    u32::try_from(count)
        .map_err(|_| invalid_lifecycle("content module component count is not bounded"))
}

fn revision_ancestors(
    by_id: &BTreeMap<ModuleRevisionId, &lorepia_core::ContentModuleRevisionSummary>,
    current_revision_id: &ModuleRevisionId,
) -> ShellResult<BTreeSet<ModuleRevisionId>> {
    let mut ancestors = BTreeSet::new();
    let mut cursor = current_revision_id.clone();
    while let Some(revision) = by_id.get(&cursor) {
        let Some(previous) = revision.previous_revision_id.as_ref() else {
            break;
        };
        if !ancestors.insert(previous.clone()) {
            return Err(storage_corrupted(
                "content module revision history contains a cycle",
            ));
        }
        cursor = previous.clone();
    }
    ancestors.remove(current_revision_id);
    Ok(ancestors)
}

fn validate_activation_request(request: &ContentModuleActivationRequest) -> ShellResult<()> {
    validate_runtime_target(&request.runtime_target)?;
    validate_identifier("binding_id", request.binding.id.as_str())?;
    validate_identifier("module_id", request.binding.module_id.as_str())?;
    if let Some(revision) = request.expected_binding_revision {
        validate_javascript_safe_integer("expected module binding revision", revision)?;
    }
    if let Some(target_id) = request.binding.target_id.as_deref() {
        validate_identifier("target_id", target_id)?;
    }
    if let Some(conversation_id) = request.binding.conversation_id.as_ref() {
        validate_identifier("binding_conversation_id", &conversation_id.0)?;
    }
    if let Some(revision_id) = request.binding.pinned_revision_id.as_ref() {
        validate_identifier("pinned_revision_id", revision_id.as_str())?;
    }
    if let Some(approval_id) = request.binding.package_import_approval_id.as_deref() {
        validate_identifier("package_import_approval_id", approval_id)?;
    }
    if request.binding.variable_overrides.values.len() > MAX_LIFECYCLE_VARIABLE_OVERRIDES {
        return Err(invalid_lifecycle(
            "module variable overrides exceed the bounded review surface",
        ));
    }
    validate_serialized("content module activation request", request)
}

fn validate_runtime_target(target: &lorepia_core::ContentModuleRuntimeTarget) -> ShellResult<()> {
    validate_identifier("conversation_id", &target.conversation_id.0)?;
    validate_identifier("branch_id", &target.branch_id.0)
}

fn validate_deactivation_request(request: &ContentModuleDeactivationRequest) -> ShellResult<()> {
    validate_runtime_target(&request.runtime_target)?;
    validate_identifier("binding_id", request.binding_id.as_str())?;
    validate_serialized("content module deactivation review request", request)
}

fn validate_deactivation_review(review: &ContentModuleDeactivationReview) -> ShellResult<()> {
    validate_runtime_target(&review.runtime_target)?;
    validate_identifier("binding_id", review.binding.id.as_str())?;
    validate_identifier("module_id", review.binding.module_id.as_str())?;
    validate_identifier("revision_id", review.binding.revision_id.as_str())?;
    validate_identifier("approved_revision_id", review.approved_revision_id.as_str())?;
    validate_javascript_safe_integer(
        "module deactivation expected binding revision",
        review.expected_binding_revision,
    )?;
    validate_serialized("content module deactivation review", review)
}

fn validate_deactivation_receipt_preflight(
    review: &ContentModuleDeactivationReview,
    durable_binding: &ModuleBinding,
) -> ShellResult<()> {
    review.verify().map_err(|error| {
        invalid_lifecycle(format!(
            "Core returned an invalid content module deactivation preflight: {error}"
        ))
    })?;
    let resulting_state_revision = review
        .expected_binding_revision
        .checked_add(1)
        .ok_or_else(|| invalid_lifecycle("module deactivation binding revision overflow"))?;
    validate_javascript_safe_integer(
        "module deactivation resulting state revision",
        resulting_state_revision,
    )?;
    if durable_binding != &review.binding
        || durable_binding.revision_id != review.approved_revision_id
    {
        return Err(storage_corrupted(
            "Core deactivation preflight disagrees with the durable binding",
        ));
    }

    validate_serialized(
        "content module deactivation receipt preflight projection",
        &PreflightDeactivationReceipt {
            verified: true,
            review,
            binding: PreflightBindingProjection {
                binding: durable_binding,
                state_revision: resulting_state_revision,
                updated_at: MAX_UTC_RFC3339_TIMESTAMP,
            },
            deleted_at: MAX_UTC_RFC3339_TIMESTAMP,
        },
    )
}

fn validate_resolution_set(resolutions: &ModuleMergeResolutionSet) -> ShellResult<()> {
    if resolutions.resolutions.len() > MAX_LIFECYCLE_COMPONENTS {
        return Err(invalid_lifecycle(
            "module conflict resolutions exceed the bounded review surface",
        ));
    }
    if resolutions.resolutions.iter().any(|resolution| {
        resolution.expected_candidates.len() > MAX_LIFECYCLE_CANDIDATES_PER_COMPONENT
    }) {
        return Err(invalid_lifecycle(
            "module conflict candidate set exceeds the bounded review surface",
        ));
    }
    validate_serialized("module conflict resolutions", resolutions)
}

fn validate_approval(approval: &ModuleActivationApproval) -> ShellResult<()> {
    validate_identifier("approval_id", &approval.approval_id)?;
    if approval.approval_id.len() > MAX_ACTIVATION_APPROVAL_ID_BYTES {
        return Err(invalid_lifecycle(
            "module approval id exceeds the bounded approval surface",
        ));
    }
    validate_serialized("module activation approval", approval)
}

fn validate_rollback_resolution(
    resolution: &ContentModuleRollbackResolutionRequest,
) -> ShellResult<()> {
    validate_runtime_target(&resolution.runtime_target)?;
    validate_identifier("binding_id", resolution.binding_id.as_str())?;
    validate_identifier("target_revision_id", resolution.target_revision_id.as_str())?;
    validate_javascript_safe_integer(
        "expected module rollback state revision",
        resolution.expected_state_revision,
    )?;
    if let Some(approval_id) = resolution.target_package_import_approval_id.as_deref() {
        validate_identifier("target_package_import_approval_id", approval_id)?;
    }
    validate_resolution_set(&resolution.resolutions)?;
    validate_serialized("module rollback resolution", resolution)
}

fn validate_activation_review(
    presentation: &ContentModuleActivationReviewPresentation,
) -> ShellResult<()> {
    validate_javascript_safe_integer(
        "module activation review state revision",
        presentation.review.state_revision,
    )?;
    validate_review_collections(&presentation.review)?;
    validate_proposed_import_authority(&presentation.review, &presentation.proposed_revision)?;
    validate_serialized("module activation review", presentation)
}

fn validate_rollback_review(
    presentation: &ContentModuleRollbackReviewPresentation,
) -> ShellResult<()> {
    validate_javascript_safe_integer(
        "module rollback review state revision",
        presentation.review.rollback.expected_state_revision,
    )?;
    validate_review_collections(&presentation.review.activation)?;
    validate_proposed_import_authority(
        &presentation.review.activation,
        &presentation.target_revision,
    )?;
    if let Some(diff) = presentation.review.rollback.diff.as_ref()
        && diff.component_changes.len() > MAX_LIFECYCLE_COMPONENTS
    {
        return Err(invalid_lifecycle(
            "module rollback diff exceeds the bounded review surface",
        ));
    }
    if presentation.review.rollback.blockers.len() > MAX_LIFECYCLE_COMPONENTS {
        return Err(invalid_lifecycle(
            "module rollback blockers exceed the bounded review surface",
        ));
    }
    validate_serialized("module rollback review", presentation)
}

fn validate_proposed_import_authority(
    review: &lorepia_core::ModuleActivationReview,
    revision: &ContentModuleActivationRevisionReview,
) -> ShellResult<()> {
    let [binding_id] = review.activation_binding_ids.as_slice() else {
        return Err(storage_corrupted(
            "Core returned a module review without exactly one proposed binding",
        ));
    };
    let binding = review
        .ordered_bindings
        .iter()
        .find(|binding| &binding.id == binding_id)
        .ok_or_else(|| {
            storage_corrupted("Core returned a module review without its proposed binding")
        })?;
    if binding.module_id != revision.module_id || binding.revision_id != revision.revision_id {
        return Err(storage_corrupted(
            "Core returned proposed module metadata for a different immutable revision",
        ));
    }
    let authorities = review
        .import_approvals
        .iter()
        .filter(|authority| authority.binding_id == *binding_id)
        .collect::<Vec<_>>();
    match revision.source_kind {
        SourceKind::ImportedPackage => {
            let Some(expected_approval_id) = binding.package_import_approval_id.as_deref() else {
                return Err(storage_corrupted(
                    "Core returned an imported proposed revision without package authority",
                ));
            };
            let [authority] = authorities.as_slice() else {
                return Err(storage_corrupted(
                    "Core returned an imported proposed revision without exactly one authority",
                ));
            };
            if authority.evidence.approval_id != expected_approval_id
                || authority.evidence.module_id != binding.module_id
                || authority.evidence.module_revision_id != binding.revision_id
                || authority.evidence.module_revision_source_sha256
                    != revision.revision_source_sha256
            {
                return Err(storage_corrupted(
                    "Core returned package authority for a different proposed revision",
                ));
            }
        }
        _ => {
            if binding.package_import_approval_id.is_some() || !authorities.is_empty() {
                return Err(storage_corrupted(
                    "Core returned package authority for a non-imported proposed revision",
                ));
            }
        }
    }
    Ok(())
}

fn validate_review_collections(review: &lorepia_core::ModuleActivationReview) -> ShellResult<()> {
    if review.ordered_bindings.len() > MAX_LIFECYCLE_BINDINGS
        || review.ignored_bindings.len() > MAX_LIFECYCLE_BINDINGS
        || review.components.len() > MAX_LIFECYCLE_COMPONENTS
        || review.conflicts.len() > MAX_LIFECYCLE_COMPONENTS
        || review.import_approvals.len() > MAX_LIFECYCLE_IMPORT_APPROVALS
        || review.effective_variable_overrides.values.len() > MAX_LIFECYCLE_VARIABLE_OVERRIDES
    {
        return Err(invalid_lifecycle(
            "module review exceeds the bounded lifecycle surface",
        ));
    }
    for component in &review.components {
        if component.candidates.len() > MAX_LIFECYCLE_CANDIDATES_PER_COMPONENT {
            return Err(invalid_lifecycle(
                "module review candidate set exceeds the bounded lifecycle surface",
            ));
        }
        if component
            .candidates
            .iter()
            .any(|candidate| candidate.sources.len() > MAX_LIFECYCLE_SOURCES_PER_CANDIDATE)
        {
            return Err(invalid_lifecycle(
                "module review candidate sources exceed the bounded lifecycle surface",
            ));
        }
    }
    if review
        .conflicts
        .iter()
        .any(|conflict| conflict.candidates.len() > MAX_LIFECYCLE_CANDIDATES_PER_COMPONENT)
    {
        return Err(invalid_lifecycle(
            "module conflict candidate set exceeds the bounded lifecycle surface",
        ));
    }
    if review.import_approvals.iter().any(|approval| {
        approval.evidence.component_authorities.len() > MAX_LIFECYCLE_COMPONENT_AUTHORITIES
            || approval.evidence.selected_package_component_ids.len()
                > MAX_LIFECYCLE_COMPONENT_AUTHORITIES
    }) {
        return Err(invalid_lifecycle(
            "module import authority exceeds the bounded lifecycle surface",
        ));
    }
    Ok(())
}

fn validate_activation_plan(plan: &ModuleActivationPlan) -> ShellResult<()> {
    validate_javascript_safe_integer(
        "module activation plan state revision",
        plan.expected_state_revision,
    )?;
    if plan.components.len() > MAX_LIFECYCLE_COMPONENTS
        || plan.omitted_components.len() > MAX_LIFECYCLE_COMPONENTS
        || plan.ordered_binding_ids.len() > MAX_LIFECYCLE_BINDINGS
        || plan.import_approvals.len() > MAX_LIFECYCLE_IMPORT_APPROVALS
        || plan.effective_variable_overrides.values.len() > MAX_LIFECYCLE_VARIABLE_OVERRIDES
    {
        return Err(invalid_lifecycle(
            "module activation plan exceeds the bounded lifecycle surface",
        ));
    }
    if plan
        .components
        .iter()
        .any(|component| component.coalesced_sources.len() > MAX_LIFECYCLE_SOURCES_PER_CANDIDATE)
    {
        return Err(invalid_lifecycle(
            "module activation plan sources exceed the bounded lifecycle surface",
        ));
    }
    validate_serialized("module activation plan", plan)
}

fn validate_rollback_plan(plan: &ContentModuleRollbackPlan) -> ShellResult<()> {
    validate_javascript_safe_integer(
        "module rollback plan state revision",
        plan.rollback.expected_state_revision,
    )?;
    validate_activation_plan(&plan.activation)?;
    validate_serialized("module rollback plan", plan)
}

fn project_activation_receipt(
    receipt: ContentModuleActivationReceipt,
) -> ShellResult<ContentModuleActivationReceiptDto> {
    validate_javascript_safe_integer(
        "module activation receipt revision",
        receipt.binding.revision,
    )?;
    validate_activation_plan(&receipt.approved_plan.plan)?;
    if receipt.approved_components.len() > MAX_LIFECYCLE_COMPONENTS {
        return Err(invalid_lifecycle(
            "module activation receipt exceeds the bounded lifecycle surface",
        ));
    }
    validate_serialized("module activation receipt", &receipt)?;
    let projected = ContentModuleActivationReceiptDto {
        verified: true,
        binding: ContentModuleLifecycleBindingDto {
            binding: receipt.binding.value,
            state_revision: receipt.binding.revision,
            updated_at: receipt.binding.updated_at,
        },
        approval_id: receipt.approved_plan.approval_id,
        approval_sha256: receipt.approved_plan.approval_sha256.into_inner(),
        review_sha256: receipt.approved_plan.plan.review_sha256.to_string(),
        plan_sha256: receipt.approved_plan.plan.plan_sha256.to_string(),
        approved_plan: receipt.approved_plan.plan,
        approved_components: receipt.approved_components,
    };
    validate_serialized("module activation receipt projection", &projected)?;
    Ok(projected)
}

fn validate_activation_receipt_preflight(
    preflight: &ContentModuleActivationReceiptPreflight,
) -> ShellResult<()> {
    preflight.verify().map_err(|error| {
        invalid_lifecycle(format!(
            "Core returned an invalid activation receipt preflight: {error}"
        ))
    })?;
    validate_javascript_safe_integer(
        "module activation resulting state revision",
        preflight.resulting_state_revision,
    )?;
    validate_activation_plan(&preflight.approved_plan.plan)?;
    if preflight.approved_components.len() > MAX_LIFECYCLE_COMPONENTS {
        return Err(invalid_lifecycle(
            "module activation receipt preflight exceeds the bounded lifecycle surface",
        ));
    }

    // The real projection has exactly these fields. Its only storage-authored
    // value absent from Core's read-only preflight is `updated_at`; use the
    // longest four-digit-year UTC RFC3339 representation so the later durable
    // receipt cannot cross the IPC byte bound merely because of its timestamp.
    let core_receipt = PreflightCoreReceipt {
        binding: PreflightStoredBinding {
            value: &preflight.binding,
            revision: preflight.resulting_state_revision,
            revision_id: None,
            created_at: MAX_UTC_RFC3339_TIMESTAMP,
            updated_at: MAX_UTC_RFC3339_TIMESTAMP,
            deleted_at: None,
        },
        approved_plan: &preflight.approved_plan,
        approved_components: &preflight.approved_components,
    };
    validate_serialized("module activation receipt preflight", &core_receipt)?;
    let projected = PreflightReceiptProjection {
        verified: true,
        binding: PreflightBindingProjection {
            binding: &preflight.binding,
            state_revision: preflight.resulting_state_revision,
            updated_at: MAX_UTC_RFC3339_TIMESTAMP,
        },
        approval_id: &preflight.approved_plan.approval_id,
        approval_sha256: preflight.approved_plan.approval_sha256.as_str(),
        review_sha256: preflight.approved_plan.plan.review_sha256.as_str(),
        plan_sha256: preflight.approved_plan.plan.plan_sha256.as_str(),
        approved_plan: &preflight.approved_plan.plan,
        approved_components: &preflight.approved_components,
    };
    validate_serialized("module activation receipt preflight projection", &projected)
}

fn project_deactivation_receipt(
    receipt: ContentModuleDeactivationReceipt,
) -> ShellResult<ContentModuleDeactivationReceiptDto> {
    validate_deactivation_review(&receipt.review)?;
    validate_javascript_safe_integer(
        "module deactivation receipt revision",
        receipt.binding.revision,
    )?;
    let deleted_at = receipt.binding.deleted_at.ok_or_else(|| {
        invalid_lifecycle("Core returned a content module deactivation receipt without deletion")
    })?;
    let projected = ContentModuleDeactivationReceiptDto {
        verified: true,
        review: receipt.review,
        binding: ContentModuleLifecycleBindingDto {
            binding: receipt.binding.value,
            state_revision: receipt.binding.revision,
            updated_at: receipt.binding.updated_at,
        },
        deleted_at,
    };
    validate_serialized("content module deactivation receipt projection", &projected)?;
    Ok(projected)
}

fn validate_serialized<T: Serialize>(kind: &str, value: &T) -> ShellResult<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| invalid_lifecycle(format!("failed to encode {kind}: {error}")))?;
    if bytes.len() > MAX_LIFECYCLE_DOCUMENT_BYTES {
        return Err(invalid_lifecycle(format!(
            "{kind} exceeds the bounded IPC document size"
        )));
    }
    Ok(())
}

fn invalid_lifecycle(message: impl Into<String>) -> ShellError {
    ShellError::from(CoreError::invalid(message.into()))
}

fn storage_corrupted(message: impl Into<String>) -> ShellError {
    ShellError::from(CoreError::new(
        CoreErrorCode::StorageCorrupted,
        message.into(),
        false,
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use lorepia_core::{
        BlockSource, ContentCapability, ContentModule, ContentModuleActivationRequest,
        ContentModuleBindingDraft, ContentModuleDeactivationRequest, ContentModuleId,
        ContentModuleRollbackResolutionRequest, ContentModuleRuntimeBindingDisposition,
        ContentModuleRuntimeTarget, CoreConfig, InstructionAuthority, MergePolicy,
        ModuleActivationApproval, ModuleBinding, ModuleBindingId, ModuleConflictResolution,
        ModuleMergeResolutionSet, ModuleRevisionId, ModuleRevisionResolutionMode, ModuleScope,
        OverflowPolicy, PackageMetadata, PlacementZone, PromptBlock, PromptBlockId,
        PromptBlockKind, Provenance, RoleHint, SafeTemplate, SourceKind, TemplatePart, TokenPolicy,
        VariableBinding, VariableId, VariableMap, VariableRef, VariableScope, VariableValue,
    };
    use tempfile::{NamedTempFile, tempdir};

    use super::{
        ActivateContentModuleInput, ApplyContentModuleRollbackInput,
        ContentModuleActivationReceiptDto, DeactivateContentModuleInput,
        ListContentModuleLifecycleBindingsInput, ListContentModuleLifecycleCandidatesInput,
        MAX_JAVASCRIPT_SAFE_INTEGER, MAX_LIFECYCLE_DOCUMENT_BYTES,
        ResolveContentModuleActivationInput, ReviewContentModuleActivationInput,
        ReviewContentModuleDeactivationInput, ReviewContentModuleRollbackInput, ShellApi,
        validate_activation_review,
    };

    struct OversizedLifecycleFixture {
        _root: tempfile::TempDir,
        shell: ShellApi,
        target: ContentModuleRuntimeTarget,
        module_id: ContentModuleId,
        rollback_target_revision_id: ModuleRevisionId,
        current_revision_id: ModuleRevisionId,
        activation: ActivateContentModuleInput,
        activation_binding: ModuleBinding,
        activation_request_bytes: usize,
        activation_review_bytes: usize,
        activation_approval_bytes: usize,
        activation_preflight_bytes: usize,
    }

    fn padded_identifier(prefix: &str, index: usize) -> String {
        let base = format!("{prefix}-{index:03}-");
        format!("{base}{}", "x".repeat(250 - base.len()))
    }

    fn oversized_variable_overrides(module_id: &ContentModuleId) -> VariableMap {
        VariableMap {
            values: (0..15)
                .map(|index| VariableBinding {
                    variable: VariableRef {
                        scope: VariableScope::Module,
                        namespace: Some(module_id.clone()),
                        id: VariableId::from(format!("receipt-bound-{index:03}")),
                    },
                    // Three-byte UTF-8 keeps each Core document below both its
                    // character and byte bounds while the combined receipt
                    // still crosses Shell's IPC byte limit.
                    value: VariableValue::Text("값".repeat(10_700)),
                })
                .collect(),
        }
    }

    fn oversized_lifecycle_fixture() -> OversizedLifecycleFixture {
        let (root, shell, target) = shell_and_target();
        let module_id_text = padded_identifier("synthetic-shell-bound-module", 0);
        let mut oversized = module(&module_id_text, "1.0.0", "7", Some("BOUND"));
        let prototype = oversized
            .prompt_fragments
            .pop()
            .expect("oversized prompt prototype");
        oversized.prompt_fragments = (0..380)
            .map(|index| {
                let mut block = prototype.clone();
                block.id = PromptBlockId::from(padded_identifier("receipt-block", index));
                block.name = format!("Bounded receipt block {index}");
                block.provenance.source_id = Some(block.id.as_str().to_owned());
                block
            })
            .collect();
        let first = shell
            .core
            .upsert_content_module(&oversized, None)
            .expect("save oversized receipt target v1");
        let rollback_target_revision_id = ModuleRevisionId::from(
            first
                .revision_id
                .expect("oversized receipt target v1 immutable revision"),
        );
        oversized.version = "2.0.0".to_owned();
        oversized.metadata.provenance.source_hash = Some("8".repeat(64));
        let second = shell
            .core
            .upsert_content_module(&oversized, Some(first.revision))
            .expect("save oversized receipt target v2");
        let current_revision_id = ModuleRevisionId::from(
            second
                .revision_id
                .expect("oversized receipt target v2 immutable revision"),
        );

        let mut request = activation(
            &module_id_text,
            &padded_identifier("synthetic-shell-bound-binding", 0),
            &target,
        );
        request.binding.scope = ModuleScope::Branch;
        request.binding.target_id = Some(target.branch_id.0.clone());
        request.binding.conversation_id = Some(target.conversation_id.clone());
        request.binding.variable_overrides = oversized_variable_overrides(&oversized.id);
        let review = shell
            .core
            .review_content_module_activation(&request)
            .expect("Core review accepts independently bounded activation material");
        let resolutions = empty_resolutions(review.review_sha256.clone());
        let plan = shell
            .core
            .resolve_content_module_activation(&request, &resolutions)
            .expect("Core resolves independently bounded activation material");
        let activation = ActivateContentModuleInput {
            activation: request,
            resolutions,
            approval: ModuleActivationApproval {
                approval_id: "synthetic-shell-oversized-receipt-approval".to_owned(),
                expected_review_sha256: review.review_sha256.clone(),
                expected_plan_sha256: plan.plan_sha256,
            },
        };
        let preflight = shell
            .core
            .preflight_content_module_activation(
                &activation.activation,
                &activation.resolutions,
                &activation.approval,
            )
            .expect("Core prepares valid oversized Shell receipt");

        OversizedLifecycleFixture {
            _root: root,
            shell,
            target,
            module_id: ContentModuleId::from(module_id_text),
            rollback_target_revision_id,
            current_revision_id,
            activation_request_bytes: serde_json::to_vec(&activation)
                .expect("activation input JSON")
                .len(),
            activation_review_bytes: serde_json::to_vec(&review)
                .expect("activation review JSON")
                .len(),
            activation_approval_bytes: serde_json::to_vec(&preflight.approved_plan)
                .expect("activation approval JSON")
                .len(),
            activation_preflight_bytes: serde_json::to_vec(&preflight)
                .expect("activation receipt preflight JSON")
                .len(),
            activation_binding: preflight.binding,
            activation,
        }
    }

    fn module(
        id: &str,
        version: &str,
        source_byte: &str,
        prompt_text: Option<&str>,
    ) -> ContentModule {
        let prompt_fragments = prompt_text
            .map(|text| {
                vec![PromptBlock {
                    id: PromptBlockId::from("synthetic.shell.shared-block"),
                    name: "Synthetic shared block".to_owned(),
                    kind: PromptBlockKind::StaticInstruction,
                    enabled: true,
                    role_hint: RoleHint::System,
                    authority: InstructionAuthority::Creator,
                    template: Some(SafeTemplate {
                        parts: vec![TemplatePart::Text {
                            value: text.to_owned(),
                        }],
                        max_output_chars: 256,
                    }),
                    condition: None,
                    source: BlockSource::Template,
                    placement_zone: PlacementZone::AssistantPrefill,
                    history_selector: None,
                    token_policy: TokenPolicy {
                        priority: 1_000,
                        min_tokens: None,
                        max_tokens: Some(64),
                        reserve_tokens: None,
                    },
                    overflow_policy: OverflowPolicy::Reject,
                    merge_policy: MergePolicy::SeparateMessage,
                    provenance: provenance(id, source_byte),
                }]
            })
            .unwrap_or_default();
        ContentModule {
            id: ContentModuleId::from(id),
            name: format!("Synthetic module {id}"),
            version: version.to_owned(),
            schema_version: 1,
            prompt_fragments,
            knowledge_book_ids: Vec::new(),
            control_specs: Vec::new(),
            transform_set_ids: Vec::new(),
            interaction_rule_set_ids: Vec::new(),
            asset_ids: Vec::new(),
            imported_components_enabled: false,
            required_capabilities: if prompt_text.is_some() {
                vec![ContentCapability::PromptFragments]
            } else {
                Vec::new()
            },
            metadata: PackageMetadata {
                author: Some("Synthetic Shell Test".to_owned()),
                license: "LicenseRef-Unknown".to_owned(),
                redistribution_allowed: false,
                homepage: None,
                description: "Project-owned synthetic lifecycle fixture".to_owned(),
                tags: vec!["synthetic".to_owned()],
                provenance: provenance(id, source_byte),
            },
        }
    }

    fn provenance(id: &str, source_byte: &str) -> Provenance {
        Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: Some(id.to_owned()),
            source_hash: Some(source_byte.repeat(64)),
            author: Some("Synthetic Shell Test".to_owned()),
            license: Some("LicenseRef-Unknown".to_owned()),
            imported_at: None,
        }
    }

    fn shell_and_target() -> (tempfile::TempDir, ShellApi, ContentModuleRuntimeTarget) {
        let root = tempdir().expect("temporary Shell root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open Shell");
        let mut source = NamedTempFile::new().expect("temporary synthetic character");
        write!(
            source,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Lifecycle","description":"Synthetic lifecycle test character."}}}}"#
        )
        .expect("write synthetic character");
        let inspection = shell
            .core
            .inspect_import(source.path())
            .expect("inspect synthetic character");
        let character = shell
            .core
            .commit_import(&inspection.id)
            .expect("commit synthetic character");
        let conversation = shell
            .core
            .open_conversation(&character.id)
            .expect("open synthetic conversation");
        let state = shell
            .core
            .get_conversation_state(&conversation.id)
            .expect("load synthetic conversation state");
        (
            root,
            shell,
            ContentModuleRuntimeTarget {
                conversation_id: conversation.id,
                branch_id: state.active_branch_id,
            },
        )
    }

    fn activation(
        module_id: &str,
        binding_id: &str,
        target: &ContentModuleRuntimeTarget,
    ) -> ContentModuleActivationRequest {
        ContentModuleActivationRequest {
            runtime_target: target.clone(),
            expected_binding_revision: None,
            binding: ContentModuleBindingDraft {
                id: ModuleBindingId::from(binding_id),
                module_id: ContentModuleId::from(module_id),
                scope: ModuleScope::App,
                target_id: None,
                conversation_id: None,
                priority: 0,
                resolution_mode: ModuleRevisionResolutionMode::Active,
                pinned_revision_id: None,
                package_import_approval_id: None,
                variable_overrides: VariableMap::default(),
            },
        }
    }

    fn empty_resolutions(review_sha256: lorepia_core::Sha256Digest) -> ModuleMergeResolutionSet {
        ModuleMergeResolutionSet {
            expected_review_sha256: review_sha256,
            resolutions: Vec::new(),
        }
    }

    struct ActivatedLifecycleFixture {
        root: tempfile::TempDir,
        shell: ShellApi,
        target: ContentModuleRuntimeTarget,
        first_module: lorepia_core::ContentModule,
        receipt: ContentModuleActivationReceiptDto,
    }

    fn activated_lifecycle_fixture() -> ActivatedLifecycleFixture {
        let (root, shell, target) = shell_and_target();
        let first_module = module("synthetic.shell.lifecycle.first", "1.0.0", "a", None);
        shell
            .core
            .upsert_content_module(&first_module, None)
            .expect("save first synthetic module");

        let candidates = shell
            .list_content_module_lifecycle_candidates(ListContentModuleLifecycleCandidatesInput {
                runtime_target: target.clone(),
                limit: 10,
            })
            .expect("list lifecycle candidates");
        let candidate = candidates
            .items
            .iter()
            .find(|candidate| candidate.module_id == first_module.id.as_str())
            .expect("first lifecycle candidate");
        assert!(candidate.local_use_allowed);
        assert!(!candidate.sharing_allowed);
        assert_eq!(candidate.license, "LicenseRef-Unknown");
        assert!(
            candidates
                .scope_targets
                .iter()
                .any(|target| target.scope == ModuleScope::Branch)
        );

        let request = activation(
            first_module.id.as_str(),
            "synthetic.shell.lifecycle.first-binding",
            &target,
        );
        let review = shell
            .review_content_module_activation(ReviewContentModuleActivationInput {
                activation: request.clone(),
            })
            .expect("review first activation");
        assert_eq!(review.proposed_revision.license, "LicenseRef-Unknown");
        assert!(review.proposed_revision.local_use_allowed);
        assert!(!review.proposed_revision.sharing_allowed);
        let mut malformed_import_projection = review.clone();
        malformed_import_projection.proposed_revision.source_kind = SourceKind::ImportedPackage;
        let malformed_import = validate_activation_review(&malformed_import_projection)
            .expect_err("an imported revision without exact authority must fail closed");
        assert_eq!(
            malformed_import.code,
            crate::ShellErrorCode::StorageCorrupted
        );
        let resolutions = empty_resolutions(review.review.review_sha256.clone());
        let plan = shell
            .resolve_content_module_activation(ResolveContentModuleActivationInput {
                activation: request.clone(),
                resolutions: resolutions.clone(),
            })
            .expect("resolve first activation");
        let activation_input = ActivateContentModuleInput {
            activation: request.clone(),
            resolutions: resolutions.clone(),
            approval: ModuleActivationApproval {
                approval_id: "synthetic-shell-stable-approval".to_owned(),
                expected_review_sha256: review.review.review_sha256,
                expected_plan_sha256: plan.plan_sha256.clone(),
            },
        };
        let receipt = shell
            .activate_content_module(activation_input.clone())
            .expect("activate first module");
        assert!(receipt.verified);
        assert_eq!(receipt.plan_sha256, plan.plan_sha256.as_str());
        assert_eq!(
            receipt.binding.binding.id.as_str(),
            "synthetic.shell.lifecycle.first-binding"
        );

        let replay = shell
            .activate_content_module(activation_input)
            .expect("recover exact activation receipt after response loss");
        assert_eq!(replay, receipt);

        let binding_workspace = shell
            .list_content_module_lifecycle_bindings(ListContentModuleLifecycleBindingsInput {
                runtime_target: target.clone(),
                limit: 10,
            })
            .expect("list bindings after activation");
        assert_eq!(binding_workspace.items.len(), 1);
        assert_eq!(
            binding_workspace.items[0].binding.state_revision,
            receipt.binding.state_revision
        );
        ActivatedLifecycleFixture {
            root,
            shell,
            target,
            first_module,
            receipt,
        }
    }

    fn assert_drifted_module_deactivation(
        shell: &ShellApi,
        target: &ContentModuleRuntimeTarget,
        first_module: &mut lorepia_core::ContentModule,
        receipt: &ContentModuleActivationReceiptDto,
    ) {
        first_module.version = "2.0.0".to_owned();
        first_module.metadata.provenance.source_hash = Some("f".repeat(64));
        shell
            .core
            .upsert_content_module(first_module, Some(1))
            .expect("advance active revision without approving it");
        let drifted_workspace = shell
            .list_content_module_lifecycle_bindings(ListContentModuleLifecycleBindingsInput {
                runtime_target: target.clone(),
                limit: 10,
            })
            .expect("list binding that needs explicit reapproval");
        let drifted = drifted_workspace
            .items
            .iter()
            .find(|item| item.binding.binding.id == receipt.binding.binding.id)
            .expect("drifted binding projection");
        assert_eq!(
            drifted.disposition,
            ContentModuleRuntimeBindingDisposition::NeedsReapproval
        );
        assert_eq!(
            drifted.approved_revision_id,
            receipt.binding.binding.revision_id.as_str()
        );
        assert_ne!(
            drifted.binding.binding.revision_id.as_str(),
            drifted.approved_revision_id
        );
        assert!(
            drifted
                .revisions
                .iter()
                .all(|revision| !revision.rollback_allowed),
            "rollback must wait until the newly resolved revision is explicitly approved"
        );

        let deactivation = ContentModuleDeactivationRequest {
            runtime_target: target.clone(),
            binding_id: receipt.binding.binding.id.clone(),
        };
        let deactivation_review = shell
            .review_content_module_deactivation(ReviewContentModuleDeactivationInput {
                deactivation: deactivation.clone(),
            })
            .expect("review exact binding deactivation");
        assert_eq!(
            deactivation_review.disposition,
            ContentModuleRuntimeBindingDisposition::NeedsReapproval
        );
        let wrong_review_sha256 = lorepia_core::Sha256Digest::parse("0".repeat(64))
            .expect("synthetic wrong deactivation review digest");
        let stale_deactivation = shell
            .deactivate_content_module(DeactivateContentModuleInput {
                deactivation: deactivation.clone(),
                expected_review_sha256: wrong_review_sha256,
            })
            .expect_err("wrong deactivation review hash must not delete the binding");
        assert_eq!(stale_deactivation.code, crate::ShellErrorCode::InvalidInput);
        assert!(
            shell
                .core
                .list_content_module_bindings(&first_module.id)
                .expect("binding after rejected deactivation preflight")
                .iter()
                .any(|stored| stored.value.id == receipt.binding.binding.id),
            "a failed deactivation preflight must not mutate the durable binding"
        );
        let deactivated = shell
            .deactivate_content_module(DeactivateContentModuleInput {
                deactivation,
                expected_review_sha256: deactivation_review.review_sha256,
            })
            .expect("deactivate exact reviewed binding");
        assert!(deactivated.verified);
        assert!(deactivated.deleted_at <= chrono::Utc::now());
        assert_eq!(
            deactivated.binding.state_revision,
            deactivation_review.expected_binding_revision + 1
        );
    }

    fn assert_stale_lifecycle_review_fails(shell: &ShellApi, target: &ContentModuleRuntimeTarget) {
        let mut stale_module = module(
            "synthetic.shell.lifecycle.stale",
            "1.0.0",
            "b",
            Some("SYNTHETIC_STALE_V1"),
        );
        let saved = shell
            .core
            .upsert_content_module(&stale_module, None)
            .expect("save stale-review module");
        let stale_request = activation(
            stale_module.id.as_str(),
            "synthetic.shell.lifecycle.stale-binding",
            target,
        );
        let stale_review = shell
            .review_content_module_activation(ReviewContentModuleActivationInput {
                activation: stale_request.clone(),
            })
            .expect("review module before metadata drift");
        stale_module.version = "2.0.0".to_owned();
        stale_module.metadata.license = "MIT".to_owned();
        stale_module.metadata.redistribution_allowed = true;
        stale_module.metadata.provenance.source_hash = Some("c".repeat(64));
        shell
            .core
            .upsert_content_module(&stale_module, Some(saved.revision))
            .expect("advance module license metadata");
        let stale_error = shell
            .resolve_content_module_activation(ResolveContentModuleActivationInput {
                activation: stale_request,
                resolutions: empty_resolutions(stale_review.review.review_sha256),
            })
            .expect_err("stale license-bound review must fail");
        assert_eq!(stale_error.code, crate::ShellErrorCode::InvalidInput);
        assert!(
            shell
                .core
                .list_content_module_bindings(&stale_module.id)
                .expect("list stale module bindings")
                .is_empty(),
            "failed review must not create a binding"
        );
    }

    #[test]
    fn activation_is_receipt_only_idempotent_and_stale_license_review_fails_closed() {
        let ActivatedLifecycleFixture {
            root,
            shell,
            target,
            mut first_module,
            receipt,
        } = activated_lifecycle_fixture();
        assert_drifted_module_deactivation(&shell, &target, &mut first_module, &receipt);
        assert_stale_lifecycle_review_fails(&shell, &target);
        drop(shell);
        let reopened = ShellApi::open(CoreConfig::new(root.path()))
            .expect("reopen Shell after content module deactivation");
        let restarted = reopened
            .list_content_module_lifecycle_bindings(ListContentModuleLifecycleBindingsInput {
                runtime_target: target,
                limit: 10,
            })
            .expect("list lifecycle bindings after restart");
        assert!(
            restarted
                .items
                .iter()
                .all(|item| item.binding.binding.id != receipt.binding.binding.id),
            "a deactivated module binding must stay absent after restart"
        );
    }

    #[test]
    fn historical_pinned_binding_is_projected_beyond_the_latest_page_and_u64_is_exact() {
        let (_root, shell, target) = shell_and_target();
        let mut historical = module(
            "synthetic.shell.historical",
            "1.0.0",
            "1",
            Some("SYNTHETIC_HISTORICAL_V1"),
        );
        historical.name = "Synthetic historical revision one".to_owned();
        let first = shell
            .core
            .upsert_content_module(&historical, None)
            .expect("save first historical module revision");
        let first_revision_id =
            ModuleRevisionId::from(first.revision_id.expect("first historical revision id"));
        let mut expected_revision = first.revision;
        for index in 2_u64..=102 {
            historical.name = format!("Synthetic historical revision {index}");
            historical.version = format!("{index}.0.0");
            historical.metadata.provenance.source_hash = Some(format!("{index:064x}"));
            historical.prompt_fragments[0]
                .template
                .as_mut()
                .expect("historical prompt template")
                .parts = vec![TemplatePart::Text {
                value: format!("SYNTHETIC_HISTORICAL_V{index}"),
            }];
            let stored = shell
                .core
                .upsert_content_module(&historical, Some(expected_revision))
                .expect("advance historical module revision");
            expected_revision = stored.revision;
        }

        let mut request = activation(
            historical.id.as_str(),
            "synthetic.shell.historical-binding",
            &target,
        );
        request.binding.resolution_mode = ModuleRevisionResolutionMode::Pinned;
        request.binding.pinned_revision_id = Some(first_revision_id.clone());
        let review = shell
            .review_content_module_activation(ReviewContentModuleActivationInput {
                activation: request.clone(),
            })
            .expect("review the oldest pinned revision");
        let resolutions = empty_resolutions(review.review.review_sha256.clone());
        let plan = shell
            .resolve_content_module_activation(ResolveContentModuleActivationInput {
                activation: request.clone(),
                resolutions: resolutions.clone(),
            })
            .expect("resolve the oldest pinned revision");
        shell
            .activate_content_module(ActivateContentModuleInput {
                activation: request,
                resolutions,
                approval: ModuleActivationApproval {
                    approval_id: "synthetic-shell-historical-approval".to_owned(),
                    expected_review_sha256: review.review.review_sha256,
                    expected_plan_sha256: plan.plan_sha256,
                },
            })
            .expect("activate the oldest pinned revision");

        let workspace = shell
            .list_content_module_lifecycle_bindings(ListContentModuleLifecycleBindingsInput {
                runtime_target: target.clone(),
                limit: 10,
            })
            .expect("project pinned binding beyond the latest revision page");
        let item = workspace
            .items
            .iter()
            .find(|item| item.binding.binding.module_id == historical.id)
            .expect("historical pinned lifecycle item");
        assert!(item.revisions_truncated);
        assert_eq!(item.module_name, "Synthetic historical revision one");
        let oldest = item
            .revisions
            .iter()
            .find(|revision| revision.revision_id == first_revision_id.as_str())
            .expect("exact pinned revision outside the newest 100");
        assert_eq!(oldest.name, "Synthetic historical revision one");
        assert_eq!(oldest.source_kind, SourceKind::UserCreated);

        let mut unsafe_request = activation(
            historical.id.as_str(),
            "synthetic.shell.unsafe-u64-binding",
            &target,
        );
        unsafe_request.expected_binding_revision = Some(MAX_JAVASCRIPT_SAFE_INTEGER + 1);
        let unsafe_error = shell
            .review_content_module_activation(ReviewContentModuleActivationInput {
                activation: unsafe_request,
            })
            .expect_err("inexact JavaScript u64 must fail before reaching Core");
        assert_eq!(unsafe_error.code, crate::ShellErrorCode::InvalidInput);
        assert!(unsafe_error.message_key.contains("invalid"));
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one authority fixture proves both activation and rollback preflight ordering"
    )]
    fn oversized_receipt_preflights_reject_before_activation_or_rollback_mutation() {
        let fixture = oversized_lifecycle_fixture();
        assert!(
            fixture.activation_request_bytes < MAX_LIFECYCLE_DOCUMENT_BYTES,
            "the caller input must remain independently IPC-bounded: {} bytes",
            fixture.activation_request_bytes
        );
        assert!(
            fixture.activation_review_bytes < MAX_LIFECYCLE_DOCUMENT_BYTES,
            "the Core review must remain independently storage-bounded: {} bytes",
            fixture.activation_review_bytes
        );
        assert!(
            fixture.activation_approval_bytes < MAX_LIFECYCLE_DOCUMENT_BYTES,
            "the approved plan must remain independently storage-bounded: {} bytes",
            fixture.activation_approval_bytes
        );
        assert!(
            fixture.activation_preflight_bytes > MAX_LIFECYCLE_DOCUMENT_BYTES,
            "the combined future receipt must cross the IPC bound: {} bytes",
            fixture.activation_preflight_bytes
        );
        eprintln!(
            "lifecycle bound fixture bytes: input={}, review={}, approval={}, activation_preflight={}",
            fixture.activation_request_bytes,
            fixture.activation_review_bytes,
            fixture.activation_approval_bytes,
            fixture.activation_preflight_bytes
        );

        assert!(
            fixture
                .shell
                .core
                .list_content_module_bindings(&fixture.module_id)
                .expect("bindings before oversized activation")
                .is_empty()
        );
        let activation_error = fixture
            .shell
            .activate_content_module(fixture.activation.clone())
            .expect_err("Shell must reject an oversized future activation receipt");
        assert_eq!(activation_error.code, crate::ShellErrorCode::InvalidInput);
        assert!(
            fixture
                .shell
                .core
                .list_content_module_bindings(&fixture.module_id)
                .expect("bindings after oversized activation preflight")
                .is_empty(),
            "receipt-size rejection must happen before the activation mutation"
        );

        // An applied runtime plan contains more material than the Shell
        // receipt and retains its own stricter storage bound. Seed the exact
        // domain-valid binding through Storage so rollback projection ordering
        // is exercised independently without weakening that runtime guard.
        let OversizedLifecycleFixture {
            _root: root,
            shell,
            target,
            module_id,
            rollback_target_revision_id,
            current_revision_id,
            activation: _,
            activation_binding,
            activation_request_bytes: _,
            activation_review_bytes: _,
            activation_approval_bytes: _,
            activation_preflight_bytes: _,
        } = fixture;
        let binding_id = activation_binding.id.clone();
        drop(shell);
        let storage = lorepia_storage::Storage::open(root.path())
            .expect("open storage for exact durable rollback fixture");
        let seeded = storage
            .save_module_binding(&activation_binding, None)
            .expect("seed exact domain-valid oversized binding");
        assert_eq!(seeded.value.revision_id, current_revision_id);
        drop(storage);
        let shell = ShellApi::open(CoreConfig::new(root.path()))
            .expect("reopen Shell over durable oversized binding");

        let rollback_review = shell
            .core
            .review_content_module_rollback(
                &binding_id,
                &rollback_target_revision_id,
                None,
                &target,
            )
            .expect("review oversized binding rollback through Core");
        let rollback_resolution = ContentModuleRollbackResolutionRequest {
            runtime_target: target,
            binding_id: binding_id.clone(),
            target_revision_id: rollback_target_revision_id,
            target_package_import_approval_id: None,
            expected_state_revision: rollback_review.rollback.expected_state_revision,
            expected_rollback_review_sha256: rollback_review.rollback.review_sha256.clone(),
            resolutions: empty_resolutions(rollback_review.activation.review_sha256.clone()),
        };
        let rollback_plan = shell
            .core
            .resolve_content_module_rollback(&rollback_resolution)
            .expect("resolve oversized binding rollback through Core");
        let rollback = ApplyContentModuleRollbackInput {
            resolution: rollback_resolution,
            expected_rollback_plan_sha256: rollback_plan.rollback.plan_sha256,
            activation_approval: ModuleActivationApproval {
                approval_id: "synthetic-shell-oversized-rollback-approval".to_owned(),
                expected_review_sha256: rollback_review.activation.review_sha256,
                expected_plan_sha256: rollback_plan.activation.plan_sha256,
            },
        };
        let rollback_preflight = shell
            .core
            .preflight_content_module_rollback(&rollback)
            .expect("Core prepares valid oversized rollback receipt");
        let rollback_preflight_bytes = serde_json::to_vec(&rollback_preflight)
            .expect("rollback receipt preflight JSON")
            .len();
        assert!(
            rollback_preflight_bytes > MAX_LIFECYCLE_DOCUMENT_BYTES,
            "the combined rollback receipt must cross the IPC bound: {rollback_preflight_bytes} bytes"
        );
        eprintln!("rollback preflight bytes: {rollback_preflight_bytes}");

        let binding_before = shell
            .core
            .list_content_module_bindings(&module_id)
            .expect("binding before oversized rollback")
            .into_iter()
            .find(|stored| stored.value.id == binding_id)
            .expect("active oversized binding before rollback");
        let rollback_error = shell
            .apply_content_module_rollback(rollback)
            .expect_err("Shell must reject an oversized future rollback receipt");
        assert_eq!(rollback_error.code, crate::ShellErrorCode::InvalidInput);
        let binding_after = shell
            .core
            .list_content_module_bindings(&module_id)
            .expect("binding after oversized rollback preflight")
            .into_iter()
            .find(|stored| stored.value.id == binding_id)
            .expect("active oversized binding remains after rollback rejection");
        assert_eq!(binding_after, binding_before);
        assert_eq!(binding_after.value.revision_id, current_revision_id);
    }

    fn activate_first_conflict_module(shell: &ShellApi, target: &ContentModuleRuntimeTarget) {
        let first = module(
            "synthetic.shell.conflict.first",
            "1.0.0",
            "d",
            Some("SYNTHETIC_CONFLICT_FIRST"),
        );
        shell
            .core
            .upsert_content_module(&first, None)
            .expect("save first conflict module");
        let first_request = activation(
            first.id.as_str(),
            "synthetic.shell.conflict.first-binding",
            target,
        );
        let first_review = shell
            .review_content_module_activation(ReviewContentModuleActivationInput {
                activation: first_request.clone(),
            })
            .expect("review first conflict module");
        let first_resolutions = empty_resolutions(first_review.review.review_sha256.clone());
        let first_plan = shell
            .resolve_content_module_activation(ResolveContentModuleActivationInput {
                activation: first_request.clone(),
                resolutions: first_resolutions.clone(),
            })
            .expect("resolve first conflict module");
        shell
            .activate_content_module(ActivateContentModuleInput {
                activation: first_request,
                resolutions: first_resolutions,
                approval: ModuleActivationApproval {
                    approval_id: "synthetic-shell-conflict-approval".to_owned(),
                    expected_review_sha256: first_review.review.review_sha256,
                    expected_plan_sha256: first_plan.plan_sha256,
                },
            })
            .expect("activate first conflict module");
    }

    fn assert_module_has_no_bindings(shell: &ShellApi, module_id: &ContentModuleId) {
        assert!(
            shell
                .core
                .list_content_module_bindings(module_id)
                .expect("list rejected module bindings")
                .is_empty()
        );
    }

    #[test]
    fn conflicts_require_exact_choose_or_omit_and_approval_ids_cannot_be_reused() {
        let (_root, shell, target) = shell_and_target();
        activate_first_conflict_module(&shell, &target);
        let second = module(
            "synthetic.shell.conflict.second",
            "1.0.0",
            "e",
            Some("SYNTHETIC_CONFLICT_SECOND"),
        );
        shell
            .core
            .upsert_content_module(&second, None)
            .expect("save second conflict module");
        let second_request = activation(
            second.id.as_str(),
            "synthetic.shell.conflict.second-binding",
            &target,
        );
        let second_review = shell
            .review_content_module_activation(ReviewContentModuleActivationInput {
                activation: second_request.clone(),
            })
            .expect("review conflicting module");
        let conflict = second_review
            .review
            .conflicts
            .first()
            .cloned()
            .expect("exact conflict projection");
        let unresolved = shell
            .resolve_content_module_activation(ResolveContentModuleActivationInput {
                activation: second_request.clone(),
                resolutions: empty_resolutions(second_review.review.review_sha256.clone()),
            })
            .expect_err("unresolved conflict must not produce a plan");
        assert_eq!(unresolved.code, crate::ShellErrorCode::InvalidInput);
        assert_module_has_no_bindings(&shell, &second.id);

        let omit = ModuleMergeResolutionSet {
            expected_review_sha256: second_review.review.review_sha256.clone(),
            resolutions: vec![ModuleConflictResolution {
                component: conflict.component.clone(),
                expected_candidates: conflict.candidates.clone(),
                selected: None,
            }],
        };
        let omitted_plan = shell
            .resolve_content_module_activation(ResolveContentModuleActivationInput {
                activation: second_request.clone(),
                resolutions: omit,
            })
            .expect("explicit omit resolves conflict");
        assert_eq!(
            omitted_plan.omitted_components,
            vec![conflict.component.clone()]
        );

        let selected = conflict
            .candidates
            .iter()
            .find(|candidate| candidate.module_id == second.id)
            .cloned()
            .expect("second module conflict candidate");
        let choose = ModuleMergeResolutionSet {
            expected_review_sha256: second_review.review.review_sha256.clone(),
            resolutions: vec![ModuleConflictResolution {
                component: conflict.component.clone(),
                expected_candidates: conflict.candidates.clone(),
                selected: Some(selected),
            }],
        };
        let chosen_plan = shell
            .resolve_content_module_activation(ResolveContentModuleActivationInput {
                activation: second_request.clone(),
                resolutions: choose.clone(),
            })
            .expect("explicit exact candidate resolves conflict");
        let reused = shell
            .activate_content_module(ActivateContentModuleInput {
                activation: second_request,
                resolutions: choose,
                approval: ModuleActivationApproval {
                    approval_id: "synthetic-shell-conflict-approval".to_owned(),
                    expected_review_sha256: second_review.review.review_sha256,
                    expected_plan_sha256: chosen_plan.plan_sha256,
                },
            })
            .expect_err("approval id reuse for another plan must fail");
        assert!(
            !reused.message_key.is_empty(),
            "conflicting approval-id reuse must return an actionable error"
        );
        assert_module_has_no_bindings(&shell, &second.id);
    }

    fn store_rollback_module_revisions(
        shell: &ShellApi,
    ) -> (lorepia_core::ContentModule, ModuleRevisionId) {
        let mut rollback_module = module(
            "synthetic.shell.rollback",
            "1.0.0",
            "1",
            Some("SYNTHETIC_ROLLBACK_V1"),
        );
        let first = shell
            .core
            .upsert_content_module(&rollback_module, None)
            .expect("save rollback v1");
        let target_revision_id =
            ModuleRevisionId::from(first.revision_id.expect("immutable rollback v1 id"));
        rollback_module.version = "2.0.0".to_owned();
        rollback_module.prompt_fragments[0]
            .template
            .as_mut()
            .expect("rollback prompt template")
            .parts = vec![TemplatePart::Text {
            value: "SYNTHETIC_ROLLBACK_V2".to_owned(),
        }];
        rollback_module.metadata.provenance.source_hash = Some("2".repeat(64));
        shell
            .core
            .upsert_content_module(&rollback_module, Some(first.revision))
            .expect("save rollback v2");
        (rollback_module, target_revision_id)
    }

    #[test]
    fn rollback_projects_exact_diff_and_succeeds_only_with_fresh_hashes() {
        let (root, shell, target) = shell_and_target();
        let (rollback_module, target_revision_id) = store_rollback_module_revisions(&shell);
        let activation = activation(
            rollback_module.id.as_str(),
            "synthetic.shell.rollback-binding",
            &target,
        );
        let review = shell
            .review_content_module_activation(ReviewContentModuleActivationInput {
                activation: activation.clone(),
            })
            .expect("review rollback module activation");
        let resolutions = empty_resolutions(review.review.review_sha256.clone());
        let plan = shell
            .resolve_content_module_activation(ResolveContentModuleActivationInput {
                activation: activation.clone(),
                resolutions: resolutions.clone(),
            })
            .expect("resolve rollback module activation");
        let activated = shell
            .activate_content_module(ActivateContentModuleInput {
                activation,
                resolutions,
                approval: ModuleActivationApproval {
                    approval_id: "synthetic-shell-rollback-activate".to_owned(),
                    expected_review_sha256: review.review.review_sha256,
                    expected_plan_sha256: plan.plan_sha256,
                },
            })
            .expect("activate rollback module");

        let rollback_review = shell
            .review_content_module_rollback(ReviewContentModuleRollbackInput {
                binding_id: activated.binding.binding.id.clone(),
                target_revision_id: target_revision_id.clone(),
                target_package_import_approval_id: None,
                runtime_target: target.clone(),
            })
            .expect("review exact rollback");
        assert!(rollback_review.review.rollback.eligible);
        assert!(
            rollback_review
                .review
                .rollback
                .diff
                .as_ref()
                .is_some_and(|diff| !diff.component_changes.is_empty())
        );
        let rollback_resolution = ContentModuleRollbackResolutionRequest {
            runtime_target: target,
            binding_id: activated.binding.binding.id,
            target_revision_id: target_revision_id.clone(),
            target_package_import_approval_id: None,
            expected_state_revision: rollback_review.review.rollback.expected_state_revision,
            expected_rollback_review_sha256: rollback_review.review.rollback.review_sha256.clone(),
            resolutions: empty_resolutions(rollback_review.review.activation.review_sha256.clone()),
        };
        let rollback_plan = shell
            .resolve_content_module_rollback(rollback_resolution.clone())
            .expect("resolve exact rollback");
        let rollback_apply = ApplyContentModuleRollbackInput {
            resolution: rollback_resolution,
            expected_rollback_plan_sha256: rollback_plan.rollback.plan_sha256,
            activation_approval: ModuleActivationApproval {
                approval_id: "synthetic-shell-rollback-approval".to_owned(),
                expected_review_sha256: rollback_review.review.activation.review_sha256,
                expected_plan_sha256: rollback_plan.activation.plan_sha256,
            },
        };
        let receipt = shell
            .apply_content_module_rollback(rollback_apply.clone())
            .expect("apply exact rollback");
        assert!(receipt.verified);
        assert_eq!(receipt.binding.binding.revision_id, target_revision_id);

        drop(shell);
        let shell = ShellApi::open(CoreConfig::new(root.path()))
            .expect("reopen Shell after losing the rollback response");
        let recovered = shell
            .apply_content_module_rollback(rollback_apply.clone())
            .expect("recover exact rollback receipt through Shell after restart");
        assert_eq!(recovered, receipt);

        let mut conflicting_retry = rollback_apply;
        conflicting_retry.activation_approval.approval_id =
            "synthetic-shell-rollback-conflicting-retry".to_owned();
        let rejected = shell
            .apply_content_module_rollback(conflicting_retry)
            .expect_err("Shell must reject rollback approval-id reuse");
        assert_eq!(rejected.code, crate::ShellErrorCode::InvalidInput);
    }
}
