use super::*;

#[test]
fn relevant_writes_rollback_replace_and_truncate_are_monotonic() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE messages(id TEXT PRIMARY KEY, content TEXT); CREATE TABLE settings(value TEXT); INSERT INTO messages VALUES('a','first');").unwrap();
    let tracking = ChangeTracking::install(&connection).unwrap();
    let epoch = tracking.lineage_epoch();
    connection
        .execute("INSERT INTO settings VALUES('setting')", [])
        .unwrap();
    assert_eq!(tracking.lineage_epoch(), epoch);
    {
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("UPDATE messages SET content='rolled back'", [])
            .unwrap();
        transaction.rollback().unwrap();
    }
    assert!(tracking.lineage_epoch() > epoch);
    let epoch = tracking.lineage_epoch();
    connection
        .execute(
            "INSERT OR REPLACE INTO messages VALUES('a','replacement')",
            [],
        )
        .unwrap();
    assert!(tracking.lineage_epoch() > epoch);
    // Both executions must invoke the row hook, even with a cached DELETE-all.
    for _ in 0..2 {
        let epoch = tracking.lineage_epoch();
        connection
            .prepare_cached("DELETE FROM messages")
            .unwrap()
            .execute([])
            .unwrap();
        assert!(tracking.lineage_epoch() > epoch);
        connection
            .execute("INSERT INTO messages VALUES('a','again')", [])
            .unwrap();
    }
    let epoch = tracking.lineage_epoch();
    connection
        .execute_batch(
            "DROP TABLE messages; CREATE TABLE messages(id TEXT PRIMARY KEY,content TEXT);",
        )
        .unwrap();
    assert!(tracking.lineage_epoch() > epoch);
}

#[test]
fn snapshot_tokens_bind_instance_branch_and_all_local_changes() {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE messages(id TEXT PRIMARY KEY,content TEXT); CREATE TABLE settings(value TEXT); INSERT INTO messages VALUES('a','first');").unwrap();
    let tracking = ChangeTracking::install(&connection).unwrap();
    let first = tracking
        .history_snapshot_token(&connection, "branch")
        .unwrap();
    assert_eq!(
        first,
        tracking
            .history_snapshot_token(&connection, "branch")
            .unwrap()
    );
    assert_ne!(
        first,
        tracking
            .history_snapshot_token(&connection, "other")
            .unwrap()
    );
    connection
        .execute("UPDATE messages SET content='same id, different body'", [])
        .unwrap();
    let changed = tracking
        .history_snapshot_token(&connection, "branch")
        .unwrap();
    assert_ne!(first, changed);
    connection
        .execute("INSERT INTO settings VALUES('unrelated')", [])
        .unwrap();
    assert_ne!(
        changed,
        tracking
            .history_snapshot_token(&connection, "branch")
            .unwrap()
    );
    let restarted = ChangeTracking::install(&connection).unwrap();
    assert_ne!(
        tracking
            .history_snapshot_token(&connection, "branch")
            .unwrap(),
        restarted
            .history_snapshot_token(&connection, "branch")
            .unwrap()
    );
    tracking.lineage.store(u64::MAX, Ordering::Relaxed);
    advance(&tracking.lineage);
    assert_eq!(tracking.lineage_epoch(), u64::MAX);
}
