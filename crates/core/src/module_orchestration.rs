//! High-level, review-bound content-module activation and rollback.
//!
//! Native callers describe an inert binding draft. Core resolves every
//! immutable module revision from storage, derives the applicable scope
//! context, and recreates reviews immediately before mutation. Approval hashes
//! therefore acknowledge Core-authored plans rather than caller-authored
//! resolved revisions.

mod runtime_plan;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ContentCapability, ContentModuleId, ContentModuleRevision, ConversationBranchId,
    ConversationId, CoreError, CoreErrorCode, CoreResult, ModuleBinding, ModuleBindingId,
    ModuleComponentRef, ModuleRevisionId, ModuleRevisionResolutionMode, ModuleScope, PackageId,
    PersonaId, Sha256Digest, SourceKind, ValidateOrchestration, VariableMap, VariableScope,
};
use lorepia_orchestration::{
    ApprovedModuleActivationPlan, ApprovedModuleRollbackPlan, IgnoredModuleBindingReason,
    ModuleActivationApproval, ModuleActivationPlan, ModuleActivationReview, ModuleCandidateSource,
    ModuleImportApprovalEvidence, ModuleImportComponentAuthority, ModuleMergeError,
    ModuleMergeResolutionSet, ModuleResolutionContext, ModuleRevisionSnapshot, ModuleRollbackPlan,
    ModuleRollbackPolicy, ModuleRollbackReview, PackageComponentKind,
    approve_module_activation_plan, approve_module_rollback_plan, prepare_module_rollback,
    resolve_module_merge, review_module_activation, review_module_rollback,
};
use lorepia_storage::{
    ActiveContentModuleRevision, CompletedPackageAuthority, PackageImportStatus,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use self::runtime_plan::content_module_runtime_binding_summary;
use crate::{Core, Revisioned, revision::project_revision};

const MAXIMUM_CONTENT_MODULE_SCHEMA_VERSION: u32 = 1;
pub(crate) const SUPPORTED_CONTENT_CAPABILITIES: [ContentCapability; 10] = [
    ContentCapability::PromptFragments,
    ContentCapability::Knowledge,
    ContentCapability::Variables,
    ContentCapability::Transforms,
    ContentCapability::DeclarativeInteractions,
    ContentCapability::ImageAssets,
    ContentCapability::AudioAssets,
    ContentCapability::VideoAssets,
    ContentCapability::AttachmentAssets,
    ContentCapability::HighRiskAssets,
];

/// Caller-authored portion of a module binding.
///
/// Activation state, immutable revision identity, and timestamps are
/// deliberately absent. Core derives all three from current durable state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleBindingDraft {
    pub id: ModuleBindingId,
    pub module_id: ContentModuleId,
    pub scope: ModuleScope,
    pub target_id: Option<String>,
    pub conversation_id: Option<ConversationId>,
    pub priority: i32,
    pub resolution_mode: ModuleRevisionResolutionMode,
    pub pinned_revision_id: Option<ModuleRevisionId>,
    pub package_import_approval_id: Option<String>,
    pub variable_overrides: VariableMap,
}

/// Reader-safe completed import authority for one immutable module revision.
///
/// Callers must explicitly copy `package_import_approval_id` into a later
/// binding draft. Core never chooses one candidate merely because it is
/// newest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleImportApprovalCandidate {
    pub package_import_approval_id: String,
    pub approval_sha256: Sha256Digest,
    pub import_id: String,
    pub import_revision: u64,
    pub package_id: PackageId,
    pub package_source_sha256: Sha256Digest,
    pub selection_sha256: Sha256Digest,
    pub capability_review_sha256: Sha256Digest,
    pub module_id: ContentModuleId,
    pub module_revision_id: ModuleRevisionId,
    pub module_revision_source_sha256: Sha256Digest,
}

pub const MAX_CONTENT_MODULE_IMPORT_APPROVAL_CANDIDATES: usize = 64;
pub const MAX_CONTENT_MODULE_REVISION_SUMMARIES: usize = 100;

/// Immutable module revision metadata used by lifecycle and rollback surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRevisionSummary {
    pub module_id: ContentModuleId,
    pub revision_id: ModuleRevisionId,
    pub state_revision: u64,
    pub name: String,
    pub version: String,
    pub source_sha256: Sha256Digest,
    pub source_kind: SourceKind,
    pub previous_revision_id: Option<ModuleRevisionId>,
    pub created_at: DateTime<Utc>,
    pub active: bool,
}

/// Why one context-relevant binding is or is not currently applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentModuleRuntimeBindingDisposition {
    Applied,
    /// The binding was approved for an older immutable revision, while its
    /// `active` resolution now points at a newer revision. Runtime remains
    /// fail-closed until the newer revision receives an explicit approval.
    NeedsReapproval,
    Disabled,
    AwaitingApproval,
}

/// One mutable binding row paired with its exact context-resolved revision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRuntimeBindingSummary {
    pub binding: ModuleBinding,
    /// Immutable revision named by the last durable approval. For an
    /// `active` binding this may differ from the freshly resolved revision in
    /// `binding.revision_id`, in which case `disposition` is
    /// [`ContentModuleRuntimeBindingDisposition::NeedsReapproval`].
    pub approved_revision_id: ModuleRevisionId,
    pub state_revision: u64,
    pub updated_at: DateTime<Utc>,
    pub disposition: ContentModuleRuntimeBindingDisposition,
}

/// Reader-safe module workspace for one existing conversation branch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRuntimeWorkspace {
    pub review_sha256: Sha256Digest,
    pub state_revision: u64,
    pub context: ModuleResolutionContext,
    pub bindings: Vec<ContentModuleRuntimeBindingSummary>,
}

/// Concrete room whose complete effective module stack is being approved.
///
/// Native callers identify only the conversation and branch. Core derives and
/// validates character, local-user, and active-persona identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRuntimeTarget {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
}

/// Stable input used for review, resolution, and the final CAS activation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationRequest {
    pub runtime_target: ContentModuleRuntimeTarget,
    /// `None` creates a binding. `Some(revision)` replaces that exact live
    /// binding revision.
    pub expected_binding_revision: Option<u64>,
    pub binding: ContentModuleBindingDraft,
}

/// One exact component authorized by an approved module plan.
///
/// Package-stored transform and interaction flags remain disabled. Runtime
/// code may derive an ephemeral enable overlay only from these hash-bound
/// component/source tuples after verifying the full approved plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedContentModuleComponent {
    pub component: ModuleComponentRef,
    pub component_sha256: Sha256Digest,
    pub selected_source: ModuleCandidateSource,
    /// Plan-approved ephemeral runtime overlay. The immutable stored
    /// component remains quarantine-disabled.
    #[serde(default)]
    pub runtime_enabled: bool,
}

/// Atomic activation receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationReceipt {
    pub binding: Revisioned<ModuleBinding>,
    pub approved_plan: ApprovedModuleActivationPlan,
    pub approved_components: Vec<ApprovedContentModuleComponent>,
}

/// Read-only projection of every bounded field a successful activation or
/// rollback receipt will expose, excluding the storage-authored update time.
///
/// Shell validates this before mutation so a result revision or payload bound
/// cannot fail only after the durable write has committed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationReceiptPreflight {
    pub binding: ModuleBinding,
    pub resulting_state_revision: u64,
    pub approved_plan: ApprovedModuleActivationPlan,
    pub approved_components: Vec<ApprovedContentModuleComponent>,
}

/// Stable caller input for a two-step, hash-reviewed binding deactivation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleDeactivationRequest {
    pub runtime_target: ContentModuleRuntimeTarget,
    pub binding_id: ModuleBindingId,
}

/// Exact context-relevant binding state acknowledged before deactivation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleDeactivationReview {
    pub review_sha256: Sha256Digest,
    pub runtime_target: ContentModuleRuntimeTarget,
    pub binding: ModuleBinding,
    pub approved_revision_id: ModuleRevisionId,
    pub expected_binding_revision: u64,
    pub binding_updated_at: DateTime<Utc>,
    pub disposition: ContentModuleRuntimeBindingDisposition,
}

