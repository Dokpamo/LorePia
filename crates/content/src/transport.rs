use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult, ImportLimits, InspectionId};
use serde_json::Value;
use zip::ZipArchive;

use crate::{archive, path::validate_archive_path};

const BUFFER_BYTES: usize = 64 * 1024;

/// Extracts a single character-card file carried by an otherwise ordinary ZIP.
///
/// This narrowly handles download wrappers without relaxing the per-entry
/// limit for real CHARX archives. The extracted file remains below the global
/// source-size and compression-ratio bounds and is validated as a card before
/// being returned to Core-owned staging.
pub fn extract_single_character_transport(
    source_path: &Path,
    limits: ImportLimits,
    staging_directory: &Path,
) -> CoreResult<Option<PathBuf>> {
    let mut magic = [0_u8; 4];
    let mut source = File::open(source_path).map_err(storage_error)?;
    if source.read(&mut magic).map_err(storage_error)? != magic.len()
        || !matches!(&magic, b"PK\x03\x04" | b"PK\x05\x06" | b"PK\x07\x08")
    {
        return Ok(None);
    }
    if archive::has_embedded_character_archive(source_path, limits)? {
        return Ok(None);
    }
    let mut archive = ZipArchive::new(File::open(source_path).map_err(storage_error)?)
        .map_err(|error| unsafe_archive(format!("invalid transport ZIP: {error}")))?;
    if archive.len() != 1 {
        return Ok(None);
    }
    let mut entry = archive
        .by_index(0)
        .map_err(|error| unsafe_archive(format!("invalid transport ZIP entry: {error}")))?;
    if entry.is_dir() || entry.encrypted() {
        return Ok(None);
    }
    if entry
        .unix_mode()
        .is_some_and(|mode| mode & 0o170_000 == 0o120_000)
    {
        return Err(unsafe_archive(
            "transport ZIP entry must not be a symbolic link",
        ));
    }
    let logical_path = validate_archive_path(entry.name()).map_err(unsafe_archive)?;
    let extension = Path::new(&logical_path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(
        extension.as_str(),
        "charx" | "jpg" | "jpeg" | "png" | "json"
    ) {
        return Ok(None);
    }
    let size = entry.size();
    let compressed = entry.compressed_size();
    if size == 0
        || size > limits.max_source_bytes
        || size > limits.max_total_uncompressed_bytes
        || compressed == 0
        || size > compressed.saturating_mul(limits.max_compression_ratio)
    {
        return Err(unsafe_archive(
            "transport ZIP entry exceeds source or compression limits",
        ));
    }
    std::fs::create_dir_all(staging_directory).map_err(storage_error)?;
    let id = InspectionId::new();
    let extracted_path = staging_directory.join(format!("transport-{}.{}", id.0, extension));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&extracted_path)
        .map_err(storage_error)?;
    let copy_result = copy_exact(&mut entry, &mut output, size).and_then(|()| {
        output.sync_all().map_err(storage_error)?;
        validate_extracted_card(&extracted_path, &extension, limits)
    });
    match copy_result {
        Ok(true) => Ok(Some(extracted_path)),
        Ok(false) => {
            let _ = std::fs::remove_file(&extracted_path);
            Ok(None)
        }
        Err(error) => {
            let _ = std::fs::remove_file(&extracted_path);
            Err(error)
        }
    }
}

fn copy_exact<R: Read, W: Write>(reader: &mut R, writer: &mut W, expected: u64) -> CoreResult<()> {
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    let mut copied = 0_u64;
    loop {
        let read = reader.read(&mut buffer).map_err(storage_error)?;
        if read == 0 {
            break;
        }
        copied = copied
            .checked_add(read as u64)
            .ok_or_else(|| unsafe_archive("transport ZIP size overflow"))?;
        if copied > expected {
            return Err(unsafe_archive(
                "transport ZIP entry exceeded its declared size",
            ));
        }
        writer.write_all(&buffer[..read]).map_err(storage_error)?;
    }
    if copied != expected {
        return Err(unsafe_archive(
            "transport ZIP entry size does not match its declaration",
        ));
    }
    Ok(())
}

fn validate_extracted_card(path: &Path, extension: &str, limits: ImportLimits) -> CoreResult<bool> {
    if archive::has_embedded_character_archive(path, limits)? {
        return Ok(true);
    }
    let mut file = File::open(path).map_err(storage_error)?;
    let mut signature = [0_u8; 8];
    let read = file.read(&mut signature).map_err(storage_error)?;
    if extension == "png" && read == signature.len() && signature == *b"\x89PNG\r\n\x1a\n" {
        return Ok(true);
    }
    if extension != "json" || path.metadata().map_err(storage_error)?.len() > 4 * 1024 * 1024 {
        return Ok(false);
    }
    let value: Value = serde_json::from_slice(&std::fs::read(path).map_err(storage_error)?)
        .map_err(|_| unsafe_archive("transport JSON is invalid"))?;
    Ok(matches!(
        value.get("spec").and_then(Value::as_str),
        Some("chara_card_v2" | "chara_card_v3")
    ))
}

fn unsafe_archive(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsafeArchive, message, false)
}

fn storage_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot read transport ZIP: {error}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tempfile::tempdir;
    use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

    use super::*;
    use crate::inspect_file;

    #[test]
    fn extracts_and_validates_a_single_character_download_wrapper() {
        let root = tempdir().expect("root");
        let source_path = root.path().join("download.zip");
        let output = File::create(&source_path).expect("wrapper");
        let mut archive = ZipWriter::new(output);
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .unix_permissions(0o644);
        archive
            .start_file("character.json", options)
            .expect("card entry");
        archive
            .write_all(
                br#"{"spec":"chara_card_v3","data":{"name":"Wrapped","description":"Safe"}}"#,
            )
            .expect("card bytes");
        archive.finish().expect("finish wrapper");

        let extracted =
            extract_single_character_transport(&source_path, ImportLimits::default(), root.path())
                .expect("extract wrapper")
                .expect("recognized wrapper");
        let inspection =
            inspect_file(&extracted, ImportLimits::default()).expect("inspect extracted card");
        assert_eq!(inspection.display_name, "Wrapped");
        assert!(inspection.is_allowed());
    }
}
