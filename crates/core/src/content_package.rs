//! Core-owned staging and approval boundary for `LorePia` content packages.
//!
//! Native callers may provide a source path exactly once, during inspection.
//! Every later operation accepts only the opaque import identifier and
//! hash/revision expectations returned by this module. Reviewed bytes are
//! promoted to Core-owned content-addressed storage before durable inspection
//! state is created and are re-inspected immediately before later transitions.

mod commit;
mod inspect;
mod lifecycle;

pub use commit::{ContentPackageCommitReceipt, ContentPackageCommitRequest};
pub use inspect::ContentPackageImportInspection;
pub use lifecycle::ContentPackageDiscardRequest;

use std::collections::BTreeSet;
#[cfg(test)]
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

#[cfg(test)]
use commit::normalize_prepared_document;
use commit::prepare_package_commit;
use inspect::{OwnedContentPackageSnapshot, stale_package_review};
#[cfg(test)]
use inspect::{
    package_capability_review, package_snapshot_path, remove_owned_snapshot, stage_content_package,
};
use lifecycle::{
    load_durable_content_package, required_capability_approvals, stored_import_plan,
    stored_package_approval, target_review_bindings, validate_package_transition_expectations,
};
#[cfg(test)]
use lorepia_content::{ContentPackageComponentKind, PreparedContentDocument};
use lorepia_content::{ContentPackageSelectionPlan, select_content_package_components};
#[cfg(test)]
use lorepia_domain::{AssetId, ContentCapability, Provenance, SourceKind};
use lorepia_domain::{CoreError, CoreErrorCode, CoreResult, ImportLimits, PackageId, Sha256Digest};
#[cfg(test)]
use lorepia_orchestration::PackageComponentKind;
use lorepia_orchestration::{
    ApprovedPackageImportPlan, PackageImportApproval, PackageSelectionRequest, SelectiveImportPlan,
    approve_selective_import_plan, build_selective_import_plan,
};
use lorepia_storage::{
    PackageCapability, PackageImportExpectation, PackageImportRecord, PackageImportStatus,
    PackageImportTargetReview, PackageInspectionExpectation, PackageNormalizationEvidence,
    PackageUpdateTargetConfirmation, package_capability_review_sha256,
    package_normalization_evidence_sha256, package_update_target_confirmations_sha256,
};
#[cfg(test)]
use lorepia_storage::{PackageCommitDocument, built_in_prompt_presets};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use uuid::Uuid;

use crate::Core;

/// Exact selection request bound to one immutable inspection plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageSelectionRequest {
    pub expected_revision: u64,
    pub expected_package_plan_hash: String,
    pub expected_review_sha256: Sha256Digest,
    pub expected_capability_review_sha256: String,
    pub selected_component_ids: Vec<String>,
}

/// Durable selection receipt containing both parser and orchestration plans.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageSelectionReceipt {
    pub import: PackageImportRecord,
    pub content_selection: ContentPackageSelectionPlan,
    pub import_plan: SelectiveImportPlan,
    pub target_review: PackageImportTargetReview,
    pub normalization_evidence_sha256: String,
    pub normalization_evidence: Vec<PackageNormalizationEvidence>,
}

/// Safe, reconstructible selection state for resuming a package review.
///
/// This projection intentionally excludes the storage-owned `VersionedJson`
/// payloads. Every value is rebuilt from the durable source and verified
/// against the immutable selection record before it is returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageImportSelectionReview {
    pub content_selection_plan_hash: String,
    pub import_plan_sha256: Sha256Digest,
    pub target_review: PackageImportTargetReview,
    pub normalization_evidence_sha256: String,
    pub normalization_evidence: Vec<PackageNormalizationEvidence>,
}

/// Safe, immutable approval state for resuming a package commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageImportApprovalReview {
    pub approval_sha256: Sha256Digest,
    pub approval_id: String,
    pub enabled_component_ids: Vec<String>,
    pub approved_capabilities: Vec<PackageCapability>,
}

/// Core-owned restart projection for an inspected package import.
///
/// Unlike [`PackageImportRecord`], this type never exposes raw persistence
/// payloads. A caller can use its exact hashes to reconstruct the next
/// selection, approval, or commit request after a process restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageImportReview {
    pub import_id: String,
    pub package_id: PackageId,
    pub status: PackageImportStatus,
    pub revision: u64,
    pub package_plan_hash: String,
    pub review_sha256: Sha256Digest,
    pub capability_review_sha256: String,
    pub selected_component_ids: Vec<String>,
    pub selection: Option<ContentPackageImportSelectionReview>,
    pub approval: Option<ContentPackageImportApprovalReview>,
}

