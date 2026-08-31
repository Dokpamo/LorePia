//! Core-owned staging and approval boundary for `LorePia` content packages.
//!
//! Native callers may provide a source path exactly once, during inspection.
//! Every later operation accepts only the opaque import identifier and
//! hash/revision expectations returned by this module. Reviewed bytes are
//! promoted to Core-owned content-addressed storage before durable inspection
//! state is created and are re-inspected immediately before later transitions.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use lorepia_content::{
    ContentCapability as InspectedContentCapability, ContentPackageComponentKind,
    ContentPackageComponentState, ContentPackageInspection, ContentPackageSelectionPlan,
    PreparedContentDocument, PreparedContentDocumentEnvelope, PreparedContentPackageImport,
    StagedContentPackageAsset, discard_staged_content_package_assets, inspect_content_package,
    prepare_content_package_import, select_content_package_components,
    stage_selected_content_package_assets,
};
use lorepia_domain::{
    AssetDescriptor, AssetId, AssetRole, AssetSource, AssetSourceKind, BlockSource,
    ContentCapability, CoreError, CoreErrorCode, CoreResult, ImportLimits, InspectionId,
    InstructionAuthority, PackageContentHash, PackageId, PackageManifest, PlacementZone,
    PromptBlockKind, Provenance, Sha256Digest, SourceKind, ValidateOrchestration, VersionedJson,
};
use lorepia_orchestration::{
    ApprovedPackageImportPlan, ObservedPackageEntry, PackageComponentDescriptor,
    PackageComponentDisposition, PackageComponentKind, PackageImportApproval,
    PackageInspectionSnapshot, PackageReview, PackageSelectionRequest, PackageValidationPolicy,
    RedistributionStatus, SelectiveImportPlan, SignatureVerification,
    approve_selective_import_plan, build_selective_import_plan, validate_package_snapshot,
};
use lorepia_storage::{
    CompletedPackageAuthority, PackageCapability, PackageCapabilityDecision,
    PackageCapabilityReview, PackageCapabilitySupport, PackageCommitDocument, PackageCommitInput,
    PackageDocumentCommitBinding, PackageImportExpectation, PackageImportRecord,
    PackageImportStatus, PackageImportTargetReview, PackageInspectionExpectation,
    PackageNormalizationEvidence, PackageSourceRecord, PackageUpdateTargetConfirmation,
    StagedAssetImport, built_in_prompt_presets, package_capability_review_sha256,
    package_normalization_evidence_sha256, package_update_target_confirmations_sha256,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::Core;

const PACKAGE_SNAPSHOT_PREFIX: &str = "content-package-";
const PACKAGE_SNAPSHOT_SUFFIX: &str = ".snapshot";
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// Native-safe result of inspecting a Core-owned content-package snapshot.
///
/// No host path is returned. The identifier is opaque to native callers and
/// may only be passed back to the package import methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageImportInspection {
    pub import_id: String,
    pub revision: u64,
    pub inspection: ContentPackageInspection,
    pub review: PackageReview,
    pub capability_review: PackageCapabilityReview,
    pub capability_review_sha256: String,
}

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

#[derive(Debug)]
pub(crate) struct OwnedContentPackageSnapshot {
    import_id: String,
    path: PathBuf,
    inspection: ContentPackageInspection,
    review: PackageReview,
}

impl OwnedContentPackageSnapshot {
    pub(crate) fn public_inspection(
        &self,
        revision: u64,
        capability_review: PackageCapabilityReview,
    ) -> CoreResult<ContentPackageImportInspection> {
        let capability_review_sha256 = package_capability_review_sha256(&capability_review)?;
        Ok(ContentPackageImportInspection {
            import_id: self.import_id.clone(),
            revision,
            inspection: self.inspection.clone(),
            review: self.review.clone(),
            capability_review,
            capability_review_sha256,
        })
    }

    pub(crate) fn inspection(&self) -> &ContentPackageInspection {
        &self.inspection
    }

    pub(crate) fn review(&self) -> &PackageReview {
        &self.review
    }

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

