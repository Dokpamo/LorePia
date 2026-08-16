//! Filesystem-level regression for the version-5 discovery redaction upgrade.

use std::{fs, path::Path};

use lorepia_storage::Storage;
use rusqlite::{Connection, params};
use tempfile::tempdir;

const MIGRATION_0001: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_import_asset_recovery.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_conversation_branches.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_provider_catalog.sql");
const MIGRATION_0005: &str = include_str!("../migrations/0005_discovery_state_machine.sql");
const NOW: &str = "2026-07-31T00:00:00Z";
const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SECRET: &[u8] = b"sk-proj-wal-remanence-canary";

#[test]
fn v5_redaction_truncates_wal_and_removes_legacy_secret_bytes() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    seed_version_four_database(&database_path);

    assert!(
        contains_bytes(&database_path, SECRET),
        "fixture must place the legacy secret in the main database"
    );
    let legacy_connection = Connection::open(&database_path).expect("open legacy WAL writer");
    legacy_connection
        .pragma_update(None, "journal_mode", "WAL")
        .expect("enable legacy WAL");
    legacy_connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .expect("disable legacy auto-checkpoint");
    legacy_connection
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("start with an empty legacy WAL");
    legacy_connection
        .execute(
            "UPDATE provider_discovery_sessions
             SET draft_json = json_object('legacy_value', ?1)",
            [std::str::from_utf8(SECRET).expect("ASCII secret")],
        )
        .expect("write the legacy secret to an uncheckpointed WAL frame");
    let wal_path = database_path.with_extension("sqlite3-wal");
    assert!(
        contains_bytes(&wal_path, SECRET),
        "fixture must place the legacy secret in the uncheckpointed WAL"
    );

    let storage = Storage::open(root.path()).expect("migrate version-four storage");
    assert!(
        storage
            .schema_version()
            .expect("read durable schema version")
            >= 20,
        "the migrated database must include the package journal schema"
    );
    let sessions = storage
        .list_discovery_sessions(10)
        .expect("hydrate redacted migrated sessions");
    assert_eq!(sessions.len(), 1);
    assert_eq!(
        sessions[0].created_at.to_rfc3339(),
        "1970-01-01T00:00:00+00:00"
    );
    assert!(
        storage
            .list_discovery_evidence(&sessions[0].session.id, 10)
            .expect("list purged legacy evidence")
            .is_empty()
    );
    drop(storage);
    drop(legacy_connection);
    let reopened = Storage::open(root.path()).expect("reopen migrated storage");
    assert_eq!(
        reopened
            .list_discovery_sessions(10)
            .expect("rehydrate migrated sessions")
            .len(),
        1
    );
    let active_database_path = active_database_path(root.path());
    drop(reopened);

    for path in [
        active_database_path.clone(),
        active_database_path.with_extension("sqlite3-wal"),
    ] {
        if path.exists() {
            assert!(
                !contains_bytes(&path, SECRET),
                "legacy discovery secret remained in {}",
                path.display()
            );
        }
    }
    assert!(
        contains_bytes(&database_path, SECRET),
        "the immutable canonical rollback source must not be rewritten in place"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn v5_redaction_is_checkpointed_before_a_later_migration_failure() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    seed_version_four_database(&database_path);

    // Force migration six to fail at its final statement, after both columns,
    // both indexes, and the first trigger have been created in its transaction.
    // Immutable-generation cutover must discard that unpublished candidate
    // without modifying the version-four rollback source.
    Connection::open(&database_path)
        .expect("open failure fixture")
        .execute_batch(
            "CREATE TRIGGER generations_provider_target_update_guard
             BEFORE UPDATE ON generations
             BEGIN
                 SELECT 1;
             END;",
        )
        .expect("prepare final schema-six trigger conflict");
    assert!(
        Storage::open(root.path()).is_err(),
        "the deliberately conflicting schema-six migration must fail"
    );

    {
        let connection = Connection::open(&database_path).expect("inspect failed migration");
        assert_eq!(
            connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get::<_, u32>(0)
                })
                .expect("schema version"),
            4,
            "failed copy-forward migration must leave the canonical source unchanged"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'trigger'
                       AND name = 'generations_provider_target_update_guard'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("conflicting trigger count"),
            1
        );
        connection
            .execute("DROP TRIGGER generations_provider_target_update_guard", [])
            .expect("remove deliberate migration conflict");
    }
    assert_no_database_generations(root.path());
    assert!(
        contains_bytes(&database_path, SECRET),
        "the immutable canonical rollback source must not be rewritten in place"
    );

    let reopened = Storage::open(root.path()).expect("resume migration after repairing conflict");
    assert!(
        reopened
            .schema_version()
            .expect("read durable schema version")
            >= 20,
        "the resumed database must include the package journal schema"
    );
    let active_database_path = active_database_path(root.path());
    drop(reopened);
    for path in [
        active_database_path.clone(),
        active_database_path.with_extension("sqlite3-wal"),
    ] {
        if path.exists() {
            assert!(
                !contains_bytes(&path, SECRET),
                "legacy discovery secret reappeared after successful reopen in {}",
                path.display()
            );
        }
    }
}

