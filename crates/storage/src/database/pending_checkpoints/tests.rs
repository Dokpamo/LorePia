use super::*;
use crate::database::{ConversationBranchId, ConversationId};

fn fixture() -> (tempfile::TempDir, Storage) {
    let directory = tempfile::tempdir().unwrap();
    let storage = Storage::open(directory.path()).unwrap();
    {
        let connection = storage.connection().unwrap();
        connection.execute_batch("INSERT INTO content_sources VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','source.bin',1,'2026-09-12T00:00:00+00:00');
          INSERT INTO characters(id,name,description,source_hash,created_at) VALUES ('c','c','','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','2026-09-12T00:00:00+00:00');
          INSERT INTO conversations(id,character_id,title,created_at,updated_at) VALUES ('room','c','','2026-09-12T00:00:00+00:00','2026-09-12T00:00:00+00:00');
          INSERT INTO messages(id,conversation_id,role,content,status,generation_id,created_at) VALUES ('a','room','assistant','base','pending','generation','2026-09-12T00:00:00+00:00');
          INSERT INTO conversation_branches(id,conversation_id,head_message_id,created_at,updated_at) VALUES ('main','room','a','2026-09-12T00:00:00+00:00','2026-09-12T00:00:00+00:00');").unwrap();
    }
    (directory, storage)
}

fn append(storage: &Storage, offset: u64, delta: &str) -> CoreResult<u64> {
    storage.append_pending_assistant_checkpoint(
        &MessageId("a".into()),
        &GenerationId("generation".into()),
        offset,
        delta,
    )
}

#[test]
fn append_replay_unicode_and_all_pending_reads_preserve_effective_content() {
    let (_directory, storage) = fixture();
    let delta = format!("{}한\0끝", "x".repeat(CHUNK_BYTES - 1));
    let total = 4 + delta.len() as u64;
    assert_eq!(append(&storage, 4, &delta).unwrap(), total);
    assert_eq!(append(&storage, 4, &delta).unwrap(), total);
    assert!(append(&storage, 4, "conflict").is_err());
    assert!(append(&storage, total + 1, "gap").is_err());
    let room = ConversationId("room".into());
    let expected = format!("base{delta}");
    assert_eq!(storage.list_messages(&room).unwrap()[0].content, expected);
    assert_eq!(
        storage
            .list_branch_messages(&ConversationBranchId("main".into()))
            .unwrap()[0]
            .content,
        expected
    );
    let page = storage
        .list_branch_messages_page(
            &ConversationBranchId("main".into()),
            None,
            None,
            30,
            None,
            true,
        )
        .unwrap();
    assert_eq!(page.messages[0].content, expected);
    assert!(
        storage
            .list_recent_messages_for_prompt(&room, 30, 10, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        storage
            .list_recent_messages_for_prompt(&room, 30, 100_000, 100_000)
            .unwrap()[0]
            .content,
        expected
    );
    let connection = storage.connection().unwrap();
    let (base, count): (String, u64) = connection.query_row("SELECT content, (SELECT COUNT(*) FROM pending_assistant_checkpoint_chunks) FROM messages WHERE id='a'", [], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(base, "base");
    assert_eq!(count, 2);
}

#[test]
fn replacement_clears_journal_and_late_append_fails() {
    let (_directory, storage) = fixture();
    append(&storage, 4, "partial").unwrap();
    let mut message = storage
        .list_messages(&ConversationId("room".into()))
        .unwrap()
        .remove(0);
    message.content = "replacement".into();
    storage.checkpoint_pending_assistant(&message).unwrap();
    assert_eq!(append(&storage, 11, "!").unwrap(), 12);
    message.content = "terminal".into();
    message.status = crate::database::MessageStatus::Complete;
    storage.save_message(&message).unwrap();
    assert!(append(&storage, 12, "late").is_err());
    assert_eq!(
        storage
            .list_messages(&ConversationId("room".into()))
            .unwrap()[0]
            .content,
        "terminal"
    );
    let connection = storage.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM pending_assistant_checkpoint_chunks",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn missing_chunk_fails_before_append_and_sql_length_filter() {
    let (_directory, storage) = fixture();
    append(&storage, 4, "one").unwrap();
    append(&storage, 7, "two").unwrap();
    storage
        .connection()
        .unwrap()
        .execute(
            "DELETE FROM pending_assistant_checkpoint_chunks WHERE sequence=0",
            [],
        )
        .unwrap();
    assert!(append(&storage, 10, "three").is_err());
    assert!(
        storage
            .list_messages(&ConversationId("room".into()))
            .is_err()
    );
    assert!(
        storage
            .list_recent_messages_for_prompt(&ConversationId("room".into()), 30, 1, 1)
            .is_err()
    );
}

#[test]
fn migration_view_errors_only_on_corruption_and_legacy_close_materializes() {
    let (_directory, storage) = fixture();
    // An unjournalled pending body must not evaluate the deliberate JSON error.
    assert_eq!(
        storage
            .list_messages(&ConversationId("room".into()))
            .unwrap()[0]
            .content,
        "base"
    );
    append(&storage, 4, "durable").unwrap();
    {
        let mut connection = storage.connection().unwrap();
        let transaction = connection.transaction().unwrap();
        crate::database::interrupted_generation_recovery::close_interrupted_generations_in_transaction(
            &transaction, &[], true,
            "2026-09-12T00:00:00+00:00".parse().unwrap(), "2026-09-12T00:00:00+00:00",
        ).unwrap();
        transaction.commit().unwrap();
    }
    assert_eq!(
        storage
            .list_messages(&ConversationId("room".into()))
            .unwrap()[0]
            .content,
        "basedurable"
    );
    assert!(append(&storage, 11, "late").is_err());
}

#[test]
fn deletion_cascades_pending_chunks_without_publishing_partial_body() {
    let (_directory, storage) = fixture();
    append(&storage, 4, "partial").unwrap();
    // Branch ownership must be detached before deleting its head; retain the FK.
    storage
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversation_branches SET head_message_id=NULL WHERE id='main'",
            [],
        )
        .unwrap();
    storage.delete_message(&MessageId("a".into())).unwrap();
    let connection = storage.connection().unwrap();
    for table in [
        "pending_assistant_checkpoints",
        "pending_assistant_checkpoint_chunks",
    ] {
        let count: u64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
    drop(connection);
    assert!(append(&storage, 11, "late").is_err());
}

#[test]
fn proof_reuse_invalidates_on_local_write_rollback_and_external_gap() {
    let (_directory, storage) = fixture();
    for index in 0..20 {
        append(&storage, 4 + index, "x").unwrap();
    }
    assert_eq!(storage.checkpoint_proofs.lock().unwrap().validations, 1);
    {
        let mut connection = storage.connection().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("UPDATE conversations SET title='rollback'", [])
            .unwrap();
        transaction.rollback().unwrap();
    }
    append(&storage, 24, "x").unwrap();
    assert_eq!(storage.checkpoint_proofs.lock().unwrap().validations, 2);
    storage
        .connection()
        .unwrap()
        .execute("UPDATE conversations SET title='other'", [])
        .unwrap();
    append(&storage, 25, "x").unwrap();
    assert_eq!(storage.checkpoint_proofs.lock().unwrap().validations, 3);
    assert!(append(&storage, 100, "conflict").is_err());
    append(&storage, 26, "x").unwrap();
    let path: String = storage
        .connection()
        .unwrap()
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name='main'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let external = rusqlite::Connection::open(path).unwrap();
    external
        .execute(
            "DELETE FROM pending_assistant_checkpoint_chunks WHERE sequence=5",
            [],
        )
        .unwrap();
    assert!(append(&storage, 27, "x").is_err());
}
