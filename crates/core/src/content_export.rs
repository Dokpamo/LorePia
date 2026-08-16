//! Path-private preparation of committed source bytes for native export.
//!
//! Core resolves only project-owned CAS objects and returns a safe descriptor
//! plus a Rust-only source path. Shell/Tauri may pass that source directly to
//! the scoped native save operation; neither paths nor bytes are serializable.

use std::{fmt, path::Path};

use lorepia_domain::{ContentKind, CoreError, CoreErrorCode, CoreResult, ImportLimits};
use lorepia_storage::PackageImportStatus;
use serde::{Deserialize, Serialize};

use crate::Core;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ContentSourceExportSelector {
    CharacterSource { character_id: String },
    ContentPackage { import_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSourceExportKind {
    CharacterCardV3,
    CharxPackage,
    LorepiaPackage,
}

/// Safe metadata that may cross the webview boundary after a native save.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentSourceExportDescriptor {
    pub kind: ContentSourceExportKind,
    pub source_id: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub suggested_file_name: String,
}

/// Verified project-owned CAS source for one native save operation.
///
/// This type deliberately implements neither `Serialize` nor `Clone`. Its
/// debug representation contains no absolute path.
pub struct PreparedContentSourceExport {
    descriptor: ContentSourceExportDescriptor,
    source_path: std::path::PathBuf,
}

impl PreparedContentSourceExport {
    pub const fn descriptor(&self) -> &ContentSourceExportDescriptor {
        &self.descriptor
    }

    #[doc(hidden)]
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
}

impl fmt::Debug for PreparedContentSourceExport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedContentSourceExport")
            .field("descriptor", &self.descriptor)
            .field("source_path", &"[REDACTED]")
            .finish()
    }
}

impl Core {
    /// Resolves one committed character or completed `LorePia` package source.
    ///
    /// The returned path is usable only by the Rust/native save adapter. Core
    /// revalidates the exact CAS row, path, size, digest, and source parser
    /// identity before returning it.
    pub fn prepare_content_source_export(
        &self,
        selector: &ContentSourceExportSelector,
    ) -> CoreResult<PreparedContentSourceExport> {
        match selector {
            ContentSourceExportSelector::CharacterSource { character_id } => {
                let source = self.storage().verified_character_source(character_id)?;
                let inspection =
                    lorepia_content::inspect_file(source.path(), ImportLimits::default())?;
                if inspection.source_sha256 != source.sha256().as_str()
                    || inspection.source_size != source.size_bytes()
                {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "committed character source inspection differs from CAS authority",
                        false,
                    ));
                }
                let (kind, extension) = match inspection.kind {
                    ContentKind::CharxPackage => (ContentSourceExportKind::CharxPackage, "charx"),
                    // Both card sources export as V3 content; the helper picks
                    // `png` or `json` from the committed source signature.
                    ContentKind::CharacterCardV3 | ContentKind::CharacterCardPng => (
                        ContentSourceExportKind::CharacterCardV3,
                        character_card_extension(source.path())?,
                    ),
                };
                Ok(PreparedContentSourceExport {
                    descriptor: ContentSourceExportDescriptor {
                        kind,
                        source_id: character_id.clone(),
                        sha256: source.sha256().as_str().to_owned(),
                        size_bytes: source.size_bytes(),
                        suggested_file_name: format!(
                            "lorepia-character-{}.{}",
                            safe_file_component(character_id, 80),
                            extension
                        ),
                    },
                    source_path: source.path().to_path_buf(),
                })
            }
            ContentSourceExportSelector::ContentPackage { import_id } => {
                let import = self.storage().get_package_import(import_id)?;
                if import.status != PackageImportStatus::Completed {
                    return Err(CoreError::invalid(
                        "only a completed content package source can be exported",
                    ));
                }
                let source = self.storage().get_package_source_for_import(import_id)?;
                let source_path = self
                    .storage()
                    .package_source_path(&source.source_sha256, source.source_size_bytes)?;
                let inspection = lorepia_content::inspect_content_package(
                    &source_path,
                    ImportLimits::default(),
                )?;
                if inspection.source_sha256 != source.source_sha256
                    || inspection.source_size != source.source_size_bytes
                    || inspection.manifest.package_id != source.package_id.as_str()
                    || inspection.manifest.version != source.version
                {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "completed package source inspection differs from CAS authority",
                        false,
                    ));
                }
                Ok(PreparedContentSourceExport {
                    descriptor: ContentSourceExportDescriptor {
                        kind: ContentSourceExportKind::LorepiaPackage,
                        source_id: import_id.clone(),
                        sha256: source.source_sha256,
                        size_bytes: source.source_size_bytes,
                        suggested_file_name: format!(
                            "lorepia-package-{}-{}.zip",
                            safe_file_component(source.package_id.as_str(), 48),
                            safe_file_component(&source.version, 48)
                        ),
                    },
                    source_path,
                })
            }
        }
    }

    /// Lists a bounded restart-safe catalog of completed package sources.
    ///
    /// Every descriptor is recreated through [`Self::prepare_content_source_export`],
    /// so one stale status, missing source, malformed package, or CAS mismatch
    /// fails the complete snapshot instead of returning a partially trusted
    /// catalog.
    pub fn list_completed_content_package_export_descriptors(
        &self,
        limit: u32,
    ) -> CoreResult<Vec<ContentSourceExportDescriptor>> {
        let import_ids = self.storage().list_completed_package_import_ids(limit)?;
        let mut descriptors = Vec::with_capacity(import_ids.len());
        for import_id in import_ids {
            let prepared =
                self.prepare_content_source_export(&ContentSourceExportSelector::ContentPackage {
                    import_id: import_id.clone(),
                })?;
            if prepared.descriptor().kind != ContentSourceExportKind::LorepiaPackage
                || prepared.descriptor().source_id != import_id
            {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "completed package export descriptor differs from its durable identity",
                    false,
                ));
            }
            descriptors.push(prepared.descriptor().clone());
        }
        Ok(descriptors)
    }
}

