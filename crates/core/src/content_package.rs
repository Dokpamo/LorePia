//! Core-owned staging and approval boundary for `LorePia` content packages.
//!
//! Native callers may provide a source path exactly once, during inspection.
//! Every later operation accepts only the opaque import identifier and
//! hash/revision expectations returned by this module. Reviewed bytes are
//! promoted to Core-owned content-addressed storage before durable inspection
//! state is created and are re-inspected immediately before later transitions.

mod inspect;

pub use inspect::ContentPackageImportInspection;

use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use inspect::{
    OwnedContentPackageSnapshot, asset_capability, package_json_error, reopen_content_package,
    stale_package_review, validate_import_id, with_cleanup_error, with_two_cleanup_errors,
};
#[cfg(test)]
use inspect::{
    package_capability_review, package_snapshot_path, remove_owned_snapshot, stage_content_package,
};
#[cfg(test)]
use lorepia_content::ContentPackageComponentKind;
use lorepia_content::{
    ContentPackageSelectionPlan, PreparedContentDocument, PreparedContentDocumentEnvelope,
    PreparedContentPackageImport, StagedContentPackageAsset, discard_staged_content_package_assets,
    prepare_content_package_import, select_content_package_components,
    stage_selected_content_package_assets,
};
#[cfg(test)]
use lorepia_domain::SourceKind;
use lorepia_domain::{
    AssetDescriptor, AssetId, BlockSource, ContentCapability, CoreError, CoreErrorCode, CoreResult,
    ImportLimits, InstructionAuthority, PackageId, PlacementZone, PromptBlockKind, Provenance,
    Sha256Digest, ValidateOrchestration,
};
#[cfg(test)]
use lorepia_orchestration::PackageComponentKind;
use lorepia_orchestration::{
    ApprovedPackageImportPlan, PackageImportApproval, PackageReview, PackageSelectionRequest,
    RedistributionStatus, SelectiveImportPlan, approve_selective_import_plan,
    build_selective_import_plan,
};
use lorepia_storage::{
    CompletedPackageAuthority, PackageCapability, PackageCommitDocument, PackageCommitInput,
    PackageDocumentCommitBinding, PackageImportExpectation, PackageImportRecord,
    PackageImportStatus, PackageImportTargetReview, PackageInspectionExpectation,
    PackageNormalizationEvidence, PackageSourceRecord, PackageUpdateTargetConfirmation,
    StagedAssetImport, built_in_prompt_presets, package_capability_review_sha256,
    package_normalization_evidence_sha256, package_update_target_confirmations_sha256,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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

/// Exact commit expectation. Commit re-inspects the private snapshot before
/// any durable content mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageCommitRequest {
    pub expected_revision: u64,
    pub expected_package_plan_hash: String,
    pub expected_content_selection_plan_hash: String,
    pub expected_review_sha256: Sha256Digest,
    pub expected_import_plan_sha256: Sha256Digest,
    pub expected_approval_sha256: Sha256Digest,
    pub expected_capability_review_sha256: String,
    pub expected_normalization_evidence_sha256: String,
}

/// Durable result of one atomic selected package commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageCommitReceipt {
    pub import: PackageImportRecord,
    pub committed_document_ids: Vec<String>,
    pub asset_ids: Vec<AssetId>,
}

/// Exact discard expectation for either an inspected or selected import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageDiscardRequest {
    pub expected_revision: u64,
    pub expected_review_sha256: Sha256Digest,
    pub expected_import_plan_sha256: Option<Sha256Digest>,
    pub expected_capability_review_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPackageApprovalPayload {
    plan: ApprovedPackageImportPlan,
    document_bindings: Vec<PackageDocumentCommitBinding>,
    target_review_sha256: String,
    confirmed_update_targets: Vec<PackageUpdateTargetConfirmation>,
    approved_capabilities: Vec<PackageCapability>,
    normalization_evidence_sha256: String,
    normalization_evidence: Vec<PackageNormalizationEvidence>,
}

struct DurableContentPackageImport {
    source: PackageSourceRecord,
    record: PackageImportRecord,
    owned: OwnedContentPackageSnapshot,
}

