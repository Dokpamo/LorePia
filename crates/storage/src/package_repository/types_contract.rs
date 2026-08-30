//! Public package repository contracts.

use chrono::{DateTime, Utc};
use lorepia_domain::{
    AssetDescriptor, AssetId, ContentCapability, CoreError, CoreResult, PackageId, VersionedJson,
};
use lorepia_orchestration::PackageComponentKind;
use serde::{Deserialize, Serialize};

use crate::orchestration::PackageImportStatus;

use super::{package_import_target_review_sha256, validate_document_target_reviews};

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
    pub(super) const ALL: [Self; 18] = [
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

    pub(super) const fn as_str(self) -> &'static str {
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

    pub(super) const fn is_never_approvable(self) -> bool {
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

    pub(super) const fn required_support(self) -> PackageCapabilitySupport {
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
    pub(super) const fn as_str(self) -> &'static str {
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
