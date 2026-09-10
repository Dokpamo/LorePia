mod container;
mod convert;
mod messagepack;
mod package;

use std::{
    fs,
    path::{Path, PathBuf},
};

use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, ImportInspection, ImportLimits, InspectionId,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use self::{
    container::{decode_external_preset, is_external_module, read_external_module},
    convert::{
        NormalizedExternalContent, convert_memory_preset, convert_module, convert_preset,
        looks_like_memory_preset,
    },
    package::write_normalized_package,
};
use crate::{inspect_content_package, sha256_file, validated_source_metadata};

const MAX_EXTERNAL_JSON_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedExternalImport {
    pub inspection: ImportInspection,
    pub plan_hash: String,
    pub normalized_package_path: PathBuf,
    pub normalized_package_sha256: String,
    pub document_count: u32,
}

pub fn prepare_external_import(
    path: &Path,
    limits: ImportLimits,
    staging_directory: &Path,
) -> CoreResult<Option<PreparedExternalImport>> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let imported_module = is_external_module(path)?;
    if !imported_module && extension != "risup" && extension != "json" {
        return Ok(None);
    }
    let source_metadata = validated_source_metadata(path, limits)?;
    let source_sha256 = sha256_file(path)?;
    let content = if imported_module {
        convert_module(
            read_external_module(path, limits, &source_sha256)?,
            &source_sha256,
        )?
    } else if extension == "risup" {
        convert_preset(&decode_external_preset(path)?, &source_sha256)?
    } else if extension == "json" && source_metadata.len() <= MAX_EXTERNAL_JSON_BYTES {
        let value: Value = serde_json::from_slice(&fs::read(path).map_err(storage_error)?)
            .map_err(|_| unsupported("JSON source is not a supported compatibility document"))?;
        if !looks_like_memory_preset(&value) {
            return Ok(None);
        }
        convert_memory_preset(&value, &source_sha256)?
    } else {
        return Ok(None);
    };
    fs::create_dir_all(staging_directory).map_err(storage_error)?;
    let inspection_id = InspectionId::new();
    let normalized_package_path = staging_directory.join(format!(
        "inspection-{}-imported-package.partial",
        inspection_id.0
    ));
    let prepared = prepare_package(
        path,
        &source_sha256,
        source_metadata.len(),
        inspection_id,
        content,
        limits,
        normalized_package_path,
    );
    prepared.map(Some)
}

#[allow(clippy::too_many_arguments)]
fn prepare_package(
    source_path: &Path,
    source_sha256: &str,
    source_size: u64,
    inspection_id: InspectionId,
    mut content: NormalizedExternalContent,
    limits: ImportLimits,
    normalized_package_path: PathBuf,
) -> CoreResult<PreparedExternalImport> {
    let written = match write_normalized_package(
        source_path,
        &normalized_package_path,
        source_sha256,
        &content,
    ) {
        Ok(written) => written,
        Err(error) => {
            let _ = fs::remove_file(&normalized_package_path);
            return Err(error);
        }
    };
    let package_inspection = match inspect_content_package(&normalized_package_path, limits) {
        Ok(inspection) => inspection,
        Err(error) => {
            let _ = fs::remove_file(&normalized_package_path);
            return Err(CoreError::new(
                error.code,
                format!(
                    "normalized imported package failed self-inspection: {}",
                    error.message
                ),
                error.recoverable,
            ));
        }
    };
    let inactive_components = package_inspection
        .components
        .iter()
        .filter(|component| !component.is_selectable())
        .count();
    let mut blocked_reasons = Vec::new();
    if inactive_components > 0 {
        blocked_reasons.push(format!(
            "{inactive_components} normalized component(s) failed the safe import contract"
        ));
    }
    content.warnings.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.message.cmp(&right.message))
    });
    content.unsupported_optional_fields.sort();
    content.unsupported_optional_fields.dedup();
    let normalized_package_sha256 = sha256_file(&normalized_package_path)?;
    let estimated_stored_size = normalized_package_path
        .metadata()
        .map_err(storage_error)?
        .len();
    let inspection = ImportInspection {
        id: inspection_id,
        kind: content.kind,
        display_name: content.name,
        description: content.description,
        representative_image: None,
        source_sha256: source_sha256.to_owned(),
        source_size,
        estimated_stored_size,
        asset_count: written.asset_count,
        dynamic_content: content.dynamic_content,
        warnings: content.warnings,
        blocked_reasons,
        unsupported_optional_fields: content.unsupported_optional_fields,
    };
    let plan_hash = external_plan_hash(
        &inspection,
        &normalized_package_sha256,
        written.document_count,
    );
    Ok(PreparedExternalImport {
        inspection,
        plan_hash,
        normalized_package_path,
        normalized_package_sha256,
        document_count: written.document_count,
    })
}

fn external_plan_hash(
    inspection: &ImportInspection,
    package_sha256: &str,
    document_count: u32,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"lorepia-external-import-v1\0");
    digest.update(inspection.source_sha256.as_bytes());
    digest.update([0]);
    digest.update(package_sha256.as_bytes());
    digest.update([0]);
    digest.update(format!("{:?}", inspection.kind).as_bytes());
    digest.update(document_count.to_le_bytes());
    hex::encode(digest.finalize())
}

fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}

fn storage_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot prepare compatibility import: {error}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use lorepia_domain::ContentKind;
    use tempfile::{Builder, tempdir};

    use super::*;

    #[test]
    fn normalizes_an_imported_memory_json_into_a_self_inspecting_package() {
        let mut source = Builder::new()
            .suffix(".json")
            .tempfile()
            .expect("memory preset source");
        source
            .write_all(
                br#"{"type":"risu","ver":1,"data":{"name":"Hypa fixture","settings":{"summarizationPrompt":"Summarize the important durable facts.\n{{slot}}","maxChatsPerSummary":8}}}"#,
            )
            .expect("write memory preset");
        source.flush().expect("flush memory preset");
        let staging = tempdir().expect("staging");

        let prepared =
            prepare_external_import(source.path(), ImportLimits::default(), staging.path())
                .expect("prepare external memory preset")
                .expect("recognized external memory preset");
        assert_eq!(prepared.inspection.kind, ContentKind::RisuMemoryPreset);
        assert!(prepared.inspection.is_allowed());
        assert_eq!(prepared.document_count, 1);
        assert_eq!(prepared.inspection.asset_count, 0);

        let package =
            inspect_content_package(&prepared.normalized_package_path, ImportLimits::default())
                .expect("inspect normalized package");
        assert!(package.is_allowed());
        assert_eq!(package.selectable_component_ids(), vec!["prompt"]);
    }
}