struct PreparedPackageCommit {
    content_selection: ContentPackageSelectionPlan,
    documents: Vec<PackageCommitDocument>,
    assets: Vec<AssetDescriptor>,
    bindings: Vec<PackageDocumentCommitBinding>,
    normalization_evidence: Vec<PackageNormalizationEvidence>,
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

    pub(crate) fn prepare(
        &self,
        selection: &ContentPackageSelectionPlan,
        expected_package_plan_hash: &str,
        expected_selection_plan_hash: &str,
        limits: ImportLimits,
    ) -> CoreResult<PreparedContentPackageImport> {
        if selection.inspection_id.0 != self.import_id
            || selection.source_sha256 != self.inspection.source_sha256
            || selection.package_plan_hash != self.inspection.plan_hash
            || expected_package_plan_hash != self.inspection.plan_hash
            || expected_selection_plan_hash != selection.selection_plan_hash
        {
            return Err(stale_package_review());
        }
        let prepared = prepare_content_package_import(&self.path, limits, selection)?;
        if prepared.inspection.plan_hash != self.inspection.plan_hash
            || prepared.inspection.source_sha256 != self.inspection.source_sha256
            || prepared.selection != *selection
        {
            return Err(CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "content package changed after approval",
                false,
            ));
        }
        Ok(prepared)
    }
}

impl Core {
    /// Reopens an existing durable inspection without consulting the original
    /// caller path. This is the restart/continuation surface for native UI.
    pub fn get_content_package_import_inspection(
        &self,
        import_id: &str,
    ) -> CoreResult<ContentPackageImportInspection> {
        let loaded = load_durable_content_package(self, import_id, ImportLimits::default())?;
        let capability_review = self.storage().get_package_capability_review(import_id)?;
        loaded
            .owned
            .public_inspection(loaded.record.revision, capability_review)
    }

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

    /// Resolves an opaque approval id into exact, completed package authority.
    ///
    /// Approved-but-uncommitted, discarded, stale, or tampered imports fail
    /// closed. This safe projection is the only package authority that content
    /// module activation may consume.
    pub fn get_completed_content_package_authority(
        &self,
        approval_id: &str,
    ) -> CoreResult<CompletedPackageAuthority> {
        self.storage()
            .get_completed_package_authority_by_approval_id(approval_id)
    }

    pub fn get_content_package_import(&self, import_id: &str) -> CoreResult<PackageImportRecord> {
        validate_import_id(import_id)?;
        self.storage().get_package_import(import_id)
    }

