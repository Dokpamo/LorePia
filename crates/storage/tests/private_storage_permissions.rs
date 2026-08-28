#[cfg(unix)]
mod unix_permissions {
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use lorepia_storage::Storage;
    use tempfile::tempdir;

    #[test]
    fn storage_open_normalizes_existing_owned_tree_to_private_modes() {
        let parent = tempdir().expect("temporary parent");
        let root = parent.path().join("data");
        drop(Storage::open(&root).expect("create storage"));

        for path in owned_tree_paths(&root) {
            let metadata = fs::symlink_metadata(&path).expect("owned path metadata");
            let public_mode = if metadata.is_dir() { 0o755 } else { 0o644 };
            fs::set_permissions(&path, fs::Permissions::from_mode(public_mode))
                .expect("make legacy mode public");
        }

        drop(Storage::open(&root).expect("reopen and harden storage"));

        for path in owned_tree_paths(&root) {
            let metadata = fs::symlink_metadata(&path).expect("hardened path metadata");
            let actual = metadata.permissions().mode() & 0o777;
            let expected = if metadata.is_dir() { 0o700 } else { 0o600 };
            assert_eq!(actual, expected, "unexpected mode for {}", path.display());
        }
    }

    fn owned_tree_paths(root: &Path) -> Vec<PathBuf> {
        let mut paths = vec![root.to_path_buf()];
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).expect("read owned directory") {
                let entry = entry.expect("owned entry");
                let path = entry.path();
                if entry.file_type().expect("owned entry type").is_dir() {
                    pending.push(path.clone());
                }
                paths.push(path);
            }
        }
        paths
    }
}
