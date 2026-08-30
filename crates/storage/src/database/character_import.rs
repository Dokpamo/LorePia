//! Character import commit and interrupted-import cleanup.

use super::package_cas::{
    recover_package_cas_promotions, reject_durable_committing_package_imports,
};
use super::provider_validation::validate_provider_local_network_approval_integrity;
use super::settings::{ensure_stable_local_user_settings, load_recovery_settings};
use super::{
    AppSettings, Character, CharacterContentV1, Connection, CoreError, CoreErrorCode, CoreResult,
    InterruptedGenerationClosure, Path, PathBuf, Storage, Utc,
    close_interrupted_generations_in_transaction, content_relative_path, ensure_real_directory, fs,
    load_interrupted_generation_closures, params, storage_corrupted, storage_db_error,
    storage_io_error, store_verified_source, u64_to_i64,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAssetImport {
    pub staged_path: PathBuf,
    pub sha256: String,
    pub media_type: String,
    pub size_bytes: u64,
}

struct InterruptedImport {
    id: String,
    source_hash: String,
    staging_path: String,
    state: String,
    asset_hashes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportCommitPhase {
    JournalCreated,
    CasFilesDurable,
    JournalMarkedFileStored,
    RecordsCommitted,
}

impl Storage {
    pub fn commit_character_import(
        &self,
        staged_path: &Path,
        character: &Character,
        source_size: u64,
        import_job_id: &str,
        staged_assets: &[StagedAssetImport],
    ) -> CoreResult<()> {
        self.commit_character_import_observed(
            staged_path,
            character,
            source_size,
            import_job_id,
            staged_assets,
            |_| {},
        )
    }

    /// Commits a character import and its normalized card content as one
    /// crash-atomic database mutation after the source and asset CAS files are
    /// durable.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_character_import_with_content(
        &self,
        staged_path: &Path,
        character: &Character,
        character_content: &CharacterContentV1,
        inspection_plan_sha256: &str,
        source_size: u64,
        import_job_id: &str,
        staged_assets: &[StagedAssetImport],
    ) -> CoreResult<()> {
        self.commit_character_import_with_content_observed(
            staged_path,
            character,
            character_content,
            inspection_plan_sha256,
            source_size,
            import_job_id,
            staged_assets,
            |_| {},
        )
    }

    pub(super) fn commit_character_import_observed(
        &self,
        staged_path: &Path,
        character: &Character,
        source_size: u64,
        import_job_id: &str,
        staged_assets: &[StagedAssetImport],
        mut observe: impl FnMut(ImportCommitPhase),
    ) -> CoreResult<()> {
        validate_staged_assets(character, staged_assets)?;
        let _cas_mutation = self.cas_mutation()?;
        self.create_import_journal(staged_path, character, import_job_id, staged_assets)?;
        observe(ImportCommitPhase::JournalCreated);
        self.store_import_files(staged_path, character, source_size, staged_assets)?;
        observe(ImportCommitPhase::CasFilesDurable);
        self.connection()?
            .execute(
                "UPDATE import_jobs SET state = 'file_stored', updated_at = ?2 WHERE id = ?1",
                params![import_job_id, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        observe(ImportCommitPhase::JournalMarkedFileStored);
        self.commit_import_records(character, source_size, import_job_id, staged_assets, None)?;
        observe(ImportCommitPhase::RecordsCommitted);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn commit_character_import_with_content_observed(
        &self,
        staged_path: &Path,
        character: &Character,
        character_content: &CharacterContentV1,
        inspection_plan_sha256: &str,
        source_size: u64,
        import_job_id: &str,
        staged_assets: &[StagedAssetImport],
        mut observe: impl FnMut(ImportCommitPhase),
    ) -> CoreResult<()> {
        validate_staged_assets(character, staged_assets)?;
        let _cas_mutation = self.cas_mutation()?;
        self.create_import_journal(staged_path, character, import_job_id, staged_assets)?;
        observe(ImportCommitPhase::JournalCreated);
        self.store_import_files(staged_path, character, source_size, staged_assets)?;
        observe(ImportCommitPhase::CasFilesDurable);
        self.connection()?
            .execute(
                "UPDATE import_jobs SET state = 'file_stored', updated_at = ?2 WHERE id = ?1",
                params![import_job_id, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        observe(ImportCommitPhase::JournalMarkedFileStored);
        self.commit_import_records(
            character,
            source_size,
            import_job_id,
            staged_assets,
            Some((character_content, inspection_plan_sha256)),
        )?;
        observe(ImportCommitPhase::RecordsCommitted);
        Ok(())
    }

    fn create_import_journal(
        &self,
        staged_path: &Path,
        character: &Character,
        import_job_id: &str,
        staged_assets: &[StagedAssetImport],
    ) -> CoreResult<()> {
        let asset_hashes = staged_assets
            .iter()
            .map(|asset| asset.sha256.as_str())
            .collect::<Vec<_>>();
        let asset_hashes_json = serde_json::to_string(&asset_hashes).map_err(|error| {
            CoreError::internal(format!("cannot encode import asset journal: {error}"))
        })?;
        self.connection()?
            .execute(
                "INSERT OR REPLACE INTO import_jobs
                 (id, source_hash, staging_path, state, updated_at, asset_hashes_json)
                 VALUES (?1, ?2, ?3, 'preparing', ?4, ?5)",
                params![
                    import_job_id,
                    character.source_hash,
                    staged_path.to_string_lossy(),
                    Utc::now().to_rfc3339(),
                    asset_hashes_json
                ],
            )
            .map_err(storage_db_error)?;
        Ok(())
    }

    fn store_import_files(
        &self,
        staged_path: &Path,
        character: &Character,
        source_size: u64,
        staged_assets: &[StagedAssetImport],
    ) -> CoreResult<()> {
        let relative_path = content_relative_path(&character.source_hash)?;
        let source_cas_root = self.root.join("sources/sha256");
        store_verified_source(
            staged_path,
            &self.root.join("sources").join(relative_path),
            &source_cas_root,
            &character.source_hash,
            source_size,
        )?;
        let asset_cas_root = self.root.join("assets/sha256");
        for asset in staged_assets {
            let relative_path = content_relative_path(&asset.sha256)?;
            store_verified_source(
                &asset.staged_path,
                &self.root.join("assets").join(relative_path),
                &asset_cas_root,
                &asset.sha256,
                asset.size_bytes,
            )?;
        }
        Ok(())
    }

    fn commit_import_records(
        &self,
        character: &Character,
        source_size: u64,
        import_job_id: &str,
        staged_assets: &[StagedAssetImport],
        character_content: Option<(&CharacterContentV1, &str)>,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        insert_content_source(&transaction, character, source_size)?;
        for asset in staged_assets {
            insert_asset(&transaction, asset)?;
        }
        insert_character(&transaction, character)?;
        for asset in staged_assets {
            link_character_asset(&transaction, character, asset)?;
        }
        if let Some((content, inspection_plan_sha256)) = character_content {
            crate::orchestration::write_imported_character_content(
                &transaction,
                &character.id,
                &character.source_hash,
                content,
                inspection_plan_sha256,
            )?;
        }
        transaction
            .execute("DELETE FROM import_jobs WHERE id = ?1", [import_job_id])
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)
    }
}

fn validate_staged_assets(
    character: &Character,
    staged_assets: &[StagedAssetImport],
) -> CoreResult<()> {
    if let Some(avatar_hash) = character.avatar_asset_hash.as_deref()
        && !staged_assets
            .iter()
            .any(|asset| asset.sha256 == avatar_hash)
    {
        return Err(CoreError::invalid(
            "character avatar does not reference a staged asset",
        ));
    }
    for asset in staged_assets {
        let _ = content_relative_path(&asset.sha256)?;
    }
    Ok(())
}
pub(super) fn recover_interrupted_work(root: &Path, connection: &mut Connection) -> CoreResult<()> {
    // A package commit is atomic in current implementations. Seeing this state
    // durably therefore indicates legacy or logically corrupted data; reject it
    // before any startup recovery mutates the database.
    reject_durable_committing_package_imports(connection)?;
    let jobs = load_interrupted_imports(connection)?;
    validate_and_cleanup_imports(root, connection, &jobs)?;
    let settings = load_recovery_settings(connection)?;
    apply_recovery_transaction(connection, &jobs, &settings)?;
    recover_package_cas_promotions(root, connection)?;
    remove_partial_files(&root.join("sources/sha256"))?;
    remove_partial_files(&root.join("assets/sha256"))?;
    Ok(())
}

pub(crate) fn prepare_cutover_candidate_for_open(connection: &mut Connection) -> CoreResult<()> {
    crate::discovery_repository::validate_native_no_effect_attestation_integrity(connection)?;
    ensure_stable_local_user_settings(connection)?;
    crate::orchestration::seed_builtin_prompt_presets(connection)?;
    validate_provider_local_network_approval_integrity(connection)?;

    let jobs = load_interrupted_imports(connection)?;
    validate_interrupted_imports(&jobs)?;
    let _settings = load_recovery_settings(connection)?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let _ = load_interrupted_generation_closures(&transaction)?;
    transaction.rollback().map_err(storage_db_error)?;
    Ok(())
}

fn apply_recovery_transaction(
    connection: &mut Connection,
    jobs: &[InterruptedImport],
    settings: &AppSettings,
) -> CoreResult<()> {
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let interrupted_generations: Vec<InterruptedGenerationClosure> =
        load_interrupted_generation_closures(&transaction)?;
    for job in jobs {
        transaction
            .execute("DELETE FROM import_jobs WHERE id = ?1", [&job.id])
            .map_err(storage_db_error)?;
    }
    let recovered_at = Utc::now();
    let recovered_at_text = recovered_at.to_rfc3339();
    close_interrupted_generations_in_transaction(
        &transaction,
        &interrupted_generations,
        settings.preserve_partial_generations,
        recovered_at,
        &recovered_at_text,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(())
}

fn remove_partial_files(root: &Path) -> CoreResult<()> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(storage_corrupted(
                "CAS recovery root is not a real directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage_io_error(error)),
    }
    for prefix in fs::read_dir(root).map_err(storage_io_error)? {
        let prefix = prefix.map_err(storage_io_error)?;
        if !prefix.file_type().map_err(storage_io_error)?.is_dir() {
            return Err(storage_corrupted(
                "CAS hash-prefix path is not a real directory",
            ));
        }
        for entry in fs::read_dir(prefix.path()).map_err(storage_io_error)? {
            let entry = entry.map_err(storage_io_error)?;
            let file_type = entry.file_type().map_err(storage_io_error)?;
            if !file_type.is_file() {
                return Err(storage_corrupted("CAS entry is not a regular file"));
            }
            let path = entry.path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.') && name.ends_with(".partial"))
            {
                fs::remove_file(path).map_err(storage_io_error)?;
            }
        }
    }
    Ok(())
}

pub(super) fn remove_abandoned_staging_files(staging: &Path) -> CoreResult<()> {
    if !staging.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(staging).map_err(storage_io_error)? {
        let entry = entry.map_err(storage_io_error)?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(storage_io_error)?;
        if metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            fs::remove_file(entry.path()).map_err(storage_io_error)?;
        }
    }
    Ok(())
}

fn insert_content_source(
    transaction: &rusqlite::Transaction<'_>,
    character: &Character,
    source_size: u64,
) -> CoreResult<()> {
    let relative_path = content_relative_path(&character.source_hash)?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO content_sources
             (sha256, relative_path, size_bytes, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                character.source_hash,
                format!("sources/{relative_path}"),
                u64_to_i64(source_size)?,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_asset(
    transaction: &rusqlite::Transaction<'_>,
    asset: &StagedAssetImport,
) -> CoreResult<()> {
    let relative_path = content_relative_path(&asset.sha256)?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO assets
             (sha256, relative_path, media_type, size_bytes, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                asset.sha256,
                format!("assets/{relative_path}"),
                asset.media_type,
                u64_to_i64(asset.size_bytes)?,
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_character(
    transaction: &rusqlite::Transaction<'_>,
    character: &Character,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO characters
             (id, name, description, source_hash, avatar_asset_hash, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                character.id,
                character.name,
                character.description,
                character.source_hash,
                character.avatar_asset_hash,
                character.created_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn link_character_asset(
    transaction: &rusqlite::Transaction<'_>,
    character: &Character,
    asset: &StagedAssetImport,
) -> CoreResult<()> {
    let role = if character.avatar_asset_hash.as_deref() == Some(asset.sha256.as_str()) {
        "avatar"
    } else {
        "attachment"
    };
    transaction
        .execute(
            "INSERT OR IGNORE INTO character_assets
             (character_id, asset_hash, role)
             VALUES (?1, ?2, ?3)",
            params![character.id, asset.sha256, role],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn load_interrupted_imports(connection: &Connection) -> CoreResult<Vec<InterruptedImport>> {
    let raw_jobs = {
        let mut statement = connection
            .prepare(
                "SELECT id, source_hash, staging_path, state, asset_hashes_json FROM import_jobs",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let jobs = raw_jobs
        .into_iter()
        .map(
            |(id, source_hash, staging_path, state, asset_hashes_json)| {
                let asset_hashes = serde_json::from_str::<Vec<String>>(&asset_hashes_json)
                    .map_err(|error| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            format!("import journal asset hashes are invalid: {error}"),
                            false,
                        )
                    })?;
                Ok(InterruptedImport {
                    id,
                    source_hash,
                    staging_path,
                    state,
                    asset_hashes,
                })
            },
        )
        .collect::<CoreResult<Vec<_>>>()?;
    Ok(jobs)
}

fn validate_and_cleanup_imports(
    root: &Path,
    connection: &Connection,
    jobs: &[InterruptedImport],
) -> CoreResult<()> {
    validate_interrupted_imports(jobs)?;

    for job in jobs {
        remove_owned_staging_file(root, &job.staging_path)?;
        remove_unreferenced_cas(
            root,
            connection,
            "content_sources",
            "sources",
            &job.source_hash,
        )?;
        for asset_hash in &job.asset_hashes {
            remove_unreferenced_cas(root, connection, "assets", "assets", asset_hash)?;
        }
    }
    Ok(())
}

fn validate_interrupted_imports(jobs: &[InterruptedImport]) -> CoreResult<()> {
    for job in jobs {
        if !matches!(job.state.as_str(), "preparing" | "file_stored") {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                format!("import journal contains unknown state: {}", job.state),
                false,
            ));
        }
        let _ = content_relative_path(&job.source_hash)?;
        for asset_hash in &job.asset_hashes {
            let _ = content_relative_path(asset_hash)?;
        }
    }
    Ok(())
}

fn remove_unreferenced_cas(
    root: &Path,
    connection: &Connection,
    table: &str,
    directory: &str,
    hash: &str,
) -> CoreResult<()> {
    let query = match table {
        "content_sources" => "SELECT EXISTS(SELECT 1 FROM content_sources WHERE sha256 = ?1)",
        "assets" => "SELECT EXISTS(SELECT 1 FROM assets WHERE sha256 = ?1)",
        _ => return Err(CoreError::internal("unsupported recovery table")),
    };
    let referenced = connection
        .query_row(query, [hash], |row| row.get::<_, bool>(0))
        .map_err(storage_db_error)?;
    if referenced {
        return Ok(());
    }
    if crate::cutover::is_rollback_cas_pinned(root, directory, hash)? {
        return Ok(());
    }
    let relative = content_relative_path(hash)?;
    let path = root.join(directory).join(&relative);
    let cas_root = root.join(directory).join("sha256");
    ensure_real_directory(&cas_root)?;
    let prefix = path
        .parent()
        .ok_or_else(|| CoreError::internal("CAS recovery path has no parent"))?;
    match fs::symlink_metadata(prefix) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(storage_corrupted(
                "CAS recovery hash-prefix path is not a real directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(storage_io_error(error)),
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage_io_error(error)),
    }
}

fn remove_owned_staging_file(root: &Path, candidate: &str) -> CoreResult<()> {
    let candidate = PathBuf::from(candidate);
    if !candidate.is_file() {
        return Ok(());
    }
    let staging = root.join("staging");
    let staging = fs::canonicalize(staging).map_err(storage_io_error)?;
    let candidate = fs::canonicalize(candidate).map_err(storage_io_error)?;
    if candidate.parent() == Some(staging.as_path()) {
        fs::remove_file(candidate).map_err(storage_io_error)?;
    }
    Ok(())
}
