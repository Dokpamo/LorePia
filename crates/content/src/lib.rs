//! Safe inspection of local character cards and CHARX packages.

mod adapters;
mod archive;
mod capabilities;
mod hashing;
mod package;
mod path;
mod png;
mod runtime;

use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use lorepia_domain::{
    CharacterContentV1, ContentKind, CoreError, CoreErrorCode, CoreResult,
    ImportDynamicContentReview, ImportImagePreview, ImportInspection, ImportLimits,
    ImportRegexRulePhase, ImportRegexRuleReview, ImportWarning, InspectionId,
    PortableTransformPhase,
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

struct InspectedCharacterSource {
    kind: ContentKind,
    metadata: adapters::CardMetadata,
    representative_image: Option<ImportImagePreview>,
    asset_count: u32,
    estimated_size: u64,
    warnings: Vec<ImportWarning>,
    blocked_reasons: Vec<String>,
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
    let mut source = inspect_character_source(path, limits, &source_sha256, source_metadata.len())?;
    source.warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });
    let character_content = source.metadata.content;
    let dynamic_content = dynamic_content_review(&character_content);
    let inspection = ImportInspection {
        id: InspectionId::new(),
        kind: source.kind,
        display_name: source.metadata.name,
        description: source.metadata.description,
        representative_image: source.representative_image,
        source_sha256,
        source_size: source_metadata.len(),
        estimated_stored_size: source.estimated_size,
        asset_count: source.asset_count,
        dynamic_content,
        warnings: source.warnings,
        blocked_reasons: source.blocked_reasons,
        unsupported_optional_fields: source.metadata.unsupported_optional_fields,
    };
    let plan_hash = character_plan_hash(&inspection, &character_content)?;
    Ok(CharacterImportPlan {
        inspection,
        character_content,
        plan_hash,
    })
}

const MAX_RUNTIME_RULES_PER_PHASE: usize = 128;

fn runtime_regex_rule_reviews(content: &CharacterContentV1) -> Vec<ImportRegexRuleReview> {
    let mut phase_counts = [0_usize; 3];
    let mut regex_rules = Vec::new();
    for transform in content
        .runtime
        .transforms
        .iter()
        .filter(|transform| transform.enabled)
    {
        let phase_index = match transform.phase {
            PortableTransformPhase::RequestContext => 0,
            PortableTransformPhase::ProviderOutput => 1,
            PortableTransformPhase::Display => 2,
        };
        let runtime_index = phase_counts[phase_index];
        phase_counts[phase_index] = runtime_index.saturating_add(1);
        if runtime_index >= MAX_RUNTIME_RULES_PER_PHASE {
            continue;
        }
        regex_rules.push(ImportRegexRuleReview {
            id: transform.id.clone(),
            name: transform.name.clone(),
            phase: match transform.phase {
                PortableTransformPhase::RequestContext => ImportRegexRulePhase::RequestContext,
                PortableTransformPhase::ProviderOutput => ImportRegexRulePhase::ProviderOutput,
                PortableTransformPhase::Display => ImportRegexRulePhase::Display,
            },
            runtime_index: u32::try_from(runtime_index).unwrap_or(u32::MAX),
            pattern: transform.pattern.clone(),
            flags: transform.flags.clone(),
        });
    }
    regex_rules
}

fn lore_regex_rule_reviews(
    content: &CharacterContentV1,
) -> (usize, usize, Vec<ImportRegexRuleReview>) {
    let mut lore_regex_rule_count = 0_usize;
    let mut enabled_lore_regex_rule_count = 0_usize;
    let mut lore_runtime_index = 0_usize;
    let mut regex_rules = Vec::new();
    if let Some(book) = content
        .knowledge_book
        .as_ref()
        .and_then(|reference| reference.embedded.as_ref())
    {
        for entry in &book.entries {
            if !entry.use_regex || entry.constant {
                continue;
            }
            let secondary_keys = entry.selective.then_some(entry.secondary_keys.as_slice());
            for (kind, keys) in [("primary", entry.primary_keys.as_slice())]
                .into_iter()
                .chain(secondary_keys.into_iter().map(|keys| ("secondary", keys)))
            {
                for (key_index, pattern) in keys.iter().enumerate() {
                    if pattern.is_empty() {
                        continue;
                    }
                    lore_regex_rule_count = lore_regex_rule_count.saturating_add(1);
                    if !entry.enabled || entry.folder {
                        continue;
                    }
                    let runtime_index = lore_runtime_index;
                    lore_runtime_index = lore_runtime_index.saturating_add(1);
                    if runtime_index >= MAX_RUNTIME_RULES_PER_PHASE {
                        continue;
                    }
                    enabled_lore_regex_rule_count = enabled_lore_regex_rule_count.saturating_add(1);
                    regex_rules.push(ImportRegexRuleReview {
                        id: format!("{}:{kind}:{key_index}", entry.id),
                        name: if entry.name.trim().is_empty() {
                            format!("Lore {kind} key")
                        } else {
                            format!("{} {kind} key", entry.name)
                        },
                        phase: ImportRegexRulePhase::Lore,
                        runtime_index: u32::try_from(runtime_index).unwrap_or(u32::MAX),
                        pattern: pattern.clone(),
                        flags: if entry.case_sensitive {
                            String::new()
                        } else {
                            "i".to_owned()
                        },
                    });
                }
            }
        }
    }
    (
        lore_regex_rule_count,
        enabled_lore_regex_rule_count,
        regex_rules,
    )
}

