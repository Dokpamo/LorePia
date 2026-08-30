use chrono::Utc;
use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use rusqlite::{Connection, OptionalExtension, params};

use super::{
    migration_provider_v4::{migrate_legacy_provider_catalog, validate_provider_catalog_migration},
    migration_registry as sql,
    migration_special::{
        apply_generation_attempt_identity_migration, truncate_sensitive_migration_wal,
        validate_legacy_messages_for_branch_migration,
    },
    migration_verification::{read_current_schema_version, read_pre_migration_schema_version},
    register_integrity_functions, storage_db_error,
    validate_provider_local_network_approval_integrity,
};

#[allow(clippy::too_many_lines)]
pub(crate) fn apply_migrations(connection: &mut Connection) -> CoreResult<()> {
    register_integrity_functions(connection)?;
    let current_version = read_pre_migration_schema_version(connection)?;
    // Schema five is the boundary that purged arbitrary legacy discovery
    // payloads. Checkpoint before attempting any later migration so a prior
    // open that failed after schema five cannot strand those deleted bytes in
    // the WAL indefinitely.
    if current_version >= 5 {
        truncate_sensitive_migration_wal(connection)?;
    }
    if current_version < 1 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0001)
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![1, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 2 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0002)
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![2, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 3 {
        validate_legacy_messages_for_branch_migration(connection)?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0003)
            .map_err(storage_db_error)?;
        let foreign_key_violation = {
            let mut statement = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage_db_error)?;
            statement
                .query_row([], |_| Ok(()))
                .optional()
                .map_err(storage_db_error)?
                .is_some()
        };
        if foreign_key_violation {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "conversation branch migration produced a foreign-key violation",
                false,
            ));
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![3, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 4 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0004)
            .map_err(storage_db_error)?;
        migrate_legacy_provider_catalog(&transaction)?;
        validate_provider_catalog_migration(&transaction)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![4, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 5 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0005)
            .map_err(storage_db_error)?;
        let foreign_key_violation = {
            let mut statement = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage_db_error)?;
            statement
                .query_row([], |_| Ok(()))
                .optional()
                .map_err(storage_db_error)?
                .is_some()
        };
        if foreign_key_violation {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider discovery migration produced a foreign-key violation",
                false,
            ));
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![5, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        // Do this immediately after the redaction migration. If a later
        // migration fails, the legacy credential-bearing pages have already
        // been overwritten and removed from the WAL.
        truncate_sensitive_migration_wal(connection)?;
    }
    if current_version < 6 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0006)
            .map_err(storage_db_error)?;
        let foreign_key_violation = {
            let mut statement = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage_db_error)?;
            statement
                .query_row([], |_| Ok(()))
                .optional()
                .map_err(storage_db_error)?
                .is_some()
        };
        if foreign_key_violation {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation provider provenance migration produced a foreign-key violation",
                false,
            ));
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![6, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 7 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0007)
            .map_err(storage_db_error)?;
        let foreign_key_violation = {
            let mut statement = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage_db_error)?;
            statement
                .query_row([], |_| Ok(()))
                .optional()
                .map_err(storage_db_error)?
                .is_some()
        };
        if foreign_key_violation {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "signed catalog history migration produced a foreign-key violation",
                false,
            ));
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![7, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 8 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0008)
            .map_err(storage_db_error)?;
        let foreign_key_violation = {
            let mut statement = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage_db_error)?;
            statement
                .query_row([], |_| Ok(()))
                .optional()
                .map_err(storage_db_error)?
                .is_some()
        };
        if foreign_key_violation {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation protocol-state migration produced a foreign-key violation",
                false,
            ));
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![8, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 9 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0009)
            .map_err(storage_db_error)?;
        let foreign_key_violation = {
            let mut statement = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage_db_error)?;
            statement
                .query_row([], |_| Ok(()))
                .optional()
                .map_err(storage_db_error)?
                .is_some()
        };
        if foreign_key_violation {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "model synchronization migration produced a foreign-key violation",
                false,
            ));
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![9, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 10 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0010)
            .map_err(storage_db_error)?;
        let foreign_key_violation = {
            let mut statement = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage_db_error)?;
            statement
                .query_row([], |_| Ok(()))
                .optional()
                .map_err(storage_db_error)?
                .is_some()
        };
        if foreign_key_violation {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider connection tombstone migration produced a foreign-key violation",
                false,
            ));
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![10, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    if current_version < 11 {
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .execute_batch(sql::MIGRATION_0011)
            .map_err(storage_db_error)?;
        // Migration 0011 mirrors the typed LAN grant into a relational table,
        // but SQL alone cannot prove that each address is canonical, private,
        // sorted, unique, and bound to an IP-literal origin. Validate those
        // Rust invariants before recording schema 11 so a malformed schema-10
        // row rolls the entire migration back.
        validate_provider_local_network_approval_integrity(&transaction)?;
        let foreign_key_violation = {
            let mut statement = transaction
                .prepare("PRAGMA foreign_key_check")
                .map_err(storage_db_error)?;
            statement
                .query_row([], |_| Ok(()))
                .optional()
                .map_err(storage_db_error)?
                .is_some()
        };
        if foreign_key_violation {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider local-network approval migration produced a foreign-key violation",
                false,
            ));
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![11, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
    }
    apply_checked_migration(
        connection,
        current_version,
        12,
        sql::MIGRATION_0012,
        "content-package foundation",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        13,
        sql::MIGRATION_0013,
        "prompt pipeline",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        14,
        sql::MIGRATION_0014,
        "knowledge",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        15,
        sql::MIGRATION_0015,
        "memory",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        16,
        sql::MIGRATION_0016,
        "transforms",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        17,
        sql::MIGRATION_0017,
        "interactions and content modules",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        18,
        sql::MIGRATION_0018,
        "persona selection",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        19,
        sql::MIGRATION_0019,
        "core lifecycle outbox",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        20,
        sql::MIGRATION_0020,
        "package CAS promotion journal",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        21,
        sql::MIGRATION_0021,
        "interaction message checkpoints",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        22,
        sql::MIGRATION_0022,
        "memory vector-space identity",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        23,
        sql::MIGRATION_0023,
        "applied module runtime plans",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        24,
        sql::MIGRATION_0024,
        "generation attempt proposals",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        25,
        sql::MIGRATION_0025,
        "conversation greeting bindings",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        26,
        sql::MIGRATION_0026,
        "provider discovery native no-effect recovery",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        27,
        sql::MIGRATION_0027,
        "provider discovery native no-effect attestations",
    )?;
    apply_generation_attempt_identity_migration(connection, current_version)?;
    apply_checked_migration(
        connection,
        current_version,
        29,
        sql::MIGRATION_0029,
        "generation attempt decision handshake",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        30,
        sql::MIGRATION_0030,
        "package document target reviews",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        31,
        sql::MIGRATION_0031,
        "message display projections",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        32,
        sql::MIGRATION_0032,
        "knowledge embedding vector spaces",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        33,
        sql::MIGRATION_0033,
        "interaction derived-event outbox",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        34,
        sql::MIGRATION_0034,
        "generation attempt derived-event authority",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        35,
        sql::MIGRATION_0035,
        "interaction derived-event terminal quarantine",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        36,
        sql::MIGRATION_0036,
        "generation attempt derived closure and evaluation seal",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        37,
        sql::MIGRATION_0037,
        "provider credential operation journal",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        38,
        sql::MIGRATION_0038,
        "conversation speaker roster and message attribution",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        39,
        sql::MIGRATION_0039,
        "portable runtime model audit",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        40,
        sql::MIGRATION_0040,
        "portable runtime state",
    )?;
    read_current_schema_version(connection)?;
    Ok(())
}

fn apply_checked_migration(
    connection: &mut Connection,
    current_version: u32,
    version: u32,
    sql: &str,
    label: &str,
) -> CoreResult<()> {
    if current_version >= version {
        return Ok(());
    }
    let transaction = connection.transaction().map_err(storage_db_error)?;
    transaction.execute_batch(sql).map_err(storage_db_error)?;
    let foreign_key_violation = {
        let mut statement = transaction
            .prepare("PRAGMA foreign_key_check")
            .map_err(storage_db_error)?;
        statement
            .query_row([], |_| Ok(()))
            .optional()
            .map_err(storage_db_error)?
            .is_some()
    };
    if foreign_key_violation {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("{label} migration produced a foreign-key violation"),
            false,
        ));
    }
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, Utc::now().to_rfc3339()],
        )
        .map_err(storage_db_error)?;
    transaction.commit().map_err(storage_db_error)
}
