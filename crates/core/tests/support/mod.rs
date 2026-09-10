use std::{
    io,
    path::{Path, PathBuf},
};

#[allow(dead_code)] // Each integration test compiles this shared support module independently.
pub(crate) fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = std::fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(entry.path().join("generation-manifest.json"))
                    .expect("read generation manifest"),
            )
            .expect("parse generation manifest");
            let sequence = manifest["activation_sequence"]
                .as_u64()
                .expect("generation activation sequence");
            let relative = manifest["active_database_relative_path"]
                .as_str()
                .expect("active database relative path")
                .to_owned();
            (sequence, relative)
        })
        .max_by_key(|(sequence, _)| *sequence)
        .expect("at least one committed database generation");
    root.join(relative)
}

// Windows deliberately opens the live owner lock with no sharing. Secret-scan
// tests may skip only that exact live lock while inspecting every other file.
#[allow(dead_code)] // Only secret-scanning integration tests need this platform exception.
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
