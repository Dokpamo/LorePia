//! UI-safe content-package review and lifecycle projections.
//!
//! The native picker path enters this module only through [`StagedImportFile`].
//! It is never serializable, and every later operation uses only a durable
//! Core-owned import identifier plus exact review hashes.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::Path,
};

use lorepia_core::{
    ContentCapability, ContentPackageApprovalRequest, ContentPackageCommitRequest,
    ContentPackageDiscardRequest, ContentPackageImportInspection, ContentPackageImportReview,
    ContentPackageSelectionRequest, ContentSourceExportDescriptor as CoreExportDescriptor,
    ContentSourceExportKind as CoreExportKind, ContentSourceExportSelector, CoreError,
    CoreErrorCode, MAX_COMPLETED_PACKAGE_EXPORTS, MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS,
    PackageCapability, PackageCapabilityDecision, PackageCapabilitySupport,
    PackageComponentDisposition, PackageComponentKind, PackageDocumentTargetDisposition,
    PackageDocumentTargetReview, PackageImportRecord, PackageImportStatus,
    PackageImportTargetReview, PackageIssueSeverity, PackageManifest, PackageNormalizationEvidence,
    PackageReview, PackageUpdateTargetConfirmation,
    PreparedContentSourceExport as CorePreparedContentSourceExport, RedistributionStatus,
    Sha256Digest,
};
use serde::{Deserialize, Serialize};

use crate::{ShellApi, ShellError, ShellResult, StagedImportFile, api::validate_identifier};

const MAX_PENDING_CONTENT_PACKAGE_IMPORTS: u32 = 100;
const MAX_CONTENT_PACKAGE_COMPONENTS: usize = 4_096;
const MAX_CONTENT_PACKAGE_TARGET_DOCUMENTS: usize = MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS;
const MAX_CONTENT_PACKAGE_NORMALIZATION_EVIDENCE: usize = 4_096;
const MAX_CONTENT_PACKAGE_ISSUES: usize = 4_096;
const MAX_CONTENT_PACKAGE_IPC_BYTES: usize = 2 * 1_024 * 1_024;
const MAX_JAVASCRIPT_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Serializable request for preparing one project-owned source export.
///
/// The result is deliberately Rust-only; the webview receives only the
/// post-delivery receipt after the scoped native save operation succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentSourceExportInput {
    CharacterSource { character_id: String },
    ContentPackage { import_id: String },
}

/// Bounded restart discovery request for completed package exports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListCompletedContentPackageExportsInput {
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportContentPackageInput {
    pub import_id: String,
}

impl From<ExportContentPackageInput> for ContentSourceExportInput {
    fn from(value: ExportContentPackageInput) -> Self {
        Self::ContentPackage {
            import_id: value.import_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSourceExportKindDto {
    CharacterCardV3,
    CharxPackage,
    LorepiaPackage,
}

/// Safe pre-save metadata. It contains no source path or bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentSourceExportDescriptorDto {
    pub kind: ContentSourceExportKindDto,
    pub source_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub suggested_file_name: String,
}

/// Safe evidence returned only after native delivery succeeds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentSourceExportReceiptDto {
    pub kind: ContentSourceExportKindDto,
    pub source_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub file_name: String,
}

impl ContentSourceExportReceiptDto {
    /// Constructs a receipt from the native-selected delivered file name.
    /// The pre-save suggestion is never re-labelled as an actual delivery.
    pub fn from_delivered_file_name(
        descriptor: &ContentSourceExportDescriptorDto,
        file_name: String,
    ) -> ShellResult<Self> {
        validate_package_identifier("export_source_id", &descriptor.source_id)?;
        validate_sha256("export_sha256", &descriptor.sha256)?;
        validate_nonzero_javascript_safe_integer(
            "content source export size",
            descriptor.size_bytes,
        )?;
        validate_export_suggested_name(
            "suggested export file name",
            &descriptor.suggested_file_name,
        )?;
        validate_export_receipt_file_name("delivered export file name", &file_name)?;
        let receipt = Self {
            kind: descriptor.kind,
            source_id: descriptor.source_id.clone(),
            sha256: descriptor.sha256.clone(),
            size_bytes: descriptor.size_bytes,
            file_name,
        };
        validate_serialized("content source export receipt", &receipt)?;
        Ok(receipt)
    }
}

/// Verified project-owned source for a scoped native save operation.
///
/// This wrapper implements neither `Serialize` nor `Clone`, and its debug
/// output contains no source path.
pub struct PreparedContentSourceExport {
    inner: CorePreparedContentSourceExport,
    descriptor: ContentSourceExportDescriptorDto,
}

impl PreparedContentSourceExport {
    pub const fn descriptor(&self) -> &ContentSourceExportDescriptorDto {
        &self.descriptor
    }

