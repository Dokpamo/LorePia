//! Package source and asset CAS promotion, journaling, and recovery.

mod journal;
mod promotion;
mod recovery;

use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};

use super::{ensure_regular_file, storage_io_error};

pub(crate) use journal::{claim_package_asset_promotions, claim_package_source_promotion};
#[cfg(test)]
pub(super) use journal::{
    cleanup_package_cas_promotion, ensure_package_cas_promotion_intents,
    mark_package_cas_file_durable,
};
pub(super) use recovery::{
    recover_package_cas_promotions, reject_durable_committing_package_imports,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PackageCasPromotionIntent {
    pub(super) import_id: String,
    pub(super) namespace: &'static str,
    pub(super) sha256: String,
    pub(super) size_bytes: u64,
    pub(super) media_type: Option<String>,
    pub(super) relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageCasPromotionJournalEntry {
    intent: PackageCasPromotionIntent,
    phase: String,
}
fn validate_owned_staged_file(root: &Path, candidate: &Path) -> CoreResult<PathBuf> {
    let metadata = fs::symlink_metadata(candidate).map_err(storage_io_error)?;
    if !metadata.file_type().is_file() {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "package promotion source is not a regular staged file",
            false,
        ));
    }
    let staging = fs::canonicalize(root.join("staging")).map_err(storage_io_error)?;
    let candidate = fs::canonicalize(candidate).map_err(storage_io_error)?;
    if candidate == staging || !candidate.starts_with(&staging) {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "package promotion source escaped the owned staging directory",
            false,
        ));
    }
    ensure_regular_file(&candidate)?;
    Ok(candidate)
}

fn verify_media_type_signature(path: &Path, media_type: &str) -> CoreResult<()> {
    let mut file = File::open(path).map_err(storage_io_error)?;
    verify_open_file_media_type_signature(&mut file, media_type)
}

pub(in crate::database) fn verify_open_file_media_type_signature(
    file: &mut File,
    media_type: &str,
) -> CoreResult<()> {
    let normalized = media_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(CoreError::invalid("package asset media type is empty"));
    }
    let mut header = [0_u8; 64];
    let read = file.read(&mut header).map_err(storage_io_error)?;
    let header = &header[..read];
    let starts = |prefix: &[u8]| header.starts_with(prefix);
    let matches = match normalized.as_str() {
        "application/octet-stream" => true,
        "image/png" => starts(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => starts(b"\xff\xd8\xff"),
        "image/gif" => starts(b"GIF87a") || starts(b"GIF89a"),
        "image/webp" => header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WEBP",
        "image/bmp" => starts(b"BM"),
        "image/avif" => header.len() >= 12 && &header[4..8] == b"ftyp" && &header[8..12] == b"avif",
        "image/heic" | "image/heif" => {
            header.len() >= 12
                && &header[4..8] == b"ftyp"
                && matches!(&header[8..12], b"heic" | b"heix" | b"heif" | b"mif1")
        }
        "audio/mpeg" => {
            starts(b"ID3") || (header.len() >= 2 && header[0] == 0xff && header[1] & 0xe0 == 0xe0)
        }
        "audio/wav" | "audio/x-wav" => {
            header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WAVE"
        }
        "audio/ogg" | "video/ogg" | "application/ogg" => starts(b"OggS"),
        "audio/flac" => starts(b"fLaC"),
        "video/mp4" | "audio/mp4" => header.len() >= 12 && &header[4..8] == b"ftyp",
        "video/webm" | "audio/webm" => starts(b"\x1a\x45\xdf\xa3"),
        "application/pdf" => starts(b"%PDF-"),
        "application/zip" => {
            starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08")
        }
        "application/json" | "text/plain" | "text/markdown" | "text/csv" => {
            std::str::from_utf8(header).is_ok()
        }
        "image/svg+xml" => std::str::from_utf8(header).is_ok_and(|text| {
            let text = text.trim_start_matches(|character: char| character.is_whitespace());
            text.starts_with("<svg") || text.starts_with("<?xml")
        }),
        _ => {
            return Err(CoreError::invalid(format!(
                "package asset media type cannot be signature-validated: {normalized}"
            )));
        }
    };
    if !matches {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "package asset bytes do not match the reviewed media type",
            false,
        ));
    }
    Ok(())
}

pub(in crate::database) fn validate_renderer_media_type(media_type: &str) -> CoreResult<()> {
    if matches!(
        media_type,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "audio/mpeg"
            | "audio/wav"
            | "audio/ogg"
            | "video/mp4"
            | "video/webm"
    ) {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "asset media type is not allowed in the renderer",
            false,
        ))
    }
}
