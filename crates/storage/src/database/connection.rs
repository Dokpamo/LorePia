use std::{path::Path, sync::MutexGuard, time::Duration};

use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use rusqlite::{Connection, OpenFlags};

use super::{
    DatabaseConnectionGuard, DatabaseConnectionMetrics, Storage, pragmas::configure_connection,
    storage_db_error,
};

pub(crate) fn reserve_source_writes(path: &Path) -> CoreResult<Connection> {
    let reservation = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_db_error)?;
    reservation
        .busy_timeout(Duration::from_secs(5))
        .map_err(storage_db_error)?;
    reservation
        .execute_batch("BEGIN IMMEDIATE")
        .map_err(storage_db_error)?;
    Ok(reservation)
}

pub(crate) fn open_cutover_source(path: &Path) -> CoreResult<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_db_error)
}

pub(crate) fn open_configured(path: &Path) -> CoreResult<Connection> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_db_error)?;
    configure_connection(&connection)?;
    Ok(connection)
}

pub(crate) fn open_backup_destination(path: &Path) -> CoreResult<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(storage_db_error)
}

impl Storage {
    pub fn database_connection_metrics(&self) -> DatabaseConnectionMetrics {
        self.connection_metrics.snapshot()
    }

    pub(crate) fn connection(&self) -> CoreResult<DatabaseConnectionGuard<'_>> {
        self.connection_metrics.acquire(&self.connection)
    }

    pub(crate) fn cas_mutation(&self) -> CoreResult<MutexGuard<'_, ()>> {
        self.cas_mutation.lock().map_err(|_| {
            CoreError::new(
                CoreErrorCode::StorageUnavailable,
                "CAS mutation lock was poisoned",
                true,
            )
        })
    }
}
