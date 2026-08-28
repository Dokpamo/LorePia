use std::{io, path::Path};

// Windows deliberately opens the live owner lock with no sharing. Secret-scan
// tests may skip only that exact live lock while inspecting every other file.
pub(crate) fn is_live_owner_lock_sharing_violation(path: &Path, error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        path.file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new(".lorepia-owner.lock"))
            && matches!(error.raw_os_error(), Some(32 | 33))
    }

    #[cfg(not(windows))]
    {
        let _ = (path, error);
        false
    }
}
