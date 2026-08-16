use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{Duration, Utc};
use lorepia_domain::{
    Character, CharacterContentV1, CharacterGreetingKind, Conversation, ConversationMode,
    CoreErrorCode, MessageRole, MessageStatus,
};
use lorepia_storage::Storage;
use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir, tempdir};
use uuid::Uuid;

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &fs::read(entry.path().join("generation-manifest.json"))
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

fn import_character(storage: &Storage, root: &TempDir, name: &str) -> Character {
    let bytes = format!("synthetic character source for {name}").into_bytes();
    let mut staged = NamedTempFile::new_in(root.path()).expect("staged character source");
    staged.write_all(&bytes).expect("write character source");
    staged.flush().expect("flush character source");
    let character = Character::new(
        name,
        "Project-owned synthetic greeting fixture",
        hex::encode(Sha256::digest(&bytes)),
    );
    storage
        .commit_character_import(
            staged.path(),
            &character,
            u64::try_from(bytes.len()).expect("small fixture"),
            &Uuid::new_v4().to_string(),
            &[],
        )
        .expect("commit synthetic character");
    character
}

fn greeting_content(default: &str, alternates: &[&str]) -> CharacterContentV1 {
    CharacterContentV1 {
        first_message: default.to_owned(),
        alternate_greetings: alternates
            .iter()
            .map(|greeting| (*greeting).to_owned())
            .collect(),
        ..CharacterContentV1::default()
    }
}

#[test]
fn safe_catalog_and_alternate_start_survive_lost_response_and_restart() {
    let root = tempdir().expect("data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let character = import_character(&storage, &root, "Greeting restart");
    let default_canary = "PRIVATE DEFAULT GREETING CANARY";
    let alternate_canary = "PRIVATE ALTERNATE GREETING CANARY";
    let stored_content = storage
        .save_character_content(
            &character.id,
            &greeting_content(default_canary, &["First alternate", alternate_canary]),
            None,
        )
        .expect("save character content");
    let revision_id = stored_content
        .revision_id
        .clone()
        .expect("immutable content revision id");

    let catalog = storage
        .character_greeting_catalog(&character.id)
        .expect("safe greeting catalog");
    assert_eq!(
        catalog.character_content_revision_id.as_deref(),
        Some(revision_id.as_str())
    );
    assert_eq!(
        catalog
            .greetings
            .iter()
            .map(|greeting| (greeting.id.as_str(), greeting.kind, greeting.enabled))
            .collect::<Vec<_>>(),
        [
            ("default", CharacterGreetingKind::Default, true),
            ("alternate-0", CharacterGreetingKind::Alternate, true),
            ("alternate-1", CharacterGreetingKind::Alternate, true),
        ]
    );
    let public_catalog = serde_json::to_string(&catalog).expect("serialize safe catalog");
    assert!(!public_catalog.contains(default_canary));
    assert!(!public_catalog.contains(alternate_canary));
    assert!(!public_catalog.contains("\"content\""));

    let conversation = Conversation::new(&character.id, "Alternate greeting room");
    let conversation_id = conversation.id.clone();
    storage
        .save_conversation_with_greeting(
            &conversation,
            ConversationMode::Chat,
            Some(&revision_id),
            Some("alternate-1"),
        )
        .expect("commit alternate greeting start");
    // Simulate losing the command response after the durable commit.
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen after lost response");
    let binding = reopened
        .get_conversation_greeting_binding(&conversation_id)
        .expect("recover exact greeting binding");
    assert_eq!(
        binding.character_content_revision_id.as_deref(),
        Some(revision_id.as_str())
    );
    assert_eq!(binding.greeting_id.as_deref(), Some("alternate-1"));
    let branches = reopened
        .list_conversation_branches(&conversation_id)
        .expect("recover root branch");
    assert_eq!(branches.len(), 1);
    let branch = &branches[0];
    let messages = reopened
        .list_branch_messages(&branch.id)
        .expect("recover greeting message");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, MessageRole::Assistant);
    assert_eq!(messages[0].status, MessageStatus::Complete);
    assert_eq!(messages[0].parent_id, None);
    assert_eq!(messages[0].content, alternate_canary);
    assert_eq!(branch.head_message_id.as_ref(), Some(&messages[0].id));

    let now = Utc::now() + Duration::seconds(1);
    let occurrences = reopened
        .claim_core_lifecycle_occurrences(now, now + Duration::minutes(1), 10)
        .expect("claim durable started occurrence");
    assert_eq!(occurrences.len(), 1);
    assert_eq!(occurrences[0].conversation_id, conversation_id);
    assert_eq!(occurrences[0].branch_id, branch.id);
    assert_eq!(
        occurrences[0].exact_head_message_id.as_ref(),
        Some(&messages[0].id)
    );
    assert_eq!(occurrences[0].owner_message_id, None);
    assert_eq!(occurrences[0].generation_id, None);
}

