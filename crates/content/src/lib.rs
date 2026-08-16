//! Safe inspection of local character cards and CHARX packages.

mod adapters;
mod archive;
mod hashing;
mod package;
mod path;
mod png;

use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use lorepia_domain::{
    CharacterContentV1, ContentKind, CoreError, CoreErrorCode, CoreResult, ImportImagePreview,
    ImportInspection, ImportLimits, ImportWarning, InspectionId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use hashing::sha256_file;
pub use package::{
    ContentCapability, ContentPackageComponent, ContentPackageComponentKind,
    ContentPackageComponentState, ContentPackageDependency, ContentPackageInspection,
    ContentPackageManifest, ContentPackageSelectionPlan, ContentPackageTransformation,
    PackageConflict, PreparedContentDocument, PreparedContentDocumentEnvelope,
    PreparedContentPackageImport, StagedContentPackageAsset, discard_staged_content_package_assets,
    inspect_content_package, prepare_content_package_import, revalidate_content_package_selection,
    select_content_package_components, stage_selected_content_package_assets,
};

const ZIP_LOCAL_FILE_MAGIC: &[u8; 4] = b"PK\x03\x04";
const ZIP_EMPTY_ARCHIVE_MAGIC: &[u8; 4] = b"PK\x05\x06";
const ZIP_SPANNED_ARCHIVE_MAGIC: &[u8; 4] = b"PK\x07\x08";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAsset {
    pub original_path: String,
    pub staged_path: PathBuf,
    pub sha256: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub signature_valid: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedImport {
    pub inspection: ImportInspection,
    pub character_content: CharacterContentV1,
    pub plan_hash: String,
    pub staged_assets: Vec<StagedAsset>,
}

/// Normalized card content and review metadata bound to a deterministic hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterImportPlan {
    pub inspection: ImportInspection,
    pub character_content: CharacterContentV1,
    pub plan_hash: String,
}

/// Inspect an untrusted staging file without writing to the final library.
pub fn inspect_file(path: &Path, limits: ImportLimits) -> CoreResult<ImportInspection> {
    inspect_character_file(path, limits).map(|plan| plan.inspection)
}

/// Inspect and normalize all supported public character-card fields without
/// writing to the final library.
pub fn inspect_character_file(
    path: &Path,
    limits: ImportLimits,
) -> CoreResult<CharacterImportPlan> {
    let source_metadata = validated_source_metadata(path, limits)?;
    let source_sha256 = sha256_file(path)?;

    let mut reader = BufReader::new(File::open(path).map_err(storage_error)?);
    let mut magic = [0_u8; 4];
    let read = reader.read(&mut magic).map_err(storage_error)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let (
        kind,
        metadata,
        representative_image,
        asset_count,
        estimated_size,
        mut warnings,
        blocked_reasons,
    ) = if read == magic.len() && is_zip_signature(magic) {
        let mut inspected = archive::inspect_archive(path, limits, &source_sha256)?;
        if extension != "charx" && extension != "zip" {
            let mut extension_warnings = inspected_extension_warning(&extension, "CHARX/ZIP");
            extension_warnings.append(&mut inspected.warnings);
            inspected.warnings = extension_warnings;
        }
        (
            ContentKind::CharxPackage,
            inspected.metadata,
            inspected.representative_image,
            inspected.asset_count,
            inspected.total_uncompressed,
            inspected.warnings,
            inspected.blocked_reasons,
        )
    } else if read == magic.len() && png::has_png_magic(&magic) {
        // The signature check only reads the first four bytes, so re-read the
        // file and let the extractor validate the full eight-byte signature.
        let bytes = std::fs::read(path).map_err(storage_error)?;
        let card = png::extract_card_metadata(&bytes)?;
        let metadata = adapters::parse_card_json_with_source(&card, &source_sha256)?;
        let mut warnings = inspected_extension_warning(&extension, "PNG");
        warnings.extend(promoted_card_warning(&metadata));
        let source_size = source_metadata.len();
        (
            ContentKind::CharacterCardPng,
            metadata,
            Some(ImportImagePreview {
                logical_asset_id: PNG_AVATAR_ASSET_ID.to_owned(),
                media_type: PNG_MEDIA_TYPE.to_owned(),
                size_bytes: source_size,
            }),
            1,
            source_size,
            warnings,
            Vec::new(),
        )
    } else {
        let bytes = std::fs::read(path).map_err(storage_error)?;
        let metadata = adapters::parse_card_json_with_source(&bytes, &source_sha256)?;
        let estimated_size = metadata.len_bytes;
        let mut warnings = inspected_extension_warning(&extension, "JSON");
        warnings.extend(promoted_card_warning(&metadata));
        (
            ContentKind::CharacterCardV3,
            metadata,
            None,
            0,
            estimated_size,
            warnings,
            Vec::new(),
        )
    };

    warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });

    let character_content = metadata.content;
    let inspection = ImportInspection {
        id: InspectionId::new(),
        kind,
        display_name: metadata.name,
        description: metadata.description,
        representative_image,
        source_sha256,
        source_size: source_metadata.len(),
        estimated_stored_size: estimated_size,
        asset_count,
        warnings,
        blocked_reasons,
        unsupported_optional_fields: metadata.unsupported_optional_fields,
    };
    let plan_hash = character_plan_hash(&inspection, &character_content)?;
    Ok(CharacterImportPlan {
        inspection,
        character_content,
        plan_hash,
    })
}