    pub(crate) fn discard(self, staging_dir: &Path) -> CoreResult<()> {
        remove_owned_snapshot(&self.path, staging_dir, &self.import_id)
    }
}

impl Core {
    /// Inspects a one-shot caller path, promotes its exact bytes into durable
    /// Core-owned CAS, and creates the first immutable review revision.
    pub fn inspect_content_package_import(
        &self,
        source_path: &Path,
    ) -> CoreResult<ContentPackageImportInspection> {
        self.inspect_content_package_import_with_limits(source_path, ImportLimits::default())
    }

    fn inspect_content_package_import_with_limits(
        &self,
        source_path: &Path,
        limits: ImportLimits,
    ) -> CoreResult<ContentPackageImportInspection> {
        let storage = self.storage();
        let staging_dir = storage.staging_dir();
        let mut staged = stage_content_package(source_path, &staging_dir, limits)?;
        let existing_source =
            match storage.get_package_source_by_hash(&staged.inspection.source_sha256) {
                Ok(source) => Some(source),
                Err(error) if error.code == CoreErrorCode::NotFound => None,
                Err(error) => {
                    let cleanup = staged.discard(&staging_dir);
                    return Err(with_cleanup_error(error, cleanup));
                }
            };
        let imported_at = existing_source
            .as_ref()
            .map_or_else(Utc::now, |source| source.created_at);
        staged.review = match review_content_inspection(&staged.inspection, imported_at) {
            Ok(review) => review,
            Err(error) => {
                let cleanup = staged.discard(&staging_dir);
                return Err(with_cleanup_error(error, cleanup));
            }
        };

        let import_id = staged.import_id.clone();
        let source_sha256 = staged.inspection.source_sha256.clone();
        let source_size = staged.inspection.source_size;
        let durable_path = match storage.promote_package_source(
            &import_id,
            &staged.path,
            &source_sha256,
            source_size,
        ) {
            Ok(path) => path,
            Err(error) => {
                let cleanup = staged.discard(&staging_dir);
                return Err(with_cleanup_error(error, cleanup));
            }
        };
        let durable =
            match reopen_content_package(&import_id, &durable_path, &staged.review, limits) {
                Ok(owned) => owned,
                Err(error) => {
                    let staging_cleanup = staged.discard(&staging_dir);
                    let source_cleanup = storage.discard_unclaimed_package_source(
                        &import_id,
                        &source_sha256,
                        source_size,
                    );
                    return Err(with_two_cleanup_errors(
                        error,
                        staging_cleanup,
                        source_cleanup.map(|_| ()),
                    ));
                }
            };
        if let Err(error) = staged.discard(&staging_dir) {
            let source_cleanup =
                storage.discard_unclaimed_package_source(&import_id, &source_sha256, source_size);
            return Err(with_cleanup_error(error, source_cleanup.map(|_| ())));
        }

        let source = if let Some(source) = existing_source {
            source
        } else {
            package_source_record(&durable.review, source_size, imported_at)?
        };
        let capability_review = package_capability_review(&durable.review);
        let now = Utc::now();
        let import = PackageImportRecord {
            id: import_id.clone(),
            package_id: source.package_id.clone(),
            status: PackageImportStatus::Inspected,
            revision: 1,
            inspection: VersionedJson {
                schema_version: 1,
                value: serde_json::to_value(&durable.review).map_err(package_json_error)?,
            },
            selection: None,
            selected_component_ids: Vec::new(),
            failure_code: None,
            created_at: now,
            updated_at: now,
        };
        let stored = match storage.create_inspected_package_import(
            &source,
            &import,
            &durable.review,
            &capability_review,
        ) {
            Ok(record) => record,
            Err(error) => {
                let cleanup = storage.discard_unclaimed_package_source(
                    &import_id,
                    &source_sha256,
                    source_size,
                );
                return Err(with_cleanup_error(error, cleanup.map(|_| ())));
            }
        };
        durable.public_inspection(stored.revision, capability_review)
    }

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

fn package_source_record(
    review: &PackageReview,
    source_size_bytes: u64,
    created_at: DateTime<Utc>,
) -> CoreResult<PackageSourceRecord> {
    Ok(PackageSourceRecord {
        id: format!("package-source-{}", review.source_sha256),
        package_id: review.manifest.package_id.clone(),
        format: review.manifest.format.clone(),
        format_version: review.manifest.format_version,
        name: review.manifest.name.clone(),
        version: review.manifest.version.clone(),
        source_sha256: review.source_sha256.as_str().to_owned(),
        source_size_bytes,
        author: review.manifest.author.clone(),
        license: review.manifest.license.clone(),
        redistribution_allowed: review.manifest.redistribution_allowed,
        manifest: VersionedJson {
            schema_version: 1,
            value: serde_json::to_value(&review.manifest).map_err(package_json_error)?,
        },
        created_at,
    })
}

fn package_capability_review(_review: &PackageReview) -> PackageCapabilityReview {
    const SUPPORTED_REASON: &str =
        "declarative capability is supported behind the Rust package boundary";
    const APPROVAL_REASON: &str =
        "declarative behavior remains inactive until this capability is explicitly approved";
    const UNSUPPORTED_REASON: &str =
        "executable, external, privileged, or high-risk package capability is unsupported";
    let decisions = [
        (
            PackageCapability::PromptFragments,
            PackageCapabilitySupport::Supported,
        ),
        (
            PackageCapability::Knowledge,
            PackageCapabilitySupport::Supported,
        ),
        (
            PackageCapability::Variables,
            PackageCapabilitySupport::Supported,
        ),
        (
            PackageCapability::Transforms,
            PackageCapabilitySupport::ApprovalRequired,
        ),
        (
            PackageCapability::DeclarativeInteractions,
            PackageCapabilitySupport::ApprovalRequired,
        ),
        (
            PackageCapability::ImageAssets,
            PackageCapabilitySupport::Supported,
        ),
        (
            PackageCapability::AudioAssets,
            PackageCapabilitySupport::Supported,
        ),
        (
            PackageCapability::VideoAssets,
            PackageCapabilitySupport::Supported,
        ),
        (
            PackageCapability::AttachmentAssets,
            PackageCapabilitySupport::Supported,
        ),
        (
            PackageCapability::HighRiskAssets,
            PackageCapabilitySupport::Unsupported,
        ),
        (
            PackageCapability::ExternalUrls,
            PackageCapabilitySupport::Unsupported,
        ),
        (
            PackageCapability::Html,
            PackageCapabilitySupport::Unsupported,
        ),
        (
            PackageCapability::Script,
            PackageCapabilitySupport::Unsupported,
        ),
        (
            PackageCapability::NativeCode,
            PackageCapabilitySupport::Unsupported,
        ),
        (
            PackageCapability::Network,
            PackageCapabilitySupport::Unsupported,
        ),
        (
            PackageCapability::Filesystem,
            PackageCapabilitySupport::Unsupported,
        ),
        (
            PackageCapability::Shell,
            PackageCapabilitySupport::Unsupported,
        ),
        (
            PackageCapability::Credentials,
            PackageCapabilitySupport::Unsupported,
        ),
    ]
    .into_iter()
    .map(|(capability, support)| PackageCapabilityDecision {
        capability,
        support,
        approved: false,
        reason: match support {
            PackageCapabilitySupport::Supported => SUPPORTED_REASON,
            PackageCapabilitySupport::ApprovalRequired => APPROVAL_REASON,
            PackageCapabilitySupport::Unsupported => UNSUPPORTED_REASON,
        }
        .to_owned(),
    })
    .collect();
    PackageCapabilityReview {
        schema_version: 1,
        decisions,
    }
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

fn package_json_error(error: serde_json::Error) -> CoreError {
    CoreError::invalid(format!("package review cannot be encoded: {error}"))
}

fn package_plan_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!("package selection or approval is invalid: {error}"))
}

