use std::{
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Seek, Write},
    path::Path,
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::{storage_corrupted, storage_io_error};

/// Stores an exact CAS object and returns whether this call published the
/// destination. Existing or concurrently won destinations return `false`.
pub(super) fn store_verified_source(
    source: &Path,
    final_path: &Path,
    cas_root: &Path,
    expected_sha256: &str,
    expected_size: u64,
) -> CoreResult<bool> {
    store_verified_source_observed(
        source,
        final_path,
        cas_root,
        expected_sha256,
        expected_size,
        || {},
    )
}

pub(super) fn store_verified_source_observed(
    source: &Path,
    final_path: &Path,
    cas_root: &Path,
    expected_sha256: &str,
    expected_size: u64,
    before_file_copy: impl FnOnce(),
) -> CoreResult<bool> {
    let parent = final_path
        .parent()
        .ok_or_else(|| CoreError::internal("source path has no parent"))?;
    create_and_sync_cas_directory(cas_root, parent)?;

    match fs::symlink_metadata(final_path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            verify_file(final_path, expected_sha256, expected_size)?;
            sync_file_and_parent(final_path, parent)?;
            return Ok(false);
        }
        Ok(_) => {
            return Err(storage_corrupted(
                "content-addressed destination is not a regular file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(storage_io_error(error)),
    }

    let temp_path = parent.join(format!(".{}.partial", Uuid::new_v4()));
    before_file_copy();
    let copy_result = copy_and_hash(source, &temp_path);
    let (actual_sha256, actual_size) = match copy_result {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
    };
    if actual_sha256 != expected_sha256 || actual_size != expected_size {
        let _ = fs::remove_file(&temp_path);
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "staging source changed while it was being committed",
            false,
        ));
    }

    let published = match publish_temp_noclobber(&temp_path, final_path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::remove_file(&temp_path).map_err(storage_io_error)?;
            ensure_regular_file(final_path)?;
            verify_file(final_path, expected_sha256, expected_size)?;
            false
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            return Err(storage_io_error(error));
        }
    };

    sync_file_and_parent(final_path, parent)?;
    Ok(published)
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
pub(super) fn publish_temp_noclobber(temp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, temp_path, CWD, final_path, RenameFlags::NOREPLACE)
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
pub(super) fn publish_temp_noclobber(temp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    fs::hard_link(temp_path, final_path)?;
    fs::remove_file(temp_path)
}

fn create_and_sync_cas_directory(cas_root: &Path, path: &Path) -> CoreResult<()> {
    let relative = path
        .strip_prefix(cas_root)
        .map_err(|_| CoreError::internal("CAS destination escaped its owned root"))?;
    if relative.components().count() != 1 {
        return Err(CoreError::internal(
            "CAS destination must have exactly one hash-prefix directory",
        ));
    }
    ensure_real_directory(cas_root)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(storage_corrupted(
                "CAS hash-prefix path is not a real directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(storage_io_error)?;
            sync_directory(cas_root)?;
        }
        Err(error) => return Err(storage_io_error(error)),
    }
    ensure_real_directory(path)?;
    sync_directory(path)
}

pub(super) fn ensure_real_directory(path: &Path) -> CoreResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(storage_io_error)?;
    if !metadata.file_type().is_dir() {
        return Err(storage_corrupted(format!(
            "owned CAS path is not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn ensure_regular_file(path: &Path) -> CoreResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(storage_io_error)?;
    if !metadata.file_type().is_file() {
        return Err(storage_corrupted(
            "content-addressed destination is not a regular file",
        ));
    }
    Ok(())
}

fn sync_file_and_parent(file_path: &Path, parent: &Path) -> CoreResult<()> {
    sync_file(file_path).map_err(storage_io_error)?;
    sync_directory(parent)
}

#[cfg(not(windows))]
fn sync_file(path: &Path) -> std::io::Result<()> {
    File::open(path).and_then(|file| file.sync_all())
}

#[cfg(windows)]
fn sync_file(path: &Path) -> std::io::Result<()> {
    // FlushFileBuffers requires a handle with write access on Windows.
    OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
}

#[cfg(unix)]
pub(super) fn sync_directory(path: &Path) -> CoreResult<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(storage_io_error)
}

#[cfg(windows)]
pub(super) fn sync_directory(path: &Path) -> CoreResult<()> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let result = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .and_then(|directory| directory.sync_all());
    match result {
        Ok(()) => Ok(()),
        // Windows does not guarantee FlushFileBuffers support for directory
        // handles on every filesystem. The CAS file itself was synced above.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(storage_io_error(error)),
    }
}

#[cfg(not(any(unix, windows)))]
pub(super) fn sync_directory(_path: &Path) -> CoreResult<()> {
    Ok(())
}

fn copy_and_hash(source: &Path, destination: &Path) -> CoreResult<(String, u64)> {
    let source = File::open(source).map_err(storage_io_error)?;
    let mut destination_options = OpenOptions::new();
    destination_options.create_new(true).write(true);
    #[cfg(unix)]
    destination_options.mode(0o600);
    let destination = destination_options
        .open(destination)
        .map_err(storage_io_error)?;
    let mut reader = BufReader::new(source);
    let mut writer = destination;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(storage_io_error)?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(storage_io_error)?;
        digest.update(&buffer[..read]);
        size = size
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| CoreError::internal("source byte count overflow"))?,
            )
            .ok_or_else(|| CoreError::internal("source size overflow"))?;
    }
    writer.flush().map_err(storage_io_error)?;
    writer.sync_all().map_err(storage_io_error)?;
    Ok((hex::encode(digest.finalize()), size))
}

pub(super) fn verify_file(
    path: &Path,
    expected_sha256: &str,
    expected_size: u64,
) -> CoreResult<()> {
    let (actual_sha256, actual_size) = hash_file(path)?;
    if actual_sha256 != expected_sha256 || actual_size != expected_size {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "content-addressed source does not match its recorded digest",
            false,
        ));
    }
    Ok(())
}

fn hash_file(path: &Path) -> CoreResult<(String, u64)> {
    let mut source = File::open(path).map_err(storage_io_error)?;
    hash_open_file(&mut source)
}

pub(super) fn hash_open_file(source: &mut File) -> CoreResult<(String, u64)> {
    source.rewind().map_err(storage_io_error)?;
    let mut reader = BufReader::new(source);
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(storage_io_error)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size = size
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| CoreError::internal("source byte count overflow"))?,
            )
            .ok_or_else(|| CoreError::internal("source size overflow"))?;
    }
    Ok((hex::encode(digest.finalize()), size))
}

pub(super) fn content_relative_path(hash: &str) -> CoreResult<String> {
    if hash.len() != 64 || !hash.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(CoreError::invalid(
            "source hash is not a SHA-256 hex digest",
        ));
    }
    Ok(format!("sha256/{}/{}", &hash[..2], &hash[2..]))
}
