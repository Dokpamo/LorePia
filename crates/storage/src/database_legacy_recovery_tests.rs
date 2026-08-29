use tempfile::tempdir;

use super::*;

fn version_two_database(root: &std::path::Path) -> Connection {
    let source_bytes = b"a";
    let source_hash = hex::encode(Sha256::digest(source_bytes));
    let source_relative_path =
        format!("sources/sha256/{}/{}", &source_hash[..2], &source_hash[2..]);
    let source_path = root.join(&source_relative_path);
    fs::create_dir_all(source_path.parent().expect("legacy source CAS parent"))
        .expect("create legacy source CAS parent");
    fs::write(&source_path, source_bytes).expect("write legacy source CAS bytes");
    fs::create_dir_all(root.join("db")).expect("db directory");
    let connection = Connection::open(root.join("db/lorepia.sqlite3")).expect("legacy database");
    connection
        .execute_batch(MIGRATION_0001)
        .expect("initial schema");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?1)",
            ["2026-01-01T00:00:00Z"],
        )
        .expect("version one");
    connection
        .execute_batch(MIGRATION_0002)
        .expect("second migration");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (2, ?1)",
            ["2026-01-01T00:00:00Z"],
        )
        .expect("version two");
    connection
        .execute(
            "INSERT INTO content_sources
             (sha256, relative_path, size_bytes, created_at)
             VALUES (?1, ?2, 1, ?3)",
            params![source_hash, source_relative_path, "2026-01-01T00:00:00Z"],
        )
        .expect("legacy source");
    connection
        .execute(
            "INSERT INTO characters
             (id, name, description, source_hash, avatar_asset_hash, created_at)
             VALUES ('character', 'Legacy', 'Legacy character', ?1, NULL, ?2)",
            params![source_hash, "2026-01-01T00:00:00Z"],
        )
        .expect("legacy character");
    connection
        .execute(
            "INSERT INTO conversations
             (id, character_id, title, created_at, updated_at)
             VALUES ('conversation', 'character', 'Legacy room', ?1, ?2)",
            params!["2026-01-01T00:00:00Z", "2026-01-01T00:00:04Z"],
        )
        .expect("legacy conversation");
    connection
}

fn insert_legacy_message(
    connection: &Connection,
    row: (&str, Option<&str>, &str, &str, &str, Option<&str>, &str),
) {
    let (id, parent_id, role, content, status, generation_id, created_at) = row;
    connection
        .execute(
            "INSERT INTO messages
             (id, conversation_id, parent_id, role, content, status,
              generation_id, created_at)
             VALUES (?1, 'conversation', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                parent_id,
                role,
                content,
                status,
                generation_id,
                created_at
            ],
        )
        .expect("legacy message");
}

fn assert_attemptless_generation_has_no_terminal_lifecycle(storage: &Storage, generation_id: &str) {
    let connection = storage.connection().expect("legacy recovery database");
    let attempt_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM generation_attempt_intents
             WHERE generation_id = ?1",
            [generation_id],
            |row| row.get::<_, u64>(0),
        )
        .expect("legacy attempt count");
    let terminal_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM core_lifecycle_outbox
             WHERE generation_id = ?1
               AND event_kind IN ('after_generation', 'message_committed')",
            [generation_id],
            |row| row.get::<_, u64>(0),
        )
        .expect("legacy terminal lifecycle count");
    assert_eq!(attempt_count, 0);
    assert_eq!(
        terminal_count, 0,
        "attempt-less legacy recovery must not create an undrainable lifecycle row"
    );
}

