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
use crate::validation::{ValidatedExportDestination, sanitize_display_name};
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
    destination: &ValidatedExportDestination,
    expected_size_bytes: u64,
    expected_sha256: &str,
) -> PlatformResult<()> {
    if !source_path.is_absolute() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let partial = std::ffi::OsString::from(format!(
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

        let mut temporary = create_export_partial(destination, &partial)?;

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
        let actual_sha256 = format!("{:x}", hasher.finalize());
        let partial_metadata = temporary
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if copied != expected_size_bytes
            || actual_sha256 != expected_sha256
            || !partial_metadata.is_file()
            || partial_metadata.len() != expected_size_bytes
        {
            return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
        }
        atomic_replace(destination, &temporary, &partial)?;
        drop(temporary);
        sync_export_parent(destination)?;
        Ok(())
    })();
    if result.is_err() {
        remove_export_partial(destination, &partial);
    }
    result
}

#[cfg(all(unix, any(target_os = "macos", test)))]
fn create_export_partial(
    destination: &ValidatedExportDestination,
    partial: &std::ffi::OsStr,
) -> PlatformResult<std::fs::File> {
    use rustix::fs::{Mode, OFlags, openat};

    let file = openat(
        destination.unix_parent(),
        partial,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    Ok(std::fs::File::from(file))
}

#[cfg(windows)]
fn create_export_partial(
    destination: &ValidatedExportDestination,
    partial: &std::ffi::OsStr,
) -> PlatformResult<std::fs::File> {
    destination.windows_parent().create_partial(partial)
}

#[cfg(all(unix, any(target_os = "macos", test)))]
fn atomic_replace(
    destination: &ValidatedExportDestination,
    _temporary: &std::fs::File,
    partial: &std::ffi::OsStr,
) -> PlatformResult<()> {
    rustix::fs::renameat(
        destination.unix_parent(),
        partial,
        destination.unix_parent(),
        destination.file_name(),
    )
    .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))
}

#[cfg(windows)]
fn atomic_replace(
    destination: &ValidatedExportDestination,
    temporary: &std::fs::File,
    partial: &std::ffi::OsStr,
) -> PlatformResult<()> {
    destination
        .windows_parent()
        .atomic_replace(temporary, partial, destination.file_name())
}

#[cfg(all(unix, any(target_os = "macos", test)))]
fn sync_export_parent(destination: &ValidatedExportDestination) -> PlatformResult<()> {
    rustix::fs::fsync(destination.unix_parent())
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))
}

#[cfg(windows)]
fn sync_export_parent(destination: &ValidatedExportDestination) -> PlatformResult<()> {
    destination.windows_parent().verify_identity()
}

#[cfg(all(unix, any(target_os = "macos", test)))]
fn remove_export_partial(destination: &ValidatedExportDestination, partial: &std::ffi::OsStr) {
    let _ = rustix::fs::unlinkat(
        destination.unix_parent(),
        partial,
        rustix::fs::AtFlags::empty(),
    );
}

