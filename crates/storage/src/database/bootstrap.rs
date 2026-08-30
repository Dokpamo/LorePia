use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

use chrono::Utc;
use lorepia_domain::CoreResult;

use super::private_path::harden_owned_tree_permissions;
use super::{
    DatabaseConnectionMetricState, Storage, VerifiedAssetCache,
    data_root::{
        acquire_data_root_owner_lock, create_owned_directory_tree, prepare_owned_data_root,
    },
    ensure_stable_local_user_settings, recover_interrupted_work, remove_abandoned_staging_files,
    validate_provider_local_network_approval_integrity,
};

impl Storage {
    pub fn open(root: impl AsRef<Path>) -> CoreResult<Self> {
        Self::open_internal(root.as_ref(), true)
    }

    /// Opens storage while deferring provider-discovery recovery to the Core.
    ///
    /// The owning Core or native host must classify validated, secret-free
    /// assistant checkpoints, reconcile any native credential effects, and
    /// call `recover_unfinished_discovery_operations_except` before exposing
    /// the instance. Standalone storage callers should use [`Self::open`],
    /// which remains conservatively self-recovering.
    pub fn open_with_deferred_discovery_recovery(root: impl AsRef<Path>) -> CoreResult<Self> {
        Self::open_internal(root.as_ref(), false)
    }

    fn open_internal(root: &Path, recover_provider_discovery: bool) -> CoreResult<Self> {
        let root = prepare_owned_data_root(root)?;
        let owner_lock = acquire_data_root_owner_lock(&root)?;
        for relative in [
            "db",
            "sources/sha256",
            "assets/sha256",
            "cache/thumbnails",
            "cache/extracted",
            "staging",
            "recovery",
        ] {
            create_owned_directory_tree(&root, Path::new(relative))?;
        }

        let database_path = root.join("db/lorepia.sqlite3");
        let mut connection = crate::cutover::open_database(&root, &database_path)?;
        crate::discovery_repository::validate_native_no_effect_attestation_integrity(&connection)?;
        ensure_stable_local_user_settings(&mut connection)?;
        crate::orchestration::seed_builtin_prompt_presets(&mut connection)?;
        validate_provider_local_network_approval_integrity(&connection)?;
        recover_interrupted_work(&root, &mut connection)?;
        crate::model_sync::recover_interrupted_model_sync_jobs(&mut connection)?;
        remove_abandoned_staging_files(&root.join("staging"))?;

        let storage = Self {
            root,
            cas_mutation: Mutex::new(()),
            connection_metrics: DatabaseConnectionMetricState::default(),
            connection: Mutex::new(connection),
            verified_asset_cache: Mutex::new(VerifiedAssetCache::default()),
            #[cfg(test)]
            approved_asset_hash_verifications: AtomicUsize::new(0),
            _owner_lock: owner_lock,
        };
        // Running memory jobs have no trustworthy in-process worker after a
        // restart. Recover them before the Storage becomes observable so a
        // durable worker can claim the retryable work exactly once.
        storage.recover_running_memory_jobs(Utc::now())?;
        storage.recover_running_memory_query_embeddings(Utc::now())?;
        storage.recover_all_core_lifecycle_occurrence_leases(Utc::now())?;
        storage.recover_all_interaction_derived_event_leases(Utc::now())?;
        storage.recover_started_runtime_model_audits(Utc::now())?;
        if recover_provider_discovery {
            storage.recover_unfinished_discovery_operations(Utc::now())?;
        }
        harden_owned_tree_permissions(&storage.root)?;
        Ok(storage)
    }

    pub fn data_root(&self) -> &Path {
        &self.root
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }
}