fn validated_source_metadata(path: &Path, limits: ImportLimits) -> CoreResult<std::fs::Metadata> {
    let source_metadata = path.symlink_metadata().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot read staging file metadata: {error}"),
            true,
        )
    })?;
    if source_metadata.file_type().is_symlink() {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "the import source must not be a symbolic link",
            false,
        ));
    }
    if !source_metadata.is_file() {
        return Err(CoreError::invalid(
            "the import source is not a regular file",
        ));
    }
    if source_metadata.len() == 0 {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            "the import source is empty",
            false,
        ));
    }
    if source_metadata.len() > limits.max_source_bytes {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            format!(
                "source is {} bytes; maximum is {} bytes",
                source_metadata.len(),
                limits.max_source_bytes
            ),
            false,
        ));
    }
    Ok(source_metadata)
}

/// Inspect a source and stage validated CHARX assets for an approved commit.
///
/// Staged assets are written as uniquely named flat files below
/// `asset_staging_directory`. Callers own their cleanup.
pub fn prepare_import(
    path: &Path,
    limits: ImportLimits,
    asset_staging_directory: &Path,
) -> CoreResult<PreparedImport> {
    let plan = inspect_character_file(path, limits)?;
    let inspection = plan.inspection;
    if inspection.is_allowed() && inspection.kind == ContentKind::CharacterCardPng {
        let staged_assets = stage_png_avatar(
            path,
            asset_staging_directory,
            &inspection.id.0,
            &inspection.source_sha256,
            inspection.source_size,
        )?;
        return Ok(PreparedImport {
            inspection,
            character_content: plan.character_content,
            plan_hash: plan.plan_hash,
            staged_assets,
        });
    }
    let staged_assets = if inspection.is_allowed() && inspection.kind == ContentKind::CharxPackage {
        let assets =
            archive::stage_archive_assets(path, limits, asset_staging_directory, &inspection.id.0)?;
        let current_hash = sha256_file(path)?;
        let current_size = path.metadata().map_err(storage_error)?.len();
        if current_hash != inspection.source_sha256 || current_size != inspection.source_size {
            for asset in &assets {
                let _ = std::fs::remove_file(&asset.staged_path);
            }
            return Err(CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "import source changed while assets were being staged",
                false,
            ));
        }
        if assets.len() != inspection.asset_count as usize {
            for asset in &assets {
                let _ = std::fs::remove_file(&asset.staged_path);
            }
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "staged asset count does not match the inspected package",
                false,
            ));
        }
        assets
    } else {
        Vec::new()
    };
    Ok(PreparedImport {
        inspection,
        character_content: plan.character_content,
        plan_hash: plan.plan_hash,
        staged_assets,
    })
}

/// Stages the PNG source itself as the card avatar.
///
/// A PNG card carries exactly one asset: the image it is embedded in. The
/// source digest and size are re-verified after the copy so a file swapped
/// during staging is rejected, matching the CHARX staging contract.
fn stage_png_avatar(
    path: &Path,
    staging_directory: &Path,
    inspection_id: &str,
    expected_sha256: &str,
    expected_size: u64,
) -> CoreResult<Vec<StagedAsset>> {
    std::fs::create_dir_all(staging_directory).map_err(storage_error)?;
    let staged_path = staging_directory.join(format!("inspection-{inspection_id}-asset-0.partial"));
    std::fs::copy(path, &staged_path).map_err(|error| {
        let _ = std::fs::remove_file(&staged_path);
        storage_error(error)
    })?;

    let staged_sha256 = sha256_file(&staged_path)?;
    let staged_size = staged_path.metadata().map_err(storage_error)?.len();
    let current_sha256 = sha256_file(path)?;
    let current_size = path.metadata().map_err(storage_error)?.len();
    if staged_sha256 != expected_sha256
        || staged_size != expected_size
        || current_sha256 != expected_sha256
        || current_size != expected_size
    {
        let _ = std::fs::remove_file(&staged_path);
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "import source changed while the card image was being staged",
            false,
        ));
    }

    Ok(vec![StagedAsset {
        original_path: PNG_AVATAR_ASSET_ID.to_owned(),
        staged_path,
        sha256: staged_sha256,
        media_type: PNG_MEDIA_TYPE.to_owned(),
        size_bytes: staged_size,
        signature_valid: true,
    }])
}