#[cfg(windows)]
fn remove_export_partial(destination: &ValidatedExportDestination, partial: &std::ffi::OsStr) {
    destination.windows_parent().remove_partial(partial);
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
        let data_root = root.path().join("data");
        std::fs::create_dir(&data_root).expect("data root");
        let source = root.path().join("source");
        let destination_path = root.path().join("character.json");
        let destination =
            crate::validation::validate_content_export_destination(&destination_path, &data_root)
                .expect("validated destination");
        let bytes = b"lossless synthetic source";
        let digest = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(&source, bytes).expect("source");
        std::fs::write(&destination_path, b"previous bytes").expect("previous destination");

        atomic_export_to_destination(&source, &destination, bytes.len() as u64, &digest)
            .expect("export");
        assert_eq!(
            std::fs::read(&destination_path).expect("destination"),
            bytes
        );

        std::fs::write(&destination_path, b"keep me").expect("reset destination");
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
            std::fs::read(&destination_path).expect("preserved destination"),
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

    #[cfg(unix)]
    #[test]
    fn content_export_pins_parent_across_data_root_symlink_swap_after_validation() {
        let root = tempdir().expect("root");
        let data_root = root.path().join("data");
        let export_parent = root.path().join("exports");
        let displaced_parent = root.path().join("exports-before-swap");
        std::fs::create_dir(&data_root).expect("data root");
        std::fs::create_dir(&export_parent).expect("export parent");

        let source = root.path().join("source");
        let destination = export_parent.join("database.sqlite3");
        let protected = data_root.join("database.sqlite3");
        let bytes = b"verified export bytes";
        let digest = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(&source, bytes).expect("source");
        std::fs::write(&protected, b"protected database").expect("protected database");

        let destination =
            crate::validation::validate_content_export_destination(&destination, &data_root)
                .expect("initial external destination");
        std::fs::rename(&export_parent, &displaced_parent).expect("displace checked parent");
        std::os::unix::fs::symlink(&data_root, &export_parent)
            .expect("replace checked parent with data-root symlink");

        atomic_export_to_destination(&source, &destination, bytes.len() as u64, &digest)
            .expect("export through pinned checked parent");
        assert_eq!(
            std::fs::read(&protected).expect("protected database remains"),
            b"protected database"
        );
        assert_eq!(
            std::fs::read(displaced_parent.join("database.sqlite3"))
                .expect("pinned export destination"),
            bytes
        );
    }

    #[cfg(windows)]
    #[test]
    fn content_export_retains_windows_ancestor_handles_after_validation() {
        let root = tempdir().expect("root");
        let data_root = root.path().join("data");
        let export_ancestor = root.path().join("external");
        let export_parent = export_ancestor.join("exports");
        let displaced_ancestor = root.path().join("external-before-swap");
        std::fs::create_dir(&data_root).expect("data root");
        std::fs::create_dir_all(&export_parent).expect("export parent");

        let destination_path = export_parent.join("database.sqlite3");
        let _destination =
            crate::validation::validate_content_export_destination(&destination_path, &data_root)
                .expect("validated destination");

        assert!(
            std::fs::rename(&export_ancestor, &displaced_ancestor).is_err(),
            "retained Windows ancestor handles must deny the rename needed for a junction swap"
        );
        assert!(
            export_parent.is_dir(),
            "the validated parent must stay named"
        );
    }

    #[cfg(windows)]
    #[test]
    fn content_export_replaces_final_reparse_entry_without_following_it() {
        let root = tempdir().expect("root");
        let data_root = root.path().join("data");
        let export_parent = root.path().join("exports");
        std::fs::create_dir(&data_root).expect("data root");
        std::fs::create_dir(&export_parent).expect("export parent");

        let source = root.path().join("source");
        let protected = data_root.join("database.sqlite3");
        let destination_path = export_parent.join("database.sqlite3");
        let bytes = b"verified export bytes";
        let digest = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(&source, bytes).expect("source");
        std::fs::write(&protected, b"protected database").expect("protected database");
        std::os::windows::fs::symlink_file(&protected, &destination_path)
            .expect("create final reparse fixture");

        let destination =
            crate::validation::validate_content_export_destination(&destination_path, &data_root)
                .expect("validated external destination");
        atomic_export_to_destination(&source, &destination, bytes.len() as u64, &digest)
            .expect("replace the reparse entry itself");

        assert_eq!(
            std::fs::read(&protected).expect("protected database remains"),
            b"protected database"
        );
        assert!(
            !std::fs::symlink_metadata(&destination_path)
                .expect("destination metadata")
                .file_type()
                .is_symlink(),
            "the final reparse entry must be replaced, not followed"
        );
        assert_eq!(
            std::fs::read(&destination_path).expect("export destination"),
            bytes
        );
    }
}
