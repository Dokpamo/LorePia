//! Durable package CAS intent, phase, claim, and cleanup transitions.

use super::super::{
    AssetDescriptor, BTreeSet, Connection, CoreError, CoreResult, MutexGuard, OptionalExtension,
    Path, StagedAssetImport, Storage, TransactionBehavior, Utc, content_relative_path, fs,
    i64_to_u64, params, storage_corrupted, storage_db_error, storage_io_error, sync_directory,
    u64_to_i64, verify_file,
};
use super::{PackageCasPromotionIntent, validate_owned_staged_file};

pub(super) fn validate_package_promotion_import_id(import_id: &str) -> CoreResult<()> {
    if import_id.is_empty()
        || import_id.len() > 256
        || import_id.trim() != import_id
        || import_id.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(
            "package CAS promotion import id is invalid",
        ));
    }
    Ok(())
}

pub(super) fn prepare_package_asset_promotion_intents(
    root: &Path,
    import_id: &str,
    staged_assets: &[StagedAssetImport],
) -> CoreResult<Vec<PackageCasPromotionIntent>> {
    validate_package_promotion_import_id(import_id)?;
    let mut identities = BTreeSet::new();
    staged_assets
        .iter()
        .map(|asset| {
            if !identities.insert(asset.sha256.as_str()) {
                return Err(CoreError::invalid(
                    "package asset digest is duplicated in the promotion set",
                ));
            }
            let _ = validate_owned_staged_file(root, &asset.staged_path)?;
            let relative = content_relative_path(&asset.sha256)?;
            Ok(PackageCasPromotionIntent {
                import_id: import_id.to_owned(),
                namespace: "asset",
                sha256: asset.sha256.clone(),
                size_bytes: asset.size_bytes,
                media_type: Some(asset.media_type.clone()),
                relative_path: format!("assets/{relative}"),
            })
        })
        .collect()
}

pub(super) fn validate_package_cas_promotion_intent(
    intent: &PackageCasPromotionIntent,
) -> CoreResult<()> {
    validate_package_promotion_import_id(&intent.import_id)?;
    let relative = content_relative_path(&intent.sha256)?;
    let expected_relative = match intent.namespace {
        "source" if intent.media_type.is_none() => format!("sources/{relative}"),
        "asset"
            if intent
                .media_type
                .as_deref()
                .is_some_and(|media_type| !media_type.trim().is_empty()) =>
        {
            format!("assets/{relative}")
        }
        "source" | "asset" => {
            return Err(CoreError::invalid(
                "package CAS promotion media metadata is invalid",
            ));
        }
        _ => {
            return Err(CoreError::internal(
                "package CAS promotion namespace is unsupported",
            ));
        }
    };
    if intent.relative_path != expected_relative {
        return Err(CoreError::invalid(
            "package CAS promotion path is not canonical",
        ));
    }
    u64_to_i64(intent.size_bytes).map(|_| ())
}

