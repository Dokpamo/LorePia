use std::collections::{BTreeMap, BTreeSet};
#[cfg(test)]
use std::{fs, path::PathBuf};

mod completed_authority;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    AssetDescriptor, AssetId, ContentCapability, ControlId, CoreError, CoreErrorCode, CoreResult,
    InstructionAuthority, ModuleComponentRef, PackageId, PlacementZone, PromptBlockId,
    Sha256Digest, SourceKind, ValidateOrchestration, VersionedJson,
};
use lorepia_orchestration::{
    ApprovedPackageImportPlan, ModuleImportApprovalEvidence, ModuleImportComponentAuthority,
    PackageComponentDisposition, PackageComponentKind, PackageReview, SelectiveImportPlan,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::database::{
    StagedAssetImport, Storage, claim_package_asset_promotions, claim_package_source_promotion,
    storage_db_error,
};
use crate::orchestration::{
    ActiveContentModuleRevision, PackageCommitDocument, PackageCommitInput, PackageImportRecord,
    PackageImportStatus, PackageSourceRecord, append_package_asset_descriptor,
    append_package_commit_document,
};

pub(crate) use completed_authority::VerifiedCompletedPackageAuthorities;
use completed_authority::{CompletedPackageAuthoritySnapshot, CompletedPackageCasFile};

const MAX_PACKAGE_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_PACKAGE_JSON_DEPTH: usize = 40;
const MAX_PACKAGE_JSON_NODES: usize = 200_000;
const MAX_CAPABILITY_REASON_BYTES: usize = 4 * 1024;
const MAX_PACKAGE_APPROVAL_BYTES: usize = 256 * 1024;
const MAX_NORMALIZATION_REASON_BYTES: usize = 512;
const MAX_COMPLETED_MODULE_AUTHORITIES: usize = 64;
pub const MAX_COMPLETED_PACKAGE_EXPORTS: u32 = 100;
pub const MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS: usize = 200;

/// Capability names persisted in the immutable import review.
///
/// The final eight variants are deliberately not expressible as executable
/// product capabilities. They exist only so an inspection can durably record
/// that hostile or unsupported package material was denied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageCapability {
    PromptFragments,
    Knowledge,
    Variables,
    Transforms,
    DeclarativeInteractions,
    ImageAssets,
    AudioAssets,
    VideoAssets,
    AttachmentAssets,
    HighRiskAssets,
    ExternalUrls,
    Html,
    Script,
    NativeCode,
    Network,
    Filesystem,
    Shell,
    Credentials,
}

impl PackageCapability {
    const ALL: [Self; 18] = [
        Self::PromptFragments,
        Self::Knowledge,
        Self::Variables,
        Self::Transforms,
        Self::DeclarativeInteractions,
        Self::ImageAssets,
        Self::AudioAssets,
        Self::VideoAssets,
        Self::AttachmentAssets,
        Self::HighRiskAssets,
        Self::ExternalUrls,
        Self::Html,
        Self::Script,
        Self::NativeCode,
        Self::Network,
        Self::Filesystem,
        Self::Shell,
        Self::Credentials,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::PromptFragments => "prompt_fragments",
            Self::Knowledge => "knowledge",
            Self::Variables => "variables",
            Self::Transforms => "transforms",
            Self::DeclarativeInteractions => "declarative_interactions",
            Self::ImageAssets => "image_assets",
            Self::AudioAssets => "audio_assets",
            Self::VideoAssets => "video_assets",
            Self::AttachmentAssets => "attachment_assets",
            Self::HighRiskAssets => "high_risk_assets",
            Self::ExternalUrls => "external_urls",
            Self::Html => "html",
            Self::Script => "script",
            Self::NativeCode => "native_code",
            Self::Network => "network",
            Self::Filesystem => "filesystem",
            Self::Shell => "shell",
            Self::Credentials => "credentials",
        }
    }

    const fn is_never_approvable(self) -> bool {
        matches!(
            self,
            Self::ExternalUrls
                | Self::Html
                | Self::Script
                | Self::NativeCode
                | Self::Network
                | Self::Filesystem
                | Self::Shell
                | Self::Credentials
        )
    }

    const fn required_support(self) -> PackageCapabilitySupport {
        match self {
            Self::Transforms | Self::DeclarativeInteractions => {
                PackageCapabilitySupport::ApprovalRequired
            }
            Self::HighRiskAssets
            | Self::ExternalUrls
            | Self::Html
            | Self::Script
            | Self::NativeCode
            | Self::Network
            | Self::Filesystem
            | Self::Shell
            | Self::Credentials => PackageCapabilitySupport::Unsupported,
            Self::PromptFragments
            | Self::Knowledge
            | Self::Variables
            | Self::ImageAssets
            | Self::AudioAssets
            | Self::VideoAssets
            | Self::AttachmentAssets => PackageCapabilitySupport::Supported,
        }
    }
}

impl From<ContentCapability> for PackageCapability {
    fn from(value: ContentCapability) -> Self {
        match value {
            ContentCapability::PromptFragments => Self::PromptFragments,
            ContentCapability::Knowledge => Self::Knowledge,
            ContentCapability::Variables => Self::Variables,
            ContentCapability::Transforms => Self::Transforms,
            ContentCapability::DeclarativeInteractions => Self::DeclarativeInteractions,
            ContentCapability::ImageAssets => Self::ImageAssets,
            ContentCapability::AudioAssets => Self::AudioAssets,
            ContentCapability::VideoAssets => Self::VideoAssets,
            ContentCapability::AttachmentAssets => Self::AttachmentAssets,
            ContentCapability::HighRiskAssets => Self::HighRiskAssets,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageCapabilitySupport {
    Supported,
    Unsupported,
    ApprovalRequired,
}

impl PackageCapabilitySupport {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Unsupported => "unsupported",
            Self::ApprovalRequired => "approval_required",
        }
    }
}

/// One deterministic capability decision included in a review snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageCapabilityDecision {
    pub capability: PackageCapability,
    pub support: PackageCapabilitySupport,
    pub approved: bool,
    pub reason: String,
}

/// Exact, hashable capability review used by package approval CAS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageCapabilityReview {
    pub schema_version: u32,
    pub decisions: Vec<PackageCapabilityDecision>,
}

/// Compare-and-swap expectation binding every reviewed package snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportExpectation {
    pub revision: u64,
    pub inspection_sha256: String,
    pub selection_sha256: String,
    pub capability_review_sha256: String,
}

/// CAS expectation before a selection exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageInspectionExpectation {
    pub revision: u64,
    pub inspection_sha256: String,
    pub capability_review_sha256: String,
}

/// Explicit binding from one reviewed source component to one normalized
/// document. One source component may yield several documents; their
/// `component_document_ordinal` values must be contiguous and are retained in
/// immutable per-document commit evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDocumentCommitBinding {
    pub document_index: u32,
    pub source_component_key: String,
    pub component_document_ordinal: u32,
    pub source_component_sha256: String,
    pub target_object_id: String,
    pub document_kind: String,
    pub document_sha256: String,
    pub expected_object_revision: Option<u64>,
}

/// Whether one normalized package document creates a new logical object or
/// appends to an exact, already reviewed object revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageDocumentTargetDisposition {
    Create,
    Update,
}

/// Safe, immutable target review for one normalized package document.
///
/// This projection contains identifiers and hashes only. Package bytes,
/// normalized document bodies and host paths never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageDocumentTargetReview {
    pub source_component_id: String,
    pub component_document_ordinal: u32,
    pub document_index: u32,
    pub document_kind: String,
    pub target_object_id: String,
    pub disposition: PackageDocumentTargetDisposition,
    pub expected_target_revision_id: Option<String>,
    pub expected_target_state_revision: Option<u64>,
    pub source_component_sha256: String,
    pub document_sha256: String,
}

/// Canonical target-review snapshot sealed at package selection time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportTargetReview {
    pub target_review_sha256: String,
    pub documents: Vec<PackageDocumentTargetReview>,
}

impl PackageImportTargetReview {
    pub fn verify(&self) -> CoreResult<()> {
        validate_document_target_reviews(&self.documents)?;
        if package_import_target_review_sha256(&self.documents)? != self.target_review_sha256 {
            return Err(CoreError::invalid(
                "package target-review digest does not match its documents",
            ));
        }
        Ok(())
    }
}

/// Exact explicit confirmation for one reviewed update target.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageUpdateTargetConfirmation {
    pub source_component_id: String,
    pub component_document_ordinal: u32,
    pub target_object_id: String,
    pub expected_target_revision_id: String,
    pub expected_target_state_revision: u64,
}

/// Immutable evidence of one safety normalization applied to package-authored
/// declarative behavior before approval. `before` preserves author intent;
/// `after` must remain `false` for imported execution flags.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageNormalizationEvidence {
    pub component_id: String,
    pub object_id: String,
    pub field: String,
    pub before: bool,
    pub after: bool,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportApprovalRecord {
    pub id: String,
    pub import_id: String,
    pub inspection_sha256: String,
    pub selection_sha256: String,
    pub capability_review_sha256: String,
    pub payload: VersionedJson,
    pub approved_at: DateTime<Utc>,
}

/// One exact document revision committed from an approved package component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPackageDocumentAuthority {
    pub document_ordinal: u32,
    pub target_object_id: String,
    pub target_revision_id: String,
    pub source_component_sha256: String,
    pub document_sha256: String,
    pub result_sha256: String,
}

/// One enabled component carried by a completed, immutable package approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPackageComponentAuthority {
    pub component_id: String,
    pub kind: PackageComponentKind,
    pub sha256: String,
    pub committed_documents: Vec<CompletedPackageDocumentAuthority>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPackageAssetSourceAuthority {
    pub component_id: String,
    pub component_sha256: String,
}

/// Exact immutable descriptor and CAS authority for one committed asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPackageAssetAuthority {
    pub asset_id: AssetId,
    pub descriptor: AssetDescriptor,
    pub descriptor_sha256: String,
    pub cas_sha256: String,
    pub source_components: Vec<CompletedPackageAssetSourceAuthority>,
}