/// Durable receipt returned after the binding CAS deletion commits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleDeactivationReceipt {
    pub review: ContentModuleDeactivationReview,
    pub binding: Revisioned<ModuleBinding>,
}

/// Reader-safe metadata for the exact immutable module revision in an
/// activation review.
///
/// The revision id and source digest tie the displayed license/share
/// disposition to the same Core-resolved target used by the hash-bound review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationRevisionReview {
    pub module_id: ContentModuleId,
    pub revision_id: ModuleRevisionId,
    pub revision_source_sha256: Sha256Digest,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub license: String,
    pub redistribution_allowed: bool,
    pub required_capabilities: Vec<ContentCapability>,
    pub source_kind: SourceKind,
    pub local_use_allowed: bool,
    pub sharing_allowed: bool,
    pub share_reasons: Vec<String>,
}

/// Activation review plus the exact safe metadata a review UI may display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleActivationReviewPresentation {
    pub review: ModuleActivationReview,
    pub proposed_revision: ContentModuleActivationRevisionReview,
}

/// Rollback eligibility and the complete target-revision activation review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRollbackReview {
    pub rollback: ModuleRollbackReview,
    pub activation: ModuleActivationReview,
}

/// Rollback review plus the exact target-revision license/share disposition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRollbackReviewPresentation {
    pub review: ContentModuleRollbackReview,
    pub target_revision: ContentModuleActivationRevisionReview,
}

/// Hash echoes and explicit conflict choices used to prepare a rollback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRollbackResolutionRequest {
    pub runtime_target: ContentModuleRuntimeTarget,
    pub binding_id: ModuleBindingId,
    pub target_revision_id: ModuleRevisionId,
    /// Exact completed-package authority for the target immutable revision.
    /// Required only when the rollback target came from an imported package.
    pub target_package_import_approval_id: Option<String>,
    pub expected_state_revision: u64,
    pub expected_rollback_review_sha256: Sha256Digest,
    pub resolutions: ModuleMergeResolutionSet,
}

/// The independently verifiable rollback and target runtime plans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRollbackPlan {
    pub rollback: ModuleRollbackPlan,
    pub activation: ModuleActivationPlan,
}

impl ContentModuleRollbackPlan {
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        self.rollback.verify()?;
        self.activation.verify()
    }
}

/// Final rollback approval request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentModuleRollbackApplyRequest {
    pub resolution: ContentModuleRollbackResolutionRequest,
    pub expected_rollback_plan_sha256: Sha256Digest,
    pub activation_approval: ModuleActivationApproval,
}

impl ContentModuleActivationReceipt {
    /// Verifies the approval hash, durable binding tuple, and explicit
    /// component projection.
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        self.approved_plan.verify()?;
        let expected_components = approved_components(&self.approved_plan);
        if self.approved_components != expected_components
            || !self.binding.value.enabled
            || !self.binding.value.approved
            || self.binding.value.activation_approval_id.as_deref()
                != Some(self.approved_plan.approval_id.as_str())
            || self.binding.value.activation_review_sha256.as_ref()
                != Some(&self.approved_plan.plan.review_sha256)
            || self.binding.value.activation_plan_sha256.as_ref()
                != Some(&self.approved_plan.plan.plan_sha256)
        {
            return Err(ModuleMergeError::ActivationApprovalHashMismatch);
        }
        Ok(())
    }
}

impl ContentModuleActivationReceiptPreflight {
    /// Verifies the exact resulting binding approval tuple and CAS increment.
    pub fn verify(&self) -> Result<(), ModuleMergeError> {
        self.approved_plan.verify()?;
        let expected_components = approved_components(&self.approved_plan);
        let expected_state_revision = self
            .approved_plan
            .plan
            .expected_state_revision
            .checked_add(1)
            .ok_or(ModuleMergeError::ActivationApprovalHashMismatch)?;
        if self.approved_components != expected_components
            || self.resulting_state_revision != expected_state_revision
            || !self.binding.enabled
            || !self.binding.approved
            || self.binding.activation_approval_id.as_deref()
                != Some(self.approved_plan.approval_id.as_str())
            || self.binding.activation_review_sha256.as_ref()
                != Some(&self.approved_plan.plan.review_sha256)
            || self.binding.activation_plan_sha256.as_ref()
                != Some(&self.approved_plan.plan.plan_sha256)
            || self.approved_plan.plan.activation_binding_ids.as_slice()
                != [self.binding.id.clone()]
        {
            return Err(ModuleMergeError::ActivationApprovalHashMismatch);
        }
        Ok(())
    }
}

impl ContentModuleDeactivationReview {
    pub fn verify(&self) -> CoreResult<()> {
        let expected = content_module_deactivation_review_sha256(
            &self.runtime_target,
            &self.binding,
            &self.approved_revision_id,
            self.expected_binding_revision,
            &self.binding_updated_at,
            self.disposition,
        )?;
        if expected != self.review_sha256 {
            return Err(CoreError::invalid(
                "content module deactivation review hash is invalid",
            ));
        }
        Ok(())
    }
}

impl ContentModuleDeactivationReceipt {
    pub fn verify(&self) -> CoreResult<()> {
        self.review.verify()?;
        let expected_revision = self
            .review
            .expected_binding_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("module binding revision overflow"))?;
        if self.binding.revision != expected_revision
            || self.binding.deleted_at.is_none()
            || self.binding.value != self.review.binding
        {
            return Err(CoreError::invalid(
                "content module deactivation receipt does not match its review",
            ));
        }
        Ok(())
    }
}

struct PreparedModuleActivation {
    review: ModuleActivationReview,
    proposed_snapshot: ModuleRevisionSnapshot,
}

struct PreparedModuleRollback {
    review: ContentModuleRollbackReview,
    target_snapshot: ModuleRevisionSnapshot,
}

impl Core {
    /// Reviews one inert binding draft against freshly resolved module
    /// revisions and the complete applicable binding stack.
    pub fn review_content_module_activation(
        &self,
        request: &ContentModuleActivationRequest,
    ) -> CoreResult<ModuleActivationReview> {
        self.prepare_content_module_activation(request)
            .map(|prepared| prepared.review)
    }

    /// Reviews activation and returns the exact immutable license/share
    /// disposition intended for native review surfaces.
    pub fn review_content_module_activation_presentation(
        &self,
        request: &ContentModuleActivationRequest,
    ) -> CoreResult<ContentModuleActivationReviewPresentation> {
        let prepared = self.prepare_content_module_activation(request)?;
        Ok(ContentModuleActivationReviewPresentation {
            proposed_revision: module_activation_revision_review(&prepared.proposed_snapshot),
            review: prepared.review,
        })
    }

    /// Lists completed import approvals that own one exact imported module
    /// revision.
    ///
    /// Results are deterministic and safe to persist in a review workspace.
    /// A later activation still reloads and verifies the selected approval
    /// against durable package, CAS, audit, and component records.
    pub fn list_content_module_import_approval_candidates(
        &self,
        module_id: &ContentModuleId,
        resolution_mode: ModuleRevisionResolutionMode,
        pinned_revision_id: Option<&ModuleRevisionId>,
        limit: usize,
    ) -> CoreResult<Vec<ContentModuleImportApprovalCandidate>> {
        if limit == 0 || limit > MAX_CONTENT_MODULE_IMPORT_APPROVAL_CANDIDATES {
            return Err(CoreError::invalid(format!(
                "content module import approval candidate limit must be between 1 and {MAX_CONTENT_MODULE_IMPORT_APPROVAL_CANDIDATES}",
            )));
        }
        let stored =
            self.resolve_content_module_revision(module_id, resolution_mode, pinned_revision_id)?;
        if stored.object.value.metadata.provenance.source_kind != SourceKind::ImportedPackage {
            return Err(CoreError::invalid(
                "content module import approval recovery requires an imported package revision",
            ));
        }
        Ok(self
            .storage()
            .list_completed_package_import_authorities_for_module_revision(&stored)?
            .into_iter()
            .take(limit)
            .map(content_module_import_approval_candidate)
            .collect())
    }

