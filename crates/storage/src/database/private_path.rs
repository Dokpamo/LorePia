use std::path::Path;

#[cfg(unix)]
use std::{fs, os::unix::fs::PermissionsExt};

use lorepia_domain::CoreResult;

#[cfg(unix)]
use super::{storage_corrupted, storage_io_error};

#[cfg(unix)]
pub(super) fn harden_private_path(path: &Path, directory: bool) -> CoreResult<()> {
    let mode = if directory { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(storage_io_error)
}

#[cfg(not(unix))]
// Keep a fallible cross-platform contract so callers cannot skip a future
// Windows hardening failure when native ACL enforcement is added.
#[allow(clippy::unnecessary_wraps)]
pub(super) fn harden_private_path(_path: &Path, _directory: bool) -> CoreResult<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn harden_owned_tree_permissions(root: &Path) -> CoreResult<()> {
    const MAXIMUM_OWNED_PATHS: usize = 1_000_000;

    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0_usize;
    while let Some(path) = pending.pop() {
        visited = visited
            .checked_add(1)
            .ok_or_else(|| storage_corrupted("owned storage tree is too large"))?;
        if visited > MAXIMUM_OWNED_PATHS {
            return Err(storage_corrupted("owned storage tree is too large"));
        }
        let metadata = fs::symlink_metadata(&path).map_err(storage_io_error)?;
        if metadata.file_type().is_symlink() {
            return Err(storage_corrupted(format!(
                "owned storage tree contains a symbolic link: {}",
                path.display()
            )));
        }
        if metadata.file_type().is_dir() {
            harden_private_path(&path, true)?;
            for entry in fs::read_dir(&path).map_err(storage_io_error)? {
                pending.push(entry.map_err(storage_io_error)?.path());
            }
        } else if metadata.file_type().is_file() {
            harden_private_path(&path, false)?;
        } else {
            return Err(storage_corrupted(format!(
                "owned storage tree contains a special file: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
// This mirrors the Unix implementation at the shared call site; Windows ACL
// hardening is intentionally tracked as follow-up work.
#[allow(clippy::unnecessary_wraps)]
pub(super) fn harden_owned_tree_permissions(_root: &Path) -> CoreResult<()> {
    Ok(())
}