fn character_card_extension(path: &Path) -> CoreResult<&'static str> {
    use std::io::Read as _;

    let mut file = std::fs::File::open(path).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot open committed character source: {error}"),
            true,
        )
    })?;
    let mut signature = [0_u8; 8];
    let read = file.read(&mut signature).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot read committed character source signature: {error}"),
            true,
        )
    })?;
    if read == signature.len() && signature == *b"\x89PNG\r\n\x1a\n" {
        Ok("png")
    } else {
        Ok("json")
    }
}

fn safe_file_component(value: &str, maximum_characters: usize) -> String {
    let mut result = String::with_capacity(value.len().min(maximum_characters));
    let mut previous_separator = false;
    for character in value.chars().take(maximum_characters) {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            result.push(character);
            previous_separator = false;
        } else if !previous_separator {
            result.push('-');
            previous_separator = true;
        }
    }
    let result = result.trim_matches(['-', '.']).to_owned();
    if result.is_empty() {
        "content".to_owned()
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_components_cannot_escape_the_native_save_name() {
        assert_eq!(safe_file_component("../private/card", 80), "private-card");
        assert_eq!(safe_file_component("  ", 80), "content");
        assert_eq!(safe_file_component(&"a".repeat(200), 48).len(), 48);
        let package_name = format!(
            "lorepia-package-{}-{}.zip",
            safe_file_component(&"a".repeat(200), 48),
            safe_file_component(&"b".repeat(200), 48),
        );
        assert!(package_name.len() <= 128);
    }

    #[test]
    fn prepared_export_debug_redacts_the_source_path() {
        let prepared = PreparedContentSourceExport {
            descriptor: ContentSourceExportDescriptor {
                kind: ContentSourceExportKind::CharacterCardV3,
                source_id: "character".to_owned(),
                sha256: "ab".repeat(32),
                size_bytes: 1,
                suggested_file_name: "character.json".to_owned(),
            },
            source_path: "/Users/synthetic/private/card.json".into(),
        };
        let debug = format!("{prepared:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("/Users/"));
    }
}