pub(in crate::database) fn ensure_package_cas_promotion_intents(
    connection: &mut Connection,
    intents: &[PackageCasPromotionIntent],
) -> CoreResult<()> {
    if intents.is_empty() {
        return Ok(());
    }
    let import_id = intents[0].import_id.as_str();
    let namespace = intents[0].namespace;
    let mut hashes = BTreeSet::new();
    for intent in intents {
        validate_package_cas_promotion_intent(intent)?;
        if intent.import_id != import_id
            || intent.namespace != namespace
            || !hashes.insert(intent.sha256.as_str())
        {
            return Err(CoreError::invalid(
                "package CAS promotion intent set is inconsistent",
            ));
        }
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let now = Utc::now().to_rfc3339();
    for intent in intents {
        transaction
            .execute(
                "INSERT OR IGNORE INTO package_cas_promotion_journal (
                    import_id, namespace, sha256, size_bytes, media_type,
                    relative_path, phase, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'intent', ?7, ?7)",
                params![
                    intent.import_id,
                    intent.namespace,
                    intent.sha256,
                    u64_to_i64(intent.size_bytes)?,
                    intent.media_type,
                    intent.relative_path,
                    now,
                ],
            )
            .map_err(storage_db_error)?;
        let stored = transaction
            .query_row(
                "SELECT size_bytes, media_type, relative_path, phase
                 FROM package_cas_promotion_journal
                 WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
                params![intent.import_id, intent.namespace, intent.sha256],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .map_err(storage_db_error)?;
        if i64_to_u64("package CAS promotion size", stored.0)? != intent.size_bytes
            || stored.1 != intent.media_type
            || stored.2 != intent.relative_path
            || !matches!(
                stored.3.as_str(),
                "intent" | "file_durable" | "row_registered"
            )
        {
            return Err(storage_corrupted(
                "package CAS promotion retry differs from its durable intent",
            ));
        }
    }
    let stored_hashes = {
        let mut statement = transaction
            .prepare(
                "SELECT sha256
                 FROM package_cas_promotion_journal
                 WHERE import_id = ?1 AND namespace = ?2
                 ORDER BY sha256",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map(params![import_id, namespace], |row| row.get::<_, String>(0))
            .map_err(storage_db_error)?
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(storage_db_error)?
    };
    if stored_hashes != hashes.into_iter().map(str::to_owned).collect() {
        return Err(CoreError::invalid(
            "package CAS promotion retry changes the exact artifact set",
        ));
    }
    transaction.commit().map_err(storage_db_error)
}

pub(in crate::database) fn mark_package_cas_file_durable(
    connection: &Connection,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<()> {
    validate_package_cas_promotion_intent(intent)?;
    let changed = connection
        .execute(
            "UPDATE package_cas_promotion_journal
             SET phase = CASE
                    WHEN phase = 'intent' THEN 'file_durable'
                    ELSE phase
                 END,
                 updated_at = ?7
             WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3
               AND size_bytes = ?4 AND media_type IS ?5
               AND relative_path = ?6
               AND phase IN ('intent', 'file_durable', 'row_registered')",
            params![
                intent.import_id,
                intent.namespace,
                intent.sha256,
                u64_to_i64(intent.size_bytes)?,
                intent.media_type,
                intent.relative_path,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "package CAS promotion durable-file phase lost its intent",
        ));
    }
    Ok(())
}

pub(super) fn mark_package_cas_row_registered(
    transaction: &rusqlite::Transaction<'_>,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<()> {
    validate_package_cas_promotion_intent(intent)?;
    let changed = transaction
        .execute(
            "UPDATE package_cas_promotion_journal
             SET phase = 'row_registered', updated_at = ?7
             WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3
               AND size_bytes = ?4 AND media_type IS ?5
               AND relative_path = ?6
               AND phase IN ('intent', 'file_durable', 'row_registered')",
            params![
                intent.import_id,
                intent.namespace,
                intent.sha256,
                u64_to_i64(intent.size_bytes)?,
                intent.media_type,
                intent.relative_path,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "package CAS promotion row-registration phase lost its intent",
        ));
    }
    Ok(())
}

pub(super) fn package_cas_product_reference_exists(
    connection: &Connection,
    namespace: &str,
    sha256: &str,
) -> CoreResult<bool> {
    let query = match namespace {
        "source" => {
            "SELECT EXISTS(
                SELECT 1 FROM package_sources WHERE source_hash = ?1
                UNION ALL
                SELECT 1 FROM characters WHERE source_hash = ?1
                UNION ALL
                SELECT 1 FROM content_revisions WHERE source_hash = ?1
             )"
        }
        "asset" => {
            "SELECT EXISTS(
                SELECT 1 FROM character_assets WHERE asset_hash = ?1
                UNION ALL
                SELECT 1 FROM characters WHERE avatar_asset_hash = ?1
                UNION ALL
                SELECT 1 FROM asset_descriptors WHERE asset_hash = ?1
                UNION ALL
                SELECT 1 FROM package_raw_extensions WHERE asset_hash = ?1
             )"
        }
        _ => {
            return Err(CoreError::internal(
                "package CAS reference namespace is unsupported",
            ));
        }
    };
    connection
        .query_row(query, [sha256], |row| row.get::<_, bool>(0))
        .map_err(storage_db_error)
}

