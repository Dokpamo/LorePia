//! Lock-safe CAS verification for completed package authorities.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    fs::File,
    io::Read,
};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::{database::Storage, verified_asset_cache::AssetFileSnapshot};

use super::{CompletedPackageAuthority, storage_corrupted, validate_sha256};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CompletedPackageAuthoritySnapshot {
    pub(super) authority: CompletedPackageAuthority,
    pub(super) cas_files: Vec<CompletedPackageCasFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletedPackageCasFile {
    pub(super) namespace: &'static str,
    pub(super) sha256: String,
    pub(super) size_bytes: u64,
    pub(super) relative_path: String,
}

/// A completed package authority whose exact CAS bytes were verified without
/// holding the repository-wide SQLite mutex.
///
/// Transaction-local consumers must still reload and compare the complete DB
/// snapshot before using it. Fields remain private so no caller can construct
/// or weaken this proof.
#[derive(Debug)]
pub(crate) struct VerifiedCompletedPackageAuthority {
    snapshot: CompletedPackageAuthoritySnapshot,
}

pub(crate) type VerifiedCompletedPackageAuthorities =
    BTreeMap<String, VerifiedCompletedPackageAuthority>;

impl Storage {
    /// Resolves a caller-supplied approval id into completed package authority.
    ///
    /// This intentionally refuses approved-but-uncommitted imports. Imported
    /// module activation therefore depends on the immutable commit evidence,
    /// not merely on possession of an approval-shaped string.
    pub fn get_completed_package_authority_by_approval_id(
        &self,
        approval_id: &str,
    ) -> CoreResult<CompletedPackageAuthority> {
        self.verify_completed_package_authority_with(
            approval_id,
            |connection, approval_id| {
                self.get_completed_package_authority_by_approval_id_in_connection(
                    connection,
                    approval_id,
                )
            },
            || {},
        )
        .map(|verified| verified.snapshot.authority)
    }

    /// Pre-verifies every distinct package approval before a caller opens a
    /// transaction that must consume the same exact authority.
    pub(crate) fn verify_completed_package_authorities<'a>(
        &self,
        approval_ids: impl IntoIterator<Item = &'a str>,
    ) -> CoreResult<VerifiedCompletedPackageAuthorities> {
        let approval_ids = approval_ids
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        approval_ids
            .into_iter()
            .map(|approval_id| {
                self.verify_completed_package_authority_with(
                    &approval_id,
                    |connection, approval_id| {
                        self.get_completed_package_authority_by_approval_id_in_connection(
                            connection,
                            approval_id,
                        )
                    },
                    || {},
                )
                .map(|verified| (approval_id, verified))
            })
            .collect()
    }

    pub(super) fn verify_completed_package_authority_with<Load, Observe>(
        &self,
        approval_id: &str,
        mut load: Load,
        before_cas_verification: Observe,
    ) -> CoreResult<VerifiedCompletedPackageAuthority>
    where
        Load: FnMut(&Connection, &str) -> CoreResult<CompletedPackageAuthoritySnapshot>,
        Observe: FnOnce(),
    {
        // Keep all internal CAS publication and cleanup excluded across both
        // DB snapshots, but never hold the SQLite mutex while hashing bytes.
        let _cas_mutation = self.cas_mutation()?;
        let initial = {
            let connection = self.connection()?;
            load(&connection, approval_id)?
        };
        before_cas_verification();
        let open_files = initial
            .cas_files
            .iter()
            .map(|file| {
                verify_owned_cas_file(
                    self,
                    file.namespace,
                    &file.sha256,
                    file.size_bytes,
                    &file.relative_path,
                )
                .map(|open| (file.namespace, open))
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let current = {
            let connection = self.connection()?;
            load(&connection, approval_id)?
        };
        if current != initial {
            return Err(storage_corrupted(
                "completed package authority changed during CAS verification",
            ));
        }
        for (namespace, file) in open_files {
            file.ensure_unchanged().map_err(|error| {
                storage_corrupted(format!(
                    "durable {namespace} CAS file changed during authority verification: {error}"
                ))
            })?;
        }
        Ok(VerifiedCompletedPackageAuthority { snapshot: current })
    }

    pub(super) fn revalidate_completed_package_authority_in_connection(
        &self,
        connection: &Connection,
        approval_id: &str,
        verified: &VerifiedCompletedPackageAuthority,
    ) -> CoreResult<CompletedPackageAuthority> {
        let current = self.get_completed_package_authority_by_approval_id_in_connection(
            connection,
            approval_id,
        )?;
        if current != verified.snapshot {
            return Err(storage_corrupted(
                "completed package authority changed after CAS verification",
            ));
        }
        Ok(current.authority)
    }
}

fn verify_owned_cas_file(
    storage: &Storage,
    namespace: &str,
    sha256: &str,
    expected_size: u64,
    stored_relative_path: &str,
) -> CoreResult<AssetFileSnapshot> {
    validate_sha256("CAS", sha256)?;
    let expected_relative = format!("{namespace}/sha256/{}/{}", &sha256[..2], &sha256[2..]);
    if stored_relative_path != expected_relative {
        return Err(storage_corrupted(format!(
            "stored {namespace} CAS path is not canonical"
        )));
    }
    let root = storage.data_root().join(namespace).join("sha256");
    let path = storage.data_root().join(stored_relative_path);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot inspect durable {namespace} CAS file: {error}"),
            true,
        )
    })?;
    if !metadata.file_type().is_file() || metadata.len() != expected_size {
        return Err(storage_corrupted(format!(
            "durable {namespace} CAS file is missing or has the wrong size"
        )));
    }
    let canonical_root = fs::canonicalize(root).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot resolve durable {namespace} CAS root: {error}"),
            true,
        )
    })?;
    let canonical_path = fs::canonicalize(path).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot resolve durable {namespace} CAS file: {error}"),
            true,
        )
    })?;
    if !canonical_path.starts_with(canonical_root) {
        return Err(storage_corrupted(format!(
            "durable {namespace} CAS file escapes its owned root"
        )));
    }
    let file = File::open(&canonical_path).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot open durable {namespace} CAS file: {error}"),
            true,
        )
    })?;
    let open_metadata = file.metadata().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot inspect open durable {namespace} CAS file: {error}"),
            true,
        )
    })?;
    if !open_metadata.is_file() || open_metadata.len() != expected_size {
        return Err(storage_corrupted(format!(
            "open durable {namespace} CAS file is missing or has the wrong size"
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        if open_metadata.nlink() != 1 {
            return Err(storage_corrupted(format!(
                "durable {namespace} CAS file must not have hard-link aliases"
            )));
        }
    }
    let mut file = AssetFileSnapshot::capture(file).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageUnavailable,
            format!("cannot snapshot durable {namespace} CAS file: {error}"),
            true,
        )
    })?;
    let mut digest = Sha256::new();
    let mut observed_size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.file_mut().read(&mut buffer).map_err(|error| {
            CoreError::new(
                CoreErrorCode::StorageUnavailable,
                format!("cannot read durable {namespace} CAS file: {error}"),
                true,
            )
        })?;
        if read == 0 {
            break;
        }
        observed_size = observed_size
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| storage_corrupted("CAS read size is out of range"))?,
            )
            .ok_or_else(|| storage_corrupted("CAS file size overflow"))?;
        if observed_size > expected_size {
            return Err(storage_corrupted(format!(
                "durable {namespace} CAS file grew during verification"
            )));
        }
        digest.update(&buffer[..read]);
    }
    if observed_size != expected_size || hex::encode(digest.finalize()) != sha256 {
        return Err(storage_corrupted(format!(
            "durable {namespace} CAS bytes do not match their reviewed digest"
        )));
    }
    file.ensure_unchanged().map_err(|error| {
        storage_corrupted(format!(
            "durable {namespace} CAS file changed while it was hashed: {error}"
        ))
    })?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use lorepia_domain::{CoreErrorCode, PackageId};
    use tempfile::tempdir;

    use crate::orchestration::PackageImportStatus;

    use super::*;

    fn empty_completed_authority_snapshot(
        approval_id: &str,
        source_sha256: &str,
        source_size: u64,
    ) -> CompletedPackageAuthoritySnapshot {
        CompletedPackageAuthoritySnapshot {
            authority: CompletedPackageAuthority {
                approval_id: approval_id.to_owned(),
                import_id: "package-import-lock-release".to_owned(),
                package_id: PackageId::from("package-lock-release"),
                status: PackageImportStatus::Completed,
                import_revision: 5,
                source_sha256: source_sha256.to_owned(),
                inspection_sha256: "11".repeat(32),
                selection_sha256: "22".repeat(32),
                capability_review_sha256: "33".repeat(32),
                approval_sha256: "44".repeat(32),
                required_capabilities: Vec::new(),
                approved_capabilities: Vec::new(),
                enabled_components: Vec::new(),
                committed_assets: Vec::new(),
            },
            cas_files: vec![CompletedPackageCasFile {
                namespace: "sources",
                sha256: source_sha256.to_owned(),
                size_bytes: source_size,
                relative_path: format!(
                    "sources/sha256/{}/{}",
                    &source_sha256[..2],
                    &source_sha256[2..]
                ),
            }],
        }
    }

    #[test]
    fn hashing_releases_sqlite_and_revalidates_exact_snapshot() {
        let root = tempdir().expect("data root");
        let storage = Storage::open(root.path()).expect("open storage");
        let source_bytes = b"completed authority lock-release fixture";
        let source_sha256 = super::super::sha256_hex(source_bytes);
        let source_size = u64::try_from(source_bytes.len()).expect("small source fixture");
        let source_path = root
            .path()
            .join("sources/sha256")
            .join(&source_sha256[..2])
            .join(&source_sha256[2..]);
        fs::create_dir_all(source_path.parent().expect("source prefix"))
            .expect("create source prefix");
        fs::write(&source_path, source_bytes).expect("write source CAS fixture");
        let snapshot = empty_completed_authority_snapshot(
            "approval-lock-release",
            &source_sha256,
            source_size,
        );
        let loads = Cell::new(0_u8);
        let sqlite_was_released = Cell::new(false);

        let verified = storage
            .verify_completed_package_authority_with(
                "approval-lock-release",
                |_, approval_id| {
                    assert_eq!(approval_id, "approval-lock-release");
                    loads.set(loads.get() + 1);
                    Ok(snapshot.clone())
                },
                || {
                    sqlite_was_released.set(storage.connection.try_lock().is_ok());
                },
            )
            .expect("verify completed authority");

        assert!(sqlite_was_released.get());
        assert_eq!(loads.get(), 2, "authority must be reloaded after hashing");
        assert_eq!(verified.snapshot, snapshot);

        let reloads = Cell::new(0_u8);
        let error = storage
            .verify_completed_package_authority_with(
                "approval-lock-release",
                |_, _| {
                    reloads.set(reloads.get() + 1);
                    let mut observed = snapshot.clone();
                    if reloads.get() == 2 {
                        observed.authority.import_revision += 1;
                    }
                    Ok(observed)
                },
                || {},
            )
            .expect_err("changed DB authority must fail closed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }
}