fn with_cleanup_error(primary: CoreError, cleanup: CoreResult<()>) -> CoreError {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) => CoreError::new(
            primary.code,
            format!(
                "{}; compensating cleanup also failed: {}",
                primary.message, cleanup.message
            ),
            primary.recoverable || cleanup.recoverable,
        ),
    }
}

fn with_two_cleanup_errors(
    primary: CoreError,
    first: CoreResult<()>,
    second: CoreResult<()>,
) -> CoreError {
    with_cleanup_error(with_cleanup_error(primary, first), second)
}

pub(crate) fn stage_content_package(
    source_path: &Path,
    staging_dir: &Path,
    limits: ImportLimits,
) -> CoreResult<OwnedContentPackageSnapshot> {
    let import_id = Uuid::new_v4().hyphenated().to_string();
    let snapshot = package_snapshot_path(staging_dir, &import_id)?;
    copy_regular_file_bounded(source_path, &snapshot, limits.max_source_bytes)?;
    let mut inspection = match inspect_content_package(&snapshot, limits) {
        Ok(inspection) => inspection,
        Err(error) => {
            let _ = remove_owned_snapshot(&snapshot, staging_dir, &import_id);
            return Err(error);
        }
    };
    inspection.id = InspectionId(import_id.clone());
    let review = review_content_inspection(&inspection, Utc::now())?;
    Ok(OwnedContentPackageSnapshot {
        import_id,
        path: snapshot,
        inspection,
        review,
    })
}

