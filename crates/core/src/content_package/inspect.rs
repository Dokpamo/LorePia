use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use lorepia_content::{
    ContentCapability as InspectedContentCapability, ContentPackageComponentKind,
    ContentPackageComponentState, ContentPackageInspection, inspect_content_package,
};
use lorepia_domain::{
    AssetDescriptor, AssetId, AssetRole, AssetSource, AssetSourceKind, ContentCapability,
    CoreError, CoreErrorCode, CoreResult, ImportLimits, InspectionId, PackageContentHash,
    PackageId, PackageManifest, Provenance, Sha256Digest, SourceKind, VersionedJson,
};
use lorepia_orchestration::{
    ObservedPackageEntry, PackageComponentDescriptor, PackageComponentDisposition,
    PackageComponentKind, PackageInspectionSnapshot, PackageReview, PackageValidationPolicy,
    SignatureVerification, validate_package_snapshot,
};
use lorepia_storage::{
    PackageCapability, PackageCapabilityDecision, PackageCapabilityReview,
    PackageCapabilitySupport, PackageImportRecord, PackageImportStatus, PackageSourceRecord,
    package_capability_review_sha256,
};
use serde::{Deserialize, Serialize};
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
#[derive(Debug)]
pub(super) struct OwnedContentPackageSnapshot {
    pub(super) import_id: String,
    pub(super) path: PathBuf,
    pub(super) inspection: ContentPackageInspection,
    pub(super) review: PackageReview,
}

impl OwnedContentPackageSnapshot {
    pub(super) fn public_inspection(
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
    pub(super) fn inspection(&self) -> &ContentPackageInspection {
        &self.inspection
    }
    pub(super) fn review(&self) -> &PackageReview {
        &self.review
    }
    pub(super) fn discard(self, staging_dir: &Path) -> CoreResult<()> {
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
pub(super) fn package_capability_review(_review: &PackageReview) -> PackageCapabilityReview {
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
pub(super) fn package_json_error(error: serde_json::Error) -> CoreError {
    CoreError::invalid(format!("package review cannot be encoded: {error}"))
}
pub(super) fn with_cleanup_error(primary: CoreError, cleanup: CoreResult<()>) -> CoreError {
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
pub(super) fn with_two_cleanup_errors(
    primary: CoreError,
    first: CoreResult<()>,
    second: CoreResult<()>,
) -> CoreError {
    with_cleanup_error(with_cleanup_error(primary, first), second)
}
pub(super) fn stage_content_package(
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
pub(super) fn reopen_content_package(
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
pub(super) fn package_snapshot_path(staging_dir: &Path, import_id: &str) -> CoreResult<PathBuf> {
    validate_import_id(import_id)?;
    Ok(staging_dir.join(format!(
        "{PACKAGE_SNAPSHOT_PREFIX}{import_id}{PACKAGE_SNAPSHOT_SUFFIX}"
    )))
}
pub(super) fn validate_import_id(import_id: &str) -> CoreResult<()> {
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
pub(super) fn asset_capability(media_type: &str) -> ContentCapability {
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
pub(super) fn remove_owned_snapshot(
    path: &Path,
    staging_dir: &Path,
    import_id: &str,
) -> CoreResult<()> {
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
pub(super) fn stale_package_review() -> CoreError {
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