pub(in crate::database) fn cleanup_package_cas_promotion(
    storage: &Storage,
    _cas_mutation: &MutexGuard<'_, ()>,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<bool> {
    validate_package_cas_promotion_intent(intent)?;
    let should_remove = {
        let mut connection = storage.connection()?;
        prepare_package_cas_cleanup(&mut connection, intent)?
    };
    if !should_remove {
        return Ok(false);
    }
    // The committed cleanup_pending phase is the recovery authority. The CAS
    // guard prevents a writer/read-validation race while file verification,
    // removal, and directory fsync run without holding the SQLite mutex.
    let removed = remove_package_cas_file(&storage.root, intent)?;
    {
        let connection = storage.connection()?;
        connection
            .execute(
                "DELETE FROM package_cas_promotion_journal
                 WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3
                   AND phase = 'cleanup_pending'",
                params![intent.import_id, intent.namespace, intent.sha256],
            )
            .map_err(storage_db_error)?;
    }
    Ok(removed)
}

fn prepare_package_cas_cleanup(
    connection: &mut Connection,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<bool> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    if !package_cas_cleanup_intent_matches(&transaction, intent)? {
        transaction.commit().map_err(storage_db_error)?;
        return Ok(false);
    }
    let referenced =
        package_cas_product_reference_exists(&transaction, intent.namespace, &intent.sha256)?;
    let shared = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM package_cas_promotion_journal
                WHERE namespace = ?1 AND sha256 = ?2
                  AND import_id <> ?3
             )",
            params![intent.namespace, intent.sha256, intent.import_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if referenced || shared {
        transaction
            .execute(
                "DELETE FROM package_cas_promotion_journal
                 WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
                params![intent.import_id, intent.namespace, intent.sha256],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        return Ok(false);
    }
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
    delete_package_cas_metadata(&transaction, intent)?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(true)
}

fn package_cas_cleanup_intent_matches(
    transaction: &rusqlite::Transaction<'_>,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<bool> {
    let stored = transaction
        .query_row(
            "SELECT size_bytes, media_type, relative_path
             FROM package_cas_promotion_journal
             WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
            params![intent.import_id, intent.namespace, intent.sha256],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some(stored) = stored else {
        return Ok(false);
    };
    if i64_to_u64("package CAS promotion size", stored.0)? != intent.size_bytes
        || stored.1 != intent.media_type
        || stored.2 != intent.relative_path
    {
        return Err(storage_corrupted(
            "package CAS cleanup differs from its durable intent",
        ));
    }
    Ok(true)
}

fn delete_package_cas_metadata(
    transaction: &rusqlite::Transaction<'_>,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<()> {
    let changed = match intent.namespace {
        "source" => transaction
            .execute(
                "DELETE FROM content_sources
                 WHERE sha256 = ?1 AND relative_path = ?2 AND size_bytes = ?3",
                params![
                    intent.sha256,
                    intent.relative_path,
                    u64_to_i64(intent.size_bytes)?,
                ],
            )
            .map_err(storage_db_error)?,
        "asset" => {
            let media_type = intent.media_type.as_deref().ok_or_else(|| {
                storage_corrupted("package CAS cleanup asset media type is missing")
            })?;
            transaction
                .execute(
                    "DELETE FROM assets
                     WHERE sha256 = ?1 AND relative_path = ?2
                       AND media_type = ?3 AND size_bytes = ?4",
                    params![
                        intent.sha256,
                        intent.relative_path,
                        media_type,
                        u64_to_i64(intent.size_bytes)?,
                    ],
                )
                .map_err(storage_db_error)?
        }
        _ => unreachable!("validated package CAS metadata"),
    };
    if changed > 1 {
        return Err(storage_corrupted(
            "package CAS cleanup removed duplicate metadata rows",
        ));
    }
    Ok(())
}

pub(super) fn remove_package_cas_file(
    root: &Path,
    intent: &PackageCasPromotionIntent,
) -> CoreResult<bool> {
    validate_package_cas_promotion_intent(intent)?;
    let rollback_namespace = match intent.namespace {
        "source" => "sources",
        "asset" => "assets",
        _ => unreachable!("validated package CAS namespace"),
    };
    if crate::cutover::is_rollback_cas_pinned(root, rollback_namespace, &intent.sha256)? {
        return Ok(false);
    }
    let path = root.join(&intent.relative_path);
    let removed = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            verify_file(&path, &intent.sha256, intent.size_bytes)?;
            fs::remove_file(&path).map_err(storage_io_error)?;
            let parent = path
                .parent()
                .ok_or_else(|| CoreError::internal("package CAS path has no parent"))?;
            sync_directory(parent)?;
            true
        }
        Ok(_) => {
            return Err(storage_corrupted(
                "package CAS cleanup target is not a regular file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(storage_io_error(error)),
    };
    Ok(removed)
}

fn claim_package_cas_promotion(
    transaction: &rusqlite::Transaction<'_>,
    intent: &PackageCasPromotionIntent,
    required: bool,
) -> CoreResult<()> {
    validate_package_cas_promotion_intent(intent)?;
    let stored = transaction
        .query_row(
            "SELECT size_bytes, media_type, relative_path, phase
             FROM package_cas_promotion_journal
             WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
            params![intent.import_id, intent.namespace, intent.sha256],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some(stored) = stored else {
        if required {
            return Err(storage_corrupted(
                "package commit has no durable CAS promotion intent",
            ));
        }
        return Ok(());
    };
    if i64_to_u64("package CAS promotion size", stored.0)? != intent.size_bytes
        || stored.1 != intent.media_type
        || stored.2 != intent.relative_path
        || stored.3 != "row_registered"
    {
        return Err(storage_corrupted(
            "package commit CAS promotion differs from its registered intent",
        ));
    }
    let changed = transaction
        .execute(
            "DELETE FROM package_cas_promotion_journal
             WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3
               AND phase = 'row_registered'",
            params![intent.import_id, intent.namespace, intent.sha256],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "package commit could not claim its CAS promotion intent",
        ));
    }
    Ok(())
}

pub(crate) fn claim_package_source_promotion(
    transaction: &rusqlite::Transaction<'_>,
    import_id: &str,
    source_sha256: &str,
    source_size: u64,
    required: bool,
) -> CoreResult<()> {
    let relative = content_relative_path(source_sha256)?;
    claim_package_cas_promotion(
        transaction,
        &PackageCasPromotionIntent {
            import_id: import_id.to_owned(),
            namespace: "source",
            sha256: source_sha256.to_owned(),
            size_bytes: source_size,
            media_type: None,
            relative_path: format!("sources/{relative}"),
        },
        required,
    )
}

pub(crate) fn claim_package_asset_promotions(
    transaction: &rusqlite::Transaction<'_>,
    import_id: &str,
    assets: &[AssetDescriptor],
    required: bool,
) -> CoreResult<()> {
    let mut seen = BTreeSet::new();
    for asset in assets {
        if !seen.insert(asset.sha256.as_str()) {
            return Err(CoreError::invalid(
                "package asset claim contains a duplicate digest",
            ));
        }
        let relative = content_relative_path(asset.sha256.as_str())?;
        claim_package_cas_promotion(
            transaction,
            &PackageCasPromotionIntent {
                import_id: import_id.to_owned(),
                namespace: "asset",
                sha256: asset.sha256.as_str().to_owned(),
                size_bytes: asset.size_bytes,
                media_type: Some(asset.media_type.clone()),
                relative_path: format!("assets/{relative}"),
            },
            required,
        )?;
    }
    Ok(())
}
