//! Fail-closed startup recovery for package CAS promotion intents.

use super::super::{
    Connection, CoreError, CoreResult, OptionalExtension, Path, TransactionBehavior, Utc,
    ensure_regular_file, i64_to_u64, params, storage_corrupted, storage_db_error, u64_to_i64,
    verify_file,
};
use super::{
    PackageCasPromotionIntent, PackageCasPromotionJournalEntry,
    journal::{
        package_cas_product_reference_exists, remove_package_cas_file,
        validate_package_cas_promotion_intent,
    },
    verify_media_type_signature,
};

pub(in crate::database) fn reject_durable_committing_package_imports(
    connection: &Connection,
) -> CoreResult<()> {
    let exists = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM package_imports WHERE state = 'committing'
             )",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if exists {
        return Err(storage_corrupted(
            "durable package import remained in the impossible committing state",
        ));
    }
    Ok(())
}

pub(in crate::database) fn recover_package_cas_promotions(
    root: &Path,
    connection: &mut Connection,
) -> CoreResult<()> {
    let entries = {
        let mut statement = connection
            .prepare(
                "SELECT import_id, namespace, sha256, size_bytes, media_type,
                        relative_path, phase
                 FROM package_cas_promotion_journal
                 ORDER BY namespace, sha256, import_id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| {
                let namespace = match row.get::<_, String>(1)?.as_str() {
                    "source" => "source",
                    "asset" => "asset",
                    _ => {
                        return Err(rusqlite::Error::InvalidColumnType(
                            1,
                            "namespace".to_owned(),
                            rusqlite::types::Type::Text,
                        ));
                    }
                };
                Ok(PackageCasPromotionJournalEntry {
                    intent: PackageCasPromotionIntent {
                        import_id: row.get(0)?,
                        namespace,
                        sha256: row.get(2)?,
                        size_bytes: row.get::<_, u64>(3)?,
                        media_type: row.get(4)?,
                        relative_path: row.get(5)?,
                    },
                    phase: row.get(6)?,
                })
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    for entry in entries {
        recover_package_cas_promotion(root, connection, &entry)?;
    }
    Ok(())
}

fn recover_package_cas_promotion(
    root: &Path,
    connection: &mut Connection,
    entry: &PackageCasPromotionJournalEntry,
) -> CoreResult<()> {
    validate_package_cas_promotion_intent(&entry.intent).map_err(|error| {
        storage_corrupted(format!("invalid package CAS journal: {}", error.message))
    })?;
    if !matches!(
        entry.phase.as_str(),
        "intent" | "file_durable" | "row_registered" | "cleanup_pending"
    ) {
        return Err(storage_corrupted(
            "package CAS journal phase is unsupported",
        ));
    }
    let referenced = package_cas_product_reference_exists(
        connection,
        entry.intent.namespace,
        &entry.intent.sha256,
    )?;
    let import_state = connection
        .query_row(
            "SELECT state FROM package_imports WHERE id = ?1",
            [entry.intent.import_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    let active_asset_promotion = entry.intent.namespace == "asset"
        && entry.phase == "row_registered"
        && import_state.as_deref().is_some_and(|state| {
            matches!(
                state,
                "inspected" | "awaiting_review" | "approved" | "committing"
            )
        });
    if referenced || active_asset_promotion {
        verify_package_cas_promotion_artifact(root, connection, &entry.intent)?;
        if referenced {
            delete_package_cas_promotion_journal_entry(connection, &entry.intent)?;
        }
        return Ok(());
    }
    let another_active = connection
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM package_cas_promotion_journal AS journal
                JOIN package_imports AS import ON import.id = journal.import_id
                WHERE journal.namespace = ?1
                  AND journal.sha256 = ?2
                  AND journal.import_id <> ?3
                  AND journal.phase = 'row_registered'
                  AND import.state IN (
                      'inspected',
                      'awaiting_review',
                      'approved',
                      'committing'
                  )
             )",
            params![
                entry.intent.namespace,
                entry.intent.sha256,
                entry.intent.import_id,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if another_active {
        verify_package_cas_promotion_artifact(root, connection, &entry.intent)?;
        delete_package_cas_promotion_journal_entry(connection, &entry.intent)?;
        return Ok(());
    }
    recover_abandoned_package_cas_promotion(root, connection, &entry.intent)
}

fn verify_package_cas_promotion_artifact(
    root: &Path,
    connection: &Connection,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<()> {
    let stored = match intent.namespace {
        "source" => connection
            .query_row(
                "SELECT relative_path, size_bytes, NULL
                 FROM content_sources WHERE sha256 = ?1",
                [intent.sha256.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?,
        "asset" => connection
            .query_row(
                "SELECT relative_path, size_bytes, media_type
                 FROM assets WHERE sha256 = ?1",
                [intent.sha256.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?,
        _ => None,
    }
    .ok_or_else(|| storage_corrupted("package CAS journal references a missing metadata row"))?;
    if stored.0 != intent.relative_path
        || i64_to_u64("package CAS journal size", stored.1)? != intent.size_bytes
        || stored.2 != intent.media_type
    {
        return Err(storage_corrupted(
            "package CAS journal metadata differs from its registered row",
        ));
    }
    let path = root.join(&intent.relative_path);
    ensure_regular_file(&path)?;
    verify_file(&path, &intent.sha256, intent.size_bytes)?;
    if let Some(media_type) = &intent.media_type {
        verify_media_type_signature(&path, media_type)?;
    }
    Ok(())
}

fn delete_package_cas_promotion_journal_entry(
    connection: &mut Connection,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<()> {
    let changed = connection
        .execute(
            "DELETE FROM package_cas_promotion_journal
             WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
            params![intent.import_id, intent.namespace, intent.sha256],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "package CAS recovery journal entry disappeared",
        ));
    }
    Ok(())
}

fn recover_abandoned_package_cas_promotion(
    root: &Path,
    connection: &mut Connection,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "UPDATE package_cas_promotion_journal
             SET phase = 'cleanup_pending', updated_at = ?4
             WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
            params![
                intent.import_id,
                intent.namespace,
                intent.sha256,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    match intent.namespace {
        "source" => {
            transaction
                .execute(
                    "DELETE FROM content_sources
                     WHERE sha256 = ?1 AND relative_path = ?2 AND size_bytes = ?3",
                    params![
                        intent.sha256,
                        intent.relative_path,
                        u64_to_i64(intent.size_bytes)?,
                    ],
                )
                .map_err(storage_db_error)?;
        }
        "asset" => {
            transaction
                .execute(
                    "DELETE FROM assets
                     WHERE sha256 = ?1 AND relative_path = ?2
                       AND media_type = ?3 AND size_bytes = ?4",
                    params![
                        intent.sha256,
                        intent.relative_path,
                        intent.media_type,
                        u64_to_i64(intent.size_bytes)?,
                    ],
                )
                .map_err(storage_db_error)?;
        }
        _ => {
            return Err(CoreError::internal(
                "package CAS recovery namespace is unsupported",
            ));
        }
    }
    transaction.commit().map_err(storage_db_error)?;
    remove_package_cas_file(root, intent)?;
    delete_package_cas_promotion_journal_entry(connection, intent)
}
