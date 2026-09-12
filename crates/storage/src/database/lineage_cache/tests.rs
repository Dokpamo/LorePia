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
    let mut cache = LineageCache::default();
    let first = read(&mut connection, &mut cache);
    let second = read(&mut connection, &mut cache);
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(cache.validations, 1);
    connection
        .execute("UPDATE messages SET parent_id=NULL WHERE id='b'", [])
        .unwrap();
    assert_eq!(read(&mut connection, &mut cache).as_slice(), ["b"]);
    external
        .execute("UPDATE messages SET parent_id='a' WHERE id='b'", [])
        .unwrap();
    assert_eq!(read(&mut connection, &mut cache).as_slice(), ["b", "a"]);

    let transaction = connection.transaction().unwrap();
    transaction
        .query_row("SELECT id FROM messages WHERE id='b'", [], |_| Ok(()))
        .unwrap();
    let snapshot = cache.load(&transaction, "room", Some("b")).unwrap();
    external
        .execute("UPDATE messages SET parent_id='missing' WHERE id='b'", [])
        .unwrap();
    assert!(Arc::ptr_eq(
        &snapshot,
        &cache.load(&transaction, "room", Some("b")).unwrap()
    ));
    transaction.commit().unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .query_row("SELECT id FROM messages WHERE id='b'", [], |_| Ok(()))
        .unwrap();
    assert!(cache.load(&transaction, "room", Some("b")).is_err());
    assert_eq!(cache.validations, 4);
}

fn read(connection: &mut Connection, cache: &mut LineageCache) -> Arc<Vec<String>> {
    let transaction = connection.transaction().unwrap();
    transaction
        .query_row("SELECT id FROM messages WHERE id='b'", [], |_| Ok(()))
        .unwrap();
    let ids = cache.load(&transaction, "room", Some("b")).unwrap();
    transaction.commit().unwrap();
    ids
}
