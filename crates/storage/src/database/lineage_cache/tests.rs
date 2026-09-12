use super::*;

#[test]
fn snapshot_reuse_detects_local_and_external_changes_with_unchanged_head() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("synthetic.sqlite3");
    let mut connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
         CREATE TABLE messages(id TEXT PRIMARY KEY, conversation_id TEXT, parent_id TEXT);
         INSERT INTO messages VALUES ('a','room',NULL),('b','room','a');",
        )
        .unwrap();
    let external = Connection::open(&path).unwrap();
    let tracking = crate::database::change_tracking::ChangeTracking::install(&connection).unwrap();
    let mut cache = LineageCache::default();
    let first = read(&mut connection, &mut cache, &tracking);
    let second = read(&mut connection, &mut cache, &tracking);
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(cache.validations, 1);
    connection
        .execute("UPDATE messages SET parent_id=NULL WHERE id='b'", [])
        .unwrap();
    assert_eq!(
        read(&mut connection, &mut cache, &tracking).as_slice(),
        ["b"]
    );
    external
        .execute("UPDATE messages SET parent_id='a' WHERE id='b'", [])
        .unwrap();
    assert_eq!(
        read(&mut connection, &mut cache, &tracking).as_slice(),
        ["b", "a"]
    );

    let transaction = connection.transaction().unwrap();
    transaction
        .query_row("SELECT id FROM messages WHERE id='b'", [], |_| Ok(()))
        .unwrap();
    let snapshot = cache
        .load(&transaction, "room", Some("b"), tracking.lineage_epoch())
        .unwrap();
    external
        .execute("UPDATE messages SET parent_id='missing' WHERE id='b'", [])
        .unwrap();
    assert!(Arc::ptr_eq(
        &snapshot,
        &cache
            .load(&transaction, "room", Some("b"), tracking.lineage_epoch())
            .unwrap()
    ));
    transaction.commit().unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .query_row("SELECT id FROM messages WHERE id='b'", [], |_| Ok(()))
        .unwrap();
    assert!(
        cache
            .load(&transaction, "room", Some("b"), tracking.lineage_epoch())
            .is_err()
    );
    assert_eq!(cache.validations, 4);
}

fn read(
    connection: &mut Connection,
    cache: &mut LineageCache,
    tracking: &crate::database::change_tracking::ChangeTracking,
) -> Arc<LineageSnapshot> {
    let transaction = connection.transaction().unwrap();
    transaction
        .query_row("SELECT id FROM messages WHERE id='b'", [], |_| Ok(()))
        .unwrap();
    let ids = cache
        .load(&transaction, "room", Some("b"), tracking.lineage_epoch())
        .unwrap();
    transaction.commit().unwrap();
    ids
}

#[test]
fn indexed_lookup_and_budget_include_index_and_disable_saturated_reuse() {
    let snapshot = LineageSnapshot::new(
        (0..100_000)
            .rev()
            .map(|index| format!("m{index:06}"))
            .collect(),
    );
    for (id, position) in [("m099999", 0), ("m050000", 49_999), ("m000000", 99_999)] {
        assert_eq!(snapshot.position(id), Some(position));
    }
    assert_eq!(snapshot.position("missing"), None);
    assert_eq!(snapshot.by_id.len(), 100_000);
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT,parent_id TEXT); INSERT INTO messages VALUES('b','room',NULL);").unwrap();
    let mut cache = LineageCache::default();
    for _ in 0..2 {
        let transaction = connection.transaction().unwrap();
        cache
            .load(&transaction, "room", Some("b"), u64::MAX)
            .unwrap();
    }
    assert_eq!(cache.validations, 2);
    assert!(cache.entry.is_none());
    let oversized = "z".repeat(MAX_CACHE_BYTES);
    connection
        .execute("INSERT INTO messages VALUES(?1,'room',NULL)", [&oversized])
        .unwrap();
    let transaction = connection.transaction().unwrap();
    let snapshot = cache
        .load(&transaction, "room", Some(&oversized), 0)
        .unwrap();
    assert_eq!(snapshot.len(), 1);
    assert!(cache.entry.is_none());
}

#[test]
fn unchanged_ancestry_reuses_after_unrelated_writes_but_not_rollback_or_delete() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection.execute_batch("CREATE TABLE messages(id TEXT PRIMARY KEY, conversation_id TEXT,parent_id TEXT); CREATE TABLE settings(value TEXT); INSERT INTO messages VALUES('a','room',NULL),('b','room','a');").unwrap();
    let tracking = crate::database::change_tracking::ChangeTracking::install(&connection).unwrap();
    let mut cache = LineageCache::default();
    let first = read(&mut connection, &mut cache, &tracking);
    connection
        .execute("INSERT INTO settings VALUES('unrelated')", [])
        .unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &read(&mut connection, &mut cache, &tracking)
    ));
    {
        let transaction = connection.transaction().unwrap();
        transaction
            .execute("UPDATE messages SET parent_id=NULL WHERE id='b'", [])
            .unwrap();
        let uncommitted = cache
            .load(&transaction, "room", Some("b"), tracking.lineage_epoch())
            .unwrap();
        assert_eq!(uncommitted.as_slice(), ["b"]);
        assert!(cache.entry.is_none());
        transaction.rollback().unwrap();
    }
    assert_eq!(
        read(&mut connection, &mut cache, &tracking).as_slice(),
        ["b", "a"]
    );
    assert_eq!(cache.validations, 3);
    connection
        .execute("DELETE FROM messages WHERE id='a'", [])
        .unwrap();
    let transaction = connection.transaction().unwrap();
    assert!(
        cache
            .load(&transaction, "room", Some("b"), tracking.lineage_epoch())
            .is_err()
    );
}

#[test]
fn unsupported_schema_and_temporary_shadow_cannot_establish_row_hook_proofs() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("schema.sqlite3");
    let mut connection = Connection::open(&path).unwrap();
    let external = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT,parent_id TEXT); INSERT INTO messages VALUES('b','room',NULL);").unwrap();
    let tracking = crate::database::change_tracking::ChangeTracking::install(&connection).unwrap();
    let mut cache = LineageCache::default();
    read(&mut connection, &mut cache, &tracking);
    assert!(cache.entry.is_some());
    external.execute_batch("DROP TABLE messages; CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT,parent_id TEXT) WITHOUT ROWID; INSERT INTO messages VALUES('b','room',NULL);").unwrap();
    read(&mut connection, &mut cache, &tracking);
    assert!(cache.entry.is_none());
    connection.execute_batch("DROP TABLE messages; CREATE TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT,parent_id TEXT); INSERT INTO messages VALUES('b','room',NULL); CREATE TEMP TABLE messages(id TEXT PRIMARY KEY,conversation_id TEXT,parent_id TEXT); INSERT INTO temp.messages VALUES('b','room',NULL);").unwrap();
    read(&mut connection, &mut cache, &tracking);
    assert!(cache.entry.is_none());
    connection.execute_batch("INSERT INTO temp.messages VALUES('a','room',NULL); UPDATE temp.messages SET parent_id='a' WHERE id='b';").unwrap();
    assert_eq!(
        read(&mut connection, &mut cache, &tracking).as_slice(),
        ["b", "a"]
    );
    assert!(cache.entry.is_none());
}
