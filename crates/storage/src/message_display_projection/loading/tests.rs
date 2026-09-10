use super::*;
use lorepia_domain::{ConversationId, CoreErrorCode, MessageId, MessageStatus};

fn assistant(index: usize) -> Message {
    let mut message = Message::pending_assistant(
        ConversationId("conversation".into()),
        MessageId("user".into()),
        GenerationId(format!("generation-{index}")),
    );
    message.id = MessageId(format!("assistant-{index}"));
    message.status = MessageStatus::Complete;
    message.content = format!("canonical-{index}");
    message
}

#[test]
fn bulk_missing_projections_take_one_lock_per_bounded_batch() {
    let root = tempfile::tempdir().expect("root");
    let storage = Storage::open(root.path()).expect("storage");
    let messages = (0..257).map(assistant).collect::<Vec<_>>();
    let refs = messages.iter().collect::<Vec<_>>();
    let before = storage.database_connection_metrics().acquisitions;
    assert!(
        storage
            .get_message_display_projections(&refs)
            .expect("identity projections")
            .is_empty()
    );
    assert_eq!(
        storage.database_connection_metrics().acquisitions - before,
        5
    );
    assert!(
        storage
            .get_message_display_projections(&[])
            .expect("empty")
            .is_empty()
    );
    assert_eq!(
        storage.database_connection_metrics().acquisitions - before,
        5
    );
}

fn fixture() -> rusqlite::Connection {
    let connection = rusqlite::Connection::open_in_memory().expect("db");
    connection.execute_batch(
        "CREATE TABLE message_display_projections (message_id TEXT PRIMARY KEY, generation_id TEXT,
          canonical_content_sha256 TEXT, display_content TEXT, display_content_sha256 TEXT,
          pipeline_diagnostics_json TEXT, diagnostics_sha256 TEXT, created_at TEXT);
         CREATE TABLE transform_set_revisions (revision_id TEXT, transform_set_id TEXT);
         CREATE TABLE transform_application_logs (message_id TEXT, generation_id TEXT,
          set_revision_id TEXT, rule_id TEXT, phase TEXT, status TEXT, before_sha256 TEXT,
          after_sha256 TEXT, error_code TEXT, diagnostics_json TEXT, created_at TEXT,
          ordinal INTEGER, id TEXT);"
    ).expect("read fixture tables");
    connection
}

#[test]
fn bulk_verification_preserves_requested_order_and_rejects_tampered_content_or_ownership() {
    let connection = fixture();
    let messages = (0..64).map(assistant).collect::<Vec<_>>();
    for message in &messages {
        let display = format!("display {}", message.content);
        connection
            .execute(
                "INSERT INTO message_display_projections VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                rusqlite::params![
                    message.id.0,
                    message.generation_id.as_ref().expect("generation").0,
                    sha256_digest(message.content.as_bytes())
                        .expect("hash")
                        .as_str(),
                    display,
                    sha256_digest(display.as_bytes()).expect("hash").as_str(),
                    r#"{"schema_version":1,"failures":[]}"#,
                    diagnostics_sha256(&[]).expect("hash").as_str(),
                    message.created_at.to_rfc3339()
                ],
            )
            .expect("projection");
    }
    let refs = messages.iter().rev().collect::<Vec<_>>();
    let loaded = load_batch(&connection, &refs).expect("batch");
    assert_eq!(
        loaded.iter().map(|p| &p.message_id).collect::<Vec<_>>(),
        refs.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
    connection
        .execute(
            "UPDATE message_display_projections SET display_content='tampered' WHERE message_id=?1",
            [&messages[0].id.0],
        )
        .expect("tamper");
    assert_eq!(
        load_batch(&connection, &refs)
            .expect_err("hash rejected")
            .code,
        CoreErrorCode::StorageCorrupted
    );
    let mut wrong_owner = messages[1].clone();
    wrong_owner.generation_id = Some(GenerationId("other".into()));
    assert_eq!(
        load_batch(&connection, &[&wrong_owner])
            .expect_err("owner rejected")
            .code,
        CoreErrorCode::StorageCorrupted
    );
}

fn load_batch(
    connection: &rusqlite::Connection,
    messages: &[&Message],
) -> CoreResult<Vec<StoredMessageDisplayProjection>> {
    verify_batch(read_batch(connection, messages)?)
}