    pub fn list_content_package_imports(
        &self,
        package_id: Option<&PackageId>,
    ) -> CoreResult<Vec<PackageImportRecord>> {
        self.storage().list_package_imports(package_id)
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

    /// Re-inspects the durable source, recreates every approved binding,
    /// streams selected assets, and commits the package atomically.
    pub fn commit_content_package_import(
        &self,
        import_id: &str,
        request: &ContentPackageCommitRequest,
    ) -> CoreResult<ContentPackageCommitReceipt> {
        let loaded = load_durable_content_package(self, import_id, ImportLimits::default())?;
        if !matches!(
            loaded.record.status,
            PackageImportStatus::Approved | PackageImportStatus::Completed
        ) {
            return Err(CoreError::invalid(
                "content package must be approved before commit",
            ));
        }
        let import_plan = stored_import_plan(&loaded.record)?;
        validate_package_transition_expectations(
            &loaded,
            &import_plan,
            &request.expected_package_plan_hash,
            &request.expected_content_selection_plan_hash,
            &request.expected_review_sha256,
            &request.expected_import_plan_sha256,
        )?;
        let (approval, approved_at) = stored_package_approval(self, import_id)?;
        if approval.plan.approval_sha256 != request.expected_approval_sha256
            || approval.plan.plan_sha256 != request.expected_import_plan_sha256
            || approval.plan.review_sha256 != request.expected_review_sha256
            || approval.approved_capabilities != required_capability_approvals(&import_plan)
            || approval.normalization_evidence_sha256
                != request.expected_normalization_evidence_sha256
            || approval.normalization_evidence_sha256
                != package_normalization_evidence_sha256(&approval.normalization_evidence)?
        {
            return Err(stale_package_review());
        }
        let replay_bindings = (loaded.record.status == PackageImportStatus::Completed)
            .then_some(approval.document_bindings.as_slice());
        let prepared = prepare_package_commit(
            self,
            &loaded,
            &import_plan,
            ImportLimits::default(),
            replay_bindings,
        )?;
        if approval.document_bindings != prepared.bindings
            || approval.plan.assets != prepared.assets
            || approval.normalization_evidence != prepared.normalization_evidence
            || package_normalization_evidence_sha256(&prepared.normalization_evidence)?
                != request.expected_normalization_evidence_sha256
        {
            return Err(stale_package_review());
        }
        let expected = PackageImportExpectation {
            revision: request.expected_revision,
            inspection_sha256: request.expected_review_sha256.as_str().to_owned(),
            selection_sha256: request.expected_import_plan_sha256.as_str().to_owned(),
            capability_review_sha256: request.expected_capability_review_sha256.clone(),
        };
        persist_prepared_package_commit(
            self,
            import_id,
            loaded,
            prepared,
            expected,
            approved_at,
            request.expected_revision,
        )
    }

    /// Discards either the unselected inspection or the exact selected plan.
    pub fn discard_content_package_import(
        &self,
        import_id: &str,
        request: &ContentPackageDiscardRequest,
    ) -> CoreResult<PackageImportRecord> {
        validate_import_id(import_id)?;
        let record = self.storage().get_package_import(import_id)?;
        let inspection = PackageInspectionExpectation {
            revision: request.expected_revision,
            inspection_sha256: request.expected_review_sha256.as_str().to_owned(),
            capability_review_sha256: request.expected_capability_review_sha256.clone(),
        };
        if record.selection.is_none() {
            if request.expected_import_plan_sha256.is_some() {
                return Err(stale_package_review());
            }
            self.storage()
                .discard_inspected_package_import(import_id, &inspection)
        } else {
            let selection = request
                .expected_import_plan_sha256
                .as_ref()
                .ok_or_else(stale_package_review)?;
            self.storage().discard_package_import(
                import_id,
                &PackageImportExpectation {
                    revision: request.expected_revision,
                    inspection_sha256: request.expected_review_sha256.as_str().to_owned(),
                    selection_sha256: selection.as_str().to_owned(),
                    capability_review_sha256: request.expected_capability_review_sha256.clone(),
                },
            )
        }
    }
}

fn persist_prepared_package_commit(
    core: &Core,
    import_id: &str,
    loaded: DurableContentPackageImport,
    prepared: PreparedPackageCommit,
    expected: PackageImportExpectation,
    approved_at: DateTime<Utc>,
    expected_revision: u64,
) -> CoreResult<ContentPackageCommitReceipt> {
    let mut approved_import = loaded.record.clone();
    approved_import.status = PackageImportStatus::Approved;
    approved_import.revision = expected_revision;
    approved_import.updated_at = approved_at;
    let input = PackageCommitInput {
        source: loaded.source,
        import: approved_import,
        documents: prepared.documents,
        assets: prepared.assets.clone(),
    };
    let staged_assets = if loaded.record.status == PackageImportStatus::Completed {
        Vec::new()
    } else {
        stage_selected_content_package_assets(
            &loaded.owned.path,
            ImportLimits::default(),
            &prepared.content_selection,
            &core.storage().staging_dir(),
        )?
    };
    let staged_imports = staged_assets
        .iter()
        .map(staged_asset_import)
        .collect::<Vec<_>>();
    let promotion_result = core
        .storage()
        .promote_package_assets(import_id, &staged_imports);
    let staging_cleanup =
        discard_staged_content_package_assets(&staged_assets, &core.storage().staging_dir());
    if let Err(error) = promotion_result {
        let cas_cleanup = core
            .storage()
            .discard_unclaimed_package_assets(import_id, &staged_imports);
        return Err(with_two_cleanup_errors(
            error,
            staging_cleanup,
            cas_cleanup.map(|_| ()),
        ));
    }
    if let Err(error) = staging_cleanup {
        let cas_cleanup = core
            .storage()
            .discard_unclaimed_package_assets(import_id, &staged_imports);
        return Err(with_cleanup_error(error, cas_cleanup.map(|_| ())));
    }
    let committed = core
        .storage()
        .commit_package_import(&input, &expected, &prepared.bindings)
        .map_err(|error| {
            let cleanup = core
                .storage()
                .discard_unclaimed_package_assets(import_id, &staged_imports);
            with_cleanup_error(error, cleanup.map(|_| ()))
        })?;
    Ok(ContentPackageCommitReceipt {
        import: committed,
        committed_document_ids: prepared
            .bindings
            .iter()
            .map(|binding| binding.target_object_id.clone())
            .collect(),
        asset_ids: prepared.assets.into_iter().map(|asset| asset.id).collect(),
    })
}

fn load_durable_content_package(
    core: &Core,
    import_id: &str,
    limits: ImportLimits,
) -> CoreResult<DurableContentPackageImport> {
    validate_import_id(import_id)?;
    let record = core.storage().get_package_import(import_id)?;
    if record.inspection.schema_version != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package inspection schema is unsupported",
            false,
        ));
    }
    let review: PackageReview =
        serde_json::from_value(record.inspection.value.clone()).map_err(|error| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                format!("stored package inspection cannot be decoded: {error}"),
                false,
            )
        })?;
    review.verify().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("stored package review is invalid: {error}"),
            false,
        )
    })?;
    let source = core.storage().get_package_source_for_import(import_id)?;
    if source.source_sha256 != review.source_sha256.as_str()
        || source.package_id != review.manifest.package_id
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package source and review identities disagree",
            false,
        ));
    }
    let source_path = core
        .storage()
        .package_source_path(&source.source_sha256, source.source_size_bytes)?;
    let owned = reopen_content_package(import_id, &source_path, &review, limits)?;
    Ok(DurableContentPackageImport {
        source,
        record,
        owned,
    })
}