#[test]
fn version_two_equal_timestamps_preserve_generation_parent_lineage() {
    let root = tempdir().expect("temp root");
    let connection = version_two_database(root.path());
    for row in [
        (
            "z-user-1",
            None,
            "user",
            "first",
            "complete",
            None,
            "2026-01-01T00:00:01Z",
        ),
        (
            "a-assistant-1",
            Some("z-user-1"),
            "assistant",
            "one",
            "complete",
            Some("generation-1"),
            "2026-01-01T00:00:01Z",
        ),
        (
            "z-user-2",
            None,
            "user",
            "second",
            "complete",
            None,
            "2026-01-01T00:00:02Z",
        ),
        (
            "a-assistant-2",
            Some("z-user-2"),
            "assistant",
            "two",
            "complete",
            Some("generation-2"),
            "2026-01-01T00:00:02Z",
        ),
    ] {
        insert_legacy_message(&connection, row);
    }
    drop(connection);

    let storage = Storage::open(root.path()).expect("migrate legacy database");
    assert_eq!(
        storage
            .schema_version()
            .expect("read durable schema version"),
        SCHEMA_VERSION
    );
    let conversation_id = ConversationId("conversation".to_owned());
    let state = storage
        .get_conversation_state(&conversation_id)
        .expect("conversation state");
    assert_eq!(state.selected_mode, ConversationMode::Chat);
    let messages = storage
        .list_branch_messages(&state.active_branch_id)
        .expect("migrated lineage");
    assert_eq!(
        messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        ["first", "one", "second", "two"]
    );
    assert_eq!(messages[0].parent_id, None);
    assert_eq!(messages[1].parent_id, Some(messages[0].id.clone()));
    assert_eq!(messages[2].parent_id, Some(messages[1].id.clone()));
    assert_eq!(messages[3].parent_id, Some(messages[2].id.clone()));
    assert_eq!(
        storage
            .get_generation(&GenerationId("generation-2".to_owned()))
            .expect("generation snapshot")
            .mode,
        ConversationMode::Chat
    );
    assert_eq!(
        storage
            .get_generation(&GenerationId("generation-1".to_owned()))
            .expect("first generation snapshot")
            .user_message_id,
        MessageId("z-user-1".to_owned())
    );
}

#[test]
fn version_two_assistant_without_a_user_parent_is_rejected_before_migration() {
    let root = tempdir().expect("temp root");
    let connection = version_two_database(root.path());
    insert_legacy_message(
        &connection,
        (
            "assistant",
            None,
            "assistant",
            "orphan",
            "complete",
            Some("generation"),
            "2026-01-01T00:00:01Z",
        ),
    );
    drop(connection);

    let Err(error) = Storage::open(root.path()) else {
        panic!("orphan assistant must be rejected");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

    let connection =
        Connection::open(root.path().join("db/lorepia.sqlite3")).expect("legacy database");
    assert_eq!(
        connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, u32>(0)
            })
            .expect("schema version"),
        2
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type = 'table' AND name = 'generations'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .expect("generation table count"),
        0
    );
}

#[test]
fn version_two_recovery_reparents_later_turns_around_discarded_partial_assistant() {
    let root = tempdir().expect("temp root");
    let connection = version_two_database(root.path());
    for row in [
        (
            "user-1",
            None,
            "user",
            "first",
            "complete",
            None,
            "2026-01-01T00:00:01Z",
        ),
        (
            "assistant-1",
            Some("user-1"),
            "assistant",
            "partial",
            "pending",
            Some("generation-1"),
            "2026-01-01T00:00:02Z",
        ),
        (
            "user-2",
            None,
            "user",
            "second",
            "complete",
            None,
            "2026-01-01T00:00:03Z",
        ),
        (
            "assistant-2",
            Some("user-2"),
            "assistant",
            "two",
            "complete",
            Some("generation-2"),
            "2026-01-01T00:00:04Z",
        ),
    ] {
        insert_legacy_message(&connection, row);
    }
    connection
        .execute(
            "INSERT INTO app_settings(key, value_json) VALUES ('application', ?1)",
            [serde_json::to_string(&AppSettings {
                preserve_partial_generations: false,
                selected_provider_profile_id: None,
                ..AppSettings::default()
            })
            .expect("settings JSON")],
        )
        .expect("discard-partial settings");
    drop(connection);

    for reopen_index in 0..2 {
        let storage = Storage::open(root.path()).expect("migrate and recover legacy database");
        assert_eq!(
            storage
                .schema_version()
                .expect("read durable schema version"),
            SCHEMA_VERSION
        );
        let state = storage
            .get_conversation_state(&ConversationId("conversation".to_owned()))
            .expect("conversation state");
        let messages = storage
            .list_branch_messages(&state.active_branch_id)
            .expect("recovered lineage");
        assert_eq!(
            messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["first", "second", "two"],
            "reopen {reopen_index} must preserve later completed turns"
        );
        assert_eq!(messages[0].parent_id, None);
        assert_eq!(messages[1].parent_id, Some(messages[0].id.clone()));
        assert_eq!(messages[2].parent_id, Some(messages[1].id.clone()));

        let discarded = storage
            .get_generation(&GenerationId("generation-1".to_owned()))
            .expect("discarded generation");
        assert_eq!(discarded.status, GenerationStatus::Cancelled);
        assert_eq!(discarded.assistant_message_id, None);
        assert_attemptless_generation_has_no_terminal_lifecycle(&storage, "generation-1");
        assert_eq!(
            storage
                .get_generation(&GenerationId("generation-2".to_owned()))
                .expect("completed generation")
                .status,
            GenerationStatus::Complete
        );
    }
}