fn dynamic_content_review(content: &CharacterContentV1) -> ImportDynamicContentReview {
    let mut regex_rules = runtime_regex_rule_reviews(content);
    let (lore_regex_rule_count, enabled_lore_regex_rule_count, mut lore_regex_rules) =
        lore_regex_rule_reviews(content);
    regex_rules.append(&mut lore_regex_rules);
    let runtime_script_count = u32::try_from(content.runtime.scripts.len()).unwrap_or(u32::MAX);
    let runtime_capabilities_declared = content.runtime.required_capabilities.is_some();
    let required_runtime_capabilities = content
        .runtime
        .required_capabilities
        .clone()
        .unwrap_or_default();
    ImportDynamicContentReview {
        runtime_script_count,
        elevated_runtime_script_count: u32::try_from(
            content
                .runtime
                .scripts
                .iter()
                .filter(|script| script.elevated_access)
                .count(),
        )
        .unwrap_or(u32::MAX),
        required_runtime_capabilities,
        runtime_capabilities_declared,
        regex_rule_count: u32::try_from(
            content
                .runtime
                .transforms
                .len()
                .saturating_add(lore_regex_rule_count),
        )
        .unwrap_or(u32::MAX),
        enabled_regex_rule_count: u32::try_from(
            content
                .runtime
                .transforms
                .iter()
                .filter(|transform| transform.enabled)
                .count()
                .saturating_add(enabled_lore_regex_rule_count),
        )
        .unwrap_or(u32::MAX),
        model_calls_possible: content.runtime.required_capabilities.as_ref().map_or(
            runtime_script_count > 0,
            |capabilities| {
                capabilities.iter().any(|capability| {
                    matches!(
                        capability,
                        lorepia_domain::PortableRuntimeCapability::ModelPrimary
                            | lorepia_domain::PortableRuntimeCapability::ModelAuxiliary
                    )
                })
            },
        ),
        custom_markup_present: !content.runtime.background_markup.trim().is_empty()
            || content
                .runtime
                .transforms
                .iter()
                .any(|transform| transform.replacement.contains('<')),
        regex_rules,
    }
}

fn inspect_character_source(
    path: &Path,
    limits: ImportLimits,
    source_sha256: &str,
    source_size: u64,
) -> CoreResult<InspectedCharacterSource> {
    let mut reader = BufReader::new(File::open(path).map_err(storage_error)?);
    let mut magic = [0_u8; 4];
    let read = reader.read(&mut magic).map_err(storage_error)?;
    let is_direct_archive = read == magic.len() && is_zip_signature(magic);
    let is_embedded_archive =
        !is_direct_archive && archive::has_embedded_character_archive(path, limits)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if is_direct_archive || is_embedded_archive {
        inspect_archive_source(path, limits, source_sha256, &extension, is_embedded_archive)
    } else if read == magic.len() && png::has_png_magic(&magic) {
        inspect_png_source(path, source_sha256, &extension, source_size)
    } else {
        inspect_json_source(path, source_sha256, &extension)
    }
}

fn inspect_archive_source(
    path: &Path,
    limits: ImportLimits,
    source_sha256: &str,
    extension: &str,
    embedded: bool,
) -> CoreResult<InspectedCharacterSource> {
    let nonportable_policy = adapters::NonPortableContentPolicy::Omit;
    let mut inspected = archive::inspect_archive(path, limits, source_sha256, nonportable_policy)?;
    if !matches!(extension, "charx" | "zip") {
        let mut extension_warnings = inspected_extension_warning(extension, "CHARX/ZIP");
        extension_warnings.append(&mut inspected.warnings);
        inspected.warnings = extension_warnings;
    }
    if embedded {
        inspected.warnings.push(ImportWarning {
            code: "embedded_character_card".to_owned(),
            message: "A character card archive was detected inside another file.".to_owned(),
        });
    }
    Ok(InspectedCharacterSource {
        kind: ContentKind::CharxPackage,
        metadata: inspected.metadata,
        representative_image: inspected.representative_image,
        asset_count: inspected.asset_count,
        estimated_size: inspected.total_uncompressed,
        warnings: inspected.warnings,
        blocked_reasons: inspected.blocked_reasons,
    })
}