/// Verified authority resolved from an approval id.
///
/// Callers can use this value to authorize imported module activation without
/// trusting a caller-supplied approval string. Construction verifies the
/// completed import, source, review, selection, approval, capability and
/// per-component commit snapshots before and after lock-free CAS hashing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedPackageAuthority {
    pub approval_id: String,
    pub import_id: String,
    pub package_id: PackageId,
    pub status: PackageImportStatus,
    pub import_revision: u64,
    pub source_sha256: String,
    pub inspection_sha256: String,
    pub selection_sha256: String,
    pub capability_review_sha256: String,
    pub approval_sha256: String,
    pub required_capabilities: Vec<ContentCapability>,
    pub approved_capabilities: Vec<PackageCapability>,
    pub enabled_components: Vec<CompletedPackageComponentAuthority>,
    pub committed_assets: Vec<CompletedPackageAssetAuthority>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageImportAuditEvent {
    pub sequence: u64,
    pub import_revision: u64,
    pub event_kind: String,
    pub payload: VersionedJson,
    pub payload_sha256: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
struct StoredImportState {
    record: PackageImportRecord,
    package_source_id: String,
    inspection_sha256: String,
    selection_sha256: Option<String>,
    capability_review_sha256: String,
    approved_selection_sha256: Option<String>,
    approved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct ReviewedComponentRow {
    ordinal: u32,
    source_component_key: String,
    component_kind: String,
    disposition: String,
    selected: bool,
    target_object_id: Option<String>,
    target_revision_id: Option<String>,
    review_json: String,
    review_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PackageApprovalPayload {
    plan: ApprovedPackageImportPlan,
    document_bindings: Vec<PackageDocumentCommitBinding>,
    target_review_sha256: String,
    confirmed_update_targets: Vec<PackageUpdateTargetConfirmation>,
    approved_capabilities: Vec<PackageCapability>,
    normalization_evidence_sha256: String,
    normalization_evidence: Vec<PackageNormalizationEvidence>,
}

type CompletedAuthorityCommitEvidence =
    BTreeMap<(String, u32), (CompletedPackageDocumentAuthority, Value)>;

impl Storage {
    pub fn get_package_source(&self, id: &str) -> CoreResult<PackageSourceRecord> {
        validate_identifier("package source", id)?;
        let connection = self.connection()?;
        read_package_source(&connection, "source.id = ?1", id)?
            .ok_or_else(|| not_found("package source"))
    }

    pub fn get_package_source_by_hash(
        &self,
        source_sha256: &str,
    ) -> CoreResult<PackageSourceRecord> {
        validate_sha256("package source", source_sha256)?;
        let connection = self.connection()?;
        read_package_source(&connection, "source.source_hash = ?1", source_sha256)?
            .ok_or_else(|| not_found("package source"))
    }

    /// Removes a promoted source only when no durable product or package
    /// record claimed it. This is the compensating operation for failure
    /// between CAS promotion and `create_inspected_package_import`.
    pub fn discard_unclaimed_package_source(
        &self,
        import_id: &str,
        source_sha256: &str,
        source_size_bytes: u64,
    ) -> CoreResult<bool> {
        validate_identifier("package import", import_id)?;
        validate_sha256("package source", source_sha256)?;
        self.cleanup_package_source_promotion(import_id, source_sha256, source_size_bytes)
    }

    /// Removes promoted asset rows and bytes that were not claimed by a
    /// descriptor, character, raw-extension preservation record or character
    /// asset link. Claimed/deduplicated assets are retained.
    pub fn discard_unclaimed_package_assets(
        &self,
        import_id: &str,
        staged_assets: &[StagedAssetImport],
    ) -> CoreResult<u64> {
        validate_identifier("package import", import_id)?;
        self.cleanup_package_asset_promotions(import_id, staged_assets)
    }

    /// Starts the durable lifecycle at `inspected` without manufacturing a
    /// selection. The immutable component review is retained in
    /// `inspection_json`; normalized selected component rows are inserted only
    /// by [`Storage::select_package_import`], because those rows are immutable.
    pub fn create_inspected_package_import(
        &self,
        source: &PackageSourceRecord,
        import: &PackageImportRecord,
        review: &PackageReview,
        capability_review: &PackageCapabilityReview,
    ) -> CoreResult<PackageImportRecord> {
        validate_inspected_import(source, import, review, capability_review)?;
        verify_source_cas(self, source)?;
        let capability_hash = package_capability_review_sha256(capability_review)?;
        let inspection_json = encode_json("package inspection", &import.inspection)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        insert_package_source(&transaction, source)?;
        if transaction
            .query_row(
                "SELECT 1 FROM package_imports WHERE id = ?1",
                [import.id.as_str()],
                |_| Ok(()),
            )
            .optional()
            .map_err(storage_db_error)?
            .is_some()
        {
            let current = read_import_state(&transaction, &import.id)?;
            if current.record == *import
                && current.package_source_id == source.id
                && current.inspection_sha256 == review.review_sha256.as_str()
                && current.capability_review_sha256 == capability_hash
            {
                claim_package_source_promotion(
                    &transaction,
                    &import.id,
                    &source.source_sha256,
                    source.source_size_bytes,
                    false,
                )?;
                transaction.commit().map_err(storage_db_error)?;
                return Ok(current.record);
            }
            return Err(revision_conflict(
                "package import",
                &import.id,
                None,
                Some(current.record.revision),
            ));
        }
        transaction
            .execute(
                "INSERT INTO package_imports (
                    id, package_source_id, inspection_schema_version, state,
                    revision, inspection_json, inspection_sha256,
                    selection_json, selection_sha256,
                    capability_review_sha256, approved_selection_sha256,
                    approved_at, failure_json, created_at, updated_at,
                    completed_at
                 ) VALUES (
                    ?1, ?2, ?3, 'inspected', 1, ?4, ?5, NULL, NULL, ?6,
                    NULL, NULL, NULL, ?7, ?8, NULL
                 )",
                params![
                    import.id,
                    source.id,
                    i64::from(import.inspection.schema_version),
                    inspection_json,
                    review.review_sha256.as_str(),
                    capability_hash,
                    import.created_at.to_rfc3339(),
                    import.updated_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        insert_capability_review(&transaction, &import.id, capability_review)?;
        append_audit(
            &transaction,
            &import.id,
            1,
            "inspected",
            &VersionedJson {
                schema_version: 1,
                value: json!({
                    "inspection_sha256": review.review_sha256.as_str(),
                    "capability_review_sha256": capability_hash,
                    "source_sha256": source.source_sha256,
                }),
            },
            import.created_at,
        )?;
        claim_package_source_promotion(
            &transaction,
            &import.id,
            &source.source_sha256,
            source.source_size_bytes,
            true,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(&import.id)
    }

    /// Binds an exact deterministic selection to a durable inspection and
    /// advances `inspected -> awaiting_review`.
    #[allow(clippy::too_many_lines)] // One transaction revalidates every immutable review seam.
    pub fn select_package_import(
        &self,
        import_id: &str,
        expected: &PackageInspectionExpectation,
        selection: &SelectiveImportPlan,
        document_bindings: &[PackageDocumentCommitBinding],
    ) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", import_id)?;
        validate_inspection_expectation(expected)?;
        selection.verify().map_err(|error| {
            CoreError::invalid(format!("package selection is invalid: {error}"))
        })?;
        let now = Utc::now();
        let next_revision = expected
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_import_state(&transaction, import_id)?;
        if current.record.status == PackageImportStatus::AwaitingReview
            && current.record.revision == next_revision
        {
            validate_selection_replay(
                &transaction,
                &current,
                expected,
                selection,
                document_bindings,
            )?;
            return Ok(current.record);
        }
        assert_inspection_expectation(&current, expected)?;
        if current.record.status != PackageImportStatus::Inspected
            || current.record.selection.is_some()
        {
            return Err(CoreError::invalid(
                "only an unselected inspected package import can be selected",
            ));
        }
        let review: PackageReview = serde_json::from_value(current.record.inspection.value.clone())
            .map_err(|error| {
                storage_corrupted(format!("stored package review is invalid: {error}"))
            })?;
        review.verify().map_err(|error| {
            storage_corrupted(format!("stored package review is invalid: {error}"))
        })?;
        validate_selection_against_review(&review, selection)?;
        if selection.review_sha256.as_str() != current.inspection_sha256
            || selection.package_id != current.record.package_id
            || selection.source_sha256.as_str()
                != read_source_hash(&transaction, &current.package_source_id)?
        {
            return Err(CoreError::invalid(
                "package selection does not match the durable inspection",
            ));
        }
        validate_stored_capability_decisions(
            &transaction,
            import_id,
            &selection.required_capabilities,
        )?;
        let reviewed =
            reviewed_selection_rows(&transaction, &review, selection, document_bindings)?;
        let component_review_sha256 = reviewed_component_rows_sha256(&reviewed.components)?;
        insert_reviewed_components(&transaction, import_id, &reviewed.components)?;
        insert_document_target_reviews(
            &transaction,
            import_id,
            &reviewed.components,
            &reviewed.documents,
        )?;
        let selection_wrapper = VersionedJson {
            schema_version: 1,
            value: serde_json::to_value(selection).map_err(|error| {
                CoreError::invalid(format!("package selection cannot be encoded: {error}"))
            })?,
        };
        let selection_json = encode_json("package selection", &selection_wrapper)?;
        let audit = VersionedJson {
            schema_version: 1,
            value: json!({
                "inspection_sha256": current.inspection_sha256,
                "selection_sha256": selection.plan_sha256.as_str(),
                "capability_review_sha256": current.capability_review_sha256,
                "selected_component_ids": selection.components.iter()
                    .map(|component| component.component.id.as_str())
                    .collect::<Vec<_>>(),
                "component_review_sha256": component_review_sha256,
                "target_review_sha256": reviewed.target_review_sha256,
            }),
        };
        append_audit(
            &transaction,
            import_id,
            next_revision,
            "review_requested",
            &audit,
            now,
        )?;
        let changed = transaction
            .execute(
                "UPDATE package_imports
                 SET state = 'awaiting_review', revision = ?2,
                     selection_json = ?3, selection_sha256 = ?4,
                     updated_at = ?5
                 WHERE id = ?1 AND state = 'inspected' AND revision = ?6",
                params![
                    import_id,
                    i64_from_u64("package import revision", next_revision)?,
                    selection_json,
                    selection.plan_sha256.as_str(),
                    now.to_rfc3339(),
                    i64_from_u64("package import revision", expected.revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(revision_conflict(
                "package import",
                import_id,
                Some(expected.revision),
                None,
            ));
        }
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(import_id)
    }

    pub fn get_package_import(&self, id: &str) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", id)?;
        let connection = self.connection()?;
        read_import_state(&connection, id).map(|state| state.record)
    }

    pub fn get_package_import_target_review(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageImportTargetReview> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let current = read_import_state(&connection, import_id)?;
        if current.record.selection.is_none() {
            return Err(CoreError::invalid(
                "package import has no selected target review",
            ));
        }
        load_package_import_target_review(&connection, &current)
    }

    pub fn get_package_source_for_import(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageSourceRecord> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let source_id = connection
            .query_row(
                "SELECT package_source_id FROM package_imports WHERE id = ?1",
                [import_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("package import"))?;
        read_package_source_by_id(&connection, &source_id)
    }

    pub fn get_package_inspection_expectation(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageInspectionExpectation> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let current = read_import_state(&connection, import_id)?;
        if current.record.selection.is_some() {
            return Err(CoreError::invalid("package import already has a selection"));
        }
        Ok(PackageInspectionExpectation {
            revision: current.record.revision,
            inspection_sha256: current.inspection_sha256,
            capability_review_sha256: current.capability_review_sha256,
        })
    }

    pub fn get_package_import_expectation(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageImportExpectation> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let current = read_import_state(&connection, import_id)?;
        Ok(PackageImportExpectation {
            revision: current.record.revision,
            inspection_sha256: current.inspection_sha256,
            selection_sha256: current
                .selection_sha256
                .ok_or_else(|| CoreError::invalid("package import has no selection"))?,
            capability_review_sha256: current.capability_review_sha256,
        })
    }

    pub fn get_package_capability_review(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageCapabilityReview> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        read_import_state(&connection, import_id)?;
        read_capability_review(&connection, import_id)
    }

    pub fn list_package_imports(
        &self,
        package_id: Option<&PackageId>,
    ) -> CoreResult<Vec<PackageImportRecord>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT import.id
                 FROM package_imports AS import
                 JOIN package_sources AS source
                   ON source.id = import.package_source_id
                 WHERE (?1 IS NULL OR source.package_id = ?1)
                 ORDER BY import.created_at DESC, import.id",
            )
            .map_err(storage_db_error)?;
        let ids = statement
            .query_map([package_id.map(PackageId::as_str)], |row| {
                row.get::<_, String>(0)
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        ids.into_iter()
            .map(|id| read_import_state(&connection, &id).map(|state| state.record))
            .collect()
    }

    /// Returns a bounded restart-discovery list of imports that can still
    /// require user action or crash recovery.
    pub fn list_pending_package_import_ids(&self, limit: u32) -> CoreResult<Vec<String>> {
        if !(1..=256).contains(&limit) {
            return Err(CoreError::invalid(
                "pending package import limit must be between 1 and 256",
            ));
        }
        let connection = self.connection()?;
        let ids = {
            let mut statement = connection
                .prepare(
                    "SELECT id
                     FROM package_imports
                     WHERE state IN (
                         'inspected',
                         'awaiting_review',
                         'approved',
                         'committing'
                     )
                     ORDER BY updated_at DESC, id
                     LIMIT ?1",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([i64::from(limit)], |row| row.get::<_, String>(0))
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        for id in &ids {
            let state = read_import_state(&connection, id)?;
            if !matches!(
                state.record.status,
                PackageImportStatus::Inspected
                    | PackageImportStatus::AwaitingReview
                    | PackageImportStatus::Approved
                    | PackageImportStatus::Committing
            ) {
                return Err(storage_corrupted(
                    "pending package import query returned a terminal state",
                ));
            }
        }
        Ok(ids)
    }

    /// Returns completed package import identities in deterministic restart
    /// discovery order. Callers must still resolve each identity through the
    /// exact completed-source export authority before projecting it.
    pub fn list_completed_package_import_ids(&self, limit: u32) -> CoreResult<Vec<String>> {
        if !(1..=MAX_COMPLETED_PACKAGE_EXPORTS).contains(&limit) {
            return Err(CoreError::invalid(format!(
                "completed package export limit must be between 1 and {MAX_COMPLETED_PACKAGE_EXPORTS}"
            )));
        }
        let connection = self.connection()?;
        let ids = {
            let mut statement = connection
                .prepare(
                    "SELECT id
                     FROM package_imports
                     WHERE state = 'completed'
                     ORDER BY updated_at DESC, id
                     LIMIT ?1",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([i64::from(limit)], |row| row.get::<_, String>(0))
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        if ids.len() > usize::try_from(limit).unwrap_or(usize::MAX) {
            return Err(storage_corrupted(
                "completed package export query exceeded its requested bound",
            ));
        }
        for id in &ids {
            validate_identifier("completed package import", id).map_err(|_| {
                storage_corrupted("completed package export query returned an invalid identity")
            })?;
            let (state, updated_at) = connection
                .query_row(
                    "SELECT state, updated_at FROM package_imports WHERE id = ?1",
                    [id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    storage_corrupted(
                        "completed package export identity disappeared during status verification",
                    )
                })?;
            if parse_import_status(&state)? != PackageImportStatus::Completed {
                return Err(storage_corrupted(
                    "completed package export query returned a non-completed state",
                ));
            }
            parse_datetime("completed package import updated_at", &updated_at)?;
        }
        Ok(ids)
    }

    /// Persists an immutable approval and advances exactly the reviewed import
    /// revision. The typed plan verifies its own canonical review, plan and
    /// approval hashes before any mutation.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    // Approval CAS and every immutable evidence input form one atomic boundary.
    pub fn approve_package_import(
        &self,
        import_id: &str,
        expected: &PackageImportExpectation,
        approved: &ApprovedPackageImportPlan,
        document_bindings: &[PackageDocumentCommitBinding],
        expected_target_review_sha256: &str,
        confirmed_update_targets: &[PackageUpdateTargetConfirmation],
        approved_capabilities: &[PackageCapability],
        normalization_evidence: &[PackageNormalizationEvidence],
    ) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", import_id)?;
        validate_expectation(expected)?;
        approved
            .verify()
            .map_err(|error| CoreError::invalid(format!("package approval is invalid: {error}")))?;
        validate_binding_snapshot_shape(document_bindings)?;
        validate_sha256("package target review", expected_target_review_sha256)?;
        let confirmed_update_targets =
            canonical_update_target_confirmations(confirmed_update_targets)?;
        let update_target_confirmations_sha256 =
            package_update_target_confirmations_sha256(&confirmed_update_targets)?;
        if approved.target_review_sha256.as_str() != expected_target_review_sha256
            || approved.update_target_confirmations_sha256.as_str()
                != update_target_confirmations_sha256
        {
            return Err(CoreError::invalid(
                "package approval hash is not bound to its target review and confirmations",
            ));
        }
        let mut approved_capabilities = approved_capabilities.to_vec();
        approved_capabilities.sort_unstable();
        if approved_capabilities
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(CoreError::invalid(
                "package capability approval contains duplicates",
            ));
        }
        let mut normalization_evidence = normalization_evidence.to_vec();
        normalization_evidence.sort();
        validate_normalization_evidence_shape(&normalization_evidence)?;
        let normalization_evidence_sha256 =
            package_normalization_evidence_sha256(&normalization_evidence)?;
        let approval_payload = PackageApprovalPayload {
            plan: approved.clone(),
            document_bindings: document_bindings.to_vec(),
            target_review_sha256: expected_target_review_sha256.to_owned(),
            confirmed_update_targets: confirmed_update_targets.clone(),
            approved_capabilities: approved_capabilities.clone(),
            normalization_evidence_sha256,
            normalization_evidence,
        };
        let payload = VersionedJson {
            schema_version: 1,
            value: serde_json::to_value(&approval_payload).map_err(|error| {
                CoreError::invalid(format!("package approval cannot be encoded: {error}"))
            })?,
        };
        let payload_json = encode_json("package approval", &payload)?;
        if payload_json.len() > MAX_PACKAGE_APPROVAL_BYTES {
            return Err(CoreError::invalid(
                "package approval exceeds the durable payload limit",
            ));
        }
        let now = Utc::now();
        let next_revision = expected
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_import_state(&transaction, import_id)?;
        if current.record.status == PackageImportStatus::Approved
            && current.record.revision == next_revision
        {
            validate_approval_replay(
                &transaction,
                &current,
                expected,
                &approval_payload,
                &payload,
            )?;
            return Ok(current.record);
        }
        assert_expectation(&current, expected)?;
        if current.record.status != PackageImportStatus::AwaitingReview {
            return Err(CoreError::invalid(
                "only an awaiting-review package import can be approved",
            ));
        }
        if approved.review_sha256.as_str() != current.inspection_sha256
            || approved.plan_sha256.as_str()
                != current
                    .selection_sha256
                    .as_deref()
                    .ok_or_else(|| storage_corrupted("package selection hash is missing"))?
            || approved.source_sha256.as_str()
                != read_source_hash(&transaction, &current.package_source_id)?
            || approved.package_id != current.record.package_id
        {
            return Err(CoreError::invalid(
                "package approval does not match the exact reviewed snapshots",
            ));
        }
        let selected_plan = decode_selection(&current.record)?;
        if approved.plan_sha256 != selected_plan.plan_sha256
            || approved.review_sha256 != selected_plan.review_sha256
            || approved.components.len() != selected_plan.components.len()
        {
            return Err(CoreError::invalid(
                "package approval payload does not match the stored selection",
            ));
        }
        let component_rows = load_selected_commit_components(&transaction, import_id)?;
        let target_review = load_package_import_target_review(&transaction, &current)?;
        if target_review.target_review_sha256 != expected_target_review_sha256 {
            return Err(CoreError::invalid(
                "package approval target-review digest is stale",
            ));
        }
        validate_approval_bindings(
            &transaction,
            document_bindings,
            &component_rows,
            &target_review,
            &confirmed_update_targets,
        )?;
        validate_normalization_evidence_linkage(
            &approval_payload.normalization_evidence,
            document_bindings,
            &component_rows,
        )?;
        validate_capability_approval_snapshot(
            &transaction,
            import_id,
            &selected_plan.required_capabilities,
            &approved_capabilities,
        )?;
        transaction
            .execute(
                "INSERT INTO package_import_approvals (
                    id, import_id, inspection_sha256, selection_sha256,
                    capability_review_sha256, approval_payload_json, approved_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    approved.approval_id,
                    import_id,
                    current.inspection_sha256,
                    current.selection_sha256,
                    current.capability_review_sha256,
                    payload_json,
                    now.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        append_audit(
            &transaction,
            import_id,
            next_revision,
            "approved",
            &payload,
            now,
        )?;
        update_import_state(
            &transaction,
            import_id,
            expected.revision,
            PackageImportStatus::Approved,
            next_revision,
            current.selection_sha256.as_deref(),
            Some(now),
            None,
            None,
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(import_id)
    }

    pub fn get_package_import_approval(
        &self,
        import_id: &str,
    ) -> CoreResult<PackageImportApprovalRecord> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let record = connection
            .query_row(
                "SELECT id, inspection_sha256, selection_sha256,
                        capability_review_sha256, approval_payload_json,
                        approved_at
                 FROM package_import_approvals
                 WHERE import_id = ?1
                 ORDER BY approved_at DESC, id
                 LIMIT 1",
                [import_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .map_or_else(
                || Err(not_found("package import approval")),
                |row| {
                    Ok(PackageImportApprovalRecord {
                        id: row.0,
                        import_id: import_id.to_owned(),
                        inspection_sha256: row.1,
                        selection_sha256: row.2,
                        capability_review_sha256: row.3,
                        payload: decode_json("package approval", &row.4)?,
                        approved_at: parse_datetime("package approval approved_at", &row.5)?,
                    })
                },
            )?;
        let current = read_import_state(&connection, import_id)?;
        let payload = read_approval_payload(&connection, import_id)?;
        if record.inspection_sha256 != current.inspection_sha256
            || current.selection_sha256.as_deref() != Some(&record.selection_sha256)
            || record.capability_review_sha256 != current.capability_review_sha256
            || payload.plan.review_sha256.as_str() != record.inspection_sha256
            || payload.plan.plan_sha256.as_str() != record.selection_sha256
        {
            return Err(storage_corrupted(
                "package approval differs from its reviewed import snapshots",
            ));
        }
        Ok(record)
    }

    #[allow(clippy::too_many_lines)] // Every immutable approval and commit seam is revalidated.
    fn get_completed_package_authority_by_approval_id_in_connection(
        connection: &Connection,
        approval_id: &str,
    ) -> CoreResult<CompletedPackageAuthoritySnapshot> {
        validate_identifier("package approval", approval_id)?;
        let approval_row = connection
            .query_row(
                "SELECT import_id, inspection_sha256, selection_sha256,
                        capability_review_sha256, approved_at
                 FROM package_import_approvals
                 WHERE id = ?1",
                [approval_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("package approval"))?;
        let current = read_import_state(connection, &approval_row.0)?;
        let completed_at = connection
            .query_row(
                "SELECT completed_at FROM package_imports WHERE id = ?1",
                [approval_row.0.as_str()],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                storage_corrupted("completed package authority has no completion timestamp")
            })?;
        parse_datetime("package import completed_at", &completed_at)?;
        if current.record.status != PackageImportStatus::Completed
            || current.inspection_sha256 != approval_row.1
            || current.selection_sha256.as_deref() != Some(approval_row.2.as_str())
            || current.capability_review_sha256 != approval_row.3
            || current.approved_selection_sha256.as_deref() != Some(approval_row.2.as_str())
            || current.approved_at
                != Some(parse_datetime(
                    "package approval approved_at",
                    &approval_row.4,
                )?)
        {
            return Err(storage_corrupted(
                "package approval is not the exact authority for a completed import",
            ));
        }
        let source = read_package_source_by_id(connection, &current.package_source_id)?;
        validate_sha256("package source", &source.source_sha256)
            .map_err(|_| storage_corrupted("completed package source digest is invalid"))?;
        let source_cas = connection
            .query_row(
                "SELECT relative_path, size_bytes
                 FROM content_sources
                 WHERE sha256 = ?1",
                [source.source_sha256.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(storage_db_error)?;
        if u64_from_i64("package source size", source_cas.1)? != source.source_size_bytes {
            return Err(storage_corrupted(
                "completed package source CAS metadata differs from its package record",
            ));
        }
        let mut cas_files = vec![CompletedPackageCasFile {
            namespace: "sources",
            sha256: source.source_sha256.clone(),
            size_bytes: source.source_size_bytes,
            relative_path: source_cas.0,
        }];
        let approval = read_approval_payload(connection, &approval_row.0)?;
        if approval.plan.approval_id != approval_id
            || approval.plan.review_sha256.as_str() != approval_row.1
            || approval.plan.plan_sha256.as_str() != approval_row.2
            || approval.plan.source_sha256.as_str() != source.source_sha256
            || approval.plan.package_id != source.package_id
            || approval.plan.package_id != current.record.package_id
        {
            return Err(storage_corrupted(
                "completed package approval payload differs from its durable identity",
            ));
        }
        let mut committed_assets = Vec::with_capacity(approval.plan.assets.len());
        for asset in &approval.plan.assets {
            let stored = connection
                .query_row(
                    "SELECT cas.relative_path, cas.media_type, cas.size_bytes,
                            descriptor.payload_json
                     FROM assets AS cas
                     JOIN asset_descriptors AS descriptor
                       ON descriptor.asset_hash = cas.sha256
                     WHERE cas.sha256 = ?1 AND descriptor.id = ?2",
                    params![asset.sha256.as_str(), asset.id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    storage_corrupted("completed package approval asset CAS row is missing")
                })?;
            if stored.1 != asset.media_type
                || u64_from_i64("package asset size", stored.2)? != asset.size_bytes
            {
                return Err(storage_corrupted(
                    "completed package approval asset metadata differs from its descriptor",
                ));
            }
            cas_files.push(CompletedPackageCasFile {
                namespace: "assets",
                sha256: asset.sha256.as_str().to_owned(),
                size_bytes: asset.size_bytes,
                relative_path: stored.0,
            });
            let expected_descriptor = encode_json("completed package asset descriptor", asset)?;
            if stored.3 != expected_descriptor {
                return Err(storage_corrupted(
                    "completed package asset descriptor differs from approval",
                ));
            }
            let source_components = approval
                .plan
                .components
                .iter()
                .filter(|component| component.component.asset_ids.contains(&asset.id))
                .map(|component| CompletedPackageAssetSourceAuthority {
                    component_id: component.component.id.clone(),
                    component_sha256: component.component.sha256.as_str().to_owned(),
                })
                .collect();
            committed_assets.push(CompletedPackageAssetAuthority {
                asset_id: asset.id.clone(),
                descriptor: asset.clone(),
                descriptor_sha256: sha256_hex(expected_descriptor.as_bytes()),
                cas_sha256: asset.sha256.as_str().to_owned(),
                source_components,
            });
        }

        let selected_rows = load_selected_commit_components(connection, &approval_row.0)?;
        if selected_rows.len() != approval.plan.components.len() {
            return Err(storage_corrupted(
                "completed package approval component count differs from selection",
            ));
        }
        for planned in &approval.plan.components {
            let row = selected_rows.get(&planned.component.id).ok_or_else(|| {
                storage_corrupted("completed package approval component is missing from selection")
            })?;
            if !row.selected
                || !matches!(row.disposition.as_str(), "create" | "update" | "conflict")
                || row.component_kind != component_kind_str(planned.component.kind)
                || sha256_hex(row.review_json.as_bytes()) != row.review_sha256
            {
                return Err(storage_corrupted(
                    "completed package component review metadata is invalid",
                ));
            }
            let descriptor: lorepia_orchestration::PackageComponentDescriptor =
                decode_json("package component review", &row.review_json)?;
            if descriptor != planned.component {
                return Err(storage_corrupted(
                    "completed package component differs from its approved descriptor",
                ));
            }
        }

        let mut statement = connection
            .prepare(
                "SELECT component.source_component_key,
                        committed_document.document_ordinal,
                        committed_document.target_object_id,
                        committed_document.target_revision_id,
                        committed_document.result_json,
                        committed_document.result_sha256
                 FROM package_import_component_commits AS committed_document
                 JOIN package_import_components AS component
                   ON component.import_id = committed_document.import_id
                  AND component.ordinal = committed_document.component_ordinal
                 WHERE committed_document.import_id = ?1
                 ORDER BY component.ordinal, committed_document.document_ordinal",
            )
            .map_err(storage_db_error)?;
        let committed_rows = statement
            .query_map([approval_row.0.as_str()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        let mut committed_documents = CompletedAuthorityCommitEvidence::new();
        for row in committed_rows {
            validate_sha256("package component commit result", &row.5)?;
            if sha256_hex(row.4.as_bytes()) != row.5 {
                return Err(storage_corrupted(
                    "completed package component result hash does not match",
                ));
            }
            let result: VersionedJson = decode_json("package component commit result", &row.4)?;
            if result.schema_version != 1
                || result
                    .value
                    .get("source_component_key")
                    .and_then(Value::as_str)
                    != Some(row.0.as_str())
                || result
                    .value
                    .get("component_document_ordinal")
                    .and_then(Value::as_u64)
                    != Some(u64::from(row.1))
                || result.value.get("target_object_id").and_then(Value::as_str)
                    != Some(row.2.as_str())
                || result
                    .value
                    .get("target_revision_id")
                    .and_then(Value::as_str)
                    != Some(row.3.as_str())
            {
                return Err(storage_corrupted(
                    "completed package component result differs from its typed columns",
                ));
            }
            let source_component_sha256 = result
                .value
                .get("source_component_sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    storage_corrupted("completed package component source hash is missing")
                })?
                .to_owned();
            let document_sha256 = result
                .value
                .get("document_sha256")
                .and_then(Value::as_str)
                .ok_or_else(|| storage_corrupted("completed package document hash is missing"))?
                .to_owned();
            validate_sha256("package component", &source_component_sha256)
                .map_err(|_| storage_corrupted("completed package component hash is invalid"))?;
            validate_sha256("package document", &document_sha256)
                .map_err(|_| storage_corrupted("completed package document hash is invalid"))?;
            let authority = CompletedPackageDocumentAuthority {
                document_ordinal: row.1,
                target_object_id: row.2,
                target_revision_id: row.3,
                source_component_sha256,
                document_sha256,
                result_sha256: row.5,
            };
            if committed_documents
                .insert((row.0, row.1), (authority, result.value))
                .is_some()
            {
                return Err(storage_corrupted(
                    "completed package component result identity is duplicated",
                ));
            }
        }
        if committed_documents.len() != approval.document_bindings.len() {
            return Err(storage_corrupted(
                "completed package commit evidence count differs from approval",
            ));
        }
        for binding in &approval.document_bindings {
            let (document, _) = committed_documents
                .get(&(
                    binding.source_component_key.clone(),
                    binding.component_document_ordinal,
                ))
                .ok_or_else(|| {
                    storage_corrupted("completed package approval binding has no commit evidence")
                })?;
            if document.target_object_id != binding.target_object_id
                || document.source_component_sha256 != binding.source_component_sha256
                || document.document_sha256 != binding.document_sha256
            {
                return Err(storage_corrupted(
                    "completed package commit evidence differs from approval binding",
                ));
            }
        }
        validate_completed_authority_audit(connection, &current, &approval, &committed_documents)?;

        let enabled_components = approval
            .plan
            .components
            .iter()
            .filter(|component| component.enabled)
            .map(|component| {
                let mut documents = committed_documents
                    .iter()
                    .filter(|((component_id, _), _)| component_id == &component.component.id)
                    .map(|(_, (document, _))| document.clone())
                    .collect::<Vec<_>>();
                documents.sort_by_key(|document| document.document_ordinal);
                CompletedPackageComponentAuthority {
                    component_id: component.component.id.clone(),
                    kind: component.component.kind,
                    sha256: component.component.sha256.as_str().to_owned(),
                    committed_documents: documents,
                }
            })
            .collect();
        Ok(CompletedPackageAuthoritySnapshot {
            authority: CompletedPackageAuthority {
                approval_id: approval_id.to_owned(),
                import_id: approval_row.0,
                package_id: approval.plan.package_id,
                status: current.record.status,
                import_revision: current.record.revision,
                source_sha256: source.source_sha256,
                inspection_sha256: approval_row.1,
                selection_sha256: approval_row.2,
                capability_review_sha256: approval_row.3,
                approval_sha256: approval.plan.approval_sha256.as_str().to_owned(),
                required_capabilities: approval.plan.required_capabilities,
                approved_capabilities: approval.approved_capabilities,
                enabled_components,
                committed_assets,
            },
            cas_files,
        })
    }

    /// Builds the exact imported-module authority consumed by the pure module
    /// resolver. The caller supplies only a stored immutable module revision
    /// and an approval id; all package and component evidence is reloaded.
    pub fn get_module_import_approval_evidence(
        &self,
        approval_id: &str,
        stored: &ActiveContentModuleRevision,
    ) -> CoreResult<ModuleImportApprovalEvidence> {
        let verified = self.verify_completed_package_authority_with(
            approval_id,
            |connection, approval_id| {
                Self::get_completed_package_authority_by_approval_id_in_connection(
                    connection,
                    approval_id,
                )
            },
            || {},
        )?;
        let connection = self.connection()?;
        let authority = Self::revalidate_completed_package_authority_in_connection(
            &connection,
            approval_id,
            &verified,
        )?;
        build_module_import_approval_evidence_in_connection(&connection, stored, &authority)
    }

    /// Lists every completed package authority that committed this exact
    /// imported module revision.
    ///
    /// The deterministic list exists for restart and lost-response recovery.
    /// Callers must present multiple candidates for an explicit choice; this
    /// method never selects an approval merely because it is newest.
    pub fn list_completed_package_import_authorities_for_module_revision(
        &self,
        stored: &ActiveContentModuleRevision,
    ) -> CoreResult<Vec<ModuleImportApprovalEvidence>> {
        validate_completed_module_authority_target(stored)?;
        let candidate_limit = i64::try_from(MAX_COMPLETED_MODULE_AUTHORITIES + 1)
            .map_err(|_| CoreError::internal("completed module authority limit overflow"))?;
        let approval_ids = {
            let connection = self.connection()?;
            let mut statement = connection
                .prepare(
                    "SELECT DISTINCT approval.id, approval.approved_at
                     FROM package_import_approvals AS approval
                     JOIN package_imports AS import
                       ON import.id = approval.import_id
                     JOIN package_sources AS source
                       ON source.id = import.package_source_id
                     JOIN package_import_component_commits AS committed_document
                       ON committed_document.import_id = import.id
                     JOIN package_import_components AS component
                       ON component.import_id = committed_document.import_id
                      AND component.ordinal =
                          committed_document.component_ordinal
                     WHERE import.state = 'completed'
                       AND source.source_hash = ?1
                       AND component.component_kind = 'content_module'
                       AND committed_document.target_object_id = ?2
                       AND committed_document.target_revision_id = ?3
                     ORDER BY approval.approved_at, approval.id
                     LIMIT ?4",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![
                        stored.module_revision.source_hash.as_str(),
                        stored.object.value.id.as_str(),
                        stored.module_revision.id.as_str(),
                        candidate_limit,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        if approval_ids.len() > MAX_COMPLETED_MODULE_AUTHORITIES {
            return Err(storage_corrupted(
                "completed module authority candidates exceed the bounded recovery limit",
            ));
        }
        let verified =
            self.verify_completed_package_authorities(approval_ids.iter().map(String::as_str))?;
        let connection = self.connection()?;
        approval_ids
            .into_iter()
            .map(|approval_id| {
                let verified = verified.get(&approval_id).ok_or_else(|| {
                    storage_corrupted("completed module authority was not CAS-verified")
                })?;
                let authority = Self::revalidate_completed_package_authority_in_connection(
                    &connection,
                    &approval_id,
                    verified,
                )?;
                build_module_import_approval_evidence_in_connection(&connection, stored, &authority)
            })
            .collect()
    }

    /// Transaction-local variant used while package-backed module activation
    /// is re-reviewed under the same database snapshot as its bindings.
    pub(crate) fn get_module_import_approval_evidence_in_transaction(
        transaction: &Transaction<'_>,
        approval_id: &str,
        stored: &ActiveContentModuleRevision,
        verified_authorities: &VerifiedCompletedPackageAuthorities,
    ) -> CoreResult<ModuleImportApprovalEvidence> {
        // No CAS path is opened here. The transaction performs only an exact
        // metadata/revision revalidation of the proof created before it began.
        let verified = verified_authorities.get(approval_id).ok_or_else(|| {
            CoreError::invalid(
                "module package approval changed after CAS authority preverification",
            )
        })?;
        let authority = Self::revalidate_completed_package_authority_in_connection(
            transaction,
            approval_id,
            verified,
        )?;
        build_module_import_approval_evidence_in_connection(transaction, stored, &authority)
    }

    /// Discards a reviewed or approved import without deleting its immutable
    /// source, review, approval, component inventory or audit evidence.
    pub fn discard_inspected_package_import(
        &self,
        import_id: &str,
        expected: &PackageInspectionExpectation,
    ) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", import_id)?;
        validate_inspection_expectation(expected)?;
        let now = Utc::now();
        let next_revision = expected
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_import_state(&transaction, import_id)?;
        let payload = VersionedJson {
            schema_version: 1,
            value: json!({
                "revision": expected.revision,
                "inspection_sha256": expected.inspection_sha256,
                "capability_review_sha256": expected.capability_review_sha256,
            }),
        };
        if current.record.status == PackageImportStatus::Discarded
            && current.record.revision == next_revision
        {
            validate_inspected_discard_replay(&transaction, &current, expected, &payload)?;
            return Ok(current.record);
        }
        assert_inspection_expectation(&current, expected)?;
        if current.record.status != PackageImportStatus::Inspected
            || current.record.selection.is_some()
        {
            return Err(CoreError::invalid(
                "only an unselected inspected package import can use this discard path",
            ));
        }
        append_audit(
            &transaction,
            import_id,
            next_revision,
            "discarded",
            &payload,
            now,
        )?;
        update_import_state(
            &transaction,
            import_id,
            expected.revision,
            PackageImportStatus::Discarded,
            next_revision,
            None,
            None,
            None,
            Some(now),
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(import_id)
    }

    /// Discards a selected or approved import using all three reviewed hashes.
    pub fn discard_package_import(
        &self,
        import_id: &str,
        expected: &PackageImportExpectation,
    ) -> CoreResult<PackageImportRecord> {
        validate_identifier("package import", import_id)?;
        validate_expectation(expected)?;
        let now = Utc::now();
        let next_revision = expected
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_import_state(&transaction, import_id)?;
        let payload = expectation_payload(expected);
        if current.record.status == PackageImportStatus::Discarded
            && current.record.revision == next_revision
        {
            validate_selected_discard_replay(&transaction, &current, expected, &payload)?;
            return Ok(current.record);
        }
        assert_expectation(&current, expected)?;
        if !matches!(
            current.record.status,
            PackageImportStatus::AwaitingReview | PackageImportStatus::Approved
        ) {
            return Err(CoreError::invalid(
                "package import cannot be discarded from its current state",
            ));
        }
        append_audit(
            &transaction,
            import_id,
            next_revision,
            "discarded",
            &payload,
            now,
        )?;
        update_import_state(
            &transaction,
            import_id,
            expected.revision,
            PackageImportStatus::Discarded,
            next_revision,
            current.approved_selection_sha256.as_deref(),
            current.approved_at,
            None,
            Some(now),
            now,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(import_id)
    }

    /// Atomically appends all selected typed documents, immutable asset
    /// descriptors, per-component commit evidence and the final state change.
    ///
    /// Asset bytes and `assets` rows must already have been committed to CAS.
    #[allow(clippy::too_many_lines)] // Document, projection, audit, and state writes must stay atomic.
    pub fn commit_package_import(
        &self,
        input: &PackageCommitInput,
        expected: &PackageImportExpectation,
        bindings: &[PackageDocumentCommitBinding],
    ) -> CoreResult<PackageImportRecord> {
        validate_expectation(expected)?;
        validate_commit_input_shape(input, bindings)?;
        let source_hash = input.source.source_sha256.as_str();
        self.package_source_path(source_hash, input.source.source_size_bytes)?;
        for asset in &input.assets {
            self.verify_package_asset_cas(asset)?;
        }
        let now = Utc::now();
        let committing_revision = expected
            .revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let completed_revision = committing_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("package import revision overflow"))?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current = read_import_state(&transaction, &input.import.id)?;
        if current.record.status == PackageImportStatus::Completed {
            validate_completed_commit_replay(&transaction, &current, input, expected, bindings)?;
            claim_package_asset_promotions(&transaction, &input.import.id, &input.assets, false)?;
            transaction.commit().map_err(storage_db_error)?;
            return Ok(current.record);
        }
        assert_expectation(&current, expected)?;
        if current.record.status != PackageImportStatus::Approved {
            return Err(CoreError::invalid(
                "only an approved package import can be committed",
            ));
        }
        let stored_source = read_package_source_by_id(&transaction, &current.package_source_id)?;
        if stored_source != input.source || input.import != current.record {
            return Err(CoreError::invalid(
                "package commit input does not match the approved durable import",
            ));
        }
        let approval = read_approval_payload(&transaction, &input.import.id)?;
        if approval.plan.review_sha256.as_str() != expected.inspection_sha256
            || approval.plan.plan_sha256.as_str() != expected.selection_sha256
            || approval.plan.source_sha256.as_str() != source_hash
            || approval.plan.package_id != input.source.package_id
            || approval.document_bindings != bindings
        {
            return Err(CoreError::invalid(
                "package commit is not bound to the exact approval snapshot",
            ));
        }
        if input.assets != approval.plan.assets {
            return Err(CoreError::invalid(
                "package commit assets differ from the approved asset inventory",
            ));
        }
        let component_rows = load_selected_commit_components(&transaction, &input.import.id)?;
        let target_review = load_package_import_target_review(&transaction, &current)?;
        validate_commit_bindings(
            &transaction,
            &input.documents,
            bindings,
            &component_rows,
            &target_review,
            &approval.confirmed_update_targets,
        )?;
        validate_document_normalization_evidence(
            &input.documents,
            bindings,
            &approval.normalization_evidence,
        )?;

        let start_payload = VersionedJson {
            schema_version: 1,
            value: json!({
                "approval_sha256": approval.plan.approval_sha256.as_str(),
                "document_count": input.documents.len(),
                "asset_count": input.assets.len(),
            }),
        };
        append_audit(
            &transaction,
            &input.import.id,
            committing_revision,
            "commit_started",
            &start_payload,
            now,
        )?;
        update_import_state(
            &transaction,
            &input.import.id,
            expected.revision,
            PackageImportStatus::Committing,
            committing_revision,
            current.approved_selection_sha256.as_deref(),
            current.approved_at,
            None,
            None,
            now,
        )?;

        for asset in &input.assets {
            append_package_asset_descriptor(&transaction, asset, source_hash)?;
        }
        let binding_by_index = bindings
            .iter()
            .map(|binding| (binding.document_index as usize, binding))
            .collect::<BTreeMap<_, _>>();
        let mut committed = Vec::with_capacity(input.documents.len());
        for (index, document) in input.documents.iter().enumerate() {
            let binding = binding_by_index
                .get(&index)
                .ok_or_else(|| CoreError::internal("validated document binding disappeared"))?;
            let written = append_package_commit_document(
                &transaction,
                document,
                binding.expected_object_revision,
                source_hash,
            )?;
            let row = component_rows
                .get(&binding.source_component_key)
                .ok_or_else(|| CoreError::internal("validated component binding disappeared"))?;
            if written.object_id != document_object_id(document) {
                return Err(storage_corrupted(
                    "package document helper returned an unexpected object identity",
                ));
            }
            let result = VersionedJson {
                schema_version: 1,
                value: json!({
                    "source_component_key": binding.source_component_key,
                    "component_document_ordinal": binding.component_document_ordinal,
                    "source_component_sha256": binding.source_component_sha256,
                    "document_sha256": binding.document_sha256,
                    "target_object_id": written.object_id,
                    "target_revision_id": written.revision_id,
                    "target_state_revision": written.state_revision,
                }),
            };
            let result_json = encode_json("package component commit result", &result)?;
            let result_sha256 = sha256_hex(result_json.as_bytes());
            transaction
                .execute(
                    "INSERT INTO package_import_component_commits (
                        import_id, component_ordinal, document_ordinal, target_object_id,
                        target_revision_id, result_json, result_sha256,
                        committed_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        input.import.id,
                        i64::from(row.ordinal),
                        i64::from(binding.component_document_ordinal),
                        written.object_id,
                        written.revision_id,
                        result_json,
                        result_sha256,
                        now.to_rfc3339(),
                    ],
                )
                .map_err(storage_db_error)?;
            committed.push(result.value);
        }
        let completion_payload = VersionedJson {
            schema_version: 1,
            value: json!({
                "approval_sha256": approval.plan.approval_sha256.as_str(),
                "components": committed,
                "asset_ids": input.assets.iter()
                    .map(|asset| asset.id.as_str())
                    .collect::<Vec<_>>(),
            }),
        };
        append_audit(
            &transaction,
            &input.import.id,
            completed_revision,
            "commit_completed",
            &completion_payload,
            now,
        )?;
        update_import_state(
            &transaction,
            &input.import.id,
            committing_revision,
            PackageImportStatus::Completed,
            completed_revision,
            current.approved_selection_sha256.as_deref(),
            current.approved_at,
            None,
            Some(now),
            now,
        )?;
        claim_package_asset_promotions(&transaction, &input.import.id, &input.assets, true)?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        self.get_package_import(&input.import.id)
    }

    pub fn list_package_import_audit(
        &self,
        import_id: &str,
    ) -> CoreResult<Vec<PackageImportAuditEvent>> {
        validate_identifier("package import", import_id)?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT sequence, import_revision, event_kind, payload_json,
                        payload_sha256, created_at
                 FROM package_import_audit_events
                 WHERE import_id = ?1
                 ORDER BY sequence",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([import_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .map(|row| {
                let expected_hash = sha256_hex(row.3.as_bytes());
                if expected_hash != row.4 {
                    return Err(storage_corrupted(
                        "package import audit payload hash does not match",
                    ));
                }
                Ok(PackageImportAuditEvent {
                    sequence: u64_from_i64("package audit sequence", row.0)?,
                    import_revision: u64_from_i64("package audit revision", row.1)?,
                    event_kind: row.2,
                    payload: decode_json("package audit payload", &row.3)?,
                    payload_sha256: row.4,
                    created_at: parse_datetime("package audit created_at", &row.5)?,
                })
            })
            .collect()
    }
}

pub fn package_capability_review_sha256(review: &PackageCapabilityReview) -> CoreResult<String> {
    validate_capability_review(review, &[])?;
    let mut canonical = review.clone();
    canonical
        .decisions
        .sort_by_key(|decision| decision.capability);
    let json = encode_json("package capability review", &canonical)?;
    Ok(sha256_hex(json.as_bytes()))
}

pub fn package_normalization_evidence_sha256(
    evidence: &[PackageNormalizationEvidence],
) -> CoreResult<String> {
    validate_normalization_evidence_shape(evidence)?;
    let wrapper = VersionedJson {
        schema_version: 1,
        value: serde_json::to_value(evidence).map_err(|error| {
            CoreError::invalid(format!(
                "package normalization evidence cannot be encoded: {error}"
            ))
        })?,
    };
    Ok(sha256_hex(
        encode_json("package normalization evidence", &wrapper)?.as_bytes(),
    ))
}

fn validate_inspected_import(
    source: &PackageSourceRecord,
    import: &PackageImportRecord,
    review: &PackageReview,
    capability_review: &PackageCapabilityReview,
) -> CoreResult<()> {
    validate_source_record(source)?;
    review
        .verify()
        .map_err(|error| CoreError::invalid(format!("package review is invalid: {error}")))?;
    if import.status != PackageImportStatus::Inspected
        || import.revision != 1
        || import.selection.is_some()
        || !import.selected_component_ids.is_empty()
        || import.failure_code.is_some()
    {
        return Err(CoreError::invalid(
            "a new package inspection must begin unselected at revision 1",
        ));
    }
    validate_identifier("package import", &import.id)?;
    if import.package_id != source.package_id
        || review.manifest.package_id != source.package_id
        || review.source_sha256.as_str() != source.source_sha256
    {
        return Err(CoreError::invalid(
            "package source and inspection identities do not match",
        ));
    }
    let manifest_value = serde_json::to_value(&review.manifest).map_err(|error| {
        CoreError::invalid(format!("package manifest cannot be encoded: {error}"))
    })?;
    let inspection_value = serde_json::to_value(review).map_err(|error| {
        CoreError::invalid(format!("package review cannot be encoded: {error}"))
    })?;
    if source.manifest.schema_version != 1
        || source.manifest.value != manifest_value
        || import.inspection.schema_version != 1
        || import.inspection.value != inspection_value
    {
        return Err(CoreError::invalid(
            "package inspection wrappers do not contain the exact reviewed payloads",
        ));
    }
    if source.format != review.manifest.format
        || source.format_version != review.manifest.format_version
        || source.name != review.manifest.name
        || source.version != review.manifest.version
        || source.author != review.manifest.author
        || source.license != review.manifest.license
        || source.redistribution_allowed != review.manifest.redistribution_allowed
        || import.updated_at < import.created_at
    {
        return Err(CoreError::invalid(
            "package inspection metadata does not match the reviewed manifest",
        ));
    }
    validate_capability_review(capability_review, &[])
}

fn verify_source_cas(storage: &Storage, source: &PackageSourceRecord) -> CoreResult<()> {
    storage
        .package_source_path(&source.source_sha256, source.source_size_bytes)
        .map(|_| ())
}

fn validate_source_record(source: &PackageSourceRecord) -> CoreResult<()> {
    validate_identifier("package source", &source.id)?;
    validate_identifier("package", source.package_id.as_str())?;
    validate_sha256("package source", &source.source_sha256)?;
    if source.format_version == 0
        || source.manifest.schema_version != 1
        || source.name.trim().is_empty()
        || source.version.trim().is_empty()
        || source.source_size_bytes > i64::MAX.unsigned_abs()
    {
        return Err(CoreError::invalid("package source metadata is invalid"));
    }
    if !matches!(
        source.format.as_str(),
        "lorepia_content_package" | "public_character_card" | "compat_import"
    ) {
        return Err(CoreError::invalid("package source format is unsupported"));
    }
    encode_json("package source manifest", &source.manifest)?;
    Ok(())
}

fn validate_capability_review(
    review: &PackageCapabilityReview,
    required: &[ContentCapability],
) -> CoreResult<()> {
    if review.schema_version != 1 {
        return Err(CoreError::invalid(
            "package capability review schema version is unsupported",
        ));
    }
    if review.decisions.len() != PackageCapability::ALL.len() {
        return Err(CoreError::invalid(
            "package capability review must contain the complete policy matrix",
        ));
    }
    let mut seen = BTreeSet::new();
    for decision in &review.decisions {
        if !seen.insert(decision.capability) {
            return Err(CoreError::invalid(
                "package capability review contains duplicate decisions",
            ));
        }
        if decision.support != decision.capability.required_support() || decision.approved {
            return Err(CoreError::invalid(
                "package capability decision differs from the storage safety policy",
            ));
        }
        if decision.reason.trim().is_empty()
            || decision.reason.len() > MAX_CAPABILITY_REASON_BYTES
            || decision.reason.chars().any(char::is_control)
        {
            return Err(CoreError::invalid(
                "package capability decision reason is invalid",
            ));
        }
    }
    if seen != PackageCapability::ALL.into_iter().collect() {
        return Err(CoreError::invalid(
            "package capability review policy matrix is incomplete",
        ));
    }
    for capability in required {
        let expected = PackageCapability::from(*capability);
        let decision = review
            .decisions
            .iter()
            .find(|decision| decision.capability == expected)
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "required package capability {} has no review decision",
                    expected.as_str()
                ))
            })?;
        if decision.support == PackageCapabilitySupport::Unsupported {
            return Err(CoreError::invalid(format!(
                "selected package requires unsupported capability {}",
                expected.as_str()
            )));
        }
    }
    Ok(())
}

fn validate_stored_capability_decisions(
    connection: &Connection,
    import_id: &str,
    required: &[ContentCapability],
) -> CoreResult<()> {
    for capability in required {
        let capability = PackageCapability::from(*capability);
        let decision = connection
            .query_row(
                "SELECT support_status
                 FROM package_capability_requests
                 WHERE import_id = ?1 AND capability = ?2",
                params![import_id, capability.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "required package capability {} has no durable review",
                    capability.as_str()
                ))
            })?;
        if decision == "unsupported" {
            return Err(CoreError::invalid(format!(
                "required package capability {} is unsupported",
                capability.as_str()
            )));
        }
    }
    Ok(())
}

fn validate_capability_approval_snapshot(
    connection: &Connection,
    import_id: &str,
    required: &[ContentCapability],
    approved: &[PackageCapability],
) -> CoreResult<()> {
    let review = read_capability_review(connection, import_id)?;
    let required_set = required
        .iter()
        .copied()
        .map(PackageCapability::from)
        .collect::<BTreeSet<_>>();
    let mut expected_approvals = BTreeSet::new();
    for capability in required_set {
        let decision = review
            .decisions
            .iter()
            .find(|decision| decision.capability == capability)
            .ok_or_else(|| {
                storage_corrupted("required package capability review decision is missing")
            })?;
        match decision.support {
            PackageCapabilitySupport::Supported => {}
            PackageCapabilitySupport::ApprovalRequired => {
                if capability.is_never_approvable() {
                    return Err(CoreError::invalid(
                        "unsafe package capability cannot be approved",
                    ));
                }
                expected_approvals.insert(capability);
            }
            PackageCapabilitySupport::Unsupported => {
                return Err(CoreError::invalid(
                    "unsupported package capability cannot be approved",
                ));
            }
        }
    }
    let supplied = approved.iter().copied().collect::<BTreeSet<_>>();
    if supplied.len() != approved.len() || supplied != expected_approvals {
        return Err(CoreError::invalid(
            "package capability approval does not match the exact required review",
        ));
    }
    Ok(())
}

fn insert_package_source(
    transaction: &Transaction<'_>,
    source: &PackageSourceRecord,
) -> CoreResult<()> {
    validate_source_record(source)?;
    let content_source = transaction
        .query_row(
            "SELECT size_bytes FROM content_sources WHERE sha256 = ?1",
            [source.source_sha256.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::invalid(
                "package source bytes must be durable in content-addressed storage first",
            )
        })?;
    if u64_from_i64("package source size", content_source)? != source.source_size_bytes {
        return Err(CoreError::invalid(
            "package source size does not match content-addressed storage",
        ));
    }
    if let Some(existing) = read_package_source(transaction, "source.id = ?1", &source.id)? {
        if existing == *source {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "package source conflicts with an existing immutable source",
        ));
    }
    if let Some(existing) = read_package_source(
        transaction,
        "source.source_hash = ?1",
        &source.source_sha256,
    )? {
        if existing == *source {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "package source hash is already bound to different immutable metadata",
        ));
    }
    let manifest_json = encode_json("package source manifest", &source.manifest)?;
    let manifest_sha256 = sha256_hex(manifest_json.as_bytes());
    let (license_expression, license_status) = license_fields(&source.license);
    let redistribution_status = if source.redistribution_allowed {
        "allowed"
    } else {
        "denied"
    };
    let signature = source
        .manifest
        .value
        .get("signature")
        .filter(|value| !value.is_null());
    let signature_json = signature
        .map(|value| encode_json("package signature", value))
        .transpose()?;
    let signature_status = if signature.is_some() {
        "untrusted"
    } else {
        "unsigned"
    };
    let required_app_version = source
        .manifest
        .value
        .get("required_app_version")
        .and_then(Value::as_str);
    transaction
        .execute(
            "INSERT INTO package_sources (
                id, source_hash, format, format_version, package_id, name,
                version, author, manifest_json, manifest_sha256,
                license_expression, license_status, redistribution_status,
                required_app_version, signature_json, signature_status,
                created_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17
             )",
            params![
                source.id,
                source.source_sha256,
                source.format,
                i64::from(source.format_version),
                source.package_id.as_str(),
                source.name,
                source.version,
                source.author,
                manifest_json,
                manifest_sha256,
                license_expression,
                license_status,
                redistribution_status,
                required_app_version,
                signature_json,
                signature_status,
                source.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn read_package_source(
    connection: &Connection,
    predicate: &str,
    value: &str,
) -> CoreResult<Option<PackageSourceRecord>> {
    let sql = format!(
        "SELECT source.id, source.package_id, source.format,
                source.format_version, source.name, source.version,
                source.source_hash, bytes.size_bytes, source.author,
                source.license_expression, source.license_status,
                source.redistribution_status, source.manifest_json,
                source.manifest_sha256,
                source.created_at
         FROM package_sources AS source
         JOIN content_sources AS bytes ON bytes.sha256 = source.source_hash
         WHERE {predicate}
         ORDER BY source.created_at, source.id
         LIMIT 1"
    );
    connection
        .query_row(&sql, [value], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<String>>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, String>(14)?,
            ))
        })
        .optional()
        .map_err(storage_db_error)?
        .map(|row| {
            if sha256_hex(row.12.as_bytes()) != row.13 {
                return Err(storage_corrupted(
                    "stored package manifest hash does not match",
                ));
            }
            let manifest = decode_json("package source manifest", &row.12)?;
            let license = row.9.unwrap_or_else(|| match row.10.as_str() {
                "unknown" => "LicenseRef-Unknown".to_owned(),
                "invalid" => "LicenseRef-Invalid".to_owned(),
                _ => String::new(),
            });
            Ok(PackageSourceRecord {
                id: row.0,
                package_id: PackageId::from(row.1),
                format: row.2,
                format_version: u32_from_i64("package format version", row.3)?,
                name: row.4,
                version: row.5,
                source_sha256: row.6,
                source_size_bytes: u64_from_i64("package source size", row.7)?,
                author: row.8,
                license,
                redistribution_allowed: row.11 == "allowed",
                manifest,
                created_at: parse_datetime("package source created_at", &row.14)?,
            })
        })
        .transpose()
}

fn read_package_source_by_id(connection: &Connection, id: &str) -> CoreResult<PackageSourceRecord> {
    read_package_source(connection, "source.id = ?1", id)?
        .ok_or_else(|| storage_corrupted("package import source is missing"))
}

fn read_source_hash(connection: &Connection, source_id: &str) -> CoreResult<String> {
    connection
        .query_row(
            "SELECT source_hash FROM package_sources WHERE id = ?1",
            [source_id],
            |row| row.get(0),
        )
        .map_err(storage_db_error)
}

fn validate_selection_against_review(
    review: &PackageReview,
    selection: &SelectiveImportPlan,
) -> CoreResult<()> {
    if !review.local_import_allowed {
        return Err(CoreError::invalid(
            "blocked package review cannot be selected for import",
        ));
    }
    if selection.review_sha256 != review.review_sha256
        || selection.source_sha256 != review.source_sha256
        || selection.package_id != review.manifest.package_id
        || selection.redistribution_status != review.redistribution_status
    {
        return Err(CoreError::invalid(
            "package selection differs from its exact review",
        ));
    }
    let reviewed_components = review
        .components
        .iter()
        .map(|component| (component.id.as_str(), component))
        .collect::<BTreeMap<_, _>>();
    let mut required_asset_ids = BTreeSet::new();
    for planned in &selection.components {
        let reviewed = reviewed_components
            .get(planned.component.id.as_str())
            .ok_or_else(|| CoreError::invalid("package selection contains an unknown component"))?;
        if **reviewed != planned.component
            || planned.component.disposition != PackageComponentDisposition::Importable
        {
            return Err(CoreError::invalid(
                "package selection component differs from its reviewed descriptor",
            ));
        }
        required_asset_ids.extend(
            planned
                .component
                .asset_ids
                .iter()
                .map(lorepia_domain::AssetId::as_str),
        );
    }
    let reviewed_assets = review
        .assets
        .iter()
        .map(|asset| (asset.descriptor.id.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    let selected_asset_ids = selection
        .assets
        .iter()
        .map(|asset| asset.id.as_str())
        .collect::<BTreeSet<_>>();
    if selected_asset_ids.len() != selection.assets.len()
        || !required_asset_ids.is_subset(&selected_asset_ids)
    {
        return Err(CoreError::invalid(
            "package selection asset closure is incomplete or duplicated",
        ));
    }
    for asset in &selection.assets {
        let reviewed = reviewed_assets
            .get(asset.id.as_str())
            .ok_or_else(|| CoreError::invalid("package selection contains an unknown asset"))?;
        if reviewed.descriptor != *asset
            || reviewed.disposition != lorepia_orchestration::AssetImportDisposition::Importable
        {
            return Err(CoreError::invalid(
                "package selection asset differs from its reviewed descriptor",
            ));
        }
    }
    Ok(())
}

struct ReviewedPackageSelectionRows {
    components: Vec<ReviewedComponentRow>,
    documents: Vec<PackageDocumentTargetReview>,
    target_review_sha256: String,
}

#[allow(clippy::too_many_lines)] // One pass must bind parent summaries to every child target row.
fn reviewed_selection_rows(
    connection: &Connection,
    review: &PackageReview,
    selection: &SelectiveImportPlan,
    document_bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<ReviewedPackageSelectionRows> {
    validate_binding_snapshot_shape(document_bindings)?;
    let selected = selection
        .components
        .iter()
        .map(|component| component.component.id.clone())
        .collect::<BTreeSet<_>>();
    let mut bindings_by_component = BTreeMap::<String, Vec<&PackageDocumentCommitBinding>>::new();
    for binding in document_bindings {
        if !selected.contains(&binding.source_component_key) {
            return Err(CoreError::invalid(
                "package selection binding names an unselected component",
            ));
        }
        bindings_by_component
            .entry(binding.source_component_key.clone())
            .or_default()
            .push(binding);
    }
    let mut rows = Vec::with_capacity(review.components.len());
    let mut document_reviews = Vec::with_capacity(document_bindings.len());
    for (ordinal, component) in review.components.iter().enumerate() {
        let ordinal = u32::try_from(ordinal)
            .map_err(|_| CoreError::invalid("too many package components"))?;
        let component_kind = component_kind_str(component.kind);
        let is_selected = selected.contains(&component.id);
        let component_bindings = bindings_by_component
            .remove(&component.id)
            .unwrap_or_default();
        let (disposition, target_object_id, target_revision_id) = match component.disposition {
            PackageComponentDisposition::Unsupported => ("unsupported".to_owned(), None, None),
            PackageComponentDisposition::Quarantined => ("quarantine".to_owned(), None, None),
            PackageComponentDisposition::Importable if !is_selected => {
                ("skip".to_owned(), None, None)
            }
            PackageComponentDisposition::Importable
                if matches!(
                    component.kind,
                    PackageComponentKind::AssetIndex | PackageComponentKind::RawExtension
                ) =>
            {
                if !component_bindings.is_empty() {
                    return Err(CoreError::invalid(
                        "package asset selection cannot carry document bindings",
                    ));
                }
                ("create".to_owned(), None, None)
            }
            PackageComponentDisposition::Importable => {
                if component_bindings.is_empty() {
                    return Err(CoreError::invalid(
                        "selected package document has no reviewed target binding",
                    ));
                }
                let mut component_document_reviews = Vec::with_capacity(component_bindings.len());
                for binding in &component_bindings {
                    if binding.source_component_key != component.id
                        || binding.source_component_sha256 != component.sha256.as_str()
                        || binding.document_kind != component_kind
                    {
                        return Err(CoreError::invalid(
                            "package selection target differs from its reviewed component",
                        ));
                    }
                    component_document_reviews
                        .push(reviewed_document_target(connection, component, binding)?);
                }
                let update_count = component_document_reviews
                    .iter()
                    .filter(|review| review.disposition == PackageDocumentTargetDisposition::Update)
                    .count();
                let aggregate = if update_count == 0 {
                    "create"
                } else if update_count == component_document_reviews.len() {
                    "update"
                } else {
                    "conflict"
                };
                let exact_single_update = (component_document_reviews.len() == 1
                    && update_count == 1)
                    .then(|| &component_document_reviews[0]);
                let target_object_id =
                    exact_single_update.map(|review| review.target_object_id.clone());
                let target_revision_id = exact_single_update
                    .and_then(|review| review.expected_target_revision_id.clone());
                document_reviews.extend(component_document_reviews);
                (aggregate.to_owned(), target_object_id, target_revision_id)
            }
        };
        if !is_selected && !component_bindings.is_empty() {
            return Err(CoreError::invalid(
                "unselected package component cannot carry document bindings",
            ));
        }
        let review_json = encode_json("package component review", component)?;
        let review_sha256 = sha256_hex(review_json.as_bytes());
        rows.push(ReviewedComponentRow {
            ordinal,
            source_component_key: component.id.clone(),
            component_kind: component_kind.to_owned(),
            disposition,
            selected: is_selected,
            target_object_id,
            target_revision_id,
            review_json,
            review_sha256,
        });
    }
    if !bindings_by_component.is_empty() {
        return Err(CoreError::invalid(
            "package selection binding names an unknown reviewed component",
        ));
    }
    rows.sort_by_key(|row| row.ordinal);
    document_reviews.sort_by_key(|review| review.document_index);
    validate_document_target_reviews(&document_reviews)?;
    let target_review_sha256 = package_import_target_review_sha256(&document_reviews)?;
    Ok(ReviewedPackageSelectionRows {
        components: rows,
        documents: document_reviews,
        target_review_sha256,
    })
}

fn reviewed_document_target(
    connection: &Connection,
    component: &lorepia_orchestration::PackageComponentDescriptor,
    binding: &PackageDocumentCommitBinding,
) -> CoreResult<PackageDocumentTargetReview> {
    let (disposition, expected_target_revision_id, expected_target_state_revision) =
        if let Some(expected_revision) = binding.expected_object_revision {
            let target = connection
                .query_row(
                    "SELECT object.object_kind, object.deleted_at,
                    state.state_version, state.active_revision_id
             FROM content_objects AS object
             JOIN content_object_state AS state
               ON state.object_id = object.id
             WHERE object.id = ?1",
                    [binding.target_object_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    CoreError::invalid("package update target is missing at review time")
                })?;
            if target.0 != binding.document_kind
                || target.1.is_some()
                || u64_from_i64("content state revision", target.2)? != expected_revision
            {
                return Err(CoreError::invalid(
                    "package update target changed before selection was stored",
                ));
            }
            (
                PackageDocumentTargetDisposition::Update,
                Some(target.3),
                Some(expected_revision),
            )
        } else {
            let target_exists = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM content_objects WHERE id = ?1)",
                    [binding.target_object_id.as_str()],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if target_exists {
                return Err(CoreError::invalid(
                    "new package target appeared before selection was stored",
                ));
            }
            (PackageDocumentTargetDisposition::Create, None, None)
        };
    Ok(PackageDocumentTargetReview {
        source_component_id: component.id.clone(),
        component_document_ordinal: binding.component_document_ordinal,
        document_index: binding.document_index,
        document_kind: binding.document_kind.clone(),
        target_object_id: binding.target_object_id.clone(),
        disposition,
        expected_target_revision_id,
        expected_target_state_revision,
        source_component_sha256: binding.source_component_sha256.clone(),
        document_sha256: binding.document_sha256.clone(),
    })
}

fn validate_document_target_reviews(documents: &[PackageDocumentTargetReview]) -> CoreResult<()> {
    if documents.len() > MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS {
        return Err(CoreError::invalid(format!(
            "package target review exceeds the {MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS}-document limit"
        )));
    }
    let mut component_documents = BTreeSet::new();
    let mut target_ids = BTreeSet::new();
    let mut ordinals_by_component = BTreeMap::<&str, Vec<u32>>::new();
    for (expected_index, document) in documents.iter().enumerate() {
        validate_identifier(
            "package target-review component",
            &document.source_component_id,
        )?;
        validate_identifier("package target-review object", &document.target_object_id)?;
        if !matches!(
            document.document_kind.as_str(),
            "prompt_preset"
                | "knowledge_book"
                | "memory_profile"
                | "transform_set"
                | "interaction_rule_set"
                | "content_module"
                | "character_content"
        ) {
            return Err(CoreError::invalid(
                "package target-review document kind is invalid",
            ));
        }
        validate_sha256(
            "package target-review component",
            &document.source_component_sha256,
        )?;
        validate_sha256("package target-review document", &document.document_sha256)?;
        if usize::try_from(document.document_index) != Ok(expected_index) {
            return Err(CoreError::invalid(
                "package target-review document indices must be contiguous",
            ));
        }
        if !component_documents.insert((
            document.source_component_id.as_str(),
            document.component_document_ordinal,
        )) {
            return Err(CoreError::invalid(
                "package target review contains a duplicate component document",
            ));
        }
        if !target_ids.insert(document.target_object_id.as_str()) {
            return Err(CoreError::invalid(
                "package target review contains a duplicate target object",
            ));
        }
        match document.disposition {
            PackageDocumentTargetDisposition::Create
                if document.expected_target_revision_id.is_none()
                    && document.expected_target_state_revision.is_none() => {}
            PackageDocumentTargetDisposition::Update
                if document
                    .expected_target_revision_id
                    .as_ref()
                    .is_some_and(|revision| !revision.trim().is_empty())
                    && document
                        .expected_target_state_revision
                        .is_some_and(|revision| revision > 0) =>
            {
                validate_identifier(
                    "package target-review revision",
                    document
                        .expected_target_revision_id
                        .as_deref()
                        .unwrap_or_default(),
                )?;
            }
            _ => {
                return Err(CoreError::invalid(
                    "package target-review disposition and expectation differ",
                ));
            }
        }
        ordinals_by_component
            .entry(&document.source_component_id)
            .or_default()
            .push(document.component_document_ordinal);
    }
    for ordinals in ordinals_by_component.values_mut() {
        ordinals.sort_unstable();
        if ordinals
            .iter()
            .enumerate()
            .any(|(expected, actual)| usize::try_from(*actual) != Ok(expected))
        {
            return Err(CoreError::invalid(
                "package target-review component ordinals must be contiguous",
            ));
        }
    }
    Ok(())
}

pub fn package_import_target_review_sha256(
    documents: &[PackageDocumentTargetReview],
) -> CoreResult<String> {
    validate_document_target_reviews(documents)?;
    let encoded = encode_json("package target review", &documents)?;
    Ok(sha256_hex(encoded.as_bytes()))
}

pub fn package_update_target_confirmations_sha256(
    confirmations: &[PackageUpdateTargetConfirmation],
) -> CoreResult<String> {
    let canonical = canonical_update_target_confirmations(confirmations)?;
    let encoded = encode_json("package update target confirmations", &canonical)?;
    Ok(sha256_hex(encoded.as_bytes()))
}

#[derive(Serialize)]
struct ReviewedComponentRowDigest<'a> {
    ordinal: u32,
    source_component_key: &'a str,
    component_kind: &'a str,
    disposition: &'a str,
    selected: bool,
    target_object_id: Option<&'a str>,
    target_revision_id: Option<&'a str>,
    review_sha256: &'a str,
}

fn reviewed_component_rows_sha256(rows: &[ReviewedComponentRow]) -> CoreResult<String> {
    let mut digests = Vec::with_capacity(rows.len());
    for row in rows {
        if sha256_hex(row.review_json.as_bytes()) != row.review_sha256 {
            return Err(storage_corrupted(
                "package component review digest does not match its payload",
            ));
        }
        digests.push(ReviewedComponentRowDigest {
            ordinal: row.ordinal,
            source_component_key: &row.source_component_key,
            component_kind: &row.component_kind,
            disposition: &row.disposition,
            selected: row.selected,
            target_object_id: row.target_object_id.as_deref(),
            target_revision_id: row.target_revision_id.as_deref(),
            review_sha256: &row.review_sha256,
        });
    }
    let encoded = encode_json("package component review rows", &digests)?;
    Ok(sha256_hex(encoded.as_bytes()))
}

fn load_reviewed_components(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<Vec<ReviewedComponentRow>> {
    let mut statement = connection
        .prepare(
            "SELECT ordinal, source_component_key, component_kind,
                    disposition, selected, target_object_id,
                    target_revision_id, review_json, review_sha256
             FROM package_import_components
             WHERE import_id = ?1
             ORDER BY ordinal",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([import_id], |row| {
            Ok(ReviewedComponentRow {
                ordinal: row.get::<_, u32>(0)?,
                source_component_key: row.get(1)?,
                component_kind: row.get(2)?,
                disposition: row.get(3)?,
                selected: row.get(4)?,
                target_object_id: row.get(5)?,
                target_revision_id: row.get(6)?,
                review_json: row.get(7)?,
                review_sha256: row.get(8)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn insert_reviewed_components(
    transaction: &Transaction<'_>,
    import_id: &str,
    rows: &[ReviewedComponentRow],
) -> CoreResult<()> {
    for row in rows {
        transaction
            .execute(
                "INSERT INTO package_import_components (
                    import_id, ordinal, source_component_key, component_kind,
                    disposition, selected, target_object_id,
                    target_revision_id, review_json, review_sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    import_id,
                    i64::from(row.ordinal),
                    row.source_component_key,
                    row.component_kind,
                    row.disposition,
                    row.selected,
                    row.target_object_id,
                    row.target_revision_id,
                    row.review_json,
                    row.review_sha256,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn insert_document_target_reviews(
    transaction: &Transaction<'_>,
    import_id: &str,
    components: &[ReviewedComponentRow],
    documents: &[PackageDocumentTargetReview],
) -> CoreResult<()> {
    validate_document_target_reviews(documents)?;
    let component_ordinals = components
        .iter()
        .map(|component| (component.source_component_key.as_str(), component.ordinal))
        .collect::<BTreeMap<_, _>>();
    for document in documents {
        let component_ordinal = component_ordinals
            .get(document.source_component_id.as_str())
            .copied()
            .ok_or_else(|| {
                CoreError::invalid("package target review names an unknown component")
            })?;
        transaction
            .execute(
                "INSERT INTO package_import_document_target_reviews (
                    import_id, component_ordinal, document_ordinal,
                    document_index, document_kind, target_object_id,
                    disposition, expected_target_revision_id,
                    expected_target_state_revision, source_component_sha256,
                    document_sha256
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    import_id,
                    i64::from(component_ordinal),
                    i64::from(document.component_document_ordinal),
                    i64::from(document.document_index),
                    document.document_kind,
                    document.target_object_id,
                    package_document_target_disposition_str(document.disposition),
                    document.expected_target_revision_id,
                    document
                        .expected_target_state_revision
                        .map(|revision| i64_from_u64("package target state revision", revision))
                        .transpose()?,
                    document.source_component_sha256,
                    document.document_sha256,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn load_document_target_reviews(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<Vec<PackageDocumentTargetReview>> {
    let mut statement = connection
        .prepare(
            "SELECT component.source_component_key,
                    target.document_ordinal, target.document_index,
                    target.document_kind, target.target_object_id,
                    target.disposition, target.expected_target_revision_id,
                    target.expected_target_state_revision,
                    target.source_component_sha256, target.document_sha256
             FROM package_import_document_target_reviews AS target
             JOIN package_import_components AS component
               ON component.import_id = target.import_id
              AND component.ordinal = target.component_ordinal
             WHERE target.import_id = ?1
             ORDER BY target.document_index",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([import_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, u32>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    let documents = rows
        .into_iter()
        .map(|row| {
            Ok(PackageDocumentTargetReview {
                source_component_id: row.0,
                component_document_ordinal: row.1,
                document_index: row.2,
                document_kind: row.3,
                target_object_id: row.4,
                disposition: parse_package_document_target_disposition(&row.5)?,
                expected_target_revision_id: row.6,
                expected_target_state_revision: row
                    .7
                    .map(|revision| u64_from_i64("package target state revision", revision))
                    .transpose()?,
                source_component_sha256: row.8,
                document_sha256: row.9,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    validate_document_target_reviews(&documents).map_err(|error| {
        storage_corrupted(format!(
            "stored package target review is invalid: {}",
            error.message
        ))
    })?;
    Ok(documents)
}

fn load_package_import_target_review(
    connection: &Connection,
    current: &StoredImportState,
) -> CoreResult<PackageImportTargetReview> {
    let component_rows = load_reviewed_components(connection, &current.record.id)?;
    let component_review_sha256 = reviewed_component_rows_sha256(&component_rows)?;
    let documents = load_document_target_reviews(connection, &current.record.id)?;
    let target_review_sha256 =
        package_import_target_review_sha256(&documents).map_err(|error| {
            storage_corrupted(format!(
                "stored package target review cannot be hashed: {}",
                error.message
            ))
        })?;
    let audit_rows = {
        let mut statement = connection
            .prepare(
                "SELECT payload_json, payload_sha256
                 FROM package_import_audit_events
                 WHERE import_id = ?1 AND event_kind = 'review_requested'
                 ORDER BY sequence",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([current.record.id.as_str()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    if audit_rows.len() != 1 || sha256_hex(audit_rows[0].0.as_bytes()) != audit_rows[0].1 {
        return Err(storage_corrupted(
            "package target review has no exact immutable selection audit",
        ));
    }
    let audit: VersionedJson = decode_json("package target-review audit", &audit_rows[0].0)?;
    if audit.schema_version != 1
        || audit
            .value
            .get("component_review_sha256")
            .and_then(Value::as_str)
            != Some(component_review_sha256.as_str())
        || audit
            .value
            .get("target_review_sha256")
            .and_then(Value::as_str)
            != Some(target_review_sha256.as_str())
    {
        return Err(storage_corrupted(
            "package target-review digest differs from its selection audit",
        ));
    }
    let target_review = PackageImportTargetReview {
        target_review_sha256,
        documents,
    };
    target_review.verify().map_err(|error| {
        storage_corrupted(format!(
            "stored package target review is invalid: {}",
            error.message
        ))
    })?;
    Ok(target_review)
}

fn package_document_target_disposition_str(
    disposition: PackageDocumentTargetDisposition,
) -> &'static str {
    match disposition {
        PackageDocumentTargetDisposition::Create => "create",
        PackageDocumentTargetDisposition::Update => "update",
    }
}

fn parse_package_document_target_disposition(
    value: &str,
) -> CoreResult<PackageDocumentTargetDisposition> {
    match value {
        "create" => Ok(PackageDocumentTargetDisposition::Create),
        "update" => Ok(PackageDocumentTargetDisposition::Update),
        _ => Err(storage_corrupted(
            "stored package target-review disposition is invalid",
        )),
    }
}

fn insert_capability_review(
    transaction: &Transaction<'_>,
    import_id: &str,
    review: &PackageCapabilityReview,
) -> CoreResult<()> {
    for decision in &review.decisions {
        transaction
            .execute(
                "INSERT INTO package_capability_requests (
                    import_id, capability, support_status, approved,
                    executable, reason
                 ) VALUES (?1, ?2, ?3, ?4, 0, ?5)",
                params![
                    import_id,
                    decision.capability.as_str(),
                    decision.support.as_str(),
                    decision.approved,
                    decision.reason,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // A read rejects cross-table snapshot drift before returning state.
fn read_import_state(connection: &Connection, id: &str) -> CoreResult<StoredImportState> {
    let row = connection
        .query_row(
            "SELECT source.package_id, import.state, import.revision,
                    import.inspection_json, import.inspection_sha256,
                    import.selection_json, import.selection_sha256,
                    import.capability_review_sha256,
                    import.approved_selection_sha256, import.approved_at,
                    import.failure_json, import.created_at, import.updated_at,
                    import.completed_at, import.package_source_id
             FROM package_imports AS import
             JOIN package_sources AS source
               ON source.id = import.package_source_id
             WHERE import.id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, String>(14)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("package import"))?;
    let selected_component_ids = {
        let mut statement = connection
            .prepare(
                "SELECT source_component_key
                 FROM package_import_components
                 WHERE import_id = ?1 AND selected = 1
                 ORDER BY source_component_key",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let inspection: VersionedJson = decode_json("package inspection", &row.3)?;
    if inspection.schema_version != 1 {
        return Err(storage_corrupted(
            "package inspection wrapper schema is unsupported",
        ));
    }
    let review: PackageReview = serde_json::from_value(inspection.value.clone())
        .map_err(|error| storage_corrupted(format!("stored package review is invalid: {error}")))?;
    review
        .verify()
        .map_err(|error| storage_corrupted(format!("stored package review is invalid: {error}")))?;
    let source_hash = read_source_hash(connection, &row.14)?;
    if review.review_sha256.as_str() != row.4
        || review.source_sha256.as_str() != source_hash
        || review.manifest.package_id.as_str() != row.0
    {
        return Err(storage_corrupted(
            "stored package inspection differs from its durable identity",
        ));
    }
    let selection: Option<VersionedJson> = row
        .5
        .as_deref()
        .map(|json| decode_json("package selection", json))
        .transpose()?;
    if selection.is_some() != row.6.is_some() {
        return Err(storage_corrupted(
            "package selection JSON and hash presence differ",
        ));
    }
    if let Some(wrapper) = &selection {
        if wrapper.schema_version != 1 {
            return Err(storage_corrupted(
                "package selection wrapper schema is unsupported",
            ));
        }
        let plan: SelectiveImportPlan =
            serde_json::from_value(wrapper.value.clone()).map_err(|error| {
                storage_corrupted(format!("stored package selection is invalid: {error}"))
            })?;
        plan.verify().map_err(|error| {
            storage_corrupted(format!("stored package selection is invalid: {error}"))
        })?;
        let selected = plan
            .components
            .iter()
            .map(|component| component.component.id.as_str())
            .collect::<BTreeSet<_>>();
        let stored_selected = selected_component_ids
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        if plan.plan_sha256.as_str() != row.6.as_deref().unwrap_or_default()
            || plan.review_sha256.as_str() != row.4
            || plan.source_sha256.as_str() != source_hash
            || plan.package_id.as_str() != row.0
            || selected.len() != plan.components.len()
            || stored_selected.len() != selected_component_ids.len()
            || stored_selected != selected
        {
            return Err(storage_corrupted(
                "stored package selection differs from its durable identity",
            ));
        }
    } else if !selected_component_ids.is_empty() {
        return Err(storage_corrupted(
            "unselected package import contains selected component rows",
        ));
    }
    let failure_code = row
        .10
        .as_deref()
        .map(|json| {
            let value: Value = decode_json("package failure", json)?;
            value
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| storage_corrupted("package failure has no code"))
        })
        .transpose()?;
    let _completed_at = row
        .13
        .as_deref()
        .map(|value| parse_datetime("package import completed_at", value))
        .transpose()?;
    let capability_review = read_capability_review(connection, id)?;
    if package_capability_review_sha256(&capability_review)? != row.7 {
        return Err(storage_corrupted(
            "stored package capability review hash does not match",
        ));
    }
    Ok(StoredImportState {
        record: PackageImportRecord {
            id: id.to_owned(),
            package_id: PackageId::from(row.0),
            status: parse_import_status(&row.1)?,
            revision: u64_from_i64("package import revision", row.2)?,
            inspection,
            selection,
            selected_component_ids,
            failure_code,
            created_at: parse_datetime("package import created_at", &row.11)?,
            updated_at: parse_datetime("package import updated_at", &row.12)?,
        },
        package_source_id: row.14,
        inspection_sha256: row.4,
        selection_sha256: row.6,
        capability_review_sha256: row.7,
        approved_selection_sha256: row.8,
        approved_at: row
            .9
            .as_deref()
            .map(|value| parse_datetime("package import approved_at", value))
            .transpose()?,
    })
}

fn read_capability_review(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<PackageCapabilityReview> {
    let mut statement = connection
        .prepare(
            "SELECT capability, support_status, approved, reason
             FROM package_capability_requests
             WHERE import_id = ?1
             ORDER BY capability",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([import_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    Ok(PackageCapabilityReview {
        schema_version: 1,
        decisions: rows
            .into_iter()
            .map(|row| {
                Ok(PackageCapabilityDecision {
                    capability: parse_package_capability(&row.0)?,
                    support: parse_capability_support(&row.1)?,
                    approved: row.2,
                    reason: row.3,
                })
            })
            .collect::<CoreResult<Vec<_>>>()?,
    })
}

fn assert_expectation(
    current: &StoredImportState,
    expected: &PackageImportExpectation,
) -> CoreResult<()> {
    if current.record.revision != expected.revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.capability_review_sha256 != expected.capability_review_sha256
    {
        return Err(revision_conflict(
            "package import",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    Ok(())
}

fn assert_inspection_expectation(
    current: &StoredImportState,
    expected: &PackageInspectionExpectation,
) -> CoreResult<()> {
    if current.record.revision != expected.revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.capability_review_sha256 != expected.capability_review_sha256
    {
        return Err(revision_conflict(
            "package import",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn update_import_state(
    transaction: &Transaction<'_>,
    import_id: &str,
    old_revision: u64,
    status: PackageImportStatus,
    new_revision: u64,
    approved_selection_sha256: Option<&str>,
    approved_at: Option<DateTime<Utc>>,
    failure_json: Option<&str>,
    completed_at: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "UPDATE package_imports
             SET state = ?2, revision = ?3,
                 approved_selection_sha256 = ?4, approved_at = ?5,
                 failure_json = ?6, completed_at = ?7, updated_at = ?8
             WHERE id = ?1 AND revision = ?9",
            params![
                import_id,
                import_status_str(status),
                i64_from_u64("package import revision", new_revision)?,
                approved_selection_sha256,
                approved_at.map(|value| value.to_rfc3339()),
                failure_json,
                completed_at.map(|value| value.to_rfc3339()),
                updated_at.to_rfc3339(),
                i64_from_u64("package import revision", old_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "package import",
            import_id,
            Some(old_revision),
            None,
        ));
    }
    Ok(())
}

fn append_audit(
    transaction: &Transaction<'_>,
    import_id: &str,
    import_revision: u64,
    event_kind: &str,
    payload: &VersionedJson,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let payload_json = encode_json("package import audit payload", payload)?;
    let payload_sha256 = sha256_hex(payload_json.as_bytes());
    let sequence = transaction
        .query_row(
            "SELECT COALESCE(MAX(sequence), 0) + 1
             FROM package_import_audit_events
             WHERE import_id = ?1",
            [import_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO package_import_audit_events (
                import_id, sequence, import_revision, event_kind,
                payload_json, payload_sha256, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                import_id,
                sequence,
                i64_from_u64("package import revision", import_revision)?,
                event_kind,
                payload_json,
                payload_sha256,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn decode_selection(record: &PackageImportRecord) -> CoreResult<SelectiveImportPlan> {
    let wrapper = record
        .selection
        .as_ref()
        .ok_or_else(|| storage_corrupted("package selection is missing"))?;
    serde_json::from_value(wrapper.value.clone())
        .map_err(|error| storage_corrupted(format!("stored package selection is invalid: {error}")))
}

fn validate_selection_replay(
    connection: &Connection,
    current: &StoredImportState,
    expected: &PackageInspectionExpectation,
    selection: &SelectiveImportPlan,
    document_bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
    let next_revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("package selection replay revision overflow"))?;
    let selection_value = serde_json::to_value(selection).map_err(|error| {
        CoreError::invalid(format!("package selection cannot be encoded: {error}"))
    })?;
    let selected = selection
        .components
        .iter()
        .map(|component| component.component.id.clone())
        .collect::<BTreeSet<_>>();
    let stored_selected = current
        .record
        .selected_component_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if current.record.status != PackageImportStatus::AwaitingReview
        || current.record.revision != next_revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.capability_review_sha256 != expected.capability_review_sha256
        || current.selection_sha256.as_deref() != Some(selection.plan_sha256.as_str())
        || selection.review_sha256.as_str() != expected.inspection_sha256
        || selection.package_id != current.record.package_id
        || current
            .record
            .selection
            .as_ref()
            .is_none_or(|wrapper| wrapper.schema_version != 1 || wrapper.value != selection_value)
        || selected.len() != selection.components.len()
        || stored_selected.len() != current.record.selected_component_ids.len()
        || stored_selected != selected
        || read_source_hash(connection, &current.package_source_id)?
            != selection.source_sha256.as_str()
    {
        return Err(revision_conflict(
            "package selection replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    let review: PackageReview = serde_json::from_value(current.record.inspection.value.clone())
        .map_err(|error| storage_corrupted(format!("stored package review is invalid: {error}")))?;
    review
        .verify()
        .map_err(|error| storage_corrupted(format!("stored package review is invalid: {error}")))?;
    validate_selection_against_review(&review, selection)?;
    let stored_rows = load_reviewed_components(connection, &current.record.id)?;
    let stored_documents = load_document_target_reviews(connection, &current.record.id)?;
    validate_selection_target_review_replay(
        &stored_rows,
        &stored_documents,
        selection,
        document_bindings,
    )?;
    let target_review_sha256 = package_import_target_review_sha256(&stored_documents)?;
    let component_review_sha256 = reviewed_component_rows_sha256(&stored_rows)?;
    let audit = VersionedJson {
        schema_version: 1,
        value: json!({
            "inspection_sha256": current.inspection_sha256,
            "selection_sha256": selection.plan_sha256.as_str(),
            "capability_review_sha256": current.capability_review_sha256,
            "selected_component_ids": selection.components.iter()
                .map(|component| component.component.id.as_str())
                .collect::<Vec<_>>(),
            "component_review_sha256": component_review_sha256,
            "target_review_sha256": target_review_sha256,
        }),
    };
    validate_audit_replay(
        connection,
        &current.record.id,
        next_revision,
        "review_requested",
        &audit,
    )
}

fn validate_selection_target_review_replay(
    component_rows: &[ReviewedComponentRow],
    document_reviews: &[PackageDocumentTargetReview],
    selection: &SelectiveImportPlan,
    document_bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
    validate_binding_snapshot_shape(document_bindings)?;
    validate_document_target_reviews(document_reviews).map_err(|error| {
        storage_corrupted(format!(
            "stored package target review is invalid: {}",
            error.message
        ))
    })?;
    if document_reviews.len() != document_bindings.len() {
        return Err(storage_corrupted(
            "stored package target-review document count differs from its selection",
        ));
    }
    for (review, binding) in document_reviews.iter().zip(document_bindings) {
        if review.document_index != binding.document_index
            || review.source_component_id != binding.source_component_key
            || review.component_document_ordinal != binding.component_document_ordinal
            || review.source_component_sha256 != binding.source_component_sha256
            || review.target_object_id != binding.target_object_id
            || review.document_kind != binding.document_kind
            || review.document_sha256 != binding.document_sha256
            || review.expected_target_state_revision != binding.expected_object_revision
        {
            return Err(CoreError::invalid(
                "package selection retry differs from its immutable target review",
            ));
        }
    }
    let selected = selection
        .components
        .iter()
        .map(|component| component.component.id.as_str())
        .collect::<BTreeSet<_>>();
    let documents_by_component = document_reviews.iter().fold(
        BTreeMap::<&str, Vec<&PackageDocumentTargetReview>>::new(),
        |mut grouped, document| {
            grouped
                .entry(document.source_component_id.as_str())
                .or_default()
                .push(document);
            grouped
        },
    );
    for row in component_rows {
        if sha256_hex(row.review_json.as_bytes()) != row.review_sha256 {
            return Err(storage_corrupted(
                "stored package component review digest does not match",
            ));
        }
        let is_selected = selected.contains(row.source_component_key.as_str());
        if row.selected != is_selected {
            return Err(storage_corrupted(
                "stored package component selection flag differs from its plan",
            ));
        }
        let documents = documents_by_component
            .get(row.source_component_key.as_str())
            .cloned()
            .unwrap_or_default();
        if !documents.is_empty() {
            let updates = documents
                .iter()
                .filter(|document| document.disposition == PackageDocumentTargetDisposition::Update)
                .count();
            let expected_disposition = if updates == 0 {
                "create"
            } else if updates == documents.len() {
                "update"
            } else {
                "conflict"
            };
            let single_update = (documents.len() == 1 && updates == 1).then(|| documents[0]);
            if row.disposition != expected_disposition
                || row.target_object_id.as_deref()
                    != single_update.map(|document| document.target_object_id.as_str())
                || row.target_revision_id.as_deref()
                    != single_update
                        .and_then(|document| document.expected_target_revision_id.as_deref())
            {
                return Err(storage_corrupted(
                    "stored package component target summary differs from document reviews",
                ));
            }
        }
    }
    Ok(())
}

fn validate_approval_replay(
    connection: &Connection,
    current: &StoredImportState,
    expected: &PackageImportExpectation,
    approval: &PackageApprovalPayload,
    audit: &VersionedJson,
) -> CoreResult<()> {
    let next_revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("package approval replay revision overflow"))?;
    if current.record.status != PackageImportStatus::Approved
        || current.record.revision != next_revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.capability_review_sha256 != expected.capability_review_sha256
        || current.approved_selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.approved_at.is_none()
        || approval.plan.review_sha256.as_str() != expected.inspection_sha256
        || approval.plan.plan_sha256.as_str() != expected.selection_sha256
        || approval.plan.package_id != current.record.package_id
        || read_source_hash(connection, &current.package_source_id)?
            != approval.plan.source_sha256.as_str()
    {
        return Err(revision_conflict(
            "package approval replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    if read_approval_payload(connection, &current.record.id)? != *approval {
        return Err(CoreError::invalid(
            "package approval retry differs from the immutable approval snapshot",
        ));
    }
    validate_audit_replay(
        connection,
        &current.record.id,
        next_revision,
        "approved",
        audit,
    )
}

fn validate_inspected_discard_replay(
    connection: &Connection,
    current: &StoredImportState,
    expected: &PackageInspectionExpectation,
    audit: &VersionedJson,
) -> CoreResult<()> {
    let next_revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("package discard replay revision overflow"))?;
    if current.record.status != PackageImportStatus::Discarded
        || current.record.revision != next_revision
        || current.record.selection.is_some()
        || current.selection_sha256.is_some()
        || current.inspection_sha256 != expected.inspection_sha256
        || current.capability_review_sha256 != expected.capability_review_sha256
    {
        return Err(revision_conflict(
            "package inspection discard replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    validate_audit_replay(
        connection,
        &current.record.id,
        next_revision,
        "discarded",
        audit,
    )
}

fn validate_selected_discard_replay(
    connection: &Connection,
    current: &StoredImportState,
    expected: &PackageImportExpectation,
    audit: &VersionedJson,
) -> CoreResult<()> {
    let next_revision = expected
        .revision
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("package discard replay revision overflow"))?;
    if current.record.status != PackageImportStatus::Discarded
        || current.record.revision != next_revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.capability_review_sha256 != expected.capability_review_sha256
    {
        return Err(revision_conflict(
            "package discard replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    validate_audit_replay(
        connection,
        &current.record.id,
        next_revision,
        "discarded",
        audit,
    )
}

fn validate_audit_replay(
    connection: &Connection,
    import_id: &str,
    revision: u64,
    event_kind: &str,
    expected: &VersionedJson,
) -> CoreResult<()> {
    let row = connection
        .query_row(
            "SELECT event_kind, payload_json, payload_sha256
             FROM package_import_audit_events
             WHERE import_id = ?1 AND import_revision = ?2",
            params![import_id, i64_from_u64("package audit revision", revision)?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("package transition has no matching audit event"))?;
    if sha256_hex(row.1.as_bytes()) != row.2 {
        return Err(storage_corrupted(
            "package transition audit payload hash does not match",
        ));
    }
    let stored: VersionedJson = decode_json("package transition audit payload", &row.1)?;
    if row.0 != event_kind || stored != *expected {
        return Err(CoreError::invalid(
            "package transition retry differs from its immutable audit event",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Replay rejects drift across every immutable approval seam.
fn read_approval_payload(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<PackageApprovalPayload> {
    let payload = connection
        .query_row(
            "SELECT approval_payload_json
             FROM package_import_approvals
             WHERE import_id = ?1
             ORDER BY approved_at DESC, id
             LIMIT 1",
            [import_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("approved package import has no approval snapshot"))?;
    let wrapper: VersionedJson = decode_json("package approval", &payload)?;
    if wrapper.schema_version != 1 {
        return Err(storage_corrupted(
            "stored package approval wrapper schema is unsupported",
        ));
    }
    let approved: PackageApprovalPayload =
        serde_json::from_value(wrapper.value).map_err(|error| {
            storage_corrupted(format!("stored package approval is invalid: {error}"))
        })?;
    approved.plan.verify().map_err(|error| {
        storage_corrupted(format!("stored package approval is invalid: {error}"))
    })?;
    validate_binding_snapshot_shape(&approved.document_bindings).map_err(|error| {
        storage_corrupted(format!(
            "stored package document binding snapshot is invalid: {}",
            error.message
        ))
    })?;
    validate_sha256("package target review", &approved.target_review_sha256).map_err(|error| {
        storage_corrupted(format!(
            "stored package target-review digest is invalid: {}",
            error.message
        ))
    })?;
    let canonical_confirmations = canonical_update_target_confirmations(
        &approved.confirmed_update_targets,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package update confirmations are invalid: {}",
            error.message
        ))
    })?;
    if canonical_confirmations != approved.confirmed_update_targets {
        return Err(storage_corrupted(
            "stored package update confirmations are not canonical",
        ));
    }
    let confirmation_sha256 = package_update_target_confirmations_sha256(&canonical_confirmations)
        .map_err(|error| {
            storage_corrupted(format!(
                "stored package update confirmations cannot be hashed: {}",
                error.message
            ))
        })?;
    if approved.plan.target_review_sha256.as_str() != approved.target_review_sha256
        || approved.plan.update_target_confirmations_sha256.as_str() != confirmation_sha256
    {
        return Err(storage_corrupted(
            "stored package approval hash is detached from target review confirmations",
        ));
    }
    if !approved
        .approved_capabilities
        .windows(2)
        .all(|pair| pair[0] < pair[1])
    {
        return Err(storage_corrupted(
            "stored package capability approvals are not canonical",
        ));
    }
    validate_capability_approval_snapshot(
        connection,
        import_id,
        &approved.plan.required_capabilities,
        &approved.approved_capabilities,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package capability approval is invalid: {}",
            error.message
        ))
    })?;
    validate_normalization_evidence_shape(&approved.normalization_evidence).map_err(|error| {
        storage_corrupted(format!(
            "stored package normalization evidence is invalid: {}",
            error.message
        ))
    })?;
    let evidence_sha256 = package_normalization_evidence_sha256(&approved.normalization_evidence)
        .map_err(|error| {
        storage_corrupted(format!(
            "stored package normalization evidence cannot be hashed: {}",
            error.message
        ))
    })?;
    if evidence_sha256 != approved.normalization_evidence_sha256 {
        return Err(storage_corrupted(
            "stored package normalization evidence hash does not match",
        ));
    }
    let components = load_selected_commit_components(connection, import_id)?;
    let current = read_import_state(connection, import_id)?;
    let target_review = load_package_import_target_review(connection, &current)?;
    if approved.target_review_sha256 != target_review.target_review_sha256 {
        return Err(storage_corrupted(
            "stored package approval target-review digest does not match selection",
        ));
    }
    validate_target_review_binding_snapshot(
        &approved.document_bindings,
        &components,
        &target_review,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package approval target-review binding is invalid: {}",
            error.message
        ))
    })?;
    validate_exact_update_target_confirmations(
        &target_review.documents,
        &approved.confirmed_update_targets,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package update confirmations differ from target review: {}",
            error.message
        ))
    })?;
    validate_normalization_evidence_linkage(
        &approved.normalization_evidence,
        &approved.document_bindings,
        &components,
    )
    .map_err(|error| {
        storage_corrupted(format!(
            "stored package normalization evidence linkage is invalid: {}",
            error.message
        ))
    })?;
    Ok(approved)
}

fn validate_commit_input_shape(
    input: &PackageCommitInput,
    bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
    validate_source_record(&input.source)?;
    validate_identifier("package import", &input.import.id)?;
    validate_normalized_package_documents(&input.documents)?;
    if input.import.status != PackageImportStatus::Approved {
        return Err(CoreError::invalid(
            "package commit input must contain the approved import snapshot",
        ));
    }
    if input.import.failure_code.is_some() {
        return Err(CoreError::invalid(
            "approved package commit input cannot contain a failure",
        ));
    }
    if bindings.len() != input.documents.len() {
        return Err(CoreError::invalid(
            "every committed package document requires exactly one binding",
        ));
    }
    validate_binding_snapshot_shape(bindings)?;
    for binding in bindings {
        let index = usize::try_from(binding.document_index)
            .map_err(|_| CoreError::invalid("package document index is invalid"))?;
        if index >= input.documents.len() {
            return Err(CoreError::invalid(
                "package document binding index is out of bounds",
            ));
        }
        let document_json = encode_json("package commit document", &input.documents[index])?;
        if sha256_hex(document_json.as_bytes()) != binding.document_sha256 {
            return Err(CoreError::invalid(
                "package document hash does not match the commit binding",
            ));
        }
    }
    Ok(())
}

fn validate_normalized_package_documents(documents: &[PackageCommitDocument]) -> CoreResult<()> {
    let built_ins = crate::orchestration::built_in_prompt_presets();
    let canonical_policy = built_ins
        .first()
        .and_then(|preset| preset.blocks.first())
        .ok_or_else(|| CoreError::internal("canonical application policy is missing"))?;
    let built_in_preset_ids = built_ins
        .iter()
        .map(|preset| preset.id.as_str())
        .collect::<BTreeSet<_>>();
    for document in documents {
        match document {
            PackageCommitDocument::PromptPreset(preset) => {
                if built_in_preset_ids.contains(preset.id.as_str()) {
                    return Err(CoreError::invalid(
                        "imported packages cannot replace built-in prompt presets",
                    ));
                }
                if preset.blocks.first() != Some(canonical_policy) {
                    return Err(CoreError::invalid(
                        "imported prompt preset lacks the canonical application policy",
                    ));
                }
                let canonical_count = preset
                    .blocks
                    .iter()
                    .filter(|block| *block == canonical_policy)
                    .count();
                if canonical_count != 1 {
                    return Err(CoreError::invalid(
                        "canonical application policy must appear exactly once",
                    ));
                }
                for block in preset.blocks.iter().skip(1) {
                    if block.authority != InstructionAuthority::ImportedContent
                        || block.placement_zone == PlacementZone::ApplicationPolicy
                        || block.provenance.source_kind == SourceKind::ApplicationBuiltIn
                    {
                        return Err(CoreError::invalid(
                            "imported prompt preset retains elevated package block authority",
                        ));
                    }
                }
            }
            PackageCommitDocument::ContentModule(module) => {
                if module.prompt_fragments.iter().any(|block| {
                    block.authority == InstructionAuthority::Application
                        || block.placement_zone == PlacementZone::ApplicationPolicy
                        || block.provenance.source_kind == SourceKind::ApplicationBuiltIn
                }) {
                    return Err(CoreError::invalid(
                        "imported content module retains application-owned prompt blocks",
                    ));
                }
            }
            PackageCommitDocument::TransformSet(set) => {
                if set.enabled
                    || set
                        .rules
                        .iter()
                        .any(|rule| rule.enabled || rule.imported_enabled)
                {
                    return Err(CoreError::invalid(
                        "imported transform sets and rules must remain inactive",
                    ));
                }
            }
            PackageCommitDocument::InteractionRuleSet(set) => {
                if set.rules.iter().any(|rule| rule.enabled) {
                    return Err(CoreError::invalid(
                        "imported interaction rules must remain inactive",
                    ));
                }
            }
            PackageCommitDocument::KnowledgeBook(book) => {
                book.validate().map_err(|error| {
                    CoreError::invalid(format!("invalid imported knowledge book: {error}"))
                })?;
            }
            PackageCommitDocument::MemoryProfile(profile) => {
                profile.validate().map_err(|error| {
                    CoreError::invalid(format!("invalid imported memory profile: {error}"))
                })?;
            }
            PackageCommitDocument::CharacterContent { .. } => {}
        }
    }
    Ok(())
}

fn validate_binding_snapshot_shape(bindings: &[PackageDocumentCommitBinding]) -> CoreResult<()> {
    let mut indices = BTreeSet::new();
    let mut component_documents = BTreeSet::new();
    let mut targets = BTreeSet::new();
    let mut ordinals_by_component = BTreeMap::<&str, Vec<u32>>::new();
    for (expected_index, binding) in bindings.iter().enumerate() {
        validate_identifier("package component", &binding.source_component_key)?;
        validate_identifier("package target object", &binding.target_object_id)?;
        if !matches!(
            binding.document_kind.as_str(),
            "prompt_preset"
                | "knowledge_book"
                | "memory_profile"
                | "transform_set"
                | "interaction_rule_set"
                | "content_module"
                | "character_content"
        ) {
            return Err(CoreError::invalid(
                "package document binding kind is invalid",
            ));
        }
        validate_sha256("package component", &binding.source_component_sha256)?;
        validate_sha256("package document", &binding.document_sha256)?;
        let index = usize::try_from(binding.document_index)
            .map_err(|_| CoreError::invalid("package document index is invalid"))?;
        if index != expected_index {
            return Err(CoreError::invalid(
                "package document bindings must be ordered by contiguous document index",
            ));
        }
        if !indices.insert(index) {
            return Err(CoreError::invalid(
                "package document bindings contain a duplicate index",
            ));
        }
        if !component_documents.insert((
            binding.source_component_key.as_str(),
            binding.component_document_ordinal,
        )) {
            return Err(CoreError::invalid(
                "package component document bindings contain a duplicate ordinal",
            ));
        }
        if !targets.insert(binding.target_object_id.as_str()) {
            return Err(CoreError::invalid(
                "package document bindings contain a duplicate target object",
            ));
        }
        ordinals_by_component
            .entry(binding.source_component_key.as_str())
            .or_default()
            .push(binding.component_document_ordinal);
    }
    if indices
        .iter()
        .copied()
        .enumerate()
        .any(|(expected, actual)| expected != actual)
    {
        return Err(CoreError::invalid(
            "package document indices must be contiguous from zero",
        ));
    }
    for ordinals in ordinals_by_component.values_mut() {
        ordinals.sort_unstable();
        if ordinals
            .iter()
            .enumerate()
            .any(|(expected, actual)| usize::try_from(*actual) != Ok(expected))
        {
            return Err(CoreError::invalid(
                "package component document ordinals must be contiguous from zero",
            ));
        }
    }
    Ok(())
}

fn validate_normalization_evidence_shape(
    evidence: &[PackageNormalizationEvidence],
) -> CoreResult<()> {
    if evidence.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(CoreError::invalid(
            "package normalization evidence must be unique and canonically ordered",
        ));
    }
    let mut keys = BTreeSet::new();
    for entry in evidence {
        validate_identifier("package normalization component", &entry.component_id)?;
        validate_identifier("package normalization object", &entry.object_id)?;
        if !keys.insert((
            entry.component_id.as_str(),
            entry.object_id.as_str(),
            entry.field.as_str(),
        )) || !matches!(entry.field.as_str(), "enabled" | "imported_enabled")
            || entry.after
            || entry.reason.trim().is_empty()
            || entry.reason.len() > MAX_NORMALIZATION_REASON_BYTES
            || entry.reason.chars().any(char::is_control)
        {
            return Err(CoreError::invalid(
                "package normalization evidence is invalid",
            ));
        }
    }
    Ok(())
}

fn validate_normalization_evidence_linkage(
    evidence: &[PackageNormalizationEvidence],
    bindings: &[PackageDocumentCommitBinding],
    components: &BTreeMap<String, ReviewedComponentRow>,
) -> CoreResult<()> {
    validate_normalization_evidence_shape(evidence)?;
    let bound_components = bindings
        .iter()
        .map(|binding| binding.source_component_key.as_str())
        .collect::<BTreeSet<_>>();
    for entry in evidence {
        if !bound_components.contains(entry.component_id.as_str()) {
            return Err(CoreError::invalid(
                "package normalization evidence names an unbound component",
            ));
        }
        let component = components.get(&entry.component_id).ok_or_else(|| {
            CoreError::invalid("package normalization evidence names an unknown component")
        })?;
        if !matches!(
            component.component_kind.as_str(),
            "transform_set" | "interaction_rule_set"
        ) || (entry.field == "imported_enabled" && component.component_kind != "transform_set")
        {
            return Err(CoreError::invalid(
                "package normalization evidence does not match declarative component type",
            ));
        }
    }
    for binding in bindings
        .iter()
        .filter(|binding| binding.document_kind == "transform_set")
    {
        if !evidence.iter().any(|entry| {
            entry.component_id == binding.source_component_key
                && entry.object_id == binding.target_object_id
                && entry.field == "enabled"
                && !entry.after
        }) {
            return Err(CoreError::invalid(
                "imported transform set lacks immutable disabled-state evidence",
            ));
        }
    }
    Ok(())
}

fn validate_document_normalization_evidence(
    documents: &[PackageCommitDocument],
    bindings: &[PackageDocumentCommitBinding],
    evidence: &[PackageNormalizationEvidence],
) -> CoreResult<()> {
    let mut expected = BTreeMap::<(String, String, String), Option<bool>>::new();
    let mut expected_entries = 0_usize;
    for (document, binding) in documents.iter().zip(bindings) {
        let component_id = binding.source_component_key.clone();
        match document {
            PackageCommitDocument::TransformSet(set) => {
                expected.insert(
                    (
                        component_id.clone(),
                        set.id.as_str().to_owned(),
                        "enabled".to_owned(),
                    ),
                    Some(set.imported_author_enabled),
                );
                expected_entries = expected_entries.saturating_add(1);
                for rule in &set.rules {
                    expected.insert(
                        (
                            component_id.clone(),
                            rule.id.as_str().to_owned(),
                            "enabled".to_owned(),
                        ),
                        Some(rule.imported_author_enabled),
                    );
                    expected_entries = expected_entries.saturating_add(1);
                    expected.insert(
                        (
                            component_id.clone(),
                            rule.id.as_str().to_owned(),
                            "imported_enabled".to_owned(),
                        ),
                        None,
                    );
                    expected_entries = expected_entries.saturating_add(1);
                }
            }
            PackageCommitDocument::InteractionRuleSet(set) => {
                for rule in &set.rules {
                    expected.insert(
                        (
                            component_id.clone(),
                            rule.id.as_str().to_owned(),
                            "enabled".to_owned(),
                        ),
                        Some(rule.imported_author_enabled),
                    );
                    expected_entries = expected_entries.saturating_add(1);
                }
            }
            PackageCommitDocument::PromptPreset(_)
            | PackageCommitDocument::KnowledgeBook(_)
            | PackageCommitDocument::MemoryProfile(_)
            | PackageCommitDocument::ContentModule(_)
            | PackageCommitDocument::CharacterContent { .. } => {}
        }
    }
    let actual = evidence
        .iter()
        .map(|entry| {
            (
                (
                    entry.component_id.clone(),
                    entry.object_id.clone(),
                    entry.field.clone(),
                ),
                entry,
            )
        })
        .collect::<BTreeMap<_, _>>();
    if expected.len() != expected_entries
        || actual.len() != evidence.len()
        || actual.keys().ne(expected.keys())
        || expected.iter().any(|(key, expected_before)| {
            expected_before.is_some_and(|value| actual[key].before != value)
        })
    {
        return Err(CoreError::invalid(
            "package normalization evidence differs from normalized document author intent",
        ));
    }
    Ok(())
}

fn load_selected_commit_components(
    connection: &Connection,
    import_id: &str,
) -> CoreResult<BTreeMap<String, ReviewedComponentRow>> {
    let mut statement = connection
        .prepare(
            "SELECT ordinal, source_component_key, component_kind,
                    disposition, selected, target_object_id,
                    target_revision_id, review_json, review_sha256
             FROM package_import_components
             WHERE import_id = ?1 AND selected = 1
             ORDER BY ordinal",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([import_id], |row| {
            Ok(ReviewedComponentRow {
                ordinal: row.get::<_, u32>(0)?,
                source_component_key: row.get(1)?,
                component_kind: row.get(2)?,
                disposition: row.get(3)?,
                selected: row.get(4)?,
                target_object_id: row.get(5)?,
                target_revision_id: row.get(6)?,
                review_json: row.get(7)?,
                review_sha256: row.get(8)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(|row| Ok((row.source_component_key.clone(), row)))
        .collect()
}

fn validate_approval_bindings(
    connection: &Connection,
    bindings: &[PackageDocumentCommitBinding],
    components: &BTreeMap<String, ReviewedComponentRow>,
    target_review: &PackageImportTargetReview,
    confirmed_update_targets: &[PackageUpdateTargetConfirmation],
) -> CoreResult<()> {
    validate_target_review_binding_snapshot(bindings, components, target_review)?;
    validate_exact_update_target_confirmations(&target_review.documents, confirmed_update_targets)?;
    validate_current_target_review_state(connection, &target_review.documents)
}

#[allow(clippy::too_many_lines)] // Parent summaries and child rows are verified together.
fn validate_target_review_binding_snapshot(
    bindings: &[PackageDocumentCommitBinding],
    components: &BTreeMap<String, ReviewedComponentRow>,
    target_review: &PackageImportTargetReview,
) -> CoreResult<()> {
    validate_binding_snapshot_shape(bindings)?;
    target_review.verify()?;
    if bindings.len() != target_review.documents.len() {
        return Err(CoreError::invalid(
            "package bindings differ from the immutable target-review document count",
        ));
    }
    let documents_by_component = target_review.documents.iter().fold(
        BTreeMap::<&str, Vec<&PackageDocumentTargetReview>>::new(),
        |mut grouped, document| {
            grouped
                .entry(document.source_component_id.as_str())
                .or_default()
                .push(document);
            grouped
        },
    );
    for row in components.values() {
        if sha256_hex(row.review_json.as_bytes()) != row.review_sha256 {
            return Err(storage_corrupted(
                "package component review hash does not match",
            ));
        }
        let descriptor: lorepia_orchestration::PackageComponentDescriptor =
            decode_json("package component review", &row.review_json)?;
        if !row.selected
            || descriptor.id != row.source_component_key
            || component_kind_str(descriptor.kind) != row.component_kind
        {
            return Err(storage_corrupted(
                "package selected component review identity is invalid",
            ));
        }
        let documents = documents_by_component
            .get(row.source_component_key.as_str())
            .cloned()
            .unwrap_or_default();
        if matches!(row.component_kind.as_str(), "asset" | "raw_extension") {
            if !documents.is_empty() {
                return Err(storage_corrupted(
                    "package non-document component has target-review rows",
                ));
            }
            continue;
        }
        if documents.is_empty() {
            return Err(CoreError::invalid(
                "every selected document component must review at least one target document",
            ));
        }
        let update_count = documents
            .iter()
            .filter(|document| document.disposition == PackageDocumentTargetDisposition::Update)
            .count();
        let expected_disposition = if update_count == 0 {
            "create"
        } else if update_count == documents.len() {
            "update"
        } else {
            "conflict"
        };
        let exact_single_update = (documents.len() == 1 && update_count == 1).then(|| documents[0]);
        if row.disposition != expected_disposition
            || row.target_object_id.as_deref()
                != exact_single_update.map(|document| document.target_object_id.as_str())
            || row.target_revision_id.as_deref()
                != exact_single_update
                    .and_then(|document| document.expected_target_revision_id.as_deref())
            || documents.iter().any(|document| {
                document.document_kind != row.component_kind
                    || document.source_component_sha256 != descriptor.sha256.as_str()
            })
        {
            return Err(storage_corrupted(
                "package component target summary differs from immutable document reviews",
            ));
        }
    }
    for (review, binding) in target_review.documents.iter().zip(bindings) {
        let row = components
            .get(&binding.source_component_key)
            .ok_or_else(|| CoreError::invalid("package binding names an unselected component"))?;
        if matches!(row.component_kind.as_str(), "asset" | "raw_extension")
            || !matches!(row.disposition.as_str(), "create" | "update" | "conflict")
            || review.source_component_id != binding.source_component_key
            || review.component_document_ordinal != binding.component_document_ordinal
            || review.document_index != binding.document_index
            || review.document_kind != binding.document_kind
            || review.target_object_id != binding.target_object_id
            || review.source_component_sha256 != binding.source_component_sha256
            || review.document_sha256 != binding.document_sha256
            || review.expected_target_state_revision != binding.expected_object_revision
        {
            return Err(CoreError::invalid(
                "package document binding differs from its immutable target review",
            ));
        }
    }
    if documents_by_component
        .keys()
        .any(|component_id| !components.contains_key(*component_id))
    {
        return Err(storage_corrupted(
            "package target review names an unselected component",
        ));
    }
    Ok(())
}

fn validate_current_target_review_state(
    connection: &Connection,
    documents: &[PackageDocumentTargetReview],
) -> CoreResult<()> {
    for review in documents {
        let target = connection
            .query_row(
                "SELECT object.object_kind, object.deleted_at,
                        state.state_version, state.active_revision_id
                 FROM content_objects AS object
                 JOIN content_object_state AS state
                   ON state.object_id = object.id
                 WHERE object.id = ?1",
                [review.target_object_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?;
        match (review.disposition, target) {
            (PackageDocumentTargetDisposition::Create, None) => {}
            (PackageDocumentTargetDisposition::Create, Some(_)) => {
                return Err(CoreError::invalid(
                    "new package target appeared after its explicit review",
                ));
            }
            (PackageDocumentTargetDisposition::Update, None) => {
                return Err(CoreError::invalid(
                    "package update target disappeared after its explicit review",
                ));
            }
            (
                PackageDocumentTargetDisposition::Update,
                Some((kind, deleted_at, actual_revision, active_revision_id)),
            ) => {
                if kind != review.document_kind
                    || deleted_at.is_some()
                    || Some(u64_from_i64("content state revision", actual_revision)?)
                        != review.expected_target_state_revision
                    || review.expected_target_revision_id.as_deref()
                        != Some(active_revision_id.as_str())
                {
                    return Err(CoreError::invalid(
                        "package update target changed after its explicit review",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn canonical_update_target_confirmations(
    confirmations: &[PackageUpdateTargetConfirmation],
) -> CoreResult<Vec<PackageUpdateTargetConfirmation>> {
    let mut canonical = confirmations.to_vec();
    let mut identities = BTreeSet::new();
    for confirmation in &canonical {
        validate_identifier(
            "package update confirmation component",
            &confirmation.source_component_id,
        )?;
        validate_identifier(
            "package update confirmation object",
            &confirmation.target_object_id,
        )?;
        validate_identifier(
            "package update confirmation revision",
            &confirmation.expected_target_revision_id,
        )?;
        if confirmation.expected_target_state_revision == 0 {
            return Err(CoreError::invalid(
                "package update confirmation state revision must be positive",
            ));
        }
        if !identities.insert((
            confirmation.source_component_id.as_str(),
            confirmation.component_document_ordinal,
            confirmation.target_object_id.as_str(),
        )) {
            return Err(CoreError::invalid(
                "package update confirmations contain a duplicate target",
            ));
        }
    }
    canonical.sort();
    Ok(canonical)
}

fn validate_exact_update_target_confirmations(
    documents: &[PackageDocumentTargetReview],
    confirmations: &[PackageUpdateTargetConfirmation],
) -> CoreResult<()> {
    let actual = canonical_update_target_confirmations(confirmations)?;
    let mut expected = documents
        .iter()
        .filter(|document| document.disposition == PackageDocumentTargetDisposition::Update)
        .map(|document| {
            Ok(PackageUpdateTargetConfirmation {
                source_component_id: document.source_component_id.clone(),
                component_document_ordinal: document.component_document_ordinal,
                target_object_id: document.target_object_id.clone(),
                expected_target_revision_id: document
                    .expected_target_revision_id
                    .clone()
                    .ok_or_else(|| {
                        storage_corrupted("reviewed update target has no immutable revision")
                    })?,
                expected_target_state_revision: document
                    .expected_target_state_revision
                    .ok_or_else(|| {
                        storage_corrupted("reviewed update target has no state revision")
                    })?,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    expected.sort();
    if actual != expected {
        return Err(CoreError::invalid(
            "package approval must explicitly confirm every and only reviewed update target",
        ));
    }
    Ok(())
}

fn validate_completed_authority_audit(
    connection: &Connection,
    current: &StoredImportState,
    approval: &PackageApprovalPayload,
    committed: &CompletedAuthorityCommitEvidence,
) -> CoreResult<()> {
    let row = connection
        .query_row(
            "SELECT event_kind, payload_json, payload_sha256
             FROM package_import_audit_events
             WHERE import_id = ?1 AND import_revision = ?2",
            params![
                current.record.id,
                i64_from_u64("package audit revision", current.record.revision)?,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("completed package import has no completion audit"))?;
    if row.0 != "commit_completed" || sha256_hex(row.1.as_bytes()) != row.2 {
        return Err(storage_corrupted(
            "completed package authority audit kind or hash is invalid",
        ));
    }
    let wrapper: VersionedJson = decode_json("package completion audit", &row.1)?;
    if wrapper.schema_version != 1
        || wrapper.value.get("approval_sha256").and_then(Value::as_str)
            != Some(approval.plan.approval_sha256.as_str())
    {
        return Err(storage_corrupted(
            "completed package authority audit differs from approval",
        ));
    }
    let asset_ids = wrapper
        .value
        .get("asset_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| storage_corrupted("package completion audit has no asset inventory"))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| storage_corrupted("package completion asset id is invalid"))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let expected_asset_ids = approval
        .plan
        .assets
        .iter()
        .map(|asset| asset.id.as_str().to_owned())
        .collect::<Vec<_>>();
    if asset_ids != expected_asset_ids {
        return Err(storage_corrupted(
            "package completion audit asset inventory differs from approval",
        ));
    }
    let mut audited = BTreeMap::new();
    for value in wrapper
        .value
        .get("components")
        .and_then(Value::as_array)
        .ok_or_else(|| storage_corrupted("package completion audit has no component evidence"))?
    {
        let component_id = value
            .get("source_component_key")
            .and_then(Value::as_str)
            .ok_or_else(|| storage_corrupted("package completion component id is invalid"))?;
        let document_ordinal = value
            .get("component_document_ordinal")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| storage_corrupted("package completion document ordinal is invalid"))?;
        if audited
            .insert((component_id.to_owned(), document_ordinal), value)
            .is_some()
        {
            return Err(storage_corrupted(
                "package completion audit contains duplicate component evidence",
            ));
        }
    }
    if audited.len() != committed.len()
        || committed
            .iter()
            .any(|(key, (_, result))| audited.get(key).is_none_or(|audited| *audited != result))
    {
        return Err(storage_corrupted(
            "package completion audit differs from durable commit evidence",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)] // Exact replay validates every immutable result row and hash.
fn validate_completed_commit_replay(
    connection: &Connection,
    current: &StoredImportState,
    input: &PackageCommitInput,
    expected: &PackageImportExpectation,
    bindings: &[PackageDocumentCommitBinding],
) -> CoreResult<()> {
    let completed_revision = expected
        .revision
        .checked_add(2)
        .ok_or_else(|| CoreError::invalid("package replay revision overflow"))?;
    if current.record.revision != completed_revision
        || current.inspection_sha256 != expected.inspection_sha256
        || current.selection_sha256.as_deref() != Some(&expected.selection_sha256)
        || current.capability_review_sha256 != expected.capability_review_sha256
        || input.import.status != PackageImportStatus::Approved
        || input.import.revision != expected.revision
        || input.import.id != current.record.id
        || input.import.package_id != current.record.package_id
        || input.import.inspection != current.record.inspection
        || input.import.selection != current.record.selection
        || input.import.selected_component_ids != current.record.selected_component_ids
        || input.import.created_at != current.record.created_at
        || current.approved_at != Some(input.import.updated_at)
    {
        return Err(revision_conflict(
            "completed package import replay",
            &current.record.id,
            Some(expected.revision),
            Some(current.record.revision),
        ));
    }
    let stored_source = read_package_source_by_id(connection, &current.package_source_id)?;
    if stored_source != input.source {
        return Err(CoreError::invalid(
            "completed package replay source differs from the committed source",
        ));
    }
    let approval = read_approval_payload(connection, &input.import.id)?;
    if approval.document_bindings != bindings
        || approval.plan.review_sha256.as_str() != expected.inspection_sha256
        || approval.plan.plan_sha256.as_str() != expected.selection_sha256
        || approval.plan.source_sha256.as_str() != input.source.source_sha256
        || approval.plan.package_id != input.source.package_id
        || approval.plan.assets != input.assets
    {
        return Err(CoreError::invalid(
            "completed package replay differs from the approved snapshot",
        ));
    }
    validate_document_normalization_evidence(
        &input.documents,
        bindings,
        &approval.normalization_evidence,
    )?;
    let mut statement = connection
        .prepare(
            "SELECT component.source_component_key,
                    committed_document.document_ordinal,
                    committed_document.target_object_id,
                    committed_document.target_revision_id,
                    committed_document.result_json,
                    committed_document.result_sha256
             FROM package_import_component_commits AS committed_document
             JOIN package_import_components AS component
               ON component.import_id = committed_document.import_id
              AND component.ordinal = committed_document.component_ordinal
             WHERE committed_document.import_id = ?1
             ORDER BY component.source_component_key,
                      committed_document.document_ordinal",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([input.import.id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, u32>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if rows.len() != bindings.len() {
        return Err(storage_corrupted(
            "completed package commit evidence count is incomplete",
        ));
    }
    let evidence = rows
        .into_iter()
        .map(|row| {
            if sha256_hex(row.4.as_bytes()) != row.5 {
                return Err(storage_corrupted(
                    "completed package commit result hash does not match",
                ));
            }
            let result: VersionedJson = decode_json("package component commit result", &row.4)?;
            Ok(((row.0, row.1), (row.2, row.3, result)))
        })
        .collect::<CoreResult<BTreeMap<_, _>>>()?;
    for binding in bindings {
        let (target_object_id, target_revision_id, result) = evidence
            .get(&(
                binding.source_component_key.clone(),
                binding.component_document_ordinal,
            ))
            .ok_or_else(|| storage_corrupted("completed package commit evidence is missing"))?;
        let document = input
            .documents
            .get(binding.document_index as usize)
            .ok_or_else(|| CoreError::invalid("package replay document index is invalid"))?;
        if target_object_id != &binding.target_object_id
            || target_object_id != &document_object_id(document)
            || result
                .value
                .get("target_revision_id")
                .and_then(Value::as_str)
                != Some(target_revision_id.as_str())
            || result
                .value
                .get("source_component_sha256")
                .and_then(Value::as_str)
                != Some(binding.source_component_sha256.as_str())
            || result.value.get("document_sha256").and_then(Value::as_str)
                != Some(binding.document_sha256.as_str())
        {
            return Err(storage_corrupted(
                "completed package commit evidence differs from replay input",
            ));
        }
    }
    Ok(())
}

fn validate_commit_bindings(
    connection: &Connection,
    documents: &[PackageCommitDocument],
    bindings: &[PackageDocumentCommitBinding],
    components: &BTreeMap<String, ReviewedComponentRow>,
    target_review: &PackageImportTargetReview,
    confirmed_update_targets: &[PackageUpdateTargetConfirmation],
) -> CoreResult<()> {
    validate_approval_bindings(
        connection,
        bindings,
        components,
        target_review,
        confirmed_update_targets,
    )?;
    for binding in bindings {
        let row = components
            .get(&binding.source_component_key)
            .ok_or_else(|| CoreError::invalid("package binding names an unselected component"))?;
        if !matches!(row.disposition.as_str(), "create" | "update" | "conflict") {
            return Err(CoreError::invalid(
                "package binding names a component that cannot be committed",
            ));
        }
        if sha256_hex(row.review_json.as_bytes()) != row.review_sha256 {
            return Err(storage_corrupted(
                "package component review hash does not match",
            ));
        }
        let descriptor: lorepia_orchestration::PackageComponentDescriptor =
            decode_json("package component review", &row.review_json)?;
        if descriptor.sha256.as_str() != binding.source_component_sha256 {
            return Err(CoreError::invalid(
                "package binding source hash differs from the approved component",
            ));
        }
        let index = binding.document_index as usize;
        let document = documents
            .get(index)
            .ok_or_else(|| CoreError::invalid("package document index is out of bounds"))?;
        if row.component_kind != document_kind(document)
            || binding.document_kind != document_kind(document)
            || descriptor.id != binding.source_component_key
            || binding.target_object_id != document_object_id(document)
        {
            return Err(CoreError::invalid(
                "package document kind does not match its approved component",
            ));
        }
    }
    Ok(())
}

fn expectation_payload(expected: &PackageImportExpectation) -> VersionedJson {
    VersionedJson {
        schema_version: 1,
        value: json!({
            "revision": expected.revision,
            "inspection_sha256": expected.inspection_sha256,
            "selection_sha256": expected.selection_sha256,
            "capability_review_sha256": expected.capability_review_sha256,
        }),
    }
}

fn validate_expectation(expected: &PackageImportExpectation) -> CoreResult<()> {
    if expected.revision == 0 {
        return Err(CoreError::invalid(
            "package import expected revision must be positive",
        ));
    }
    validate_sha256("package inspection", &expected.inspection_sha256)?;
    validate_sha256("package selection", &expected.selection_sha256)?;
    validate_sha256(
        "package capability review",
        &expected.capability_review_sha256,
    )
}

fn validate_inspection_expectation(expected: &PackageInspectionExpectation) -> CoreResult<()> {
    if expected.revision == 0 {
        return Err(CoreError::invalid(
            "package inspection expected revision must be positive",
        ));
    }
    validate_sha256("package inspection", &expected.inspection_sha256)?;
    validate_sha256(
        "package capability review",
        &expected.capability_review_sha256,
    )
}

fn document_object_id(document: &PackageCommitDocument) -> String {
    match document {
        PackageCommitDocument::PromptPreset(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::KnowledgeBook(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::MemoryProfile(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::TransformSet(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::InteractionRuleSet(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::ContentModule(value) => value.id.as_str().to_owned(),
        PackageCommitDocument::CharacterContent { character_id, .. } => {
            format!("character-content:{character_id}")
        }
    }
}

fn document_kind(document: &PackageCommitDocument) -> &'static str {
    match document {
        PackageCommitDocument::PromptPreset(_) => "prompt_preset",
        PackageCommitDocument::KnowledgeBook(_) => "knowledge_book",
        PackageCommitDocument::MemoryProfile(_) => "memory_profile",
        PackageCommitDocument::TransformSet(_) => "transform_set",
        PackageCommitDocument::InteractionRuleSet(_) => "interaction_rule_set",
        PackageCommitDocument::ContentModule(_) => "content_module",
        PackageCommitDocument::CharacterContent { .. } => "character_content",
    }
}

fn validate_completed_module_authority_target(
    stored: &ActiveContentModuleRevision,
) -> CoreResult<()> {
    validate_identifier("content module", stored.object.value.id.as_str())?;
    validate_identifier(
        "content module revision",
        stored.module_revision.id.as_str(),
    )?;
    let module_document_json =
        encode_json("imported module authority document", &stored.object.value)?;
    let provenance = &stored.object.value.metadata.provenance;
    if sha256_hex(module_document_json.as_bytes()) != stored.object.sha256
        || stored.object.revision_id != stored.module_revision.id.as_str()
        || stored.object.object_id != stored.object.value.id.as_str()
        || stored.object.value.id != stored.module_revision.module_id
        || provenance.source_kind != SourceKind::ImportedPackage
        || provenance.source_hash.as_deref() != Some(stored.module_revision.source_hash.as_str())
        || provenance.source_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(storage_corrupted(
            "imported module authority target differs from its immutable revision",
        ));
    }
    Ok(())
}

enum ModuleAuthorityComponent {
    Embedded,
    Linked {
        target_object_id: String,
        target_revision_id: String,
    },
    Asset(AssetDescriptor),
}

fn read_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
) -> CoreResult<ModuleAuthorityComponent> {
    let revision_id = stored.module_revision.id.as_str();
    match &component.component {
        ModuleComponentRef::PromptBlock { id } => {
            read_prompt_block_module_authority_component(connection, stored, component, id)
        }
        ModuleComponentRef::Control { id } => {
            read_control_module_authority_component(connection, stored, component, id)
        }
        ModuleComponentRef::KnowledgeBook { id } => {
            if !stored.object.value.knowledge_book_ids.contains(id) {
                return Err(storage_corrupted(
                    "module knowledge projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "knowledge_book",
                "knowledge_book_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::TransformSet { id } => {
            if !stored.object.value.transform_set_ids.contains(id) {
                return Err(storage_corrupted(
                    "module transform projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "transform_set",
                "transform_set_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::InteractionRuleSet { id } => {
            if !stored.object.value.interaction_rule_set_ids.contains(id) {
                return Err(storage_corrupted(
                    "module interaction projection is absent from its immutable document",
                ));
            }
            read_linked_module_authority_component(
                connection,
                revision_id,
                "interaction_rule_set",
                "interaction_rule_set_revision_id",
                id.as_str(),
                component,
            )
        }
        ModuleComponentRef::Asset { id } => {
            read_asset_module_authority_component(connection, stored, component, id)
        }
    }
}

fn read_prompt_block_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &PromptBlockId,
) -> CoreResult<ModuleAuthorityComponent> {
    let expected = stored
        .object
        .value
        .prompt_fragments
        .iter()
        .find(|block| block.id == *id)
        .ok_or_else(|| {
            storage_corrupted(
                "module prompt-block projection is absent from its immutable document",
            )
        })?;
    let expected_json = encode_json("module prompt block authority", expected)?;
    let row = connection
        .query_row(
            "SELECT component.component_sha256, block.document_json
             FROM content_module_components AS component
             JOIN content_module_prompt_blocks AS block
               ON block.module_revision_id = component.module_revision_id
              AND block.block_id = component.prompt_block_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'prompt_block'
               AND component.prompt_block_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module prompt-block authority"))?;
    validate_embedded_module_authority(component, &expected_json, &row)?;
    Ok(ModuleAuthorityComponent::Embedded)
}

fn read_control_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &ControlId,
) -> CoreResult<ModuleAuthorityComponent> {
    let expected = stored
        .object
        .value
        .control_specs
        .iter()
        .find(|control| control.id == *id)
        .ok_or_else(|| {
            storage_corrupted("module control projection is absent from its immutable document")
        })?;
    let expected_json = encode_json("module control authority", expected)?;
    let row = connection
        .query_row(
            "SELECT component.component_sha256, control.document_json
             FROM content_module_components AS component
             JOIN content_module_controls AS control
               ON control.module_revision_id = component.module_revision_id
              AND control.control_id = component.control_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'control'
               AND component.control_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module control authority"))?;
    validate_embedded_module_authority(component, &expected_json, &row)?;
    Ok(ModuleAuthorityComponent::Embedded)
}

fn read_asset_module_authority_component(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    component: &lorepia_domain::ComponentHash,
    id: &AssetId,
) -> CoreResult<ModuleAuthorityComponent> {
    if !stored.object.value.asset_ids.contains(id) {
        return Err(storage_corrupted(
            "module asset projection is absent from its immutable document",
        ));
    }
    let row = connection
        .query_row(
            "SELECT component.component_sha256, descriptor.payload_json
             FROM content_module_components AS component
             JOIN asset_descriptors AS descriptor
               ON descriptor.id = component.asset_descriptor_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'asset'
               AND component.asset_descriptor_id = ?2",
            params![stored.module_revision.id.as_str(), id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("module asset authority"))?;
    if row.0 != component.sha256.as_str() || sha256_hex(row.1.as_bytes()) != row.0 {
        return Err(storage_corrupted(
            "module asset authority hash differs from its immutable projection",
        ));
    }
    let descriptor: AssetDescriptor = decode_json("module asset authority descriptor", &row.1)?;
    if descriptor.id != *id {
        return Err(storage_corrupted(
            "module asset authority descriptor has a different identity",
        ));
    }
    Ok(ModuleAuthorityComponent::Asset(descriptor))
}

fn validate_embedded_module_authority(
    component: &lorepia_domain::ComponentHash,
    expected_json: &str,
    stored: &(String, String),
) -> CoreResult<()> {
    if stored.0 != component.sha256.as_str()
        || sha256_hex(stored.1.as_bytes()) != stored.0
        || stored.1 != expected_json
    {
        return Err(storage_corrupted(
            "embedded module authority differs from its immutable document",
        ));
    }
    Ok(())
}

fn read_linked_module_authority_component(
    connection: &Connection,
    module_revision_id: &str,
    object_kind: &'static str,
    revision_column: &'static str,
    object_id: &str,
    component: &lorepia_domain::ComponentHash,
) -> CoreResult<ModuleAuthorityComponent> {
    let query = match revision_column {
        "knowledge_book_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.knowledge_book_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'knowledge_book'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        "transform_set_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.transform_set_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'transform_set'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        "interaction_rule_set_revision_id" => {
            "SELECT component.component_sha256, content.object_id, content.id,
                    content.document_json, content.document_sha256
             FROM content_module_components AS component
             JOIN content_revisions AS content
               ON content.id = component.interaction_rule_set_revision_id
             WHERE component.module_revision_id = ?1
               AND component.component_kind = 'interaction_rule_set'
               AND content.object_kind = ?2 AND content.object_id = ?3"
        }
        _ => {
            return Err(CoreError::internal(
                "module authority linked revision column is unsupported",
            ));
        }
    };
    let row = connection
        .query_row(
            query,
            params![module_revision_id, object_kind, object_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("linked module component authority"))?;
    if row.0 != component.sha256.as_str()
        || row.4 != row.0
        || sha256_hex(row.3.as_bytes()) != row.4
        || row.1 != object_id
    {
        return Err(storage_corrupted(
            "linked module authority differs from its immutable revision",
        ));
    }
    Ok(ModuleAuthorityComponent::Linked {
        target_object_id: row.1,
        target_revision_id: row.2,
    })
}

#[allow(clippy::too_many_lines)] // One conversion covers every typed module component authority.
fn build_module_import_approval_evidence_in_connection(
    connection: &Connection,
    stored: &ActiveContentModuleRevision,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportApprovalEvidence> {
    validate_completed_module_authority_target(stored)?;
    let source_sha256 = parse_authority_sha256("package source", &authority.source_sha256)?;
    let provenance = &stored.object.value.metadata.provenance;
    if authority.status != PackageImportStatus::Completed
        || authority.import_revision == 0
        || provenance.source_kind != SourceKind::ImportedPackage
        || provenance.source_id.as_deref() != Some(authority.package_id.as_str())
        || provenance.source_hash.as_deref() != Some(authority.source_sha256.as_str())
        || stored.module_revision.source_hash != source_sha256
    {
        return Err(package_authority_denied(
            "completed package authority does not own the imported module source",
        ));
    }
    // `document_sha256` authenticates the tagged `PackageCommitDocument`
    // approval payload and is revalidated while loading `authority`; it is not
    // the hash of the inner content revision. The immutable inner module is
    // authenticated by `validate_completed_module_authority_target`, while the
    // exact commit link is the object/revision/component tuple below.
    let module_matches = authority
        .enabled_components
        .iter()
        .filter(|component| component.kind == PackageComponentKind::ContentModule)
        .flat_map(|component| {
            component
                .committed_documents
                .iter()
                .filter(|document| {
                    document.target_object_id == stored.object.value.id.as_str()
                        && document.target_revision_id == stored.module_revision.id.as_str()
                        && document.source_component_sha256 == component.sha256
                })
                .map(move |document| (component, document))
        })
        .collect::<Vec<_>>();
    let [(module_component, module_document)] = module_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not select the exact module revision",
        ));
    };

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
        return Err(storage_corrupted(
            "completed package authority contains duplicate enabled components",
        ));
    }
    let mut authorized_capabilities = authority.required_capabilities.clone();
    authorized_capabilities.sort();
    authorized_capabilities.dedup();

    let mut component_authorities =
        Vec::with_capacity(stored.module_revision.component_hashes.len());
    for component in &stored.module_revision.component_hashes {
        let material = read_module_authority_component(connection, stored, component)?;
        let component_authority = match (&component.component, material) {
            (
                ModuleComponentRef::PromptBlock { .. } | ModuleComponentRef::Control { .. },
                ModuleAuthorityComponent::Embedded,
            ) => module_document_component_authority(component, module_component, module_document)?,
            (
                ModuleComponentRef::KnowledgeBook { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::KnowledgeBook,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (
                ModuleComponentRef::TransformSet { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::TransformSet,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (
                ModuleComponentRef::InteractionRuleSet { .. },
                ModuleAuthorityComponent::Linked {
                    target_object_id,
                    target_revision_id,
                },
            ) => document_module_component_authority(
                component,
                PackageComponentKind::InteractionRuleSet,
                &target_object_id,
                &target_revision_id,
                authority,
            )?,
            (ModuleComponentRef::Asset { id }, ModuleAuthorityComponent::Asset(descriptor)) => {
                asset_module_component_authority(
                    component,
                    id,
                    &descriptor,
                    module_component,
                    authority,
                )?
            }
            _ => {
                return Err(storage_corrupted(
                    "module component material kind differs from its immutable reference",
                ));
            }
        };
        component_authorities.push(component_authority);
    }
    component_authorities.sort();

    Ok(ModuleImportApprovalEvidence {
        approval_id: authority.approval_id.clone(),
        approval_sha256: parse_authority_sha256("package approval", &authority.approval_sha256)?,
        import_id: authority.import_id.clone(),
        import_revision: authority.import_revision,
        package_id: authority.package_id.clone(),
        package_source_sha256: source_sha256,
        selection_sha256: parse_authority_sha256("package selection", &authority.selection_sha256)?,
        capability_review_sha256: parse_authority_sha256(
            "package capability review",
            &authority.capability_review_sha256,
        )?,
        module_id: stored.object.value.id.clone(),
        module_revision_id: stored.module_revision.id.clone(),
        module_revision_source_sha256: stored.module_revision.source_hash.clone(),
        module_package_component_id: module_component.component_id.clone(),
        module_package_component_sha256: parse_authority_sha256(
            "package module component",
            &module_component.sha256,
        )?,
        module_commit_result_sha256: parse_authority_sha256(
            "package module commit",
            &module_document.result_sha256,
        )?,
        selected_package_component_ids,
        authorized_capabilities,
        component_authorities,
    })
}

fn module_document_component_authority(
    component: &lorepia_domain::ComponentHash,
    module_component: &CompletedPackageComponentAuthority,
    module_document: &CompletedPackageDocumentAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: module_component.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package module component",
            &module_component.sha256,
        )?,
        committed_target_object_id: module_document.target_object_id.clone(),
        committed_target_revision_id: module_document.target_revision_id.clone(),
        committed_result_sha256: parse_authority_sha256(
            "package module commit",
            &module_document.result_sha256,
        )?,
        committed_content_sha256: None,
    })
}

fn document_module_component_authority(
    component: &lorepia_domain::ComponentHash,
    kind: PackageComponentKind,
    target_object_id: &str,
    target_revision_id: &str,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    // The approved document hash covers the tagged package-commit envelope.
    // `read_linked_module_authority_component` has already authenticated the
    // inner child revision and its component hash, so bind that immutable child
    // to the exact committed object/revision/component tuple here.
    let matches = authority
        .enabled_components
        .iter()
        .filter(|candidate| candidate.kind == kind)
        .flat_map(|candidate| {
            candidate
                .committed_documents
                .iter()
                .filter(|document| {
                    document.target_object_id == target_object_id
                        && document.target_revision_id == target_revision_id
                        && document.source_component_sha256 == candidate.sha256
                })
                .map(move |document| (candidate, document))
        })
        .collect::<Vec<_>>();
    let [(package_component, document)] = matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not cover an exact module component revision",
        ));
    };
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: package_component.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package component",
            &package_component.sha256,
        )?,
        committed_target_object_id: document.target_object_id.clone(),
        committed_target_revision_id: document.target_revision_id.clone(),
        committed_result_sha256: parse_authority_sha256(
            "package component commit",
            &document.result_sha256,
        )?,
        committed_content_sha256: None,
    })
}

fn asset_module_component_authority(
    component: &lorepia_domain::ComponentHash,
    asset_id: &AssetId,
    descriptor: &AssetDescriptor,
    module_component: &CompletedPackageComponentAuthority,
    authority: &CompletedPackageAuthority,
) -> CoreResult<ModuleImportComponentAuthority> {
    let asset_matches = authority
        .committed_assets
        .iter()
        .filter(|asset| {
            asset.asset_id == *asset_id
                && asset.descriptor == *descriptor
                && asset.descriptor_sha256 == component.sha256.as_str()
                && asset.cas_sha256 == descriptor.sha256.as_str()
        })
        .collect::<Vec<_>>();
    let [asset] = asset_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not cover an exact module asset",
        ));
    };
    let source_matches = asset
        .source_components
        .iter()
        .filter(|source| {
            source.component_id == module_component.component_id
                && source.component_sha256 == module_component.sha256
        })
        .collect::<Vec<_>>();
    let [source] = source_matches.as_slice() else {
        return Err(package_authority_denied(
            "completed package authority does not bind the exact asset to the module component",
        ));
    };
    let descriptor_sha256 =
        parse_authority_sha256("package asset descriptor", &asset.descriptor_sha256)?;
    Ok(ModuleImportComponentAuthority {
        component: component.component.clone(),
        component_sha256: component.sha256.clone(),
        package_component_id: source.component_id.clone(),
        package_component_sha256: parse_authority_sha256(
            "package asset component",
            &source.component_sha256,
        )?,
        committed_target_object_id: asset.asset_id.as_str().to_owned(),
        committed_target_revision_id: asset.descriptor_sha256.clone(),
        committed_result_sha256: descriptor_sha256,
        committed_content_sha256: Some(parse_authority_sha256(
            "package asset content",
            &asset.cas_sha256,
        )?),
    })
}

fn parse_authority_sha256(label: &str, value: &str) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(value.to_owned())
        .map_err(|error| storage_corrupted(format!("completed {label} hash is invalid: {error}")))
}

fn package_authority_denied(message: &'static str) -> CoreError {
    CoreError::new(CoreErrorCode::PermissionDenied, message, false)
}

const fn component_kind_str(kind: PackageComponentKind) -> &'static str {
    match kind {
        PackageComponentKind::PromptPreset => "prompt_preset",
        PackageComponentKind::MemoryProfile => "memory_profile",
        PackageComponentKind::KnowledgeBook => "knowledge_book",
        PackageComponentKind::TransformSet => "transform_set",
        PackageComponentKind::InteractionRuleSet => "interaction_rule_set",
        PackageComponentKind::ContentModule => "content_module",
        PackageComponentKind::AssetIndex => "asset",
        PackageComponentKind::RawExtension => "raw_extension",
    }
}

const fn import_status_str(status: PackageImportStatus) -> &'static str {
    match status {
        PackageImportStatus::Inspected => "inspected",
        PackageImportStatus::AwaitingReview => "awaiting_review",
        PackageImportStatus::Approved => "approved",
        PackageImportStatus::Committing => "committing",
        PackageImportStatus::Completed => "completed",
        PackageImportStatus::Failed => "failed",
        PackageImportStatus::Discarded => "discarded",
        PackageImportStatus::RolledBack => "rolled_back",
    }
}

fn parse_import_status(value: &str) -> CoreResult<PackageImportStatus> {
    match value {
        "inspected" => Ok(PackageImportStatus::Inspected),
        "awaiting_review" => Ok(PackageImportStatus::AwaitingReview),
        "approved" => Ok(PackageImportStatus::Approved),
        "committing" => Ok(PackageImportStatus::Committing),
        "completed" => Ok(PackageImportStatus::Completed),
        "failed" => Ok(PackageImportStatus::Failed),
        "discarded" => Ok(PackageImportStatus::Discarded),
        "rolled_back" => Ok(PackageImportStatus::RolledBack),
        _ => Err(storage_corrupted("stored package import state is invalid")),
    }
}

fn parse_package_capability(value: &str) -> CoreResult<PackageCapability> {
    match value {
        "prompt_fragments" => Ok(PackageCapability::PromptFragments),
        "knowledge" => Ok(PackageCapability::Knowledge),
        "variables" => Ok(PackageCapability::Variables),
        "transforms" => Ok(PackageCapability::Transforms),
        "declarative_interactions" => Ok(PackageCapability::DeclarativeInteractions),
        "image_assets" => Ok(PackageCapability::ImageAssets),
        "audio_assets" => Ok(PackageCapability::AudioAssets),
        "video_assets" => Ok(PackageCapability::VideoAssets),
        "attachment_assets" => Ok(PackageCapability::AttachmentAssets),
        "high_risk_assets" => Ok(PackageCapability::HighRiskAssets),
        "external_urls" => Ok(PackageCapability::ExternalUrls),
        "html" => Ok(PackageCapability::Html),
        "script" => Ok(PackageCapability::Script),
        "native_code" => Ok(PackageCapability::NativeCode),
        "network" => Ok(PackageCapability::Network),
        "filesystem" => Ok(PackageCapability::Filesystem),
        "shell" => Ok(PackageCapability::Shell),
        "credentials" => Ok(PackageCapability::Credentials),
        _ => Err(storage_corrupted(
            "stored package capability name is invalid",
        )),
    }
}

fn parse_capability_support(value: &str) -> CoreResult<PackageCapabilitySupport> {
    match value {
        "supported" => Ok(PackageCapabilitySupport::Supported),
        "unsupported" => Ok(PackageCapabilitySupport::Unsupported),
        "approval_required" => Ok(PackageCapabilitySupport::ApprovalRequired),
        _ => Err(storage_corrupted(
            "stored package capability support is invalid",
        )),
    }
}

fn license_fields(license: &str) -> (Option<&str>, &'static str) {
    let license = license.trim();
    if license.is_empty() {
        (None, "missing")
    } else if license.eq_ignore_ascii_case("unknown")
        || license.eq_ignore_ascii_case("LicenseRef-Unknown")
    {
        (Some(license), "unknown")
    } else {
        (Some(license), "declared")
    }
}

fn encode_json<T: Serialize>(label: &str, value: &T) -> CoreResult<String> {
    let json = serde_json::to_string(value)
        .map_err(|error| CoreError::invalid(format!("{label} cannot be encoded: {error}")))?;
    validate_json(label, &json)?;
    Ok(json)
}

fn decode_json<T: DeserializeOwned>(label: &str, json: &str) -> CoreResult<T> {
    validate_json(label, json).map_err(|error| {
        storage_corrupted(format!(
            "{label} violates storage bounds: {}",
            error.message
        ))
    })?;
    serde_json::from_str(json)
        .map_err(|error| storage_corrupted(format!("stored {label} is invalid: {error}")))
}

fn validate_json(label: &str, json: &str) -> CoreResult<()> {
    if json.len() > MAX_PACKAGE_JSON_BYTES {
        return Err(CoreError::invalid(format!(
            "{label} exceeds the package JSON limit"
        )));
    }
    let value: Value = serde_json::from_str(json)
        .map_err(|error| CoreError::invalid(format!("{label} is invalid JSON: {error}")))?;
    let mut pending = vec![(&value, 0_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_PACKAGE_JSON_NODES || depth > MAX_PACKAGE_JSON_DEPTH {
            return Err(CoreError::invalid(format!(
                "{label} exceeds package JSON structural limits"
            )));
        }
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    if is_secret_key(key) {
                        return Err(CoreError::invalid(format!(
                            "{label} contains a raw credential field"
                        )));
                    }
                    pending.push((child, depth.saturating_add(1)));
                }
            }
            Value::Array(array) => {
                pending.extend(array.iter().map(|child| (child, depth.saturating_add(1))));
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "api_key"
            | "authorization"
            | "password"
            | "private_key"
            | "client_secret"
            | "access_token"
            | "refresh_token"
            | "credential"
    )
}

fn validate_identifier(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(format!("{label} identifier is invalid")));
    }
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> CoreResult<()> {
    if value.len() != 64
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        || value.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        return Err(CoreError::invalid(format!(
            "{label} SHA-256 digest is invalid"
        )));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn parse_datetime(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| storage_corrupted(format!("stored {label} is invalid: {error}")))
}

fn u64_from_i64(label: &str, value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| storage_corrupted(format!("stored {label} is negative")))
}

fn u32_from_i64(label: &str, value: i64) -> CoreResult<u32> {
    u32::try_from(value).map_err(|_| storage_corrupted(format!("stored {label} is out of range")))
}

fn i64_from_u64(label: &str, value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid(format!("{label} exceeds SQLite range")))
}

fn not_found(kind: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{kind} was not found"),
        false,
    )
}

fn revision_conflict(
    kind: &str,
    id: &str,
    expected: Option<u64>,
    actual: Option<u64>,
) -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        format!(
            "{kind} revision conflict for {id}: expected {}, current {}",
            expected.map_or_else(|| "new".to_owned(), |value| value.to_string()),
            actual.map_or_else(|| "missing".to_owned(), |value| value.to_string())
        ),
        true,
    )
}

fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{
        AssetDescriptor, AssetId, AssetRole, AssetSource, AssetSourceKind, KnowledgeBook,
        MemoryProfile, PackageManifest, PromptPresetId, Provenance, Sha256Digest,
    };
    use lorepia_orchestration::RedistributionStatus;
    use tempfile::tempdir;

    use super::*;

    fn write_staged(storage: &Storage, name: &str, bytes: &[u8]) -> PathBuf {
        let path = storage.staging_dir().join(name);
        fs::write(&path, bytes).expect("write owned staged fixture");
        path
    }

    fn source_record(hash: &str, size: u64) -> PackageSourceRecord {
        PackageSourceRecord {
            id: format!("package-source-{hash}"),
            package_id: PackageId::from("cleanup-package"),
            format: "lorepia_content_package".to_owned(),
            format_version: 1,
            name: "Cleanup package".to_owned(),
            version: "1.0.0".to_owned(),
            source_sha256: hash.to_owned(),
            source_size_bytes: size,
            author: Some("LorePia tests".to_owned()),
            license: "MIT".to_owned(),
            redistribution_allowed: true,
            manifest: VersionedJson {
                schema_version: 1,
                value: json!({}),
            },
            created_at: Utc::now(),
        }
    }

    fn invalid_review(hash: &str) -> PackageReview {
        let digest = Sha256Digest::parse(hash).expect("fixture digest");
        PackageReview {
            review_sha256: Sha256Digest::parse("00".repeat(32)).expect("review digest"),
            source_sha256: digest.clone(),
            manifest: PackageManifest {
                format: "lorepia_content_package".to_owned(),
                format_version: 1,
                package_id: PackageId::from("cleanup-package"),
                name: "Cleanup package".to_owned(),
                version: "1.0.0".to_owned(),
                author: Some("LorePia tests".to_owned()),
                license: "MIT".to_owned(),
                redistribution_allowed: true,
                required_app_version: None,
                required_capabilities: Vec::new(),
                content_hashes: Vec::new(),
                signature: None,
                provenance: Provenance {
                    source_kind: SourceKind::ImportedPackage,
                    source_id: Some("cleanup-package".to_owned()),
                    source_hash: Some(hash.to_owned()),
                    author: Some("LorePia tests".to_owned()),
                    license: Some("MIT".to_owned()),
                    imported_at: Some(Utc::now()),
                },
            },
            components: Vec::new(),
            assets: Vec::new(),
            issues: Vec::new(),
            local_import_allowed: true,
            redistribution_status: RedistributionStatus::Allowed,
        }
    }

    fn promote_missing_approved_source(storage: &Storage) -> (String, u64) {
        let source_bytes = b"synthetic commit source";
        let source_hash = sha256_hex(source_bytes);
        let source_size = u64::try_from(source_bytes.len()).expect("small source fixture");
        let source_staged = write_staged(storage, "commit.snapshot", source_bytes);
        storage
            .promote_package_source(
                "missing-approved-import",
                &source_staged,
                &source_hash,
                source_size,
            )
            .expect("promote source");
        (source_hash, source_size)
    }

    fn imported_document_provenance() -> Provenance {
        Provenance {
            source_kind: SourceKind::ImportedPackage,
            source_id: Some("dev.lorepia.storage-validation-test".to_owned()),
            source_hash: Some("ab".repeat(32)),
            author: Some("LorePia tests".to_owned()),
            license: Some("MIT".to_owned()),
            imported_at: None,
        }
    }

    #[test]
    fn storage_rejects_noncanonical_imported_knowledge_before_persistence() {
        let book: KnowledgeBook = serde_json::from_value(json!({
            "id": "storage.package.invalid-knowledge",
            "name": "Invalid imported knowledge",
            "schema_version": 1,
            "entries": [],
            "scan_depth": 1025,
            "token_budget": {"max_tokens": 1024},
            "recursive": false,
            "max_recursion_depth": 0,
            "provenance": imported_document_provenance()
        }))
        .expect("typed invalid knowledge fixture");
        let error =
            validate_normalized_package_documents(&[PackageCommitDocument::KnowledgeBook(book)])
                .expect_err("storage must reject invalid knowledge before persistence");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
    }

    #[test]
    fn storage_rejects_noncanonical_imported_memory_before_persistence() {
        let profile: MemoryProfile = serde_json::from_value(json!({
            "id": "storage.package.invalid-memory",
            "name": "Invalid imported memory",
            "schema_version": 1,
            "summary_task": "memory-summary",
            "embedding_task": null,
            "turns_per_summary": 0,
            "recent_raw_budget": {"max_tokens": 1024},
            "episodic_budget": {"max_tokens": 1024},
            "semantic_budget": {"max_tokens": 1024},
            "retrieval_count": 8,
            "recency_weight": 1.0,
            "similarity_weight": 1.0,
            "importance_weight": 1.0,
            "preserve_invalidated_records": false,
            "summary_schema": "memory-summary-v1",
            "provenance": imported_document_provenance()
        }))
        .expect("typed invalid memory fixture");
        let error =
            validate_normalized_package_documents(&[PackageCommitDocument::MemoryProfile(profile)])
                .expect_err("storage must reject invalid memory before persistence");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
    }

    #[test]
    fn storage_rejects_imported_prompt_with_elevated_package_block_authority() {
        let imported_provenance = imported_document_provenance();
        let mut preset = crate::orchestration::built_in_prompt_presets()[0].clone();
        preset.id = PromptPresetId::from("storage.package.prompt-authority-boundary");
        preset.metadata.provenance = imported_provenance.clone();
        for block in preset.blocks.iter_mut().skip(1) {
            block.authority = InstructionAuthority::ImportedContent;
            block.provenance = imported_provenance.clone();
        }
        preset.blocks[1].authority = InstructionAuthority::Creator;

        let error =
            validate_normalized_package_documents(&[PackageCommitDocument::PromptPreset(preset)])
                .expect_err(
                    "storage must reject elevated imported prompt authority before persistence",
                );
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
    }

    fn promote_missing_approved_asset(
        storage: &Storage,
        source_hash: &str,
    ) -> (StagedAssetImport, PathBuf, AssetDescriptor) {
        let asset_bytes = b"\x89PNG\r\n\x1a\nsynthetic-package-image";
        let asset_hash = sha256_hex(asset_bytes);
        let asset_size = u64::try_from(asset_bytes.len()).expect("small asset fixture");
        let staged_asset = StagedAssetImport {
            staged_path: write_staged(storage, "asset.partial", asset_bytes),
            sha256: asset_hash.clone(),
            media_type: "image/png".to_owned(),
            size_bytes: asset_size,
        };
        let durable_assets = storage
            .promote_package_assets(
                "missing-approved-import",
                std::slice::from_ref(&staged_asset),
            )
            .expect("promote asset");
        assert_eq!(durable_assets.len(), 1);
        let durable_asset = durable_assets
            .into_iter()
            .next()
            .expect("one promoted asset");
        assert!(durable_asset.is_file());
        let descriptor = AssetDescriptor {
            id: AssetId::from("cleanup-asset"),
            sha256: Sha256Digest::parse(&asset_hash).expect("asset digest"),
            media_type: "image/png".to_owned(),
            role: AssetRole::Illustration,
            name: "asset.png".to_owned(),
            size_bytes: asset_size,
            width: None,
            height: None,
            duration_ms: None,
            source: AssetSource {
                kind: AssetSourceKind::LorepiaPackage,
                source_sha256: Some(Sha256Digest::parse(source_hash).expect("source digest")),
                logical_path: Some("assets/asset.png".to_owned()),
            },
        };
        (staged_asset, durable_asset, descriptor)
    }

    fn missing_approved_package_commit_input(
        source_hash: &str,
        source_size: u64,
        asset: AssetDescriptor,
    ) -> PackageCommitInput {
        let now = Utc::now();
        PackageCommitInput {
            source: source_record(source_hash, source_size),
            import: PackageImportRecord {
                id: "missing-approved-import".to_owned(),
                package_id: PackageId::from("cleanup-package"),
                status: PackageImportStatus::Approved,
                revision: 3,
                inspection: VersionedJson {
                    schema_version: 1,
                    value: json!({}),
                },
                selection: Some(VersionedJson {
                    schema_version: 1,
                    value: json!({}),
                }),
                selected_component_ids: Vec::new(),
                failure_code: None,
                created_at: now,
                updated_at: now,
            },
            documents: Vec::new(),
            assets: vec![asset],
        }
    }

    #[test]
    fn failed_inspection_creation_removes_unclaimed_source_row_and_cas_bytes() {
        let root = tempdir().expect("data root");
        let storage = Storage::open(root.path()).expect("open storage");
        let bytes = b"synthetic package source";
        let hash = sha256_hex(bytes);
        let source_size = u64::try_from(bytes.len()).expect("small source fixture");
        let staged = write_staged(&storage, "source.snapshot", bytes);
        let durable = storage
            .promote_package_source("cleanup-invalid-inspection", &staged, &hash, source_size)
            .expect("promote source");
        assert!(durable.is_file());

        let source = source_record(&hash, source_size);
        let now = Utc::now();
        let import = PackageImportRecord {
            id: "cleanup-invalid-inspection".to_owned(),
            package_id: source.package_id.clone(),
            status: PackageImportStatus::Inspected,
            revision: 1,
            inspection: VersionedJson {
                schema_version: 1,
                value: json!({}),
            },
            selection: None,
            selected_component_ids: Vec::new(),
            failure_code: None,
            created_at: now,
            updated_at: now,
        };
        let error = storage
            .create_inspected_package_import(
                &source,
                &import,
                &invalid_review(&hash),
                &PackageCapabilityReview {
                    schema_version: 1,
                    decisions: Vec::new(),
                },
            )
            .expect_err("invalid inspection must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(
            storage
                .discard_unclaimed_package_source("cleanup-invalid-inspection", &hash, source_size,)
                .expect("compensate source")
        );
        assert!(!durable.exists());
        assert_eq!(
            storage
                .package_source_path(&hash, source_size)
                .expect_err("source row must be removed")
                .code,
            CoreErrorCode::NotFound
        );
    }

    #[test]
    fn failed_package_commit_removes_unclaimed_asset_row_and_cas_bytes() {
        let root = tempdir().expect("data root");
        let storage = Storage::open(root.path()).expect("open storage");
        let (source_hash, source_size) = promote_missing_approved_source(&storage);
        let (staged_asset, durable_asset, asset) =
            promote_missing_approved_asset(&storage, &source_hash);
        let input = missing_approved_package_commit_input(&source_hash, source_size, asset);
        let expectation = PackageImportExpectation {
            revision: 3,
            inspection_sha256: "11".repeat(32),
            selection_sha256: "22".repeat(32),
            capability_review_sha256: "33".repeat(32),
        };
        let error = storage
            .commit_package_import(&input, &expectation, &[])
            .expect_err("missing approved import must fail after CAS verification");
        assert_eq!(error.code, CoreErrorCode::NotFound);
        assert_eq!(
            storage
                .discard_unclaimed_package_assets(
                    "missing-approved-import",
                    std::slice::from_ref(&staged_asset),
                )
                .expect("compensate asset"),
            1
        );
        assert!(!durable_asset.exists());
        assert!(
            storage
                .discard_unclaimed_package_source(
                    "missing-approved-import",
                    &source_hash,
                    source_size,
                )
                .expect("compensate source")
        );
    }

    struct ModuleAssetAuthorityFixture {
        asset_id: AssetId,
        asset_content_sha256: Sha256Digest,
        descriptor: AssetDescriptor,
        descriptor_sha256: String,
        module_component: CompletedPackageComponentAuthority,
        component: lorepia_domain::ComponentHash,
        authority: CompletedPackageAuthority,
    }

    fn module_asset_authority_fixture() -> ModuleAssetAuthorityFixture {
        let asset_id = AssetId::from("module-asset");
        let asset_content_sha256 =
            Sha256Digest::parse("11".repeat(32)).expect("asset content digest");
        let descriptor = AssetDescriptor {
            id: asset_id.clone(),
            sha256: asset_content_sha256.clone(),
            media_type: "image/png".to_owned(),
            role: AssetRole::Illustration,
            name: "module-asset.png".to_owned(),
            size_bytes: 123,
            width: Some(16),
            height: Some(16),
            duration_ms: None,
            source: AssetSource {
                kind: AssetSourceKind::LorepiaPackage,
                source_sha256: Some(
                    Sha256Digest::parse("22".repeat(32)).expect("package source digest"),
                ),
                logical_path: Some("assets/module-asset.png".to_owned()),
            },
        };
        let descriptor_json =
            encode_json("module asset descriptor fixture", &descriptor).expect("encode descriptor");
        let descriptor_sha256 = sha256_hex(descriptor_json.as_bytes());
        let module_component = CompletedPackageComponentAuthority {
            component_id: "content-module-component".to_owned(),
            kind: PackageComponentKind::ContentModule,
            sha256: "33".repeat(32),
            committed_documents: Vec::new(),
        };
        let component = lorepia_domain::ComponentHash {
            component: ModuleComponentRef::Asset {
                id: asset_id.clone(),
            },
            sha256: Sha256Digest::parse(&descriptor_sha256).expect("descriptor digest"),
        };
        let authority = CompletedPackageAuthority {
            approval_id: "package-approval".to_owned(),
            import_id: "package-import".to_owned(),
            package_id: PackageId::from("module-package"),
            status: PackageImportStatus::Completed,
            import_revision: 5,
            source_sha256: "22".repeat(32),
            inspection_sha256: "44".repeat(32),
            selection_sha256: "55".repeat(32),
            capability_review_sha256: "66".repeat(32),
            approval_sha256: "77".repeat(32),
            required_capabilities: vec![ContentCapability::ImageAssets],
            approved_capabilities: Vec::new(),
            enabled_components: vec![
                module_component.clone(),
                CompletedPackageComponentAuthority {
                    component_id: "asset-index-component".to_owned(),
                    kind: PackageComponentKind::AssetIndex,
                    sha256: "88".repeat(32),
                    committed_documents: Vec::new(),
                },
            ],
            committed_assets: vec![CompletedPackageAssetAuthority {
                asset_id: asset_id.clone(),
                descriptor: descriptor.clone(),
                descriptor_sha256: descriptor_sha256.clone(),
                cas_sha256: asset_content_sha256.as_str().to_owned(),
                source_components: vec![
                    CompletedPackageAssetSourceAuthority {
                        component_id: module_component.component_id.clone(),
                        component_sha256: module_component.sha256.clone(),
                    },
                    CompletedPackageAssetSourceAuthority {
                        component_id: "asset-index-component".to_owned(),
                        component_sha256: "88".repeat(32),
                    },
                ],
            }],
        };
        ModuleAssetAuthorityFixture {
            asset_id,
            asset_content_sha256,
            descriptor,
            descriptor_sha256,
            module_component,
            component,
            authority,
        }
    }

    #[test]
    fn module_asset_authority_is_bound_to_the_content_module_component() {
        let fixture = module_asset_authority_fixture();
        let evidence = asset_module_component_authority(
            &fixture.component,
            &fixture.asset_id,
            &fixture.descriptor,
            &fixture.module_component,
            &fixture.authority,
        )
        .expect("module asset authority");
        assert_eq!(
            evidence.package_component_id,
            fixture.module_component.component_id
        );
        assert_eq!(
            evidence.package_component_sha256.as_str(),
            fixture.module_component.sha256
        );
        assert_eq!(
            evidence.committed_target_object_id,
            fixture.asset_id.as_str()
        );
        assert_eq!(
            evidence.committed_target_revision_id,
            fixture.descriptor_sha256
        );
        assert_eq!(
            evidence.committed_result_sha256.as_str(),
            fixture.descriptor_sha256
        );
        assert_eq!(
            evidence.committed_content_sha256.as_ref(),
            Some(&fixture.asset_content_sha256)
        );

        let unrelated_module_component = CompletedPackageComponentAuthority {
            component_id: "other-content-module".to_owned(),
            kind: PackageComponentKind::ContentModule,
            sha256: "99".repeat(32),
            committed_documents: Vec::new(),
        };
        assert_eq!(
            asset_module_component_authority(
                &fixture.component,
                &fixture.asset_id,
                &fixture.descriptor,
                &unrelated_module_component,
                &fixture.authority,
            )
            .expect_err("unrelated module component must not authorize the asset")
            .code,
            CoreErrorCode::PermissionDenied
        );
    }

    #[test]
    fn linked_module_authority_keeps_package_and_inner_document_hashes_distinct() {
        let target_object_id = "module-knowledge";
        let target_revision_id = "module-knowledge-revision";
        let inner_document_sha256 = Sha256Digest::parse("11".repeat(32)).expect("inner digest");
        let package_component_sha256 = "22".repeat(32);
        let package_document_sha256 = "33".repeat(32);
        let commit_result_sha256 = "44".repeat(32);
        let component = lorepia_domain::ComponentHash {
            component: ModuleComponentRef::KnowledgeBook {
                id: lorepia_domain::KnowledgeBookId::from(target_object_id),
            },
            sha256: inner_document_sha256.clone(),
        };
        let package_component = CompletedPackageComponentAuthority {
            component_id: "knowledge-component".to_owned(),
            kind: PackageComponentKind::KnowledgeBook,
            sha256: package_component_sha256.clone(),
            committed_documents: vec![CompletedPackageDocumentAuthority {
                document_ordinal: 0,
                target_object_id: target_object_id.to_owned(),
                target_revision_id: target_revision_id.to_owned(),
                source_component_sha256: package_component_sha256.clone(),
                document_sha256: package_document_sha256.clone(),
                result_sha256: commit_result_sha256.clone(),
            }],
        };
        let authority = CompletedPackageAuthority {
            approval_id: "package-approval".to_owned(),
            import_id: "package-import".to_owned(),
            package_id: PackageId::from("module-package"),
            status: PackageImportStatus::Completed,
            import_revision: 5,
            source_sha256: "55".repeat(32),
            inspection_sha256: "66".repeat(32),
            selection_sha256: "77".repeat(32),
            capability_review_sha256: "88".repeat(32),
            approval_sha256: "99".repeat(32),
            required_capabilities: vec![ContentCapability::Knowledge],
            approved_capabilities: Vec::new(),
            enabled_components: vec![package_component.clone()],
            committed_assets: Vec::new(),
        };

        assert_ne!(
            package_document_sha256,
            inner_document_sha256.as_str(),
            "the package binding hashes a tagged commit envelope, not the inner revision"
        );
        let evidence = document_module_component_authority(
            &component,
            PackageComponentKind::KnowledgeBook,
            target_object_id,
            target_revision_id,
            &authority,
        )
        .expect("exact linked document authority");
        assert_eq!(evidence.component_sha256, inner_document_sha256);
        assert_eq!(
            evidence.package_component_id,
            package_component.component_id
        );
        assert_eq!(
            evidence.package_component_sha256.as_str(),
            package_component_sha256
        );
        assert_eq!(evidence.committed_target_object_id, target_object_id);
        assert_eq!(evidence.committed_target_revision_id, target_revision_id);
        assert_eq!(
            evidence.committed_result_sha256.as_str(),
            commit_result_sha256
        );

        assert_eq!(
            document_module_component_authority(
                &component,
                PackageComponentKind::KnowledgeBook,
                target_object_id,
                "different-revision",
                &authority,
            )
            .expect_err("a different immutable revision must not be authorized")
            .code,
            CoreErrorCode::PermissionDenied
        );
        let mut wrong_source = authority;
        wrong_source.enabled_components[0].committed_documents[0].source_component_sha256 =
            "aa".repeat(32);
        assert_eq!(
            document_module_component_authority(
                &component,
                PackageComponentKind::KnowledgeBook,
                target_object_id,
                target_revision_id,
                &wrong_source,
            )
            .expect_err("a different reviewed package component must not authorize the child")
            .code,
            CoreErrorCode::PermissionDenied
        );
    }

    fn target_review_document(index: u32) -> PackageDocumentTargetReview {
        PackageDocumentTargetReview {
            source_component_id: format!("component-{index}"),
            component_document_ordinal: 0,
            document_index: index,
            document_kind: "knowledge_book".to_owned(),
            target_object_id: format!("target-{index}"),
            disposition: PackageDocumentTargetDisposition::Create,
            expected_target_revision_id: None,
            expected_target_state_revision: None,
            source_component_sha256: "11".repeat(32),
            document_sha256: "22".repeat(32),
        }
    }

    #[test]
    fn target_review_document_limit_is_enforced_by_the_canonical_digest_boundary() {
        let allowed = (0..u32::try_from(MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS)
            .expect("target-review limit fits u32"))
            .map(target_review_document)
            .collect::<Vec<_>>();
        package_import_target_review_sha256(&allowed).expect("bounded target review");

        let mut excessive = allowed;
        excessive.push(target_review_document(
            u32::try_from(MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS)
                .expect("target-review limit fits u32"),
        ));
        let error = package_import_target_review_sha256(&excessive)
            .expect_err("oversized target review must fail before persistence");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.message.contains("200-document limit"));
    }

    #[test]
    fn update_confirmations_are_canonical_and_exact_while_create_needs_none() {
        let mut documents = vec![target_review_document(0), target_review_document(1)];
        documents[0].source_component_id = "mixed-component".to_owned();
        documents[0].component_document_ordinal = 0;
        documents[0].target_object_id = "existing-target".to_owned();
        documents[0].disposition = PackageDocumentTargetDisposition::Update;
        documents[0].expected_target_revision_id = Some("existing-revision".to_owned());
        documents[0].expected_target_state_revision = Some(7);
        documents[1].source_component_id = "mixed-component".to_owned();
        documents[1].component_document_ordinal = 1;
        let confirmation = PackageUpdateTargetConfirmation {
            source_component_id: "mixed-component".to_owned(),
            component_document_ordinal: 0,
            target_object_id: "existing-target".to_owned(),
            expected_target_revision_id: "existing-revision".to_owned(),
            expected_target_state_revision: 7,
        };

        validate_document_target_reviews(&documents).expect("mixed target review");
        validate_exact_update_target_confirmations(&documents, std::slice::from_ref(&confirmation))
            .expect("exact update confirmation");
        validate_exact_update_target_confirmations(&documents, &[])
            .expect_err("missing update confirmation");
        let mut stale = confirmation;
        stale.expected_target_state_revision += 1;
        validate_exact_update_target_confirmations(&documents, &[stale])
            .expect_err("stale update confirmation");
    }

    #[test]
    fn completed_package_export_listing_is_bounded_ordered_and_status_filtered() {
        let root = tempdir().expect("data root");
        let storage = Storage::open(root.path()).expect("open storage");
        let source_sha256 = "ab".repeat(32);
        let connection = storage.connection().expect("open fixture connection");
        connection
            .execute("DROP TRIGGER package_imports_initial_state_guard", [])
            .expect("allow terminal-state list fixtures");
        connection
            .execute(
                "DROP TRIGGER package_imports_require_inspected_initial_state_v19",
                [],
            )
            .expect("allow terminal-state recovery fixtures");
        connection
            .execute(
                "INSERT INTO content_sources (
                    sha256, relative_path, size_bytes, created_at
                 ) VALUES (?1, 'sources/synthetic', 1, '2026-08-09T00:00:00Z')",
                [source_sha256.as_str()],
            )
            .expect("content source fixture");
        connection
            .execute(
                "INSERT INTO package_sources (
                    id, source_hash, format, format_version, package_id, name,
                    version, author, manifest_json, manifest_sha256,
                    license_expression, license_status, redistribution_status,
                    required_app_version, signature_json, signature_status,
                    created_at
                 ) VALUES (
                    'completed-export-source', ?1, 'lorepia_content_package', 1,
                    'completed-export-package', 'Completed export package',
                    '1.0.0', NULL, '{}', ?2, NULL, 'unknown', 'unknown',
                    NULL, NULL, 'unsigned', '2026-08-09T00:00:00Z'
                 )",
                params![source_sha256, "cd".repeat(32)],
            )
            .expect("package source fixture");
        for (id, state, updated_at) in [
            ("completed-old", "completed", "2026-08-09T01:00:00Z"),
            ("completed-b", "completed", "2026-08-09T03:00:00Z"),
            ("completed-a", "completed", "2026-08-09T03:00:00Z"),
            ("rolled-back-newer", "rolled_back", "2026-08-09T04:00:00Z"),
        ] {
            connection
                .execute(
                    "INSERT INTO package_imports (
                        id, package_source_id, inspection_schema_version,
                        state, revision, inspection_json, inspection_sha256,
                        selection_json, selection_sha256,
                        capability_review_sha256, approved_selection_sha256,
                        approved_at, failure_json, created_at, updated_at,
                        completed_at
                     ) VALUES (
                        ?1, 'completed-export-source', 1, ?2, 4, '{}', ?3,
                        '{}', ?3, ?3, ?3, '2026-08-09T00:30:00Z', NULL,
                        '2026-08-09T00:00:00Z', ?4, ?4
                     )",
                    params![id, state, "ef".repeat(32), updated_at],
                )
                .expect("terminal package import fixture");
        }
        drop(connection);

        assert_eq!(
            storage
                .list_completed_package_import_ids(2)
                .expect("bounded completed export identities"),
            ["completed-a", "completed-b"]
        );
        assert_eq!(
            storage
                .list_completed_package_import_ids(MAX_COMPLETED_PACKAGE_EXPORTS)
                .expect("all completed export identities"),
            ["completed-a", "completed-b", "completed-old"]
        );
        for invalid_limit in [0, MAX_COMPLETED_PACKAGE_EXPORTS + 1] {
            let error = storage
                .list_completed_package_import_ids(invalid_limit)
                .expect_err("completed export list limit must fail closed");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
        }
    }
}
