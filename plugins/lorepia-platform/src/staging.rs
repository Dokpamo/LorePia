use std::{fs::OpenOptions, io::Read};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(any(target_os = "android", target_os = "macos", windows, test))]
use std::path::Path;
#[cfg(any(target_os = "macos", windows, test))]
use std::{io::Write, path::PathBuf};
#[cfg(any(target_os = "macos", windows, test))]
use uuid::Uuid;
#[cfg(any(target_os = "android", test))]
use zeroize::Zeroizing;

#[cfg(any(target_os = "macos", windows, test))]
use crate::validation::sanitize_display_name;
use crate::{PlatformError, PlatformErrorCode, PlatformResult, StagedImport};
#[cfg(any(target_os = "macos", windows, test))]
use sha2::{Digest, Sha256};

#[cfg(any(target_os = "android", test))]
pub(crate) const SENSITIVE_CAPTURE_DIRECTORY: &str = "sensitive-capture";
#[cfg(any(target_os = "android", test))]
pub(crate) const SENSITIVE_CAPTURE_PREFIX: &str = "lorepia-sensitive-";

#[cfg(any(target_os = "macos", windows, test))]
const COPY_BUFFER_BYTES: usize = 64 * 1024;
#[cfg(any(target_os = "macos", windows, test))]
pub(crate) const OWNED_STAGING_PREFIX: &str = "lorepia-tauri-";
#[cfg(any(target_os = "macos", windows, test))]
pub(crate) const CONTENT_EXPORT_TEMP_PREFIX: &str = ".lorepia-export-";

