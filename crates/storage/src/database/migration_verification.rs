use lorepia_domain::CoreResult;
use rusqlite::Connection;

use super::{schema::SCHEMA_VERSION, storage_corrupted};

pub(crate) fn read_pre_migration_schema_version(connection: &Connection) -> CoreResult<u32> {
    let registry_exists = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM sqlite_schema
                 WHERE type = 'table' AND name = 'schema_migrations'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| {
            storage_corrupted(format!("cannot inspect schema migration registry: {error}"))
        })?;
    if registry_exists {
        return read_contiguous_schema_version(connection);
    }

    let user_table_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM sqlite_schema
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .map_err(|error| {
            storage_corrupted(format!(
                "cannot inspect unregistered database schema: {error}"
            ))
        })?;
    if user_table_count != 0 {
        return Err(storage_corrupted(
            "database contains tables but its schema migration registry is missing",
        ));
    }
    Ok(0)
}

fn read_contiguous_schema_version(connection: &Connection) -> CoreResult<u32> {
    let mut statement = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .map_err(|error| {
            storage_corrupted(format!(
                "cannot prepare schema migration registry validation: {error}"
            ))
        })?;
    let mut rows = statement.query([]).map_err(|error| {
        storage_corrupted(format!(
            "cannot query schema migration registry for validation: {error}"
        ))
    })?;
    let mut expected_version = 1_u32;
    while let Some(row) = rows.next().map_err(|error| {
        storage_corrupted(format!(
            "cannot advance schema migration registry validation: {error}"
        ))
    })? {
        let raw_version = row.get::<_, i64>(0).map_err(|error| {
            storage_corrupted(format!(
                "schema migration registry contains an unreadable version: {error}"
            ))
        })?;
        let version = u32::try_from(raw_version).map_err(|_| {
            storage_corrupted(format!(
                "schema migration registry contains invalid version {raw_version}"
            ))
        })?;
        if version == 0 {
            return Err(storage_corrupted(
                "schema migration registry contains invalid version zero",
            ));
        }
        if version > SCHEMA_VERSION {
            return Err(storage_corrupted(format!(
                "database schema {version} is newer than supported schema {SCHEMA_VERSION}"
            )));
        }
        if version < expected_version {
            return Err(storage_corrupted(format!(
                "schema migration registry contains duplicate or non-monotonic version {version}"
            )));
        }
        if version > expected_version {
            return Err(storage_corrupted(format!(
                "schema migration registry is missing version {expected_version} before version {version}"
            )));
        }
        expected_version = expected_version
            .checked_add(1)
            .ok_or_else(|| storage_corrupted("schema migration version overflowed"))?;
    }
    Ok(expected_version - 1)
}

pub(crate) fn read_current_schema_version(connection: &Connection) -> CoreResult<u32> {
    let version = read_contiguous_schema_version(connection)?;
    if version != SCHEMA_VERSION {
        return Err(storage_corrupted(format!(
            "database migration registry ended at schema {version}, expected {SCHEMA_VERSION}"
        )));
    }
    Ok(version)
}