fn inspect_png_source(
    path: &Path,
    source_sha256: &str,
    extension: &str,
    source_size: u64,
) -> CoreResult<InspectedCharacterSource> {
    // The routing check reads four bytes; the extractor validates the full
    // eight-byte PNG signature and bounded metadata chunks.
    let bytes = std::fs::read(path).map_err(storage_error)?;
    let card = png::extract_card_metadata(&bytes)?;
    let metadata = adapters::parse_card_json_with_source(&card, source_sha256)?;
    let mut warnings = inspected_extension_warning(extension, "PNG");
    warnings.extend(promoted_card_warning(&metadata));
    Ok(InspectedCharacterSource {
        kind: ContentKind::CharacterCardPng,
        metadata,
        representative_image: Some(ImportImagePreview {
            logical_asset_id: PNG_AVATAR_ASSET_ID.to_owned(),
            media_type: PNG_MEDIA_TYPE.to_owned(),
            size_bytes: source_size,
        }),
        asset_count: 1,
        estimated_size: source_size,
        warnings,
        blocked_reasons: Vec::new(),
    })
}

fn inspect_json_source(
    path: &Path,
    source_sha256: &str,
    extension: &str,
) -> CoreResult<InspectedCharacterSource> {
    let bytes = std::fs::read(path).map_err(storage_error)?;
    let metadata = adapters::parse_card_json_with_source(&bytes, source_sha256)?;
    let estimated_size = metadata.len_bytes;
    let mut warnings = inspected_extension_warning(extension, "JSON");
    warnings.extend(promoted_card_warning(&metadata));
    Ok(InspectedCharacterSource {
        kind: ContentKind::CharacterCardV3,
        metadata,
        representative_image: None,
        asset_count: 0,
        estimated_size,
        warnings,
        blocked_reasons: Vec::new(),
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
        let assets = archive::stage_archive_assets(
            path,
            limits,
            asset_staging_directory,
            &inspection.id.0,
            adapters::NonPortableContentPolicy::Omit,
        )?;
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
        dynamic_content: &'a ImportDynamicContentReview,
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
        dynamic_content: &inspection.dynamic_content,
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
    fn dynamic_review_exposes_only_runtime_reachable_rule_metadata() {
        let content: CharacterContentV1 = serde_json::from_value(serde_json::json!({
            "knowledge_book": {
                "embedded": {
                    "id": "book",
                    "name": "Runtime lore",
                    "entries": [
                        {
                            "id": "active-lore",
                            "name": "Active lore",
                            "content": "content",
                            "enabled": true,
                            "primary_keys": ["(?<=hero)\\s+"],
                            "secondary_keys": ["(a+)+$"],
                            "selective": true,
                            "use_regex": true
                        },
                        {
                            "id": "disabled-lore",
                            "content": "content",
                            "enabled": false,
                            "primary_keys": ["disabled"],
                            "use_regex": true
                        }
                    ]
                }
            },
            "runtime": {
                "transforms": [
                    {
                        "id": "active-display",
                        "name": "Status",
                        "phase": "display",
                        "pattern": "(?<=Status:)\\s+",
                        "replacement": " ",
                        "flags": "gu"
                    },
                    {
                        "id": "disabled-output",
                        "phase": "provider_output",
                        "enabled": false,
                        "pattern": "(a+)+$",
                        "replacement": "",
                        "flags": ""
                    }
                ],
                "scripts": [{
                    "id": "script",
                    "language": "lua",
                    "source": "return",
                    "elevated_access": true
                }],
                "background_markup": "<style>.card { color: red; }</style>"
            }
        }))
        .expect("dynamic character content");

        let review = dynamic_content_review(&content);

        assert_eq!(review.runtime_script_count, 1);
        assert_eq!(review.elevated_runtime_script_count, 1);
        assert_eq!(review.regex_rule_count, 5);
        assert_eq!(review.enabled_regex_rule_count, 3);
        assert!(review.model_calls_possible);
        assert!(review.custom_markup_present);
        assert_eq!(review.regex_rules.len(), 3);
        assert_eq!(review.regex_rules[0].id, "active-display");
        assert_eq!(review.regex_rules[0].runtime_index, 0);
        assert_eq!(review.regex_rules[1].id, "active-lore:primary:0");
        assert_eq!(review.regex_rules[1].phase, ImportRegexRulePhase::Lore);
        assert_eq!(review.regex_rules[2].id, "active-lore:secondary:0");
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
