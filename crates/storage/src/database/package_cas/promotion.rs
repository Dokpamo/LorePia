//! Package source and asset CAS publication facade.

use super::super::{
    BTreeSet, CoreError, CoreErrorCode, CoreResult, OptionalExtension, Path, PathBuf,
    StagedAssetImport, Storage, TransactionBehavior, Utc, content_relative_path,
    ensure_regular_file, i64_to_u64, params, storage_corrupted, storage_db_error,
    store_verified_source, store_verified_source_observed, u64_to_i64, verify_file,
};
use super::{
    PackageCasPromotionIntent,
    journal::{
        cleanup_package_cas_promotion, ensure_package_cas_promotion_intents,
        mark_package_cas_file_durable, mark_package_cas_row_registered,
        prepare_package_asset_promotion_intents, validate_package_promotion_import_id,
    },
    validate_owned_staged_file, verify_media_type_signature,
};

impl Storage {
    /// Promotes an owned staged package snapshot into durable source CAS.
    ///
    /// The bytes are streamed, hashed, size-checked, fsynced, and registered
    /// before this returns. Package review state must reference this durable
    /// source rather than a staging path.
    pub fn promote_package_source(
        &self,
        import_id: &str,
        staged_path: &Path,
        source_sha256: &str,
        source_size: u64,
    ) -> CoreResult<PathBuf> {
        self.promote_package_source_observed(
            import_id,
            staged_path,
            source_sha256,
            source_size,
            || {},
        )
    }

    pub(in crate::database) fn promote_package_source_observed(
        &self,
        import_id: &str,
        staged_path: &Path,
        source_sha256: &str,
        source_size: u64,
        before_file_publication: impl FnOnce(),
    ) -> CoreResult<PathBuf> {
        validate_package_promotion_import_id(import_id)?;
        let staged_path = validate_owned_staged_file(&self.root, staged_path)?;
        let relative_path = content_relative_path(source_sha256)?;
        let final_path = self.root.join("sources").join(&relative_path);
        let intent = PackageCasPromotionIntent {
            import_id: import_id.to_owned(),
            namespace: "source",
            sha256: source_sha256.to_owned(),
            size_bytes: source_size,
            media_type: None,
            relative_path: format!("sources/{relative_path}"),
        };
        // CAS mutation is the outer lock. The durable intent is committed in a
        // short DB phase, then the connection mutex is deliberately released
        // while copy/hash/fsync publishes the immutable file.
        let cas_mutation = self.cas_mutation()?;
        {
            let mut connection = self.connection()?;
            ensure_package_cas_promotion_intents(&mut connection, std::slice::from_ref(&intent))?;
        }
        let store_result = store_verified_source_observed(
            &staged_path,
            &final_path,
            &self.root.join("sources/sha256"),
            source_sha256,
            source_size,
            before_file_publication,
        );
        if let Err(error) = store_result {
            cleanup_package_cas_promotion(self, &cas_mutation, &intent)?;
            return Err(error);
        }
        let registration = (|| -> CoreResult<()> {
            let mut connection = self.connection()?;
            mark_package_cas_file_durable(&connection, &intent)?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_db_error)?;
            transaction
                .execute(
                    "INSERT OR IGNORE INTO content_sources
                     (sha256, relative_path, size_bytes, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        source_sha256,
                        format!("sources/{relative_path}"),
                        u64_to_i64(source_size)?,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .map_err(storage_db_error)?;
            let stored = transaction
                .query_row(
                    "SELECT relative_path, size_bytes
                     FROM content_sources WHERE sha256 = ?1",
                    [source_sha256],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .map_err(storage_db_error)?;
            if stored.0 != format!("sources/{relative_path}")
                || i64_to_u64("package source size", stored.1)? != source_size
            {
                return Err(storage_corrupted(
                    "package source CAS metadata conflicts with an existing record",
                ));
            }
            mark_package_cas_row_registered(&transaction, &intent)?;
            transaction.commit().map_err(storage_db_error)
        })();
        if let Err(error) = registration {
            cleanup_package_cas_promotion(self, &cas_mutation, &intent)?;
            return Err(error);
        }
        verify_file(&final_path, source_sha256, source_size)?;
        Ok(final_path)
    }

    /// Reopens a durable package source by exact digest and size.
    pub fn package_source_path(
        &self,
        source_sha256: &str,
        source_size: u64,
    ) -> CoreResult<PathBuf> {
        let relative = content_relative_path(source_sha256)?;
        let expected_relative = format!("sources/{relative}");
        // Keep cleanup and publication excluded while the DB identity snapshot
        // is revalidated against its immutable file, but release SQLite before
        // opening and hashing that file.
        let _cas_mutation = self.cas_mutation()?;
        let stored = {
            let connection = self.connection()?;
            connection
                .query_row(
                    "SELECT relative_path, size_bytes
                     FROM content_sources WHERE sha256 = ?1",
                    [source_sha256],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::NotFound,
                        "durable package source was not found",
                        false,
                    )
                })?
        };
        if stored.0 != expected_relative
            || i64_to_u64("package source size", stored.1)? != source_size
        {
            return Err(storage_corrupted(
                "durable package source metadata does not match its identity",
            ));
        }
        let path = self.root.join(stored.0);
        ensure_regular_file(&path)?;
        verify_file(&path, source_sha256, source_size)?;
        Ok(path)
    }