pub(crate) fn reopen_content_package(
    import_id: &str,
    durable_source_path: &Path,
    expected_review: &PackageReview,
    limits: ImportLimits,
) -> CoreResult<OwnedContentPackageSnapshot> {
    validate_import_id(import_id)?;
    let mut inspection = inspect_content_package(durable_source_path, limits)?;
    inspection.id = InspectionId(import_id.to_owned());
    if inspection.source_sha256 != expected_review.source_sha256.as_str() {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "durable content package source no longer matches its reviewed digest",
            false,
        ));
    }
    let imported_at = expected_review
        .manifest
        .provenance
        .imported_at
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored package review has no import timestamp",
                false,
            )
        })?;
    let review = review_content_inspection(&inspection, imported_at)?;
    if review != *expected_review {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "content package review changed before commit",
            false,
        ));
    }
    Ok(OwnedContentPackageSnapshot {
        import_id: import_id.to_owned(),
        path: durable_source_path.to_path_buf(),
        inspection,
        review,
    })
}

#[cfg(test)]
pub(crate) fn discard_content_package_snapshot(
    import_id: &str,
    staging_dir: &Path,
) -> CoreResult<()> {
    let snapshot = package_snapshot_path(staging_dir, import_id)?;
    remove_owned_snapshot(&snapshot, staging_dir, import_id)
}

fn package_snapshot_path(staging_dir: &Path, import_id: &str) -> CoreResult<PathBuf> {
    validate_import_id(import_id)?;
    Ok(staging_dir.join(format!(
        "{PACKAGE_SNAPSHOT_PREFIX}{import_id}{PACKAGE_SNAPSHOT_SUFFIX}"
    )))
}

fn validate_import_id(import_id: &str) -> CoreResult<()> {
    let parsed = Uuid::parse_str(import_id)
        .map_err(|_| CoreError::invalid("content package import id is invalid"))?;
    if parsed.hyphenated().to_string() != import_id {
        return Err(CoreError::invalid(
            "content package import id is not canonical",
        ));
    }
    Ok(())
}

fn review_content_inspection(
    inspection: &ContentPackageInspection,
    imported_at: DateTime<Utc>,
) -> CoreResult<PackageReview> {
    let snapshot = orchestration_snapshot(inspection, imported_at)?;
    let review = validate_package_snapshot(&snapshot, &PackageValidationPolicy::default())
        .map_err(package_validation_error)?;
    review.verify().map_err(package_validation_error)?;
    if inspection.is_allowed() != review.local_import_allowed {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "content and orchestration package reviews disagree",
            false,
        ));
    }
    Ok(review)
}