    /// Lists newest-first immutable revision metadata for one module.
    pub fn list_content_module_revision_summaries(
        &self,
        module_id: &ContentModuleId,
        limit: usize,
    ) -> CoreResult<Vec<ContentModuleRevisionSummary>> {
        if limit == 0 || limit > MAX_CONTENT_MODULE_REVISION_SUMMARIES {
            return Err(CoreError::invalid(format!(
                "content module revision summary limit must be between 1 and {MAX_CONTENT_MODULE_REVISION_SUMMARIES}",
            )));
        }
        let active = self
            .storage()
            .get_active_content_module_revision(module_id)?;
        self.storage()
            .list_content_module_revisions(module_id)?
            .into_iter()
            .rev()
            .take(limit)
            .map(|object_revision| {
                let revision_id = ModuleRevisionId::from(object_revision.revision_id);
                let stored = self
                    .storage()
                    .get_content_module_revision(module_id, &revision_id)?;
                if stored.module_revision.id != revision_id
                    || stored.module_revision.module_id != *module_id
                {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "content module revision projection has a different identity",
                        false,
                    ));
                }
                Ok(ContentModuleRevisionSummary {
                    module_id: stored.module_revision.module_id,
                    revision_id: stored.module_revision.id.clone(),
                    state_revision: object_revision.revision,
                    name: stored.object.value.name,
                    version: stored.module_revision.version,
                    source_sha256: stored.module_revision.source_hash,
                    source_kind: stored.object.value.metadata.provenance.source_kind,
                    previous_revision_id: stored.module_revision.previous_revision_id,
                    created_at: stored.module_revision.created_at,
                    active: active.module_revision.id == stored.module_revision.id,
                })
            })
            .collect()
    }

    /// Loads one exact immutable revision summary even when it falls outside
    /// the bounded newest-first history page used by lifecycle readers.
    pub fn get_content_module_revision_summary(
        &self,
        module_id: &ContentModuleId,
        revision_id: &ModuleRevisionId,
    ) -> CoreResult<ContentModuleRevisionSummary> {
        let active = self
            .storage()
            .get_active_content_module_revision(module_id)?;
        let stored = self
            .storage()
            .get_content_module_revision(module_id, revision_id)?;
        Ok(ContentModuleRevisionSummary {
            module_id: module_id.clone(),
            revision_id: stored.module_revision.id.clone(),
            state_revision: stored.object.revision,
            name: stored.object.value.name,
            version: stored.module_revision.version,
            source_sha256: stored.module_revision.source_hash,
            source_kind: stored.object.value.metadata.provenance.source_kind,
            previous_revision_id: stored.module_revision.previous_revision_id,
            created_at: stored.module_revision.created_at,
            active: active.module_revision.id == stored.module_revision.id,
        })
    }

    /// Builds the context-relevant binding workspace for one existing room.
    ///
    /// Bindings aimed at other scopes or rooms are omitted. Every returned
    /// binding carries the exact revision resolved for this review.
    pub fn review_content_module_runtime_workspace(
        &self,
        runtime_target: &ContentModuleRuntimeTarget,
    ) -> CoreResult<ContentModuleRuntimeWorkspace> {
        let branch = self
            .storage()
            .get_conversation_branch(&runtime_target.branch_id)?;
        if branch.conversation_id != runtime_target.conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "module runtime branch was not found in the conversation",
                false,
            ));
        }
        let context = self.content_module_context_for_proposed_branch(
            &runtime_target.conversation_id,
            &runtime_target.branch_id,
        )?;
        let prepared = self.prepare_current_content_module_runtime(&context)?;
        let mut bindings_by_id = prepared
            .bindings
            .into_iter()
            .map(|(stored, resolved)| (resolved.id.clone(), (stored, resolved)))
            .collect::<BTreeMap<_, _>>();
        let mut bindings = Vec::new();
        for resolved in &prepared.review.ordered_bindings {
            let (stored, _) = bindings_by_id.remove(&resolved.id).ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "module runtime review references a missing binding",
                    false,
                )
            })?;
            let disposition = if stored.value.revision_id == resolved.revision_id {
                ContentModuleRuntimeBindingDisposition::Applied
            } else {
                ContentModuleRuntimeBindingDisposition::NeedsReapproval
            };
            bindings.push(content_module_runtime_binding_summary(
                stored,
                resolved.clone(),
                disposition,
            ));
        }
        for ignored in &prepared.review.ignored_bindings {
            let disposition = match ignored.reason {
                IgnoredModuleBindingReason::Disabled => {
                    ContentModuleRuntimeBindingDisposition::Disabled
                }
                IgnoredModuleBindingReason::AwaitingApproval => {
                    let (stored, resolved) =
                        bindings_by_id.get(&ignored.binding_id).ok_or_else(|| {
                            CoreError::new(
                                CoreErrorCode::StorageCorrupted,
                                "module runtime review references a missing ignored binding",
                                false,
                            )
                        })?;
                    if stored.value.approved && stored.value.revision_id != resolved.revision_id {
                        ContentModuleRuntimeBindingDisposition::NeedsReapproval
                    } else {
                        ContentModuleRuntimeBindingDisposition::AwaitingApproval
                    }
                }
                IgnoredModuleBindingReason::DifferentTarget => continue,
            };
            let (stored, resolved) =
                bindings_by_id.remove(&ignored.binding_id).ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "module runtime review references a missing ignored binding",
                        false,
                    )
                })?;
            bindings.push(content_module_runtime_binding_summary(
                stored,
                resolved,
                disposition,
            ));
        }
        Ok(ContentModuleRuntimeWorkspace {
            review_sha256: prepared.review.review_sha256,
            state_revision: prepared.review.state_revision,
            context: prepared.review.context,
            bindings,
        })
    }

    /// Reviews the exact context-relevant binding before a CAS deactivation.
    pub fn review_content_module_deactivation(
        &self,
        request: &ContentModuleDeactivationRequest,
    ) -> CoreResult<ContentModuleDeactivationReview> {
        let workspace = self.review_content_module_runtime_workspace(&request.runtime_target)?;
        let summary = workspace
            .bindings
            .into_iter()
            .find(|summary| summary.binding.id == request.binding_id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "content module binding was not found in the runtime room",
                    false,
                )
            })?;
        let durable = self
            .storage()
            .list_all_module_bindings()?
            .into_iter()
            .find(|stored| stored.value.id == request.binding_id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "content module runtime review references a missing durable binding",
                    false,
                )
            })?;
        if durable.revision != summary.state_revision
            || durable.updated_at != summary.updated_at
            || durable.value.revision_id != summary.approved_revision_id
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "content module runtime review disagrees with its durable binding",
                false,
            ));
        }
        let review_sha256 = content_module_deactivation_review_sha256(
            &request.runtime_target,
            &durable.value,
            &summary.approved_revision_id,
            summary.state_revision,
            &summary.updated_at,
            summary.disposition,
        )?;
        let review = ContentModuleDeactivationReview {
            review_sha256,
            runtime_target: request.runtime_target.clone(),
            binding: durable.value,
            approved_revision_id: summary.approved_revision_id,
            expected_binding_revision: summary.state_revision,
            binding_updated_at: summary.updated_at,
            disposition: summary.disposition,
        };
        review.verify()?;
        Ok(review)
    }

    /// Recreates an exact deactivation review and then CAS-deletes the binding.
    pub fn deactivate_content_module(
        &self,
        request: &ContentModuleDeactivationRequest,
        expected_review_sha256: &Sha256Digest,
    ) -> CoreResult<ContentModuleDeactivationReceipt> {
        let review = self.review_content_module_deactivation(request)?;
        if &review.review_sha256 != expected_review_sha256 {
            return Err(CoreError::invalid(
                "content module deactivation review is stale",
            ));
        }
        let binding =
            self.unbind_content_module(&review.binding.id, review.expected_binding_revision)?;
        let receipt = ContentModuleDeactivationReceipt { review, binding };
        receipt.verify()?;
        Ok(receipt)
    }

    /// Resolves every reviewed component conflict without changing durable
    /// activation state.
    pub fn resolve_content_module_activation(
        &self,
        request: &ContentModuleActivationRequest,
        resolutions: &ModuleMergeResolutionSet,
    ) -> CoreResult<ModuleActivationPlan> {
        let prepared = self.prepare_content_module_activation(request)?;
        resolve_module_merge(&prepared.review, resolutions).map_err(module_merge_error)
    }

    /// Activates an exact, freshly recreated review and plan.
    ///
    /// The storage commit performs the final binding CAS and module-revision
    /// recheck in one immediate transaction.
    pub fn activate_content_module(
        &self,
        request: &ContentModuleActivationRequest,
        resolutions: &ModuleMergeResolutionSet,
        approval: &ModuleActivationApproval,
    ) -> CoreResult<ContentModuleActivationReceipt> {
        if let Some(receipt) =
            self.recover_applied_content_module_activation(request, resolutions, approval)?
        {
            return Ok(receipt);
        }
        let (prepared, approved) =
            self.prepare_approved_content_module_activation(request, resolutions, approval)?;
        self.commit_content_module_activation(&prepared, &approved)
    }

    /// Prepares and verifies every bounded field of a future activation
    /// receipt without mutating storage. The commit path independently
    /// recreates the review and approval before its transactional rechecks.
    pub fn preflight_content_module_activation(
        &self,
        request: &ContentModuleActivationRequest,
        resolutions: &ModuleMergeResolutionSet,
        approval: &ModuleActivationApproval,
    ) -> CoreResult<ContentModuleActivationReceiptPreflight> {
        if let Some(receipt) =
            self.recover_applied_content_module_activation(request, resolutions, approval)?
        {
            return content_module_activation_receipt_preflight_from_receipt(&receipt);
        }
        let (prepared, approved) =
            self.prepare_approved_content_module_activation(request, resolutions, approval)?;
        content_module_activation_receipt_preflight(&prepared.review, &approved)
    }

    fn prepare_approved_content_module_activation(
        &self,
        request: &ContentModuleActivationRequest,
        resolutions: &ModuleMergeResolutionSet,
        approval: &ModuleActivationApproval,
    ) -> CoreResult<(PreparedModuleActivation, ApprovedModuleActivationPlan)> {
        let prepared = self.prepare_content_module_activation(request)?;
        let plan =
            resolve_module_merge(&prepared.review, resolutions).map_err(module_merge_error)?;
        let approved =
            approve_module_activation_plan(&plan, approval).map_err(module_merge_error)?;
        Ok((prepared, approved))
    }

    /// Returns the exact first receipt when an already committed activation
    /// response was lost. Every caller-authored field, resolution, hash, and
    /// pre-write CAS revision must match the persisted authority.
    fn recover_applied_content_module_activation(
        &self,
        request: &ContentModuleActivationRequest,
        resolutions: &ModuleMergeResolutionSet,
        approval: &ModuleActivationApproval,
    ) -> CoreResult<Option<ContentModuleActivationReceipt>> {
        let Some(recovered) = self
            .storage()
            .recover_applied_module_activation(&request.binding.id, approval)?
        else {
            return Ok(None);
        };
        let lorepia_storage::RecoveredModuleActivation {
            review,
            approved,
            binding,
        } = recovered;
        let (expected_state_revision, request_cas_matches) = match request.expected_binding_revision
        {
            None => (0, review.state_revision == 0),
            Some(expected) => (expected, expected != 0 && review.state_revision == expected),
        };
        let expected_applied_revision = expected_state_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("module activation binding revision overflow"))?;
        let reviewed_binding = review
            .ordered_bindings
            .iter()
            .find(|candidate| candidate.id == request.binding.id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "applied module review has no activation binding",
                    false,
                )
            })?;
        if !request_cas_matches
            || binding.revision != expected_applied_revision
            || review.context.conversation_id.as_deref()
                != Some(request.runtime_target.conversation_id.0.as_str())
            || review.context.branch_id.as_deref()
                != Some(request.runtime_target.branch_id.0.as_str())
            || !binding_matches_draft(reviewed_binding, &request.binding)
            || !binding_matches_draft(&binding.value, &request.binding)
            || reviewed_binding.enabled
            || reviewed_binding.approved
            || reviewed_binding.activation_approval_id.is_some()
            || reviewed_binding.activation_review_sha256.is_some()
            || reviewed_binding.activation_plan_sha256.is_some()
            || reviewed_binding.revision_id != binding.value.revision_id
        {
            return Err(CoreError::invalid(
                "applied module activation does not match the retried request",
            ));
        }
        let resolved = resolve_module_merge(&review, resolutions).map_err(module_merge_error)?;
        if resolved != approved.plan {
            return Err(CoreError::invalid(
                "retried module conflict resolutions differ from the applied plan",
            ));
        }
        let reconstructed =
            approve_module_activation_plan(&resolved, approval).map_err(module_merge_error)?;
        if reconstructed != approved {
            return Err(CoreError::invalid(
                "retried module approval differs from the applied authority",
            ));
        }
        let receipt = ContentModuleActivationReceipt {
            binding: project_revision(binding),
            approved_components: approved_components(&approved),
            approved_plan: approved,
        };
        receipt.verify().map_err(module_merge_error)?;
        Ok(Some(receipt))
    }

    /// Produces a hash-bound rollback review from the exact live binding and
    /// immutable target revision.
    pub fn review_content_module_rollback(
        &self,
        binding_id: &ModuleBindingId,
        target_revision_id: &ModuleRevisionId,
        target_package_import_approval_id: Option<&str>,
        runtime_target: &ContentModuleRuntimeTarget,
    ) -> CoreResult<ContentModuleRollbackReview> {
        self.prepare_content_module_rollback(
            binding_id,
            target_revision_id,
            target_package_import_approval_id,
            runtime_target,
        )
        .map(|prepared| prepared.review)
    }

    /// Reviews rollback and returns the exact immutable target license/share
    /// disposition intended for native review surfaces.
    pub fn review_content_module_rollback_presentation(
        &self,
        binding_id: &ModuleBindingId,
        target_revision_id: &ModuleRevisionId,
        target_package_import_approval_id: Option<&str>,
        runtime_target: &ContentModuleRuntimeTarget,
    ) -> CoreResult<ContentModuleRollbackReviewPresentation> {
        let prepared = self.prepare_content_module_rollback(
            binding_id,
            target_revision_id,
            target_package_import_approval_id,
            runtime_target,
        )?;
        Ok(ContentModuleRollbackReviewPresentation {
            target_revision: module_activation_revision_review(&prepared.target_snapshot),
            review: prepared.review,
        })
    }

    /// Resolves the target runtime composition without mutating durable state.
    pub fn resolve_content_module_rollback(
        &self,
        request: &ContentModuleRollbackResolutionRequest,
    ) -> CoreResult<ContentModuleRollbackPlan> {
        let prepared = self.prepare_content_module_rollback(
            &request.binding_id,
            &request.target_revision_id,
            request.target_package_import_approval_id.as_deref(),
            &request.runtime_target,
        )?;
        let review = prepared.review;
        validate_rollback_request(&review, request)?;
        let rollback =
            prepare_module_rollback(&review.rollback, &request.expected_rollback_review_sha256)
                .map_err(module_merge_error)?;
        let activation = resolve_module_merge(&review.activation, &request.resolutions)
            .map_err(module_merge_error)?;
        Ok(ContentModuleRollbackPlan {
            rollback,
            activation,
        })
    }

    /// Recreates both reviews, verifies both plan hash echoes, and commits the
    /// rollback together with its target component overlay in one transaction.
    pub fn apply_content_module_rollback(
        &self,
        request: &ContentModuleRollbackApplyRequest,
    ) -> CoreResult<ContentModuleActivationReceipt> {
        if let Some(receipt) = self.recover_applied_content_module_rollback(request)? {
            return Ok(receipt);
        }
        let approved = self.prepare_approved_content_module_rollback(request)?;
        self.commit_content_module_rollback(&approved)
    }

    /// Prepares and verifies every bounded field of a future rollback receipt
    /// without mutating storage. Exact response-loss replays are projected
    /// from their durable rollback authority.
    pub fn preflight_content_module_rollback(
        &self,
        request: &ContentModuleRollbackApplyRequest,
    ) -> CoreResult<ContentModuleActivationReceiptPreflight> {
        if let Some(receipt) = self.recover_applied_content_module_rollback(request)? {
            return content_module_activation_receipt_preflight_from_receipt(&receipt);
        }
        let approved = self.prepare_approved_content_module_rollback(request)?;
        content_module_activation_receipt_preflight(
            &approved.activation_review,
            &approved.activation,
        )
    }

    fn prepare_approved_content_module_rollback(
        &self,
        request: &ContentModuleRollbackApplyRequest,
    ) -> CoreResult<ApprovedModuleRollbackPlan> {
        let prepared = self.prepare_content_module_rollback(
            &request.resolution.binding_id,
            &request.resolution.target_revision_id,
            request
                .resolution
                .target_package_import_approval_id
                .as_deref(),
            &request.resolution.runtime_target,
        )?;
        let review = prepared.review;
        validate_rollback_request(&review, &request.resolution)?;
        let approved = approve_module_rollback_plan(
            &review.rollback,
            &request.resolution.expected_rollback_review_sha256,
            &review.activation,
            &request.resolution.resolutions,
            &request.activation_approval,
        )
        .map_err(module_merge_error)?;
        if approved.rollback.plan_sha256 != request.expected_rollback_plan_sha256 {
            return Err(CoreError::invalid("content module rollback plan is stale"));
        }
        Ok(approved)
    }

    /// Returns the exact first receipt when an already committed rollback
    /// response was lost. Recovery is attempted before recreating live state,
    /// because a successful rollback has necessarily advanced the binding CAS
    /// revision and made the original rollback review non-current.
    fn recover_applied_content_module_rollback(
        &self,
        request: &ContentModuleRollbackApplyRequest,
    ) -> CoreResult<Option<ContentModuleActivationReceipt>> {
        let Some(recovered) = self.storage().recover_applied_module_rollback(
            &request.resolution.binding_id,
            &request.activation_approval,
        )?
        else {
            return Ok(None);
        };
        let lorepia_storage::RecoveredModuleRollback { approved, binding } = recovered;
        approved.verify().map_err(module_merge_error)?;

        let rollback = &approved.rollback;
        let activation_review = &approved.activation_review;
        let approved_activation = &approved.activation;
        let resolution = &request.resolution;
        let expected_applied_revision = resolution
            .expected_state_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("module rollback binding revision overflow"))?;
        if rollback.binding_id != resolution.binding_id
            || rollback.target_revision_id != resolution.target_revision_id
            || rollback.expected_state_revision != resolution.expected_state_revision
            || rollback.review_sha256 != resolution.expected_rollback_review_sha256
            || rollback.plan_sha256 != request.expected_rollback_plan_sha256
            || binding.revision != expected_applied_revision
            || binding.value.id != resolution.binding_id
            || binding.value.revision_id != resolution.target_revision_id
            || activation_review.state_revision != resolution.expected_state_revision
            || activation_review.context.conversation_id.as_deref()
                != Some(resolution.runtime_target.conversation_id.0.as_str())
            || activation_review.context.branch_id.as_deref()
                != Some(resolution.runtime_target.branch_id.0.as_str())
        {
            return Err(CoreError::invalid(
                "applied module rollback does not match the retried request",
            ));
        }

        let proposed = activation_review
            .ordered_bindings
            .iter()
            .find(|candidate| candidate.id == resolution.binding_id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "applied module rollback review has no target binding",
                    false,
                )
            })?;
        let proposed_draft = ContentModuleBindingDraft {
            id: proposed.id.clone(),
            module_id: proposed.module_id.clone(),
            scope: proposed.scope,
            target_id: proposed.target_id.clone(),
            conversation_id: proposed.conversation_id.clone(),
            priority: proposed.priority,
            resolution_mode: proposed.resolution_mode,
            pinned_revision_id: proposed.pinned_revision_id.clone(),
            package_import_approval_id: proposed.package_import_approval_id.clone(),
            variable_overrides: proposed.variable_overrides.clone(),
        };
        if proposed.resolution_mode != ModuleRevisionResolutionMode::Pinned
            || proposed.pinned_revision_id.as_ref() != Some(&resolution.target_revision_id)
            || proposed.revision_id != resolution.target_revision_id
            || proposed.package_import_approval_id != resolution.target_package_import_approval_id
            || proposed.enabled
            || proposed.approved
            || proposed.activation_approval_id.is_some()
            || proposed.activation_review_sha256.is_some()
            || proposed.activation_plan_sha256.is_some()
            || !binding_matches_draft(&binding.value, &proposed_draft)
            || binding.value.created_at != proposed.created_at
        {
            return Err(CoreError::invalid(
                "applied module rollback target differs from the retried request",
            ));
        }

        let resolved = resolve_module_merge(activation_review, &resolution.resolutions)
            .map_err(module_merge_error)?;
        if resolved != approved_activation.plan {
            return Err(CoreError::invalid(
                "retried module rollback resolutions differ from the applied plan",
            ));
        }
        let reconstructed = approve_module_activation_plan(&resolved, &request.activation_approval)
            .map_err(module_merge_error)?;
        if reconstructed != *approved_activation {
            return Err(CoreError::invalid(
                "retried module rollback approval differs from the applied authority",
            ));
        }

        let receipt = ContentModuleActivationReceipt {
            binding: project_revision(binding),
            approved_components: approved_components(approved_activation),
            approved_plan: approved.activation,
        };
        receipt.verify().map_err(module_merge_error)?;
        Ok(Some(receipt))
    }

    fn commit_content_module_rollback(
        &self,
        approved: &ApprovedModuleRollbackPlan,
    ) -> CoreResult<ContentModuleActivationReceipt> {
        let binding = self.storage().apply_approved_module_rollback(approved)?;
        let receipt = ContentModuleActivationReceipt {
            binding: project_revision(binding),
            approved_plan: approved.activation.clone(),
            approved_components: approved_components(&approved.activation),
        };
        receipt.verify().map_err(module_merge_error)?;
        Ok(receipt)
    }

    fn prepare_content_module_activation(
        &self,
        request: &ContentModuleActivationRequest,
    ) -> CoreResult<PreparedModuleActivation> {
        let stored_bindings = self.storage().list_all_module_bindings()?;
        let current = stored_bindings
            .iter()
            .find(|stored| stored.value.id == request.binding.id);
        match (request.expected_binding_revision, current) {
            (None, None) => {}
            (Some(expected), Some(stored)) if expected == stored.revision => {}
            _ => {
                return Err(CoreError::invalid(
                    "content module binding changed before activation review",
                ));
            }
        }

        let context = self.module_activation_context(&request.binding, &request.runtime_target)?;
        let mut snapshots = BTreeMap::new();
        let mut bindings = Vec::with_capacity(stored_bindings.len());
        for stored in &stored_bindings {
            let (binding, snapshot) = self.resolve_module_binding(&stored.value)?;
            // `review_module_activation` replaces this exact binding with the
            // proposed draft before it resolves any components. Do not retain
            // the stale binding's revision-scoped package authority: an
            // imported active-revision advance can legitimately replace it
            // with the new exact approval for the same resolved revision.
            if stored.value.id != request.binding.id {
                insert_revision_snapshot(&mut snapshots, snapshot)?;
            }
            bindings.push(binding);
        }

        let (revision_id, proposed_snapshot) =
            self.resolve_module_draft_revision(&request.binding)?;
        // A create review may be replayed across several API calls before the
        // binding exists. Use immutable Core-owned revision time rather than
        // wall-clock time so review -> resolve -> activate hashes stay stable.
        let created_at = current.map_or(proposed_snapshot.revision.created_at, |stored| {
            stored.created_at
        });
        let proposed_binding = ModuleBinding {
            id: request.binding.id.clone(),
            module_id: request.binding.module_id.clone(),
            scope: request.binding.scope,
            target_id: request.binding.target_id.clone(),
            conversation_id: request.binding.conversation_id.clone(),
            priority: request.binding.priority,
            resolution_mode: request.binding.resolution_mode,
            pinned_revision_id: request.binding.pinned_revision_id.clone(),
            enabled: false,
            approved: false,
            package_import_approval_id: request.binding.package_import_approval_id.clone(),
            activation_approval_id: None,
            activation_review_sha256: None,
            activation_plan_sha256: None,
            variable_overrides: request.binding.variable_overrides.clone(),
            revision_id,
            created_at,
        };
        validate_module_binding_variables(&proposed_binding)?;
        proposed_binding
            .validate()
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        insert_revision_snapshot(&mut snapshots, proposed_snapshot.clone())?;

        let review = review_module_activation(
            request.expected_binding_revision,
            &context,
            &bindings,
            &proposed_binding,
            &snapshots.into_values().collect::<Vec<_>>(),
        )
        .map_err(module_merge_error)?;
        Ok(PreparedModuleActivation {
            review,
            proposed_snapshot,
        })
    }

    fn resolve_module_draft_revision(
        &self,
        draft: &ContentModuleBindingDraft,
    ) -> CoreResult<(ModuleRevisionId, ModuleRevisionSnapshot)> {
        let stored = self.resolve_content_module_revision(
            &draft.module_id,
            draft.resolution_mode,
            draft.pinned_revision_id.as_ref(),
        )?;
        let revision_id = stored.module_revision.id.clone();
        let snapshot =
            self.module_snapshot_for_binding(stored, draft.package_import_approval_id.as_deref())?;
        Ok((revision_id, snapshot))
    }

    fn resolve_content_module_revision(
        &self,
        module_id: &ContentModuleId,
        resolution_mode: ModuleRevisionResolutionMode,
        pinned_revision_id: Option<&ModuleRevisionId>,
    ) -> CoreResult<ActiveContentModuleRevision> {
        match (resolution_mode, pinned_revision_id) {
            (ModuleRevisionResolutionMode::Active, None) => {
                self.storage().get_active_content_module_revision(module_id)
            }
            (ModuleRevisionResolutionMode::Pinned, Some(pinned)) => self
                .storage()
                .get_content_module_revision(module_id, pinned),
            (ModuleRevisionResolutionMode::Pinned, None) => Err(CoreError::invalid(
                "pinned module activation requires a revision id",
            )),
            (ModuleRevisionResolutionMode::Active, Some(_)) => Err(CoreError::invalid(
                "active module activation cannot include a pinned revision",
            )),
        }
    }

    fn module_activation_context(
        &self,
        draft: &ContentModuleBindingDraft,
        runtime_target: &ContentModuleRuntimeTarget,
    ) -> CoreResult<ModuleResolutionContext> {
        let conversation = self
            .storage()
            .get_conversation(&runtime_target.conversation_id)?;
        let branch = self
            .storage()
            .get_conversation_branch(&runtime_target.branch_id)?;
        if branch.conversation_id != runtime_target.conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "module runtime branch was not found in the conversation",
                false,
            ));
        }
        let settings = self.storage().load_settings()?;
        let persona_id = self
            .storage()
            .get_conversation_persona_selection(&runtime_target.conversation_id)?
            .map(|selection| selection.value.persona_id);
        let context = ModuleResolutionContext {
            local_user_id: settings.local_user_id,
            persona_id,
            character_id: Some(conversation.character_id),
            conversation_id: Some(runtime_target.conversation_id.0.clone()),
            branch_id: Some(runtime_target.branch_id.0.clone()),
            supported_capabilities: SUPPORTED_CONTENT_CAPABILITIES.to_vec(),
        };
        let applies = match draft.scope {
            ModuleScope::App | ModuleScope::User
                if draft.target_id.is_none() && draft.conversation_id.is_none() =>
            {
                true
            }
            ModuleScope::Persona if draft.conversation_id.is_none() => {
                draft.target_id.as_deref() == context.persona_id.as_ref().map(PersonaId::as_str)
            }
            ModuleScope::Character if draft.conversation_id.is_none() => {
                draft.target_id.as_deref() == context.character_id.as_deref()
            }
            ModuleScope::Conversation if draft.conversation_id.is_none() => {
                draft.target_id.as_deref() == context.conversation_id.as_deref()
            }
            ModuleScope::Branch => {
                draft.target_id.as_deref() == context.branch_id.as_deref()
                    && draft.conversation_id.as_ref() == Some(&runtime_target.conversation_id)
            }
            _ => false,
        };
        if !applies {
            return Err(CoreError::invalid(
                "content module binding does not apply to the concrete runtime room",
            ));
        }
        Ok(context)
    }

    fn prepare_content_module_rollback(
        &self,
        binding_id: &ModuleBindingId,
        target_revision_id: &ModuleRevisionId,
        target_package_import_approval_id: Option<&str>,
        runtime_target: &ContentModuleRuntimeTarget,
    ) -> CoreResult<PreparedModuleRollback> {
        let stored_binding = self
            .storage()
            .list_all_module_bindings()?
            .into_iter()
            .find(|stored| stored.value.id == *binding_id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "content module binding was not found",
                    false,
                )
            })?;
        validate_module_binding_variables(&stored_binding.value)?;
        let current = match stored_binding.value.resolution_mode {
            ModuleRevisionResolutionMode::Active => self
                .storage()
                .get_active_content_module_revision(&stored_binding.value.module_id)?,
            ModuleRevisionResolutionMode::Pinned => self.storage().get_content_module_revision(
                &stored_binding.value.module_id,
                stored_binding
                    .value
                    .pinned_revision_id
                    .as_ref()
                    .ok_or_else(|| {
                        CoreError::invalid("stored pinned module binding has no revision id")
                    })?,
            )?,
        };
        let target = self
            .storage()
            .get_content_module_revision(&stored_binding.value.module_id, target_revision_id)?;
        let known_revisions = self.load_module_revision_lineage(&stored_binding.value.module_id)?;
        let target_assets = target.object.value.asset_ids.clone();
        let current_snapshot = module_snapshot(current);
        let target_snapshot = module_snapshot(target);
        let rollback = review_module_rollback(
            &stored_binding.value,
            &current_snapshot,
            &target_snapshot,
            &known_revisions,
            &ModuleRollbackPolicy {
                state_revision: stored_binding.revision,
                maximum_module_schema_version: MAXIMUM_CONTENT_MODULE_SCHEMA_VERSION,
                scope_target_exists: true,
                available_asset_ids: target_assets,
                supported_capabilities: SUPPORTED_CONTENT_CAPABILITIES.to_vec(),
                quarantined_revision_ids: Vec::new(),
                unresolved_components: Vec::new(),
            },
        )
        .map_err(module_merge_error)?;
        let activation_request = ContentModuleActivationRequest {
            runtime_target: runtime_target.clone(),
            expected_binding_revision: Some(stored_binding.revision),
            binding: ContentModuleBindingDraft {
                id: stored_binding.value.id,
                module_id: stored_binding.value.module_id,
                scope: stored_binding.value.scope,
                target_id: stored_binding.value.target_id,
                conversation_id: stored_binding.value.conversation_id,
                priority: stored_binding.value.priority,
                resolution_mode: ModuleRevisionResolutionMode::Pinned,
                pinned_revision_id: Some(target_revision_id.clone()),
                package_import_approval_id: target_package_import_approval_id.map(str::to_owned),
                variable_overrides: stored_binding.value.variable_overrides,
            },
        };
        let activation = self
            .prepare_content_module_activation(&activation_request)?
            .review;
        Ok(PreparedModuleRollback {
            review: ContentModuleRollbackReview {
                rollback,
                activation,
            },
            target_snapshot,
        })
    }

    fn load_module_revision_lineage(
        &self,
        module_id: &ContentModuleId,
    ) -> CoreResult<Vec<ContentModuleRevision>> {
        self.storage()
            .list_content_module_revisions(module_id)?
            .into_iter()
            .map(|revision| {
                self.storage()
                    .get_content_module_revision(
                        module_id,
                        &ModuleRevisionId::from(revision.revision_id),
                    )
                    .map(|stored| stored.module_revision)
            })
            .collect()
    }

    fn commit_content_module_activation(
        &self,
        prepared: &PreparedModuleActivation,
        approved: &ApprovedModuleActivationPlan,
    ) -> CoreResult<ContentModuleActivationReceipt> {
        let binding = self
            .storage()
            .apply_approved_module_activation(&prepared.review, approved)?;
        let receipt = ContentModuleActivationReceipt {
            binding: project_revision(binding),
            approved_plan: approved.clone(),
            approved_components: approved_components(approved),
        };
        receipt.verify().map_err(module_merge_error)?;
        Ok(receipt)
    }
}

