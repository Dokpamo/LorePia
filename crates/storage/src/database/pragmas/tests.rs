use std::{fs, path::Path};

use rusqlite::Connection;

use super::{MAX_RETAINED_JOURNAL_BYTES, configure_connection};

fn open(path: &Path) -> Connection {
    let connection = Connection::open(path).expect("open synthetic database");
    configure_connection(&connection).expect("configure production pragmas");
    connection
}

fn wal_bytes(path: &Path) -> u64 {
    fs::metadata(path.with_extension("sqlite-wal"))
        .expect("WAL exists while connection is open")
        .len()
}

#[test]
fn large_transaction_reclaims_excess_wal_on_reuse_without_weakening_durability() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("data.sqlite");
    let connection = open(&path);
    for (pragma, expected) in [
        ("foreign_keys", 1),
        ("synchronous", 2),
        ("wal_autocheckpoint", 1000),
        ("journal_size_limit", MAX_RETAINED_JOURNAL_BYTES),
    ] {
        assert_eq!(
            connection
                .pragma_query_value(None, pragma, |r| r.get::<_, i64>(0))
                .expect("pragma"),
            expected,
            "{pragma}"
        );
    }
    connection
        .execute_batch(
            "CREATE TABLE payloads(id INTEGER PRIMARY KEY, bytes BLOB);
         CREATE TABLE markers(id INTEGER PRIMARY KEY);
         INSERT INTO payloads VALUES(1, zeroblob(16777216));",
        )
        .expect("large committed import");
    let peak = wal_bytes(&path);
    assert!(peak > MAX_RETAINED_JOURNAL_BYTES as u64);
    connection
        .execute("INSERT INTO markers VALUES(1)", [])
        .expect("next small transaction");
    let retained = wal_bytes(&path);
    assert!(retained <= MAX_RETAINED_JOURNAL_BYTES as u64);
    eprintln!("WAL bytes after 16 MiB transaction: peak={peak}, retained={retained}");
    connection
        .execute_batch("BEGIN; INSERT INTO markers VALUES(2); ROLLBACK;")
        .expect("rollback remains atomic");
    drop(connection);
    let reopened = open(&path);
    assert_eq!(
        reopened
            .query_row("SELECT length(bytes) FROM payloads", [], |r| r
                .get::<_, i64>(0))
            .expect("durable payload"),
        16 * 1024 * 1024
    );
    assert_eq!(
        reopened
            .query_row("SELECT count(*) FROM markers", [], |r| r.get::<_, i64>(0))
            .expect("only committed marker"),
        1
    );
}

#[test]
fn reader_pinned_wal_stays_intact_until_sqlite_can_reset_it() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("data.sqlite");
    let writer = open(&path);
    writer
        .execute_batch(
            "CREATE TABLE payloads(id INTEGER PRIMARY KEY, bytes BLOB);
         CREATE TABLE markers(id INTEGER PRIMARY KEY);
         PRAGMA wal_checkpoint(TRUNCATE);",
        )
        .expect("initial database");
    let reader = open(&path);
    reader.execute_batch("BEGIN").expect("read transaction");
    assert_eq!(
        reader
            .query_row("SELECT count(*) FROM payloads", [], |r| r.get::<_, i64>(0))
            .expect("pin read snapshot"),
        0
    );
    writer
        .execute("INSERT INTO payloads VALUES(1, zeroblob(16777216))", [])
        .expect("write behind reader");
    writer
        .execute("INSERT INTO markers VALUES(1)", [])
        .expect("next write");
    assert!(wal_bytes(&path) > MAX_RETAINED_JOURNAL_BYTES as u64);
    assert_eq!(
        reader
            .query_row("SELECT count(*) FROM payloads", [], |r| r.get::<_, i64>(0))
            .expect("old snapshot is still intact"),
        0
    );
    reader.execute_batch("ROLLBACK").expect("release reader");
    writer
        .execute_batch("PRAGMA wal_checkpoint(PASSIVE)")
        .expect("complete checkpoint");
    writer
        .execute("INSERT INTO markers VALUES(2)", [])
        .expect("safe WAL reuse");
    assert!(wal_bytes(&path) <= MAX_RETAINED_JOURNAL_BYTES as u64);
    assert_eq!(
        writer
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .expect("database integrity"),
        "ok"
    );
}