fn active_database_path(root: &Path) -> std::path::PathBuf {
    let cutover = root.join("db/schema-cutover");
    let manifests = fs::read_dir(cutover)
        .expect("read committed database generations")
        .map(|entry| entry.expect("read database generation entry").path())
        .filter(|generation| generation.join("generation-committed.json").is_file())
        .map(|generation| generation.join("generation-manifest.json"))
        .collect::<Vec<_>>();
    assert_eq!(
        manifests.len(),
        1,
        "this fixture must publish exactly one database generation"
    );
    let manifest = serde_json::from_slice::<serde_json::Value>(
        &fs::read(&manifests[0]).expect("read committed generation manifest"),
    )
    .expect("parse committed generation manifest");
    root.join(
        manifest["active_database_relative_path"]
            .as_str()
            .expect("active database relative path"),
    )
}

fn assert_no_database_generations(root: &Path) {
    let cutover = root.join("db/schema-cutover");
    let entries = match fs::read_dir(cutover) {
        Ok(entries) => entries.count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("read database generation directory: {error}"),
    };
    assert_eq!(
        entries, 0,
        "failed migration must remove its unpublished database generation"
    );
}

#[test]
fn version_five_database_migrates_to_current_schema_and_reopens() {
    let root = tempdir().expect("temporary data root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    seed_version_four_database(&database_path);
    {
        let mut connection = Connection::open(&database_path).expect("open version-four fixture");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("enable foreign keys");
        let transaction = connection.transaction().expect("version-five transaction");
        transaction
            .execute_batch(MIGRATION_0005)
            .expect("apply migration five");
        transaction
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (5, ?1)",
                [NOW],
            )
            .expect("record migration five");
        transaction.commit().expect("commit migration five");
    }

    let storage = Storage::open(root.path()).expect("migrate version-five storage");
    assert!(
        storage
            .schema_version()
            .expect("read durable schema version")
            >= 20,
        "the migrated database must include the package journal schema"
    );
    let sessions = storage
        .list_discovery_sessions(10)
        .expect("hydrate migrated session");
    assert_eq!(sessions.len(), 1);
    assert!(
        storage
            .list_discovery_evidence(&sessions[0].session.id, 10)
            .expect("list migrated evidence")
            .is_empty()
    );
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen current storage");
    assert!(
        reopened
            .schema_version()
            .expect("read durable schema version")
            >= 20,
        "the reopened database must include the package journal schema"
    );
    assert_eq!(
        reopened
            .list_discovery_sessions(10)
            .expect("rehydrate reopened session")
            .len(),
        1
    );
}

fn seed_version_four_database(database_path: &Path) {
    let connection = Connection::open(database_path).expect("open version-four fixture");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    connection
        .execute_batch(MIGRATION_0001)
        .expect("apply migration 1");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?1)",
            [NOW],
        )
        .expect("record migration 1");
    connection
        .execute_batch(MIGRATION_0002)
        .expect("apply migration 2");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (2, ?1)",
            [NOW],
        )
        .expect("record migration 2");
    connection
        .execute_batch(MIGRATION_0003)
        .expect("apply migration 3");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (3, ?1)",
            [NOW],
        )
        .expect("record migration 3");
    connection
        .execute_batch(MIGRATION_0004)
        .expect("apply migration 4");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (4, ?1)",
            [NOW],
        )
        .expect("record migration 4");

    let secret = std::str::from_utf8(SECRET).expect("ASCII secret");
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, draft_json,
                 created_at, updated_at
             ) VALUES (
                 ?1, 'fetching_documents',
                 json_object(
                     'site_url', 'https://provider.example/',
                     'credential_ref', ?1,
                     'unexpected_secret', ?1
                 ),
                 json_object('authorization', ?1),
                 ?1, ?1
             )",
            [secret],
        )
        .expect("insert legacy secret session");
    connection
        .execute(
            "INSERT INTO provider_discovery_evidence (
                 id, session_id, kind, source_url, content_sha256,
                 extracted_json, fetched_at
             ) VALUES (
                 ?1, ?1, ?1,
                 'https://provider.example/docs', ?2,
                 json_object('api_key', ?1), ?1
             )",
            params![secret, HASH],
        )
        .expect("insert legacy secret evidence");
}

fn contains_bytes(path: &Path, needle: &[u8]) -> bool {
    fs::read(path)
        .expect("read database artifact")
        .windows(needle.len())
        .any(|window| window == needle)
}
