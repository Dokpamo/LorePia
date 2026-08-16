use std::{
    env, fs,
    path::{Path, PathBuf},
};

use lorepia_core::{ConversationMode, Core, CoreConfig};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::tempdir;

const FIXTURE_SQL: &str =
    include_str!("../../../testdata/tauri-upgrade/native-schema-11/schema-11.sql");
const SOURCE_PACKAGE: &[u8] = include_bytes!("../../../testdata/packages/with-avatar.charx");
const AVATAR_ASSET: &[u8] =
    include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/assets/avatar.png");
const SOURCE_SHA256: &str = "2c528a64fbf36a011e29c1a692cd13568b83f76e764ea03487393c28a2e666de";
const AVATAR_SHA256: &str = "aa7bb0431aaeb198a77c26a14fe6dd714a75e4d7db94e3e1238a1fdcbfe1f8d4";
const POST_CUTOVER_TITLE: &str = "Post-cutover retained-native continuity evidence";
const COMPATIBLE_ROLLBACK_TITLE: &str = "Source-compatible rollback continuity evidence";

#[test]
fn retained_native_core_reopens_active_candidate_with_post_cutover_write() {
    let external_root = env::var_os("LOREPIA_SCHEMA11_RUNTIME_ROOT").map(PathBuf::from);
    let temporary_root = external_root
        .is_none()
        .then(|| tempdir().expect("temporary frozen Core root"));
    let root = external_root.as_deref().unwrap_or_else(|| {
        temporary_root
            .as_ref()
            .expect("temporary root exists")
            .path()
    });
    if external_root.is_none() {
        install_frozen_schema_eleven_fixture(root);
    }
    let canonical = root.join("db/lorepia.sqlite3");
    let canonical_sha256 = file_sha256(&canonical);

    let core = Core::open(CoreConfig::new(root)).expect("cut over with current Core");
    assert!(
        core.health_check()
            .expect("current Core health")
            .schema_version
            > 11,
        "the retained native rollback client must carry the current storage schema"
    );
    let character = core
        .list_characters()
        .expect("list frozen fixture characters")
        .into_iter()
        .next()
        .expect("frozen fixture character");
    let written = core
        .create_conversation(&character.id, POST_CUTOVER_TITLE, ConversationMode::Chat)
        .expect("persist post-cutover conversation");
    drop(core);

    assert_eq!(database_schema_version(&canonical), 11);
    assert_eq!(file_sha256(&canonical), canonical_sha256);
    assert_eq!(conversation_count(&canonical, &written.id.0), 0);
    let active = active_database_path(root);
    assert_eq!(conversation_count(&active, &written.id.0), 1);

    let reopened = Core::open(CoreConfig::new(root)).expect("reopen current Core");
    assert_eq!(
        reopened
            .get_conversation(&written.id)
            .expect("read post-cutover conversation")
            .title,
        "Post-cutover retained-native continuity evidence"
    );
    drop(reopened);

    if let Some(state_path) = env::var_os("LOREPIA_SCHEMA11_RUNTIME_STATE") {
        let state = serde_json::json!({
            "format_version": 1,
            "root": root,
            "canonical_database_sha256": canonical_sha256,
            "active_database_relative_path": active
                .strip_prefix(root)
                .expect("active database is below root"),
            "post_cutover_conversation_id": written.id.0,
            "post_cutover_conversation_title": POST_CUTOVER_TITLE,
            "post_cutover_conversation_visible_in_canonical": false,
            "post_cutover_conversation_visible_in_active": true
        });
        fs::write(
            state_path,
            serde_json::to_vec_pretty(&state).expect("encode runtime state evidence"),
        )
        .expect("write runtime state evidence");
    }
}