    #[doc(hidden)]
    pub fn source_path(&self) -> &Path {
        self.inner.source_path()
    }
}

impl fmt::Debug for PreparedContentSourceExport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedContentSourceExport")
            .field("inner", &"[REDACTED]")
            .field("descriptor", &self.descriptor)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageImportStatusDto {
    Inspected,
    AwaitingReview,
    Approved,
    Committing,
    Completed,
    Failed,
    Discarded,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageComponentKindDto {
    PromptPreset,
    MemoryProfile,
    KnowledgeBook,
    TransformSet,
    InteractionRuleSet,
    ContentModule,
    AssetIndex,
    RawExtension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageComponentDispositionDto {
    Importable,
    Unsupported,
    Quarantined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageIssueSeverityDto {
    Warning,
    Blocker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageRedistributionStatusDto {
    Allowed,
    DeniedByManifest,
    LicenseUnclear,
    ProvenanceIncomplete,
    ValidationBlocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageCapabilityDto {
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

/// The only package capabilities a caller may explicitly approve.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovableContentPackageCapabilityDto {
    Transforms,
    DeclarativeInteractions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageCapabilitySupportDto {
    Supported,
    Unsupported,
    ApprovalRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageManifestReviewDto {
    pub package_id: String,
    pub name: String,
    pub version: String,
    pub author: Option<String>,
    pub license: String,
    pub redistribution_allowed: bool,
    pub required_app_version: Option<String>,
    pub required_capabilities: Vec<ContentPackageCapabilityDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageComponentReviewDto {
    pub id: String,
    pub kind: ContentPackageComponentKindDto,
    pub disposition: ContentPackageComponentDispositionDto,
    pub dependency_ids: Vec<String>,
    pub conflict_ids: Vec<String>,
    pub required_capabilities: Vec<ContentPackageCapabilityDto>,
    pub asset_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageIssueDto {
    pub severity: ContentPackageIssueSeverityDto,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageCapabilityDecisionDto {
    pub capability: ContentPackageCapabilityDto,
    pub support: ContentPackageCapabilitySupportDto,
    pub approved: bool,
    pub reason: String,
}

/// Bounded review of an immutable package inspection.
///
/// Logical archive paths, component digests, raw manifests, signatures,
/// provenance documents, and host paths are deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageInspectionReviewDto {
    pub import_id: String,
    pub revision: u64,
    pub manifest: ContentPackageManifestReviewDto,
    pub source_size_bytes: u64,
    pub total_uncompressed_size_bytes: u64,
    pub components: Vec<ContentPackageComponentReviewDto>,
    pub asset_count: u32,
    pub issues: Vec<ContentPackageIssueDto>,
    pub local_import_allowed: bool,
    pub redistribution_status: ContentPackageRedistributionStatusDto,
    pub package_plan_hash: String,
    pub review_sha256: String,
    pub capability_review_sha256: String,
    pub capability_decisions: Vec<ContentPackageCapabilityDecisionDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageNormalizationEvidenceDto {
    pub component_id: String,
    pub object_id: String,
    pub field: String,
    pub before: bool,
    pub after: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageTargetDispositionDto {
    Create,
    Update,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentPackageTargetDocumentKindDto {
    PromptPreset,
    KnowledgeBook,
    MemoryProfile,
    TransformSet,
    InteractionRuleSet,
    ContentModule,
    CharacterContent,
}

/// Bounded, body-free projection of one normalized document's reviewed target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageTargetReviewDocumentDto {
    pub source_component_id: String,
    pub component_document_ordinal: u32,
    pub document_index: u32,
    pub document_kind: ContentPackageTargetDocumentKindDto,
    pub target_object_id: String,
    pub disposition: ContentPackageTargetDispositionDto,
    pub expected_target_revision_id: Option<String>,
    pub expected_target_state_revision: Option<u64>,
    pub source_component_sha256: String,
    pub document_sha256: String,
}

/// Exact immutable target-review snapshot returned by Core after selection.
///
/// Raw document bodies, source paths, and storage payloads are deliberately
/// absent from this serializable contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageTargetReviewDto {
    pub target_review_sha256: String,
    pub documents: Vec<ContentPackageTargetReviewDocumentDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfirmedContentPackageUpdateTargetDto {
    pub source_component_id: String,
    pub component_document_ordinal: u32,
    pub target_object_id: String,
    pub expected_target_revision_id: String,
    pub expected_target_state_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageSelectionReviewDto {
    pub content_selection_plan_hash: String,
    pub import_plan_sha256: String,
    pub normalization_evidence_sha256: String,
    pub normalization_evidence: Vec<PackageNormalizationEvidenceDto>,
    pub target_review: ContentPackageTargetReviewDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageApprovalReviewDto {
    pub approval_sha256: String,
    pub approval_id: String,
    pub enabled_component_ids: Vec<String>,
    pub approved_capabilities: Vec<ApprovableContentPackageCapabilityDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageImportReviewDto {
    pub import_id: String,
    pub package_id: String,
    pub status: ContentPackageImportStatusDto,
    pub revision: u64,
    pub package_plan_hash: String,
    pub review_sha256: String,
    pub capability_review_sha256: String,
    pub selected_component_ids: Vec<String>,
    pub selection: Option<ContentPackageSelectionReviewDto>,
    pub approval: Option<ContentPackageApprovalReviewDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageWorkspaceDto {
    pub inspection: ContentPackageInspectionReviewDto,
    pub lifecycle: ContentPackageImportReviewDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReopenContentPackageImportInput {
    pub import_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListPendingContentPackageImportsInput {
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectContentPackageImportInput {
    pub import_id: String,
    pub expected_revision: u64,
    pub expected_package_plan_hash: String,
    pub expected_review_sha256: String,
    pub expected_capability_review_sha256: String,
    pub selected_component_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectContentPackageImportReceiptDto {
    pub import_id: String,
    pub status: ContentPackageImportStatusDto,
    pub revision: u64,
    pub package_plan_hash: String,
    pub review_sha256: String,
    pub capability_review_sha256: String,
    pub selected_component_ids: Vec<String>,
    pub selection: ContentPackageSelectionReviewDto,
    pub required_capabilities: Vec<ContentPackageCapabilityDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveContentPackageImportInput {
    pub import_id: String,
    pub expected_revision: u64,
    pub expected_package_plan_hash: String,
    pub expected_content_selection_plan_hash: String,
    pub expected_review_sha256: String,
    pub expected_import_plan_sha256: String,
    pub expected_capability_review_sha256: String,
    pub expected_normalization_evidence_sha256: String,
    pub expected_target_review_sha256: String,
    pub approval_id: String,
    pub enable_component_ids: Vec<String>,
    pub approved_capabilities: Vec<ApprovableContentPackageCapabilityDto>,
    pub confirmed_update_targets: Vec<ConfirmedContentPackageUpdateTargetDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveContentPackageImportReceiptDto {
    pub import_id: String,
    pub status: ContentPackageImportStatusDto,
    pub revision: u64,
    pub package_plan_hash: String,
    pub content_selection_plan_hash: String,
    pub review_sha256: String,
    pub import_plan_sha256: String,
    pub capability_review_sha256: String,
    pub normalization_evidence_sha256: String,
    pub normalization_evidence: Vec<PackageNormalizationEvidenceDto>,
    pub target_review: ContentPackageTargetReviewDto,
    pub approval_sha256: String,
    pub approval_id: String,
    pub enabled_component_ids: Vec<String>,
    pub approved_capabilities: Vec<ApprovableContentPackageCapabilityDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitContentPackageImportInput {
    pub import_id: String,
    pub expected_revision: u64,
    pub expected_package_plan_hash: String,
    pub expected_content_selection_plan_hash: String,
    pub expected_review_sha256: String,
    pub expected_import_plan_sha256: String,
    pub expected_approval_sha256: String,
    pub expected_capability_review_sha256: String,
    pub expected_normalization_evidence_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitContentPackageImportReceiptDto {
    pub import_id: String,
    pub package_id: String,
    pub status: ContentPackageImportStatusDto,
    pub revision: u64,
    pub committed_document_ids: Vec<String>,
    pub asset_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscardContentPackageImportInput {
    pub import_id: String,
    pub expected_revision: u64,
    pub expected_review_sha256: String,
    pub expected_import_plan_sha256: Option<String>,
    pub expected_capability_review_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentPackageImportSummaryDto {
    pub import_id: String,
    pub package_id: String,
    pub status: ContentPackageImportStatusDto,
    pub revision: u64,
    pub selected_component_ids: Vec<String>,
    pub failure_code: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl ShellApi {
    /// Prepares a verified source for the Rust/native save adapter.
    ///
    /// The returned wrapper is not serializable. Native code must deliver its
    /// `source_path()` through the scoped platform operation, then construct a
    /// [`ContentSourceExportReceiptDto`] from the actual delivered file name.
    pub fn prepare_content_source_export(
        &self,
        input: ContentSourceExportInput,
    ) -> ShellResult<PreparedContentSourceExport> {
        validate_serialized("content source export request", &input)?;
        let (selector, expected_source_id) = match input {
            ContentSourceExportInput::CharacterSource { character_id } => {
                validate_package_identifier("character_id", &character_id)?;
                (
                    ContentSourceExportSelector::CharacterSource {
                        character_id: character_id.clone(),
                    },
                    character_id,
                )
            }
            ContentSourceExportInput::ContentPackage { import_id } => {
                validate_package_identifier("import_id", &import_id)?;
                (
                    ContentSourceExportSelector::ContentPackage {
                        import_id: import_id.clone(),
                    },
                    import_id,
                )
            }
        };
        let inner = self
            .core
            .prepare_content_source_export(&selector)
            .map_err(ShellError::from)?;
        let descriptor = project_export_descriptor(inner.descriptor())?;
        if descriptor.source_id != expected_source_id {
            return Err(storage_corrupted(
                "prepared content source export differs from its exact selector",
            ));
        }
        Ok(PreparedContentSourceExport { inner, descriptor })
    }

    /// Lists safe descriptors for completed package sources after restart.
    ///
    /// Core revalidates every completed state, parser identity, and exact CAS
    /// object before this method projects the all-or-nothing bounded snapshot.
    pub fn list_completed_content_package_exports(
        &self,
        input: ListCompletedContentPackageExportsInput,
    ) -> ShellResult<Vec<ContentSourceExportDescriptorDto>> {
        validate_completed_package_export_limit(input.limit)?;
        validate_serialized("completed content package export list request", &input)?;
        let descriptors = self
            .core
            .list_completed_content_package_export_descriptors(input.limit)
            .map_err(ShellError::from)?;
        let projected = project_completed_package_export_descriptors(descriptors, input.limit)?;
        validate_serialized("completed content package export list", &projected)?;
        Ok(projected)
    }

    pub fn prepare_content_package_export(
        &self,
        input: ExportContentPackageInput,
    ) -> ShellResult<PreparedContentSourceExport> {
        self.prepare_content_source_export(input.into())
    }

    /// Rust-only native path ingress. No serialized request can construct the
    /// path wrapper used here.
    #[doc(hidden)]
    pub fn inspect_content_package_import(
        &self,
        staged_file: &StagedImportFile,
    ) -> ShellResult<ContentPackageInspectionReviewDto> {
        self.core
            .inspect_content_package_import(staged_file.as_path())
            .map_err(ShellError::from)
            .and_then(project_inspection)
    }

    pub fn reopen_content_package_import(
        &self,
        input: ReopenContentPackageImportInput,
    ) -> ShellResult<ContentPackageWorkspaceDto> {
        validate_package_identifier("import_id", &input.import_id)?;
        validate_serialized("content package reopen request", &input)?;
        let lifecycle = self
            .core
            .get_content_package_import_review(&input.import_id)
            .map_err(ShellError::from)?;
        let inspection = self
            .core
            .get_content_package_import_inspection(&input.import_id)
            .map_err(ShellError::from)?;
        if lifecycle.revision != inspection.revision
            || lifecycle.import_id != inspection.import_id
            || lifecycle.package_id != inspection.review.manifest.package_id
            || lifecycle.package_plan_hash != inspection.inspection.plan_hash
            || lifecycle.review_sha256 != inspection.review.review_sha256
            || lifecycle.capability_review_sha256 != inspection.capability_review_sha256
        {
            return Err(storage_corrupted(
                "content package import changed while its review was reopened",
            ));
        }
        let workspace = ContentPackageWorkspaceDto {
            inspection: project_inspection(inspection)?,
            lifecycle: project_import_review(lifecycle)?,
        };
        validate_serialized("content package workspace", &workspace)?;
        Ok(workspace)
    }

    pub fn list_pending_content_package_imports(
        &self,
        input: ListPendingContentPackageImportsInput,
    ) -> ShellResult<Vec<ContentPackageImportReviewDto>> {
        if !(1..=MAX_PENDING_CONTENT_PACKAGE_IMPORTS).contains(&input.limit) {
            return Err(invalid_package(
                "pending content package import limit is outside the supported bound",
            ));
        }
        validate_serialized("pending content package request", &input)?;
        let reviews = self
            .core
            .list_pending_content_package_import_reviews(input.limit)
            .map_err(ShellError::from)?
            .into_iter()
            .map(project_import_review)
            .collect::<ShellResult<Vec<_>>>()?;
        validate_serialized("pending content package reviews", &reviews)?;
        Ok(reviews)
    }

    pub fn select_content_package_import(
        &self,
        input: SelectContentPackageImportInput,
    ) -> ShellResult<SelectContentPackageImportReceiptDto> {
        validate_package_identifier("import_id", &input.import_id)?;
        validate_next_javascript_revision("content package selection", input.expected_revision)?;
        validate_canonical_identifier_list(
            "selected_component_ids",
            &input.selected_component_ids,
            MAX_CONTENT_PACKAGE_COMPONENTS,
        )?;
        let review_sha256 = parse_sha256("expected_review_sha256", &input.expected_review_sha256)?;
        validate_sha256(
            "expected_package_plan_hash",
            &input.expected_package_plan_hash,
        )?;
        validate_sha256(
            "expected_capability_review_sha256",
            &input.expected_capability_review_sha256,
        )?;
        validate_serialized("content package selection request", &input)?;
        let receipt = self
            .core
            .select_content_package_import(
                &input.import_id,
                &ContentPackageSelectionRequest {
                    expected_revision: input.expected_revision,
                    expected_package_plan_hash: input.expected_package_plan_hash.clone(),
                    expected_review_sha256: review_sha256,
                    expected_capability_review_sha256: input
                        .expected_capability_review_sha256
                        .clone(),
                    selected_component_ids: input.selected_component_ids.clone(),
                },
            )
            .map_err(ShellError::from)?;

        let expected_revision = input.expected_revision + 1;
        if receipt.import.id != input.import_id
            || receipt.import.status != PackageImportStatus::AwaitingReview
            || receipt.import.revision != expected_revision
            || receipt.import.selected_component_ids != input.selected_component_ids
            || receipt.content_selection.package_plan_hash != input.expected_package_plan_hash
            || receipt.content_selection.selected_component_ids != input.selected_component_ids
            || receipt.import_plan.review_sha256.as_str() != input.expected_review_sha256
        {
            return Err(storage_corrupted(
                "content package selection receipt diverges from its exact request",
            ));
        }
        validate_sha256(
            "content_selection_plan_hash",
            &receipt.content_selection.selection_plan_hash,
        )?;
        validate_sha256(
            "import_plan_sha256",
            receipt.import_plan.plan_sha256.as_str(),
        )?;
        validate_sha256(
            "normalization_evidence_sha256",
            &receipt.normalization_evidence_sha256,
        )?;
        let target_review =
            project_target_review(receipt.target_review, &input.selected_component_ids)?;
        let required_capabilities = receipt.import_plan.required_capabilities;
        validate_unique_ordered_values("required_capabilities", &required_capabilities, 18)?;
        let projected = SelectContentPackageImportReceiptDto {
            import_id: receipt.import.id,
            status: receipt.import.status.into(),
            revision: receipt.import.revision,
            package_plan_hash: receipt.content_selection.package_plan_hash,
            review_sha256: receipt.import_plan.review_sha256.as_str().to_owned(),
            capability_review_sha256: input.expected_capability_review_sha256,
            selected_component_ids: receipt.content_selection.selected_component_ids,
            selection: ContentPackageSelectionReviewDto {
                content_selection_plan_hash: receipt.content_selection.selection_plan_hash,
                import_plan_sha256: receipt.import_plan.plan_sha256.as_str().to_owned(),
                normalization_evidence_sha256: receipt.normalization_evidence_sha256,
                normalization_evidence: project_normalization_evidence(
                    receipt.normalization_evidence,
                )?,
                target_review,
            },
            required_capabilities: required_capabilities.into_iter().map(Into::into).collect(),
        };
        validate_serialized("content package selection receipt", &projected)?;
        Ok(projected)
    }

    pub fn approve_content_package_import(
        &self,
        input: ApproveContentPackageImportInput,
    ) -> ShellResult<ApproveContentPackageImportReceiptDto> {
        let (review_sha256, import_plan_sha256, confirmed_update_targets) =
            validate_content_package_approval_input(&input)?;

        // Validate the complete body-free projection before the durable CAS.
        // Core recreates and revalidates the same target snapshot in its
        // transaction, so this read-only check closes post-commit IPC bounds
        // without weakening durable authority.
        let current = self
            .core
            .get_content_package_import_review(&input.import_id)
            .map_err(ShellError::from)?;
        let preflight = validate_approval_preflight(&input, &current)?;
        let approved_capabilities = input
            .approved_capabilities
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<_>>();
        let receipt = self
            .core
            .approve_content_package_import(
                &input.import_id,
                &ContentPackageApprovalRequest {
                    expected_revision: input.expected_revision,
                    expected_package_plan_hash: input.expected_package_plan_hash.clone(),
                    expected_content_selection_plan_hash: input
                        .expected_content_selection_plan_hash
                        .clone(),
                    expected_review_sha256: review_sha256,
                    expected_import_plan_sha256: import_plan_sha256,
                    expected_capability_review_sha256: input
                        .expected_capability_review_sha256
                        .clone(),
                    expected_normalization_evidence_sha256: input
                        .expected_normalization_evidence_sha256
                        .clone(),
                    expected_target_review_sha256: input.expected_target_review_sha256.clone(),
                    confirmed_update_targets,
                    approval_id: input.approval_id.clone(),
                    enable_component_ids: input.enable_component_ids.clone(),
                    approved_capabilities,
                },
            )
            .map_err(ShellError::from)?;
        let enabled_component_ids = receipt
            .approved_plan
            .components
            .iter()
            .filter(|component| component.enabled)
            .map(|component| component.component.id.clone())
            .collect::<Vec<_>>();
        let target_review =
            project_target_review(receipt.target_review, &current.selected_component_ids)?;
        let normalization_evidence =
            project_normalization_evidence(receipt.normalization_evidence)?;
        if receipt.import.id != input.import_id
            || receipt.import.status != PackageImportStatus::Approved
            || receipt.import.revision != input.expected_revision + 1
            || receipt.approved_plan.review_sha256.as_str() != input.expected_review_sha256
            || receipt.approved_plan.plan_sha256.as_str() != input.expected_import_plan_sha256
            || receipt.approved_plan.approval_id != input.approval_id
            || receipt.normalization_evidence_sha256 != input.expected_normalization_evidence_sha256
            || target_review.target_review_sha256 != input.expected_target_review_sha256
            || target_review != preflight.target_review
            || normalization_evidence != preflight.normalization_evidence
            || enabled_component_ids != input.enable_component_ids
        {
            return Err(storage_corrupted(
                "content package approval receipt diverges from its exact approval input",
            ));
        }
        validate_sha256(
            "approval_sha256",
            receipt.approved_plan.approval_sha256.as_str(),
        )?;
        let projected = ApproveContentPackageImportReceiptDto {
            import_id: receipt.import.id,
            status: receipt.import.status.into(),
            revision: receipt.import.revision,
            package_plan_hash: input.expected_package_plan_hash.clone(),
            content_selection_plan_hash: input.expected_content_selection_plan_hash.clone(),
            review_sha256: receipt.approved_plan.review_sha256.as_str().to_owned(),
            import_plan_sha256: receipt.approved_plan.plan_sha256.as_str().to_owned(),
            capability_review_sha256: input.expected_capability_review_sha256.clone(),
            normalization_evidence_sha256: receipt.normalization_evidence_sha256,
            normalization_evidence,
            target_review,
            approval_sha256: receipt.approved_plan.approval_sha256.as_str().to_owned(),
            approval_id: receipt.approved_plan.approval_id,
            enabled_component_ids,
            approved_capabilities: input.approved_capabilities.clone(),
        };
        validate_serialized("content package approval receipt", &projected)?;
        Ok(projected)
    }

    pub fn commit_content_package_import(
        &self,
        input: CommitContentPackageImportInput,
    ) -> ShellResult<CommitContentPackageImportReceiptDto> {
        validate_package_identifier("import_id", &input.import_id)?;
        validate_next_javascript_revision("content package commit", input.expected_revision)?;
        validate_all_review_hashes(
            &input.expected_package_plan_hash,
            &input.expected_content_selection_plan_hash,
            &input.expected_capability_review_sha256,
            &input.expected_normalization_evidence_sha256,
        )?;
        validate_sha256("expected_approval_sha256", &input.expected_approval_sha256)?;
        validate_serialized("content package commit request", &input)?;
        validate_commit_receipt_preflight(self, &input)?;
        let request = ContentPackageCommitRequest {
            expected_revision: input.expected_revision,
            expected_package_plan_hash: input.expected_package_plan_hash.clone(),
            expected_content_selection_plan_hash: input
                .expected_content_selection_plan_hash
                .clone(),
            expected_review_sha256: parse_sha256(
                "expected_review_sha256",
                &input.expected_review_sha256,
            )?,
            expected_import_plan_sha256: parse_sha256(
                "expected_import_plan_sha256",
                &input.expected_import_plan_sha256,
            )?,
            expected_approval_sha256: parse_sha256(
                "expected_approval_sha256",
                &input.expected_approval_sha256,
            )?,
            expected_capability_review_sha256: input.expected_capability_review_sha256.clone(),
            expected_normalization_evidence_sha256: input
                .expected_normalization_evidence_sha256
                .clone(),
        };
        let receipt = self
            .core
            .commit_content_package_import(&input.import_id, &request)
            .map_err(ShellError::from)?;
        let committed_document_ids = receipt.committed_document_ids;
        validate_unique_identifier_list(
            "committed_document_ids",
            &committed_document_ids,
            MAX_CONTENT_PACKAGE_TARGET_DOCUMENTS,
        )?;
        let asset_ids = receipt
            .asset_ids
            .into_iter()
            .map(|id| id.0)
            .collect::<Vec<_>>();
        validate_unique_identifier_list("asset_ids", &asset_ids, MAX_CONTENT_PACKAGE_COMPONENTS)?;
        if receipt.import.id != input.import_id
            || receipt.import.status != PackageImportStatus::Completed
            || receipt.import.revision != input.expected_revision + 1
        {
            return Err(storage_corrupted(
                "content package commit receipt diverges from its exact request",
            ));
        }
        validate_package_identifier("package_id", &receipt.import.package_id.0)?;
        let projected = CommitContentPackageImportReceiptDto {
            import_id: receipt.import.id,
            package_id: receipt.import.package_id.0,
            status: receipt.import.status.into(),
            revision: receipt.import.revision,
            committed_document_ids,
            asset_ids,
        };
        validate_serialized("content package commit receipt", &projected)?;
        Ok(projected)
    }

    pub fn discard_content_package_import(
        &self,
        input: DiscardContentPackageImportInput,
    ) -> ShellResult<ContentPackageImportSummaryDto> {
        validate_package_identifier("import_id", &input.import_id)?;
        validate_next_javascript_revision("content package discard", input.expected_revision)?;
        validate_sha256(
            "expected_capability_review_sha256",
            &input.expected_capability_review_sha256,
        )?;
        validate_serialized("content package discard request", &input)?;
        let current = self
            .core
            .get_content_package_import_review(&input.import_id)
            .map_err(ShellError::from)?;
        project_import_review(current)?;
        let request = ContentPackageDiscardRequest {
            expected_revision: input.expected_revision,
            expected_review_sha256: parse_sha256(
                "expected_review_sha256",
                &input.expected_review_sha256,
            )?,
            expected_import_plan_sha256: input
                .expected_import_plan_sha256
                .as_deref()
                .map(|value| parse_sha256("expected_import_plan_sha256", value))
                .transpose()?,
            expected_capability_review_sha256: input.expected_capability_review_sha256.clone(),
        };
        let record = self
            .core
            .discard_content_package_import(&input.import_id, &request)
            .map_err(ShellError::from)?;
        if record.id != input.import_id
            || record.status != PackageImportStatus::Discarded
            || record.revision != input.expected_revision + 1
        {
            return Err(storage_corrupted(
                "content package discard receipt diverges from its exact request",
            ));
        }
        project_import_summary(record)
    }
}

impl From<PackageImportStatus> for ContentPackageImportStatusDto {
    fn from(value: PackageImportStatus) -> Self {
        match value {
            PackageImportStatus::Inspected => Self::Inspected,
            PackageImportStatus::AwaitingReview => Self::AwaitingReview,
            PackageImportStatus::Approved => Self::Approved,
            PackageImportStatus::Committing => Self::Committing,
            PackageImportStatus::Completed => Self::Completed,
            PackageImportStatus::Failed => Self::Failed,
            PackageImportStatus::Discarded => Self::Discarded,
            PackageImportStatus::RolledBack => Self::RolledBack,
        }
    }
}

impl From<CoreExportKind> for ContentSourceExportKindDto {
    fn from(value: CoreExportKind) -> Self {
        match value {
            CoreExportKind::CharacterCardV3 => Self::CharacterCardV3,
            CoreExportKind::CharxPackage => Self::CharxPackage,
            CoreExportKind::LorepiaPackage => Self::LorepiaPackage,
        }
    }
}

impl From<PackageComponentKind> for ContentPackageComponentKindDto {
    fn from(value: PackageComponentKind) -> Self {
        match value {
            PackageComponentKind::PromptPreset => Self::PromptPreset,
            PackageComponentKind::MemoryProfile => Self::MemoryProfile,
            PackageComponentKind::KnowledgeBook => Self::KnowledgeBook,
            PackageComponentKind::TransformSet => Self::TransformSet,
            PackageComponentKind::InteractionRuleSet => Self::InteractionRuleSet,
            PackageComponentKind::ContentModule => Self::ContentModule,
            PackageComponentKind::AssetIndex => Self::AssetIndex,
            PackageComponentKind::RawExtension => Self::RawExtension,
        }
    }
}

impl From<PackageComponentDisposition> for ContentPackageComponentDispositionDto {
    fn from(value: PackageComponentDisposition) -> Self {
        match value {
            PackageComponentDisposition::Importable => Self::Importable,
            PackageComponentDisposition::Unsupported => Self::Unsupported,
            PackageComponentDisposition::Quarantined => Self::Quarantined,
        }
    }
}

impl From<PackageDocumentTargetDisposition> for ContentPackageTargetDispositionDto {
    fn from(value: PackageDocumentTargetDisposition) -> Self {
        match value {
            PackageDocumentTargetDisposition::Create => Self::Create,
            PackageDocumentTargetDisposition::Update => Self::Update,
        }
    }
}

impl From<PackageIssueSeverity> for ContentPackageIssueSeverityDto {
    fn from(value: PackageIssueSeverity) -> Self {
        match value {
            PackageIssueSeverity::Warning => Self::Warning,
            PackageIssueSeverity::Blocker => Self::Blocker,
        }
    }
}

impl From<RedistributionStatus> for ContentPackageRedistributionStatusDto {
    fn from(value: RedistributionStatus) -> Self {
        match value {
            RedistributionStatus::Allowed => Self::Allowed,
            RedistributionStatus::DeniedByManifest => Self::DeniedByManifest,
            RedistributionStatus::LicenseUnclear => Self::LicenseUnclear,
            RedistributionStatus::ProvenanceIncomplete => Self::ProvenanceIncomplete,
            RedistributionStatus::ValidationBlocked => Self::ValidationBlocked,
        }
    }
}

impl From<ContentCapability> for ContentPackageCapabilityDto {
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

impl From<PackageCapability> for ContentPackageCapabilityDto {
    fn from(value: PackageCapability) -> Self {
        match value {
            PackageCapability::PromptFragments => Self::PromptFragments,
            PackageCapability::Knowledge => Self::Knowledge,
            PackageCapability::Variables => Self::Variables,
            PackageCapability::Transforms => Self::Transforms,
            PackageCapability::DeclarativeInteractions => Self::DeclarativeInteractions,
            PackageCapability::ImageAssets => Self::ImageAssets,
            PackageCapability::AudioAssets => Self::AudioAssets,
            PackageCapability::VideoAssets => Self::VideoAssets,
            PackageCapability::AttachmentAssets => Self::AttachmentAssets,
            PackageCapability::HighRiskAssets => Self::HighRiskAssets,
            PackageCapability::ExternalUrls => Self::ExternalUrls,
            PackageCapability::Html => Self::Html,
            PackageCapability::Script => Self::Script,
            PackageCapability::NativeCode => Self::NativeCode,
            PackageCapability::Network => Self::Network,
            PackageCapability::Filesystem => Self::Filesystem,
            PackageCapability::Shell => Self::Shell,
            PackageCapability::Credentials => Self::Credentials,
        }
    }
}

impl From<PackageCapabilitySupport> for ContentPackageCapabilitySupportDto {
    fn from(value: PackageCapabilitySupport) -> Self {
        match value {
            PackageCapabilitySupport::Supported => Self::Supported,
            PackageCapabilitySupport::Unsupported => Self::Unsupported,
            PackageCapabilitySupport::ApprovalRequired => Self::ApprovalRequired,
        }
    }
}

impl From<ApprovableContentPackageCapabilityDto> for PackageCapability {
    fn from(value: ApprovableContentPackageCapabilityDto) -> Self {
        match value {
            ApprovableContentPackageCapabilityDto::Transforms => Self::Transforms,
            ApprovableContentPackageCapabilityDto::DeclarativeInteractions => {
                Self::DeclarativeInteractions
            }
        }
    }
}

fn project_export_descriptor(
    value: &CoreExportDescriptor,
) -> ShellResult<ContentSourceExportDescriptorDto> {
    validate_package_identifier("export_source_id", &value.source_id)
        .map_err(|_| storage_corrupted("Core returned an invalid export source identifier"))?;
    validate_sha256("export_sha256", &value.sha256)
        .map_err(|_| storage_corrupted("Core returned a non-canonical export source digest"))?;
    validate_nonzero_javascript_safe_integer("content source export size", value.size_bytes)
        .map_err(|_| {
            storage_corrupted("Core returned an invalid export size for the JavaScript boundary")
        })?;
    validate_export_suggested_name("suggested export file name", &value.suggested_file_name)
        .map_err(|_| storage_corrupted("Core returned an unsafe suggested export file name"))?;
    let projected = ContentSourceExportDescriptorDto {
        kind: value.kind.into(),
        source_id: value.source_id.clone(),
        sha256: value.sha256.clone(),
        size_bytes: value.size_bytes,
        suggested_file_name: value.suggested_file_name.clone(),
    };
    validate_serialized("content source export descriptor", &projected)?;
    Ok(projected)
}

fn project_completed_package_export_descriptors(
    values: Vec<CoreExportDescriptor>,
    requested_limit: u32,
) -> ShellResult<Vec<ContentSourceExportDescriptorDto>> {
    validate_completed_package_export_limit(requested_limit)?;
    if values.len() > usize::try_from(requested_limit).unwrap_or(usize::MAX)
        || values.len() > usize::try_from(MAX_COMPLETED_PACKAGE_EXPORTS).unwrap_or(usize::MAX)
    {
        return Err(storage_corrupted(
            "Core returned more completed package exports than requested",
        ));
    }
    let mut source_ids = BTreeSet::new();
    values
        .iter()
        .map(|value| {
            if value.kind != CoreExportKind::LorepiaPackage {
                return Err(storage_corrupted(
                    "Core returned a non-package completed export descriptor",
                ));
            }
            if !source_ids.insert(value.source_id.as_str()) {
                return Err(storage_corrupted(
                    "Core returned duplicate completed package export identities",
                ));
            }
            project_export_descriptor(value)
        })
        .collect()
}

fn validate_completed_package_export_limit(limit: u32) -> ShellResult<()> {
    if !(1..=MAX_COMPLETED_PACKAGE_EXPORTS).contains(&limit) {
        return Err(invalid_package(format!(
            "completed package export limit must be between 1 and {MAX_COMPLETED_PACKAGE_EXPORTS}"
        )));
    }
    Ok(())
}

fn validate_export_suggested_name(field: &str, value: &str) -> ShellResult<()> {
    let valid_bytes = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    let stem = value.split('.').next().unwrap_or_default();
    let reserved_windows_stem = matches!(
        stem.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    );
    if !valid_bytes
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains("..")
        || reserved_windows_stem
    {
        return Err(invalid_package(format!(
            "{field} is not a bounded cross-platform file name"
        )));
    }
    Ok(())
}

fn validate_export_receipt_file_name(field: &str, value: &str) -> ShellResult<()> {
    const MAX_RECEIPT_NAME_CHARACTERS: usize = 255;
    const MAX_RECEIPT_NAME_BYTES: usize = MAX_RECEIPT_NAME_CHARACTERS * 4;
    if value.trim().is_empty()
        || matches!(value, "." | "..")
        || value.len() > MAX_RECEIPT_NAME_BYTES
        || value.chars().count() > MAX_RECEIPT_NAME_CHARACTERS
        || value
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
        || Path::new(value).file_name().and_then(|name| name.to_str()) != Some(value)
    {
        return Err(invalid_package(format!(
            "{field} is not a bounded native display name"
        )));
    }
    Ok(())
}

fn validate_content_package_approval_input(
    input: &ApproveContentPackageImportInput,
) -> ShellResult<(
    Sha256Digest,
    Sha256Digest,
    Vec<PackageUpdateTargetConfirmation>,
)> {
    validate_package_identifier("import_id", &input.import_id)?;
    validate_package_identifier("approval_id", &input.approval_id)?;
    validate_next_javascript_revision("content package approval", input.expected_revision)?;
    validate_canonical_identifier_list(
        "enable_component_ids",
        &input.enable_component_ids,
        MAX_CONTENT_PACKAGE_COMPONENTS,
    )?;
    validate_input_unique_ordered_values("approved_capabilities", &input.approved_capabilities, 2)?;
    validate_all_review_hashes(
        &input.expected_package_plan_hash,
        &input.expected_content_selection_plan_hash,
        &input.expected_capability_review_sha256,
        &input.expected_normalization_evidence_sha256,
    )?;
    validate_sha256(
        "expected_target_review_sha256",
        &input.expected_target_review_sha256,
    )?;
    let review_sha256 = parse_sha256("expected_review_sha256", &input.expected_review_sha256)?;
    let import_plan_sha256 = parse_sha256(
        "expected_import_plan_sha256",
        &input.expected_import_plan_sha256,
    )?;
    let confirmed_update_targets =
        project_update_confirmations_for_core(&input.confirmed_update_targets)?;
    validate_serialized("content package approval request", input)?;
    Ok((review_sha256, import_plan_sha256, confirmed_update_targets))
}

fn project_inspection(
    value: ContentPackageImportInspection,
) -> ShellResult<ContentPackageInspectionReviewDto> {
    value
        .review
        .verify()
        .map_err(|_| storage_corrupted("Core returned an invalid content package review"))?;
    validate_package_identifier("import_id", &value.import_id)?;
    validate_package_identifier("package_id", &value.review.manifest.package_id.0)?;
    validate_javascript_safe_integer("content package revision", value.revision)?;
    validate_javascript_safe_integer("content package source size", value.inspection.source_size)?;
    validate_javascript_safe_integer(
        "content package uncompressed size",
        value.inspection.total_uncompressed_size,
    )?;
    validate_sha256("package_plan_hash", &value.inspection.plan_hash)?;
    validate_sha256("review_sha256", value.review.review_sha256.as_str())?;
    validate_sha256("capability_review_sha256", &value.capability_review_sha256)?;
    validate_unique_ordered_values(
        "capability decisions",
        &value
            .capability_review
            .decisions
            .iter()
            .map(|decision| decision.capability)
            .collect::<Vec<_>>(),
        18,
    )?;
    let capability_decisions = value
        .capability_review
        .decisions
        .into_iter()
        .map(project_capability_decision)
        .collect();
    let review_sha256 = value.review.review_sha256.as_str().to_owned();
    let review = project_package_review(value.review)?;
    let projected = ContentPackageInspectionReviewDto {
        import_id: value.import_id,
        revision: value.revision,
        manifest: review.manifest,
        source_size_bytes: value.inspection.source_size,
        total_uncompressed_size_bytes: value.inspection.total_uncompressed_size,
        components: review.components,
        asset_count: review.asset_count,
        issues: review.issues,
        local_import_allowed: review.local_import_allowed,
        redistribution_status: review.redistribution_status,
        package_plan_hash: value.inspection.plan_hash,
        review_sha256,
        capability_review_sha256: value.capability_review_sha256,
        capability_decisions,
    };
    validate_serialized("content package inspection review", &projected)?;
    Ok(projected)
}

struct ProjectedPackageReview {
    manifest: ContentPackageManifestReviewDto,
    components: Vec<ContentPackageComponentReviewDto>,
    asset_count: u32,
    issues: Vec<ContentPackageIssueDto>,
    local_import_allowed: bool,
    redistribution_status: ContentPackageRedistributionStatusDto,
}

fn project_package_review(value: PackageReview) -> ShellResult<ProjectedPackageReview> {
    validate_count(
        "content package components",
        value.components.len(),
        MAX_CONTENT_PACKAGE_COMPONENTS,
    )?;
    validate_count(
        "content package issues",
        value.issues.len(),
        MAX_CONTENT_PACKAGE_ISSUES,
    )?;
    validate_count(
        "content package assets",
        value.assets.len(),
        MAX_CONTENT_PACKAGE_COMPONENTS,
    )?;
    let manifest = project_package_manifest(value.manifest)?;
    let mut component_ids = BTreeSet::new();
    let components = value
        .components
        .into_iter()
        .map(|component| {
            validate_package_identifier("package_component_id", &component.id)?;
            if !component_ids.insert(component.id.clone()) {
                return Err(storage_corrupted(
                    "content package review contains a duplicate component",
                ));
            }
            validate_unique_identifier_list(
                "package_dependency_ids",
                &component.dependencies,
                MAX_CONTENT_PACKAGE_COMPONENTS,
            )?;
            validate_unique_identifier_list(
                "package_conflict_ids",
                &component.conflicts_with,
                MAX_CONTENT_PACKAGE_COMPONENTS,
            )?;
            validate_unique_ordered_values(
                "component required capabilities",
                &component.required_capabilities,
                10,
            )?;
            let asset_count = u32::try_from(component.asset_ids.len()).map_err(|_| {
                storage_corrupted("content package component asset count exceeds u32")
            })?;
            Ok(ContentPackageComponentReviewDto {
                id: component.id,
                kind: component.kind.into(),
                disposition: component.disposition.into(),
                dependency_ids: component.dependencies,
                conflict_ids: component.conflicts_with,
                required_capabilities: component
                    .required_capabilities
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                asset_count,
            })
        })
        .collect::<ShellResult<Vec<_>>>()?;
    let issues = value
        .issues
        .into_iter()
        .map(|issue| {
            validate_package_identifier("package_issue_code", &issue.code)?;
            Ok(ContentPackageIssueDto {
                severity: issue.severity.into(),
                code: issue.code,
                message: issue.message,
            })
        })
        .collect::<ShellResult<Vec<_>>>()?;
    let asset_count = u32::try_from(value.assets.len())
        .map_err(|_| storage_corrupted("content package asset count exceeds u32"))?;
    Ok(ProjectedPackageReview {
        manifest,
        components,
        asset_count,
        issues,
        local_import_allowed: value.local_import_allowed,
        redistribution_status: value.redistribution_status.into(),
    })
}

fn project_package_manifest(
    manifest: PackageManifest,
) -> ShellResult<ContentPackageManifestReviewDto> {
    validate_unique_ordered_values(
        "manifest required capabilities",
        &manifest.required_capabilities,
        10,
    )?;
    Ok(ContentPackageManifestReviewDto {
        package_id: manifest.package_id.0,
        name: manifest.name,
        version: manifest.version,
        author: manifest.author,
        license: manifest.license,
        redistribution_allowed: manifest.redistribution_allowed,
        required_app_version: manifest.required_app_version,
        required_capabilities: manifest
            .required_capabilities
            .into_iter()
            .map(Into::into)
            .collect(),
    })
}

fn project_capability_decision(
    value: PackageCapabilityDecision,
) -> ContentPackageCapabilityDecisionDto {
    ContentPackageCapabilityDecisionDto {
        capability: value.capability.into(),
        support: value.support.into(),
        approved: value.approved,
        reason: value.reason,
    }
}

fn project_import_review(
    value: ContentPackageImportReview,
) -> ShellResult<ContentPackageImportReviewDto> {
    validate_package_identifier("import_id", &value.import_id)?;
    validate_package_identifier("package_id", &value.package_id.0)?;
    validate_javascript_safe_integer("content package import revision", value.revision)?;
    validate_sha256("package_plan_hash", &value.package_plan_hash)?;
    validate_sha256("review_sha256", value.review_sha256.as_str())?;
    validate_sha256("capability_review_sha256", &value.capability_review_sha256)?;
    validate_canonical_identifier_list(
        "selected_component_ids",
        &value.selected_component_ids,
        MAX_CONTENT_PACKAGE_COMPONENTS,
    )?;
    let selected_component_ids = value.selected_component_ids.clone();
    let selection = value
        .selection
        .map(|selection| -> ShellResult<_> {
            validate_sha256(
                "content_selection_plan_hash",
                &selection.content_selection_plan_hash,
            )?;
            validate_sha256("import_plan_sha256", selection.import_plan_sha256.as_str())?;
            validate_sha256(
                "normalization_evidence_sha256",
                &selection.normalization_evidence_sha256,
            )?;
            let target_review =
                project_target_review(selection.target_review, &selected_component_ids)?;
            let projected = ContentPackageSelectionReviewDto {
                content_selection_plan_hash: selection.content_selection_plan_hash,
                import_plan_sha256: selection.import_plan_sha256.as_str().to_owned(),
                normalization_evidence_sha256: selection.normalization_evidence_sha256,
                normalization_evidence: project_normalization_evidence(
                    selection.normalization_evidence,
                )?,
                target_review,
            };
            validate_serialized("content package selection review", &projected)?;
            Ok(projected)
        })
        .transpose()?;
    let approval = value
        .approval
        .map(|approval| -> ShellResult<_> {
            validate_sha256("approval_sha256", approval.approval_sha256.as_str())?;
            validate_package_identifier("approval_id", &approval.approval_id)?;
            validate_canonical_identifier_list(
                "enabled_component_ids",
                &approval.enabled_component_ids,
                MAX_CONTENT_PACKAGE_COMPONENTS,
            )?;
            if approval
                .enabled_component_ids
                .iter()
                .any(|id| selected_component_ids.binary_search(id).is_err())
            {
                return Err(storage_corrupted(
                    "content package approval enables an unselected component",
                ));
            }
            validate_unique_ordered_values(
                "approved_capabilities",
                &approval.approved_capabilities,
                2,
            )?;
            Ok(ContentPackageApprovalReviewDto {
                approval_sha256: approval.approval_sha256.as_str().to_owned(),
                approval_id: approval.approval_id,
                enabled_component_ids: approval.enabled_component_ids,
                approved_capabilities: approval
                    .approved_capabilities
                    .into_iter()
                    .map(project_approved_capability)
                    .collect::<ShellResult<Vec<_>>>()?,
            })
        })
        .transpose()?;
    validate_projected_import_review_status(
        value.status,
        selection.is_some(),
        approval.is_some(),
        selected_component_ids.is_empty(),
    )?;
    let projected = ContentPackageImportReviewDto {
        import_id: value.import_id,
        package_id: value.package_id.0,
        status: value.status.into(),
        revision: value.revision,
        package_plan_hash: value.package_plan_hash,
        review_sha256: value.review_sha256.as_str().to_owned(),
        capability_review_sha256: value.capability_review_sha256,
        selected_component_ids,
        selection,
        approval,
    };
    validate_serialized("content package import review", &projected)?;
    Ok(projected)
}

fn validate_projected_import_review_status(
    status: PackageImportStatus,
    has_selection: bool,
    has_approval: bool,
    selected_components_empty: bool,
) -> ShellResult<()> {
    if status == PackageImportStatus::Inspected
        && (has_selection || has_approval || !selected_components_empty)
    {
        return Err(storage_corrupted(
            "inspected content package unexpectedly contains selection authority",
        ));
    }
    if status == PackageImportStatus::AwaitingReview && (!has_selection || has_approval) {
        return Err(storage_corrupted(
            "content package awaiting review has inconsistent durable authority",
        ));
    }
    if matches!(
        status,
        PackageImportStatus::Approved
            | PackageImportStatus::Committing
            | PackageImportStatus::Completed
            | PackageImportStatus::RolledBack
    ) && (!has_selection || !has_approval)
    {
        return Err(storage_corrupted(
            "approved content package is missing reviewed authority",
        ));
    }
    Ok(())
}

fn project_approved_capability(
    value: PackageCapability,
) -> ShellResult<ApprovableContentPackageCapabilityDto> {
    match value {
        PackageCapability::Transforms => Ok(ApprovableContentPackageCapabilityDto::Transforms),
        PackageCapability::DeclarativeInteractions => {
            Ok(ApprovableContentPackageCapabilityDto::DeclarativeInteractions)
        }
        _ => Err(storage_corrupted(
            "stored package approval contains a non-approvable capability",
        )),
    }
}

fn project_target_review(
    value: PackageImportTargetReview,
    selected_component_ids: &[String],
) -> ShellResult<ContentPackageTargetReviewDto> {
    value.verify().map_err(|_| {
        storage_corrupted("Core returned an invalid immutable package target review")
    })?;
    validate_sha256("target_review_sha256", &value.target_review_sha256)?;
    validate_count(
        "package target-review documents",
        value.documents.len(),
        MAX_CONTENT_PACKAGE_TARGET_DOCUMENTS,
    )?;
    validate_canonical_identifier_list(
        "selected_component_ids",
        selected_component_ids,
        MAX_CONTENT_PACKAGE_COMPONENTS,
    )?;
    let selected = selected_component_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut component_documents = BTreeSet::new();
    let mut target_ids = BTreeSet::new();
    let mut ordinals_by_component = BTreeMap::<String, Vec<u32>>::new();
    let mut projected = Vec::with_capacity(value.documents.len());
    for (expected_index, document) in value.documents.into_iter().enumerate() {
        validate_package_identifier(
            "target_review_source_component_id",
            &document.source_component_id,
        )?;
        validate_package_identifier("target_review_object_id", &document.target_object_id)?;
        validate_sha256(
            "target_review_source_component_sha256",
            &document.source_component_sha256,
        )?;
        validate_sha256("target_review_document_sha256", &document.document_sha256)?;
        if !selected.contains(document.source_component_id.as_str()) {
            return Err(storage_corrupted(
                "package target review names an unselected component",
            ));
        }
        if usize::try_from(document.document_index) != Ok(expected_index) {
            return Err(storage_corrupted(
                "package target-review document indices are not contiguous",
            ));
        }
        if !component_documents.insert((
            document.source_component_id.clone(),
            document.component_document_ordinal,
        )) {
            return Err(storage_corrupted(
                "package target review contains a duplicate component document",
            ));
        }
        if !target_ids.insert(document.target_object_id.clone()) {
            return Err(storage_corrupted(
                "package target review contains a duplicate target object",
            ));
        }
        ordinals_by_component
            .entry(document.source_component_id.clone())
            .or_default()
            .push(document.component_document_ordinal);
        validate_target_disposition(&document)?;
        projected.push(ContentPackageTargetReviewDocumentDto {
            source_component_id: document.source_component_id,
            component_document_ordinal: document.component_document_ordinal,
            document_index: document.document_index,
            document_kind: project_target_document_kind(&document.document_kind)?,
            target_object_id: document.target_object_id,
            disposition: document.disposition.into(),
            expected_target_revision_id: document.expected_target_revision_id,
            expected_target_state_revision: document.expected_target_state_revision,
            source_component_sha256: document.source_component_sha256,
            document_sha256: document.document_sha256,
        });
    }
    for ordinals in ordinals_by_component.values_mut() {
        ordinals.sort_unstable();
        if ordinals
            .iter()
            .enumerate()
            .any(|(expected, actual)| usize::try_from(*actual) != Ok(expected))
        {
            return Err(storage_corrupted(
                "package target-review component ordinals are not contiguous",
            ));
        }
    }
    let projected = ContentPackageTargetReviewDto {
        target_review_sha256: value.target_review_sha256,
        documents: projected,
    };
    validate_serialized("content package target review", &projected)?;
    Ok(projected)
}

fn project_target_document_kind(value: &str) -> ShellResult<ContentPackageTargetDocumentKindDto> {
    match value {
        "prompt_preset" => Ok(ContentPackageTargetDocumentKindDto::PromptPreset),
        "knowledge_book" => Ok(ContentPackageTargetDocumentKindDto::KnowledgeBook),
        "memory_profile" => Ok(ContentPackageTargetDocumentKindDto::MemoryProfile),
        "transform_set" => Ok(ContentPackageTargetDocumentKindDto::TransformSet),
        "interaction_rule_set" => Ok(ContentPackageTargetDocumentKindDto::InteractionRuleSet),
        "content_module" => Ok(ContentPackageTargetDocumentKindDto::ContentModule),
        "character_content" => Ok(ContentPackageTargetDocumentKindDto::CharacterContent),
        _ => Err(storage_corrupted(
            "package target review contains an unsupported document kind",
        )),
    }
}

fn validate_target_disposition(value: &PackageDocumentTargetReview) -> ShellResult<()> {
    match value.disposition {
        PackageDocumentTargetDisposition::Create
            if value.expected_target_revision_id.is_none()
                && value.expected_target_state_revision.is_none() =>
        {
            Ok(())
        }
        PackageDocumentTargetDisposition::Update => {
            let revision_id = value
                .expected_target_revision_id
                .as_deref()
                .ok_or_else(|| {
                    storage_corrupted("package update target is missing its reviewed revision")
                })?;
            let state_revision = value.expected_target_state_revision.ok_or_else(|| {
                storage_corrupted("package update target is missing its reviewed state revision")
            })?;
            validate_package_identifier("target_review_revision_id", revision_id)?;
            if state_revision == 0 {
                return Err(storage_corrupted(
                    "package update target has a zero state revision",
                ));
            }
            validate_javascript_safe_integer("package update target state", state_revision)
        }
        PackageDocumentTargetDisposition::Create => Err(storage_corrupted(
            "package target disposition differs from its reviewed expectations",
        )),
    }
}

fn project_normalization_evidence(
    values: Vec<PackageNormalizationEvidence>,
) -> ShellResult<Vec<PackageNormalizationEvidenceDto>> {
    validate_count(
        "package normalization evidence",
        values.len(),
        MAX_CONTENT_PACKAGE_NORMALIZATION_EVIDENCE,
    )?;
    validate_unique_ordered_values(
        "package normalization evidence",
        &values,
        MAX_CONTENT_PACKAGE_NORMALIZATION_EVIDENCE,
    )?;
    let projected = values
        .into_iter()
        .map(|value| {
            validate_package_identifier("normalization_component_id", &value.component_id)?;
            validate_package_identifier("normalization_object_id", &value.object_id)?;
            validate_package_identifier("normalization_field", &value.field)?;
            if !matches!(value.field.as_str(), "enabled" | "imported_enabled")
                || value.after
                || value.reason.trim().is_empty()
                || value.reason.len() > 512
                || value.reason.chars().any(char::is_control)
            {
                return Err(storage_corrupted(
                    "package normalization evidence has an invalid safety projection",
                ));
            }
            Ok(PackageNormalizationEvidenceDto {
                component_id: value.component_id,
                object_id: value.object_id,
                field: value.field,
                before: value.before,
                after: value.after,
                reason: value.reason,
            })
        })
        .collect::<ShellResult<Vec<_>>>()?;
    validate_serialized("package normalization evidence", &projected)?;
    Ok(projected)
}

fn project_import_summary(
    value: PackageImportRecord,
) -> ShellResult<ContentPackageImportSummaryDto> {
    validate_package_identifier("import_id", &value.id)?;
    validate_package_identifier("package_id", &value.package_id.0)?;
    validate_javascript_safe_integer("content package import revision", value.revision)?;
    validate_canonical_identifier_list(
        "selected_component_ids",
        &value.selected_component_ids,
        MAX_CONTENT_PACKAGE_COMPONENTS,
    )?;
    if let Some(failure_code) = &value.failure_code {
        validate_package_identifier("failure_code", failure_code)?;
    }
    let projected = ContentPackageImportSummaryDto {
        import_id: value.id,
        package_id: value.package_id.0,
        status: value.status.into(),
        revision: value.revision,
        selected_component_ids: value.selected_component_ids,
        failure_code: value.failure_code,
        created_at: value.created_at.to_rfc3339(),
        updated_at: value.updated_at.to_rfc3339(),
    };
    validate_serialized("content package import summary", &projected)?;
    Ok(projected)
}

#[derive(Debug)]
struct PackageApprovalPreflight {
    target_review: ContentPackageTargetReviewDto,
    normalization_evidence: Vec<PackageNormalizationEvidenceDto>,
}

fn validate_approval_preflight(
    input: &ApproveContentPackageImportInput,
    current: &ContentPackageImportReview,
) -> ShellResult<PackageApprovalPreflight> {
    let projected = project_import_review(current.clone())?;
    let is_initial = current.status == PackageImportStatus::AwaitingReview
        && current.revision == input.expected_revision;
    let is_exact_replay = current.status == PackageImportStatus::Approved
        && current.revision == input.expected_revision + 1;
    if !is_initial && !is_exact_replay {
        return Err(invalid_package(
            "content package approval does not match the current durable revision",
        ));
    }
    if projected.import_id != input.import_id
        || projected.package_plan_hash != input.expected_package_plan_hash
        || projected.review_sha256 != input.expected_review_sha256
        || projected.capability_review_sha256 != input.expected_capability_review_sha256
    {
        return Err(invalid_package(
            "content package approval review hashes are stale",
        ));
    }
    let selection = projected.selection.as_ref().ok_or_else(|| {
        storage_corrupted("content package approval has no durable target review")
    })?;
    if selection.content_selection_plan_hash != input.expected_content_selection_plan_hash
        || selection.import_plan_sha256 != input.expected_import_plan_sha256
        || selection.normalization_evidence_sha256 != input.expected_normalization_evidence_sha256
        || selection.target_review.target_review_sha256 != input.expected_target_review_sha256
    {
        return Err(invalid_package(
            "content package approval selection hashes are stale",
        ));
    }
    if input
        .enable_component_ids
        .iter()
        .any(|id| projected.selected_component_ids.binary_search(id).is_err())
    {
        return Err(invalid_package(
            "content package approval enables an unselected component",
        ));
    }
    let expected_confirmations = expected_update_confirmations(&selection.target_review.documents)?;
    if expected_confirmations != input.confirmed_update_targets {
        return Err(invalid_package(
            "content package update confirmations differ from the immutable target review",
        ));
    }
    let approval_sha256 = if is_exact_replay {
        let approval = projected
            .approval
            .as_ref()
            .ok_or_else(|| storage_corrupted("approved content package has no durable approval"))?;
        if approval.approval_id != input.approval_id
            || approval.enabled_component_ids != input.enable_component_ids
            || approval.approved_capabilities != input.approved_capabilities
        {
            return Err(invalid_package(
                "content package approval replay differs from its durable approval",
            ));
        }
        approval.approval_sha256.clone()
    } else {
        "0".repeat(64)
    };
    let receipt_preflight = ApproveContentPackageImportReceiptDto {
        import_id: input.import_id.clone(),
        status: ContentPackageImportStatusDto::Approved,
        revision: input.expected_revision + 1,
        package_plan_hash: input.expected_package_plan_hash.clone(),
        content_selection_plan_hash: input.expected_content_selection_plan_hash.clone(),
        review_sha256: input.expected_review_sha256.clone(),
        import_plan_sha256: input.expected_import_plan_sha256.clone(),
        capability_review_sha256: input.expected_capability_review_sha256.clone(),
        normalization_evidence_sha256: input.expected_normalization_evidence_sha256.clone(),
        normalization_evidence: selection.normalization_evidence.clone(),
        target_review: selection.target_review.clone(),
        approval_sha256,
        approval_id: input.approval_id.clone(),
        enabled_component_ids: input.enable_component_ids.clone(),
        approved_capabilities: input.approved_capabilities.clone(),
    };
    validate_serialized(
        "content package approval receipt preflight",
        &receipt_preflight,
    )?;
    Ok(PackageApprovalPreflight {
        target_review: selection.target_review.clone(),
        normalization_evidence: selection.normalization_evidence.clone(),
    })
}

fn validate_commit_receipt_preflight(
    shell: &ShellApi,
    input: &CommitContentPackageImportInput,
) -> ShellResult<()> {
    let current = shell
        .core
        .get_content_package_import_review(&input.import_id)
        .map_err(ShellError::from)?;
    let review = project_import_review(current.clone())?;
    let is_initial = current.status == PackageImportStatus::Approved
        && current.revision == input.expected_revision;
    let is_exact_replay = current.status == PackageImportStatus::Completed
        && current.revision == input.expected_revision + 1;
    if !is_initial && !is_exact_replay {
        return Err(invalid_package(
            "content package commit does not match the current durable revision",
        ));
    }
    let selection = review.selection.as_ref().ok_or_else(|| {
        storage_corrupted("content package commit is missing its reviewed selection")
    })?;
    let approval = review.approval.as_ref().ok_or_else(|| {
        storage_corrupted("content package commit is missing its immutable approval")
    })?;
    if review.import_id != input.import_id
        || review.package_plan_hash != input.expected_package_plan_hash
        || review.review_sha256 != input.expected_review_sha256
        || review.capability_review_sha256 != input.expected_capability_review_sha256
        || selection.content_selection_plan_hash != input.expected_content_selection_plan_hash
        || selection.import_plan_sha256 != input.expected_import_plan_sha256
        || selection.normalization_evidence_sha256 != input.expected_normalization_evidence_sha256
        || approval.approval_sha256 != input.expected_approval_sha256
    {
        return Err(invalid_package("content package commit hashes are stale"));
    }
    let inspection = shell
        .core
        .get_content_package_import_inspection(&input.import_id)
        .map_err(ShellError::from)
        .and_then(project_inspection)?;
    if inspection.import_id != review.import_id || inspection.revision != current.revision {
        return Err(storage_corrupted(
            "content package commit preflight changed while it was reconstructed",
        ));
    }
    let committed_document_ids = selection
        .target_review
        .documents
        .iter()
        .map(|document| document.target_object_id.clone())
        .collect();
    let asset_ids = (0..inspection.asset_count)
        .map(|_| "x".repeat(256))
        .collect();
    validate_serialized(
        "content package commit receipt preflight",
        &CommitContentPackageImportReceiptDto {
            import_id: review.import_id,
            package_id: review.package_id,
            status: ContentPackageImportStatusDto::Completed,
            revision: input.expected_revision + 1,
            committed_document_ids,
            asset_ids,
        },
    )
}

fn project_update_confirmations_for_core(
    values: &[ConfirmedContentPackageUpdateTargetDto],
) -> ShellResult<Vec<PackageUpdateTargetConfirmation>> {
    validate_count(
        "content package update confirmations",
        values.len(),
        MAX_CONTENT_PACKAGE_TARGET_DOCUMENTS,
    )?;
    let mut component_documents = BTreeSet::new();
    let mut targets = BTreeSet::new();
    values
        .iter()
        .map(|value| {
            validate_package_identifier(
                "update_confirmation_source_component_id",
                &value.source_component_id,
            )?;
            validate_package_identifier(
                "update_confirmation_target_object_id",
                &value.target_object_id,
            )?;
            validate_package_identifier(
                "update_confirmation_target_revision_id",
                &value.expected_target_revision_id,
            )?;
            if value.expected_target_state_revision == 0 {
                return Err(invalid_package(
                    "content package update confirmation state revision must be positive",
                ));
            }
            validate_javascript_safe_integer(
                "content package update confirmation state revision",
                value.expected_target_state_revision,
            )?;
            if !component_documents.insert((
                value.source_component_id.as_str(),
                value.component_document_ordinal,
            )) || !targets.insert(value.target_object_id.as_str())
            {
                return Err(invalid_package(
                    "content package update confirmations contain a duplicate target",
                ));
            }
            Ok(PackageUpdateTargetConfirmation {
                source_component_id: value.source_component_id.clone(),
                component_document_ordinal: value.component_document_ordinal,
                target_object_id: value.target_object_id.clone(),
                expected_target_revision_id: value.expected_target_revision_id.clone(),
                expected_target_state_revision: value.expected_target_state_revision,
            })
        })
        .collect()
}

fn expected_update_confirmations(
    documents: &[ContentPackageTargetReviewDocumentDto],
) -> ShellResult<Vec<ConfirmedContentPackageUpdateTargetDto>> {
    documents
        .iter()
        .filter(|document| document.disposition == ContentPackageTargetDispositionDto::Update)
        .map(|document| {
            Ok(ConfirmedContentPackageUpdateTargetDto {
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
        .collect()
}

fn validate_package_identifier(field: &str, value: &str) -> ShellResult<()> {
    validate_identifier(field, value)?;
    if value.len() > 256 || value.trim() != value {
        return Err(invalid_package(format!(
            "{field} is not a canonical package identifier"
        )));
    }
    Ok(())
}

fn validate_unique_identifier_list(
    field: &str,
    values: &[String],
    maximum: usize,
) -> ShellResult<()> {
    validate_count(field, values.len(), maximum)?;
    let mut unique = BTreeSet::new();
    for value in values {
        validate_package_identifier(field, value)?;
        if !unique.insert(value.as_str()) {
            return Err(invalid_package(format!("{field} must be unique")));
        }
    }
    Ok(())
}

fn validate_canonical_identifier_list(
    field: &str,
    values: &[String],
    maximum: usize,
) -> ShellResult<()> {
    validate_unique_identifier_list(field, values, maximum)?;
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid_package(format!(
            "{field} must be unique and canonically ordered"
        )));
    }
    Ok(())
}

fn validate_unique_ordered_values<T: Ord>(
    field: &str,
    values: &[T],
    maximum: usize,
) -> ShellResult<()> {
    validate_count(field, values.len(), maximum)?;
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(storage_corrupted(format!(
            "{field} must be unique and canonically ordered"
        )));
    }
    Ok(())
}

fn validate_input_unique_ordered_values<T: Ord>(
    field: &str,
    values: &[T],
    maximum: usize,
) -> ShellResult<()> {
    validate_count(field, values.len(), maximum)?;
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid_package(format!(
            "{field} must be unique and canonically ordered"
        )));
    }
    Ok(())
}

fn validate_count(field: &str, actual: usize, maximum: usize) -> ShellResult<()> {
    if actual > maximum {
        return Err(invalid_package(format!(
            "{field} exceeds the supported {maximum}-item bound"
        )));
    }
    Ok(())
}

fn validate_javascript_safe_integer(field: &str, value: u64) -> ShellResult<()> {
    if value > MAX_JAVASCRIPT_SAFE_INTEGER {
        return Err(invalid_package(format!(
            "{field} exceeds the exact JavaScript integer boundary"
        )));
    }
    Ok(())
}

fn validate_nonzero_javascript_safe_integer(field: &str, value: u64) -> ShellResult<()> {
    if value == 0 {
        return Err(invalid_package(format!(
            "{field} must be greater than zero"
        )));
    }
    validate_javascript_safe_integer(field, value)
}

fn validate_next_javascript_revision(field: &str, value: u64) -> ShellResult<()> {
    validate_javascript_safe_integer(field, value)?;
    if value >= MAX_JAVASCRIPT_SAFE_INTEGER {
        return Err(invalid_package(format!(
            "{field} result would exceed the exact JavaScript integer boundary"
        )));
    }
    Ok(())
}

fn parse_sha256(field: &str, value: &str) -> ShellResult<Sha256Digest> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_package(format!(
            "{field} must be an exact lowercase SHA-256 digest"
        )));
    }
    Sha256Digest::parse(value.to_owned())
        .map_err(|_| invalid_package(format!("{field} must be a canonical SHA-256 digest")))
}

fn validate_sha256(field: &str, value: &str) -> ShellResult<()> {
    parse_sha256(field, value).map(|_| ())
}

fn validate_all_review_hashes(
    package_plan_hash: &str,
    content_selection_plan_hash: &str,
    capability_review_sha256: &str,
    normalization_evidence_sha256: &str,
) -> ShellResult<()> {
    validate_sha256("expected_package_plan_hash", package_plan_hash)?;
    validate_sha256(
        "expected_content_selection_plan_hash",
        content_selection_plan_hash,
    )?;
    validate_sha256(
        "expected_capability_review_sha256",
        capability_review_sha256,
    )?;
    validate_sha256(
        "expected_normalization_evidence_sha256",
        normalization_evidence_sha256,
    )
}

fn validate_serialized<T: Serialize>(kind: &str, value: &T) -> ShellResult<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| invalid_package(format!("failed to encode {kind}: {error}")))?;
    if bytes.len() > MAX_CONTENT_PACKAGE_IPC_BYTES {
        return Err(invalid_package(format!(
            "{kind} exceeds the bounded IPC document size"
        )));
    }
    Ok(())
}

fn invalid_package(message: impl Into<String>) -> ShellError {
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
    use std::{fs::File, io::Write};

    use lorepia_core::CoreConfig;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::*;

    fn write_quarantined_executable_module_package(path: &Path) {
        let module = serde_json::to_vec(&json!({
            "id": "shell.quarantined-module",
            "name": "Quarantined executable module",
            "version": "1.0.0",
            "schema_version": 1,
            "prompt_fragments": [],
            "knowledge_book_ids": [],
            "control_specs": [],
            "transform_set_ids": [],
            "interaction_rule_set_ids": [],
            "asset_ids": [],
            "required_capabilities": [],
            "script": "fetch('https://invalid.example')",
            "html": "<script>not executable</script>"
        }))
        .expect("encode hostile module fixture");
        let module_sha256 = format!("{:x}", Sha256::digest(&module));
        let manifest = serde_json::to_vec(&json!({
            "format": "lorepia_content_package",
            "format_version": 1,
            "package_id": "dev.lorepia.shell-quarantined-module-test",
            "name": "Quarantined module fixture",
            "version": "1.0.0",
            "author": "LorePia tests",
            "license": "MIT",
            "redistribution_allowed": true,
            "required_app_version": "0.1.0",
            "required_capabilities": ["content_modules"],
            "dependencies": [],
            "conflicts": [],
            "content_hashes": {"modules/hostile.json": module_sha256},
            "content_types": {"modules/hostile.json": "application/json"},
            "components": [{
                "id": "hostile-module",
                "path": "modules/hostile.json",
                "kind": "content_module"
            }],
            "signature": null
        }))
        .expect("encode hostile package manifest");
        let file = File::create(path).expect("create hostile module package");
        let mut archive = ZipWriter::new(file);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .unix_permissions(0o644);
        archive
            .start_file("manifest.json", options)
            .expect("start manifest");
        archive.write_all(&manifest).expect("write manifest");
        archive
            .start_file("modules/hostile.json", options)
            .expect("start hostile module");
        archive.write_all(&module).expect("write hostile module");
        archive.finish().expect("finish hostile module package");
    }

    #[test]
    fn quarantined_script_and_html_module_never_enters_shell_runtime_authority() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("hostile-module.zip");
        write_quarantined_executable_module_package(&source);
        let shell = ShellApi::open(CoreConfig::new(data_root.path())).expect("open Shell");
        let inspection = shell
            .inspect_content_package_import(&StagedImportFile::new(&source))
            .expect("inspect hostile package without executing it");
        let component = inspection
            .components
            .iter()
            .find(|component| component.id == "hostile-module")
            .expect("hostile component review");
        assert_eq!(
            component.disposition,
            ContentPackageComponentDispositionDto::Quarantined
        );
        let selection = shell
            .select_content_package_import(SelectContentPackageImportInput {
                import_id: inspection.import_id,
                expected_revision: inspection.revision,
                expected_package_plan_hash: inspection.package_plan_hash,
                expected_review_sha256: inspection.review_sha256,
                expected_capability_review_sha256: inspection.capability_review_sha256,
                selected_component_ids: vec!["hostile-module".to_owned()],
            })
            .expect_err("quarantined executable component must never become selectable");
        assert_eq!(selection.code, crate::ShellErrorCode::UnsafeArchive);
        assert!(
            shell
                .list_content_modules()
                .expect("list runtime-eligible modules")
                .is_empty(),
            "a rejected executable component must create no module or activation candidate"
        );
    }

    fn valid_create_target_review() -> PackageImportTargetReview {
        PackageImportTargetReview {
            target_review_sha256:
                "c98593d549309f85eada32420fa5eeeff46a6d30745897490d21d32dd3bf1740".to_owned(),
            documents: vec![PackageDocumentTargetReview {
                source_component_id: "component-1".to_owned(),
                component_document_ordinal: 0,
                document_index: 0,
                document_kind: "prompt_preset".to_owned(),
                target_object_id: "object-1".to_owned(),
                disposition: PackageDocumentTargetDisposition::Create,
                expected_target_revision_id: None,
                expected_target_state_revision: None,
                source_component_sha256: "a".repeat(64),
                document_sha256: "b".repeat(64),
            }],
        }
    }

    #[test]
    fn serialized_package_inputs_have_no_path_or_raw_document_channel() {
        for invalid in [
            r#"{"import_id":"import-1","path":"/Users/private/package.zip"}"#,
            r#"{"import_id":"import-1","bytes":[1,2,3]}"#,
            r#"{"import_id":"import-1","document":{"kind":"prompt_preset"}}"#,
        ] {
            assert!(
                serde_json::from_str::<ReopenContentPackageImportInput>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn approval_requires_target_review_and_confirmation_echoes() {
        let missing_target_review = r#"{
            "import_id":"import-1",
            "expected_revision":2,
            "expected_package_plan_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "expected_content_selection_plan_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "expected_review_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "expected_import_plan_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "expected_capability_review_sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "expected_normalization_evidence_sha256":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "approval_id":"approval-1",
            "enable_component_ids":[],
            "approved_capabilities":[],
            "confirmed_update_targets":[]
        }"#;
        let missing_confirmations = r#"{
            "import_id":"import-1",
            "expected_revision":2,
            "expected_package_plan_hash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "expected_content_selection_plan_hash":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "expected_review_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            "expected_import_plan_sha256":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "expected_capability_review_sha256":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            "expected_normalization_evidence_sha256":"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "expected_target_review_sha256":"1111111111111111111111111111111111111111111111111111111111111111",
            "approval_id":"approval-1",
            "enable_component_ids":[],
            "approved_capabilities":[]
        }"#;
        assert!(
            serde_json::from_str::<ApproveContentPackageImportInput>(missing_target_review)
                .is_err()
        );
        assert!(
            serde_json::from_str::<ApproveContentPackageImportInput>(missing_confirmations)
                .is_err()
        );
    }

    #[test]
    fn target_review_projection_is_exact_bounded_and_body_free() {
        let projected =
            project_target_review(valid_create_target_review(), &["component-1".to_owned()])
                .expect("project target review");
        assert_eq!(projected.documents.len(), 1);
        assert_eq!(projected.documents[0].document_index, 0);
        assert_eq!(
            projected.documents[0].source_component_sha256,
            "a".repeat(64)
        );
        assert_eq!(
            projected.documents[0].document_kind,
            ContentPackageTargetDocumentKindDto::PromptPreset
        );
        let json = serde_json::to_string(&projected).expect("serialize target review");
        assert!(json.contains("\"document_index\":0"));
        assert!(json.contains("\"source_component_sha256\""));
        for forbidden in [
            "path",
            "bytes",
            "body",
            "normalized_document",
            "versioned_json",
        ] {
            assert!(!json.contains(forbidden), "{forbidden}");
        }
    }

    #[test]
    fn target_review_rejects_index_identity_membership_and_state_drift() {
        let mut wrong_index = valid_create_target_review();
        wrong_index.documents[0].document_index = 1;
        let error = project_target_review(wrong_index, &["component-1".to_owned()])
            .expect_err("non-contiguous index must fail");
        assert_eq!(error.code, crate::ShellErrorCode::StorageCorrupted);

        let error = project_target_review(
            valid_create_target_review(),
            &["different-component".to_owned()],
        )
        .expect_err("unselected component must fail");
        assert_eq!(error.code, crate::ShellErrorCode::StorageCorrupted);

        let mut unsafe_update = valid_create_target_review().documents.remove(0);
        unsafe_update.disposition = PackageDocumentTargetDisposition::Update;
        unsafe_update.expected_target_revision_id = Some("revision-1".to_owned());
        unsafe_update.expected_target_state_revision = Some(MAX_JAVASCRIPT_SAFE_INTEGER + 1);
        assert!(validate_target_disposition(&unsafe_update).is_err());
    }

    #[test]
    fn package_hashes_identifiers_lists_and_result_revision_are_canonical() {
        assert!(parse_sha256("hash", &"a".repeat(64)).is_ok());
        for invalid in ["A".repeat(64), "g".repeat(64), "a".repeat(63)] {
            let error = parse_sha256("hash", &invalid).expect_err("digest must fail");
            assert_eq!(error.code, crate::ShellErrorCode::InvalidInput);
        }
        assert!(validate_package_identifier("id", " id").is_err());
        assert!(validate_package_identifier("id", &"x".repeat(257)).is_err());
        assert!(
            validate_canonical_identifier_list("ids", &["b".to_owned(), "a".to_owned()], 2,)
                .is_err()
        );
        assert!(
            validate_next_javascript_revision("revision", MAX_JAVASCRIPT_SAFE_INTEGER).is_err()
        );
    }

    #[test]
    fn update_confirmations_preserve_exact_target_document_order() {
        let document =
            |component: &str, ordinal: u32, target: &str| ContentPackageTargetReviewDocumentDto {
                source_component_id: component.to_owned(),
                component_document_ordinal: ordinal,
                document_index: ordinal,
                document_kind: ContentPackageTargetDocumentKindDto::PromptPreset,
                target_object_id: target.to_owned(),
                disposition: ContentPackageTargetDispositionDto::Update,
                expected_target_revision_id: Some(format!("revision-{ordinal}")),
                expected_target_state_revision: Some(u64::from(ordinal) + 1),
                source_component_sha256: "a".repeat(64),
                document_sha256: "b".repeat(64),
            };
        let documents = vec![
            document("component-1", 0, "target-1"),
            document("component-1", 1, "target-2"),
        ];
        let expected = expected_update_confirmations(&documents).expect("confirmations");
        assert_eq!(expected[0].target_object_id, "target-1");
        assert_eq!(expected[1].target_object_id, "target-2");
        let mut reordered = expected.clone();
        reordered.reverse();
        assert_ne!(expected, reordered);
        assert!(project_update_confirmations_for_core(&expected).is_ok());
    }

    #[test]
    fn target_review_and_ipc_document_bounds_fail_closed() {
        assert!(
            validate_count(
                "target review",
                MAX_CONTENT_PACKAGE_TARGET_DOCUMENTS + 1,
                MAX_CONTENT_PACKAGE_TARGET_DOCUMENTS,
            )
            .is_err()
        );
        assert!(
            validate_serialized(
                "oversized package projection",
                &"x".repeat(MAX_CONTENT_PACKAGE_IPC_BYTES + 1),
            )
            .is_err()
        );
    }

    #[test]
    fn export_request_and_descriptor_have_no_path_or_byte_channel() {
        let credential_canary = "sk-synthetic-package-export-canary-4f91";
        let path_canary = "/Users/synthetic/private-origin-path-canary/package.zip";
        let request = ContentSourceExportInput::ContentPackage {
            import_id: "import-1".to_owned(),
        };
        let request_json = serde_json::to_string(&request).expect("serialize request");
        assert_eq!(
            request_json,
            r#"{"kind":"content_package","import_id":"import-1"}"#
        );
        assert!(
            serde_json::from_str::<ContentSourceExportInput>(
                r#"{"kind":"content_package","import_id":"import-1","path":"/private/source"}"#,
            )
            .is_err()
        );
        for rejected in [
            format!(
                r#"{{"kind":"content_package","import_id":"import-1","credential":"{credential_canary}"}}"#
            ),
            format!(
                r#"{{"kind":"content_package","import_id":"import-1","path":"{path_canary}"}}"#
            ),
        ] {
            let error = serde_json::from_str::<ContentSourceExportInput>(&rejected)
                .expect_err("credential and path fields must not enter the export contract");
            let rendered = format!("{error:?}");
            assert!(!rendered.contains(credential_canary));
            assert!(!rendered.contains(path_canary));
        }

        let descriptor = project_export_descriptor(&CoreExportDescriptor {
            kind: CoreExportKind::LorepiaPackage,
            source_id: "import-1".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 42,
            suggested_file_name: "lorepia-package.zip".to_owned(),
        })
        .expect("project descriptor");
        let descriptor_json = serde_json::to_string(&descriptor).expect("serialize descriptor");
        assert!(descriptor_json.contains("\"suggested_file_name\""));
        for forbidden in ["\"path\"", "\"bytes\"", "source_path", "source_bytes"] {
            assert!(!descriptor_json.contains(forbidden), "{forbidden}");
        }
        assert!(!descriptor_json.contains(credential_canary));
        assert!(!descriptor_json.contains(path_canary));
    }

    #[test]
    fn export_receipt_requires_the_actual_portable_delivery_name() {
        let credential_canary = "sk-synthetic-package-export-canary-4f91";
        let path_canary = "/Users/synthetic/private-origin-path-canary/package.zip";
        let descriptor = ContentSourceExportDescriptorDto {
            kind: ContentSourceExportKindDto::LorepiaPackage,
            source_id: "import-1".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 42,
            suggested_file_name: "suggested.zip".to_owned(),
        };
        let receipt = ContentSourceExportReceiptDto::from_delivered_file_name(
            &descriptor,
            "card:copy?.json".to_owned(),
        )
        .expect("receipt");
        assert_eq!(receipt.file_name, "card:copy?.json");
        let json = serde_json::to_string(&receipt).expect("serialize receipt");
        assert!(!json.contains("suggested"));
        assert!(!json.contains("path"));
        assert!(!json.contains(credential_canary));
        assert!(!json.contains(path_canary));
        for invalid in [
            "../actual.zip".to_owned(),
            "folder\\actual.zip".to_owned(),
            "\u{0000}actual.zip".to_owned(),
            "x".repeat(256),
        ] {
            assert!(
                ContentSourceExportReceiptDto::from_delivered_file_name(&descriptor, invalid)
                    .is_err()
            );
        }
        let mut empty_descriptor = descriptor;
        empty_descriptor.size_bytes = 0;
        assert!(
            ContentSourceExportReceiptDto::from_delivered_file_name(
                &empty_descriptor,
                "actual.zip".to_owned(),
            )
            .is_err()
        );
    }

    #[test]
    fn export_descriptor_rejects_noncanonical_hash_and_unsafe_size() {
        let descriptor = |sha256: String, size_bytes| CoreExportDescriptor {
            kind: CoreExportKind::LorepiaPackage,
            source_id: "import-1".to_owned(),
            sha256,
            size_bytes,
            suggested_file_name: "package.zip".to_owned(),
        };
        let error = project_export_descriptor(&descriptor("A".repeat(64), 1))
            .expect_err("uppercase hash must fail");
        assert_eq!(error.code, crate::ShellErrorCode::StorageCorrupted);
        let error = project_export_descriptor(&descriptor("a".repeat(64), 0))
            .expect_err("empty source size must fail");
        assert_eq!(error.code, crate::ShellErrorCode::StorageCorrupted);
        let error =
            project_export_descriptor(&descriptor("a".repeat(64), MAX_JAVASCRIPT_SAFE_INTEGER + 1))
                .expect_err("unsafe size must fail");
        assert_eq!(error.code, crate::ShellErrorCode::StorageCorrupted);
    }

    #[test]
    fn completed_package_export_catalog_is_bounded_ordered_and_body_free() {
        let descriptor = |source_id: &str| CoreExportDescriptor {
            kind: CoreExportKind::LorepiaPackage,
            source_id: source_id.to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 42,
            suggested_file_name: format!("{source_id}.zip"),
        };
        let projected = project_completed_package_export_descriptors(
            vec![descriptor("newer"), descriptor("older")],
            2,
        )
        .expect("project completed export catalog");
        assert_eq!(
            projected
                .iter()
                .map(|value| value.source_id.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
        let json = serde_json::to_string(&projected).expect("serialize completed export catalog");
        for forbidden in ["\"path\"", "\"bytes\"", "source_path", "inspection_json"] {
            assert!(!json.contains(forbidden), "{forbidden}");
        }
        assert!(
            serde_json::from_str::<ListCompletedContentPackageExportsInput>(
                r#"{"limit":100,"path":"/private/catalog"}"#,
            )
            .is_err()
        );
        for invalid_limit in [0, MAX_COMPLETED_PACKAGE_EXPORTS + 1] {
            assert!(validate_completed_package_export_limit(invalid_limit).is_err());
        }
        assert!(
            project_completed_package_export_descriptors(
                vec![descriptor("one"), descriptor("two")],
                1,
            )
            .is_err()
        );
        assert!(
            project_completed_package_export_descriptors(
                vec![descriptor("duplicate"), descriptor("duplicate")],
                2,
            )
            .is_err()
        );
        let mut wrong_kind = descriptor("character");
        wrong_kind.kind = CoreExportKind::CharacterCardV3;
        assert!(project_completed_package_export_descriptors(vec![wrong_kind], 1).is_err());
    }

    #[test]
    fn review_projection_type_has_no_raw_persistence_fields() {
        let field_names = serde_json::to_value(ContentPackageImportReviewDto {
            import_id: "import-1".to_owned(),
            package_id: "package-1".to_owned(),
            status: ContentPackageImportStatusDto::Inspected,
            revision: 1,
            package_plan_hash: "a".repeat(64),
            review_sha256: "b".repeat(64),
            capability_review_sha256: "c".repeat(64),
            selected_component_ids: Vec::new(),
            selection: None,
            approval: None,
        })
        .expect("serialize");
        let json = field_names.to_string();
        for forbidden in [
            "path",
            "bytes",
            "versioned_json",
            "signature",
            "provenance",
            "inspection_json",
            "selection_json",
        ] {
            assert!(!json.contains(forbidden), "{forbidden}");
        }
    }
}
