use super::SQL;
use rusqlite::{Connection, OptionalExtension, StatementStatus};

const BASELINE: &str = include_str!("baseline.sql");
const NOW: &str = "2026-09-12T00:00:00Z";

#[test]
fn durable_eligibility_matches_baseline_without_per_candidate_history_scans() {
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE task_profile_revisions(revision_id TEXT PRIMARY KEY, concurrency_limit INTEGER,rate_limit_per_seconds INTEGER,rate_limit_requests INTEGER);
        CREATE TABLE memory_jobs(id TEXT PRIMARY KEY,state TEXT,attempts INTEGER,available_at TEXT,created_at TEXT,task_profile_revision_id TEXT,payload_json TEXT,started_at TEXT);
        CREATE INDEX memory_jobs_queue ON memory_jobs(state,available_at,created_at,id);
        INSERT INTO task_profile_revisions VALUES ('task',2,60,3);
        WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1030)
        INSERT INTO memory_jobs SELECT printf('%08d',x),CASE WHEN x>1000 THEN 'queued' ELSE 'completed' END,
            1,'2020-01-01','2020-01-01','task','{\"attempt_started_at\":[\"2020-01-01\"]}','2020-01-01' FROM n;").unwrap();
    let (expected, original_work) = selected(&db, BASELINE);
    let (actual, work) = selected(&db, SQL);
    assert_eq!(actual, expected);
    assert!(work * 5 < original_work, "new={work}, old={original_work}");
    for update in [
        "UPDATE memory_jobs SET payload_json='{\"attempt_started_at\":[\"2026-09-12T00:00:00Z\",\"2026-09-12T00:00:00Z\",\"2026-09-12T00:00:00Z\"]}' WHERE id='00000001'",
        "UPDATE memory_jobs SET payload_json='{}',started_at='2026-09-12T00:00:00Z' WHERE id IN ('00000001','00000002','00000003')",
        "UPDATE memory_jobs SET state='running' WHERE id IN ('00000001','00000002')",
        "UPDATE memory_jobs SET task_profile_revision_id=NULL WHERE id='00001001'",
        "UPDATE memory_jobs SET attempts=32 WHERE id='00001001'",
        "UPDATE memory_jobs SET task_profile_revision_id='missing' WHERE state='queued'",
    ] {
        db.execute_batch(update).unwrap();
        assert_eq!(selected(&db, SQL).0, selected(&db, BASELINE).0, "{update}");
    }
}

fn selected(db: &Connection, sql: &str) -> (Option<String>, i32) {
    let mut statement = db.prepare(sql).unwrap();
    let id = statement
        .query_row([NOW], |row| row.get(0))
        .optional()
        .unwrap();
    (id, statement.get_status(StatementStatus::VmStep))
}