    /// Promotes all selected package assets before the package metadata
    /// transaction begins.
    pub fn promote_package_assets(
        &self,
        import_id: &str,
        staged_assets: &[StagedAssetImport],
    ) -> CoreResult<Vec<PathBuf>> {
        let intents =
            prepare_package_asset_promotion_intents(&self.root, import_id, staged_assets)?;
        let mut promoted = Vec::with_capacity(staged_assets.len());
        // Package and legacy character imports share these CAS roots. Keep
        // their file mutations serialized while releasing the DB mutex for
        // file copy, hashing, signature inspection, and fsync.
        let cas_mutation = self.cas_mutation()?;
        {
            let mut connection = self.connection()?;
            ensure_package_cas_promotion_intents(&mut connection, &intents)?;
        }
        for (asset, intent) in staged_assets.iter().zip(&intents) {
            let staged_path = validate_owned_staged_file(&self.root, &asset.staged_path)?;
            let relative = content_relative_path(&asset.sha256)?;
            let final_path = self.root.join("assets").join(&relative);
            if let Err(error) = store_verified_source(
                &staged_path,
                &final_path,
                &self.root.join("assets/sha256"),
                &asset.sha256,
                asset.size_bytes,
            ) {
                for cleanup in &intents {
                    cleanup_package_cas_promotion(self, &cas_mutation, cleanup)?;
                }
                return Err(error);
            }
            if let Err(error) = verify_media_type_signature(&final_path, &asset.media_type) {
                for cleanup in &intents {
                    cleanup_package_cas_promotion(self, &cas_mutation, cleanup)?;
                }
                return Err(error);
            }
            promoted.push((asset, intent, relative, final_path));
        }
        let registration = (|| -> CoreResult<()> {
            let mut connection = self.connection()?;
            for intent in &intents {
                mark_package_cas_file_durable(&connection, intent)?;
            }
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage_db_error)?;
            for (asset, intent, relative, _) in &promoted {
                transaction
                    .execute(
                        "INSERT OR IGNORE INTO assets
                         (sha256, relative_path, media_type, size_bytes, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![
                            asset.sha256,
                            format!("assets/{relative}"),
                            asset.media_type,
                            u64_to_i64(asset.size_bytes)?,
                            Utc::now().to_rfc3339(),
                        ],
                    )
                    .map_err(storage_db_error)?;
                let stored = transaction
                    .query_row(
                        "SELECT relative_path, media_type, size_bytes
                         FROM assets WHERE sha256 = ?1",
                        [asset.sha256.as_str()],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                            ))
                        },
                    )
                    .map_err(storage_db_error)?;
                if stored.0 != format!("assets/{relative}")
                    || stored.1 != asset.media_type
                    || i64_to_u64("package asset size", stored.2)? != asset.size_bytes
                {
                    return Err(storage_corrupted(
                        "package asset CAS metadata conflicts with an existing record",
                    ));
                }
                mark_package_cas_row_registered(&transaction, intent)?;
            }
            transaction.commit().map_err(storage_db_error)
        })();
        if let Err(error) = registration {
            for cleanup in &intents {
                cleanup_package_cas_promotion(self, &cas_mutation, cleanup)?;
            }
            return Err(error);
        }
        Ok(promoted.into_iter().map(|(_, _, _, path)| path).collect())
    }

    pub(crate) fn cleanup_package_source_promotion(
        &self,
        import_id: &str,
        source_sha256: &str,
        source_size: u64,
    ) -> CoreResult<bool> {
        let relative = content_relative_path(source_sha256)?;
        let intent = PackageCasPromotionIntent {
            import_id: import_id.to_owned(),
            namespace: "source",
            sha256: source_sha256.to_owned(),
            size_bytes: source_size,
            media_type: None,
            relative_path: format!("sources/{relative}"),
        };
        let cas_mutation = self.cas_mutation()?;
        cleanup_package_cas_promotion(self, &cas_mutation, &intent)
    }

    pub(crate) fn cleanup_package_asset_promotions(
        &self,
        import_id: &str,
        staged_assets: &[StagedAssetImport],
    ) -> CoreResult<u64> {
        validate_package_promotion_import_id(import_id)?;
        let mut identities = BTreeSet::new();
        let intents = staged_assets
            .iter()
            .map(|asset| {
                if !identities.insert(asset.sha256.as_str()) {
                    return Err(CoreError::invalid(
                        "package asset cleanup contains a duplicate digest",
                    ));
                }
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
            .collect::<CoreResult<Vec<_>>>()?;
        let cas_mutation = self.cas_mutation()?;
        intents.into_iter().try_fold(0_u64, |removed, intent| {
            let was_removed = cleanup_package_cas_promotion(self, &cas_mutation, &intent)?;
            removed
                .checked_add(u64::from(was_removed))
                .ok_or_else(|| CoreError::internal("package asset cleanup count overflow"))
        })
    }

    pub(crate) fn verify_package_asset_cas(
        &self,
        descriptor: &lorepia_domain::AssetDescriptor,
    ) -> CoreResult<()> {
        let relative = content_relative_path(descriptor.sha256.as_str())?;
        let expected_relative = format!("assets/{relative}");
        let _cas_mutation = self.cas_mutation()?;
        let stored = {
            let connection = self.connection()?;
            connection
                .query_row(
                    "SELECT relative_path, media_type, size_bytes
                     FROM assets WHERE sha256 = ?1",
                    [descriptor.sha256.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    CoreError::invalid(
                        "package asset bytes must be durable in content-addressed storage first",
                    )
                })?
        };
        if stored.0 != expected_relative
            || stored.1 != descriptor.media_type
            || i64_to_u64("package asset size", stored.2)? != descriptor.size_bytes
        {
            return Err(storage_corrupted(
                "package asset descriptor does not match durable CAS metadata",
            ));
        }
        let path = self.root.join(stored.0);
        ensure_regular_file(&path)?;
        verify_file(&path, descriptor.sha256.as_str(), descriptor.size_bytes)?;
        verify_media_type_signature(&path, &descriptor.media_type)
    }
}
