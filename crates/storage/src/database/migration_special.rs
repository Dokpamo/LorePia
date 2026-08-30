use chrono::Utc;
use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use rusqlite::{Connection, OptionalExtension};

use super::{migration_registry::MIGRATION_0028, storage_corrupted, storage_db_error};
use crate::interaction_repository::validate_generation_attempt_identity_migration_legacy_rows;

pub(super) fn apply_generation_attempt_identity_migration(
    connection: &mut Connection,
    current_version: u32,
) -> CoreResult<()> {
    if current_version >= 28 {
        return Ok(());
    }
    let transaction = connection.transaction().map_err(storage_db_error)?;
    validate_generation_attempt_identity_migration_legacy_rows(&transaction)?;
    transaction
        .execute_batch(MIGRATION_0028)
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
        return Err(storage_corrupted(
            "generation attempt storage identity migration produced a foreign-key violation",
        ));
    }
    transaction
        .execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (28, ?1)",
            [Utc::now().to_rfc3339()],
        )
        .map_err(storage_db_error)?;
    transaction.commit().map_err(storage_db_error)
}

pub(super) fn validate_legacy_messages_for_branch_migration(
    connection: &Connection,
) -> CoreResult<()> {
    let invalid_enum_count = legacy_branch_migration_count(
        connection,
        "SELECT COUNT(*)
             FROM messages
             WHERE role NOT IN ('system', 'user', 'assistant')
                OR status NOT IN ('pending', 'complete', 'cancelled', 'failed')
                OR (role = 'assistant' AND generation_id IS NULL)
                OR (role = 'assistant' AND parent_id IS NULL)
                OR (
                  role = 'assistant'
                  AND NOT EXISTS (
                    SELECT 1
                    FROM messages AS parent
                    WHERE parent.conversation_id = messages.conversation_id
                      AND parent.id = messages.parent_id
                      AND parent.role = 'user'
                  )
                )
                OR (role <> 'assistant' AND generation_id IS NOT NULL)
                OR (role <> 'assistant' AND status <> 'complete')",
    )?;
    if invalid_enum_count != 0 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "legacy messages contain invalid role, status, or generation ownership",
            false,
        ));
    }
    let duplicate_generation_count = legacy_branch_migration_count(
        connection,
        "SELECT COUNT(*)
             FROM (
               SELECT generation_id
               FROM messages
               WHERE generation_id IS NOT NULL
               GROUP BY generation_id
               HAVING COUNT(*) > 1
             )",
    )?;
    if duplicate_generation_count != 0 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "legacy messages reuse a generation id",
            false,
        ));
    }
    let inconsistent_parent_count = legacy_branch_migration_count(
        connection,
        "WITH migration_order AS (
               SELECT message.id,
                      message.conversation_id,
                      message.parent_id,
                      message.role,
                      message.created_at,
                      CASE
                        WHEN message.role = 'assistant' THEN parent.created_at
                        ELSE message.created_at
                      END AS turn_created_at,
                      CASE
                        WHEN message.role = 'assistant' THEN parent.id
                        ELSE message.id
                      END AS turn_id,
                      CASE
                        WHEN message.role = 'assistant' THEN 1
                        ELSE 0
                      END AS turn_position
               FROM messages AS message
               LEFT JOIN messages AS parent
                 ON message.role = 'assistant'
                AND parent.conversation_id = message.conversation_id
                AND parent.id = message.parent_id
                AND parent.role = 'user'
             ),
             lineage AS (
               SELECT parent_id,
                      LAG(id) OVER (
                        PARTITION BY conversation_id
                        ORDER BY turn_created_at, turn_id, turn_position, created_at, id
                      ) AS expected_parent_id
               FROM migration_order
             )
             SELECT COUNT(*)
             FROM lineage
             WHERE parent_id IS NOT NULL
               AND parent_id IS NOT expected_parent_id",
    )?;
    if inconsistent_parent_count != 0 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "legacy message parents disagree with the persisted timeline order",
            false,
        ));
    }
    Ok(())
}

fn legacy_branch_migration_count(connection: &Connection, query: &str) -> CoreResult<u64> {
    connection
        .query_row(query, [], |row| row.get::<_, u64>(0))
        .map_err(storage_db_error)
}

pub(crate) fn truncate_sensitive_migration_wal(connection: &Connection) -> CoreResult<()> {
    let (busy, remaining_frames, checkpointed_frames) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(storage_db_error)?;
    if busy != 0 || remaining_frames != 0 || checkpointed_frames != 0 {
        return Err(storage_corrupted(format!(
            "sensitive discovery migration WAL purge did not complete \
             (busy={busy}, remaining={remaining_frames}, checkpointed={checkpointed_frames})"
        )));
    }
    Ok(())
}