fn orchestration_snapshot(
    inspection: &ContentPackageInspection,
    imported_at: DateTime<Utc>,
) -> CoreResult<PackageInspectionSnapshot> {
    let source_sha256 =
        Sha256Digest::parse(inspection.source_sha256.clone()).map_err(package_hash_error)?;
    let manifest = snapshot_manifest(inspection, imported_at)?;
    let SnapshotComponents {
        components,
        assets,
        observed_entries,
    } = snapshot_components(inspection, &source_sha256)?;
    Ok(PackageInspectionSnapshot {
        source_sha256,
        source_size_bytes: inspection.source_size,
        manifest,
        signature_verification: if inspection.manifest.signature_present {
            SignatureVerification::Unsupported
        } else {
            SignatureVerification::Absent
        },
        components,
        assets,
        observed_entries,
    })
}

struct SnapshotComponents {
    components: Vec<PackageComponentDescriptor>,
    assets: Vec<AssetDescriptor>,
    observed_entries: Vec<ObservedPackageEntry>,
}

fn snapshot_manifest(
    inspection: &ContentPackageInspection,
    imported_at: DateTime<Utc>,
) -> CoreResult<PackageManifest> {
    let author =
        (!inspection.manifest.author.trim().is_empty()).then(|| inspection.manifest.author.clone());
    let provenance = Provenance {
        source_kind: SourceKind::ImportedPackage,
        source_id: Some(inspection.manifest.package_id.clone()),
        source_hash: Some(inspection.source_sha256.clone()),
        author: author.clone(),
        license: Some(inspection.manifest.license.clone()),
        imported_at: Some(imported_at),
    };
    let mut content_hashes = Vec::with_capacity(inspection.manifest.content_hashes.len());
    for (logical_path, sha256) in &inspection.manifest.content_hashes {
        let component = inspection
            .components
            .iter()
            .find(|component| component.path == *logical_path)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::UnsafeArchive,
                    "manifest hash has no inspected component",
                    false,
                )
            })?;
        content_hashes.push(PackageContentHash {
            logical_path: logical_path.clone(),
            sha256: Sha256Digest::parse(sha256.clone()).map_err(package_hash_error)?,
            size_bytes: component.size_bytes,
        });
    }
    let mut manifest_capabilities = inspection
        .manifest
        .required_capabilities
        .iter()
        .filter_map(map_content_capability)
        .collect::<Vec<_>>();
    if inspection
        .manifest
        .required_capabilities
        .iter()
        .any(|capability| capability.0 == "media_assets")
    {
        manifest_capabilities.extend(
            inspection
                .components
                .iter()
                .filter(|component| component.kind == ContentPackageComponentKind::Asset)
                .filter_map(component_kind_capability),
        );
    }
    manifest_capabilities.sort();
    manifest_capabilities.dedup();
    Ok(PackageManifest {
        format: inspection.manifest.format.clone(),
        format_version: inspection.manifest.format_version,
        package_id: PackageId::from(inspection.manifest.package_id.clone()),
        name: inspection.manifest.name.clone(),
        version: inspection.manifest.version.clone(),
        author,
        license: inspection.manifest.license.clone(),
        redistribution_allowed: inspection.manifest.redistribution_allowed,
        required_app_version: inspection.manifest.required_app_version.clone(),
        required_capabilities: manifest_capabilities,
        content_hashes,
        signature: None,
        provenance,
    })
}