fn stored_import_plan(record: &PackageImportRecord) -> CoreResult<SelectiveImportPlan> {
    let selection = record.selection.as_ref().ok_or_else(|| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package import has no selection",
            false,
        )
    })?;
    if selection.schema_version != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package selection schema is unsupported",
            false,
        ));
    }
    let plan: SelectiveImportPlan =
        serde_json::from_value(selection.value.clone()).map_err(|error| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                format!("stored package selection cannot be decoded: {error}"),
                false,
            )
        })?;
    plan.verify().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("stored package selection is invalid: {error}"),
            false,
        )
    })?;
    Ok(plan)
}

fn stored_package_approval(
    core: &Core,
    import_id: &str,
) -> CoreResult<(StoredPackageApprovalPayload, DateTime<Utc>)> {
    let approval = core.storage().get_package_import_approval(import_id)?;
    let approved_at = approval.approved_at;
    if approval.payload.schema_version != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored package approval schema is unsupported",
            false,
        ));
    }
    let payload: StoredPackageApprovalPayload = serde_json::from_value(approval.payload.value)
        .map_err(|error| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                format!("stored package approval cannot be decoded: {error}"),
                false,
            )
        })?;
    payload.plan.verify().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("stored package approval is invalid: {error}"),
            false,
        )
    })?;
    Ok((payload, approved_at))
}

fn target_review_bindings(
    target_review: &PackageImportTargetReview,
) -> CoreResult<Vec<PackageDocumentCommitBinding>> {
    target_review.verify()?;
    Ok(target_review
        .documents
        .iter()
        .map(|document| PackageDocumentCommitBinding {
            document_index: document.document_index,
            source_component_key: document.source_component_id.clone(),
            component_document_ordinal: document.component_document_ordinal,
            source_component_sha256: document.source_component_sha256.clone(),
            target_object_id: document.target_object_id.clone(),
            document_kind: document.document_kind.clone(),
            document_sha256: document.document_sha256.clone(),
            expected_object_revision: document.expected_target_state_revision,
        })
        .collect())
}