#[cfg(any(target_os = "macos", windows, test))]
pub(crate) fn stage_file(
    source_path: &Path,
    staging_root: &Path,
    maximum_bytes: u64,
) -> PlatformResult<StagedImport> {
    if maximum_bytes == 0 {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let source_metadata = std::fs::symlink_metadata(source_path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    if !source_metadata.file_type().is_file() {
        return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
    }
    if source_metadata.len() > maximum_bytes {
        return Err(PlatformError::new(PlatformErrorCode::SelectedFileTooLarge));
    }

    let display_name = source_path.file_name().map_or_else(
        || "selected-file".to_owned(),
        |name| sanitize_display_name(&name.to_string_lossy()),
    );
    let suffix = safe_suffix(&display_name);
    let basename = format!("{OWNED_STAGING_PREFIX}{}", Uuid::new_v4());
    let destination = staging_root.join(format!("{basename}{suffix}"));
    let partial = staging_root.join(format!("{basename}{suffix}.partial"));

    let result = copy_bounded(source_path, &partial, maximum_bytes).and_then(|copied| {
        std::fs::rename(&partial, &destination)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        Ok(StagedImport::new(destination.clone(), display_name, copied))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
        let _ = std::fs::remove_file(&destination);
    }
    result
}

pub(crate) fn read_staged_file(
    staged: &StagedImport,
    maximum_bytes: u64,
) -> PlatformResult<Vec<u8>> {
    if maximum_bytes == 0 || staged.size_bytes() > maximum_bytes {
        return Err(PlatformError::new(PlatformErrorCode::SelectedFileTooLarge));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let source = options
        .open(staged.path())
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    let metadata = source
        .metadata()
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if !metadata.is_file() || metadata.len() > maximum_bytes {
        return Err(PlatformError::new(PlatformErrorCode::SelectedFileTooLarge));
    }

    let bounded_length = maximum_bytes
        .checked_add(1)
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let mut bytes = Vec::new();
    source
        .take(bounded_length)
        .read_to_end(&mut bytes)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
        return Err(PlatformError::new(PlatformErrorCode::SelectedFileTooLarge));
    }
    Ok(bytes)
}

/// Copy a verified source into a same-directory temporary file and replace the
/// picker-selected destination only after the temporary bytes match exactly.
///
/// The picker owns overwrite consent. This helper never chooses a destination
/// and never exposes it outside the native Rust boundary.
#[cfg(any(target_os = "macos", windows, test))]
pub(crate) fn atomic_export_to_destination(
    source_path: &Path,
    destination: &Path,
    expected_size_bytes: u64,
    expected_sha256: &str,
) -> PlatformResult<()> {
    if !source_path.is_absolute() || !destination.is_absolute() || source_path == destination {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let parent = destination
        .parent()
        .filter(|parent| parent.is_absolute())
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let parent_metadata = std::fs::metadata(parent)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if !parent_metadata.is_dir() || destination.file_name().is_none() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    if destination.exists()
        && std::fs::canonicalize(destination)
            .ok()
            .zip(std::fs::canonicalize(source_path).ok())
            .is_some_and(|(destination, source)| destination == source)
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }

    let partial = parent.join(format!(
        "{CONTENT_EXPORT_TEMP_PREFIX}{}.partial",
        Uuid::new_v4()
    ));
    let result = (|| {
        let mut source_options = OpenOptions::new();
        source_options.read(true);
        #[cfg(unix)]
        source_options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let mut source = source_options
            .open(source_path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        let source_metadata = source
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if !source_metadata.is_file() || source_metadata.len() != expected_size_bytes {
            return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
        }

        let mut destination_options = OpenOptions::new();
        destination_options.write(true).create_new(true);
        #[cfg(unix)]
        destination_options
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
        let mut temporary = destination_options
            .open(&partial)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;

        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; COPY_BUFFER_BYTES].into_boxed_slice();
        let mut copied = 0_u64;
        loop {
            let read = source
                .read(&mut buffer)
                .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(u64::try_from(read).unwrap_or(u64::MAX))
                .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
            if copied > expected_size_bytes {
                return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
            }
            hasher.update(&buffer[..read]);
            temporary
                .write_all(&buffer[..read])
                .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        }
        temporary
            .flush()
            .and_then(|()| temporary.sync_all())
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        drop(temporary);

        let actual_sha256 = format!("{:x}", hasher.finalize());
        let partial_metadata = std::fs::symlink_metadata(&partial)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if copied != expected_size_bytes
            || actual_sha256 != expected_sha256
            || !partial_metadata.file_type().is_file()
            || partial_metadata.len() != expected_size_bytes
        {
            return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
        }

        atomic_replace(&partial, destination)?;
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

#[cfg(any(target_os = "macos", all(test, not(windows))))]
fn atomic_replace(source: &Path, destination: &Path) -> PlatformResult<()> {
    std::fs::rename(source, destination)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))
}

#[cfg(windows)]
fn atomic_replace(source: &Path, destination: &Path) -> PlatformResult<()> {
    crate::desktop::windows::atomic_replace_file(source, destination)
}

#[cfg(any(target_os = "android", test))]
pub(crate) fn consume_sensitive_capture(
    path: &Path,
    data_root: &Path,
    expected_size: u64,
    maximum_bytes: u64,
) -> PlatformResult<Vec<u8>> {
    let capture_root = data_root.join(SENSITIVE_CAPTURE_DIRECTORY);
    let valid_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(SENSITIVE_CAPTURE_PREFIX));
    let owned_capture_path =
        path.is_absolute() && path.parent() == Some(capture_root.as_path()) && valid_name;
    if !owned_capture_path
        || maximum_bytes == 0
        || expected_size == 0
        || expected_size > maximum_bytes
    {
        if owned_capture_path {
            let _ = std::fs::remove_file(path);
        }
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let result = (|| {
        let source = options
            .open(path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        let metadata = source
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if !metadata.is_file() || metadata.len() != expected_size || metadata.len() > maximum_bytes
        {
            return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
        }
        let bounded_length = maximum_bytes
            .checked_add(1)
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
        let mut bytes = Zeroizing::new(Vec::new());
        source
            .take(bounded_length)
            .read_to_end(&mut bytes)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_size {
            return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
        }
        Ok(std::mem::take(&mut *bytes))
    })();
    let removed = std::fs::remove_file(path).is_ok();
    match (result, removed) {
        (Ok(bytes), true) => Ok(bytes),
        (Ok(mut bytes), false) => {
            use zeroize::Zeroize;
            bytes.zeroize();
            Err(PlatformError::new(PlatformErrorCode::StorageUnavailable))
        }
        (Err(error), _) => Err(error),
    }
}

#[cfg(any(target_os = "macos", windows, test))]
fn copy_bounded(
    source_path: &Path,
    partial_path: &Path,
    maximum_bytes: u64,
) -> PlatformResult<u64> {
    let mut source_options = OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    source_options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut source = source_options
        .open(source_path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    let metadata = source
        .metadata()
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    if !metadata.is_file() {
        return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
    }

    let mut destination_options = OpenOptions::new();
    destination_options.write(true).create_new(true);
    #[cfg(unix)]
    destination_options.mode(0o600);
    let mut destination = destination_options
        .open(partial_path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;

    let mut copied = 0_u64;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        if count == 0 {
            break;
        }
        copied = copied
            .checked_add(count as u64)
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectedFileTooLarge))?;
        if copied > maximum_bytes {
            return Err(PlatformError::new(PlatformErrorCode::SelectedFileTooLarge));
        }
        destination
            .write_all(&buffer[..count])
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    }
    destination
        .sync_all()
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    Ok(copied)
}

#[cfg(any(target_os = "macos", windows, test))]
fn safe_suffix(display_name: &str) -> &'static str {
    match PathBuf::from(display_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("charx") => ".charx",
        Some("zip") => ".zip",
        Some("json") => ".json",
        _ => ".pending",
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::tempdir;

    use sha2::{Digest, Sha256};

    use super::{
        CONTENT_EXPORT_TEMP_PREFIX, SENSITIVE_CAPTURE_DIRECTORY, atomic_export_to_destination,
        consume_sensitive_capture, read_staged_file, stage_file,
    };

    #[test]
    fn stage_file_uses_random_app_owned_name_and_enforces_limit() {
        let root = tempdir().expect("root");
        let source = root.path().join("private-name.json");
        std::fs::File::create(&source)
            .and_then(|mut file| file.write_all(b"synthetic"))
            .expect("write source");
        let staging = root.path().join("staging");
        std::fs::create_dir(&staging).expect("create staging");

        let staged = stage_file(&source, &staging, 9).expect("stage");
        assert_eq!(staged.display_name(), "private-name.json");
        assert_eq!(staged.size_bytes(), 9);
        assert_eq!(staged.path().parent(), Some(staging.as_path()));
        assert_ne!(staged.path().file_name(), source.file_name());
        assert_eq!(
            read_staged_file(&staged, 9).expect("read staged"),
            b"synthetic"
        );
        assert!(read_staged_file(&staged, 8).is_err());
        assert!(stage_file(&source, &staging, 8).is_err());
    }

    #[test]
    fn rejected_sensitive_capture_never_deletes_an_unowned_path() {
        let root = tempdir().expect("root");
        let unowned = root.path().join("unowned");
        std::fs::write(&unowned, b"synthetic").expect("write unowned file");

        assert!(
            consume_sensitive_capture(&unowned, root.path(), 9, 9).is_err(),
            "an unowned path must be rejected"
        );
        assert_eq!(
            std::fs::read(&unowned).expect("unowned file remains"),
            b"synthetic"
        );

        let owned_root = root.path().join(SENSITIVE_CAPTURE_DIRECTORY);
        std::fs::create_dir(&owned_root).expect("create owned root");
        let malformed_owned = owned_root.join("wrong-prefix");
        std::fs::write(&malformed_owned, b"synthetic").expect("write malformed owned file");
        assert!(
            consume_sensitive_capture(&malformed_owned, root.path(), 9, 9).is_err(),
            "an unowned filename must be rejected"
        );
        assert!(
            malformed_owned.exists(),
            "a rejected unowned filename must not be deleted"
        );
    }

    #[test]
    fn content_export_verifies_before_atomic_replacement_and_cleans_temporary_files() {
        let root = tempdir().expect("root");
        let source = root.path().join("source");
        let destination = root.path().join("character.json");
        let bytes = b"lossless synthetic source";
        let digest = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(&source, bytes).expect("source");
        std::fs::write(&destination, b"previous bytes").expect("previous destination");

        atomic_export_to_destination(&source, &destination, bytes.len() as u64, &digest)
            .expect("export");
        assert_eq!(std::fs::read(&destination).expect("destination"), bytes);

        std::fs::write(&destination, b"keep me").expect("reset destination");
        assert!(
            atomic_export_to_destination(
                &source,
                &destination,
                bytes.len() as u64,
                &"0".repeat(64),
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(&destination).expect("preserved destination"),
            b"keep me"
        );
        let leaked = std::fs::read_dir(root.path())
            .expect("entries")
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(CONTENT_EXPORT_TEMP_PREFIX)
            });
        assert!(
            !leaked,
            "failed exports must clean same-directory temporary files"
        );
    }
}