/// Exact user approval for a reviewed selection and storage revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageApprovalRequest {
    pub expected_revision: u64,
    pub expected_package_plan_hash: String,
    pub expected_content_selection_plan_hash: String,
    pub expected_review_sha256: Sha256Digest,
    pub expected_import_plan_sha256: Sha256Digest,
    pub expected_capability_review_sha256: String,
    pub expected_normalization_evidence_sha256: String,
    pub expected_target_review_sha256: String,
    pub confirmed_update_targets: Vec<PackageUpdateTargetConfirmation>,
    pub approval_id: String,
    pub enable_component_ids: Vec<String>,
    pub approved_capabilities: Vec<PackageCapability>,
}

/// Immutable approval and the durable CAS revision that stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageApprovalReceipt {
    pub import: PackageImportRecord,
    pub approved_plan: ApprovedPackageImportPlan,
    pub target_review: PackageImportTargetReview,
    pub normalization_evidence_sha256: String,
    pub normalization_evidence: Vec<PackageNormalizationEvidence>,
}

impl OwnedContentPackageSnapshot {
    pub(crate) fn select(
        &self,
        request: &ContentPackageSelectionRequest,
    ) -> CoreResult<ContentPackageSelectionPlan> {
        if request.expected_package_plan_hash != self.inspection.plan_hash
            || request.expected_review_sha256 != self.review.review_sha256
        {
            return Err(stale_package_review());
        }
        let plan =
            select_content_package_components(&self.inspection, &request.selected_component_ids)?;
        if plan.inspection_id.0 != self.import_id {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "content package selection escaped its owned inspection",
                false,
            ));
        }
        Ok(plan)
    }
}