fn validate_package_transition_expectations(
    loaded: &DurableContentPackageImport,
    import_plan: &SelectiveImportPlan,
    expected_package_plan_hash: &str,
    expected_content_selection_plan_hash: &str,
    expected_review_sha256: &Sha256Digest,
    expected_import_plan_sha256: &Sha256Digest,
) -> CoreResult<()> {
    let content_selection = select_content_package_components(
        loaded.owned.inspection(),
        &loaded.record.selected_component_ids,
    )?;
    if loaded.owned.inspection.plan_hash != expected_package_plan_hash
        || content_selection.selection_plan_hash != expected_content_selection_plan_hash
        || loaded.owned.review.review_sha256 != *expected_review_sha256
        || import_plan.review_sha256 != *expected_review_sha256
        || import_plan.plan_sha256 != *expected_import_plan_sha256
        || import_plan.source_sha256.as_str() != loaded.source.source_sha256
    {
        return Err(stale_package_review());
    }
    Ok(())
}

fn prepare_package_commit(
    core: &Core,
    loaded: &DurableContentPackageImport,
    import_plan: &SelectiveImportPlan,
    limits: ImportLimits,
    replay_bindings: Option<&[PackageDocumentCommitBinding]>,
) -> CoreResult<PreparedPackageCommit> {
    let content_selection = select_content_package_components(
        loaded.owned.inspection(),
        &loaded.record.selected_component_ids,
    )?;
    let prepared = loaded.owned.prepare(
        &content_selection,
        &loaded.owned.inspection.plan_hash,
        &content_selection.selection_plan_hash,
        limits,
    )?;
    let mut normalization_evidence = prepared
        .transformations
        .iter()
        .map(|transformation| PackageNormalizationEvidence {
            component_id: transformation.component_id.clone(),
            object_id: transformation.object_id.clone(),
            field: transformation.field.clone(),
            before: transformation.before,
            after: transformation.after,
            reason: transformation.reason.clone(),
        })
        .collect::<Vec<_>>();
    normalization_evidence.sort();
    package_normalization_evidence_sha256(&normalization_evidence)?;
    let mut assets = prepared.assets;
    assets.sort_by(|left, right| left.id.cmp(&right.id));
    if assets != import_plan.assets {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "content and orchestration package asset inventories disagree",
            false,
        ));
    }
    let imported_provenance = loaded.owned.review.manifest.provenance.clone();
    let mut prepared_documents = prepared.documents;
    prepared_documents.sort_by(|left, right| {
        matches!(&left.document, PreparedContentDocument::ContentModule(_))
            .cmp(&matches!(
                &right.document,
                PreparedContentDocument::ContentModule(_)
            ))
            .then_with(|| {
                left.source_component_ordinal
                    .cmp(&right.source_component_ordinal)
            })
            .then_with(|| left.document_ordinal.cmp(&right.document_ordinal))
            .then_with(|| left.source_component_id.cmp(&right.source_component_id))
    });
    let redistribution_allowed = import_plan.redistribution_status == RedistributionStatus::Allowed;
    let (documents, bindings) = prepare_package_commit_documents(
        core,
        loaded,
        prepared_documents,
        &imported_provenance,
        redistribution_allowed,
        replay_bindings,
    )?;
    validate_content_module_package_bindings(&documents, &assets, import_plan)?;
    Ok(PreparedPackageCommit {
        content_selection,
        documents,
        assets,
        bindings,
        normalization_evidence,
    })
}