fn module_snapshot(stored: ActiveContentModuleRevision) -> ModuleRevisionSnapshot {
    ModuleRevisionSnapshot {
        module: stored.object.value,
        revision: stored.module_revision,
        import_approval: None,
    }
}

fn content_module_import_approval_candidate(
    evidence: ModuleImportApprovalEvidence,
) -> ContentModuleImportApprovalCandidate {
    ContentModuleImportApprovalCandidate {
        package_import_approval_id: evidence.approval_id,
        approval_sha256: evidence.approval_sha256,
        import_id: evidence.import_id,
        import_revision: evidence.import_revision,
        package_id: evidence.package_id,
        package_source_sha256: evidence.package_source_sha256,
        selection_sha256: evidence.selection_sha256,
        capability_review_sha256: evidence.capability_review_sha256,
        module_id: evidence.module_id,
        module_revision_id: evidence.module_revision_id,
        module_revision_source_sha256: evidence.module_revision_source_sha256,
    }
}

#[derive(Serialize)]
struct ContentModuleDeactivationReviewDigest<'a> {
    runtime_target: &'a ContentModuleRuntimeTarget,
    binding: &'a ModuleBinding,
    approved_revision_id: &'a ModuleRevisionId,
    expected_binding_revision: u64,
    binding_updated_at: &'a DateTime<Utc>,
    disposition: ContentModuleRuntimeBindingDisposition,
}

