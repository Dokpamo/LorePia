use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use uuid::Uuid;

pub(super) fn snapshot_import_source(
    source_path: &Path,
    staging_dir: &Path,
    max_source_bytes: u64,
) -> CoreResult<PathBuf> {
    let source_metadata = fs::symlink_metadata(source_path).map_err(import_io_error)?;
    if !source_metadata.file_type().is_file() {
        return Err(CoreError::invalid(
            "the import source must be a regular file and cannot be a symbolic link",
        ));
    }
    if source_metadata.len() > max_source_bytes {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            format!(
                "source is {} bytes; maximum is {} bytes",
                source_metadata.len(),
                max_source_bytes
            ),
            false,
        ));
    }

    fs::create_dir_all(staging_dir).map_err(import_io_error)?;
    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default();
    let snapshot = staging_dir.join(format!("inspection-{}{extension}", Uuid::new_v4()));
    let result = (|| {
        let source = File::open(source_path).map_err(import_io_error)?;
        let opened_metadata = source.metadata().map_err(import_io_error)?;
        if !opened_metadata.is_file() {
            return Err(CoreError::invalid(
                "the import source is not a regular file",
            ));
        }
        let mut reader = BufReader::new(source);
        let mut destination = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&snapshot)
            .map_err(import_io_error)?;
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer).map_err(import_io_error)?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(
                    u64::try_from(read)
                        .map_err(|_| CoreError::internal("import byte count overflow"))?,
                )
                .ok_or_else(|| CoreError::internal("import size overflow"))?;
            if copied > max_source_bytes {
                return Err(CoreError::new(
                    CoreErrorCode::UnsupportedContent,
                    format!("source exceeds the {max_source_bytes} byte import limit"),
                    false,
                ));
            }
            destination
                .write_all(&buffer[..read])
                .map_err(import_io_error)?;
        }
        destination.flush().map_err(import_io_error)?;
        destination.sync_all().map_err(import_io_error)?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&snapshot);
        return Err(error);
    }
    Ok(snapshot)
}

pub(super) fn remove_snapshot(snapshot: &Path, staging_dir: &Path) -> CoreResult<()> {
    if snapshot.parent() != Some(staging_dir) || snapshot.file_name().is_none() {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "pending import snapshot is outside the owned staging directory",
            false,
        ));
    }
    match fs::remove_file(snapshot) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(import_io_error(error)),
    }
}

fn import_io_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot stage import source: {error}"),
        true,
    )
}