fn prepare_package_commit_documents(
    core: &Core,
    loaded: &DurableContentPackageImport,
    prepared_documents: Vec<PreparedContentDocumentEnvelope>,
    imported_provenance: &Provenance,
    redistribution_allowed: bool,
    replay_bindings: Option<&[PackageDocumentCommitBinding]>,
) -> CoreResult<(
    Vec<PackageCommitDocument>,
    Vec<PackageDocumentCommitBinding>,
)> {
    let mut documents = Vec::with_capacity(prepared_documents.len());
    let mut bindings = Vec::with_capacity(prepared_documents.len());
    for (index, envelope) in prepared_documents.into_iter().enumerate() {
        let source_component = loaded
            .owned
            .review
            .components
            .iter()
            .find(|component| component.id == envelope.source_component_id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "prepared package document has no reviewed source component",
                    false,
                )
            })?;
        let document = normalize_prepared_document(
            envelope.document,
            imported_provenance,
            redistribution_allowed,
        )?;
        if let PackageCommitDocument::PromptPreset(preset) = &document {
            core.validate_prompt_preset(preset)?;
        }
        let (document_kind, target_object_id) = package_document_identity(&document);
        if envelope.document_kind != document_kind || envelope.document_id != target_object_id {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "normalized package document changed its reviewed identity",
                false,
            ));
        }
        let document_json = serde_json::to_vec(&document).map_err(package_json_error)?;
        let document_sha256 = format!("{:x}", Sha256::digest(&document_json));
        let document_index = u32::try_from(index)
            .map_err(|_| CoreError::invalid("package contains too many documents"))?;
        let expected_object_revision = if let Some(replay_bindings) = replay_bindings {
            let approved = replay_bindings
                .iter()
                .find(|binding| binding.document_index == document_index)
                .ok_or_else(stale_package_review)?;
            if approved.source_component_key != envelope.source_component_id
                || approved.component_document_ordinal != envelope.document_ordinal
                || approved.source_component_sha256 != source_component.sha256.as_str()
                || approved.target_object_id != target_object_id
                || approved.document_kind != document_kind
                || approved.document_sha256 != document_sha256
            {
                return Err(stale_package_review());
            }
            approved.expected_object_revision
        } else {
            expected_document_revision(core, &document)?
        };
        bindings.push(PackageDocumentCommitBinding {
            document_index,
            source_component_key: envelope.source_component_id,
            component_document_ordinal: envelope.document_ordinal,
            source_component_sha256: source_component.sha256.as_str().to_owned(),
            target_object_id: target_object_id.to_owned(),
            document_kind: document_kind.to_owned(),
            document_sha256,
            expected_object_revision,
        });
        documents.push(document);
    }
    Ok((documents, bindings))
}