fn content_module_deactivation_review_sha256(
    runtime_target: &ContentModuleRuntimeTarget,
    binding: &ModuleBinding,
    approved_revision_id: &ModuleRevisionId,
    expected_binding_revision: u64,
    binding_updated_at: &DateTime<Utc>,
    disposition: ContentModuleRuntimeBindingDisposition,
) -> CoreResult<Sha256Digest> {
    let encoded = serde_json::to_vec(&ContentModuleDeactivationReviewDigest {
        runtime_target,
        binding,
        approved_revision_id,
        expected_binding_revision,
        binding_updated_at,
        disposition,
    })
    .map_err(|error| {
        CoreError::invalid(format!(
            "cannot encode content module deactivation review: {error}"
        ))
    })?;
    Sha256Digest::parse(format!("{:x}", Sha256::digest(encoded)))
        .map_err(|error| CoreError::invalid(error.clone()))
}

fn module_activation_revision_review(
    snapshot: &ModuleRevisionSnapshot,
) -> ContentModuleActivationRevisionReview {
    let mut share_reasons = Vec::new();
    let license = snapshot.module.metadata.license.trim();
    if license.is_empty()
        || license.eq_ignore_ascii_case("unknown")
        || license.eq_ignore_ascii_case("LicenseRef-Unknown")
    {
        share_reasons.push("content license is unknown".to_owned());
    }
    if !snapshot.module.metadata.redistribution_allowed {
        share_reasons.push("content metadata does not allow redistribution".to_owned());
    }
    if snapshot
        .module
        .required_capabilities
        .contains(&ContentCapability::HighRiskAssets)
    {
        share_reasons.push("module contains high-risk assets".to_owned());
    }
    if snapshot.module.metadata.provenance.source_kind == SourceKind::ImportedPackage
        && snapshot.module.metadata.provenance.source_hash.is_none()
    {
        share_reasons.push("imported module has no immutable source hash".to_owned());
    }
    ContentModuleActivationRevisionReview {
        module_id: snapshot.module.id.clone(),
        revision_id: snapshot.revision.id.clone(),
        revision_source_sha256: snapshot.revision.source_hash.clone(),
        name: snapshot.module.name.clone(),
        version: snapshot.module.version.clone(),
        author: snapshot.module.metadata.author.clone(),
        license: snapshot.module.metadata.license.clone(),
        redistribution_allowed: snapshot.module.metadata.redistribution_allowed,
        required_capabilities: snapshot.module.required_capabilities.clone(),
        source_kind: snapshot.module.metadata.provenance.source_kind.clone(),
        local_use_allowed: true,
        sharing_allowed: share_reasons.is_empty(),
        share_reasons,
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "module activation rebinds one complete package approval and commit chain"
)]
fn module_import_approval_evidence(
    stored: &ActiveContentModuleRevision,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportApprovalEvidence> {
    if authority.status != PackageImportStatus::Completed
        || authority.import_revision == 0
        || authority.package_id.as_str()
            != stored
                .object
                .value
                .metadata
                .provenance
                .source_id
                .as_deref()
                .unwrap_or_default()
        || authority.source_sha256
            != stored
                .object
                .value
                .metadata
                .provenance
                .source_hash
                .as_deref()
                .unwrap_or_default()
    {
        return Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "completed package authority does not own the imported module source",
            false,
        ));
    }
    let module_components = authority
        .enabled_components
        .iter()
        .filter(|component| component.kind == PackageComponentKind::ContentModule)
        .filter_map(|component| {
            component
                .committed_documents
                .iter()
                .find(|document| {
                    document.target_object_id == stored.object.value.id.as_str()
                        && document.target_revision_id == stored.module_revision.id.as_str()
                })
                .map(|document| (component, document))
        })
        .collect::<Vec<_>>();
    if module_components.len() != 1 {
        return Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "completed package authority does not select the exact module revision",
            false,
        ));
    }
    let (module_component, module_document) = module_components[0];

    let mut selected_package_component_ids = authority
        .enabled_components
        .iter()
        .map(|component| component.component_id.clone())
        .collect::<Vec<_>>();
    selected_package_component_ids.sort();
    if selected_package_component_ids
        .windows(2)
        .any(|pair| pair[0] == pair[1])
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "completed package authority contains duplicate component identifiers",
            false,
        ));
    }
    let mut authorized_capabilities = authority.required_capabilities.clone();
    authorized_capabilities.sort();
    authorized_capabilities.dedup();

    let mut component_authorities =
        Vec::with_capacity(stored.module_revision.component_hashes.len());
    for component in &stored.module_revision.component_hashes {
        if let ModuleComponentRef::Asset { id } = &component.component {
            let matches = authority
                .committed_assets
                .iter()
                .filter(|asset| asset.asset_id == *id)
                .collect::<Vec<_>>();
            let [asset] = matches.as_slice() else {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "completed package authority does not cover the exact module asset",
                    false,
                ));
            };
            if asset.descriptor.id != *id {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "completed package asset descriptor has a different identity",
                    false,
                ));
            }
            let descriptor_sha256 = parse_module_authority_sha256(
                "package asset descriptor",
                &asset.descriptor_sha256,
            )?;
            let cas_sha256 =
                parse_module_authority_sha256("package asset content", &asset.cas_sha256)?;
            if descriptor_sha256 != component.sha256 || cas_sha256 != asset.descriptor.sha256 {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "completed package asset differs from the exact module component",
                    false,
                ));
            }
            let source_matches = asset
                .source_components
                .iter()
                .filter(|source| {
                    source.component_id == module_component.component_id
                        && source.component_sha256 == module_component.sha256
                })
                .collect::<Vec<_>>();
            let [source] = source_matches.as_slice() else {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "completed package asset is not authorized by the module component",
                    false,
                ));
            };
            component_authorities.push(ModuleImportComponentAuthority {
                component: component.component.clone(),
                component_sha256: component.sha256.clone(),
                package_component_id: source.component_id.clone(),
                package_component_sha256: parse_module_authority_sha256(
                    "package asset source component",
                    &source.component_sha256,
                )?,
                committed_target_object_id: id.as_str().to_owned(),
                committed_target_revision_id: asset.descriptor_sha256.clone(),
                committed_result_sha256: descriptor_sha256,
                committed_content_sha256: Some(cas_sha256),
            });
            continue;
        }
        let expected_kind = match component.component {
            ModuleComponentRef::PromptBlock { .. } | ModuleComponentRef::Control { .. } => None,
            ModuleComponentRef::KnowledgeBook { .. } => Some(PackageComponentKind::KnowledgeBook),
            ModuleComponentRef::TransformSet { .. } => Some(PackageComponentKind::TransformSet),
            ModuleComponentRef::InteractionRuleSet { .. } => {
                Some(PackageComponentKind::InteractionRuleSet)
            }
            ModuleComponentRef::Asset { .. } => unreachable!("asset components are handled above"),
        };
        let target_object_id = module_component_object_id(&component.component);
        let committed = match expected_kind {
            None => (module_component, module_document),
            Some(kind) => {
                let matches = authority
                    .enabled_components
                    .iter()
                    .filter(|candidate| candidate.kind == kind)
                    .flat_map(|candidate| {
                        candidate
                            .committed_documents
                            .iter()
                            .filter(|document| document.target_object_id == target_object_id)
                            .map(move |document| (candidate, document))
                    })
                    .collect::<Vec<_>>();
                let [(candidate, document)] = matches.as_slice() else {
                    return Err(CoreError::new(
                        CoreErrorCode::PermissionDenied,
                        "completed package authority does not cover every module component",
                        false,
                    ));
                };
                (*candidate, *document)
            }
        };
        component_authorities.push(ModuleImportComponentAuthority {
            component: component.component.clone(),
            component_sha256: component.sha256.clone(),
            package_component_id: committed.0.component_id.clone(),
            package_component_sha256: parse_module_authority_sha256(
                "package component",
                &committed.0.sha256,
            )?,
            committed_target_object_id: committed.1.target_object_id.clone(),
            committed_target_revision_id: committed.1.target_revision_id.clone(),
            committed_result_sha256: parse_module_authority_sha256(
                "package component commit",
                &committed.1.result_sha256,
            )?,
            committed_content_sha256: None,
        });
    }
    component_authorities.sort();

    Ok(ModuleImportApprovalEvidence {
        approval_id: authority.approval_id.clone(),
        approval_sha256: parse_module_authority_sha256(
            "package approval",
            &authority.approval_sha256,
        )?,
        import_id: authority.import_id.clone(),
        import_revision: authority.import_revision,
        package_id: authority.package_id.clone(),
        package_source_sha256: parse_module_authority_sha256(
            "package source",
            &authority.source_sha256,
        )?,
        selection_sha256: parse_module_authority_sha256(
            "package selection",
            &authority.selection_sha256,
        )?,
        capability_review_sha256: parse_module_authority_sha256(
            "package capability review",
            &authority.capability_review_sha256,
        )?,
        module_id: stored.object.value.id.clone(),
        module_revision_id: stored.module_revision.id.clone(),
        module_revision_source_sha256: stored.module_revision.source_hash.clone(),
        module_package_component_id: module_component.component_id.clone(),
        module_package_component_sha256: parse_module_authority_sha256(
            "package module component",
            &module_component.sha256,
        )?,
        module_commit_result_sha256: parse_module_authority_sha256(
            "package module commit",
            &module_document.result_sha256,
        )?,
        selected_package_component_ids,
        authorized_capabilities,
        component_authorities,
    })
}

