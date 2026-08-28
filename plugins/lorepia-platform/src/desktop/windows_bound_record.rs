use std::{
    fs::{File, OpenOptions},
    path::Path,
};

use crate::{PlatformError, PlatformErrorCode, PlatformResult};

#[cfg(windows)]
pub(super) fn open(options: &OpenOptions, path: &Path) -> PlatformResult<File> {
    // A losing no-replace claimant can briefly hold the winner's final record
    // while reconciling it. Retry only sharing/lock violations; every other
    // open failure remains fail-closed.
    const MAXIMUM_SHARING_RETRIES: usize = 64;
    for attempt in 0..=MAXIMUM_SHARING_RETRIES {
        match options.open(path) {
            Ok(file) => return Ok(file),
            Err(error)
                if attempt < MAXIMUM_SHARING_RETRIES
                    && matches!(error.raw_os_error(), Some(32 | 33)) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Err(_) => break,
        }
    }
    Err(recovery_required())
}

#[cfg(not(windows))]
pub(super) fn open(options: &OpenOptions, path: &Path) -> PlatformResult<File> {
    super::validate_windows_locator_file(
        &std::fs::symlink_metadata(path).map_err(|_| recovery_required())?,
    )?;
    options.open(path).map_err(|_| recovery_required())
}

fn recovery_required() -> PlatformError {
    PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired)
}