fn required_capability_approvals(plan: &SelectiveImportPlan) -> Vec<PackageCapability> {
    let mut approvals = plan
        .required_capabilities
        .iter()
        .filter_map(|capability| match capability {
            ContentCapability::Transforms => Some(PackageCapability::Transforms),
            ContentCapability::DeclarativeInteractions => {
                Some(PackageCapability::DeclarativeInteractions)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    approvals.sort_unstable();
    approvals.dedup();
    approvals
}

fn normalize_prepared_document(
    document: PreparedContentDocument,
    imported_provenance: &Provenance,
    redistribution_allowed: bool,
) -> CoreResult<PackageCommitDocument> {
    match document {
        PreparedContentDocument::PromptPreset(preset) => {
            let mut preset = *preset;
            if built_in_prompt_presets()
                .iter()
                .any(|built_in| built_in.id == preset.id)
            {
                return Err(CoreError::invalid(
                    "imported packages cannot replace built-in prompt presets",
                ));
            }
            preset.metadata.provenance = imported_provenance.clone();
            for block in &mut preset.blocks {
                block.provenance = imported_provenance.clone();
                block.authority = InstructionAuthority::ImportedContent;
            }
            crate::orchestration::enforce_application_policy(&mut preset);
            Ok(PackageCommitDocument::PromptPreset(preset))
        }
        PreparedContentDocument::KnowledgeBook(book) => {
            let mut book = *book;
            book.provenance = imported_provenance.clone();
            for entry in &mut book.entries {
                entry.provenance = imported_provenance.clone();
            }
            book.validate().map_err(|error| {
                CoreError::invalid(format!("invalid imported knowledge book: {error}"))
            })?;
            Ok(PackageCommitDocument::KnowledgeBook(book))
        }
        PreparedContentDocument::MemoryProfile(profile) => {
            let mut profile = *profile;
            profile.provenance = imported_provenance.clone();
            profile.validate().map_err(|error| {
                CoreError::invalid(format!("invalid imported memory profile: {error}"))
            })?;
            Ok(PackageCommitDocument::MemoryProfile(profile))
        }
        PreparedContentDocument::TransformSet(set) => {
            let mut set = *set;
            set.provenance = imported_provenance.clone();
            set.enabled = false;
            for rule in &mut set.rules {
                rule.provenance = imported_provenance.clone();
                rule.enabled = false;
                rule.imported_enabled = false;
            }
            Ok(PackageCommitDocument::TransformSet(set))
        }
        PreparedContentDocument::InteractionRuleSet(set) => {
            let mut set = *set;
            set.provenance = imported_provenance.clone();
            for rule in &mut set.rules {
                rule.provenance = imported_provenance.clone();
                rule.enabled = false;
            }
            Ok(PackageCommitDocument::InteractionRuleSet(set))
        }
        PreparedContentDocument::ContentModule(module) => {
            let mut module = *module;
            if module.schema_version != 1 {
                return Err(CoreError::invalid(
                    "imported content module schema_version must be 1",
                ));
            }
            module.metadata.provenance = imported_provenance.clone();
            module
                .metadata
                .author
                .clone_from(&imported_provenance.author);
            module.metadata.license = imported_provenance
                .license
                .clone()
                .unwrap_or_else(|| "UNKNOWN".to_owned());
            module.metadata.redistribution_allowed = redistribution_allowed;
            for (index, block) in module.prompt_fragments.iter_mut().enumerate() {
                if block.kind == PromptBlockKind::LatestUserTurn
                    || block.source == BlockSource::LatestUser
                    || matches!(
                        block.placement_zone,
                        PlacementZone::ApplicationPolicy | PlacementZone::LatestUser
                    )
                {
                    return Err(CoreError::invalid(format!(
                        "imported content module prompt_fragments[{index}] uses a reserved application or latest-user block",
                    )));
                }
                block.authority = InstructionAuthority::ImportedContent;
                block.provenance = imported_provenance.clone();
            }
            module.validate().map_err(|error| {
                CoreError::invalid(format!("invalid imported content module: {error}"))
            })?;
            Ok(PackageCommitDocument::ContentModule(module))
        }
    }
}

fn validate_content_module_package_bindings(
    documents: &[PackageCommitDocument],
    assets: &[AssetDescriptor],
    import_plan: &SelectiveImportPlan,
) -> CoreResult<()> {
    let knowledge_ids = documents
        .iter()
        .filter_map(|document| match document {
            PackageCommitDocument::KnowledgeBook(value) => Some(value.id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let transform_ids = documents
        .iter()
        .filter_map(|document| match document {
            PackageCommitDocument::TransformSet(value) => Some(value.id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let interaction_ids = documents
        .iter()
        .filter_map(|document| match document {
            PackageCommitDocument::InteractionRuleSet(value) => Some(value.id.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let assets_by_id = assets
        .iter()
        .map(|asset| (&asset.id, asset))
        .collect::<BTreeMap<_, _>>();
    let approved_capabilities = import_plan
        .required_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();

    for module in documents.iter().filter_map(|document| match document {
        PackageCommitDocument::ContentModule(value) => Some(value),
        _ => None,
    }) {
        validate_content_module_package_binding(
            module,
            &knowledge_ids,
            &transform_ids,
            &interaction_ids,
            &assets_by_id,
            &approved_capabilities,
        )?;
    }
    Ok(())
}

fn validate_content_module_package_binding(
    module: &lorepia_domain::ContentModule,
    knowledge_ids: &BTreeSet<&str>,
    transform_ids: &BTreeSet<&str>,
    interaction_ids: &BTreeSet<&str>,
    assets_by_id: &BTreeMap<&AssetId, &AssetDescriptor>,
    approved_capabilities: &BTreeSet<ContentCapability>,
) -> CoreResult<()> {
    module
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid imported content module: {error}")))?;
    let missing_link = module
        .knowledge_book_ids
        .iter()
        .map(|id| {
            (
                "knowledge book",
                id.as_str(),
                knowledge_ids.contains(id.as_str()),
            )
        })
        .chain(module.transform_set_ids.iter().map(|id| {
            (
                "transform set",
                id.as_str(),
                transform_ids.contains(id.as_str()),
            )
        }))
        .chain(module.interaction_rule_set_ids.iter().map(|id| {
            (
                "interaction rule set",
                id.as_str(),
                interaction_ids.contains(id.as_str()),
            )
        }))
        .find(|(_, _, present)| !present);
    if let Some((kind, id, _)) = missing_link {
        return Err(CoreError::invalid(format!(
            "content module {} references a {kind} outside the approved package selection: {id}",
            module.id.as_str()
        )));
    }

    let declared = module
        .required_capabilities
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut required = BTreeSet::new();
    required.extend(
        (!module.prompt_fragments.is_empty()).then_some(ContentCapability::PromptFragments),
    );
    required
        .extend((!module.knowledge_book_ids.is_empty()).then_some(ContentCapability::Knowledge));
    required.extend((!module.control_specs.is_empty()).then_some(ContentCapability::Variables));
    required
        .extend((!module.transform_set_ids.is_empty()).then_some(ContentCapability::Transforms));
    required.extend(
        (!module.interaction_rule_set_ids.is_empty())
            .then_some(ContentCapability::DeclarativeInteractions),
    );
    for asset_id in &module.asset_ids {
        let asset = assets_by_id.get(asset_id).ok_or_else(|| {
            CoreError::invalid(format!(
                "content module {} references an asset outside the approved package selection: {}",
                module.id.as_str(),
                asset_id.as_str()
            ))
        })?;
        required.insert(asset_capability(&asset.media_type));
    }
    if let Some(missing) = required
        .iter()
        .find(|capability| !declared.contains(capability))
    {
        return Err(CoreError::invalid(format!(
            "content module {} omits required capability {missing:?}",
            module.id.as_str()
        )));
    }
    if let Some(unapproved) = declared
        .iter()
        .find(|capability| !approved_capabilities.contains(capability))
    {
        return Err(CoreError::invalid(format!(
            "content module {} capability was not part of the approved import plan: {unapproved:?}",
            module.id.as_str()
        )));
    }
    Ok(())
}

fn package_document_identity(document: &PackageCommitDocument) -> (&'static str, &str) {
    match document {
        PackageCommitDocument::PromptPreset(value) => ("prompt_preset", value.id.as_str()),
        PackageCommitDocument::KnowledgeBook(value) => ("knowledge_book", value.id.as_str()),
        PackageCommitDocument::MemoryProfile(value) => ("memory_profile", value.id.as_str()),
        PackageCommitDocument::TransformSet(value) => ("transform_set", value.id.as_str()),
        PackageCommitDocument::InteractionRuleSet(value) => {
            ("interaction_rule_set", value.id.as_str())
        }
        PackageCommitDocument::ContentModule(value) => ("content_module", value.id.as_str()),
        PackageCommitDocument::CharacterContent { character_id, .. } => {
            ("character_content", character_id)
        }
    }
}

fn expected_document_revision(
    core: &Core,
    document: &PackageCommitDocument,
) -> CoreResult<Option<u64>> {
    let result = match document {
        PackageCommitDocument::PromptPreset(value) => core
            .storage()
            .get_prompt_preset(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::KnowledgeBook(value) => core
            .storage()
            .get_knowledge_book(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::MemoryProfile(value) => core
            .storage()
            .get_memory_profile(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::TransformSet(value) => core
            .storage()
            .get_transform_set(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::InteractionRuleSet(value) => core
            .storage()
            .get_interaction_rule_set(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::ContentModule(value) => core
            .storage()
            .get_content_module(&value.id)
            .map(|value| value.revision),
        PackageCommitDocument::CharacterContent { .. } => {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "unsupported document kind reached a content package commit",
                false,
            ));
        }
    };
    match result {
        Ok(revision) => Ok(Some(revision)),
        Err(error) if error.code == CoreErrorCode::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn staged_asset_import(asset: &StagedContentPackageAsset) -> StagedAssetImport {
    StagedAssetImport {
        staged_path: asset.staged_path.clone(),
        sha256: asset.descriptor.sha256.as_str().to_owned(),
        media_type: asset.descriptor.media_type.clone(),
        size_bytes: asset.descriptor.size_bytes,
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