fn module_component_object_id(component: &ModuleComponentRef) -> &str {
    match component {
        ModuleComponentRef::PromptBlock { id } => id.as_str(),
        ModuleComponentRef::Control { id } => id.as_str(),
        ModuleComponentRef::KnowledgeBook { id } => id.as_str(),
        ModuleComponentRef::TransformSet { id } => id.as_str(),
        ModuleComponentRef::InteractionRuleSet { id } => id.as_str(),
        ModuleComponentRef::Asset { id } => id.as_str(),
    }
}

fn parse_module_authority_sha256(label: &str, value: &str) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(value.to_owned()).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("completed {label} hash is invalid: {error}"),
            false,
        )
    })
}

fn insert_revision_snapshot(
    snapshots: &mut BTreeMap<ModuleRevisionId, ModuleRevisionSnapshot>,
    snapshot: ModuleRevisionSnapshot,
) -> CoreResult<()> {
    if let Some(previous) = snapshots.insert(snapshot.revision.id.clone(), snapshot.clone())
        && previous != snapshot
    {
        return Err(CoreError::internal(
            "one module revision id resolved to different immutable content",
        ));
    }
    Ok(())
}

fn validate_module_binding_variables(binding: &ModuleBinding) -> CoreResult<()> {
    binding.variable_overrides.validate().map_err(|error| {
        CoreError::invalid(format!(
            "module binding variable overrides are invalid: {error}"
        ))
    })?;
    if binding.variable_overrides.values.iter().any(|value| {
        value.variable.scope != VariableScope::Module
            || value.variable.namespace.as_ref() != Some(&binding.module_id)
    }) {
        return Err(CoreError::invalid(
            "module binding may override only its own namespaced module variables",
        ));
    }
    Ok(())
}

