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

use crate::{
    database::Storage,
    verified_asset_cache::{AssetFileSnapshot, open_cas_file},
};

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
/// holding the repository-wide `SQLite` mutex.
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
                Self::get_completed_package_authority_by_approval_id_in_connection(
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
                        Self::get_completed_package_authority_by_approval_id_in_connection(
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
        // Seal each identity and release the handle immediately. A package can
        // contain thousands of assets, while macOS may permit only 256 FDs.
        // Reopen and compare every sealed identity after the DB snapshot check.
        let identities = initial
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
                .and_then(|open| {
                    open.into_identity_proof().map_err(|error| {
                        storage_corrupted(format!(
                            "durable CAS identity could not be sealed: {error}"
                        ))
                    })
                })
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
        for (file, identity) in initial.cas_files.iter().zip(&identities) {
            let reopened = open_owned_cas_file(
                self,
                file.namespace,
                &file.sha256,
                file.size_bytes,
                &file.relative_path,
            )?;
            let snapshot = AssetFileSnapshot::capture(reopened).map_err(|error| {
                storage_corrupted(format!("durable CAS identity could not be read: {error}"))
            })?;
            snapshot.verify_identity_proof(identity).map_err(|error| {
                storage_corrupted(format!(
                    "durable {} CAS file changed during authority verification: {error}",
                    file.namespace
                ))
            })?;
        }
        Ok(VerifiedCompletedPackageAuthority { snapshot: current })
    }

    pub(super) fn revalidate_completed_package_authority_in_connection(
        connection: &Connection,
        approval_id: &str,
        verified: &VerifiedCompletedPackageAuthority,
    ) -> CoreResult<CompletedPackageAuthority> {
        let current = Self::get_completed_package_authority_by_approval_id_in_connection(
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
    let file = open_owned_cas_file(
        storage,
        namespace,
        sha256,
        expected_size,
        stored_relative_path,
    )?;
    if namespace == "sources" {
        let current = AssetFileSnapshot::capture(file.try_clone().map_err(|error| {
            storage_corrupted(format!("cannot retain current source identity: {error}"))
        })?)
        .map_err(|error| {
            storage_corrupted(format!("cannot read current source identity: {error}"))
        })?;
        if let Some(verified) = super::source_verification_cache::lookup(sha256, &current)? {
            return Ok(verified);
        }
    }
    let verified = capture_and_hash_owned_cas_file(file, namespace, sha256, expected_size)?;
    if namespace == "sources" {
        super::source_verification_cache::insert(sha256, &verified)?;
    }
    Ok(verified)
}

fn open_owned_cas_file(
    storage: &Storage,
    namespace: &str,
    sha256: &str,
    expected_size: u64,
    stored_relative_path: &str,
) -> CoreResult<File> {
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
    let file = open_cas_file(storage.data_root(), namespace, sha256).map_err(|error| {
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
    Ok(file)
}

fn capture_and_hash_owned_cas_file(
    file: File,
    namespace: &str,
    sha256: &str,
    expected_size: u64,
) -> CoreResult<AssetFileSnapshot> {
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

    #[test]
    fn replacing_verified_bytes_with_an_identical_file_invalidates_authority() {
        let root = tempdir().expect("data root");
        let storage = Storage::open(root.path()).expect("storage");
        let bytes = b"immutable package source";
        let digest = super::super::sha256_hex(bytes);
        let snapshot =
            empty_completed_authority_snapshot("replacement", &digest, bytes.len() as u64);
        let path = root.path().join(&snapshot.cas_files[0].relative_path);
        fs::create_dir_all(path.parent().expect("prefix")).expect("prefix directory");
        fs::write(&path, bytes).expect("source");
        let calls = Cell::new(0);
        let error = storage
            .verify_completed_package_authority_with(
                "replacement",
                |_, _| {
                    calls.set(calls.get() + 1);
                    if calls.get() == 2 {
                        let replacement = path.with_extension("replacement");
                        fs::write(&replacement, bytes).expect("replacement");
                        fs::rename(&replacement, &path).expect("replace verified source");
                    }
                    Ok(snapshot.clone())
                },
                || {},
            )
            .expect_err("identical bytes on another identity must be re-reviewed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }

    #[test]
    fn warm_source_lease_rejects_same_size_mutation() {
        let root = tempdir().expect("data root");
        let storage = Storage::open(root.path()).expect("storage");
        let bytes = b"source lease mutation fixture";
        let digest = super::super::sha256_hex(bytes);
        let snapshot = empty_completed_authority_snapshot("warm", &digest, bytes.len() as u64);
        let path = root.path().join(&snapshot.cas_files[0].relative_path);
        fs::create_dir_all(path.parent().expect("prefix")).expect("prefix directory");
        fs::write(&path, bytes).expect("source");
        for _ in 0..2 {
            storage
                .verify_completed_package_authority_with("warm", |_, _| Ok(snapshot.clone()), || {})
                .expect("cold and warm source checks");
        }
        let mut changed = bytes.to_vec();
        changed[0] ^= 1;
        fs::write(&path, changed).expect("same-size mutation");
        let error = storage
            .verify_completed_package_authority_with("warm", |_, _| Ok(snapshot.clone()), || {})
            .expect_err("a retained lease cannot authorize changed bytes");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn large_package_verification_uses_bounded_file_descriptors() {
        const MARKER: &str = "LOREPIA_CAS_FD_REGRESSION_CHILD";
        if std::env::var_os(MARKER).is_none() {
            // Limit only the subprocess: parallel tests keep their original limit.
            let result = std::process::Command::new("sh")
                .args(["-c", "ulimit -n 128; exec \"$1\" --exact package_repository::completed_authority::tests::large_package_verification_uses_bounded_file_descriptors --nocapture", "cas-fd-test"])
                .arg(std::env::current_exe().expect("test executable"))
                .env(MARKER, "1")
                .output().expect("run descriptor-limited child");
            assert!(
                result.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        let root = tempdir().expect("data root");
        let storage = Storage::open(root.path()).expect("storage");
        let mut snapshot = empty_completed_authority_snapshot("large", &"aa".repeat(32), 1);
        snapshot.cas_files.clear();
        for index in 0..512 {
            let bytes = format!("synthetic asset {index}");
            let digest = super::super::sha256_hex(bytes.as_bytes());
            let file = CompletedPackageCasFile {
                namespace: "assets",
                relative_path: format!("assets/sha256/{}/{}", &digest[..2], &digest[2..]),
                sha256: digest,
                size_bytes: bytes.len() as u64,
            };
            let path = root.path().join(&file.relative_path);
            fs::create_dir_all(path.parent().expect("prefix")).expect("prefix directory");
            fs::write(path, bytes).expect("asset");
            snapshot.cas_files.push(file);
        }
        storage
            .verify_completed_package_authority_with("large", |_, _| Ok(snapshot.clone()), || {})
            .expect("512 assets with only 128 available descriptors");
        let path = root.path().join(&snapshot.cas_files[0].relative_path);
        fs::write(path, "tampered asset").expect("tamper");
        assert_eq!(
            storage
                .verify_completed_package_authority_with(
                    "large",
                    |_, _| Ok(snapshot.clone()),
                    || {}
                )
                .expect_err("tampering still fails closed")
                .code,
            CoreErrorCode::StorageCorrupted
        );
    }
}