fn snapshot_components(
    inspection: &ContentPackageInspection,
    source_sha256: &Sha256Digest,
) -> CoreResult<SnapshotComponents> {
    let mut assets = Vec::new();
    let mut components = Vec::with_capacity(inspection.components.len());
    let mut observed_entries = Vec::with_capacity(inspection.components.len());
    for component in &inspection.components {
        let sha256 = Sha256Digest::parse(component.sha256.clone()).map_err(package_hash_error)?;
        let mut asset_ids = Vec::new();
        if component.kind == ContentPackageComponentKind::Asset {
            let asset_id = AssetId::from(format!("sha256:{}", component.sha256));
            asset_ids.push(asset_id.clone());
            assets.push(AssetDescriptor {
                id: asset_id,
                sha256: sha256.clone(),
                media_type: component.media_type.clone(),
                role: asset_role(&component.media_type),
                name: component
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&component.path)
                    .to_owned(),
                size_bytes: component.size_bytes,
                width: None,
                height: None,
                duration_ms: None,
                source: AssetSource {
                    kind: AssetSourceKind::LorepiaPackage,
                    source_sha256: Some(source_sha256.clone()),
                    logical_path: Some(component.path.clone()),
                },
            });
        } else if component.kind == ContentPackageComponentKind::ContentModule {
            asset_ids.extend(component.referenced_asset_ids.iter().cloned());
        }
        let mut required_capabilities = component
            .required_capabilities
            .iter()
            .filter_map(|capability| {
                if capability.0 == "media_assets" {
                    component_kind_capability(component)
                } else {
                    map_content_capability(capability)
                }
            })
            .collect::<Vec<_>>();
        if let Some(capability) = component_kind_capability(component) {
            required_capabilities.push(capability);
        }
        required_capabilities.sort();
        required_capabilities.dedup();
        components.push(PackageComponentDescriptor {
            id: component.id.clone(),
            kind: package_component_kind(component.kind),
            logical_path: component.path.clone(),
            sha256: sha256.clone(),
            dependencies: component.depends_on.clone(),
            conflicts_with: component.conflicts_with.clone(),
            required_capabilities,
            asset_ids,
            disposition: package_component_disposition(component.state),
        });
        observed_entries.push(ObservedPackageEntry {
            logical_path: component.path.clone(),
            sha256,
            size_bytes: component.size_bytes,
        });
    }
    Ok(SnapshotComponents {
        components,
        assets,
        observed_entries,
    })
}

fn map_content_capability(capability: &InspectedContentCapability) -> Option<ContentCapability> {
    match capability.0.as_str() {
        "prompt_presets" => Some(ContentCapability::PromptFragments),
        "knowledge_books" => Some(ContentCapability::Knowledge),
        "safe_transforms" => Some(ContentCapability::Transforms),
        "declarative_interactions" => Some(ContentCapability::DeclarativeInteractions),
        "variables" => Some(ContentCapability::Variables),
        "image_assets" => Some(ContentCapability::ImageAssets),
        "audio_assets" => Some(ContentCapability::AudioAssets),
        "video_assets" => Some(ContentCapability::VideoAssets),
        "attachment_assets" => Some(ContentCapability::AttachmentAssets),
        "high_risk_assets" => Some(ContentCapability::HighRiskAssets),
        _ => None,
    }
}

fn component_kind_capability(
    component: &lorepia_content::ContentPackageComponent,
) -> Option<ContentCapability> {
    match component.kind {
        ContentPackageComponentKind::Prompt => Some(ContentCapability::PromptFragments),
        ContentPackageComponentKind::Knowledge => Some(ContentCapability::Knowledge),
        ContentPackageComponentKind::Transform => Some(ContentCapability::Transforms),
        ContentPackageComponentKind::Interaction => {
            Some(ContentCapability::DeclarativeInteractions)
        }
        ContentPackageComponentKind::ContentModule
        | ContentPackageComponentKind::Memory
        | ContentPackageComponentKind::Unsupported => None,
        ContentPackageComponentKind::Asset => Some(asset_capability(&component.media_type)),
    }
}

fn asset_capability(media_type: &str) -> ContentCapability {
    if media_type.starts_with("image/") {
        ContentCapability::ImageAssets
    } else if media_type.starts_with("audio/") {
        ContentCapability::AudioAssets
    } else if media_type.starts_with("video/") {
        ContentCapability::VideoAssets
    } else {
        ContentCapability::AttachmentAssets
    }
}

const fn package_component_kind(kind: ContentPackageComponentKind) -> PackageComponentKind {
    match kind {
        ContentPackageComponentKind::Prompt => PackageComponentKind::PromptPreset,
        ContentPackageComponentKind::Knowledge => PackageComponentKind::KnowledgeBook,
        ContentPackageComponentKind::Memory => PackageComponentKind::MemoryProfile,
        ContentPackageComponentKind::Transform => PackageComponentKind::TransformSet,
        ContentPackageComponentKind::Interaction => PackageComponentKind::InteractionRuleSet,
        ContentPackageComponentKind::ContentModule => PackageComponentKind::ContentModule,
        ContentPackageComponentKind::Asset => PackageComponentKind::AssetIndex,
        ContentPackageComponentKind::Unsupported => PackageComponentKind::RawExtension,
    }
}