fn binding_matches_draft(binding: &ModuleBinding, draft: &ContentModuleBindingDraft) -> bool {
    binding.id == draft.id
        && binding.module_id == draft.module_id
        && binding.scope == draft.scope
        && binding.target_id == draft.target_id
        && binding.conversation_id == draft.conversation_id
        && binding.priority == draft.priority
        && binding.resolution_mode == draft.resolution_mode
        && binding.pinned_revision_id == draft.pinned_revision_id
        && binding.package_import_approval_id == draft.package_import_approval_id
        && binding.variable_overrides == draft.variable_overrides
}

fn validate_rollback_request(
    review: &ContentModuleRollbackReview,
    request: &ContentModuleRollbackResolutionRequest,
) -> CoreResult<()> {
    if review.rollback.binding_id != request.binding_id
        || review.rollback.target_revision_id != request.target_revision_id
        || review.rollback.expected_state_revision != request.expected_state_revision
        || review.rollback.review_sha256 != request.expected_rollback_review_sha256
        || review.activation.review_sha256 != request.resolutions.expected_review_sha256
    {
        return Err(CoreError::invalid(
            "content module rollback review is stale",
        ));
    }
    Ok(())
}

fn module_merge_error(error: ModuleMergeError) -> CoreError {
    CoreError::invalid(format!("content module orchestration failed: {error}"))
}