impl Core {
    /// Reconstructs the exact safe review state needed to resume after a
    /// process restart, without exposing storage JSON or any host path.
    pub fn get_content_package_import_review(
        &self,
        import_id: &str,
    ) -> CoreResult<ContentPackageImportReview> {
        let loaded = load_durable_content_package(self, import_id, ImportLimits::default())?;
        let capability_review = self.storage().get_package_capability_review(import_id)?;
        let capability_review_sha256 = package_capability_review_sha256(&capability_review)?;
        let mut review = ContentPackageImportReview {
            import_id: loaded.record.id.clone(),
            package_id: loaded.record.package_id.clone(),
            status: loaded.record.status,
            revision: loaded.record.revision,
            package_plan_hash: loaded.owned.inspection.plan_hash.clone(),
            review_sha256: loaded.owned.review.review_sha256.clone(),
            capability_review_sha256,
            selected_component_ids: loaded.record.selected_component_ids.clone(),
            selection: None,
            approval: None,
        };
        if loaded.record.selection.is_none() {
            return Ok(review);
        }

        let import_plan = stored_import_plan(&loaded.record)?;
        validate_package_transition_expectations(
            &loaded,
            &import_plan,
            &loaded.owned.inspection.plan_hash,
            &select_content_package_components(
                loaded.owned.inspection(),
                &loaded.record.selected_component_ids,
            )?
            .selection_plan_hash,
            &loaded.owned.review.review_sha256,
            &import_plan.plan_sha256,
        )?;
        let target_review = self.storage().get_package_import_target_review(import_id)?;
        let replay_bindings = target_review_bindings(&target_review)?;
        let prepared = prepare_package_commit(
            self,
            &loaded,
            &import_plan,
            ImportLimits::default(),
            Some(&replay_bindings),
        )?;
        let normalization_evidence_sha256 =
            package_normalization_evidence_sha256(&prepared.normalization_evidence)?;
        review.selection = Some(ContentPackageImportSelectionReview {
            content_selection_plan_hash: prepared.content_selection.selection_plan_hash,
            import_plan_sha256: import_plan.plan_sha256.clone(),
            target_review,
            normalization_evidence_sha256: normalization_evidence_sha256.clone(),
            normalization_evidence: prepared.normalization_evidence.clone(),
        });

        let approval = match stored_package_approval(self, import_id) {
            Ok((approval, _)) => Some(approval),
            Err(error) if error.code == CoreErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        if let Some(approval) = approval {
            if approval.plan.plan_sha256 != import_plan.plan_sha256
                || approval.plan.review_sha256 != loaded.owned.review.review_sha256
                || approval.plan.source_sha256.as_str() != loaded.source.source_sha256
                || approval.normalization_evidence_sha256 != normalization_evidence_sha256
                || approval.normalization_evidence != prepared.normalization_evidence
                || approval.approved_capabilities != required_capability_approvals(&import_plan)
            {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "stored package approval diverges from its durable reviewed source",
                    false,
                ));
            }
            let enabled_component_ids = approval
                .plan
                .components
                .iter()
                .filter(|component| component.enabled)
                .map(|component| component.component.id.clone())
                .collect();
            review.approval = Some(ContentPackageImportApprovalReview {
                approval_sha256: approval.plan.approval_sha256,
                approval_id: approval.plan.approval_id,
                enabled_component_ids,
                approved_capabilities: approval.approved_capabilities,
            });
        } else if matches!(
            loaded.record.status,
            PackageImportStatus::Approved
                | PackageImportStatus::Committing
                | PackageImportStatus::Completed
                | PackageImportStatus::RolledBack
        ) {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "approved package import is missing its immutable approval",
                false,
            ));
        }
        Ok(review)
    }

    /// Lists only bounded, nonterminal imports that can be resumed after a
    /// `WebView` or process restart.
    ///
    /// Storage returns opaque identifiers only. Core then reopens,
    /// re-inspects, and verifies each durable source before exposing its safe
    /// review projection.
    pub fn list_pending_content_package_import_reviews(
        &self,
        limit: u32,
    ) -> CoreResult<Vec<ContentPackageImportReview>> {
        self.storage()
            .list_pending_package_import_ids(limit)?
            .into_iter()
            .map(|import_id| self.get_content_package_import_review(&import_id))
            .collect()
    }

    /// Binds a deterministic component selection to one durable inspection.
    pub fn select_content_package_import(
        &self,
        import_id: &str,
        request: &ContentPackageSelectionRequest,
    ) -> CoreResult<ContentPackageSelectionReceipt> {
        let mut loaded = load_durable_content_package(self, import_id, ImportLimits::default())?;
        if !matches!(
            loaded.record.status,
            PackageImportStatus::Inspected | PackageImportStatus::AwaitingReview
        ) {
            return Err(CoreError::invalid(
                "content package cannot be selected from its current state",
            ));
        }
        let expected = PackageInspectionExpectation {
            revision: request.expected_revision,
            inspection_sha256: request.expected_review_sha256.as_str().to_owned(),
            capability_review_sha256: request.expected_capability_review_sha256.clone(),
        };
        let content_selection = loaded.owned.select(request)?;
        let import_plan = build_selective_import_plan(
            loaded.owned.review(),
            &PackageSelectionRequest {
                expected_review_sha256: request.expected_review_sha256.clone(),
                component_ids: request.selected_component_ids.clone(),
                standalone_asset_ids: Vec::new(),
            },
        )
        .map_err(package_plan_error)?;
        let planned_ids = import_plan
            .components
            .iter()
            .map(|component| component.component.id.as_str())
            .collect::<BTreeSet<_>>();
        let content_ids = content_selection
            .selected_component_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if planned_ids != content_ids {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "content and orchestration package selections disagree",
                false,
            ));
        }
        loaded
            .record
            .selected_component_ids
            .clone_from(&content_selection.selected_component_ids);
        let replay_target_review = (loaded.record.status == PackageImportStatus::AwaitingReview)
            .then(|| self.storage().get_package_import_target_review(import_id))
            .transpose()?;
        let replay_bindings = replay_target_review
            .as_ref()
            .map(target_review_bindings)
            .transpose()?;
        let prepared = prepare_package_commit(
            self,
            &loaded,
            &import_plan,
            ImportLimits::default(),
            replay_bindings.as_deref(),
        )?;
        let normalization_evidence_sha256 =
            package_normalization_evidence_sha256(&prepared.normalization_evidence)?;
        let import = self.storage().select_package_import(
            import_id,
            &expected,
            &import_plan,
            &prepared.bindings,
        )?;
        let target_review = self.storage().get_package_import_target_review(import_id)?;
        Ok(ContentPackageSelectionReceipt {
            import,
            content_selection,
            import_plan,
            target_review,
            normalization_evidence_sha256,
            normalization_evidence: prepared.normalization_evidence,
        })
    }

    /// Stores an exact user approval and a complete document/revision binding
    /// snapshot. No selected content is written by this step.
    pub fn approve_content_package_import(
        &self,
        import_id: &str,
        request: &ContentPackageApprovalRequest,
    ) -> CoreResult<ContentPackageApprovalReceipt> {
        let loaded = load_durable_content_package(self, import_id, ImportLimits::default())?;
        if !matches!(
            loaded.record.status,
            PackageImportStatus::AwaitingReview | PackageImportStatus::Approved
        ) {
            return Err(CoreError::invalid(
                "content package cannot be approved from its current state",
            ));
        }
        let import_plan = stored_import_plan(&loaded.record)?;
        let expected = PackageImportExpectation {
            revision: request.expected_revision,
            inspection_sha256: request.expected_review_sha256.as_str().to_owned(),
            selection_sha256: request.expected_import_plan_sha256.as_str().to_owned(),
            capability_review_sha256: request.expected_capability_review_sha256.clone(),
        };
        validate_package_transition_expectations(
            &loaded,
            &import_plan,
            &request.expected_package_plan_hash,
            &request.expected_content_selection_plan_hash,
            &request.expected_review_sha256,
            &request.expected_import_plan_sha256,
        )?;
        let replay_approval = (loaded.record.status == PackageImportStatus::Approved)
            .then(|| stored_package_approval(self, import_id).map(|value| value.0))
            .transpose()?;
        let prepared = prepare_package_commit(
            self,
            &loaded,
            &import_plan,
            ImportLimits::default(),
            replay_approval
                .as_ref()
                .map(|approval| approval.document_bindings.as_slice()),
        )?;
        let normalization_evidence_sha256 =
            package_normalization_evidence_sha256(&prepared.normalization_evidence)?;
        if normalization_evidence_sha256 != request.expected_normalization_evidence_sha256 {
            return Err(stale_package_review());
        }
        let target_review = self.storage().get_package_import_target_review(import_id)?;
        if target_review.target_review_sha256 != request.expected_target_review_sha256 {
            return Err(stale_package_review());
        }
        let approved_plan = approve_selective_import_plan(
            &import_plan,
            &PackageImportApproval {
                approval_id: request.approval_id.clone(),
                expected_review_sha256: request.expected_review_sha256.clone(),
                expected_plan_sha256: request.expected_import_plan_sha256.clone(),
                target_review_sha256: Sha256Digest::parse(
                    request.expected_target_review_sha256.clone(),
                )
                .map_err(|error| {
                    CoreError::invalid(format!("package target-review digest is invalid: {error}"))
                })?,
                update_target_confirmations_sha256: Sha256Digest::parse(
                    package_update_target_confirmations_sha256(&request.confirmed_update_targets)?,
                )
                .map_err(|error| {
                    CoreError::internal(format!(
                        "canonical package confirmation digest is invalid: {error}"
                    ))
                })?,
                enable_component_ids: request.enable_component_ids.clone(),
            },
        )
        .map_err(package_plan_error)?;
        let import = self.storage().approve_package_import(
            import_id,
            &expected,
            &approved_plan,
            &prepared.bindings,
            &request.expected_target_review_sha256,
            &request.confirmed_update_targets,
            &request.approved_capabilities,
            &prepared.normalization_evidence,
        )?;
        Ok(ContentPackageApprovalReceipt {
            import,
            approved_plan,
            target_review,
            normalization_evidence_sha256,
            normalization_evidence: prepared.normalization_evidence,
        })
    }
}

fn package_plan_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!("package selection or approval is invalid: {error}"))
}

#[cfg(test)]
pub(crate) fn discard_content_package_snapshot(
    import_id: &str,
    staging_dir: &Path,
) -> CoreResult<()> {
    let snapshot = package_snapshot_path(staging_dir, import_id)?;
    remove_owned_snapshot(&snapshot, staging_dir, import_id)
}

#[cfg(test)]
mod tests {
    include!("content_package/tests/support.rs");
    include!("content_package/tests/canonical_and_module_authority.rs");
    include!("content_package/tests/durability_and_atomicity.rs");
    include!("content_package/tests/prompt_and_snapshot_security.rs");
}