#[test]
#[ignore = "requires the prebuilt source-compatible rollback subprocess runtime state"]
fn current_core_reopens_source_compatible_rollback_writes() {
    let root = PathBuf::from(
        env::var_os("LOREPIA_SCHEMA11_RUNTIME_ROOT")
            .expect("LOREPIA_SCHEMA11_RUNTIME_ROOT is required"),
    );
    let state_path = PathBuf::from(
        env::var_os("LOREPIA_SCHEMA11_RUNTIME_STATE")
            .expect("LOREPIA_SCHEMA11_RUNTIME_STATE is required"),
    );
    let state = serde_json::from_slice::<serde_json::Value>(
        &fs::read(&state_path).expect("read compatible rollback runtime state"),
    )
    .expect("parse compatible rollback runtime state");
    assert_eq!(state["format_version"].as_u64(), Some(1));
    assert_eq!(
        PathBuf::from(
            state["root"]
                .as_str()
                .expect("runtime state root is a string")
        ),
        root
    );
    assert_eq!(
        state["post_cutover_conversation_visible_in_active"].as_bool(),
        Some(true)
    );
    assert_eq!(
        state["compatible_rollback_conversation_visible_in_active"].as_bool(),
        Some(true)
    );
    assert_eq!(
        state["compatible_rollback_conversation_visible_in_canonical"].as_bool(),
        Some(false)
    );

    let canonical = root.join("db/lorepia.sqlite3");
    assert_eq!(database_schema_version(&canonical), 11);
    assert_eq!(
        file_sha256(&canonical),
        state["canonical_database_sha256"]
            .as_str()
            .expect("canonical database SHA-256")
    );
    let active = root.join(
        state["active_database_relative_path"]
            .as_str()
            .expect("active database relative path"),
    );
    assert!(database_schema_version(&active) > 11);

    let a_id = lorepia_core::ConversationId(
        state["post_cutover_conversation_id"]
            .as_str()
            .expect("post-cutover conversation ID")
            .to_owned(),
    );
    let b_id = lorepia_core::ConversationId(
        state["compatible_rollback_conversation_id"]
            .as_str()
            .expect("compatible rollback conversation ID")
            .to_owned(),
    );
    assert_eq!(conversation_count(&canonical, &a_id.0), 0);
    assert_eq!(conversation_count(&canonical, &b_id.0), 0);
    assert_eq!(conversation_count(&active, &a_id.0), 1);
    assert_eq!(conversation_count(&active, &b_id.0), 1);

    let core = Core::open(CoreConfig::new(&root)).expect("current Core reopens active generation");
    assert_eq!(
        core.get_conversation(&a_id)
            .expect("current Core reads active-generation write A")
            .title,
        POST_CUTOVER_TITLE
    );
    assert_eq!(
        core.get_conversation(&b_id)
            .expect("current Core reads compatible rollback write B")
            .title,
        COMPATIBLE_ROLLBACK_TITLE
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

    write_fixture_cas_object(root, "sources", SOURCE_SHA256, SOURCE_PACKAGE);
    write_fixture_cas_object(root, "assets", AVATAR_SHA256, AVATAR_ASSET);
}

fn write_fixture_cas_object(root: &Path, namespace: &str, sha256: &str, bytes: &[u8]) {
    assert_eq!(format!("{:x}", Sha256::digest(bytes)), sha256);
    let directory = root.join(namespace).join("sha256").join(&sha256[..2]);
    fs::create_dir_all(&directory).expect("create fixture CAS directory");
    fs::write(directory.join(&sha256[2..]), bytes).expect("write fixture CAS object");
}

fn active_database_path(root: &Path) -> PathBuf {
    let mut selected: Option<(u64, serde_json::Value)> = None;
    for entry in
        fs::read_dir(root.join("db/schema-cutover")).expect("read committed database generations")
    {
        let entry = entry.expect("read database generation entry");
        if !entry.path().join("generation-committed.json").is_file() {
            continue;
        }
        let manifest = serde_json::from_slice::<serde_json::Value>(
            &fs::read(entry.path().join("generation-manifest.json"))
                .expect("read database generation manifest"),
        )
        .expect("parse database generation manifest");
        let sequence = manifest["activation_sequence"]
            .as_u64()
            .expect("database generation activation sequence");
        if selected
            .as_ref()
            .is_none_or(|(current, _)| sequence > *current)
        {
            selected = Some((sequence, manifest));
        }
    }
    let (_, manifest) = selected.expect("active committed database generation");
    root.join(
        manifest["active_database_relative_path"]
            .as_str()
            .expect("active database relative path"),
    )
}

fn database_schema_version(path: &Path) -> u32 {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open database registry read-only")
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
            row.get::<_, u32>(0)
        })
        .expect("read database schema version")
}

fn conversation_count(path: &Path, conversation_id: &str) -> u32 {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("open conversation database read-only")
        .query_row(
            "SELECT COUNT(*) FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| row.get::<_, u32>(0),
        )
        .expect("count post-cutover conversation")
}

fn file_sha256(path: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(fs::read(path).expect("read file for SHA-256"))
    )
}
