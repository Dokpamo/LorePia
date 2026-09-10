use rusqlite::{Connection, Error, params};

#[test]
fn persisted_unsigned_counters_keep_the_sqlite_integer_range() {
    let mut connection = Connection::open_in_memory().expect("open SQLite");
    connection
        .execute_batch("CREATE TABLE counters (value INTEGER NOT NULL)")
        .expect("create table");
    for value in [0_u64, 1, i64::MAX as u64] {
        connection
            .execute("INSERT INTO counters VALUES (?)", [value])
            .expect("store representable unsigned value");
        let (kind, restored): (String, u64) = connection
            .query_row(
                "SELECT typeof(value), value FROM counters ORDER BY rowid DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("read unsigned value");
        assert_eq!(kind, "integer");
        assert_eq!(restored, value);
    }

    for value in [(i64::MAX as u64) + 1, u64::MAX] {
        let transaction = connection.transaction().expect("begin transaction");
        transaction
            .execute("INSERT INTO counters VALUES (17)", [])
            .expect("write before rejected counter");
        assert!(matches!(
            transaction.execute("INSERT INTO counters VALUES (?)", params![value]),
            Err(Error::ToSqlConversionFailure(_))
        ));
        transaction.rollback().expect("rollback rejected operation");
    }
    let count: u64 = connection
        .query_row("SELECT COUNT(*) FROM counters", [], |row| row.get(0))
        .expect("count committed counters");
    assert_eq!(count, 3);

    assert!(matches!(
        connection.query_row("SELECT -1", [], |row| row.get::<_, u64>(0)),
        Err(Error::IntegralValueOutOfRange(0, -1))
    ));
    assert!(matches!(
        connection.query_row("SELECT 1.5", [], |row| row.get::<_, u64>(0)),
        Err(Error::InvalidColumnType(..))
    ));
}
