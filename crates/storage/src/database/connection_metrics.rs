use std::{
    ops::{Deref, DerefMut},
    sync::{
        Mutex, MutexGuard, TryLockError,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use rusqlite::Connection;

/// Process-local observations for the repository-wide SQLite connection lock.
///
/// Durations are cumulative or maximum monotonic-clock nanoseconds. Counters
/// saturate instead of wrapping so long-lived sessions remain diagnosable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DatabaseConnectionMetrics {
    /// Successful connection-lock acquisitions.
    pub acquisitions: u64,
    /// Attempts that observed the connection lock already held.
    pub contended_acquisitions: u64,
    /// Cumulative successful-acquisition wait time.
    pub total_wait_ns: u64,
    /// Longest successful-acquisition wait time.
    pub max_wait_ns: u64,
    /// Cumulative time completed guards held the connection lock.
    pub total_hold_ns: u64,
    /// Longest time a completed guard held the connection lock.
    pub max_hold_ns: u64,
}

#[derive(Debug, Default)]
pub(super) struct DatabaseConnectionMetricState {
    acquisitions: AtomicU64,
    contended_acquisitions: AtomicU64,
    total_wait_ns: AtomicU64,
    max_wait_ns: AtomicU64,
    total_hold_ns: AtomicU64,
    max_hold_ns: AtomicU64,
}

impl DatabaseConnectionMetricState {
    pub(super) fn acquire<'a>(
        &'a self,
        connection: &'a Mutex<Connection>,
    ) -> CoreResult<DatabaseConnectionGuard<'a>> {
        let started_at = Instant::now();
        let guard = match connection.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                self.record_contention();
                connection.lock().map_err(|_| database_lock_poisoned())?
            }
            Err(TryLockError::Poisoned(_)) => return Err(database_lock_poisoned()),
        };
        let acquired_at = Instant::now();
        self.record_acquisition(duration_ns(acquired_at.duration_since(started_at)));
        Ok(DatabaseConnectionGuard {
            guard,
            metrics: self,
            acquired_at,
        })
    }

    pub(super) fn snapshot(&self) -> DatabaseConnectionMetrics {
        DatabaseConnectionMetrics {
            acquisitions: self.acquisitions.load(Ordering::Relaxed),
            contended_acquisitions: self.contended_acquisitions.load(Ordering::Relaxed),
            total_wait_ns: self.total_wait_ns.load(Ordering::Relaxed),
            max_wait_ns: self.max_wait_ns.load(Ordering::Relaxed),
            total_hold_ns: self.total_hold_ns.load(Ordering::Relaxed),
            max_hold_ns: self.max_hold_ns.load(Ordering::Relaxed),
        }
    }

    fn record_contention(&self) {
        saturating_atomic_add(&self.contended_acquisitions, 1);
    }

    fn record_acquisition(&self, wait_ns: u64) {
        saturating_atomic_add(&self.acquisitions, 1);
        saturating_atomic_add(&self.total_wait_ns, wait_ns);
        self.max_wait_ns.fetch_max(wait_ns, Ordering::Relaxed);
    }

    fn record_hold(&self, hold_ns: u64) {
        saturating_atomic_add(&self.total_hold_ns, hold_ns);
        self.max_hold_ns.fetch_max(hold_ns, Ordering::Relaxed);
    }
}

pub(crate) struct DatabaseConnectionGuard<'a> {
    guard: MutexGuard<'a, Connection>,
    metrics: &'a DatabaseConnectionMetricState,
    acquired_at: Instant,
}

impl Deref for DatabaseConnectionGuard<'_> {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for DatabaseConnectionGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

impl Drop for DatabaseConnectionGuard<'_> {
    fn drop(&mut self) {
        self.metrics
            .record_hold(duration_ns(self.acquired_at.elapsed()));
    }
}

fn saturating_atomic_add(counter: &AtomicU64, value: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_add(value))
    });
}

fn duration_ns(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn database_lock_poisoned() -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        "database lock was poisoned",
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Arc, Barrier, Mutex},
        thread,
        time::{Duration, Instant},
    };

    use rusqlite::{Connection, params};
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::super::{SCHEMA_VERSION, Storage, read_current_schema_version};
    use super::DatabaseConnectionMetricState;

    #[test]
    fn metrics_observe_wait_hold_and_contention() {
        let connection = Arc::new(Mutex::new(
            Connection::open_in_memory().expect("in-memory database"),
        ));
        let metrics = Arc::new(DatabaseConnectionMetricState::default());
        let held_connection = connection.lock().expect("hold raw connection lock");
        let start = Arc::new(Barrier::new(2));
        let worker_connection = Arc::clone(&connection);
        let worker_metrics = Arc::clone(&metrics);
        let worker_start = Arc::clone(&start);
        let worker = thread::spawn(move || {
            worker_start.wait();
            let guard = worker_metrics
                .acquire(&worker_connection)
                .expect("acquire measured connection");
            thread::sleep(Duration::from_millis(1));
            guard
                .query_row("SELECT 40 + 2", [], |row| row.get::<_, i64>(0))
                .expect("query through measured guard")
        });
        start.wait();

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if metrics.snapshot().contended_acquisitions == 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "connection waiter did not report deterministic contention"
            );
            thread::yield_now();
        }
        assert_eq!(metrics.snapshot().acquisitions, 0);
        drop(held_connection);

        assert_eq!(worker.join().expect("database waiter thread"), 42);
        let completed = metrics.snapshot();
        assert_eq!(completed.acquisitions, 1);
        assert_eq!(completed.contended_acquisitions, 1);
        assert!(completed.total_wait_ns > 0);
        assert!(completed.max_wait_ns > 0);
        assert!(completed.total_hold_ns > 0);
        assert!(completed.max_hold_ns > 0);
    }

    #[test]
    fn package_source_publication_releases_database_mutex_during_file_io() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let bytes = b"package publication may pause without blocking SQLite";
        let staged = storage.staging_dir().join("database-unlocked.snapshot");
        fs::write(&staged, bytes).expect("write staged package source");
        let sha256 = hex::encode(Sha256::digest(bytes));
        let import_id = "package-database-unlocked";
        let size_bytes = u64::try_from(bytes.len()).expect("small package source");

        let promoted_path = storage
            .promote_package_source_observed(import_id, &staged, &sha256, size_bytes, || {
                let connection = storage
                    .connection
                    .try_lock()
                    .expect("SQLite mutex must be free while CAS file copy is paused");
                assert_eq!(
                    read_current_schema_version(&connection).expect("read schema version"),
                    SCHEMA_VERSION
                );
                assert_eq!(
                    connection
                        .query_row(
                            "SELECT phase FROM package_cas_promotion_journal
                                 WHERE import_id = ?1 AND namespace = 'source' AND sha256 = ?2",
                            params![import_id, sha256],
                            |row| row.get::<_, String>(0),
                        )
                        .expect("read durable promotion phase"),
                    "intent"
                );
            })
            .expect("promote source");
        assert!(promoted_path.is_file());
    }
}