fn approved_components(
    approved: &ApprovedModuleActivationPlan,
) -> Vec<ApprovedContentModuleComponent> {
    approved
        .plan
        .components
        .iter()
        .map(|component| ApprovedContentModuleComponent {
            component: component.component.clone(),
            component_sha256: component.sha256.clone(),
            selected_source: component.selected_source.clone(),
            runtime_enabled: component.runtime_enabled,
        })
        .collect()
}

fn content_module_activation_receipt_preflight(
    review: &ModuleActivationReview,
    approved: &ApprovedModuleActivationPlan,
) -> CoreResult<ContentModuleActivationReceiptPreflight> {
    review.verify().map_err(module_merge_error)?;
    approved.verify().map_err(module_merge_error)?;
    let binding_id = approved
        .plan
        .activation_binding_ids
        .as_slice()
        .first()
        .ok_or_else(|| CoreError::invalid("module activation requires one binding"))?;
    if approved.plan.activation_binding_ids.len() != 1 {
        return Err(CoreError::invalid(
            "module activation requires exactly one binding",
        ));
    }
    let mut binding = review
        .ordered_bindings
        .iter()
        .find(|binding| &binding.id == binding_id)
        .cloned()
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "approved module review has no activation binding",
                false,
            )
        })?;
    binding.enabled = true;
    binding.approved = true;
    binding.activation_approval_id = Some(approved.approval_id.clone());
    binding.activation_review_sha256 = Some(review.review_sha256.clone());
    binding.activation_plan_sha256 = Some(approved.plan.plan_sha256.clone());
    let resulting_state_revision = review
        .state_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("module activation binding revision overflow"))?;
    let preflight = ContentModuleActivationReceiptPreflight {
        binding,
        resulting_state_revision,
        approved_plan: approved.clone(),
        approved_components: approved_components(approved),
    };
    preflight.verify().map_err(module_merge_error)?;
    Ok(preflight)
}

fn content_module_activation_receipt_preflight_from_receipt(
    receipt: &ContentModuleActivationReceipt,
) -> CoreResult<ContentModuleActivationReceiptPreflight> {
    receipt.verify().map_err(module_merge_error)?;
    let preflight = ContentModuleActivationReceiptPreflight {
        binding: receipt.binding.value.clone(),
        resulting_state_revision: receipt.binding.revision,
        approved_plan: receipt.approved_plan.clone(),
        approved_components: receipt.approved_components.clone(),
    };
    preflight.verify().map_err(module_merge_error)?;
    Ok(preflight)
}
