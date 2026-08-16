use std::path::{Path, PathBuf};

use lorepia_domain::CoreErrorCode;
use lorepia_storage::Storage;
use rusqlite::{Connection, functions::FunctionFlags, params};
use tempfile::{TempDir, tempdir};

fn active_database_path(root: &Path) -> PathBuf {
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

fn fresh_database() -> (TempDir, PathBuf, u32) {
    let root = tempdir().expect("temporary data root");
    let storage = Storage::open(root.path()).expect("open fresh storage");
    let version = storage
        .schema_version()
        .expect("read fresh durable schema version");
    drop(storage);
    let database_path = active_database_path(root.path());
    (root, database_path, version)
}

fn assert_corrupted_open(root: &TempDir, expected_fragment: &str) {
    let Err(error) = Storage::open(root.path()) else {
        panic!("tampered migration registry unexpectedly opened");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    assert!(
        error.message.contains(expected_fragment),
        "unexpected registry error: {}",
        error.message
    );
}

fn register_integrity_function_names(connection: &Connection) {
    let flags = FunctionFlags::SQLITE_UTF8
        | FunctionFlags::SQLITE_DETERMINISTIC
        | FunctionFlags::SQLITE_INNOCUOUS;
    for (name, arguments) in [
        ("lorepia_sha256_hex", 1),
        ("lorepia_discovery_commit_plan_sha256", 1),
        ("lorepia_canonical_origin", 1),
        ("lorepia_header_name", 1),
        ("lorepia_native_no_effect_evidence_sha256", 8),
    ] {
        connection
            .create_scalar_function(name, arguments, flags, |_| {
                Ok::<Option<String>, rusqlite::Error>(None)
            })
            .expect("register integrity function name for raw registry tamper");
    }
}

#[test]
fn schema_version_reads_the_durable_registry_and_rejects_live_tampering() {
    {
        let root = tempdir().expect("temporary data root");
        let storage = Storage::open(root.path()).expect("open fresh storage");
        let current = storage
            .schema_version()
            .expect("read current durable schema version");
        assert!(current > 2, "fixture requires a nontrivial migration chain");

        Connection::open(active_database_path(root.path()))
            .expect("open registry gap tamper connection")
            .execute(
                "DELETE FROM schema_migrations WHERE version = ?1",
                [current - 1],
            )
            .expect("delete one durable migration row");

        let error = storage
            .schema_version()
            .expect_err("schema_version must re-read and reject the durable gap");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert!(error.message.contains("missing version"));
    }

    {
        let root = tempdir().expect("temporary data root");
        let storage = Storage::open(root.path()).expect("open fresh storage");
        let current = storage
            .schema_version()
            .expect("read current durable schema version");

        Connection::open(active_database_path(root.path()))
            .expect("open registry tail tamper connection")
            .execute(
                "DELETE FROM schema_migrations WHERE version = ?1",
                [current],
            )
            .expect("delete durable migration tail");

        let error = storage
            .schema_version()
            .expect_err("schema_version must reject a missing durable migration tail");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert!(error.message.contains("ended at schema"));
    }
}

#[test]
fn open_fails_closed_for_missing_gap_unknown_and_duplicate_registry_tampering() {
    {
        let (root, database_path, _current) = fresh_database();
        Connection::open(database_path)
            .expect("open missing-registry tamper connection")
            .execute("DROP TABLE schema_migrations", [])
            .expect("drop migration registry");
        assert_corrupted_open(&root, "migration registry is missing");
    }

    {
        let (root, database_path, current) = fresh_database();
        Connection::open(database_path)
            .expect("open gap tamper connection")
            .execute(
                "DELETE FROM schema_migrations WHERE version = ?1",
                [current - 1],
            )
            .expect("delete migration row");
        assert_corrupted_open(&root, "missing version");
    }

    {
        let (root, database_path, current) = fresh_database();
        Connection::open(database_path)
            .expect("open unknown-version tamper connection")
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)",
                params![current + 1, "2026-08-09T00:00:00Z"],
            )
            .expect("insert unsupported migration row");
        assert_corrupted_open(&root, "newer than supported schema");
    }

    {
        let (root, database_path, _current) = fresh_database();
        let connection =
            Connection::open(database_path).expect("open duplicate-version tamper connection");
        register_integrity_function_names(&connection);
        connection
            .execute_batch(
                "ALTER TABLE schema_migrations RENAME TO schema_migrations_original;
                 CREATE TABLE schema_migrations (
                     version INTEGER NOT NULL,
                     applied_at TEXT NOT NULL
                 );
                 INSERT INTO schema_migrations(version, applied_at)
                 SELECT version, applied_at FROM schema_migrations_original;
                 INSERT INTO schema_migrations(version, applied_at)
                 SELECT version, applied_at
                 FROM schema_migrations_original
                 WHERE version = 1;
                 DROP TABLE schema_migrations_original;",
            )
            .expect("replace registry with duplicate-capable tampered table");
        assert_corrupted_open(&root, "duplicate or non-monotonic version");
    }
}
