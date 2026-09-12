use super::SQL;

#[test]
fn idle_claim_uses_existing_partial_index_with_acknowledged_history() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
    let schema = include_str!("../../../migrations/0019_lifecycle_outbox.sql");
    let start = schema.find("CREATE TABLE core_lifecycle_outbox (").unwrap();
    let end = schema
        .find("CREATE TRIGGER core_lifecycle_outbox_identity_guard")
        .unwrap();
    db.execute_batch(&schema[start..end]).unwrap();
    db.execute_batch("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10000)
        INSERT INTO core_lifecycle_outbox
        (occurrence_id,event_kind,conversation_id,branch_id,occurred_at,available_at,status,acknowledged_at,created_at)
        SELECT printf('%08d',x),'conversation_opened','room','branch','2026','2026','acknowledged','2026','2026' FROM n;
        ANALYZE;").unwrap();
    let mut statement = db.prepare(SQL).unwrap();
    assert!(
        statement
            .query(["2027", "1"])
            .unwrap()
            .next()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        statement.get_status(rusqlite::StatementStatus::FullscanStep),
        0
    );
}
