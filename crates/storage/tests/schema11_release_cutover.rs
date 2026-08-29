use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use lorepia_domain::{Conversation, ConversationMode};
use lorepia_storage::Storage;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const FROZEN_SCHEMA_VERSION: u32 = 11;
const FIXTURE_SQL: &str =
    include_str!("../../../testdata/tauri-upgrade/native-schema-11/schema-11.sql");
const SOURCE_PACKAGE: &[u8] = include_bytes!("../../../testdata/packages/with-avatar.charx");
const AVATAR_ASSET: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/assets/avatar.png");
const SOURCE_SHA256: &str = "2c528a64fbf36a011e29c1a692cd13568b83f76e764ea03487393c28a2e666de";
const AVATAR_SHA256: &str = "aa7bb0431aaeb198a77c26a14fe6dd714a75e4d7db94e3e1238a1fdcbfe1f8d4";

#[test]
fn fresh_install_publishes_current_database_as_an_immutable_generation() {
    let root = tempdir().expect("temporary fresh-install Core root");
    let storage = Storage::open(root.path()).expect("open fresh storage");
    let current_schema = storage.schema_version().expect("read fresh schema");
    drop(storage);

    let canonical = root.path().join("db/lorepia.sqlite3");
    let active = active_database_path(root.path());
    assert_ne!(
        active, canonical,
        "even a fresh current database must become an immutable committed generation"
    );
    assert_eq!(database_schema_version(&canonical), current_schema);
    assert_eq!(database_schema_version(&active), current_schema);
    assert_eq!(committed_generation_count(root.path()), 1);
}

#[test]
fn fresh_install_from_previous_release_is_copy_forwarded_on_the_next_schema() {
    let seed = tempdir().expect("temporary previous-release seed root");
    let current = Storage::open(seed.path()).expect("create current fresh database");
    let current_schema = current.schema_version().expect("read current schema");
    drop(current);
    assert!(current_schema > FROZEN_SCHEMA_VERSION + 1);

    let seed_canonical = seed.path().join("db/lorepia.sqlite3");
    downgrade_latest_schema_by_one(&seed_canonical, current_schema);
    let previous_schema = database_schema_version(&seed_canonical);
    assert_eq!(previous_schema + 1, current_schema);

    let root = tempdir().expect("temporary simulated next-schema root");
    let canonical = root.path().join("db/lorepia.sqlite3");
    fs::create_dir_all(canonical.parent().expect("canonical parent"))
        .expect("create simulated previous-release database directory");
    fs::copy(&seed_canonical, &canonical).expect("install simulated previous-release canonical");
    let canonical_before = file_sha256(&canonical);

    let upgraded = Storage::open(root.path()).expect("copy forward previous-release canonical");
    assert_eq!(
        upgraded.schema_version().expect("read upgraded schema"),
        current_schema
    );
    drop(upgraded);
    assert_eq!(database_schema_version(&canonical), previous_schema);
    assert_eq!(file_sha256(&canonical), canonical_before);
    let active = active_database_path(root.path());
    assert_eq!(database_schema_version(&active), current_schema);
    assert_eq!(committed_generation_count(root.path()), 1);
}