const fn package_component_disposition(
    state: ContentPackageComponentState,
) -> PackageComponentDisposition {
    match state {
        ContentPackageComponentState::Selectable => PackageComponentDisposition::Importable,
        ContentPackageComponentState::InactiveUnsupported => {
            PackageComponentDisposition::Unsupported
        }
        ContentPackageComponentState::Quarantined => PackageComponentDisposition::Quarantined,
    }
}

fn asset_role(media_type: &str) -> AssetRole {
    if media_type.starts_with("image/") {
        AssetRole::Illustration
    } else if media_type.starts_with("audio/") {
        AssetRole::Audio
    } else if media_type.starts_with("video/") {
        AssetRole::Video
    } else {
        AssetRole::Attachment
    }
}

fn package_hash_error(message: String) -> CoreError {
    CoreError::new(
        CoreErrorCode::UnsafeArchive,
        format!("content package contains an invalid digest: {message}"),
        false,
    )
}

fn package_validation_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::new(
        CoreErrorCode::UnsafeArchive,
        format!("content package review failed: {error}"),
        false,
    )
}

fn copy_regular_file_bounded(
    source_path: &Path,
    destination_path: &Path,
    maximum_bytes: u64,
) -> CoreResult<()> {
    let source_metadata = fs::symlink_metadata(source_path).map_err(package_io_error)?;
    if !source_metadata.file_type().is_file() {
        return Err(CoreError::invalid(
            "content package source must be a regular file and cannot be a symbolic link",
        ));
    }
    if source_metadata.len() > maximum_bytes {
        return Err(package_too_large(maximum_bytes));
    }
    let parent = destination_path.parent().ok_or_else(|| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "content package snapshot has no owned staging parent",
            false,
        )
    })?;
    fs::create_dir_all(parent).map_err(package_io_error)?;
    let result = (|| {
        let source = File::open(source_path).map_err(package_io_error)?;
        if !source.metadata().map_err(package_io_error)?.is_file() {
            return Err(CoreError::invalid(
                "content package source is not a regular file",
            ));
        }
        let mut reader = BufReader::new(source);
        let mut destination = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(destination_path)
            .map_err(package_io_error)?;
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
        loop {
            let read = reader.read(&mut buffer).map_err(package_io_error)?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(
                    u64::try_from(read)
                        .map_err(|_| CoreError::internal("package byte count overflow"))?,
                )
                .ok_or_else(|| CoreError::internal("package byte count overflow"))?;
            if copied > maximum_bytes {
                return Err(package_too_large(maximum_bytes));
            }
            destination
                .write_all(&buffer[..read])
                .map_err(package_io_error)?;
        }
        destination.flush().map_err(package_io_error)?;
        destination.sync_all().map_err(package_io_error)
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(destination_path);
        return Err(error);
    }
    Ok(())
}

fn remove_owned_snapshot(path: &Path, staging_dir: &Path, import_id: &str) -> CoreResult<()> {
    let expected = package_snapshot_path(staging_dir, import_id)?;
    if path != expected || path.parent() != Some(staging_dir) {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "content package snapshot is outside Core-owned staging",
            false,
        ));
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(package_io_error(error)),
    }
}

fn stale_package_review() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "content package review or selection is stale",
        true,
    )
}

fn package_too_large(maximum_bytes: u64) -> CoreError {
    CoreError::new(
        CoreErrorCode::UnsupportedContent,
        format!("content package exceeds the {maximum_bytes}-byte source limit"),
        false,
    )
}

fn package_io_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot stage content package: {error}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    include!("content_package/tests/support.rs");
    include!("content_package/tests/canonical_and_module_authority.rs");
    include!("content_package/tests/durability_and_atomicity.rs");
    include!("content_package/tests/prompt_and_snapshot_security.rs");
}