#[test]
fn omitted_greeting_keeps_first_message_compatibility() {
    let root = tempdir().expect("data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let character = import_character(&storage, &root, "Default greeting");
    let default_greeting = "The compatible first_message greeting.";
    let content = storage
        .save_character_content(
            &character.id,
            &greeting_content(default_greeting, &["Unused alternate"]),
            None,
        )
        .expect("save character content");
    let revision_id = content.revision_id.expect("content revision id");

    let conversation = Conversation::new(&character.id, "Default greeting room");
    let (branch, _) = storage
        .save_conversation_with_mode(&conversation, ConversationMode::Story)
        .expect("legacy-compatible conversation start");
    let messages = storage
        .list_branch_messages(&branch.id)
        .expect("default greeting lineage");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, default_greeting);
    assert_eq!(branch.head_message_id.as_ref(), Some(&messages[0].id));
    let binding = storage
        .get_conversation_greeting_binding(&conversation.id)
        .expect("default greeting binding");
    assert_eq!(
        binding.character_content_revision_id.as_deref(),
        Some(revision_id.as_str())
    );
    assert_eq!(binding.greeting_id.as_deref(), Some("default"));

    let legacy = import_character(&storage, &root, "Compatible legacy absence");
    let legacy_conversation = Conversation::new(&legacy.id, "No greeting room");
    let legacy_started = storage
        .save_conversation_with_greeting(&legacy_conversation, ConversationMode::Chat, None, None)
        .expect("exact legacy-absence start");
    assert!(legacy_started.initial_message.is_none());
    let legacy_binding = storage
        .get_conversation_greeting_binding(&legacy_conversation.id)
        .expect("legacy-absence binding");
    assert_eq!(legacy_binding.character_content_revision_id, None);
    assert_eq!(legacy_binding.greeting_id, None);
}

#[test]
fn lifecycle_insert_failure_rolls_back_conversation_branch_and_greeting_message() {
    let root = tempdir().expect("data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let character = import_character(&storage, &root, "Atomic greeting");
    let content = storage
        .save_character_content(
            &character.id,
            &greeting_content("Atomic default greeting", &[]),
            None,
        )
        .expect("save character content");
    let revision_id = content.revision_id.expect("content revision id");
    let projection_connection =
        Connection::open(active_database_path(root.path())).expect("projection connection");
    projection_connection
        .execute_batch(
            "CREATE TRIGGER synthetic_reject_conversation_started
             BEFORE INSERT ON core_lifecycle_outbox
             WHEN NEW.event_kind = 'conversation_started'
             BEGIN
               SELECT RAISE(ABORT, 'synthetic lifecycle failure');
             END;",
        )
        .expect("install synthetic failure trigger");
    drop(projection_connection);

    let error = storage
        .save_conversation_with_greeting(
            &Conversation::new(&character.id, "Must roll back"),
            ConversationMode::Chat,
            Some(&revision_id),
            None,
        )
        .expect_err("outbox failure must abort conversation start");
    assert_eq!(error.code, CoreErrorCode::StorageUnavailable);

    let verification =
        Connection::open(active_database_path(root.path())).expect("verification connection");
    for table in [
        "conversations",
        "conversation_branches",
        "conversation_state",
        "conversation_greeting_bindings",
        "messages",
        "core_lifecycle_outbox",
    ] {
        let count = verification
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, u64>(0)
            })
            .expect("count atomic rows");
        assert_eq!(count, 0, "{table} must roll back with the outbox insert");
    }
}

