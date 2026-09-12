use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn fixture(count: usize, assistants: &[usize]) -> (Connection, Vec<String>) {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE messages(id TEXT PRIMARY KEY, conversation_id TEXT, parent_id TEXT,
         role TEXT, content TEXT, status TEXT, generation_id TEXT, created_at TEXT);
         CREATE VIEW messages_with_checkpoints AS SELECT * FROM messages;",
        )
        .unwrap();
    let ids = (0..count)
        .map(|index| format!("m{index}"))
        .collect::<Vec<_>>();
    let transaction = connection.transaction().unwrap();
    for (index, id) in ids.iter().enumerate() {
        transaction.execute(
            "INSERT INTO messages VALUES(?1,'room',NULL,?2,?1,'complete',NULL,'2026-09-12T00:00:00Z')",
            params![id, if assistants.contains(&index) {"assistant"} else {"user"}]
        ).unwrap();
    }
    transaction.commit().unwrap();
    (connection, ids)
}

fn page_message(connection: &Connection, id: &str) -> Message {
    connection.query_row("SELECT id,conversation_id,parent_id,role,content,status,generation_id,created_at FROM messages WHERE id=?1",[id],map_message).unwrap()
}

#[test]
fn head_page_assistant_requires_no_sql_and_prefix_scan_does_not_visit_100k_ids() {
    let (connection, mut lineage) = fixture(31, &[30]);
    let page = vec![page_message(&connection, "m30")];
    // A missing schema would fail any attempted query, proving the head shortcut.
    assert!(
        load_last_assistant(
            &Connection::open_in_memory().unwrap(),
            "room",
            &lineage,
            &page,
            0
        )
        .unwrap()
        .is_none()
    );
    lineage.extend((31..100_000).map(|index| format!("m{index}")));
    let steps = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&steps);
    connection
        .progress_handler(
            1,
            Some(move || counter.fetch_add(1, Ordering::Relaxed) >= 5000),
        )
        .unwrap();
    assert!(
        load_last_assistant(&connection, "room", &lineage, &page, 30)
            .unwrap()
            .is_none()
    );
    assert!(steps.load(Ordering::Relaxed) < 5000);
}

#[test]
fn bounded_batches_stop_at_latest_assistant_and_ignore_later_corrupt_bodies() {
    let (connection, lineage) = fixture(400, &[129, 300]);
    connection
        .execute(
            "UPDATE messages SET content=CAST(x'80' AS TEXT) WHERE id='m300'",
            [],
        )
        .unwrap();
    let result = load_last_assistant(&connection, "room", &lineage, &[], 0)
        .unwrap()
        .unwrap();
    assert_eq!(result.id.0, "m129");
    assert_eq!(result.content, "m129");
    let page = vec![page_message(&connection, "m129")];
    assert!(
        load_last_assistant(&connection, "room", &lineage, &page, 129)
            .unwrap()
            .is_none()
    );
}

#[test]
fn newer_assistant_is_returned_but_foreign_roles_and_missing_assistants_are_not() {
    let (connection, lineage) = fixture(40, &[2, 30]);
    let page = vec![page_message(&connection, "m30")];
    assert_eq!(
        load_last_assistant(&connection, "room", &lineage, &page, 30)
            .unwrap()
            .unwrap()
            .id
            .0,
        "m2"
    );
    connection
        .execute(
            "UPDATE messages SET conversation_id='foreign' WHERE id='m2'",
            [],
        )
        .unwrap();
    assert!(
        load_last_assistant(&connection, "room", &lineage, &page, 30)
            .unwrap()
            .is_none()
    );
    let (connection, lineage) = fixture(260, &[]);
    assert!(
        load_last_assistant(&connection, "room", &lineage, &[], 0)
            .unwrap()
            .is_none()
    );
}