fn character_plan_hash(
    inspection: &ImportInspection,
    content: &CharacterContentV1,
) -> CoreResult<String> {
    #[derive(Serialize)]
    struct HashInput<'a> {
        kind: ContentKind,
        display_name: &'a str,
        description: &'a str,
        source_sha256: &'a str,
        source_size: u64,
        estimated_stored_size: u64,
        asset_count: u32,
        warnings: &'a [ImportWarning],
        blocked_reasons: &'a [String],
        unsupported_optional_fields: &'a [String],
        content: &'a CharacterContentV1,
    }

    let bytes = serde_json::to_vec(&HashInput {
        kind: inspection.kind,
        display_name: &inspection.display_name,
        description: &inspection.description,
        source_sha256: &inspection.source_sha256,
        source_size: inspection.source_size,
        estimated_stored_size: inspection.estimated_stored_size,
        asset_count: inspection.asset_count,
        warnings: &inspection.warnings,
        blocked_reasons: &inspection.blocked_reasons,
        unsupported_optional_fields: &inspection.unsupported_optional_fields,
        content,
    })
    .map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("cannot serialize character import plan: {error}"),
            false,
        )
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn is_zip_signature(magic: [u8; 4]) -> bool {
    matches!(
        &magic,
        ZIP_LOCAL_FILE_MAGIC | ZIP_EMPTY_ARCHIVE_MAGIC | ZIP_SPANNED_ARCHIVE_MAGIC
    )
}

/// Logical id given to the PNG source when it is staged as the card avatar.
pub(crate) const PNG_AVATAR_ASSET_ID: &str = "card.png";
pub(crate) const PNG_MEDIA_TYPE: &str = "image/png";

/// Tells the reviewer that a V2 card was promoted before anything is committed.
fn promoted_card_warning(metadata: &adapters::CardMetadata) -> Vec<ImportWarning> {
    if metadata.promoted_from_v2 {
        vec![ImportWarning {
            code: "character_card_v2_promoted".to_owned(),
            message: "Card declares the V2 specification and was promoted to V3. \
                      Fields that only V3 defines are empty."
                .to_owned(),
        }]
    } else {
        Vec::new()
    }
}

fn inspected_extension_warning(extension: &str, detected: &str) -> Vec<ImportWarning> {
    let expected = match detected {
        "JSON" => extension == "json",
        "PNG" => extension == "png",
        _ => matches!(extension, "charx" | "zip"),
    };
    if expected {
        Vec::new()
    } else {
        let actual = if extension.is_empty() {
            "no extension".to_owned()
        } else {
            format!(".{extension}")
        };
        vec![ImportWarning {
            code: "extension_mismatch".to_owned(),
            message: format!("File contents are {detected}, but the file has {actual}."),
        }]
    }
}

fn storage_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("staging file access failed: {error}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    #[cfg(unix)]
    use std::fs;
    use tempfile::NamedTempFile;
    #[cfg(unix)]
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn inspects_minimal_v3_card() {
        let mut file = NamedTempFile::new().expect("temp file");
        write!(
            file,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Segu","description":"Guide"}}}}"#
        )
        .expect("write fixture");

        let inspection =
            inspect_file(file.path(), ImportLimits::default()).expect("valid inspection");
        assert_eq!(inspection.kind, ContentKind::CharacterCardV3);
        assert_eq!(inspection.display_name, "Segu");
        assert!(inspection.is_allowed());
    }

    #[test]
    fn rejects_oversized_source_before_parsing() {
        let mut file = NamedTempFile::new().expect("temp file");
        file.write_all(b"{}").expect("write fixture");
        let limits = ImportLimits {
            max_source_bytes: 1,
            ..ImportLimits::default()
        };

        let error = inspect_file(file.path(), limits).expect_err("must reject");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    }

    #[test]
    fn accepts_a_source_at_the_exact_size_boundary() {
        let mut bytes = br#"{"spec":"chara_card_v3","data":{"name":"Boundary"}}"#.to_vec();
        bytes.extend(std::iter::repeat_n(b' ', 32));
        let mut file = NamedTempFile::new().expect("temp file");
        file.write_all(&bytes).expect("write fixture");
        let limits = ImportLimits {
            max_source_bytes: bytes.len() as u64,
            ..ImportLimits::default()
        };

        let inspection = inspect_file(file.path(), limits).expect("boundary is inclusive");
        assert_eq!(inspection.source_size, bytes.len() as u64);
        assert_eq!(inspection.estimated_stored_size, bytes.len() as u64);
    }

    #[test]
    fn rejects_an_empty_source_with_a_stable_error() {
        let file = NamedTempFile::new().expect("temp file");

        let error = inspect_file(file.path(), ImportLimits::default()).expect_err("empty source");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
        assert_eq!(error.message, "the import source is empty");
        assert!(!error.recoverable);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symbolic_link_source() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("temp directory");
        let target = directory.path().join("target.json");
        fs::write(
            &target,
            br#"{"spec":"chara_card_v3","data":{"name":"Target"}}"#,
        )
        .expect("write target");
        let link = directory.path().join("link.json");
        symlink(&target, &link).expect("create symlink");

        let error = inspect_file(&link, ImportLimits::default()).expect_err("reject symlink");
        assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
    }
}