#[test]
fn durable_binding_constraints_reject_cross_character_revision_and_unknown_greeting() {
    let root = tempdir().expect("data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let first_character = import_character(&storage, &root, "Binding first character");
    let second_character = import_character(&storage, &root, "Binding second character");
    let first_revision = storage
        .save_character_content(
            &first_character.id,
            &greeting_content("First character greeting", &[]),
            None,
        )
        .expect("save first character content")
        .revision_id
        .expect("first revision id");
    let second_revision = storage
        .save_character_content(
            &second_character.id,
            &greeting_content("Second character greeting", &[]),
            None,
        )
        .expect("save second character content")
        .revision_id
        .expect("second revision id");
    let connection =
        Connection::open(active_database_path(root.path())).expect("constraint connection");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable foreign keys");
    let now = Utc::now().to_rfc3339();
    connection
        .execute(
            "INSERT INTO conversations
             (id, character_id, title, created_at, updated_at)
             VALUES ('constraint-room', ?1, 'Constraint room', ?2, ?2)",
            params![first_character.id, now],
        )
        .expect("insert constraint conversation");

    assert!(
        connection
            .execute(
                "INSERT INTO conversation_greeting_bindings
                 (conversation_id, character_content_revision_id, greeting_id, created_at)
                 VALUES ('constraint-room', ?1, 'default', ?2)",
                params![second_revision, now],
            )
            .is_err(),
        "a revision belonging to another character must fail"
    );
    assert!(
        connection
            .execute(
                "INSERT INTO conversation_greeting_bindings
                 (conversation_id, character_content_revision_id, greeting_id, created_at)
                 VALUES ('constraint-room', ?1, 'missing-greeting', ?2)",
                params![first_revision, now],
            )
            .is_err(),
        "the composite revision/greeting foreign key must fail closed"
    );
    connection
        .execute(
            "INSERT INTO conversation_greeting_bindings
             (conversation_id, character_content_revision_id, greeting_id, created_at)
             VALUES ('constraint-room', ?1, 'default', ?2)",
            params![first_revision, now],
        )
        .expect("insert valid exact binding");
}

fn assert_greeting_start_rejected(
    storage: &Storage,
    character_id: &str,
    title: &str,
    revision_id: Option<&str>,
    greeting_id: &str,
) {
    let error = storage
        .save_conversation_with_greeting(
            &Conversation::new(character_id, title),
            ConversationMode::Chat,
            revision_id,
            Some(greeting_id),
        )
        .expect_err("invalid greeting start must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
}

#[test]
fn stale_revision_and_unknown_or_legacy_greeting_ids_commit_nothing() {
    let root = tempdir().expect("data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let character = import_character(&storage, &root, "Stale greeting");
    let first = storage
        .save_character_content(
            &character.id,
            &greeting_content("Version one", &["Old alternate"]),
            None,
        )
        .expect("save first content revision");
    let stale_revision_id = first.revision_id.expect("first revision id");
    let current = storage
        .save_character_content(
            &character.id,
            &greeting_content("Version two", &["Current alternate"]),
            Some(first.revision),
        )
        .expect("save second content revision");
    let current_revision_id = current.revision_id.expect("second revision id");
    let disabled_content_canary = "DISABLED GREETING CONTENT MUST STAY PRIVATE";
    let projection_connection =
        Connection::open(active_database_path(root.path())).expect("projection connection");
    projection_connection
        .pragma_update(None, "foreign_keys", true)
        .expect("enable projection foreign keys");
    projection_connection
        .execute(
            "INSERT INTO character_greetings
             (character_content_revision_id, ordinal, greeting_id, kind,
              content, enabled, payload_json)
             VALUES (?1, 99, 'disabled-alternate', 'alternate', ?2, 0, '{}')",
            params![current_revision_id, disabled_content_canary],
        )
        .expect("install immutable disabled projection fixture");
    drop(projection_connection);
    let catalog = storage
        .character_greeting_catalog(&character.id)
        .expect("catalog with disabled option");
    let disabled = catalog
        .greetings
        .iter()
        .find(|greeting| greeting.id == "disabled-alternate")
        .expect("disabled selector remains visible");
    assert!(!disabled.enabled);
    assert!(
        !serde_json::to_string(&catalog)
            .expect("serialize catalog")
            .contains(disabled_content_canary)
    );

    assert_greeting_start_rejected(
        &storage,
        &character.id,
        "Stale revision",
        Some(&stale_revision_id),
        "alternate-0",
    );
    assert_greeting_start_rejected(
        &storage,
        &character.id,
        "Unknown greeting",
        Some(&current_revision_id),
        "alternate-does-not-exist",
    );
    assert_greeting_start_rejected(
        &storage,
        &character.id,
        "Disabled greeting",
        Some(&current_revision_id),
        "disabled-alternate",
    );

    let legacy = import_character(&storage, &root, "Legacy no sidecar");
    assert_greeting_start_rejected(
        &storage,
        &legacy.id,
        "Invalid legacy greeting",
        None,
        "default",
    );

    assert!(
        storage
            .list_conversations()
            .expect("conversations after rejected starts")
            .is_empty()
    );
    let now = Utc::now() + Duration::seconds(1);
    assert!(
        storage
            .claim_core_lifecycle_occurrences(now, now + Duration::minutes(1), 10)
            .expect("outbox after rejected starts")
            .is_empty()
    );
}
