use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

#[cfg(windows)]
use std::fs::OpenOptions;

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};

use super::{
    cas_filesystem::sync_directory, private_path::harden_private_path, storage_corrupted,
    storage_io_error,
};

pub(super) fn prepare_owned_data_root(root: &Path) -> CoreResult<PathBuf> {
    if root.as_os_str().is_empty() {
        return Err(CoreError::invalid("data root must not be empty"));
    }
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(storage_corrupted(
                "data root must be a real directory, not a file or symbolic link",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(storage_io_error)?;
        }
        Err(error) => return Err(storage_io_error(error)),
    }
    let metadata = fs::symlink_metadata(root).map_err(storage_io_error)?;
    if !metadata.file_type().is_dir() {
        return Err(storage_corrupted(
            "data root must be a real directory, not a file or symbolic link",
        ));
    }
    harden_private_path(root, true)?;
    fs::canonicalize(root).map_err(storage_io_error)
}

fn validate_owner_lock_file(file: &File) -> CoreResult<()> {
    let metadata = file.metadata().map_err(storage_io_error)?;
    if !metadata.file_type().is_file() {
        return Err(storage_corrupted(
            "data root owner lock is not a regular file",
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn reject_non_regular_owner_lock(path: &Path) -> CoreResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(storage_corrupted(
            "data root owner lock is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage_io_error(error)),
    }
}

fn data_root_already_owned() -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        "data root is already owned by another LorePia process",
        true,
    )
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
pub(super) fn acquire_data_root_owner_lock(root: &Path) -> CoreResult<File> {
    use rustix::fs::{FlockOperation, Mode, OFlags, flock, open, openat};

    let lock_path = root.join(".lorepia-owner.lock");
    reject_non_regular_owner_lock(&lock_path)?;
    let root_fd = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(rustix_storage_io_error)?;
    let lock_fd = openat(
        &root_fd,
        ".lorepia-owner.lock",
        OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(rustix_storage_io_error)?;
    let file = File::from(lock_fd);
    validate_owner_lock_file(&file)?;
    flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
        let error = std::io::Error::from_raw_os_error(error.raw_os_error());
        if error.kind() == std::io::ErrorKind::WouldBlock {
            data_root_already_owned()
        } else {
            storage_io_error(error)
        }
    })?;
    Ok(file)
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn rustix_storage_io_error(error: rustix::io::Errno) -> CoreError {
    storage_io_error(std::io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(windows)]
pub(super) fn acquire_data_root_owner_lock(root: &Path) -> CoreResult<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let lock_path = root.join(".lorepia-owner.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&lock_path)
        .map_err(|error| {
            if matches!(error.raw_os_error(), Some(32 | 33)) {
                data_root_already_owned()
            } else if fs::symlink_metadata(&lock_path)
                .is_ok_and(|metadata| !metadata.file_type().is_file())
            {
                storage_corrupted("data root owner lock is not a regular file")
            } else {
                storage_io_error(error)
            }
        })?;
    validate_owner_lock_file(&file)?;
    Ok(file)
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_vendor = "apple",
    windows
)))]
pub(super) fn acquire_data_root_owner_lock(root: &Path) -> CoreResult<File> {
    let _ = root;
    Err(CoreError::new(
        CoreErrorCode::StorageUnavailable,
        "exclusive data root ownership is not supported on this platform",
        false,
    ))
}

pub(super) fn create_owned_directory_tree(root: &Path, relative: &Path) -> CoreResult<()> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(CoreError::internal(
                "owned storage directory must use a relative normal path",
            ));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(storage_corrupted(format!(
                    "owned storage path is not a real directory: {}",
                    current.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(storage_io_error)?;
                if let Some(parent) = current.parent() {
                    sync_directory(parent)?;
                }
            }
            Err(error) => return Err(storage_io_error(error)),
        }
        harden_private_path(&current, true)?;
    }
    sync_directory(&current)
}