#[test]
fn frozen_schema_eleven_cutover_keeps_canonical_database_byte_identical() {
    let root = tempdir().expect("temporary frozen Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    let before_sha256 = file_sha256(&canonical);

    let storage = Storage::open(root.path()).expect("cut over frozen schema eleven root");
    let active_schema = storage
        .schema_version()
        .expect("read active schema after cutover");
    assert!(
        active_schema > FROZEN_SCHEMA_VERSION,
        "the current Core must open a schema newer than the frozen baseline"
    );
    drop(storage);

    assert_eq!(
        database_schema_version(&canonical),
        FROZEN_SCHEMA_VERSION,
        "cutover must never migrate the frozen canonical database in place"
    );
    assert_eq!(
        file_sha256(&canonical),
        before_sha256,
        "cutover must leave the clean frozen canonical SQLite bytes unchanged"
    );
    let active = active_database_path(root.path());
    assert_eq!(
        database_schema_version(&active),
        active_schema,
        "the manifest must select the fully migrated current schema"
    );
    assert_fixture_semantics(&active);
}

#[test]
fn malformed_frozen_settings_never_publish_a_poisoned_generation() {
    let root = tempdir().expect("temporary malformed-settings Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    let connection = Connection::open(&canonical).expect("open malformed-settings fixture");
    let valid_settings = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("read valid frozen settings");
    connection
        .execute(
            "UPDATE app_settings SET value_json = '{' WHERE key = 'application'",
            [],
        )
        .expect("inject malformed frozen settings");
    checkpoint_and_close(connection);
    let malformed_sha256 = file_sha256(&canonical);

    let Err(error) = Storage::open(root.path()) else {
        panic!("malformed settings must reject cutover");
    };
    assert_eq!(error.code.as_str(), "storage_corrupted");
    assert_eq!(database_schema_version(&canonical), FROZEN_SCHEMA_VERSION);
    assert_eq!(file_sha256(&canonical), malformed_sha256);
    assert_eq!(
        committed_generation_count(root.path()),
        0,
        "a candidate that production startup cannot read must not be committed"
    );

    let connection = Connection::open(&canonical).expect("reopen malformed-settings fixture");
    connection
        .execute(
            "UPDATE app_settings SET value_json = ?1 WHERE key = 'application'",
            [valid_settings],
        )
        .expect("repair frozen settings");
    checkpoint_and_close(connection);
    drop(Storage::open(root.path()).expect("retry cutover after repairing settings"));
    assert_eq!(committed_generation_count(root.path()), 1);
}

#[test]
fn concurrent_canonical_writer_is_excluded_until_generation_publication() {
    let root = tempdir().expect("temporary concurrent-writer Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    let paused = spawn_paused_cutover(root.path());
    let writer = Connection::open(&canonical).expect("open concurrent canonical writer");
    writer
        .busy_timeout(Duration::ZERO)
        .expect("disable concurrent writer wait");
    let write = writer.execute(
        "UPDATE conversations
         SET title = 'Concurrent stale source write'
         WHERE id = '2faca127-ff70-4acb-b26f-60f1042b8d11'",
        [],
    );
    drop(writer);
    paused.finish();

    assert!(
        matches!(
            write,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if matches!(
                    error.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        ),
        "the source writer must be excluded until the generation is durably published: {write:?}"
    );
    drop(Storage::open(root.path()).expect("immediately reopen committed generation"));
    assert_eq!(
        conversation_title(&canonical),
        "Synthetic continuity conversation"
    );
}

#[test]
fn sealed_source_writer_is_excluded_through_startup_validation() {
    let root = tempdir().expect("temporary sealed-source validation root");
    install_frozen_schema_eleven_fixture(root.path());
    drop(Storage::open(root.path()).expect("publish sealed source generation"));
    let canonical = root.path().join("db/lorepia.sqlite3");
    let paused = spawn_paused_storage_open(root.path(), "after_sealed_source_fingerprint");
    let writer = Connection::open(&canonical).expect("open sealed source writer");
    writer
        .busy_timeout(Duration::ZERO)
        .expect("disable sealed writer wait");
    let write = writer.execute(
        "UPDATE conversations
         SET title = 'Raced sealed-source validation'
         WHERE id = '2faca127-ff70-4acb-b26f-60f1042b8d11'",
        [],
    );
    drop(writer);
    paused.finish();

    assert!(
        matches!(
            write,
            Err(rusqlite::Error::SqliteFailure(error, _))
                if matches!(
                    error.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                )
        ),
        "sealed source must remain write-reserved until validation selects the active generation: {write:?}"
    );
}

#[test]
fn previous_release_generation_pins_source_and_asset_cas() {
    for namespace in ["sources", "assets"] {
        let root = tempdir().expect("temporary previous-release CAS root");
        let (sha256, cas_path) = install_previous_release_cas_fixture(root.path(), namespace);
        drop(Storage::open(root.path()).expect("cut over previous-release CAS fixture"));
        let (_, manifest) = selected_generation_manifest(root.path());
        assert_eq!(
            manifest["rollback_cas_pin_count"].as_u64(),
            Some(1),
            "{namespace} CAS inventory must be pinned for the sealed previous release"
        );

        let mut tampered = fs::read(&cas_path).expect("read previous-release CAS bytes");
        tampered[0] ^= 0xff;
        fs::write(&cas_path, tampered).expect("tamper previous-release CAS bytes");
        let Err(error) = Storage::open(root.path()) else {
            panic!("tampered previous-release rollback CAS must fail closed");
        };
        assert_eq!(error.code.as_str(), "storage_corrupted");
        assert!(
            error.message.contains("rollback CAS object"),
            "{namespace} rollback CAS failure must identify the pinned object: {}",
            error.message
        );
        assert_eq!(sha256.len(), 64);
    }
}

#[test]
fn previous_release_generation_pins_file_backed_package_cas_journal_entries() {
    for namespace in ["source", "asset"] {
        let root = tempdir().expect("temporary previous-release journal CAS root");
        let (sha256, cas_path) =
            install_previous_release_journal_cas_fixture(root.path(), namespace);
        drop(Storage::open(root.path()).expect("cut over previous-release journal CAS fixture"));
        let (_, manifest) = selected_generation_manifest(root.path());
        assert_eq!(
            manifest["rollback_cas_pin_count"].as_u64(),
            Some(1),
            "{namespace} file-durable journal CAS must be part of the rollback inventory"
        );

        let mut tampered = fs::read(&cas_path).expect("read journal-only rollback CAS bytes");
        tampered[0] ^= 0xff;
        fs::write(&cas_path, tampered).expect("tamper journal-only rollback CAS bytes");
        let Err(error) = Storage::open(root.path()) else {
            panic!("tampered journal-only rollback CAS must fail closed");
        };
        assert_eq!(error.code.as_str(), "storage_corrupted");
        assert!(
            error.message.contains("rollback CAS object"),
            "{namespace} journal-only rollback CAS failure must identify the object: {}",
            error.message
        );
        assert_eq!(sha256.len(), 64);
    }
}

#[test]
fn source_manifest_records_wal_visible_logical_page_span() {
    let root = tempdir().expect("temporary logical-size Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    let writer = spawn_dirty_wal_writer(root.path());
    let main_file_size = fs::metadata(&canonical)
        .expect("inspect canonical main file")
        .len();
    let source =
        Connection::open_with_flags(&canonical, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open WAL-visible source read-only");
    let page_count = source
        .query_row("PRAGMA page_count", [], |row| row.get::<_, u64>(0))
        .expect("read WAL-visible page count");
    let page_size = source
        .query_row("PRAGMA page_size", [], |row| row.get::<_, u64>(0))
        .expect("read source page size");
    let logical_page_span = page_count
        .checked_mul(page_size)
        .expect("logical page span");
    drop(source);

    drop(Storage::open(root.path()).expect("cut over WAL-visible logical size fixture"));
    let (_, manifest) = selected_generation_manifest(root.path());
    assert_eq!(
        manifest["source_database_size_bytes"].as_u64(),
        Some(logical_page_span),
        "the manifest size must describe the same WAL-visible SQLite snapshot as its fingerprint"
    );
    assert!(
        logical_page_span > main_file_size,
        "the helper must extend the logical database beyond the unchanged main file"
    );
    writer.finish();
}

#[test]
fn sqlite_sequence_drift_is_bound_by_the_source_fingerprint() {
    let root = tempdir().expect("temporary sqlite-sequence Core root");
    install_frozen_schema_eleven_fixture(root.path());
    drop(Storage::open(root.path()).expect("cut over sqlite-sequence fixture"));
    let canonical = root.path().join("db/lorepia.sqlite3");
    let connection = Connection::open(&canonical).expect("open frozen sqlite-sequence source");
    let updated = connection
        .execute(
            "UPDATE sqlite_sequence SET seq = seq + 1
             WHERE name = 'provider_discovery_audit_log'",
            [],
        )
        .expect("tamper source sqlite sequence");
    assert_eq!(updated, 1, "frozen fixture must exercise sqlite_sequence");
    checkpoint_and_close(connection);

    let Err(error) = Storage::open(root.path()) else {
        panic!("sqlite_sequence drift must invalidate the sealed source");
    };
    assert_eq!(error.code.as_str(), "storage_corrupted");
    assert!(error.message.contains("generation source diverged"));
}

#[cfg(unix)]
#[test]
fn canonical_database_symlink_is_rejected_by_the_sqlite_open_boundary() {
    use std::os::unix::fs::symlink;

    let root = tempdir().expect("temporary canonical symlink Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    let relocated = root.path().join("db/relocated-canonical.sqlite3");
    fs::rename(&canonical, &relocated).expect("relocate canonical database");
    symlink(&relocated, &canonical).expect("replace canonical database with symlink");

    let Err(error) = Storage::open(root.path()) else {
        panic!("SQLite cutover must reject a symlink database path");
    };
    assert!(matches!(
        error.code.as_str(),
        "storage_corrupted" | "storage_unavailable"
    ));
}

#[test]
fn cutover_includes_committed_source_wal_without_rewriting_canonical_files() {
    let root = tempdir().expect("temporary dirty-WAL Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    let canonical_before_writer = file_sha256(&canonical);
    let writer = spawn_dirty_wal_writer(root.path());
    let wal = canonical.with_extension("sqlite3-wal");
    let shm = canonical.with_extension("sqlite3-shm");

    assert!(
        fs::metadata(&wal)
            .expect("inspect committed source WAL")
            .len()
            > 32,
        "the helper must leave at least one committed frame in the WAL"
    );
    assert!(
        fs::metadata(&shm).expect("inspect source SHM").len() > 0,
        "the helper must keep the WAL shared-memory index live"
    );
    assert_eq!(
        file_sha256(&canonical),
        canonical_before_writer,
        "the committed source write must still reside outside the main database"
    );
    let canonical_before_cutover = file_sha256(&canonical);
    let wal_before_cutover = file_sha256(&wal);

    let storage = Storage::open(root.path()).expect("cut over dirty committed source WAL");
    let active = active_database_path(root.path());
    assert_eq!(
        conversation_title(&active),
        "Committed only in frozen source WAL",
        "SQLite online backup must include committed WAL state"
    );
    assert_eq!(
        file_sha256(&canonical),
        canonical_before_cutover,
        "cutover must not rewrite the canonical main database"
    );
    assert_eq!(
        file_sha256(&wal),
        wal_before_cutover,
        "cutover must not checkpoint or append to the canonical WAL"
    );
    drop(storage);
    writer.finish();

    checkpoint_and_close(
        Connection::open(&canonical).expect("recover and checkpoint terminated source WAL"),
    );
    let reopened =
        Storage::open(root.path()).expect("reopen after benign source WAL recovery checkpoint");
    assert_eq!(
        conversation_title(&active_database_path(root.path())),
        "Committed only in frozen source WAL"
    );
    drop(reopened);
}

#[cfg(unix)]
#[test]
fn active_generation_ancestor_symlink_fails_closed() {
    use std::os::unix::fs::symlink;

    let root = tempdir().expect("temporary symlink-tamper Core root");
    install_frozen_schema_eleven_fixture(root.path());
    drop(Storage::open(root.path()).expect("create active candidate generation"));
    let active = active_database_path(root.path());
    let generation = active.parent().expect("active generation directory");
    let relocated_generation = root.path().join("db/relocated-active-generation");
    fs::rename(generation, &relocated_generation).expect("relocate active generation directory");
    symlink(&relocated_generation, generation).expect("replace generation ancestor with symlink");

    let Err(error) = Storage::open(root.path()) else {
        panic!("an active path below a symlink ancestor must fail closed");
    };
    assert_eq!(error.code.as_str(), "storage_corrupted");
}

#[test]
fn candidate_post_write_and_reopen_preserve_frozen_source_and_avatar_cas() {
    let root = tempdir().expect("temporary CAS continuity Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    let canonical_sha256 = file_sha256(&canonical);
    let source = fixture_cas_path(root.path(), "sources", SOURCE_SHA256);
    let avatar = fixture_cas_path(root.path(), "assets", AVATAR_SHA256);
    let source_sha256 = file_sha256(&source);
    let avatar_sha256 = file_sha256(&avatar);

    let storage = Storage::open(root.path()).expect("cut over frozen CAS fixture");
    let character = storage
        .list_characters()
        .expect("list cut-over characters")
        .into_iter()
        .next()
        .expect("frozen fixture character");
    assert_eq!(character.source_hash, SOURCE_SHA256);
    assert_eq!(character.avatar_asset_hash.as_deref(), Some(AVATAR_SHA256));
    let conversation = Conversation::new(&character.id, "Post-cutover CAS continuity");
    storage
        .save_conversation_with_mode(&conversation, ConversationMode::Chat)
        .expect("persist candidate post-cutover write");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen active candidate");
    assert_eq!(
        reopened
            .get_conversation(&conversation.id)
            .expect("read candidate post-cutover conversation")
            .title,
        "Post-cutover CAS continuity"
    );
    let reopened_character = reopened
        .get_character(&character.id)
        .expect("read frozen character after candidate reopen");
    assert_eq!(reopened_character.source_hash, SOURCE_SHA256);
    assert_eq!(
        reopened_character.avatar_asset_hash.as_deref(),
        Some(AVATAR_SHA256)
    );
    drop(reopened);

    assert_eq!(file_sha256(&canonical), canonical_sha256);
    assert_eq!(file_sha256(&source), source_sha256);
    assert_eq!(file_sha256(&avatar), avatar_sha256);
    assert_eq!(source_sha256, SOURCE_SHA256);
    assert_eq!(avatar_sha256, AVATAR_SHA256);
}

#[test]
fn tampered_frozen_rollback_cas_objects_fail_closed_before_candidate_reopen() {
    for (namespace, sha256, label) in [
        ("sources", SOURCE_SHA256, "source"),
        ("assets", AVATAR_SHA256, "avatar"),
    ] {
        let root = tempdir().expect("temporary rollback-CAS tamper Core root");
        install_frozen_schema_eleven_fixture(root.path());
        drop(Storage::open(root.path()).expect("create rollback-CAS-pinned generation"));
        let active = active_database_path(root.path());
        let path = fixture_cas_path(root.path(), namespace, sha256);
        let mut tampered = fs::read(&path).expect("read rollback CAS object");
        tampered[0] ^= 0xff;
        fs::write(&path, tampered).expect("tamper rollback CAS object in place");

        let Err(error) = Storage::open(root.path()) else {
            panic!("rollback {label} CAS tampering must fail closed");
        };
        assert_eq!(error.code.as_str(), "storage_corrupted");
        assert!(
            error.message.contains("rollback CAS object"),
            "rollback {label} CAS corruption must be classified at the pinned object: {}",
            error.message
        );
        assert_eq!(
            conversation_title(&active),
            "Synthetic continuity conversation",
            "{label} CAS validation failure must not mutate the committed candidate"
        );
    }
}

#[test]
fn crash_cutpoints_recover_without_publishing_partial_active_generations() {
    for failpoint in [
        "after_backup",
        "after_migrations",
        "after_candidate_sync",
        "after_generation_manifest",
        "after_generation_commit",
    ] {
        let root = tempdir().expect("temporary crash-cutpoint Core root");
        install_frozen_schema_eleven_fixture(root.path());
        let canonical = root.path().join("db/lorepia.sqlite3");
        let canonical_sha256 = file_sha256(&canonical);

        let output = Command::new(
            env::current_exe().expect("current schema cutover test executable for crash helper"),
        )
        .args([
            "--exact",
            "cutover_crash_helper",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("LOREPIA_TEST_CUTOVER_ROOT", root.path())
        .env("LOREPIA_TEST_CUTOVER_FAILPOINT", failpoint)
        .stdin(Stdio::null())
        .output()
        .expect("run cutover crash helper");
        assert!(!output.status.success(), "{failpoint} must terminate early");
        assert_eq!(
            output.status.code(),
            Some(86),
            "unexpected {failpoint} helper exit\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(database_schema_version(&canonical), FROZEN_SCHEMA_VERSION);
        assert_eq!(
            file_sha256(&canonical),
            canonical_sha256,
            "{failpoint} must leave the frozen canonical database byte-identical"
        );

        drop(
            Storage::open(root.path())
                .unwrap_or_else(|error| panic!("recover after {failpoint}: {error:?}")),
        );
        assert_eq!(database_schema_version(&canonical), FROZEN_SCHEMA_VERSION);
        assert_eq!(file_sha256(&canonical), canonical_sha256);
        assert_eq!(
            committed_generation_count(root.path()),
            1,
            "recovery after {failpoint} must select exactly one committed generation"
        );
        assert_no_uncommitted_generation_directories(root.path());
        let active = active_database_path(root.path());
        assert!(database_schema_version(&active) > FROZEN_SCHEMA_VERSION);
        assert_fixture_semantics(&active);
    }
}

#[test]
#[ignore = "subprocess helper for cutover crash-point recovery"]
fn cutover_crash_helper() {
    let Some(root) = env::var_os("LOREPIA_TEST_CUTOVER_ROOT") else {
        return;
    };
    match Storage::open(PathBuf::from(root)) {
        Ok(_) => panic!("cutover completed without triggering the requested failpoint"),
        Err(error) => panic!("cutover failed before triggering the requested failpoint: {error:?}"),
    }
}

#[test]
#[ignore = "subprocess helper for the committed-WAL cutover regression"]
fn dirty_wal_writer_helper() {
    if env::var_os("LOREPIA_SCHEMA11_DIRTY_WAL_HELPER").is_none() {
        return;
    }
    let root = PathBuf::from(
        env::var_os("LOREPIA_SCHEMA11_DIRTY_WAL_ROOT")
            .expect("dirty-WAL helper root environment variable"),
    );
    let ready = PathBuf::from(
        env::var_os("LOREPIA_SCHEMA11_DIRTY_WAL_READY")
            .expect("dirty-WAL helper ready environment variable"),
    );
    let release = PathBuf::from(
        env::var_os("LOREPIA_SCHEMA11_DIRTY_WAL_RELEASE")
            .expect("dirty-WAL helper release environment variable"),
    );
    let canonical = root.join("db/lorepia.sqlite3");
    let connection = Connection::open(&canonical).expect("open dirty-WAL source writer");
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .expect("keep source in WAL mode");
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .expect("disable source auto-checkpoint");
    connection
        .execute(
            "UPDATE conversations
             SET title = 'Committed only in frozen source WAL'
             WHERE id = '2faca127-ff70-4acb-b26f-60f1042b8d11'",
            [],
        )
        .expect("commit source update into WAL");
    connection
        .execute(
            "INSERT INTO conversations
             (id, character_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![
                "ccf21311-c5a0-4fbc-918d-5ac0bd5ed1e3",
                "3c0ffb8c-e24b-48a0-ad31-d0455665c290",
                "w".repeat(256 * 1024),
                "2026-08-12T00:00:00+00:00",
            ],
        )
        .expect("extend the source database through committed WAL pages");
    fs::write(&ready, b"ready").expect("publish dirty-WAL helper readiness");

    let deadline = Instant::now() + Duration::from_mins(2);
    while !release.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(release.exists(), "parent did not release dirty-WAL helper");
    std::process::exit(0);
}

#[test]
#[ignore = "subprocess helper for concurrent cutover writer exclusion"]
fn paused_cutover_helper() {
    let Some(root) = env::var_os("LOREPIA_TEST_PAUSED_CUTOVER_ROOT") else {
        return;
    };
    drop(Storage::open(PathBuf::from(root)).expect("complete paused cutover"));
}

#[test]
fn failed_schema_eleven_upgrade_leaves_no_partial_active_database() {
    let root = tempdir().expect("temporary frozen Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    let connection = Connection::open(&canonical).expect("open failure-injection fixture");
    connection
        .execute_batch("CREATE TABLE prompt_blocks (conflict INTEGER);")
        .expect("inject migration-thirteen object conflict");
    checkpoint_and_close(connection);
    let before_sha256 = file_sha256(&canonical);

    let Err(error) = Storage::open(root.path()) else {
        panic!("conflicting schema-eleven root must not open");
    };
    assert!(
        !error.message.is_empty(),
        "failed cutover must return a classified storage error"
    );
    assert_eq!(
        database_schema_version(&canonical),
        FROZEN_SCHEMA_VERSION,
        "a failed cutover must not commit an intermediate registry prefix"
    );
    assert_eq!(
        file_sha256(&canonical),
        before_sha256,
        "a failed cutover must not change the frozen canonical database"
    );
    assert_eq!(
        committed_generation_count(root.path()),
        0,
        "an incomplete candidate must never publish a generation commit marker"
    );
}

#[test]
fn abandoned_partial_candidate_is_ignored_without_an_active_manifest() {
    let root = tempdir().expect("temporary frozen Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let abandoned_relative =
        "db/schema-cutover/00000000-0000-4000-8000-000000000001/lorepia.sqlite3";
    let abandoned = root.path().join(abandoned_relative);
    fs::create_dir_all(abandoned.parent().expect("abandoned candidate parent"))
        .expect("create abandoned candidate directory");
    let connection = Connection::open(&abandoned).expect("create abandoned candidate");
    connection
        .execute_batch(
            "CREATE TABLE schema_migrations (
                 version INTEGER PRIMARY KEY,
                 applied_at TEXT NOT NULL
             );
             INSERT INTO schema_migrations(version, applied_at)
             VALUES (12, 'partial-candidate');",
        )
        .expect("seed abandoned intermediate registry");
    drop(connection);

    drop(Storage::open(root.path()).expect("recover from abandoned inactive candidate"));

    assert_eq!(
        database_schema_version(&root.path().join("db/lorepia.sqlite3")),
        FROZEN_SCHEMA_VERSION
    );
    assert_ne!(
        active_database_path(root.path()),
        abandoned,
        "a partial candidate without a manifest must never become active"
    );
    assert!(
        !abandoned.exists(),
        "startup recovery must remove an unreferenced partial candidate"
    );
}

#[test]
fn tampered_active_manifest_fails_closed_without_falling_back_to_legacy() {
    let root = tempdir().expect("temporary frozen Core root");
    install_frozen_schema_eleven_fixture(root.path());
    let canonical = root.path().join("db/lorepia.sqlite3");
    drop(Storage::open(root.path()).expect("create active candidate manifest"));
    let canonical_sha256 = file_sha256(&canonical);
    let manifest_path = active_generation_manifest_path(root.path());
    let mut manifest = serde_json::from_slice::<serde_json::Value>(
        &fs::read(&manifest_path).expect("read active manifest"),
    )
    .expect("parse active manifest");
    manifest["checksum_sha256"] = serde_json::Value::String("00".repeat(32));
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("encode tampered manifest"),
    )
    .expect("tamper active manifest");

    let Err(error) = Storage::open(root.path()) else {
        panic!("tampered active manifest must fail closed");
    };
    assert_eq!(error.code.as_str(), "storage_corrupted");
    assert_eq!(database_schema_version(&canonical), FROZEN_SCHEMA_VERSION);
    assert_eq!(file_sha256(&canonical), canonical_sha256);
}

#[test]
fn canonical_write_after_cutover_fails_closed_and_preserves_both_generations() {
    let root = tempdir().expect("temporary frozen Core root");
    install_frozen_schema_eleven_fixture(root.path());
    drop(Storage::open(root.path()).expect("create active candidate manifest"));
    let canonical = root.path().join("db/lorepia.sqlite3");
    let active = active_database_path(root.path());
    let connection = Connection::open(&canonical).expect("open frozen rollback database");
    connection
        .execute(
            "UPDATE conversations
             SET title = 'Written by frozen native rollback'
             WHERE id = '2faca127-ff70-4acb-b26f-60f1042b8d11'",
            [],
        )
        .expect("simulate frozen native post-rollback write");
    checkpoint_and_close(connection);

    let Err(error) = Storage::open(root.path()) else {
        panic!("canonical drift must not silently select a stale candidate");
    };
    assert_eq!(error.code.as_str(), "storage_corrupted");
    assert!(
        error.message.contains("diverged")
            && error
                .message
                .contains("preserve both canonical and active generations"),
        "canonical drift must give the two-generation recovery instruction: {}",
        error.message
    );
    assert_eq!(
        conversation_title(&active),
        "Synthetic continuity conversation",
        "failed drift validation must not mutate the active candidate"
    );
}

fn install_frozen_schema_eleven_fixture(root: &Path) {
    let database_dir = root.join("db");
    fs::create_dir_all(&database_dir).expect("create frozen database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let connection = Connection::open(&database_path).expect("open frozen fixture database");
    connection
        .execute_batch(FIXTURE_SQL)
        .expect("restore frozen schema-eleven SQL fixture");
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .expect("restore frozen WAL mode");
    connection
        .pragma_update(None, "synchronous", "FULL")
        .expect("restore frozen synchronous mode");
    checkpoint_and_close(connection);

    write_fixture_cas_object(root, "sources", SOURCE_SHA256, SOURCE_PACKAGE);
    write_fixture_cas_object(root, "assets", AVATAR_SHA256, AVATAR_ASSET);
}

fn write_fixture_cas_object(root: &Path, namespace: &str, sha256: &str, bytes: &[u8]) {
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), sha256);
    let path = fixture_cas_path(root, namespace, sha256);
    let directory = path.parent().expect("fixture CAS parent");
    fs::create_dir_all(directory).expect("create fixture CAS directory");
    fs::write(path, bytes).expect("write fixture CAS object");
}

fn fixture_cas_path(root: &Path, namespace: &str, sha256: &str) -> PathBuf {
    root.join(namespace)
        .join("sha256")
        .join(&sha256[..2])
        .join(&sha256[2..])
}

struct DirtyWalWriter {
    child: Option<Child>,
    release: PathBuf,
}

struct PausedCutover {
    child: Option<Child>,
    release: PathBuf,
}

impl PausedCutover {
    fn finish(mut self) {
        fs::write(&self.release, b"release").expect("release paused cutover");
        let status = self
            .child
            .take()
            .expect("paused cutover child")
            .wait()
            .expect("wait for paused cutover");
        assert!(status.success(), "paused cutover failed: {status}");
    }
}

impl Drop for PausedCutover {
    fn drop(&mut self) {
        let _ = fs::write(&self.release, b"release");
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.wait();
    }
}

impl DirtyWalWriter {
    fn finish(mut self) {
        fs::write(&self.release, b"release").expect("release dirty-WAL helper");
        let status = self
            .child
            .take()
            .expect("dirty-WAL helper child")
            .wait()
            .expect("wait for dirty-WAL helper");
        assert!(status.success(), "dirty-WAL helper failed: {status}");
    }
}

impl Drop for DirtyWalWriter {
    fn drop(&mut self) {
        let _ = fs::write(&self.release, b"release");
        let Some(mut child) = self.child.take() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
    }
}

fn spawn_dirty_wal_writer(root: &Path) -> DirtyWalWriter {
    let ready = root.join("dirty-wal-writer.ready");
    let release = root.join("dirty-wal-writer.release");
    let child = Command::new(env::current_exe().expect("current schema cutover test executable"))
        .args([
            "--exact",
            "dirty_wal_writer_helper",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("LOREPIA_SCHEMA11_DIRTY_WAL_HELPER", "1")
        .env("LOREPIA_SCHEMA11_DIRTY_WAL_ROOT", root)
        .env("LOREPIA_SCHEMA11_DIRTY_WAL_READY", &ready)
        .env("LOREPIA_SCHEMA11_DIRTY_WAL_RELEASE", &release)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn dirty-WAL helper");
    let mut writer = DirtyWalWriter {
        child: Some(child),
        release,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if ready.exists() {
            return writer;
        }
        let status = writer
            .child
            .as_mut()
            .expect("dirty-WAL helper child")
            .try_wait()
            .expect("poll dirty-WAL helper");
        assert!(
            status.is_none(),
            "dirty-WAL helper exited before publishing readiness: {status:?}"
        );
        assert!(
            Instant::now() < deadline,
            "dirty-WAL helper did not publish readiness"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn spawn_paused_cutover(root: &Path) -> PausedCutover {
    spawn_paused_storage_open(root, "before_generation_publication")
}

fn spawn_paused_storage_open(root: &Path, pausepoint: &str) -> PausedCutover {
    let ready = root.join("cutover-publication.ready");
    let release = root.join("cutover-publication.release");
    let child = Command::new(env::current_exe().expect("current schema cutover test executable"))
        .args([
            "--exact",
            "paused_cutover_helper",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("LOREPIA_TEST_PAUSED_CUTOVER_ROOT", root)
        .env("LOREPIA_TEST_CUTOVER_PAUSEPOINT", pausepoint)
        .env("LOREPIA_TEST_CUTOVER_PAUSE_READY", &ready)
        .env("LOREPIA_TEST_CUTOVER_PAUSE_RELEASE", &release)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn paused cutover helper");
    let mut paused = PausedCutover {
        child: Some(child),
        release,
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if ready.exists() {
            return paused;
        }
        let status = paused
            .child
            .as_mut()
            .expect("paused cutover child")
            .try_wait()
            .expect("poll paused cutover");
        assert!(
            status.is_none(),
            "paused cutover exited before publishing readiness: {status:?}"
        );
        assert!(
            Instant::now() < deadline,
            "paused cutover did not publish readiness"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn install_previous_release_cas_fixture(root: &Path, namespace: &str) -> (String, PathBuf) {
    let seed = tempdir().expect("temporary previous-release CAS seed");
    let current = Storage::open(seed.path()).expect("create current CAS seed database");
    let current_schema = current
        .schema_version()
        .expect("read current CAS seed schema");
    drop(current);
    let seed_canonical = seed.path().join("db/lorepia.sqlite3");
    downgrade_latest_schema_by_one(&seed_canonical, current_schema);

    let bytes: &[u8] = match namespace {
        "sources" => b"previous-release rollback source",
        "assets" => b"previous-release rollback asset",
        _ => panic!("unsupported previous-release CAS namespace"),
    };
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let relative_path = format!("{namespace}/sha256/{}/{}", &sha256[..2], &sha256[2..]);
    let connection = Connection::open(&seed_canonical).expect("open previous-release CAS seed");
    match namespace {
        "sources" => connection
            .execute(
                "INSERT INTO content_sources
                 (sha256, relative_path, size_bytes, created_at)
                 VALUES (?1, ?2, ?3, '2026-08-12T00:00:00Z')",
                rusqlite::params![
                    sha256,
                    relative_path,
                    i64::try_from(bytes.len()).expect("small previous-release source")
                ],
            )
            .expect("insert previous-release source row"),
        "assets" => connection
            .execute(
                "INSERT INTO assets
                 (sha256, relative_path, media_type, size_bytes, created_at)
                 VALUES (?1, ?2, 'application/octet-stream', ?3, '2026-08-12T00:00:00Z')",
                rusqlite::params![
                    sha256,
                    relative_path,
                    i64::try_from(bytes.len()).expect("small previous-release asset")
                ],
            )
            .expect("insert previous-release asset row"),
        _ => unreachable!("validated previous-release CAS namespace"),
    };
    checkpoint_and_close(connection);

    let canonical = root.join("db/lorepia.sqlite3");
    fs::create_dir_all(
        canonical
            .parent()
            .expect("previous-release database parent"),
    )
    .expect("create previous-release database directory");
    fs::copy(&seed_canonical, &canonical).expect("copy previous-release CAS database");
    let cas_path = root.join(&relative_path);
    fs::create_dir_all(cas_path.parent().expect("previous-release CAS parent"))
        .expect("create previous-release CAS directory");
    fs::write(&cas_path, bytes).expect("write previous-release CAS bytes");
    (sha256, cas_path)
}

fn install_previous_release_journal_cas_fixture(root: &Path, namespace: &str) -> (String, PathBuf) {
    let seed = tempdir().expect("temporary previous-release journal CAS seed");
    let current = Storage::open(seed.path()).expect("create current journal CAS seed database");
    let current_schema = current
        .schema_version()
        .expect("read current journal CAS seed schema");
    drop(current);
    let seed_canonical = seed.path().join("db/lorepia.sqlite3");
    downgrade_latest_schema_by_one(&seed_canonical, current_schema);

    let bytes: &[u8] = match namespace {
        "source" => b"previous-release journal-only source",
        "asset" => b"previous-release journal-only asset",
        _ => panic!("unsupported journal CAS namespace"),
    };
    let sha256 = format!("{:x}", Sha256::digest(bytes));
    let directory = match namespace {
        "source" => "sources",
        "asset" => "assets",
        _ => unreachable!("validated journal CAS namespace"),
    };
    let relative_path = format!("{directory}/sha256/{}/{}", &sha256[..2], &sha256[2..]);
    let connection = Connection::open(&seed_canonical).expect("open journal CAS seed");
    connection
        .execute(
            "INSERT INTO package_cas_promotion_journal
             (import_id, namespace, sha256, size_bytes, media_type,
              relative_path, phase, created_at, updated_at)
             VALUES ('previous-release-journal', ?1, ?2, ?3, ?4, ?5,
                     'file_durable', '2026-08-12T00:00:00Z', '2026-08-12T00:00:00Z')",
            rusqlite::params![
                namespace,
                sha256,
                i64::try_from(bytes.len()).expect("small journal CAS object"),
                (namespace == "asset").then_some("application/octet-stream"),
                relative_path,
            ],
        )
        .expect("insert previous-release journal CAS row");
    checkpoint_and_close(connection);

    let canonical = root.join("db/lorepia.sqlite3");
    fs::create_dir_all(canonical.parent().expect("journal CAS database parent"))
        .expect("create journal CAS database directory");
    fs::copy(&seed_canonical, &canonical).expect("copy previous-release journal CAS database");
    let cas_path = root.join(&relative_path);
    fs::create_dir_all(cas_path.parent().expect("journal CAS parent"))
        .expect("create journal CAS directory");
    fs::write(&cas_path, bytes).expect("write journal-only CAS bytes");
    (sha256, cas_path)
}

fn checkpoint_and_close(connection: Connection) {
    let checkpoint = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .expect("checkpoint fixture database");
    assert_eq!(checkpoint, (0, 0, 0));
    drop(connection);
}

/// Strips exactly the newest migration so a fixture sits one schema behind.
///
/// Update the migration constant and the expected version together whenever a
/// migration is added; the assertion below is what forces that.
fn downgrade_latest_schema_by_one(path: &Path, current_schema: u32) {
    const LATEST_MIGRATION: &str = include_str!("../migrations/0040_portable_runtime_state.sql");
    const LATEST_SCHEMA: u32 = 40;

    assert_eq!(
        current_schema, LATEST_SCHEMA,
        "update this deterministic previous-release fixture for the new latest migration"
    );
    let connection = Connection::open(path).expect("open current database for fixture downgrade");
    let created_objects = LATEST_MIGRATION
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_ascii_whitespace();
            if tokens.next() != Some("CREATE") {
                return None;
            }
            let object_type = tokens.next()?;
            let (object_type, name) = if object_type == "UNIQUE" {
                (tokens.next()?, tokens.next()?)
            } else {
                (object_type, tokens.next()?)
            };
            Some((object_type, name.trim_end_matches(';')))
        })
        .collect::<Vec<_>>();
    const EXPECTED_SCHEMA_40_OBJECTS: &[(&str, &str)] = &[
        ("TABLE", "portable_runtime_branch_epochs"),
        ("TRIGGER", "portable_runtime_branch_epoch_on_branch_insert"),
        ("TABLE", "portable_runtime_state_sequence"),
        ("TABLE", "portable_runtime_states"),
        ("INDEX", "portable_runtime_states_lru"),
        ("TRIGGER", "portable_runtime_state_scope_guard_insert"),
        ("TRIGGER", "portable_runtime_state_scope_guard_update"),
    ];
    assert_eq!(
        created_objects.as_slice(),
        EXPECTED_SCHEMA_40_OBJECTS,
        "schema-40 inverse must track every additive object"
    );
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .expect("disable foreign keys for the previous-release fixture downgrade");
    for object_type in ["VIEW", "TRIGGER", "INDEX", "TABLE"] {
        for (_, name) in created_objects
            .iter()
            .rev()
            .filter(|(candidate_type, _)| *candidate_type == object_type)
        {
            connection
                .execute(&format!("DROP {object_type} \"{name}\""), [])
                .unwrap_or_else(|error| panic!("drop schema-40 {object_type} {name}: {error}"));
        }
    }
    connection
        .execute(
            "DELETE FROM schema_migrations WHERE version = ?1",
            [LATEST_SCHEMA],
        )
        .expect("remove schema-40 migration registry row");
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .expect("reenable foreign keys after the previous-release fixture downgrade");
    checkpoint_and_close(connection);
}

fn database_schema_version(path: &Path) -> u32 {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open database registry read-only")
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get::<_, u32>(0)
        })
        .expect("read database schema version")
}

fn active_database_path(root: &Path) -> PathBuf {
    let (_, manifest) = selected_generation_manifest(root);
    let relative = manifest["active_database_relative_path"]
        .as_str()
        .expect("active database relative path");
    root.join(relative)
}

fn active_generation_manifest_path(root: &Path) -> PathBuf {
    selected_generation_manifest(root).0
}

fn selected_generation_manifest(root: &Path) -> (PathBuf, serde_json::Value) {
    let cutover = root.join("db/schema-cutover");
    let mut selected: Option<(u64, PathBuf, serde_json::Value)> = None;
    for entry in fs::read_dir(&cutover).expect("read committed generation directory") {
        let entry = entry.expect("read committed generation entry");
        let manifest_path = entry.path().join("generation-manifest.json");
        let commit_path = entry.path().join("generation-committed.json");
        let manifest_exists = manifest_path.is_file();
        let commit_exists = commit_path.is_file();
        assert_eq!(
            manifest_exists, commit_exists,
            "generation manifest and commit marker must be published as a pair after recovery"
        );
        if !manifest_exists {
            continue;
        }

        let manifest_bytes = fs::read(&manifest_path).expect("read generation manifest");
        let manifest = serde_json::from_slice::<serde_json::Value>(&manifest_bytes)
            .expect("parse generation manifest");
        let commit = serde_json::from_slice::<serde_json::Value>(
            &fs::read(&commit_path).expect("read generation commit marker"),
        )
        .expect("parse generation commit marker");
        let directory_id = entry
            .file_name()
            .into_string()
            .expect("generation directory ID is UTF-8");
        assert_eq!(manifest["cutover_id"].as_str(), Some(directory_id.as_str()));
        assert_eq!(commit["cutover_id"].as_str(), Some(directory_id.as_str()));
        let manifest_sha256 = format!("{:x}", Sha256::digest(&manifest_bytes));
        assert_eq!(
            commit["manifest_sha256"].as_str(),
            Some(manifest_sha256.as_str()),
            "commit marker must bind the exact generation manifest bytes"
        );
        let activation_sequence = manifest["activation_sequence"]
            .as_u64()
            .expect("generation activation sequence");
        if let Some((selected_sequence, _, _)) = &selected {
            assert_ne!(
                *selected_sequence, activation_sequence,
                "generation activation sequences must be unique"
            );
        }
        if selected
            .as_ref()
            .is_none_or(|(sequence, _, _)| activation_sequence > *sequence)
        {
            selected = Some((activation_sequence, manifest_path, manifest));
        }
    }
    let (_, path, manifest) = selected.expect("at least one committed database generation");
    (path, manifest)
}

fn committed_generation_count(root: &Path) -> usize {
    let cutover = root.join("db/schema-cutover");
    match fs::read_dir(cutover) {
        Ok(entries) => entries
            .map(|entry| entry.expect("read candidate generation entry"))
            .filter(|entry| entry.path().join("generation-committed.json").is_file())
            .count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("read candidate generation directory: {error}"),
    }
}

fn assert_no_uncommitted_generation_directories(root: &Path) {
    let cutover = root.join("db/schema-cutover");
    for entry in fs::read_dir(cutover).expect("read recovered generation directory") {
        let entry = entry.expect("read recovered generation entry");
        assert!(
            entry
                .file_type()
                .expect("inspect recovered generation entry")
                .is_dir(),
            "cutover root may contain only real generation directories"
        );
        assert!(
            entry.path().join("generation-manifest.json").is_file(),
            "recovery left a generation without its manifest: {}",
            entry.path().display()
        );
        assert!(
            entry.path().join("generation-committed.json").is_file(),
            "recovery left an uncommitted generation: {}",
            entry.path().display()
        );
    }
}

fn assert_fixture_semantics(path: &Path) {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open active candidate read-only");
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM characters", [], |row| {
                row.get::<_, u32>(0)
            })
            .expect("count fixture characters"),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT content FROM messages WHERE role = 'assistant'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("read fixture assistant message"),
        "Synthetic assistant reply."
    );
    let settings_json = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("read fixture application settings");
    let settings = serde_json::from_str::<serde_json::Value>(&settings_json)
        .expect("parse fixture application settings");
    assert_eq!(
        settings["selected_model_route_id"].as_str(),
        Some("fixture-openai-route")
    );
}

fn conversation_title(path: &Path) -> String {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open conversation database read-only")
        .query_row(
            "SELECT title FROM conversations
             WHERE id = '2faca127-ff70-4acb-b26f-60f1042b8d11'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("read fixture conversation title")
}

fn file_sha256(path: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(fs::read(path).expect("read file for SHA-256"))
    )
}
