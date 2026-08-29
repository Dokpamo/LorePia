mod asset_delivery;
mod connection_metrics;
mod private_path;

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Seek, Write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use chrono::{DateTime, Utc};
#[cfg(test)]
use lorepia_domain::AssetId;
use lorepia_domain::discovery::DiscoveryPreviousSelection;
use lorepia_domain::{
    ApiFamily, AppSettings, AssetDescriptor, AuthBinding, BoundedJson, CanonicalOrigin,
    CapabilityKey, CapabilityObservation, CapabilityValue, Character, CharacterContentV1,
    CharacterGreetingCatalog, CharacterGreetingKind, CharacterGreetingOption, Confidence,
    ConnectionConfig, ConnectionConfigEntry, ConnectionConfigValue, ConnectionFieldSpec,
    ConnectionFieldType, ConnectionStatus, Conversation, ConversationBranch, ConversationBranchId,
    ConversationGreetingBinding, ConversationId, ConversationMode, ConversationStart,
    ConversationState, CoreError, CoreErrorCode, CoreResult, CredentialRedirectPolicy,
    CredentialRef, CredentialScope, DecoderId, EndpointPath, EndpointSpec, EvidenceId,
    GenerationId, GenerationPreset, GenerationPresetId, GenerationPromptCacheSettings,
    GenerationReasoningSettings, GenerationRecord, GenerationStatus, GenerationUsage, HttpMethod,
    LocalUserId, MAX_OPAQUE_REASONING_SERIALIZED_BYTES, ManifestDecoders, ManifestEndpoints,
    Message, MessageId, MessageRole, MessageStatus, ModelAvailability, ModelMetadataSource,
    ModelRoute, ModelRouteConfig, ModelRouteId, ModelSyncJobId, ObservationId, ObservationSource,
    OpaqueReasoningState, ParameterDefaultMode, ParameterId, ParameterLiteral, ParameterSpec,
    ParameterType, ParameterValue, ParameterValueState, ProviderConnection, ProviderConnectionId,
    ProviderLocalNetworkApproval, ProviderManifest, ProviderNetworkMode, ProviderParameterMapping,
    ProviderParameterTarget, ProviderProfile, ProviderTemplate, ProviderTemplateId, Sha256Digest,
    SupportStatus, TemplateSource, UiParameterLevel, validate_opaque_reasoning_states,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use private_path::{harden_owned_tree_permissions, harden_private_path};

pub(crate) use connection_metrics::DatabaseConnectionGuard;
use connection_metrics::DatabaseConnectionMetricState;
pub use connection_metrics::DatabaseConnectionMetrics;

use crate::interaction_repository::{
    InteractionStateKey, clone_interaction_checkpoint_for_branch_transaction,
    interaction_state_key_for_branch, materialize_generation_attempt_interaction_for_append,
    validate_generation_attempt_identity_migration_legacy_rows,
};
use crate::verified_asset_cache::VerifiedAssetCache;

pub(crate) const SCHEMA_VERSION: u32 = 40;
const MIGRATION_0001: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATION_0002: &str = include_str!("../migrations/0002_import_asset_recovery.sql");
const MIGRATION_0003: &str = include_str!("../migrations/0003_conversation_branches.sql");
const MIGRATION_0004: &str = include_str!("../migrations/0004_provider_catalog.sql");
const MIGRATION_0005: &str = crate::discovery::DISCOVERY_STATE_MACHINE_MIGRATION;
const MIGRATION_0006: &str = include_str!("../migrations/0006_generation_provider_provenance.sql");
const MIGRATION_0007: &str = crate::catalog::SIGNED_CATALOG_HISTORY_MIGRATION;
const MIGRATION_0008: &str = include_str!("../migrations/0008_generation_protocol_state.sql");
const MIGRATION_0009: &str = include_str!("../migrations/0009_model_sync_jobs.sql");
const MIGRATION_0010: &str = include_str!("../migrations/0010_provider_connection_tombstones.sql");
const MIGRATION_0011: &str =
    include_str!("../migrations/0011_provider_local_network_approvals.sql");
pub(crate) const FROZEN_NATIVE_SCHEMA_VERSION: u32 = 11;
pub(crate) const FROZEN_NATIVE_MIGRATIONS: [&str; FROZEN_NATIVE_SCHEMA_VERSION as usize] = [
    MIGRATION_0001,
    MIGRATION_0002,
    MIGRATION_0003,
    MIGRATION_0004,
    MIGRATION_0005,
    MIGRATION_0006,
    MIGRATION_0007,
    MIGRATION_0008,
    MIGRATION_0009,
    MIGRATION_0010,
    MIGRATION_0011,
];
const MIGRATION_0012: &str = include_str!("../migrations/0012_content_package_foundation.sql");
const MIGRATION_0013: &str = include_str!("../migrations/0013_prompt_pipeline.sql");
const MIGRATION_0014: &str = include_str!("../migrations/0014_knowledge.sql");
const MIGRATION_0015: &str = include_str!("../migrations/0015_memory.sql");
const MIGRATION_0016: &str = include_str!("../migrations/0016_transforms.sql");
const MIGRATION_0017: &str = include_str!("../migrations/0017_interactions_modules.sql");
const MIGRATION_0018: &str = include_str!("../migrations/0018_persona_selection.sql");
const MIGRATION_0019: &str = include_str!("../migrations/0019_lifecycle_outbox.sql");
const MIGRATION_0020: &str = include_str!("../migrations/0020_package_cas_promotion_journal.sql");
const MIGRATION_0021: &str = include_str!("../migrations/0021_interaction_checkpoints.sql");
const MIGRATION_0022: &str = include_str!("../migrations/0022_memory_vector_space.sql");
const MIGRATION_0023: &str = include_str!("../migrations/0023_applied_module_runtime_plans.sql");
const MIGRATION_0024: &str = include_str!("../migrations/0024_generation_attempt_proposals.sql");
const MIGRATION_0025: &str = include_str!("../migrations/0025_conversation_greeting_bindings.sql");
const MIGRATION_0026: &str =
    include_str!("../migrations/0026_provider_discovery_native_no_effect.sql");
const MIGRATION_0027: &str =
    include_str!("../migrations/0027_provider_discovery_native_attestations.sql");
const MIGRATION_0028: &str =
    include_str!("../migrations/0028_generation_attempt_storage_identities.sql");
const MIGRATION_0029: &str =
    include_str!("../migrations/0029_generation_attempt_decision_handshake.sql");
const MIGRATION_0030: &str = include_str!("../migrations/0030_package_document_target_reviews.sql");
const MIGRATION_0031: &str = include_str!("../migrations/0031_message_display_projections.sql");
const MIGRATION_0032: &str = include_str!("../migrations/0032_knowledge_vector_space.sql");
const MIGRATION_0033: &str =
    include_str!("../migrations/0033_interaction_derived_event_outbox.sql");
const MIGRATION_0034: &str =
    include_str!("../migrations/0034_generation_attempt_derived_event_authority.sql");
const MIGRATION_0035: &str =
    include_str!("../migrations/0035_interaction_derived_event_quarantine.sql");
const MIGRATION_0036: &str =
    include_str!("../migrations/0036_generation_attempt_derived_closure.sql");
const MIGRATION_0037: &str = include_str!("../migrations/0037_provider_credential_operations.sql");
const MIGRATION_0038: &str = include_str!("../migrations/0038_conversation_speakers.sql");
const MIGRATION_0039: &str = include_str!("../migrations/0039_runtime_model_audit.sql");
const MIGRATION_0040: &str = include_str!("../migrations/0040_portable_runtime_state.sql");
const LEGACY_PROVIDER_TEMPLATE_ID: &str = "custom-openai-chat-v1";
const LEGACY_PROVIDER_TEMPLATE_VERSION: u32 = 1;
const LEGACY_BASE_URL_CONFIG_KEY: &str = "api_base_url";
const TEMPERATURE_PARAMETER_ID: &str = "temperature";
const MAX_OUTPUT_TOKENS_PARAMETER_ID: &str = "max_output_tokens";
const MAX_CAPABILITY_VALUE_BYTES: usize = 16 * 1024;
const MAX_CAPABILITY_VALUE_CHARS: usize = 8 * 1024;
const MAX_CAPABILITY_ENUM_VALUES: usize = 128;
const PROVIDER_API_CAPABILITY_FRESHNESS: chrono::Duration = chrono::Duration::hours(24);
const MAX_APPROVED_ASSET_READ_BYTES: u64 = 64 * 1_024 * 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseStats {
    pub characters: u64,
    pub conversations: u64,
    pub messages: u64,
    pub pending_imports: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageGenerationAction {
    EditUser,
    RegenerateAssistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageGenerationActionContext {
    pub fork_message_id: Option<MessageId>,
    pub user_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedAssetImport {
    pub staged_path: PathBuf,
    pub sha256: String,
    pub media_type: String,
    pub size_bytes: u64,
}

/// One verified, bounded byte range from an approved content-addressed asset.
///
/// The storage-owned path and database row never leave this crate. Callers
/// receive only immutable descriptor metadata and bytes from the exact
/// digest-addressed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovedAssetRange {
    pub descriptor: AssetDescriptor,
    pub start: u64,
    pub bytes: Vec<u8>,
}

pub struct Storage {
    root: PathBuf,
    /// Serializes mutation of the shared source/asset CAS.
    ///
    /// Lock order is always `cas_mutation` -> `connection`. A CAS mutation
    /// must never be started while the connection mutex is held. Startup
    /// recovery mutates the same CAS before `Storage` becomes observable and
    /// is additionally protected by the exclusive data-root owner lock.
    cas_mutation: Mutex<()>,
    connection_metrics: DatabaseConnectionMetricState,
    pub(crate) connection: Mutex<Connection>,
    verified_asset_cache: Mutex<VerifiedAssetCache>,
    #[cfg(test)]
    approved_asset_hash_verifications: AtomicUsize,
    _owner_lock: File,
}

struct InterruptedImport {
    id: String,
    source_hash: String,
    staging_path: String,
    state: String,
    asset_hashes: Vec<String>,
}

struct InterruptedGenerationClosure {
    generation_id: GenerationId,
    route: StoredGenerationRoute,
    assistant: Option<Message>,
    attempt_present: bool,
}

impl InterruptedGenerationClosure {
    fn has_durable_partial_checkpoint(&self) -> bool {
        self.attempt_present
            && self
                .assistant
                .as_ref()
                .is_some_and(|assistant| !assistant.content.is_empty())
    }
}

struct RawInterruptedGenerationClosure {
    generation_id: String,
    conversation: String,
    branch: String,
    user_message: String,
    assistant_message: Option<String>,
    provider_family: Option<String>,
    attempt_present: bool,
}

struct StoredGenerationRoute {
    conversation: String,
    branch: String,
    user_message: String,
    assistant_message: Option<String>,
    provider_family: Option<ApiFamily>,
}

type PromptPlanObservation<'a> = (
    &'a crate::orchestration::GenerationPromptPlanRecord,
    &'a [crate::orchestration::KnowledgeActivationLog],
);

struct GenerationAppendObservation<'a> {
    branch_id: &'a ConversationBranchId,
    expected_head: Option<&'a MessageId>,
    user: &'a Message,
    assistant: &'a Message,
    generation: &'a GenerationRecord,
    prompt_plan: Option<PromptPlanObservation<'a>>,
    require_attempt: bool,
}

struct PreparedGenerationAppendAttempt {
    attempt: crate::generation_attempt::StoredGenerationAttempt,
    target_key: InteractionStateKey,
}

struct MessageActionAppendObservation<'a> {
    source_branch_id: &'a ConversationBranchId,
    expected_source_head: Option<&'a MessageId>,
    target_message_id: &'a MessageId,
    action: MessageGenerationAction,
    branch: &'a ConversationBranch,
    target_interaction_state_key: Option<&'a InteractionStateKey>,
    user: &'a Message,
    assistant: &'a Message,
    generation: &'a GenerationRecord,
    prompt_plan: Option<PromptPlanObservation<'a>>,
    require_attempt: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportCommitPhase {
    JournalCreated,
    CasFilesDurable,
    JournalMarkedFileStored,
    RecordsCommitted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageCasPromotionIntent {
    import_id: String,
    namespace: &'static str,
    sha256: String,
    size_bytes: u64,
    media_type: Option<String>,
    relative_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageCasPromotionJournalEntry {
    intent: PackageCasPromotionIntent,
    phase: String,
}

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

    pub fn database_connection_metrics(&self) -> DatabaseConnectionMetrics {
        self.connection_metrics.snapshot()
    }

    pub fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }

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

    fn promote_package_source_observed(
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

    /// Reads and validates the durable migration registry.
    ///
    /// This intentionally does not return the compile-time schema constant:
    /// corruption or out-of-band registry edits after open must remain
    /// observable instead of being reported as a successful current schema.
    pub fn schema_version(&self) -> CoreResult<u32> {
        let connection = self.connection()?;
        read_current_schema_version(&connection)
    }

    pub fn recovery_pending(&self) -> CoreResult<bool> {
        self.connection()?
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM import_jobs
                    UNION ALL
                    SELECT 1 FROM package_cas_promotion_journal
                 )",
                [],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)
    }

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

    fn commit_character_import_observed(
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

    pub fn list_characters(&self) -> CoreResult<Vec<Character>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, name, description, source_hash, avatar_asset_hash, created_at
                 FROM characters ORDER BY name COLLATE NOCASE, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], map_character)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_character(&self, id: &str) -> CoreResult<Character> {
        self.connection()?
            .query_row(
                "SELECT id, name, description, source_hash, avatar_asset_hash, created_at
                 FROM characters WHERE id = ?1",
                [id],
                map_character,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "character was not found", false)
            })
    }

    pub fn save_conversation(&self, conversation: &Conversation) -> CoreResult<()> {
        self.save_conversation_with_mode(conversation, ConversationMode::Chat)
            .map(|_| ())
    }

    /// Returns only selector metadata for the exact active character-content
    /// revision. Greeting source text never crosses this read boundary.
    pub fn character_greeting_catalog(
        &self,
        character_id: &str,
    ) -> CoreResult<CharacterGreetingCatalog> {
        let connection = self.connection()?;
        let revision_id = active_character_content_revision(&connection, character_id)?;
        let greetings = if let Some(revision_id) = revision_id.as_deref() {
            let mut statement = connection
                .prepare(
                    "SELECT greeting_id, kind, enabled
                     FROM character_greetings
                     WHERE character_content_revision_id = ?1
                     ORDER BY ordinal, greeting_id",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([revision_id], |row| {
                    let kind = row.get::<_, String>(1)?;
                    let enabled = row.get::<_, i64>(2)?;
                    Ok(CharacterGreetingOption {
                        id: row.get(0)?,
                        kind: str_to_character_greeting_kind(&kind, 1)?,
                        enabled: match enabled {
                            0 => false,
                            1 => true,
                            other => {
                                return Err(invalid_enum(
                                    2,
                                    "character greeting enabled flag",
                                    &other.to_string(),
                                ));
                            }
                        },
                    })
                })
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        } else {
            Vec::new()
        };
        Ok(CharacterGreetingCatalog {
            character_id: character_id.to_owned(),
            character_content_revision_id: revision_id,
            greetings,
        })
    }

    pub fn save_conversation_with_mode(
        &self,
        conversation: &Conversation,
        mode: ConversationMode,
    ) -> CoreResult<(ConversationBranch, ConversationState)> {
        let catalog = self.character_greeting_catalog(&conversation.character_id)?;
        self.save_conversation_with_greeting(
            conversation,
            mode,
            catalog.character_content_revision_id.as_deref(),
            None,
        )
        .map(|started| (started.branch, started.state))
    }

    /// Atomically binds a new conversation to an exact character-content
    /// revision and resolves its optional greeting inside the same write
    /// transaction.
    ///
    /// `expected_character_content_revision_id = None` means the caller
    /// observed an exact legacy absence, not "choose whatever is current".
    /// `greeting_id = None` deterministically selects the enabled default
    /// greeting for that exact revision, preserving `first_message`
    /// compatibility. An explicit ID never falls back to another greeting.
    pub fn save_conversation_with_greeting(
        &self,
        conversation: &Conversation,
        mode: ConversationMode,
        expected_character_content_revision_id: Option<&str>,
        greeting_id: Option<&str>,
    ) -> CoreResult<ConversationStart> {
        if let Some(greeting_id) = greeting_id {
            validate_character_greeting_id(greeting_id)?;
        }

        let mut branch = ConversationBranch::root(conversation.id.clone());
        let state = ConversationState {
            conversation_id: conversation.id.clone(),
            active_branch_id: branch.id.clone(),
            selected_mode: mode,
            updated_at: conversation.updated_at,
        };
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let active_revision_id =
            active_character_content_revision(&transaction, &conversation.character_id)?;
        if active_revision_id.as_deref() != expected_character_content_revision_id {
            return Err(stale_character_greeting_catalog_error());
        }
        let selected_greeting =
            resolve_character_greeting(&transaction, active_revision_id.as_deref(), greeting_id)?;
        let initial_message = selected_greeting.as_ref().map(|(_, content)| Message {
            id: MessageId::new(),
            conversation_id: conversation.id.clone(),
            parent_id: None,
            role: MessageRole::Assistant,
            content: content.clone(),
            status: MessageStatus::Complete,
            generation_id: Some(GenerationId::for_character_greeting(&conversation.id)),
            created_at: conversation.created_at,
        });
        branch.head_message_id = initial_message.as_ref().map(|message| message.id.clone());
        insert_conversation_start_rows(
            &transaction,
            conversation,
            &branch,
            &state,
            active_revision_id.as_deref(),
            selected_greeting.as_ref(),
            initial_message.as_ref(),
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(ConversationStart {
            conversation: conversation.clone(),
            branch,
            state,
            initial_message,
            character_content_revision_id: active_revision_id,
            greeting_id: selected_greeting.map(|(id, _)| id),
        })
    }

    pub fn list_conversations(&self) -> CoreResult<Vec<Conversation>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, character_id, title, created_at, updated_at
                 FROM conversations ORDER BY updated_at DESC, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(Conversation {
                    id: ConversationId(row.get(0)?),
                    character_id: row.get(1)?,
                    title: row.get(2)?,
                    created_at: parse_datetime_sql(row.get::<_, String>(3)?, 3)?,
                    updated_at: parse_datetime_sql(row.get::<_, String>(4)?, 4)?,
                })
            })
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_conversation_greeting_binding(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<ConversationGreetingBinding> {
        self.connection()?
            .query_row(
                "SELECT conversation_id, character_content_revision_id,
                        greeting_id, created_at
                 FROM conversation_greeting_bindings
                 WHERE conversation_id = ?1",
                [&conversation_id.0],
                |row| {
                    Ok(ConversationGreetingBinding {
                        conversation_id: ConversationId(row.get(0)?),
                        character_content_revision_id: row.get(1)?,
                        greeting_id: row.get(2)?,
                        created_at: parse_datetime_sql(row.get::<_, String>(3)?, 3)?,
                    })
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "conversation greeting binding was not found",
                    false,
                )
            })
    }

    pub fn list_conversations_for_character(
        &self,
        character_id: &str,
    ) -> CoreResult<Vec<Conversation>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, character_id, title, created_at, updated_at
                 FROM conversations
                 WHERE character_id = ?1
                 ORDER BY updated_at DESC, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([character_id], map_conversation)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_conversation(&self, id: &ConversationId) -> CoreResult<Conversation> {
        self.connection()?
            .query_row(
                "SELECT id, character_id, title, created_at, updated_at
                 FROM conversations WHERE id = ?1",
                [&id.0],
                map_conversation,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "conversation was not found", false)
            })
    }

    pub fn get_conversation_state(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<ConversationState> {
        self.connection()?
            .query_row(
                "SELECT conversation_id, active_branch_id, selected_mode, updated_at
                 FROM conversation_state
                 WHERE conversation_id = ?1",
                [&conversation_id.0],
                map_conversation_state,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "conversation state was not found",
                    false,
                )
            })
    }

    pub fn list_conversation_branches(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<Vec<ConversationBranch>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, conversation_id, title, fork_message_id, head_message_id,
                        created_at, updated_at
                 FROM conversation_branches
                 WHERE conversation_id = ?1
                 ORDER BY updated_at DESC, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([&conversation_id.0], map_conversation_branch)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_conversation_branch(
        &self,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ConversationBranch> {
        self.connection()?
            .query_row(
                "SELECT id, conversation_id, title, fork_message_id, head_message_id,
                        created_at, updated_at
                 FROM conversation_branches
                 WHERE id = ?1",
                [&branch_id.0],
                map_conversation_branch,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "conversation branch was not found",
                    false,
                )
            })
    }

    pub fn create_conversation_branch(
        &self,
        conversation_id: &ConversationId,
        from_message_id: Option<&MessageId>,
        title: Option<String>,
    ) -> CoreResult<ConversationBranch> {
        let branch = ConversationBranch {
            id: ConversationBranchId::new(),
            conversation_id: conversation_id.clone(),
            title,
            fork_message_id: from_message_id.cloned(),
            head_message_id: from_message_id.cloned(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let source_branch_id = transaction
            .query_row(
                "SELECT active_branch_id
                 FROM conversation_state
                 WHERE conversation_id = ?1",
                [&conversation_id.0],
                |row| row.get::<_, String>(0).map(ConversationBranchId),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "conversation was not found", false)
            })?;
        if let Some(message_id) = from_message_id {
            let exists = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM messages
                       WHERE id = ?1 AND conversation_id = ?2 AND status <> 'pending'
                     )",
                    params![message_id.0, conversation_id.0],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if !exists {
                return Err(CoreError::new(
                    CoreErrorCode::NotFound,
                    "branch source message was not found in the conversation",
                    false,
                ));
            }
        }
        transaction
            .execute(
                "INSERT INTO conversation_branches
                 (id, conversation_id, title, fork_message_id, head_message_id,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    branch.id.0,
                    branch.conversation_id.0,
                    branch.title,
                    branch
                        .fork_message_id
                        .as_ref()
                        .map(|message_id| message_id.0.as_str()),
                    branch
                        .head_message_id
                        .as_ref()
                        .map(|message_id| message_id.0.as_str()),
                    branch.created_at.to_rfc3339(),
                    branch.updated_at.to_rfc3339()
                ],
            )
            .map_err(storage_db_error)?;
        let target_key = interaction_state_key_for_branch(conversation_id, &branch.id)?;
        clone_interaction_checkpoint_for_branch_transaction(
            &transaction,
            conversation_id,
            &source_branch_id,
            from_message_id,
            &target_key,
            branch.created_at,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(branch)
    }

    pub fn select_conversation_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ConversationState> {
        let now = Utc::now();
        let changed = self
            .connection()?
            .execute(
                "UPDATE conversation_state
                 SET active_branch_id = ?2, updated_at = ?3
                 WHERE conversation_id = ?1
                   AND EXISTS(
                     SELECT 1 FROM conversation_branches
                     WHERE conversation_id = ?1 AND id = ?2
                   )",
                params![conversation_id.0, branch_id.0, now.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        self.get_conversation_state(conversation_id)
    }

    pub fn set_conversation_mode(
        &self,
        conversation_id: &ConversationId,
        mode: ConversationMode,
    ) -> CoreResult<ConversationState> {
        let now = Utc::now();
        let changed = self
            .connection()?
            .execute(
                "UPDATE conversation_state
                 SET selected_mode = ?2, updated_at = ?3
                 WHERE conversation_id = ?1",
                params![conversation_id.0, mode_to_str(mode), now.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "conversation state was not found",
                false,
            ));
        }
        self.get_conversation_state(conversation_id)
    }

    pub fn save_message(&self, message: &Message) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let changed = transaction
            .execute(
                "INSERT INTO messages
                 (id, conversation_id, parent_id, role, content, status, generation_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                   content = excluded.content,
                   status = excluded.status
                 WHERE messages.conversation_id = excluded.conversation_id
                   AND messages.parent_id IS excluded.parent_id
                   AND messages.role = excluded.role
                   AND messages.generation_id IS excluded.generation_id
                   AND messages.created_at = excluded.created_at",
                params![
                    message.id.0,
                    message.conversation_id.0,
                    message.parent_id.as_ref().map(|value| value.0.as_str()),
                    role_to_str(message.role),
                    message.content,
                    status_to_str(message.status),
                    message.generation_id.as_ref().map(|value| value.0.as_str()),
                    message.created_at.to_rfc3339()
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "message identity fields cannot be replaced",
                false,
            ));
        }
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![message.conversation_id.0, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(())
    }

    /// Updates only the content of the matching in-flight assistant row.
    ///
    /// This conditional update prevents a delayed streaming checkpoint from
    /// replacing a terminal message or a row owned by another generation.
    pub fn checkpoint_pending_assistant(&self, message: &Message) -> CoreResult<()> {
        if message.role != MessageRole::Assistant || message.status != MessageStatus::Pending {
            return Err(CoreError::invalid(
                "only a pending assistant message can be checkpointed",
            ));
        }
        let generation_id = message.generation_id.as_ref().ok_or_else(|| {
            CoreError::invalid("a pending assistant checkpoint requires a generation id")
        })?;
        let changed = self
            .connection()?
            .execute(
                "UPDATE messages
                 SET content = ?3
                 WHERE id = ?1
                   AND generation_id = ?2
                   AND role = 'assistant'
                   AND status = 'pending'",
                params![message.id.0, generation_id.0, message.content],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(CoreError::new(
                CoreErrorCode::NotFound,
                "pending assistant checkpoint target was not found",
                false,
            ))
        }
    }

    pub fn delete_message(&self, id: &MessageId) -> CoreResult<()> {
        self.connection()?
            .execute("DELETE FROM messages WHERE id = ?1", [&id.0])
            .map_err(storage_db_error)?;
        Ok(())
    }

    pub fn list_messages(&self, conversation_id: &ConversationId) -> CoreResult<Vec<Message>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM messages WHERE conversation_id = ?1
                 ORDER BY created_at, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([&conversation_id.0], map_message)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn list_branch_messages(
        &self,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Vec<Message>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH RECURSIVE lineage(
                   id, conversation_id, parent_id, role, content, status,
                   generation_id, created_at, depth
                 ) AS (
                   SELECT messages.id, messages.conversation_id, messages.parent_id,
                          messages.role, messages.content, messages.status,
                          messages.generation_id, messages.created_at, 0
                   FROM conversation_branches
                   JOIN messages
                     ON messages.conversation_id = conversation_branches.conversation_id
                    AND messages.id = conversation_branches.head_message_id
                   WHERE conversation_branches.id = ?1
                   UNION ALL
                   SELECT parent.id, parent.conversation_id, parent.parent_id,
                          parent.role, parent.content, parent.status,
                          parent.generation_id, parent.created_at, lineage.depth + 1
                   FROM messages AS parent
                   JOIN lineage
                     ON parent.conversation_id = lineage.conversation_id
                    AND parent.id = lineage.parent_id
                 )
                 SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM lineage
                 ORDER BY depth DESC",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([&branch_id.0], map_message)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn prepare_message_generation_action(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
    ) -> CoreResult<MessageGenerationActionContext> {
        let connection = self.connection()?;
        load_message_generation_action_context(
            &connection,
            conversation_id,
            branch_id,
            expected_head,
            target_message_id,
            action,
        )
    }

    /// Loads the immutable message context needed to derive a generation-action identity.
    ///
    /// This does not authorize a new action against the live branch snapshot. Callers must
    /// either resolve an exact durable operation replay or call
    /// [`Self::prepare_message_generation_action`] before creating a new attempt.
    pub fn load_message_generation_action_identity_context(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
    ) -> CoreResult<MessageGenerationActionContext> {
        let connection = self.connection()?;
        load_message_generation_action_identity_context(
            &connection,
            conversation_id,
            branch_id,
            target_message_id,
            action,
        )
    }

    pub fn list_recent_message_lineage_for_prompt(
        &self,
        conversation_id: &ConversationId,
        head_message_id: Option<&MessageId>,
        max_messages: usize,
        max_message_bytes: usize,
        max_message_chars: usize,
    ) -> CoreResult<Vec<Message>> {
        if head_message_id.is_none()
            || max_messages == 0
            || max_message_bytes == 0
            || max_message_chars == 0
        {
            return Ok(Vec::new());
        }
        let max_messages = i64::try_from(max_messages)
            .map_err(|_| CoreError::invalid("message limit exceeds SQLite integer range"))?;
        let max_message_bytes = i64::try_from(max_message_bytes)
            .map_err(|_| CoreError::invalid("byte limit exceeds SQLite integer range"))?;
        let max_message_chars = i64::try_from(max_message_chars)
            .map_err(|_| CoreError::invalid("character limit exceeds SQLite integer range"))?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH RECURSIVE lineage(
                   id, conversation_id, parent_id, role, content, status,
                   generation_id, created_at, depth
                 ) AS (
                   SELECT id, conversation_id, parent_id, role, content, status,
                          generation_id, created_at, 0
                   FROM messages
                   WHERE conversation_id = ?1 AND id = ?2
                   UNION ALL
                   SELECT parent.id, parent.conversation_id, parent.parent_id,
                          parent.role, parent.content, parent.status,
                          parent.generation_id, parent.created_at, lineage.depth + 1
                   FROM messages AS parent
                   JOIN lineage
                     ON parent.conversation_id = lineage.conversation_id
                    AND parent.id = lineage.parent_id
                   WHERE lineage.depth < 511
                 ),
                 selected AS (
                   SELECT *
                   FROM lineage
                   WHERE role != 'system'
                     AND status != 'pending'
                     AND (status = 'complete' OR length(content) > 0)
                     AND length(CAST(content AS BLOB)) <= ?4
                     AND length(content) <= ?5
                   ORDER BY depth
                   LIMIT ?3
                 )
                 SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM selected
                 ORDER BY depth DESC",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map(
                params![
                    conversation_id.0,
                    head_message_id.map(|message_id| message_id.0.as_str()),
                    max_messages,
                    max_message_bytes,
                    max_message_chars
                ],
                map_message,
            )
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    /// Loads the newest eligible suffix from one selected message lineage.
    pub fn list_recent_branch_messages_for_prompt(
        &self,
        branch_id: &ConversationBranchId,
        max_messages: usize,
        max_message_bytes: usize,
        max_message_chars: usize,
    ) -> CoreResult<Vec<Message>> {
        if max_messages == 0 || max_message_bytes == 0 || max_message_chars == 0 {
            return Ok(Vec::new());
        }
        let max_messages = i64::try_from(max_messages)
            .map_err(|_| CoreError::invalid("message limit exceeds SQLite integer range"))?;
        let max_message_bytes = i64::try_from(max_message_bytes)
            .map_err(|_| CoreError::invalid("byte limit exceeds SQLite integer range"))?;
        let max_message_chars = i64::try_from(max_message_chars)
            .map_err(|_| CoreError::invalid("character limit exceeds SQLite integer range"))?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH RECURSIVE lineage(
                   id, conversation_id, parent_id, role, content, status,
                   generation_id, created_at, depth
                 ) AS (
                   SELECT messages.id, messages.conversation_id, messages.parent_id,
                          messages.role, messages.content, messages.status,
                          messages.generation_id, messages.created_at, 0
                   FROM conversation_branches
                   JOIN messages
                     ON messages.conversation_id = conversation_branches.conversation_id
                    AND messages.id = conversation_branches.head_message_id
                   WHERE conversation_branches.id = ?1
                   UNION ALL
                   SELECT parent.id, parent.conversation_id, parent.parent_id,
                          parent.role, parent.content, parent.status,
                          parent.generation_id, parent.created_at, lineage.depth + 1
                   FROM messages AS parent
                   JOIN lineage
                     ON parent.conversation_id = lineage.conversation_id
                    AND parent.id = lineage.parent_id
                   WHERE lineage.depth < 511
                 ),
                 selected AS (
                   SELECT *
                   FROM lineage
                   WHERE role != 'system'
                     AND status != 'pending'
                     AND (status = 'complete' OR length(content) > 0)
                     AND length(CAST(content AS BLOB)) <= ?3
                     AND length(content) <= ?4
                   ORDER BY depth
                   LIMIT ?2
                 )
                 SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM selected
                 ORDER BY depth DESC",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map(
                params![
                    branch_id.0,
                    max_messages,
                    max_message_bytes,
                    max_message_chars
                ],
                map_message,
            )
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn append_generation(
        &self,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
    ) -> CoreResult<()> {
        self.append_generation_observed(
            branch_id,
            expected_head,
            user,
            assistant,
            generation,
            None,
            false,
            None,
            false,
        )
    }

    /// Atomically appends the user/assistant messages, a sealed prompt plan,
    /// its credential-free provider request evidence, and the linked
    /// generation. Any validation, head-CAS, or persistence failure rolls the
    /// complete `SQLite` mutation back.
    #[allow(clippy::too_many_arguments)]
    pub fn append_generation_with_prompt_plan(
        &self,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
        knowledge_logs: &[crate::orchestration::KnowledgeActivationLog],
    ) -> CoreResult<()> {
        self.append_generation_observed(
            branch_id,
            expected_head,
            user,
            assistant,
            generation,
            Some((prompt_plan, knowledge_logs)),
            false,
            None,
            false,
        )
    }

    /// Production append boundary for a generation whose exact
    /// `BeforeGeneration` processing has reached `dispatch_ready`.
    ///
    /// The attempt identity, source head, target branch, module composition,
    /// and prompt fingerprint are rechecked in the same transaction that
    /// makes the generation visible and transitions the attempt to `running`.
    #[allow(clippy::too_many_arguments)]
    pub fn append_generation_attempt_with_prompt_plan(
        &self,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
        knowledge_logs: &[crate::orchestration::KnowledgeActivationLog],
        credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        self.append_generation_observed(
            branch_id,
            expected_head,
            user,
            assistant,
            generation,
            Some((prompt_plan, knowledge_logs)),
            true,
            credential_authority,
            require_exact_credential_authority,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn append_generation_observed(
        &self,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: Option<(
            &crate::orchestration::GenerationPromptPlanRecord,
            &[crate::orchestration::KnowledgeActivationLog],
        )>,
        require_attempt: bool,
        credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        let observation = GenerationAppendObservation {
            branch_id,
            expected_head,
            user,
            assistant,
            generation,
            prompt_plan,
            require_attempt,
        };
        validate_generation_append(branch_id, expected_head, user, assistant, generation)?;
        validate_generation_prompt_plan_link(
            branch_id,
            expected_head,
            user,
            generation,
            prompt_plan.map(|value| value.0),
        )?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        ensure_generation_provider_credential_settled(
            &transaction,
            generation,
            credential_authority,
            require_exact_credential_authority,
        )?;
        require_generation_append_branch(&transaction, &observation)?;
        let dispatch_attempt = prepare_generation_append_attempt(&transaction, &observation)?;
        let occurred_at = Utc::now();
        if let Some(prepared) = dispatch_attempt.as_ref() {
            materialize_generation_append_attempt(
                self,
                &transaction,
                &observation,
                prepared,
                occurred_at,
            )?;
        }
        write_generation_append(
            &transaction,
            &observation,
            dispatch_attempt.as_ref(),
            occurred_at,
        )?;
        transaction.commit().map_err(storage_db_error)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append_message_generation_action(
        &self,
        source_branch_id: &ConversationBranchId,
        expected_source_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
        branch: &ConversationBranch,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
    ) -> CoreResult<()> {
        self.append_message_generation_action_observed(
            source_branch_id,
            expected_source_head,
            target_message_id,
            action,
            branch,
            None,
            user,
            assistant,
            generation,
            None,
            false,
            None,
            false,
        )
    }

    /// Atomic message-action variant that seals and attaches exact prompt
    /// provenance before the new generation becomes visible.
    #[allow(clippy::too_many_arguments)]
    pub fn append_message_generation_action_with_prompt_plan(
        &self,
        source_branch_id: &ConversationBranchId,
        expected_source_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
        branch: &ConversationBranch,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
        knowledge_logs: &[crate::orchestration::KnowledgeActivationLog],
    ) -> CoreResult<()> {
        self.append_message_generation_action_observed(
            source_branch_id,
            expected_source_head,
            target_message_id,
            action,
            branch,
            None,
            user,
            assistant,
            generation,
            Some((prompt_plan, knowledge_logs)),
            false,
            None,
            false,
        )
    }

    /// Attempt-bound action append. The new branch and its complete generation
    /// remain invisible unless the exact source snapshot and dispatch-ready
    /// attempt both validate in this transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn append_message_generation_action_attempt_with_prompt_plan(
        &self,
        source_branch_id: &ConversationBranchId,
        expected_source_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
        branch: &ConversationBranch,
        target_interaction_state_key: &InteractionStateKey,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
        knowledge_logs: &[crate::orchestration::KnowledgeActivationLog],
        credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        self.append_message_generation_action_observed(
            source_branch_id,
            expected_source_head,
            target_message_id,
            action,
            branch,
            Some(target_interaction_state_key),
            user,
            assistant,
            generation,
            Some((prompt_plan, knowledge_logs)),
            true,
            credential_authority,
            require_exact_credential_authority,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn append_message_generation_action_observed(
        &self,
        source_branch_id: &ConversationBranchId,
        expected_source_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
        branch: &ConversationBranch,
        target_interaction_state_key: Option<&InteractionStateKey>,
        user: &Message,
        assistant: &Message,
        generation: &GenerationRecord,
        prompt_plan: Option<(
            &crate::orchestration::GenerationPromptPlanRecord,
            &[crate::orchestration::KnowledgeActivationLog],
        )>,
        require_attempt: bool,
        credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        let observation = MessageActionAppendObservation {
            source_branch_id,
            expected_source_head,
            target_message_id,
            action,
            branch,
            target_interaction_state_key,
            user,
            assistant,
            generation,
            prompt_plan,
            require_attempt,
        };
        validate_message_action_append(&observation)?;

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        ensure_generation_provider_credential_settled(
            &transaction,
            generation,
            credential_authority,
            require_exact_credential_authority,
        )?;
        require_message_action_context(&transaction, &observation)?;
        let dispatch_attempt = prepare_message_action_attempt(&transaction, &observation)?;
        let occurred_at = Utc::now();
        write_message_action_append(
            self,
            &transaction,
            &observation,
            dispatch_attempt.as_ref(),
            occurred_at,
        )?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn remove_message_from_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        target_message_id: &MessageId,
    ) -> CoreResult<ConversationBranch> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let target = load_branch_action_target(
            &transaction,
            conversation_id,
            branch_id,
            expected_head,
            target_message_id,
        )?;
        if target.status == MessageStatus::Pending {
            return Err(active_generation_action_error());
        }
        if !matches!(target.role, MessageRole::User | MessageRole::Assistant) {
            return Err(CoreError::invalid(
                "only user or assistant messages can be removed from a branch",
            ));
        }
        require_removal_parent_not_pending(
            &transaction,
            conversation_id,
            target.parent_id.as_ref(),
        )?;

        let invalidated_at = Utc::now();
        let removed_head = expected_head.ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "non-empty branch removal is missing its validated head",
                false,
            )
        })?;
        crate::orchestration::invalidate_memory_range_in_transaction(
            &transaction,
            conversation_id,
            branch_id,
            target_message_id,
            removed_head,
            invalidated_at,
        )?;
        let now = invalidated_at.to_rfc3339();
        let changed = transaction
            .execute(
                "UPDATE conversation_branches
                 SET head_message_id = ?3, updated_at = ?4
                 WHERE id = ?1
                   AND conversation_id = ?2
                   AND (
                     (head_message_id IS NULL AND ?5 IS NULL)
                     OR head_message_id = ?5
                   )",
                params![
                    branch_id.0,
                    conversation_id.0,
                    target
                        .parent_id
                        .as_ref()
                        .map(|message_id| message_id.0.as_str()),
                    now,
                    expected_head.map(|message_id| message_id.0.as_str())
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(stale_branch_error());
        }
        crate::portable_runtime_state::invalidate_portable_runtime_state_for_branch_transaction(
            &transaction,
            &conversation_id.0,
            &branch_id.0,
            invalidated_at,
        )?;
        transaction
            .execute(
                "UPDATE conversation_state
                 SET updated_at = ?3
                 WHERE conversation_id = ?1 AND active_branch_id = ?2",
                params![conversation_id.0, branch_id.0, now],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![conversation_id.0, now],
            )
            .map_err(storage_db_error)?;
        let branch = transaction
            .query_row(
                "SELECT id, conversation_id, title, fork_message_id, head_message_id,
                        created_at, updated_at
                 FROM conversation_branches
                 WHERE id = ?1 AND conversation_id = ?2",
                params![branch_id.0, conversation_id.0],
                map_conversation_branch,
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(branch)
    }

    pub fn finalize_generation(
        &self,
        assistant: &Message,
        usage: Option<&GenerationUsage>,
        error_code: Option<&str>,
        keep_assistant: bool,
    ) -> CoreResult<()> {
        self.finalize_generation_with_protocol_state(
            assistant,
            usage,
            &[],
            error_code,
            keep_assistant,
        )
    }

    pub fn finalize_generation_with_protocol_state(
        &self,
        assistant: &Message,
        usage: Option<&GenerationUsage>,
        opaque_reasoning_state: &[OpaqueReasoningState],
        error_code: Option<&str>,
        keep_assistant: bool,
    ) -> CoreResult<()> {
        self.finalize_generation_with_protocol_state_and_display(
            assistant,
            usage,
            opaque_reasoning_state,
            error_code,
            keep_assistant,
            None,
        )
    }

    /// Atomically finalizes a generation together with its bounded `DisplayOnly`
    /// sidecar and content-free transform application diagnostics.
    pub fn finalize_generation_with_protocol_state_and_display(
        &self,
        assistant: &Message,
        usage: Option<&GenerationUsage>,
        opaque_reasoning_state: &[OpaqueReasoningState],
        error_code: Option<&str>,
        keep_assistant: bool,
        display_projection: Option<
            &crate::message_display_projection::MessageDisplayProjectionWrite,
        >,
    ) -> CoreResult<()> {
        if assistant.role != MessageRole::Assistant || assistant.status == MessageStatus::Pending {
            return Err(CoreError::invalid(
                "only a terminal assistant message can finalize a generation",
            ));
        }
        if assistant.status != MessageStatus::Complete && !opaque_reasoning_state.is_empty() {
            return Err(CoreError::invalid(
                "opaque reasoning state can be stored only for a completed generation",
            ));
        }
        if !keep_assistant && display_projection.is_some() {
            return Err(CoreError::invalid(
                "a discarded assistant message cannot retain a display projection",
            ));
        }
        let generation_id = assistant.generation_id.as_ref().ok_or_else(|| {
            CoreError::invalid("a terminal assistant message requires a generation id")
        })?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let generation = load_running_generation(&transaction, generation_id)?;
        validate_generation_assistant_ownership(&generation, assistant)?;
        let opaque_reasoning_state = serialize_opaque_reasoning_state_for_family(
            generation.provider_family,
            opaque_reasoning_state,
        )?;
        let occurred_at = Utc::now();
        let now = occurred_at.to_rfc3339();
        persist_terminal_assistant(
            &transaction,
            assistant,
            generation_id,
            &generation,
            &now,
            keep_assistant,
        )?;
        if keep_assistant {
            crate::message_display_projection::persist_terminal_message_display_projection(
                &transaction,
                assistant,
                display_projection,
                occurred_at,
            )?;
        }
        Self::update_generation_terminal_row(
            &transaction,
            generation_id,
            assistant.status,
            usage,
            opaque_reasoning_state.as_deref(),
            error_code,
            &now,
        )?;
        crate::generation_attempt::mark_attempt_completed_if_present_in_transaction(
            &transaction,
            generation_id,
            occurred_at,
        )?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![assistant.conversation_id.0, now],
            )
            .map_err(storage_db_error)?;
        Self::insert_generation_terminal_occurrences(
            &transaction,
            assistant,
            generation_id,
            &generation,
            keep_assistant,
            occurred_at,
        )?;
        transaction.commit().map_err(storage_db_error)
    }

    fn update_generation_terminal_row(
        transaction: &rusqlite::Transaction<'_>,
        generation_id: &GenerationId,
        assistant_status: MessageStatus,
        usage: Option<&GenerationUsage>,
        opaque_reasoning_state: Option<&str>,
        error_code: Option<&str>,
        finished_at: &str,
    ) -> CoreResult<()> {
        let token_count = |value: Option<u64>| value.map(u64_to_i64).transpose();
        let input_tokens = token_count(usage.and_then(|usage| usage.input_tokens))?;
        let cached_read_tokens = token_count(usage.and_then(|usage| usage.cached_read_tokens))?;
        let cached_write_tokens = token_count(usage.and_then(|usage| usage.cached_write_tokens))?;
        let output_tokens = token_count(usage.and_then(|usage| usage.output_tokens))?;
        let reasoning_tokens = token_count(usage.and_then(|usage| usage.reasoning_tokens))?;
        let tool_tokens = token_count(usage.and_then(|usage| usage.tool_tokens))?;
        let provider_raw_summary = usage
            .and_then(|usage| usage.provider_raw_summary.as_ref())
            .map(BoundedJson::as_str);
        transaction
            .execute(
                "UPDATE generations
                 SET status = ?2,
                     input_tokens = ?3,
                     cached_read_tokens = ?4,
                     cached_write_tokens = ?5,
                     output_tokens = ?6,
                     reasoning_tokens = ?7,
                     tool_tokens = ?8,
                     provider_raw_summary_json = ?9,
                     opaque_reasoning_state_json = ?10,
                     error_code = ?11,
                     finished_at = ?12
                 WHERE id = ?1 AND status = 'running'",
                params![
                    generation_id.0,
                    generation_status_to_str(message_status_to_generation_status(assistant_status)),
                    input_tokens,
                    cached_read_tokens,
                    cached_write_tokens,
                    output_tokens,
                    reasoning_tokens,
                    tool_tokens,
                    provider_raw_summary,
                    opaque_reasoning_state,
                    error_code,
                    finished_at
                ],
            )
            .map_err(storage_db_error)?;
        Ok(())
    }

    fn insert_generation_terminal_occurrences(
        transaction: &rusqlite::Transaction<'_>,
        assistant: &Message,
        generation_id: &GenerationId,
        generation: &StoredGenerationRoute,
        keep_assistant: bool,
        occurred_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let exact_head_message_id = transaction
            .query_row(
                "SELECT head_message_id
                 FROM conversation_branches
                 WHERE conversation_id = ?1 AND id = ?2",
                params![assistant.conversation_id.0, generation.branch],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(storage_db_error)?
            .map(MessageId);
        crate::lifecycle_outbox::insert_occurrence(
            transaction,
            &crate::lifecycle_outbox::LifecycleOccurrenceWrite {
                occurrence_id: format!("after-generation:{}", generation_id.0),
                event_kind: crate::lifecycle_outbox::LifecycleOccurrenceKind::AfterGeneration,
                conversation_id: assistant.conversation_id.clone(),
                branch_id: ConversationBranchId(generation.branch.clone()),
                exact_head_message_id: exact_head_message_id.clone(),
                owner_message_id: keep_assistant.then(|| assistant.id.clone()),
                generation_id: Some(generation_id.clone()),
                occurred_at,
            },
            false,
        )?;
        if keep_assistant {
            crate::lifecycle_outbox::insert_occurrence(
                transaction,
                &crate::lifecycle_outbox::LifecycleOccurrenceWrite {
                    occurrence_id: format!("message-committed:{}", assistant.id.0),
                    event_kind: crate::lifecycle_outbox::LifecycleOccurrenceKind::MessageCommitted,
                    conversation_id: assistant.conversation_id.clone(),
                    branch_id: ConversationBranchId(generation.branch.clone()),
                    exact_head_message_id,
                    owner_message_id: Some(assistant.id.clone()),
                    generation_id: Some(generation_id.clone()),
                    occurred_at,
                },
                false,
            )?;
        }
        Ok(())
    }

    /// Marks a generation failed after its normal terminal transaction could not complete.
    ///
    /// This intentionally stores only a stable error code. Provider credentials and raw
    /// persistence errors must never enter the conversation database.
    pub fn fail_generation_after_finalize_error(
        &self,
        assistant: &Message,
        keep_assistant: bool,
    ) -> CoreResult<()> {
        if assistant.role != MessageRole::Assistant || assistant.status != MessageStatus::Failed {
            return Err(CoreError::invalid(
                "only a failed assistant message can compensate a generation finalization",
            ));
        }
        let generation_id = assistant.generation_id.as_ref().ok_or_else(|| {
            CoreError::invalid("a failed assistant message requires a generation id")
        })?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let generation = load_running_generation(&transaction, generation_id)?;
        if generation.conversation != assistant.conversation_id.0
            || generation.assistant_message.as_deref() != Some(assistant.id.0.as_str())
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation assistant ownership is inconsistent",
                false,
            ));
        }
        let occurred_at = Utc::now();
        let now = occurred_at.to_rfc3339();
        compensate_terminal_assistant(
            &transaction,
            assistant,
            generation_id,
            &generation,
            &now,
            keep_assistant,
        )?;
        let changed = transaction
            .execute(
                "UPDATE generations
                SET status = 'failed',
                     input_tokens = NULL,
                     cached_read_tokens = NULL,
                     cached_write_tokens = NULL,
                     output_tokens = NULL,
                     reasoning_tokens = NULL,
                     tool_tokens = NULL,
                     provider_raw_summary_json = NULL,
                     opaque_reasoning_state_json = NULL,
                     error_code = 'storage_unavailable',
                     finished_at = ?2
                 WHERE id = ?1 AND status = 'running'",
                params![generation_id.0, now],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation compensation target was not found",
                false,
            ));
        }
        crate::generation_attempt::mark_attempt_completed_if_present_in_transaction(
            &transaction,
            generation_id,
            occurred_at,
        )?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![assistant.conversation_id.0, now],
            )
            .map_err(storage_db_error)?;
        Self::insert_generation_terminal_occurrences(
            &transaction,
            assistant,
            generation_id,
            &generation,
            keep_assistant,
            occurred_at,
        )?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn get_generation(&self, id: &GenerationId) -> CoreResult<GenerationRecord> {
        self.connection()?
            .query_row(
                "SELECT id, conversation_id, branch_id, user_message_id,
                        assistant_message_id, mode, model, status, input_tokens,
                        output_tokens, error_code, started_at, finished_at,
                        model_route_id, generation_preset_id, provider_family,
                        cached_read_tokens, cached_write_tokens, reasoning_tokens,
                        tool_tokens, provider_raw_summary_json,
                        opaque_reasoning_state_json
                 FROM generations
                WHERE id = ?1",
                [&id.0],
                map_generation,
            )
            .optional()
            .map_err(generation_read_error)?
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "generation was not found", false)
            })
    }

    /// Loads a bounded recent suffix without materializing oversized legacy rows.
    pub fn list_recent_messages_for_prompt(
        &self,
        conversation_id: &ConversationId,
        max_messages: usize,
        max_message_bytes: usize,
        max_message_chars: usize,
    ) -> CoreResult<Vec<Message>> {
        if max_messages == 0 || max_message_bytes == 0 || max_message_chars == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM (
                   SELECT id, conversation_id, parent_id, role, content, status,
                          generation_id, created_at
                   FROM messages
                   WHERE conversation_id = ?1
                     AND role != 'system'
                     AND length(CAST(content AS BLOB)) <= ?3
                     AND length(content) <= ?4
                   ORDER BY created_at DESC, id DESC
                   LIMIT ?2
                 )
                 ORDER BY created_at, id",
            )
            .map_err(storage_db_error)?;
        let max_messages = i64::try_from(max_messages)
            .map_err(|_| CoreError::invalid("message limit exceeds SQLite integer range"))?;
        let max_message_bytes = i64::try_from(max_message_bytes)
            .map_err(|_| CoreError::invalid("byte limit exceeds SQLite integer range"))?;
        let max_message_chars = i64::try_from(max_message_chars)
            .map_err(|_| CoreError::invalid("character limit exceeds SQLite integer range"))?;
        let rows = statement
            .query_map(
                params![
                    conversation_id.0,
                    max_messages,
                    max_message_bytes,
                    max_message_chars
                ],
                map_message,
            )
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn load_settings(&self) -> CoreResult<AppSettings> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let settings = load_or_create_settings_in_transaction(&transaction)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let stored = load_or_create_settings_in_transaction(&transaction)?;
        let mut requested = settings.clone();
        // The local identity is repository-owned. Shell/API callers may update
        // preferences but cannot replace the durable user identity.
        requested.local_user_id = stored.local_user_id;
        let settings = normalize_settings_for_write(&transaction, &requested)?;
        write_application_settings(&transaction, &settings)?;
        // `save_settings` is a user-facing settings intent. Advancing even for
        // an identical provider selection records an explicit None after a
        // discovery-owned clear and closes that otherwise invisible ABA.
        advance_provider_selection_revision(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Atomically persists one explicit generation-target selection intent.
    ///
    /// Other application preferences are loaded and retained inside the same
    /// transaction. The provider-selection revision advances even when all
    /// three requested values are already `None`, because that no-op value is
    /// still newer user intent than an earlier discovery compensation clear.
    pub fn save_generation_target_selection(
        &self,
        selected_provider_profile_id: Option<String>,
        selected_model_route_id: Option<ModelRouteId>,
        selected_generation_preset_id: Option<GenerationPresetId>,
    ) -> CoreResult<AppSettings> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let mut settings = load_or_create_settings_in_transaction(&transaction)?;
        settings.selected_provider_profile_id = selected_provider_profile_id;
        settings.selected_model_route_id = selected_model_route_id;
        settings.selected_generation_preset_id = selected_generation_preset_id;
        let settings = normalize_settings_for_write(&transaction, &settings)?;
        write_application_settings(&transaction, &settings)?;
        advance_provider_selection_revision(&transaction)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(settings)
    }

    pub fn list_provider_profiles(&self) -> CoreResult<Vec<ProviderProfile>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT profile.id, profile.display_name, profile.base_url,
                        profile.model, profile.timeout_seconds
                 FROM provider_profiles AS profile
                 JOIN provider_connections AS connection
                   ON connection.id = profile.id
                  AND connection.archived_at IS NULL
                 ORDER BY profile.display_name COLLATE NOCASE, profile.id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], map_provider_profile)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_provider_profile(&self, id: &str) -> CoreResult<ProviderProfile> {
        self.connection()?
            .query_row(
                "SELECT profile.id, profile.display_name, profile.base_url,
                        profile.model, profile.timeout_seconds
                 FROM provider_profiles AS profile
                 JOIN provider_connections AS connection
                   ON connection.id = profile.id
                  AND connection.archived_at IS NULL
                 WHERE profile.id = ?1",
                [id],
                map_provider_profile,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "provider profile was not found",
                    false,
                )
            })
    }

    pub fn save_provider_profile(&self, profile: &ProviderProfile) -> CoreResult<()> {
        let (connection_value, mut route, mut preset) = legacy_provider_graph(profile, Utc::now())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        ensure_legacy_template_exists(&transaction)?;
        upsert_provider_profile_row(&transaction, profile)?;
        upsert_provider_connection_row(&transaction, &connection_value)?;

        // A legacy profile can change its selected model, but a ModelRoute
        // identity cannot be renamed. Reuse an exact route when it exists;
        // otherwise create a deterministic sibling and preserve the old
        // route, presets, and generation/conversation references.
        route.id = legacy_model_route_id(&transaction, &route)?;
        preset.id = GenerationPresetId::from(route.id.as_str());
        preset.model_route_id = route.id.clone();
        upsert_model_route_row(&transaction, &route)?;
        if !row_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM generation_presets WHERE id = ?1)",
            preset.id.as_str(),
        )? {
            upsert_generation_preset_row(&transaction, &preset)?;
        }
        update_stored_settings(&transaction, |settings| {
            if settings.selected_provider_profile_id.as_deref() == Some(profile.id.as_str()) {
                settings.selected_model_route_id = Some(route.id.clone());
                settings.selected_generation_preset_id = Some(preset.id.clone());
            }
            Ok(())
        })?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn delete_provider_profile(&self, id: &str) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let active_profile = row_exists(
            &transaction,
            "SELECT EXISTS(
               SELECT 1
               FROM provider_profiles AS profile
               JOIN provider_connections AS connection
                 ON connection.id = profile.id
                AND connection.archived_at IS NULL
               WHERE profile.id = ?1
             )",
            id,
        )?;
        if !active_profile {
            return Err(not_found("provider profile"));
        }
        archive_provider_connection_row(&transaction, id, Utc::now())?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn list_provider_templates(&self) -> CoreResult<Vec<ProviderTemplate>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
                 FROM provider_templates
                 ORDER BY display_name COLLATE NOCASE, id, version DESC",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter().map(decode_provider_template_row).collect()
    }

    pub fn get_provider_template(
        &self,
        id: &ProviderTemplateId,
        version: u32,
    ) -> CoreResult<ProviderTemplate> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
                 FROM provider_templates WHERE id = ?1 AND version = ?2",
                params![id.as_str(), version],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("provider template"))?;
        decode_provider_template_row(row)
    }

    pub fn save_provider_template(&self, template: &ProviderTemplate) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        save_provider_template_row(&transaction, template)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn list_provider_connections(&self) -> CoreResult<Vec<ProviderConnection>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, template_id, template_version, display_name, api_origin,
                        config_json, credential_ref, credential_scope_json, timeout_seconds,
                        status, created_at, updated_at
                 FROM provider_connections
                 WHERE archived_at IS NULL
                 ORDER BY display_name COLLATE NOCASE, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], provider_connection_columns)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter()
            .map(decode_provider_connection_row)
            .collect()
    }

    pub fn get_provider_connection(
        &self,
        id: &ProviderConnectionId,
    ) -> CoreResult<ProviderConnection> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, template_id, template_version, display_name, api_origin,
                        config_json, credential_ref, credential_scope_json, timeout_seconds,
                        status, created_at, updated_at
                 FROM provider_connections
                 WHERE id = ?1 AND archived_at IS NULL",
                [id.as_str()],
                provider_connection_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("provider connection"))?;
        decode_provider_connection_row(row)
    }

    pub fn save_provider_connection(
        &self,
        connection_value: &ProviderConnection,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        validate_provider_connection(&transaction, connection_value)?;
        upsert_provider_connection_row(&transaction, connection_value)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Inserts a newly reviewed connection without permitting an existing
    /// identity to be overwritten.
    pub fn insert_provider_connection(
        &self,
        connection_value: &ProviderConnection,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        validate_provider_connection(&transaction, connection_value)?;
        ensure_provider_connection_id_vacant(&transaction, &connection_value.id)?;
        upsert_provider_connection_row(&transaction, connection_value)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Save a connection only while the reviewed provider catalog state is
    /// still active.
    ///
    /// Signed templates are copied into the immutable template table solely
    /// to satisfy the connection foreign key. This transaction-level CAS
    /// prevents a concurrent catalog rollback from creating a new connection
    /// against a template which is no longer active.
    pub fn save_provider_connection_for_catalog_state(
        &self,
        connection_value: &ProviderConnection,
        catalog_template: &ProviderTemplate,
        expected_catalog_state_version: u64,
    ) -> CoreResult<()> {
        self.persist_provider_connection_for_catalog_state(
            connection_value,
            catalog_template,
            expected_catalog_state_version,
            false,
        )
    }

    /// Inserts a newly reviewed signed-catalog connection while atomically
    /// rejecting both a stale catalog review and an occupied connection ID.
    pub fn insert_provider_connection_for_catalog_state(
        &self,
        connection_value: &ProviderConnection,
        catalog_template: &ProviderTemplate,
        expected_catalog_state_version: u64,
    ) -> CoreResult<()> {
        self.persist_provider_connection_for_catalog_state(
            connection_value,
            catalog_template,
            expected_catalog_state_version,
            true,
        )
    }

    fn persist_provider_connection_for_catalog_state(
        &self,
        connection_value: &ProviderConnection,
        catalog_template: &ProviderTemplate,
        expected_catalog_state_version: u64,
        insert_only: bool,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let current_state_version = transaction
            .query_row(
                "SELECT state_version
                 FROM provider_catalog_state
                 WHERE singleton = 1",
                [],
                |row| row.get::<_, u64>(0),
            )
            .map_err(storage_db_error)?;
        if current_state_version != expected_catalog_state_version {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "provider catalog changed; review the connection again",
                true,
            ));
        }
        if connection_value.template_id != catalog_template.id
            || connection_value.template_version != catalog_template.manifest_version
            || catalog_template.source != TemplateSource::SignedCatalog
        {
            return Err(CoreError::invalid(
                "catalog connection does not match its signed provider template",
            ));
        }
        save_provider_template_row(&transaction, catalog_template)?;
        validate_provider_connection(&transaction, connection_value)?;
        if insert_only {
            ensure_provider_connection_id_vacant(&transaction, &connection_value.id)?;
        }
        upsert_provider_connection_row(&transaction, connection_value)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn delete_provider_connection(&self, id: &ProviderConnectionId) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let credential_bound = transaction
            .query_row(
                "SELECT credential_ref IS NOT NULL
                 FROM provider_connections
                 WHERE id = ?1 AND archived_at IS NULL",
                [id.as_str()],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("provider connection"))?;
        if credential_bound {
            return Err(CoreError::invalid(
                "credential-bound provider connections require durable native removal before archive",
            ));
        }
        archive_provider_connection_row(&transaction, id.as_str(), Utc::now())?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn list_model_routes(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<Vec<ModelRoute>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, connection_id, api_family, model_id, display_name, route_json,
                        availability, raw_metadata_json, miss_count, metadata_source_kind,
                        metadata_observed_at, last_reconciled_sync_job_id,
                        metadata_sync_job_id, first_seen_at, last_seen_at
                 FROM provider_models WHERE connection_id = ?1
                 ORDER BY model_id COLLATE NOCASE, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([connection_id.as_str()], model_route_columns)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter().map(decode_model_route_row).collect()
    }

    pub fn get_model_route(&self, id: &ModelRouteId) -> CoreResult<ModelRoute> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, connection_id, api_family, model_id, display_name, route_json,
                        availability, raw_metadata_json, miss_count, metadata_source_kind,
                        metadata_observed_at, last_reconciled_sync_job_id,
                        metadata_sync_job_id, first_seen_at, last_seen_at
                 FROM provider_models WHERE id = ?1",
                [id.as_str()],
                model_route_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("model route"))?;
        decode_model_route_row(row)
    }

    pub fn save_model_route(&self, route: &ModelRoute) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        if active_retained_legacy_profile_exists(&transaction, route.connection_id.as_str())? {
            return Err(CoreError::invalid(
                "migrated legacy model routes are managed through their retained provider profile",
            ));
        }
        validate_model_route(&transaction, route)?;
        upsert_model_route_row(&transaction, route)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn reconcile_model_routes(
        &self,
        connection_id: &ProviderConnectionId,
        listed_routes: &[ModelRoute],
        observed_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        if !row_exists(
            &transaction,
            "SELECT EXISTS(
               SELECT 1 FROM provider_connections
               WHERE id = ?1 AND archived_at IS NULL
             )",
            connection_id.as_str(),
        )? {
            return Err(not_found("provider connection"));
        }
        let existing_routes = load_model_routes_for_reconciliation(&transaction, connection_id)?;
        if existing_routes.iter().any(|route| {
            route
                .last_seen_at
                .is_some_and(|last_seen_at| last_seen_at > observed_at)
        }) {
            return Err(CoreError::invalid(
                "model reconciliation observation is older than stored model data",
            ));
        }
        let mut listed_ids = std::collections::BTreeSet::new();
        for route in listed_routes {
            if route.connection_id.as_str() != connection_id.as_str() {
                return Err(CoreError::invalid(
                    "every reconciled model route must belong to the requested connection",
                ));
            }
            if !listed_ids.insert(route.id.as_str()) {
                return Err(CoreError::invalid(
                    "reconciled model route identifiers must be unique",
                ));
            }
            let mut seen = route.clone();
            // This legacy wrapper represents a successful list response and
            // therefore normalizes every returned route to available. The
            // durable model-sync path preserves richer reviewed availability.
            seen.status = ModelAvailability::Available;
            seen.miss_count = 0;
            seen.last_seen_at = Some(observed_at);
            seen.first_seen_at = existing_routes
                .iter()
                .find(|existing| existing.id == route.id)
                .map_or(observed_at, |existing| existing.first_seen_at);
            upsert_model_route_row(&transaction, &seen)?;
        }
        for existing in existing_routes {
            if !listed_ids.contains(existing.id.as_str()) {
                transaction
                    .execute(
                        "UPDATE provider_models
                         SET miss_count = MIN(miss_count + 1, 4294967295),
                             availability = CASE
                               WHEN availability IN (
                                 'documented_only', 'access_denied', 'deprecated', 'retired'
                               ) THEN availability
                               ELSE 'missing_temporarily'
                             END
                         WHERE id = ?1 AND connection_id = ?2",
                        params![existing.id.as_str(), connection_id.as_str()],
                    )
                    .map_err(storage_db_error)?;
            }
        }
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Publishes an entire model refresh as one transaction so routes, initial
    /// presets, and connection status can never be observed half-applied.
    #[allow(
        clippy::too_many_lines,
        reason = "the refresh graph and its authoritative observation snapshot share one transaction"
    )]
    pub fn commit_model_refresh(
        &self,
        expected_connection: &ProviderConnection,
        refreshed_connection: &ProviderConnection,
        listed_routes: &[ModelRoute],
        new_presets: &[GenerationPreset],
        capability_observations: &[CapabilityObservation],
        observed_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        if expected_connection.id != refreshed_connection.id {
            return Err(CoreError::invalid(
                "model refresh connection identity cannot change",
            ));
        }
        let stored_connection = transaction
            .query_row(
                "SELECT id, template_id, template_version, display_name, api_origin,
                        config_json, credential_ref, credential_scope_json, timeout_seconds,
                        status, created_at, updated_at
                 FROM provider_connections
                 WHERE id = ?1 AND archived_at IS NULL",
                [expected_connection.id.as_str()],
                provider_connection_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("provider connection"))
            .and_then(decode_provider_connection_row)?;
        if stored_connection != *expected_connection {
            return Err(CoreError::invalid(
                "provider connection changed while its model list was refreshing",
            ));
        }
        let existing_routes =
            load_model_routes_for_reconciliation(&transaction, &refreshed_connection.id)?;
        if existing_routes.iter().any(|route| {
            route
                .last_seen_at
                .is_some_and(|last_seen_at| last_seen_at > observed_at)
        }) {
            return Err(CoreError::invalid(
                "model reconciliation observation is older than stored model data",
            ));
        }

        let mut listed_ids = std::collections::BTreeSet::new();
        for route in listed_routes {
            if route.connection_id != refreshed_connection.id {
                return Err(CoreError::invalid(
                    "every reconciled model route must belong to the requested connection",
                ));
            }
            if !listed_ids.insert(route.id.as_str()) {
                return Err(CoreError::invalid(
                    "reconciled model route identifiers must be unique",
                ));
            }
            let mut seen = route.clone();
            seen.miss_count = 0;
            seen.last_seen_at = Some(observed_at);
            seen.first_seen_at = existing_routes
                .iter()
                .find(|existing| existing.id == route.id)
                .map_or(observed_at, |existing| existing.first_seen_at);
            upsert_model_route_row(&transaction, &seen)?;
        }
        validate_provider_api_snapshot_observations_for_routes(
            capability_observations,
            &listed_ids,
            observed_at,
        )?;
        for existing in existing_routes {
            if !listed_ids.contains(existing.id.as_str()) {
                transaction
                    .execute(
                        "UPDATE provider_models
                         SET miss_count = MIN(miss_count + 1, 4294967295),
                             availability = CASE
                               WHEN availability IN (
                                 'documented_only', 'access_denied', 'deprecated', 'retired'
                               ) THEN availability
                               ELSE 'missing_temporarily'
                             END
                         WHERE id = ?1 AND connection_id = ?2",
                        params![existing.id.as_str(), refreshed_connection.id.as_str()],
                    )
                    .map_err(storage_db_error)?;
            }
        }
        for preset in new_presets {
            upsert_generation_preset_row(&transaction, preset)?;
        }
        for listed_id in &listed_ids {
            transaction
                .execute(
                    "DELETE FROM model_capability_observations
                     WHERE model_route_id = ?1 AND source_kind = 'provider_api'",
                    [*listed_id],
                )
                .map_err(storage_db_error)?;
        }
        for observation in capability_observations {
            if !listed_ids.contains(observation.model_route_id.as_str()) {
                return Err(CoreError::invalid(
                    "model refresh capability observations must belong to a listed route",
                ));
            }
            upsert_capability_observation_row(&transaction, observation)?;
        }
        upsert_provider_connection_row(&transaction, refreshed_connection)?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn delete_model_route(&self, id: &ModelRouteId) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let connection_id = transaction
            .query_row(
                "SELECT connection_id FROM provider_models WHERE id = ?1",
                [id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("model route"))?;
        if current_migrated_legacy_route_exists(&transaction, id.as_str())? {
            return Err(CoreError::invalid(
                "delete the migrated legacy provider connection instead of its current model route",
            ));
        }
        clear_provider_selections_for_route(&transaction, id.as_str(), &connection_id)?;
        transaction
            .execute(
                "DELETE FROM generation_presets WHERE model_route_id = ?1",
                [id.as_str()],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute("DELETE FROM provider_models WHERE id = ?1", [id.as_str()])
            .map_err(storage_db_error)?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Stores one source-attributed capability observation.
    ///
    /// Observation identity, route, key, and source are immutable. Reusing an
    /// ID is idempotent for an identical value and may otherwise only advance
    /// its observation timestamp. This lets provider refreshes keep a stable
    /// per-source ID without allowing provenance to be rewritten.
    pub fn upsert_capability_observation(
        &self,
        observation: &CapabilityObservation,
    ) -> CoreResult<()> {
        self.upsert_capability_observations(std::slice::from_ref(observation))
    }

    /// Atomically stores a bounded set of capability observations.
    pub fn upsert_capability_observations(
        &self,
        observations: &[CapabilityObservation],
    ) -> CoreResult<()> {
        if observations.len() > 1_024 {
            return Err(CoreError::invalid(
                "at most 1024 capability observations may be stored at once",
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let mut ids = std::collections::BTreeSet::new();
        for observation in observations {
            if !ids.insert(observation.id.as_str()) {
                return Err(CoreError::invalid(
                    "capability observation identifiers must be unique within one write",
                ));
            }
            upsert_capability_observation_row(&transaction, observation)?;
        }
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn get_capability_observation(
        &self,
        id: &ObservationId,
    ) -> CoreResult<CapabilityObservation> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, model_route_id, capability_key, value_json, support_status,
                        source_kind, confidence, evidence_ref, observed_at, expires_at
                 FROM model_capability_observations
                 WHERE id = ?1",
                [id.as_str()],
                capability_observation_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("capability observation"))?;
        decode_capability_observation_row(row)
    }

    pub fn list_capability_observations(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        self.list_capability_observations_filtered(model_route_id, None)
    }

    /// Returns every candidate needed to compute the effective value for one
    /// route/key pair, including expired observations so callers can expose
    /// stale state instead of silently treating it as current.
    pub fn list_capability_observations_for_key(
        &self,
        model_route_id: &ModelRouteId,
        key: CapabilityKey,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        self.list_capability_observations_filtered(model_route_id, Some(key))
    }

    fn list_capability_observations_filtered(
        &self,
        model_route_id: &ModelRouteId,
        key: Option<CapabilityKey>,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        // Distinguish an empty observation list from a route that does not
        // exist. Otherwise native clients could display a missing route as
        // merely having unknown capabilities.
        self.get_model_route(model_route_id)?;
        let connection = self.connection()?;
        let rows = if let Some(key) = key {
            let mut statement = connection
                .prepare(
                    "SELECT id, model_route_id, capability_key, value_json,
                                support_status, source_kind, confidence, evidence_ref,
                                observed_at, expires_at
                         FROM model_capability_observations
                         WHERE model_route_id = ?1 AND capability_key = ?2
                         ORDER BY observed_at DESC, id",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![model_route_id.as_str(), capability_key_to_str(key)],
                    capability_observation_columns,
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        } else {
            let mut statement = connection
                .prepare(
                    "SELECT id, model_route_id, capability_key, value_json,
                                support_status, source_kind, confidence, evidence_ref,
                                observed_at, expires_at
                         FROM model_capability_observations
                         WHERE model_route_id = ?1
                         ORDER BY capability_key, observed_at DESC, id",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([model_route_id.as_str()], capability_observation_columns)
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        rows.into_iter()
            .map(decode_capability_observation_row)
            .collect()
    }

    pub fn delete_capability_observation(&self, id: &ObservationId) -> CoreResult<()> {
        let deleted = self
            .connection()?
            .execute(
                "DELETE FROM model_capability_observations WHERE id = ?1",
                [id.as_str()],
            )
            .map_err(storage_db_error)?;
        if deleted == 0 {
            Err(not_found("capability observation"))
        } else {
            Ok(())
        }
    }

    pub fn list_generation_presets(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<GenerationPreset>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, model_route_id, display_name, values_json, created_at, updated_at
                 FROM generation_presets WHERE model_route_id = ?1
                 ORDER BY display_name COLLATE NOCASE, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([model_route_id.as_str()], generation_preset_columns)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter().map(decode_generation_preset_row).collect()
    }

    pub fn get_generation_preset(&self, id: &GenerationPresetId) -> CoreResult<GenerationPreset> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, model_route_id, display_name, values_json, created_at, updated_at
                 FROM generation_presets WHERE id = ?1",
                [id.as_str()],
                generation_preset_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation preset"))?;
        decode_generation_preset_row(row)
    }

    pub fn save_generation_preset(&self, preset: &GenerationPreset) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let connection_id = transaction
            .query_row(
                "SELECT connection_id FROM provider_models WHERE id = ?1",
                [preset.model_route_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("model route"))?;
        if active_retained_legacy_profile_exists(&transaction, &connection_id)? {
            return Err(CoreError::invalid(
                "migrated legacy generation presets are managed through their retained provider profile",
            ));
        }
        validate_generation_preset(&transaction, preset)?;
        upsert_generation_preset_row(&transaction, preset)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn delete_generation_preset(&self, id: &GenerationPresetId) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let route_id = transaction
            .query_row(
                "SELECT model_route_id FROM generation_presets WHERE id = ?1",
                [id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation preset"))?;
        if id.as_str() == route_id && current_migrated_legacy_route_exists(&transaction, &route_id)?
        {
            return Err(CoreError::invalid(
                "the migrated legacy current preset cannot be deleted independently",
            ));
        }
        clear_provider_selections_for_preset(&transaction, id.as_str(), &route_id)?;
        transaction
            .execute(
                "DELETE FROM generation_presets WHERE id = ?1",
                [id.as_str()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn stats(&self) -> CoreResult<DatabaseStats> {
        let connection = self.connection()?;
        Ok(DatabaseStats {
            characters: count(&connection, "characters")?,
            conversations: count(&connection, "conversations")?,
            messages: count(&connection, "messages")?,
            pending_imports: count(&connection, "import_jobs")?,
        })
    }

    pub(crate) fn connection(&self) -> CoreResult<DatabaseConnectionGuard<'_>> {
        self.connection_metrics.acquire(&self.connection)
    }

    pub(super) fn cas_mutation(&self) -> CoreResult<MutexGuard<'_, ()>> {
        self.cas_mutation.lock().map_err(|_| {
            CoreError::new(
                CoreErrorCode::StorageUnavailable,
                "CAS mutation lock was poisoned",
                true,
            )
        })
    }
}

fn insert_conversation_start_rows(
    transaction: &rusqlite::Transaction<'_>,
    conversation: &Conversation,
    branch: &ConversationBranch,
    state: &ConversationState,
    active_revision_id: Option<&str>,
    selected_greeting: Option<&(String, String)>,
    initial_message: Option<&Message>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO conversations
             (id, character_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                conversation.id.0,
                conversation.character_id,
                conversation.title,
                conversation.created_at.to_rfc3339(),
                conversation.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO conversation_greeting_bindings
             (conversation_id, character_content_revision_id, greeting_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                conversation.id.0,
                active_revision_id,
                selected_greeting.map(|(greeting_id, _)| greeting_id.as_str()),
                conversation.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    if let Some(initial_message) = initial_message {
        insert_message(transaction, initial_message)?;
    }
    transaction
        .execute(
            "INSERT INTO conversation_branches
             (id, conversation_id, title, fork_message_id, head_message_id,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
            params![
                branch.id.0,
                branch.conversation_id.0,
                branch.title,
                branch
                    .head_message_id
                    .as_ref()
                    .map(|message_id| message_id.0.as_str()),
                branch.created_at.to_rfc3339(),
                branch.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO conversation_state
             (conversation_id, active_branch_id, selected_mode, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                state.conversation_id.0,
                state.active_branch_id.0,
                mode_to_str(state.selected_mode),
                state.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    crate::lifecycle_outbox::insert_occurrence(
        transaction,
        &crate::lifecycle_outbox::LifecycleOccurrenceWrite {
            occurrence_id: format!("conversation-started:{}", conversation.id.0),
            event_kind: crate::lifecycle_outbox::LifecycleOccurrenceKind::ConversationStarted,
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
            exact_head_message_id: branch.head_message_id.clone(),
            owner_message_id: None,
            generation_id: None,
            occurred_at: conversation.created_at,
        },
        false,
    )?;
    Ok(())
}

fn require_generation_append_branch(
    transaction: &rusqlite::Transaction<'_>,
    observation: &GenerationAppendObservation<'_>,
) -> CoreResult<()> {
    let stored = transaction
        .query_row(
            "SELECT conversation_id, head_message_id
             FROM conversation_branches
             WHERE id = ?1",
            [&observation.branch_id.0],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found",
                false,
            )
        })?;
    if stored.0 != observation.user.conversation_id.0
        || stored.1.as_deref()
            != observation
                .expected_head
                .map(|message_id| message_id.0.as_str())
    {
        return Err(stale_branch_error());
    }
    let Some(head_id) = observation.expected_head else {
        return Ok(());
    };
    let pending = transaction
        .query_row(
            "SELECT status = 'pending'
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            params![observation.user.conversation_id.0, head_id.0],
            |row| row.get::<_, bool>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "expected branch head was not found",
                false,
            )
        })?;
    if pending {
        Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "cannot append while the branch head is still generating",
            true,
        ))
    } else {
        Ok(())
    }
}

fn prepare_generation_append_attempt(
    transaction: &rusqlite::Transaction<'_>,
    observation: &GenerationAppendObservation<'_>,
) -> CoreResult<Option<PreparedGenerationAppendAttempt>> {
    if !observation.require_attempt {
        return Ok(None);
    }
    let prompt_plan = observation
        .prompt_plan
        .map(|value| value.0)
        .ok_or_else(|| CoreError::invalid("generation attempt requires a prompt plan"))?;
    let module_plan_sha256 = generation_prompt_module_plan_sha256(prompt_plan)?;
    let prompt_plan_sha256 =
        Sha256Digest::parse(prompt_plan.plan_sha256.clone()).map_err(CoreError::invalid)?;
    let input_fingerprint_sha256 =
        Sha256Digest::parse(prompt_plan.input_fingerprint_sha256.clone())
            .map_err(CoreError::invalid)?;
    let attempt = crate::generation_attempt::require_dispatch_ready_attempt(
        transaction,
        &observation.generation.id,
        &observation.user.conversation_id,
        observation.branch_id,
        observation.branch_id,
        observation.expected_head,
        &module_plan_sha256,
        &prompt_plan_sha256,
        &input_fingerprint_sha256,
    )?;
    crate::interaction_repository::require_generation_attempt_prompt_context_authority_transaction(
        transaction,
        &attempt,
        prompt_plan,
    )?;
    let state_id = transaction
        .query_row(
            "SELECT id
             FROM interaction_state
             WHERE conversation_id = ?1 AND branch_id = ?2",
            params![
                observation.user.conversation_id.0.as_str(),
                observation.branch_id.0.as_str()
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "generation append interaction state was not found",
                false,
            )
        })?;
    Ok(Some(PreparedGenerationAppendAttempt {
        attempt,
        target_key: InteractionStateKey {
            state_id,
            conversation_id: observation.user.conversation_id.clone(),
            branch_id: observation.branch_id.clone(),
        },
    }))
}

fn materialize_generation_append_attempt(
    storage: &Storage,
    transaction: &rusqlite::Transaction<'_>,
    observation: &GenerationAppendObservation<'_>,
    prepared: &PreparedGenerationAppendAttempt,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    let final_prompt_plan = observation
        .prompt_plan
        .map(|value| value.0)
        .ok_or_else(|| {
            CoreError::invalid("generation interaction materialization requires a prompt plan")
        })?;
    materialize_and_validate_generation_attempt(
        storage,
        transaction,
        &prepared.attempt,
        &prepared.target_key,
        final_prompt_plan,
        occurred_at,
    )
}

fn materialize_and_validate_generation_attempt(
    storage: &Storage,
    transaction: &rusqlite::Transaction<'_>,
    attempt: &crate::generation_attempt::StoredGenerationAttempt,
    target_key: &InteractionStateKey,
    prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    let receipt = materialize_generation_attempt_interaction_for_append(
        storage,
        transaction,
        attempt,
        target_key,
        prompt_plan,
        occurred_at,
    )?;
    let seal = attempt.dispatch_seal.as_ref().ok_or_else(|| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "dispatch-ready generation attempt is missing its seal",
            false,
        )
    })?;
    if receipt.final_state_revision == seal.final_interaction_state_revision
        && receipt.final_state_snapshot_sha256 == seal.final_interaction_state_sha256
    {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation interaction materialization receipt differs from its dispatch seal",
            false,
        ))
    }
}

fn write_generation_append(
    transaction: &rusqlite::Transaction<'_>,
    observation: &GenerationAppendObservation<'_>,
    prepared: Option<&PreparedGenerationAppendAttempt>,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    insert_message(transaction, observation.user)?;
    insert_message(transaction, observation.assistant)?;
    let prompt_plan_link = observation
        .prompt_plan
        .map(|(record, logs)| {
            crate::orchestration::write_generation_prompt_plan(transaction, record, logs)
        })
        .transpose()?;
    insert_generation(
        transaction,
        observation.generation,
        prompt_plan_link.as_ref(),
    )?;
    if let Some(prepared) = prepared {
        crate::generation_attempt::mark_attempt_running_in_transaction(
            transaction,
            &prepared.attempt,
            occurred_at,
        )?;
    }
    let now = occurred_at.to_rfc3339();
    let changed = transaction
        .execute(
            "UPDATE conversation_branches
             SET head_message_id = ?3, updated_at = ?4
             WHERE id = ?1
               AND conversation_id = ?2
               AND ((head_message_id IS NULL AND ?5 IS NULL) OR head_message_id = ?5)",
            params![
                observation.branch_id.0,
                observation.user.conversation_id.0,
                observation.assistant.id.0,
                now,
                observation
                    .expected_head
                    .map(|message_id| message_id.0.as_str())
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(stale_branch_error());
    }
    transaction
        .execute(
            "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
            params![observation.user.conversation_id.0, now],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_message_action_append(
    observation: &MessageActionAppendObservation<'_>,
) -> CoreResult<()> {
    validate_generation_append(
        &observation.branch.id,
        observation.branch.fork_message_id.as_ref(),
        observation.user,
        observation.assistant,
        observation.generation,
    )?;
    validate_generation_prompt_plan_link(
        &observation.branch.id,
        observation.branch.fork_message_id.as_ref(),
        observation.user,
        observation.generation,
        observation.prompt_plan.map(|value| value.0),
    )?;
    if observation.branch.conversation_id != observation.user.conversation_id
        || observation.branch.head_message_id.as_ref() != Some(&observation.assistant.id)
        || observation.branch.fork_message_id != observation.user.parent_id
    {
        return Err(CoreError::invalid(
            "message action branch does not own the appended generation",
        ));
    }
    match (
        observation.require_attempt,
        observation.target_interaction_state_key,
    ) {
        (true, Some(key))
            if key.conversation_id == observation.branch.conversation_id
                && key.branch_id == observation.branch.id
                && !key.state_id.trim().is_empty() =>
        {
            Ok(())
        }
        (true, _) => Err(CoreError::invalid(
            "attempt-bound message action requires its exact target interaction state key",
        )),
        (false, None) => Ok(()),
        (false, Some(_)) => Err(CoreError::invalid(
            "legacy message action cannot materialize a generation interaction attempt",
        )),
    }
}

fn require_message_action_context(
    transaction: &rusqlite::Transaction<'_>,
    observation: &MessageActionAppendObservation<'_>,
) -> CoreResult<MessageGenerationActionContext> {
    let context = load_message_generation_action_context(
        transaction,
        &observation.user.conversation_id,
        observation.source_branch_id,
        observation.expected_source_head,
        observation.target_message_id,
        observation.action,
    )?;
    if context.fork_message_id != observation.branch.fork_message_id
        || (observation.action == MessageGenerationAction::RegenerateAssistant
            && context.user_text != observation.user.content)
    {
        Err(stale_branch_error())
    } else {
        Ok(context)
    }
}

fn prepare_message_action_attempt(
    transaction: &rusqlite::Transaction<'_>,
    observation: &MessageActionAppendObservation<'_>,
) -> CoreResult<Option<crate::generation_attempt::StoredGenerationAttempt>> {
    if !observation.require_attempt {
        return Ok(None);
    }
    let prompt_plan = observation
        .prompt_plan
        .map(|value| value.0)
        .ok_or_else(|| CoreError::invalid("generation attempt requires a prompt plan"))?;
    let module_plan_sha256 = generation_prompt_module_plan_sha256(prompt_plan)?;
    let prompt_plan_sha256 =
        Sha256Digest::parse(prompt_plan.plan_sha256.clone()).map_err(CoreError::invalid)?;
    let input_fingerprint_sha256 =
        Sha256Digest::parse(prompt_plan.input_fingerprint_sha256.clone())
            .map_err(CoreError::invalid)?;
    let attempt = crate::generation_attempt::require_dispatch_ready_attempt(
        transaction,
        &observation.generation.id,
        &observation.user.conversation_id,
        observation.source_branch_id,
        &observation.branch.id,
        observation.expected_source_head,
        &module_plan_sha256,
        &prompt_plan_sha256,
        &input_fingerprint_sha256,
    )?;
    crate::interaction_repository::require_generation_attempt_prompt_context_authority_transaction(
        transaction,
        &attempt,
        prompt_plan,
    )?;
    Ok(Some(attempt))
}

fn write_message_action_append(
    storage: &Storage,
    transaction: &rusqlite::Transaction<'_>,
    observation: &MessageActionAppendObservation<'_>,
    dispatch_attempt: Option<&crate::generation_attempt::StoredGenerationAttempt>,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    insert_message(transaction, observation.user)?;
    insert_message(transaction, observation.assistant)?;
    insert_message_action_branch(transaction, observation.branch)?;
    if let (Some(attempt), Some(target_key)) =
        (dispatch_attempt, observation.target_interaction_state_key)
    {
        let prompt_plan = observation
            .prompt_plan
            .map(|value| value.0)
            .ok_or_else(|| {
                CoreError::invalid("generation interaction materialization requires a prompt plan")
            })?;
        materialize_and_validate_generation_attempt(
            storage,
            transaction,
            attempt,
            target_key,
            prompt_plan,
            occurred_at,
        )?;
    }
    let prompt_plan_link = observation
        .prompt_plan
        .map(|(record, logs)| {
            crate::orchestration::write_generation_prompt_plan(transaction, record, logs)
        })
        .transpose()?;
    insert_generation(
        transaction,
        observation.generation,
        prompt_plan_link.as_ref(),
    )?;
    if let Some(attempt) = dispatch_attempt {
        crate::generation_attempt::mark_attempt_running_in_transaction(
            transaction,
            attempt,
            occurred_at,
        )?;
    }
    activate_message_action_branch(transaction, observation, occurred_at)
}

fn insert_message_action_branch(
    transaction: &rusqlite::Transaction<'_>,
    branch: &ConversationBranch,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO conversation_branches
             (id, conversation_id, title, fork_message_id, head_message_id,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                branch.id.0,
                branch.conversation_id.0,
                branch.title,
                branch
                    .fork_message_id
                    .as_ref()
                    .map(|message_id| message_id.0.as_str()),
                branch
                    .head_message_id
                    .as_ref()
                    .map(|message_id| message_id.0.as_str()),
                branch.created_at.to_rfc3339(),
                branch.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn activate_message_action_branch(
    transaction: &rusqlite::Transaction<'_>,
    observation: &MessageActionAppendObservation<'_>,
    occurred_at: DateTime<Utc>,
) -> CoreResult<()> {
    let now = occurred_at.to_rfc3339();
    let changed = transaction
        .execute(
            "UPDATE conversation_state
             SET active_branch_id = ?3, updated_at = ?4
             WHERE conversation_id = ?1 AND active_branch_id = ?2",
            params![
                observation.user.conversation_id.0,
                observation.source_branch_id.0,
                observation.branch.id.0,
                now
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(stale_branch_error());
    }
    transaction
        .execute(
            "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
            params![observation.user.conversation_id.0, now],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn require_removal_parent_not_pending(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &ConversationId,
    parent_id: Option<&MessageId>,
) -> CoreResult<()> {
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    let status = transaction
        .query_row(
            "SELECT status
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0, parent_id.0],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "message action parent was not found",
                false,
            )
        })?;
    if str_to_status(&status, 0).map_err(storage_db_error)? == MessageStatus::Pending {
        Err(active_generation_action_error())
    } else {
        Ok(())
    }
}

pub(crate) fn write_discovered_provider_graph_rows(
    transaction: &rusqlite::Transaction<'_>,
    template: &ProviderTemplate,
    connection: &ProviderConnection,
    routes: &[ModelRoute],
    observations: &[CapabilityObservation],
    presets: &[GenerationPreset],
) -> CoreResult<()> {
    save_provider_template_row(transaction, template)?;
    upsert_provider_connection_row(transaction, connection)?;
    for route in routes {
        upsert_model_route_row(transaction, route)?;
    }
    for observation in observations {
        upsert_capability_observation_row(transaction, observation)?;
    }
    for preset in presets {
        upsert_generation_preset_row(transaction, preset)?;
    }
    validate_provider_catalog_foreign_keys(transaction)
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct StoredDiscoveredProviderGraphRows {
    pub template: ProviderTemplate,
    pub connection: ProviderConnection,
    pub routes: Vec<ModelRoute>,
    pub observations: Vec<CapabilityObservation>,
    pub presets: Vec<GenerationPreset>,
}

pub(crate) fn load_discovered_provider_graph_rows(
    transaction: &Connection,
    template_id: &ProviderTemplateId,
    template_version: u32,
    connection_id: &ProviderConnectionId,
) -> CoreResult<Option<StoredDiscoveredProviderGraphRows>> {
    let connection_row = transaction
        .query_row(
            "SELECT id, template_id, template_version, display_name, api_origin,
                    config_json, credential_ref, credential_scope_json, timeout_seconds,
                    status, created_at, updated_at
             FROM provider_connections
             WHERE id = ?1",
            [connection_id.as_str()],
            provider_connection_columns,
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some(connection_row) = connection_row else {
        return Ok(None);
    };
    let connection = decode_provider_connection_row(connection_row)?;
    if connection.template_id != *template_id || connection.template_version != template_version {
        return Err(CoreError::invalid(
            "stored discovered connection does not match its commit template",
        ));
    }
    let template_row = transaction
        .query_row(
            "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
             FROM provider_templates
             WHERE id = ?1 AND version = ?2",
            params![template_id.as_str(), template_version],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("discovered provider template is missing"))?;
    let template = decode_provider_template_row(template_row)?;
    let routes = load_model_routes_for_reconciliation(transaction, connection_id)?;
    let observations = {
        let mut statement = transaction
            .prepare(
                "SELECT observation.id, observation.model_route_id,
                        observation.capability_key, observation.value_json,
                        observation.support_status, observation.source_kind,
                        observation.confidence, observation.evidence_ref,
                        observation.observed_at, observation.expires_at
                 FROM model_capability_observations AS observation
                 JOIN provider_models AS route
                   ON route.id = observation.model_route_id
                 WHERE route.connection_id = ?1
                 ORDER BY observation.id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([connection_id.as_str()], capability_observation_columns)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
            .into_iter()
            .map(decode_capability_observation_row)
            .collect::<CoreResult<Vec<_>>>()?
    };
    let presets = {
        let mut statement = transaction
            .prepare(
                "SELECT preset.id, preset.model_route_id, preset.display_name,
                        preset.values_json, preset.created_at, preset.updated_at
                 FROM generation_presets AS preset
                 JOIN provider_models AS route
                   ON route.id = preset.model_route_id
                 WHERE route.connection_id = ?1
                 ORDER BY preset.id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([connection_id.as_str()], generation_preset_columns)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
            .into_iter()
            .map(decode_generation_preset_row)
            .collect::<CoreResult<Vec<_>>>()?
    };
    Ok(Some(StoredDiscoveredProviderGraphRows {
        template,
        connection,
        routes,
        observations,
        presets,
    }))
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

fn validate_package_promotion_import_id(import_id: &str) -> CoreResult<()> {
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

fn prepare_package_asset_promotion_intents(
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

fn validate_package_cas_promotion_intent(intent: &PackageCasPromotionIntent) -> CoreResult<()> {
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

fn ensure_package_cas_promotion_intents(
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

fn mark_package_cas_file_durable(
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

fn mark_package_cas_row_registered(
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

fn package_cas_product_reference_exists(
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

fn cleanup_package_cas_promotion(
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

fn remove_package_cas_file(root: &Path, intent: &PackageCasPromotionIntent) -> CoreResult<bool> {
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

#[allow(clippy::too_many_lines)]
pub(crate) fn apply_migrations(connection: &mut Connection) -> CoreResult<()> {
    crate::cutover::register_integrity_functions(connection)?;
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
            .execute_batch(MIGRATION_0001)
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
            .execute_batch(MIGRATION_0002)
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
            .execute_batch(MIGRATION_0003)
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
            .execute_batch(MIGRATION_0004)
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
            .execute_batch(MIGRATION_0005)
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
            .execute_batch(MIGRATION_0006)
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
            .execute_batch(MIGRATION_0007)
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
            .execute_batch(MIGRATION_0008)
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
            .execute_batch(MIGRATION_0009)
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
            .execute_batch(MIGRATION_0010)
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
            .execute_batch(MIGRATION_0011)
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
        MIGRATION_0012,
        "content-package foundation",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        13,
        MIGRATION_0013,
        "prompt pipeline",
    )?;
    apply_checked_migration(connection, current_version, 14, MIGRATION_0014, "knowledge")?;
    apply_checked_migration(connection, current_version, 15, MIGRATION_0015, "memory")?;
    apply_checked_migration(
        connection,
        current_version,
        16,
        MIGRATION_0016,
        "transforms",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        17,
        MIGRATION_0017,
        "interactions and content modules",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        18,
        MIGRATION_0018,
        "persona selection",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        19,
        MIGRATION_0019,
        "core lifecycle outbox",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        20,
        MIGRATION_0020,
        "package CAS promotion journal",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        21,
        MIGRATION_0021,
        "interaction message checkpoints",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        22,
        MIGRATION_0022,
        "memory vector-space identity",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        23,
        MIGRATION_0023,
        "applied module runtime plans",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        24,
        MIGRATION_0024,
        "generation attempt proposals",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        25,
        MIGRATION_0025,
        "conversation greeting bindings",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        26,
        MIGRATION_0026,
        "provider discovery native no-effect recovery",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        27,
        MIGRATION_0027,
        "provider discovery native no-effect attestations",
    )?;
    apply_generation_attempt_identity_migration(connection, current_version)?;
    apply_checked_migration(
        connection,
        current_version,
        29,
        MIGRATION_0029,
        "generation attempt decision handshake",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        30,
        MIGRATION_0030,
        "package document target reviews",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        31,
        MIGRATION_0031,
        "message display projections",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        32,
        MIGRATION_0032,
        "knowledge embedding vector spaces",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        33,
        MIGRATION_0033,
        "interaction derived-event outbox",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        34,
        MIGRATION_0034,
        "generation attempt derived-event authority",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        35,
        MIGRATION_0035,
        "interaction derived-event terminal quarantine",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        36,
        MIGRATION_0036,
        "generation attempt derived closure and evaluation seal",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        37,
        MIGRATION_0037,
        "provider credential operation journal",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        38,
        MIGRATION_0038,
        "conversation speaker roster and message attribution",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        39,
        MIGRATION_0039,
        "portable runtime model audit",
    )?;
    apply_checked_migration(
        connection,
        current_version,
        40,
        MIGRATION_0040,
        "portable runtime state",
    )?;
    read_current_schema_version(connection)?;
    Ok(())
}

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

fn apply_generation_attempt_identity_migration(
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

pub(crate) type ProviderConnectionRow = (
    String,
    String,
    i64,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    String,
    String,
    String,
);

pub(crate) type ModelRouteRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

type CapabilityObservationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    Option<String>,
);

type GenerationPresetRow = (String, String, String, String, String, String);

fn migrate_legacy_provider_catalog(transaction: &rusqlite::Transaction<'_>) -> CoreResult<()> {
    for table in [
        "provider_templates",
        "provider_connections",
        "provider_models",
        "model_capability_observations",
        "generation_presets",
        "provider_discovery_sessions",
        "provider_discovery_evidence",
    ] {
        if count(transaction, table)? != 0 {
            return Err(storage_corrupted(format!(
                "provider catalog migration found pre-existing rows in {table}"
            )));
        }
    }

    insert_legacy_provider_template(transaction)?;
    let profiles = {
        let mut statement = transaction
            .prepare(
                "SELECT id, display_name, base_url, model, timeout_seconds
                 FROM provider_profiles ORDER BY id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let migrated_at = Utc::now();
    for (id, display_name, base_url, model, timeout_seconds) in profiles {
        let timeout_seconds = u32::try_from(timeout_seconds).map_err(|_| {
            storage_corrupted("legacy provider timeout is outside the supported range")
        })?;
        let profile = ProviderProfile {
            id,
            display_name,
            base_url,
            model,
            timeout_seconds,
        };
        let (connection, route, preset) =
            legacy_provider_graph(&profile, migrated_at).map_err(provider_migration_error)?;
        insert_provider_connection_during_v4_migration(transaction, &connection)
            .map_err(provider_migration_error)?;
        insert_model_route_during_v4_migration(transaction, &route)
            .map_err(provider_migration_error)?;
        insert_generation_preset_during_v4_migration(transaction, &preset)
            .map_err(provider_migration_error)?;
    }

    if let Some(settings_json) = transaction
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
    {
        let settings = serde_json::from_str::<AppSettings>(&settings_json).map_err(|error| {
            storage_corrupted(format!(
                "provider catalog migration found invalid settings: {error}"
            ))
        })?;
        let settings = normalize_settings_during_v4_migration(transaction, &settings)
            .map_err(provider_migration_error)?;
        let settings_json = serde_json::to_string(&settings).map_err(|error| {
            CoreError::internal(format!("cannot encode migrated settings: {error}"))
        })?;
        transaction
            .execute(
                "UPDATE app_settings SET value_json = ?1 WHERE key = 'application'",
                [settings_json],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

/// Migration 0004 runs before the tombstone and local-network approval
/// migrations, so it must not use the current-schema connection upsert.
fn insert_provider_connection_during_v4_migration(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    validate_provider_connection(transaction, connection)?;
    if connection.updated_at < connection.created_at {
        return Err(CoreError::invalid(
            "provider connection updated_at must not precede created_at",
        ));
    }
    let config_json = serde_json::to_string(&connection.config).map_err(|error| {
        CoreError::internal(format!("cannot encode provider connection config: {error}"))
    })?;
    let credential_scope_json = connection
        .credential_scope
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| CoreError::internal(format!("cannot encode credential scope: {error}")))?;
    transaction
        .execute(
            "INSERT INTO provider_connections
             (id, template_id, template_version, display_name, api_origin, config_json,
              credential_ref, credential_scope_json, timeout_seconds, status,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                connection.id.as_str(),
                connection.template_id.as_str(),
                connection.template_version,
                connection.display_name,
                connection.api_origin.as_str(),
                config_json,
                connection
                    .credential_ref
                    .as_ref()
                    .map(CredentialRef::as_str),
                credential_scope_json,
                connection.timeout_seconds,
                connection_status_to_str(connection.status),
                connection.created_at.to_rfc3339(),
                connection.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

/// Migration 0004 runs before the durable model-sync columns are introduced
/// by migration 0009, so it must write only the v4 route shape.
fn insert_model_route_during_v4_migration(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
) -> CoreResult<()> {
    validate_model_route_for_schema(transaction, route, false)?;
    if route
        .last_seen_at
        .is_some_and(|last_seen_at| last_seen_at < route.first_seen_at)
    {
        return Err(CoreError::invalid(
            "model route last_seen_at must not precede first_seen_at",
        ));
    }
    let route_json = serde_json::to_string(&route.route_config)
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    transaction
        .execute(
            "INSERT INTO provider_models
             (id, connection_id, api_family, model_id, display_name, route_json,
              availability, raw_metadata_json, first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                route.id.as_str(),
                route.connection_id.as_str(),
                api_family_to_str(route.api_family),
                route.model_id,
                route.display_name,
                route_json,
                model_availability_to_str(route.status),
                route.raw_metadata.as_ref().map(BoundedJson::as_str),
                route.first_seen_at.to_rfc3339(),
                route.last_seen_at.map(|value| value.to_rfc3339()),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_generation_preset_during_v4_migration(
    transaction: &rusqlite::Transaction<'_>,
    preset: &GenerationPreset,
) -> CoreResult<()> {
    validate_generation_preset_for_schema(transaction, preset, false)?;
    if preset.updated_at < preset.created_at {
        return Err(CoreError::invalid(
            "generation preset updated_at must not precede created_at",
        ));
    }
    let values_json = encode_generation_preset_values(preset)?;
    transaction
        .execute(
            "INSERT INTO generation_presets
             (id, model_route_id, display_name, values_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                preset.id.as_str(),
                preset.model_route_id.as_str(),
                preset.display_name,
                values_json,
                preset.created_at.to_rfc3339(),
                preset.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn provider_migration_error(error: CoreError) -> CoreError {
    if error.code == CoreErrorCode::StorageCorrupted {
        error
    } else {
        storage_corrupted(format!(
            "provider catalog migration rejected legacy data: {}",
            error.message
        ))
    }
}

fn validate_provider_catalog_migration(transaction: &rusqlite::Transaction<'_>) -> CoreResult<()> {
    let legacy_count = count(transaction, "provider_profiles")?;
    let connection_count = query_count(
        transaction,
        "SELECT COUNT(*) FROM provider_connections
         WHERE template_id = ?1 AND template_version = ?2",
        params![
            LEGACY_PROVIDER_TEMPLATE_ID,
            LEGACY_PROVIDER_TEMPLATE_VERSION
        ],
    )?;
    let route_count = query_count(
        transaction,
        "SELECT COUNT(*)
         FROM provider_models AS model
         JOIN provider_connections AS connection
           ON connection.id = model.connection_id
         WHERE connection.template_id = ?1
           AND connection.template_version = ?2",
        params![
            LEGACY_PROVIDER_TEMPLATE_ID,
            LEGACY_PROVIDER_TEMPLATE_VERSION
        ],
    )?;
    let preset_count = query_count(
        transaction,
        "SELECT COUNT(*)
         FROM generation_presets AS preset
         JOIN provider_models AS model ON model.id = preset.model_route_id
         JOIN provider_connections AS connection
           ON connection.id = model.connection_id
         WHERE connection.template_id = ?1
           AND connection.template_version = ?2",
        params![
            LEGACY_PROVIDER_TEMPLATE_ID,
            LEGACY_PROVIDER_TEMPLATE_VERSION
        ],
    )?;
    if connection_count != legacy_count
        || route_count != legacy_count
        || preset_count != legacy_count
    {
        return Err(storage_corrupted(format!(
            "provider catalog migration row-count mismatch: legacy={legacy_count}, \
             connections={connection_count}, routes={route_count}, presets={preset_count}"
        )));
    }
    let mismatched_ids = query_count(
        transaction,
        "SELECT COUNT(*)
         FROM provider_profiles AS legacy
         LEFT JOIN provider_connections AS connection
           ON connection.id = legacy.id
         LEFT JOIN provider_models AS model
           ON model.id = legacy.id AND model.connection_id = connection.id
         LEFT JOIN generation_presets AS preset
           ON preset.id = legacy.id AND preset.model_route_id = model.id
         WHERE connection.id IS NULL OR model.id IS NULL OR preset.id IS NULL",
        [],
    )?;
    if mismatched_ids != 0 {
        return Err(storage_corrupted(
            "provider catalog migration did not preserve legacy stable identifiers",
        ));
    }
    validate_provider_catalog_foreign_keys(transaction)
}

fn insert_legacy_provider_template(transaction: &rusqlite::Transaction<'_>) -> CoreResult<()> {
    let template = legacy_provider_template()?;
    save_provider_template_row(transaction, &template)
}

fn save_provider_template_row(
    transaction: &rusqlite::Transaction<'_>,
    template: &ProviderTemplate,
) -> CoreResult<()> {
    validate_provider_template(template)?;
    let manifest_json = serde_json::to_string(template).map_err(|error| {
        CoreError::internal(format!("cannot encode provider template: {error}"))
    })?;
    let manifest_sha256 = hex::encode(Sha256::digest(manifest_json.as_bytes()));
    if let Some(existing_row) = transaction
        .query_row(
            "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
             FROM provider_templates WHERE id = ?1 AND version = ?2",
            params![template.id.as_str(), template.manifest_version],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
    {
        let existing = decode_provider_template_row(existing_row)?;
        if existing == *template {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "provider template versions are immutable; save changes under a new version",
        ));
    }
    transaction
        .execute(
            "INSERT INTO provider_templates
             (id, version, display_name, source_kind, manifest_json, manifest_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                template.id.as_str(),
                template.manifest_version,
                template.display_name,
                template_source_to_str(template.source),
                manifest_json,
                manifest_sha256,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_provider_template(template: &ProviderTemplate) -> CoreResult<()> {
    validate_nonempty("provider template id", template.id.as_str())?;
    validate_nonempty("provider template display name", &template.display_name)?;
    if template.manifest_version == 0 || template.default_manifest.schema_version == 0 {
        return Err(CoreError::invalid(
            "provider template and manifest versions must be positive",
        ));
    }
    if template.api_family != template.default_manifest.api_family {
        return Err(CoreError::invalid(
            "provider template API family must match its manifest",
        ));
    }
    let mut connection_field_keys = std::collections::BTreeSet::new();
    for field in &template.connection_fields {
        validate_nonempty("provider connection field key", &field.key)?;
        validate_nonempty("provider connection field label", &field.label_key)?;
        if !connection_field_keys.insert(field.key.as_str()) {
            return Err(CoreError::invalid(
                "provider connection field keys must be unique",
            ));
        }
        if is_sensitive_configuration_key(&field.key)
            && field.value_type != ConnectionFieldType::Credential
        {
            return Err(CoreError::invalid(
                "secret-like provider connection fields must use the credential field type",
            ));
        }
    }
    let mut parameter_ids = std::collections::BTreeSet::new();
    for specification in &template.default_manifest.parameters {
        validate_nonempty("provider parameter id", specification.id.as_str())?;
        validate_nonempty("provider parameter label", &specification.label_key)?;
        validate_nonempty(
            "provider parameter mapping field",
            &specification.provider_mapping.field_name,
        )?;
        if !parameter_ids.insert(specification.id.as_str()) {
            return Err(CoreError::invalid(
                "provider manifest parameter identifiers must be unique",
            ));
        }
        if specification
            .minimum
            .is_some_and(|value| !value.is_finite())
            || specification
                .maximum
                .is_some_and(|value| !value.is_finite())
            || specification
                .step
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || matches!(
                (specification.minimum, specification.maximum),
                (Some(minimum), Some(maximum)) if minimum > maximum
            )
        {
            return Err(CoreError::invalid(
                "provider parameter numeric constraints are invalid",
            ));
        }
        for choice in &specification.allowed_values {
            validate_nonempty("provider parameter choice label", &choice.label_key)?;
            validate_parameter_value(
                specification,
                &ParameterValueState::Explicit(choice.value.clone()),
            )?;
        }
    }
    for source in &template.default_manifest.sources {
        if let Some(hash) = source.content_sha256.as_deref()
            && (hash.len() != 64
                || hash
                    .bytes()
                    .any(|value| !value.is_ascii_hexdigit() || value.is_ascii_uppercase()))
        {
            return Err(CoreError::invalid(
                "provider manifest source hash must be lowercase SHA-256 hex",
            ));
        }
    }
    Ok(())
}

fn legacy_provider_template() -> CoreResult<ProviderTemplate> {
    let temperature = ParameterSpec {
        id: ParameterId::from(TEMPERATURE_PARAMETER_ID),
        label_key: "provider.parameter.temperature".to_owned(),
        description_key: Some("provider.parameter.temperature.description".to_owned()),
        value_type: ParameterType::Number,
        allowed_values: Vec::new(),
        minimum: Some(0.0),
        maximum: Some(2.0),
        step: Some(0.1),
        default_mode: ParameterDefaultMode::ProviderDefault,
        visibility: None,
        conflicts: Vec::new(),
        provider_mapping: ProviderParameterMapping {
            target: ProviderParameterTarget::RequestBody,
            field_name: TEMPERATURE_PARAMETER_ID.to_owned(),
        },
        level: UiParameterLevel::Basic,
    };
    let max_output_tokens = ParameterSpec {
        id: ParameterId::from(MAX_OUTPUT_TOKENS_PARAMETER_ID),
        label_key: "provider.parameter.max_output_tokens".to_owned(),
        description_key: Some("provider.parameter.max_output_tokens.description".to_owned()),
        value_type: ParameterType::Integer,
        allowed_values: Vec::new(),
        minimum: Some(1.0),
        maximum: Some(f64::from(u32::MAX)),
        step: Some(1.0),
        default_mode: ParameterDefaultMode::ProviderDefault,
        visibility: None,
        conflicts: Vec::new(),
        provider_mapping: ProviderParameterMapping {
            target: ProviderParameterTarget::RequestBody,
            field_name: "max_tokens".to_owned(),
        },
        level: UiParameterLevel::Basic,
    };
    Ok(ProviderTemplate {
        id: ProviderTemplateId::from(LEGACY_PROVIDER_TEMPLATE_ID),
        display_name: "Custom OpenAI-compatible Chat".to_owned(),
        manifest_version: LEGACY_PROVIDER_TEMPLATE_VERSION,
        source: TemplateSource::BuiltIn,
        api_family: ApiFamily::OpenAiChatCompletions,
        connection_fields: vec![
            ConnectionFieldSpec {
                key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
                label_key: "provider.connection.api_base_url".to_owned(),
                description_key: Some("provider.connection.api_base_url.description".to_owned()),
                value_type: ConnectionFieldType::Text,
                required: true,
            },
            ConnectionFieldSpec {
                key: "api_key".to_owned(),
                label_key: "provider.connection.api_key".to_owned(),
                description_key: Some("provider.connection.api_key.description".to_owned()),
                value_type: ConnectionFieldType::Credential,
                required: false,
            },
        ],
        default_manifest: ProviderManifest {
            schema_version: 1,
            api_family: ApiFamily::OpenAiChatCompletions,
            sources: Vec::new(),
            default_api_origin: None,
            auth: AuthBinding::BearerHeader,
            endpoints: ManifestEndpoints {
                models: Some(EndpointSpec {
                    method: HttpMethod::Get,
                    path: endpoint_path("/models")?,
                }),
                generate: EndpointSpec {
                    method: HttpMethod::Post,
                    path: endpoint_path("/chat/completions")?,
                },
                embeddings: None,
            },
            decoders: ManifestDecoders {
                response: DecoderId::OpenAiJsonV1,
                streaming: Some(DecoderId::OpenAiSseV1),
            },
            parameters: vec![temperature, max_output_tokens],
        },
    })
}

fn endpoint_path(value: &str) -> CoreResult<EndpointPath> {
    EndpointPath::parse(value).map_err(|error| {
        CoreError::internal(format!("built-in provider endpoint is invalid: {error}"))
    })
}

fn legacy_provider_graph(
    profile: &ProviderProfile,
    timestamp: DateTime<Utc>,
) -> CoreResult<(ProviderConnection, ModelRoute, GenerationPreset)> {
    validate_legacy_provider_profile(profile)?;
    let api_origin = canonical_origin_for_legacy_base_url(&profile.base_url)?;
    let id = profile.id.as_str();
    let connection = ProviderConnection {
        id: ProviderConnectionId::from(id),
        template_id: ProviderTemplateId::from(LEGACY_PROVIDER_TEMPLATE_ID),
        template_version: LEGACY_PROVIDER_TEMPLATE_VERSION,
        display_name: profile.display_name.clone(),
        api_origin: api_origin.clone(),
        config: ConnectionConfig {
            api_base_path: legacy_api_base_path(&profile.base_url)?,
            network_mode: legacy_network_mode(&profile.base_url)?,
            local_network_approval: None,
            values: vec![ConnectionConfigEntry {
                key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
                value: ConnectionConfigValue::Text(profile.base_url.clone()),
            }],
        },
        credential_ref: Some(CredentialRef(profile.id.clone())),
        credential_scope: Some(CredentialScope {
            allowed_origins: vec![api_origin],
            auth_binding: AuthBinding::BearerHeader,
            redirect_policy: CredentialRedirectPolicy::Deny,
        }),
        timeout_seconds: profile.timeout_seconds,
        status: ConnectionStatus::Untested,
        created_at: timestamp,
        updated_at: timestamp,
    };
    let route = ModelRoute {
        id: ModelRouteId::from(id),
        connection_id: ProviderConnectionId::from(id),
        api_family: ApiFamily::OpenAiChatCompletions,
        model_id: profile.model.clone(),
        display_name: Some(profile.model.clone()),
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::Legacy,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: timestamp,
        last_seen_at: None,
    };
    let preset = GenerationPreset {
        id: GenerationPresetId::from(id),
        model_route_id: ModelRouteId::from(id),
        display_name: "Default".to_owned(),
        values: vec![
            ParameterValue {
                parameter_id: ParameterId::from(TEMPERATURE_PARAMETER_ID),
                state: ParameterValueState::Explicit(ParameterLiteral::Number(1.0)),
            },
            ParameterValue {
                parameter_id: ParameterId::from(MAX_OUTPUT_TOKENS_PARAMETER_ID),
                state: ParameterValueState::Explicit(ParameterLiteral::Integer(4096)),
            },
        ],
        reasoning: GenerationReasoningSettings {
            preserve_opaque_state: false,
            ..GenerationReasoningSettings::default()
        },
        prompt_cache: GenerationPromptCacheSettings::default(),
        created_at: timestamp,
        updated_at: timestamp,
    };
    Ok((connection, route, preset))
}

fn legacy_model_route_id(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
) -> CoreResult<ModelRouteId> {
    let route_json = serde_json::to_string(&route.route_config)
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    if let Some(existing_id) = transaction
        .query_row(
            "SELECT id FROM provider_models
             WHERE connection_id = ?1
               AND api_family = ?2
               AND model_id = ?3
               AND route_json = ?4
             ORDER BY first_seen_at, id
             LIMIT 1",
            params![
                route.connection_id.as_str(),
                api_family_to_str(route.api_family),
                route.model_id,
                route_json,
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
    {
        return Ok(ModelRouteId::from(existing_id));
    }
    if !row_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
        route.id.as_str(),
    )? {
        return Ok(route.id.clone());
    }
    let identity = format!(
        "lorepia:legacy-model-route:v1\u{0}{}\u{0}{}\u{0}{}",
        route.connection_id.as_str(),
        api_family_to_str(route.api_family),
        route.model_id,
    );
    Ok(ModelRouteId::from(
        Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string(),
    ))
}

fn validate_legacy_provider_profile(profile: &ProviderProfile) -> CoreResult<()> {
    validate_nonempty("provider profile id", &profile.id)?;
    validate_nonempty("provider display name", &profile.display_name)?;
    validate_nonempty("provider model", &profile.model)?;
    if !(1..=600).contains(&profile.timeout_seconds) {
        return Err(CoreError::invalid(
            "provider timeout must be from 1 to 600 seconds",
        ));
    }
    canonical_origin_for_legacy_base_url(&profile.base_url)?;
    Ok(())
}

fn canonical_origin_for_legacy_base_url(base_url: &str) -> CoreResult<CanonicalOrigin> {
    if base_url.trim() != base_url || base_url.is_empty() {
        return Err(CoreError::invalid(
            "provider base URL must be non-empty and contain no surrounding whitespace",
        ));
    }
    let url = Url::parse(base_url)
        .map_err(|error| CoreError::invalid(format!("invalid provider base URL: {error}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CoreError::invalid(
            "provider base URL must not contain embedded credentials",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(CoreError::invalid(
            "provider base URL must not contain a query or fragment",
        ));
    }
    match url.scheme() {
        "https" => {}
        "http" if url.host_str().is_some_and(is_loopback_host) => {}
        "http" => {
            return Err(CoreError::invalid(
                "unencrypted HTTP is allowed only for loopback provider URLs",
            ));
        }
        _ => {
            return Err(CoreError::invalid(
                "provider base URL must use HTTPS or loopback HTTP",
            ));
        }
    }
    CanonicalOrigin::parse(&url.origin().ascii_serialization())
        .map_err(|error| CoreError::invalid(format!("invalid provider API origin: {error}")))
}

fn legacy_api_base_path(base_url: &str) -> CoreResult<Option<EndpointPath>> {
    let url = Url::parse(base_url)
        .map_err(|error| CoreError::invalid(format!("invalid provider base URL: {error}")))?;
    if url.path() == "/" {
        Ok(None)
    } else {
        EndpointPath::parse(url.path())
            .map(Some)
            .map_err(|error| CoreError::invalid(format!("invalid provider API base path: {error}")))
    }
}

fn legacy_network_mode(base_url: &str) -> CoreResult<ProviderNetworkMode> {
    let url = Url::parse(base_url)
        .map_err(|error| CoreError::invalid(format!("invalid provider base URL: {error}")))?;
    Ok(if url.host_str().is_some_and(is_loopback_host) {
        ProviderNetworkMode::LocalLoopback
    } else {
        ProviderNetworkMode::Public
    })
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn validate_nonempty(label: &str, value: &str) -> CoreResult<()> {
    if value.trim().is_empty() {
        Err(CoreError::invalid(format!("{label} must not be empty")))
    } else {
        Ok(())
    }
}

fn validate_connection_config(config: &ConnectionConfig) -> CoreResult<()> {
    let mut keys = std::collections::BTreeSet::new();
    for entry in &config.values {
        validate_nonempty("connection configuration key", &entry.key)?;
        if !keys.insert(entry.key.as_str()) {
            return Err(CoreError::invalid(
                "connection configuration keys must be unique",
            ));
        }
        if is_sensitive_configuration_key(&entry.key) {
            return Err(CoreError::invalid(
                "credentials must be referenced by credential_ref and never stored in configuration",
            ));
        }
    }
    Ok(())
}

fn validate_provider_network_contract(
    api_origin: &CanonicalOrigin,
    config: &ConnectionConfig,
) -> CoreResult<()> {
    let origin = Url::parse(api_origin.as_str())
        .map_err(|error| CoreError::invalid(format!("provider API origin is invalid: {error}")))?;
    let host = origin
        .host_str()
        .ok_or_else(|| CoreError::invalid("provider API origin requires a host"))?;
    let loopback = is_loopback_host(host);
    match (config.network_mode, config.local_network_approval.as_ref()) {
        (ProviderNetworkMode::Public, None) => {
            if origin.scheme() != "https" {
                return Err(CoreError::invalid(
                    "public provider connections require an https API origin",
                ));
            }
            if loopback {
                return Err(CoreError::invalid(
                    "public provider connections cannot use a loopback API origin",
                ));
            }
            if host
                .trim_matches(['[', ']'])
                .parse::<IpAddr>()
                .is_ok_and(is_rfc1918_or_ula)
            {
                return Err(CoreError::invalid(
                    "private IP origins require approved local-network mode",
                ));
            }
        }
        (ProviderNetworkMode::LocalLoopback, None) => {
            if !loopback {
                return Err(CoreError::invalid(
                    "loopback provider mode requires a loopback API origin",
                ));
            }
        }
        (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
            validate_provider_local_network_approval(api_origin, approval)?;
        }
        (ProviderNetworkMode::ApprovedLocalNetwork, None) => {
            return Err(CoreError::invalid(
                "approved local-network mode requires an exact origin and address approval",
            ));
        }
        (ProviderNetworkMode::Public | ProviderNetworkMode::LocalLoopback, Some(_)) => {
            return Err(CoreError::invalid(
                "local-network approval is only valid in approved local-network mode",
            ));
        }
    }
    Ok(())
}

fn validate_provider_local_network_approval(
    api_origin: &CanonicalOrigin,
    approval: &ProviderLocalNetworkApproval,
) -> CoreResult<()> {
    if &approval.origin != api_origin {
        return Err(CoreError::invalid(
            "local-network approval origin must exactly match the provider API origin",
        ));
    }
    if approval.addresses.is_empty() || approval.addresses.len() > 16 {
        return Err(CoreError::invalid(
            "local-network approval requires from 1 to 16 exact IP addresses",
        ));
    }
    let mut normalized = approval
        .addresses
        .iter()
        .copied()
        .map(normalize_approved_ip)
        .collect::<Vec<_>>();
    if normalized
        .iter()
        .any(|address| !is_rfc1918_or_ula(*address))
    {
        return Err(CoreError::invalid(
            "local-network approval accepts only RFC1918 IPv4 or ULA IPv6 addresses",
        ));
    }
    normalized.sort_unstable();
    normalized.dedup();
    if normalized != approval.addresses {
        return Err(CoreError::invalid(
            "local-network approval addresses must be normalized, sorted, and unique",
        ));
    }
    let origin = Url::parse(api_origin.as_str())
        .map_err(|error| CoreError::invalid(format!("provider API origin is invalid: {error}")))?;
    if let Some(literal) = origin
        .host_str()
        .and_then(|host| host.trim_matches(['[', ']']).parse::<IpAddr>().ok())
        .map(normalize_approved_ip)
        && approval.addresses.as_slice() != [literal]
    {
        return Err(CoreError::invalid(
            "an IP-literal local-network origin must approve only that exact address",
        ));
    }
    Ok(())
}

fn validate_provider_local_network_approval_integrity(connection: &Connection) -> CoreResult<()> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT id, api_origin, config_json
                 FROM provider_connections
                 ORDER BY id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    for (id, api_origin, config_json) in rows {
        let api_origin = CanonicalOrigin::parse(&api_origin).map_err(|error| {
            storage_corrupted(format!("stored provider API origin is invalid: {error}"))
        })?;
        let config = serde_json::from_str::<ConnectionConfig>(&config_json).map_err(|error| {
            storage_corrupted(format!(
                "stored provider connection config is invalid: {error}"
            ))
        })?;
        validate_provider_network_contract(&api_origin, &config).map_err(stored_catalog_error)?;
        let mirror = connection
            .query_row(
                "SELECT origin, addresses_json
                 FROM provider_connection_local_network_approvals
                 WHERE connection_id = ?1",
                [&id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?;
        match (config.local_network_approval.as_ref(), mirror) {
            (None, None) => {}
            (None, Some(_)) | (Some(_), None) => {
                return Err(storage_corrupted(
                    "stored provider local-network approval mirror is incomplete",
                ));
            }
            (Some(approval), Some((origin, addresses_json))) => {
                let addresses =
                    serde_json::from_str::<Vec<IpAddr>>(&addresses_json).map_err(|error| {
                        storage_corrupted(format!(
                            "stored local-network approval addresses are invalid: {error}"
                        ))
                    })?;
                if origin != approval.origin.as_str() || addresses != approval.addresses {
                    return Err(storage_corrupted(
                        "stored provider local-network approval mirror does not match config",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn normalize_approved_ip(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map_or(IpAddr::V6(address), IpAddr::V4),
        IpAddr::V4(address) => IpAddr::V4(address),
    }
}

const fn is_rfc1918_or_ula(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => is_rfc1918(address),
        IpAddr::V6(address) => is_ula(address),
    }
}

const fn is_rfc1918(address: Ipv4Addr) -> bool {
    let octets = address.octets();
    octets[0] == 10
        || (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31)
        || (octets[0] == 192 && octets[1] == 168)
}

const fn is_ula(address: Ipv6Addr) -> bool {
    address.octets()[0] & 0xfe == 0xfc
}

fn validate_route_config(config: &ModelRouteConfig) -> CoreResult<()> {
    let mut keys = std::collections::BTreeSet::new();
    for entry in &config.values {
        validate_nonempty("model route configuration key", &entry.key)?;
        if !keys.insert(entry.key.as_str()) {
            return Err(CoreError::invalid(
                "model route configuration keys must be unique",
            ));
        }
        if is_sensitive_configuration_key(&entry.key) {
            return Err(CoreError::invalid(
                "credentials must never be stored in model route configuration",
            ));
        }
    }
    Ok(())
}

fn is_sensitive_configuration_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalized == "authorization"
        || normalized == "apikey"
        || normalized.ends_with("apikey")
        || normalized.contains("password")
        || normalized.contains("secret")
        || normalized.ends_with("token")
        || normalized.contains("credential")
        || normalized == "cookie"
}

fn validate_provider_connection(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    validate_nonempty("provider connection id", connection.id.as_str())?;
    validate_nonempty("provider connection display name", &connection.display_name)?;
    if connection.template_version == 0 {
        return Err(CoreError::invalid(
            "provider connection template version must be positive",
        ));
    }
    if !(1..=600).contains(&connection.timeout_seconds) {
        return Err(CoreError::invalid(
            "provider timeout must be from 1 to 600 seconds",
        ));
    }
    validate_provider_network_contract(&connection.api_origin, &connection.config)?;
    let template_json = transaction
        .query_row(
            "SELECT manifest_json FROM provider_templates
             WHERE id = ?1 AND version = ?2",
            params![connection.template_id.as_str(), connection.template_version],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("provider template"))?;
    let template = serde_json::from_str::<ProviderTemplate>(&template_json).map_err(|error| {
        storage_corrupted(format!("stored provider template is invalid: {error}"))
    })?;
    validate_connection_config(&connection.config)?;
    match (&connection.credential_ref, &connection.credential_scope) {
        (None, None) => {}
        (Some(reference), Some(scope)) => {
            validate_nonempty("credential reference", reference.as_str())?;
            if scope.allowed_origins.is_empty() {
                return Err(CoreError::invalid(
                    "credential scope requires at least one allowed origin",
                ));
            }
            if !scope
                .allowed_origins
                .iter()
                .any(|origin| origin == &connection.api_origin)
            {
                return Err(CoreError::invalid(
                    "credential scope must include the provider API origin",
                ));
            }
            let unique_origins = scope
                .allowed_origins
                .iter()
                .map(CanonicalOrigin::as_str)
                .collect::<std::collections::BTreeSet<_>>();
            if unique_origins.len() != scope.allowed_origins.len() {
                return Err(CoreError::invalid(
                    "credential scope origins must be unique",
                ));
            }
        }
        _ => {
            return Err(CoreError::invalid(
                "credential_ref and credential_scope must be set or cleared together",
            ));
        }
    }
    validate_connection_against_template(connection, &template)?;
    if connection.template_id.as_str() == LEGACY_PROVIDER_TEMPLATE_ID
        && connection.template_version == LEGACY_PROVIDER_TEMPLATE_VERSION
    {
        validate_legacy_provider_connection(transaction, connection)?;
    }
    Ok(())
}

fn validate_connection_against_template(
    connection: &ProviderConnection,
    template: &ProviderTemplate,
) -> CoreResult<()> {
    if connection.config.network_mode == ProviderNetworkMode::ApprovedLocalNetwork
        && !matches!(template.default_manifest.auth, AuthBinding::None)
        && Url::parse(connection.api_origin.as_str()).is_ok_and(|origin| origin.scheme() != "https")
    {
        return Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "credential-bearing local-network providers require an https API origin",
            false,
        ));
    }
    for entry in &connection.config.values {
        let field = template
            .connection_fields
            .iter()
            .find(|field| field.key == entry.key)
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "provider connection contains undeclared field {}",
                    entry.key
                ))
            })?;
        let type_matches = matches!(
            (field.value_type, &entry.value),
            (ConnectionFieldType::Text, ConnectionConfigValue::Text(_))
                | (
                    ConnectionFieldType::Integer,
                    ConnectionConfigValue::Integer(_)
                )
                | (
                    ConnectionFieldType::Boolean,
                    ConnectionConfigValue::Boolean(_)
                )
        );
        if !type_matches {
            return Err(CoreError::invalid(format!(
                "provider connection field {} has the wrong value type",
                entry.key
            )));
        }
    }
    for field in &template.connection_fields {
        let present = if field.value_type == ConnectionFieldType::Credential {
            connection.credential_ref.is_some()
        } else {
            connection
                .config
                .values
                .iter()
                .any(|entry| entry.key == field.key)
        };
        if field.required && !present {
            return Err(CoreError::invalid(format!(
                "provider connection is missing required field {}",
                field.key
            )));
        }
    }
    if let Some(scope) = connection.credential_scope.as_ref()
        && scope.auth_binding != template.default_manifest.auth
    {
        return Err(CoreError::invalid(
            "credential scope authentication does not match the provider template",
        ));
    }
    if matches!(template.default_manifest.auth, AuthBinding::None)
        && connection.credential_ref.is_some()
    {
        return Err(CoreError::invalid(
            "a no-auth provider template cannot persist a credential reference",
        ));
    }
    Ok(())
}

fn validate_legacy_provider_connection(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    let base_urls = connection
        .config
        .values
        .iter()
        .filter(|entry| entry.key == LEGACY_BASE_URL_CONFIG_KEY)
        .collect::<Vec<_>>();
    if base_urls.len() != 1 {
        return Err(CoreError::invalid(
            "legacy provider connection requires exactly one api_base_url value",
        ));
    }
    let ConnectionConfigValue::Text(base_url) = &base_urls[0].value else {
        return Err(CoreError::invalid(
            "legacy provider api_base_url must be text",
        ));
    };
    let origin = canonical_origin_for_legacy_base_url(base_url)?;
    if origin != connection.api_origin {
        return Err(CoreError::invalid(
            "legacy provider api_base_url origin does not match api_origin",
        ));
    }
    if legacy_api_base_path(base_url)? != connection.config.api_base_path {
        return Err(CoreError::invalid(
            "legacy provider api_base_url path does not match api_base_path",
        ));
    }
    if legacy_network_mode(base_url)? != connection.config.network_mode {
        return Err(CoreError::invalid(
            "legacy provider api_base_url does not match its network mode",
        ));
    }
    if connection
        .credential_ref
        .as_ref()
        .is_some_and(|reference| reference.as_str() != connection.id.as_str())
    {
        return Err(CoreError::invalid(
            "legacy provider credential_ref, when set, must equal the connection id",
        ));
    }
    if let Some((display_name, legacy_base_url, timeout_seconds)) = transaction
        .query_row(
            "SELECT display_name, base_url, timeout_seconds
             FROM provider_profiles WHERE id = ?1",
            [connection.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        && (display_name != connection.display_name
            || legacy_base_url != *base_url
            || timeout_seconds != connection.timeout_seconds)
    {
        return Err(CoreError::invalid(
            "legacy provider connection fields must match its provider profile",
        ));
    }
    Ok(())
}

fn validate_model_route(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
) -> CoreResult<()> {
    validate_model_route_for_schema(transaction, route, true)
}

fn validate_model_route_for_schema(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
    require_active_connection: bool,
) -> CoreResult<()> {
    validate_nonempty("model route id", route.id.as_str())?;
    validate_nonempty("provider connection id", route.connection_id.as_str())?;
    validate_nonempty("model id", &route.model_id)?;
    if route
        .display_name
        .as_deref()
        .is_some_and(|display_name| display_name.trim().is_empty())
    {
        return Err(CoreError::invalid(
            "model route display name must not be empty",
        ));
    }
    validate_route_config(&route.route_config)?;
    let template_query = if require_active_connection {
        "SELECT template.manifest_json
         FROM provider_connections AS connection
         JOIN provider_templates AS template
           ON template.id = connection.template_id
          AND template.version = connection.template_version
         WHERE connection.id = ?1
           AND connection.archived_at IS NULL"
    } else {
        "SELECT template.manifest_json
         FROM provider_connections AS connection
         JOIN provider_templates AS template
           ON template.id = connection.template_id
          AND template.version = connection.template_version
         WHERE connection.id = ?1"
    };
    let template_json = transaction
        .query_row(template_query, [route.connection_id.as_str()], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("provider connection"))?;
    let template = serde_json::from_str::<ProviderTemplate>(&template_json).map_err(|error| {
        storage_corrupted(format!("stored provider template is invalid: {error}"))
    })?;
    if template.api_family != route.api_family
        || template.default_manifest.api_family != route.api_family
    {
        return Err(CoreError::invalid(
            "model route API family does not match its provider template",
        ));
    }
    if template.id.as_str() == LEGACY_PROVIDER_TEMPLATE_ID
        && template.manifest_version == LEGACY_PROVIDER_TEMPLATE_VERSION
        && route.id.as_str() == route.connection_id.as_str()
        && let Some(legacy_model) = transaction
            .query_row(
                "SELECT model FROM provider_profiles WHERE id = ?1",
                [route.connection_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
        && legacy_model != route.model_id
    {
        return Err(CoreError::invalid(
            "legacy model route must match its provider profile model",
        ));
    }
    Ok(())
}

fn validate_capability_observation(
    transaction: &rusqlite::Transaction<'_>,
    observation: &CapabilityObservation,
) -> CoreResult<()> {
    validate_bounded_identifier("capability observation id", observation.id.as_str(), 256)?;
    validate_bounded_identifier("model route id", observation.model_route_id.as_str(), 256)?;
    if !row_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
        observation.model_route_id.as_str(),
    )? {
        return Err(not_found("model route"));
    }
    if observation
        .expires_at
        .is_some_and(|expires_at| expires_at <= observation.observed_at)
    {
        return Err(CoreError::invalid(
            "capability observation expires_at must follow observed_at",
        ));
    }
    if let Some(evidence_ref) = observation.evidence_ref.as_ref() {
        validate_bounded_identifier("capability evidence reference", evidence_ref.as_str(), 512)?;
    }
    validate_capability_value(observation.key, &observation.value)?;
    if observation.status == SupportStatus::Unsupported
        && observation.value != CapabilityValue::Boolean(false)
    {
        return Err(CoreError::invalid(
            "an unsupported capability observation must carry boolean false",
        ));
    }
    Ok(())
}

pub(crate) fn validate_provider_api_snapshot_observation(
    observation: &CapabilityObservation,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    validate_bounded_identifier("capability observation id", observation.id.as_str(), 256)?;
    validate_bounded_identifier("model route id", observation.model_route_id.as_str(), 256)?;
    let expected_expires_at = observed_at
        .checked_add_signed(PROVIDER_API_CAPABILITY_FRESHNESS)
        .ok_or_else(|| {
            CoreError::invalid("provider API snapshot observation freshness cannot be represented")
        })?;
    if observation.source != ObservationSource::ProviderApi
        || observation.confidence != Confidence::High
        || observation.observed_at != observed_at
        || observation.expires_at != Some(expected_expires_at)
        || observation.evidence_ref.is_some()
    {
        return Err(CoreError::invalid(
            "provider API snapshot observation provenance or freshness is inconsistent",
        ));
    }
    validate_capability_value(observation.key, &observation.value)?;
    let shape_is_valid = matches!(
        (observation.key, observation.status, &observation.value),
        (
            CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens,
            SupportStatus::Verified,
            CapabilityValue::Integer(_),
        ) | (
            CapabilityKey::Reasoning,
            SupportStatus::Verified,
            CapabilityValue::Structured(_),
        ) | (
            CapabilityKey::Reasoning
                | CapabilityKey::ToolCalling
                | CapabilityKey::ParallelToolCalling
                | CapabilityKey::StructuredOutput
                | CapabilityKey::JsonMode
                | CapabilityKey::Logprobs
                | CapabilityKey::Seed,
            SupportStatus::Verified,
            CapabilityValue::Boolean(true),
        ) | (
            CapabilityKey::Reasoning
                | CapabilityKey::ToolCalling
                | CapabilityKey::ParallelToolCalling
                | CapabilityKey::StructuredOutput
                | CapabilityKey::JsonMode
                | CapabilityKey::Logprobs
                | CapabilityKey::Seed,
            SupportStatus::Unsupported,
            CapabilityValue::Boolean(false),
        )
    );
    if !shape_is_valid {
        return Err(CoreError::invalid(
            "provider API snapshot observation key, status, or value is inconsistent",
        ));
    }
    Ok(())
}

fn validate_provider_api_snapshot_observations_for_routes(
    observations: &[CapabilityObservation],
    listed_route_ids: &std::collections::BTreeSet<&str>,
    observed_at: DateTime<Utc>,
) -> CoreResult<()> {
    let mut observation_ids = std::collections::BTreeSet::new();
    for observation in observations {
        if !listed_route_ids.contains(observation.model_route_id.as_str())
            || !observation_ids.insert(observation.id.as_str())
        {
            return Err(CoreError::invalid(
                "model refresh capability observations must be unique and belong to a listed route",
            ));
        }
        validate_provider_api_snapshot_observation(observation, observed_at)?;
    }
    Ok(())
}

fn validate_bounded_identifier(label: &str, value: &str, max_bytes: usize) -> CoreResult<()> {
    validate_nonempty(label, value)?;
    if value.trim() != value || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(CoreError::invalid(format!(
            "{label} is oversized, contains control characters, or is not trimmed"
        )));
    }
    Ok(())
}

fn validate_capability_value(key: CapabilityKey, value: &CapabilityValue) -> CoreResult<()> {
    match (key, value) {
        (
            CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens,
            CapabilityValue::Integer(value),
        ) if *value > 0 => {}
        (
            CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens,
            CapabilityValue::Structured(value),
        ) => validate_capability_structured_value(value)?,
        (CapabilityKey::ContextWindow | CapabilityKey::MaxOutputTokens, _) => {
            return Err(CoreError::invalid(
                "token-limit capabilities require a positive integer or structured value",
            ));
        }
        (_, CapabilityValue::Integer(_)) => {
            return Err(CoreError::invalid(
                "integer capability values are reserved for numeric token limits",
            ));
        }
        (_, CapabilityValue::EnumValues(values)) => {
            if values.is_empty() || values.len() > MAX_CAPABILITY_ENUM_VALUES {
                return Err(CoreError::invalid(
                    "capability enum values must contain from 1 to 128 entries",
                ));
            }
            let mut unique = std::collections::BTreeSet::new();
            for value in values {
                validate_bounded_identifier("capability enum value", value, 256)?;
                if !unique.insert(value.as_str()) {
                    return Err(CoreError::invalid("capability enum values must be unique"));
                }
            }
        }
        (_, CapabilityValue::Structured(value)) => validate_capability_structured_value(value)?,
        (_, CapabilityValue::Boolean(_)) => {}
    }
    Ok(())
}

fn validate_capability_structured_value(value: &serde_json::Value) -> CoreResult<()> {
    if !value.is_object() {
        return Err(CoreError::invalid(
            "structured capability values must be JSON objects",
        ));
    }
    let encoded = serde_json::to_string(value).map_err(|error| {
        CoreError::invalid(format!("structured capability value is invalid: {error}"))
    })?;
    if encoded.len() > MAX_CAPABILITY_VALUE_BYTES
        || encoded.chars().count() > MAX_CAPABILITY_VALUE_CHARS
    {
        return Err(CoreError::invalid(
            "structured capability value exceeds the storage limit",
        ));
    }
    let mut pending = vec![(value, 0_usize)];
    let mut visited = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        visited = visited.saturating_add(1);
        if visited > 2_048 || depth > 16 {
            return Err(CoreError::invalid(
                "structured capability value exceeds nesting or node limits",
            ));
        }
        match node {
            serde_json::Value::Object(object) => {
                for (key, child) in object {
                    if is_sensitive_configuration_key(key) {
                        return Err(CoreError::invalid(
                            "raw credentials and secret-like fields must never be stored in capability metadata",
                        ));
                    }
                    pending.push((child, depth.saturating_add(1)));
                }
            }
            serde_json::Value::Array(array) => {
                pending.extend(array.iter().map(|child| (child, depth.saturating_add(1))));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_generation_preset(
    transaction: &rusqlite::Transaction<'_>,
    preset: &GenerationPreset,
) -> CoreResult<()> {
    validate_generation_preset_for_schema(transaction, preset, true)
}

fn validate_generation_preset_for_schema(
    transaction: &rusqlite::Transaction<'_>,
    preset: &GenerationPreset,
    require_active_connection: bool,
) -> CoreResult<()> {
    validate_nonempty("generation preset id", preset.id.as_str())?;
    validate_nonempty("generation preset display name", &preset.display_name)?;
    let template_query = if require_active_connection {
        "SELECT template.manifest_json
         FROM provider_models AS model
         JOIN provider_connections AS connection
           ON connection.id = model.connection_id
         JOIN provider_templates AS template
           ON template.id = connection.template_id
          AND template.version = connection.template_version
         WHERE model.id = ?1
           AND connection.archived_at IS NULL"
    } else {
        "SELECT template.manifest_json
         FROM provider_models AS model
         JOIN provider_connections AS connection
           ON connection.id = model.connection_id
         JOIN provider_templates AS template
           ON template.id = connection.template_id
          AND template.version = connection.template_version
         WHERE model.id = ?1"
    };
    let template_json = transaction
        .query_row(template_query, [preset.model_route_id.as_str()], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("model route"))?;
    let template = serde_json::from_str::<ProviderTemplate>(&template_json).map_err(|error| {
        storage_corrupted(format!("stored provider template is invalid: {error}"))
    })?;
    let mut ids = std::collections::BTreeSet::new();
    for value in &preset.values {
        if !ids.insert(value.parameter_id.as_str()) {
            return Err(CoreError::invalid(
                "generation preset parameter identifiers must be unique",
            ));
        }
        let specification = template
            .default_manifest
            .parameters
            .iter()
            .find(|specification| specification.id == value.parameter_id)
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "generation preset references unknown parameter {}",
                    value.parameter_id
                ))
            })?;
        validate_parameter_value(specification, &value.state)?;
    }
    Ok(())
}

fn validate_parameter_value(
    specification: &ParameterSpec,
    state: &ParameterValueState,
) -> CoreResult<()> {
    let ParameterValueState::Explicit(literal) = state else {
        return Ok(());
    };
    let type_matches = matches!(
        (specification.value_type, literal),
        (ParameterType::Boolean, ParameterLiteral::Boolean(_))
            | (ParameterType::Integer, ParameterLiteral::Integer(_))
            | (ParameterType::Number, ParameterLiteral::Number(_))
            | (ParameterType::String, ParameterLiteral::String(_))
            | (ParameterType::Enum, ParameterLiteral::Enum(_))
            | (ParameterType::StringList, ParameterLiteral::StringList(_))
            | (ParameterType::JsonSchema, ParameterLiteral::JsonSchema(_))
            | (
                ParameterType::StopSequenceList,
                ParameterLiteral::StopSequenceList(_)
            )
            | (ParameterType::ToolPolicy, ParameterLiteral::ToolPolicy(_))
    );
    if !type_matches {
        return Err(CoreError::invalid(format!(
            "generation preset parameter {} has the wrong value type",
            specification.id
        )));
    }
    let numeric_value = match literal {
        ParameterLiteral::Integer(value) => {
            Some(value.to_string().parse::<f64>().map_err(|error| {
                CoreError::internal(format!(
                    "cannot validate generation preset integer value: {error}"
                ))
            })?)
        }
        ParameterLiteral::Number(value) if value.is_finite() => Some(*value),
        ParameterLiteral::Number(_) => {
            return Err(CoreError::invalid(
                "generation preset numeric values must be finite",
            ));
        }
        _ => None,
    };
    if let Some(value) = numeric_value
        && (specification.minimum.is_some_and(|minimum| value < minimum)
            || specification.maximum.is_some_and(|maximum| value > maximum))
    {
        return Err(CoreError::invalid(format!(
            "generation preset parameter {} is outside its allowed range",
            specification.id
        )));
    }
    if !specification.allowed_values.is_empty()
        && !specification
            .allowed_values
            .iter()
            .any(|choice| choice.value == *literal)
    {
        return Err(CoreError::invalid(format!(
            "generation preset parameter {} is not an allowed value",
            specification.id
        )));
    }
    Ok(())
}

fn upsert_provider_profile_row(
    transaction: &rusqlite::Transaction<'_>,
    profile: &ProviderProfile,
) -> CoreResult<()> {
    validate_legacy_provider_profile(profile)?;
    let existing_base_url = transaction
        .query_row(
            "SELECT base_url FROM provider_profiles WHERE id = ?1",
            [profile.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if existing_base_url
        .as_deref()
        .is_some_and(|base_url| base_url != profile.base_url)
    {
        return Err(CoreError::invalid(
            "an existing provider profile cannot change its API endpoint; \
             create a new connection instead",
        ));
    }
    transaction
        .execute(
            "INSERT INTO provider_profiles
             (id, display_name, base_url, model, timeout_seconds)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               model = excluded.model,
               timeout_seconds = excluded.timeout_seconds",
            params![
                profile.id,
                profile.display_name,
                profile.base_url,
                profile.model,
                profile.timeout_seconds
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(crate) fn upsert_provider_connection_row(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    validate_provider_connection(transaction, connection)?;
    if connection.updated_at < connection.created_at {
        return Err(CoreError::invalid(
            "provider connection updated_at must not precede created_at",
        ));
    }
    let archived_connection_exists = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1 FROM provider_connections
           WHERE id = ?1 AND archived_at IS NOT NULL
         )",
        connection.id.as_str(),
    )?;
    if archived_connection_exists {
        return Err(CoreError::invalid(
            "an archived provider connection identifier cannot be reused",
        ));
    }
    let existing_connection = transaction
        .query_row(
            "SELECT id, template_id, template_version, display_name, api_origin,
                    config_json, credential_ref, credential_scope_json, timeout_seconds,
                    status, created_at, updated_at
             FROM provider_connections
             WHERE id = ?1 AND archived_at IS NULL",
            [connection.id.as_str()],
            provider_connection_columns,
        )
        .optional()
        .map_err(storage_db_error)?
        .map(decode_provider_connection_row)
        .transpose()?;
    if existing_connection.as_ref().is_some_and(|existing| {
        existing.template_id != connection.template_id
            || existing.template_version != connection.template_version
    }) {
        return Err(CoreError::invalid(
            "an existing provider connection cannot change its template identity",
        ));
    }
    if let Some(existing) = existing_connection.as_ref()
        && (existing.api_origin != connection.api_origin
            || existing.config != connection.config
            || existing.credential_ref != connection.credential_ref
            || existing.credential_scope != connection.credential_scope)
    {
        return Err(CoreError::invalid(
            "an existing provider connection cannot change its endpoint configuration, \
             network approval, or credential binding; create a new connection instead",
        ));
    }
    let config_json = serde_json::to_string(&connection.config).map_err(|error| {
        CoreError::internal(format!("cannot encode provider connection config: {error}"))
    })?;
    let credential_scope_json = connection
        .credential_scope
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| CoreError::internal(format!("cannot encode credential scope: {error}")))?;
    transaction
        .execute(
            "INSERT INTO provider_connections
             (id, template_id, template_version, display_name, api_origin, config_json,
              credential_ref, credential_scope_json, timeout_seconds, status,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               api_origin = excluded.api_origin,
               config_json = excluded.config_json,
               credential_ref = excluded.credential_ref,
               credential_scope_json = excluded.credential_scope_json,
               timeout_seconds = excluded.timeout_seconds,
               status = excluded.status,
               updated_at = excluded.updated_at",
            params![
                connection.id.as_str(),
                connection.template_id.as_str(),
                connection.template_version,
                connection.display_name,
                connection.api_origin.as_str(),
                config_json,
                connection
                    .credential_ref
                    .as_ref()
                    .map(CredentialRef::as_str),
                credential_scope_json,
                connection.timeout_seconds,
                connection_status_to_str(connection.status),
                connection.created_at.to_rfc3339(),
                connection.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    sync_provider_local_network_approval_row(transaction, connection)?;
    Ok(())
}

fn ensure_provider_connection_id_vacant(
    transaction: &rusqlite::Transaction<'_>,
    id: &ProviderConnectionId,
) -> CoreResult<()> {
    if row_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM provider_connections WHERE id = ?1)",
        id.as_str(),
    )? {
        return Err(CoreError::invalid(
            "provider connection identifier already exists; choose a new identifier",
        ));
    }
    Ok(())
}

fn sync_provider_local_network_approval_row(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    let existing = transaction
        .query_row(
            "SELECT origin, addresses_json
             FROM provider_connection_local_network_approvals
             WHERE connection_id = ?1",
            [connection.id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(storage_db_error)?;
    match (connection.config.local_network_approval.as_ref(), existing) {
        (None, None) => Ok(()),
        (None, Some(_)) => Err(storage_corrupted(
            "stored local-network approval has no matching typed connection grant",
        )),
        (Some(approval), Some((origin, addresses_json))) => {
            let addresses =
                serde_json::from_str::<Vec<IpAddr>>(&addresses_json).map_err(|error| {
                    storage_corrupted(format!(
                        "stored local-network approval addresses are invalid: {error}"
                    ))
                })?;
            if origin != approval.origin.as_str() || addresses != approval.addresses {
                return Err(storage_corrupted(
                    "stored local-network approval does not match its typed connection grant",
                ));
            }
            Ok(())
        }
        (Some(approval), None) => {
            let addresses_json = serde_json::to_string(&approval.addresses).map_err(|error| {
                CoreError::internal(format!(
                    "cannot encode provider local-network approval: {error}"
                ))
            })?;
            transaction
                .execute(
                    "INSERT INTO provider_connection_local_network_approvals
                     (connection_id, origin, addresses_json, approved_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        connection.id.as_str(),
                        approval.origin.as_str(),
                        addresses_json,
                        connection.created_at.to_rfc3339(),
                    ],
                )
                .map_err(storage_db_error)?;
            Ok(())
        }
    }
}

pub(crate) fn upsert_model_route_row(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
) -> CoreResult<()> {
    validate_model_route(transaction, route)?;
    if route
        .last_seen_at
        .is_some_and(|last_seen_at| last_seen_at < route.first_seen_at)
    {
        return Err(CoreError::invalid(
            "model route last_seen_at must not precede first_seen_at",
        ));
    }
    let route_json = serde_json::to_string(&route.route_config)
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    let existing_identity = transaction
        .query_row(
            "SELECT connection_id, api_family, model_id, route_json
             FROM provider_models WHERE id = ?1",
            [route.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    if let Some((connection_id, api_family, model_id, stored_route_json)) = existing_identity
        && (connection_id != route.connection_id.as_str()
            || api_family != api_family_to_str(route.api_family)
            || model_id != route.model_id
            || stored_route_json != route_json)
    {
        return Err(CoreError::invalid(
            "an existing model route cannot change its stable identity",
        ));
    }
    let raw_metadata_json = route.raw_metadata.as_ref().map(BoundedJson::as_str);
    transaction
        .execute(
            "INSERT INTO provider_models
             (id, connection_id, api_family, model_id, display_name, route_json,
              availability, raw_metadata_json, miss_count, metadata_source_kind,
              metadata_observed_at, last_reconciled_sync_job_id, metadata_sync_job_id,
              first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               availability = excluded.availability,
               raw_metadata_json = excluded.raw_metadata_json,
               miss_count = excluded.miss_count,
               metadata_source_kind = excluded.metadata_source_kind,
               metadata_observed_at = excluded.metadata_observed_at,
               last_reconciled_sync_job_id = excluded.last_reconciled_sync_job_id,
               metadata_sync_job_id = excluded.metadata_sync_job_id,
               first_seen_at = MIN(provider_models.first_seen_at, excluded.first_seen_at),
               last_seen_at = excluded.last_seen_at",
            params![
                route.id.as_str(),
                route.connection_id.as_str(),
                api_family_to_str(route.api_family),
                route.model_id,
                route.display_name,
                route_json,
                model_availability_to_str(route.status),
                raw_metadata_json,
                route.miss_count,
                model_metadata_source_to_str(route.metadata_source),
                route.metadata_observed_at.map(|value| value.to_rfc3339()),
                route
                    .last_reconciled_sync_job_id
                    .as_ref()
                    .map(ModelSyncJobId::as_str),
                route
                    .metadata_sync_job_id
                    .as_ref()
                    .map(ModelSyncJobId::as_str),
                route.first_seen_at.to_rfc3339(),
                route.last_seen_at.map(|value| value.to_rfc3339()),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(crate) fn upsert_capability_observation_row(
    transaction: &rusqlite::Transaction<'_>,
    observation: &CapabilityObservation,
) -> CoreResult<()> {
    validate_capability_observation(transaction, observation)?;
    let existing = transaction
        .query_row(
            "SELECT id, model_route_id, capability_key, value_json, support_status,
                    source_kind, confidence, evidence_ref, observed_at, expires_at
             FROM model_capability_observations
             WHERE id = ?1",
            [observation.id.as_str()],
            capability_observation_columns,
        )
        .optional()
        .map_err(storage_db_error)?
        .map(decode_capability_observation_row)
        .transpose()?;
    if let Some(existing) = existing.as_ref() {
        if existing.model_route_id != observation.model_route_id
            || existing.key != observation.key
            || existing.source != observation.source
        {
            return Err(CoreError::invalid(
                "an existing capability observation cannot change route, key, or source",
            ));
        }
        if observation.observed_at < existing.observed_at {
            return Err(CoreError::invalid(
                "capability observation updates must not move observed_at backwards",
            ));
        }
        if observation.observed_at == existing.observed_at {
            if existing == observation {
                return Ok(());
            }
            return Err(CoreError::invalid(
                "a capability observation cannot change without advancing observed_at",
            ));
        }
    }

    let value_json = serde_json::to_string(&observation.value).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode capability observation value: {error}"
        ))
    })?;
    transaction
        .execute(
            "INSERT INTO model_capability_observations
             (id, model_route_id, capability_key, value_json, support_status,
              source_kind, confidence, evidence_ref, observed_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
               value_json = excluded.value_json,
               support_status = excluded.support_status,
               confidence = excluded.confidence,
               evidence_ref = excluded.evidence_ref,
               observed_at = excluded.observed_at,
               expires_at = excluded.expires_at",
            params![
                observation.id.as_str(),
                observation.model_route_id.as_str(),
                capability_key_to_str(observation.key),
                value_json,
                support_status_to_str(observation.status),
                observation_source_to_str(observation.source),
                confidence_to_str(observation.confidence),
                observation.evidence_ref.as_ref().map(EvidenceId::as_str),
                observation.observed_at.to_rfc3339(),
                observation.expires_at.map(|value| value.to_rfc3339()),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(crate) fn upsert_generation_preset_row(
    transaction: &rusqlite::Transaction<'_>,
    preset: &GenerationPreset,
) -> CoreResult<()> {
    validate_generation_preset(transaction, preset)?;
    if preset.updated_at < preset.created_at {
        return Err(CoreError::invalid(
            "generation preset updated_at must not precede created_at",
        ));
    }
    let existing_route = transaction
        .query_row(
            "SELECT model_route_id FROM generation_presets WHERE id = ?1",
            [preset.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if existing_route
        .as_deref()
        .is_some_and(|id| id != preset.model_route_id.as_str())
    {
        return Err(CoreError::invalid(
            "an existing generation preset cannot change its model route",
        ));
    }
    let values_json = encode_generation_preset_values(preset)?;
    transaction
        .execute(
            "INSERT INTO generation_presets
             (id, model_route_id, display_name, values_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               values_json = excluded.values_json,
               updated_at = excluded.updated_at",
            params![
                preset.id.as_str(),
                preset.model_route_id.as_str(),
                preset.display_name,
                values_json,
                preset.created_at.to_rfc3339(),
                preset.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn encode_generation_preset_values(preset: &GenerationPreset) -> CoreResult<String> {
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "values": &preset.values,
        "reasoning": &preset.reasoning,
        "prompt_cache": &preset.prompt_cache,
    }))
    .map_err(|error| {
        CoreError::internal(format!("cannot encode generation preset values: {error}"))
    })
}

fn ensure_legacy_template_exists(transaction: &rusqlite::Transaction<'_>) -> CoreResult<()> {
    let row = transaction
        .query_row(
            "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
             FROM provider_templates WHERE id = ?1 AND version = ?2",
            params![
                LEGACY_PROVIDER_TEMPLATE_ID,
                LEGACY_PROVIDER_TEMPLATE_VERSION
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("built-in legacy provider template is missing"))?;
    let stored = decode_provider_template_row(row)?;
    let expected = legacy_provider_template()?;
    if stored != expected {
        return Err(storage_corrupted(
            "built-in legacy provider template does not match the supported definition",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn normalize_settings_for_write(
    transaction: &rusqlite::Transaction<'_>,
    settings: &AppSettings,
) -> CoreResult<AppSettings> {
    normalize_settings_for_schema(transaction, settings, true)
}

fn normalize_settings_during_v4_migration(
    transaction: &rusqlite::Transaction<'_>,
    settings: &AppSettings,
) -> CoreResult<AppSettings> {
    normalize_settings_for_schema(transaction, settings, false)
}

#[allow(clippy::too_many_lines)]
fn normalize_settings_for_schema(
    transaction: &rusqlite::Transaction<'_>,
    settings: &AppSettings,
    require_active_connection: bool,
) -> CoreResult<AppSettings> {
    let mut normalized = settings.clone();
    let profile_exists_query = if require_active_connection {
        "SELECT EXISTS(
           SELECT 1
           FROM provider_profiles AS profile
           JOIN provider_connections AS connection
             ON connection.id = profile.id
            AND connection.archived_at IS NULL
           WHERE profile.id = ?1
         )"
    } else {
        "SELECT EXISTS(
           SELECT 1
           FROM provider_profiles AS profile
           JOIN provider_connections AS connection
             ON connection.id = profile.id
           WHERE profile.id = ?1
         )"
    };
    let route_exists_query = if require_active_connection {
        "SELECT EXISTS(
           SELECT 1
           FROM provider_models AS model
           JOIN provider_connections AS connection
             ON connection.id = model.connection_id
            AND connection.archived_at IS NULL
           WHERE model.id = ?1
         )"
    } else {
        "SELECT EXISTS(
           SELECT 1
           FROM provider_models AS model
           JOIN provider_connections AS connection
             ON connection.id = model.connection_id
           WHERE model.id = ?1
         )"
    };
    if let Some(profile_id) = normalized.selected_provider_profile_id.as_deref() {
        if !row_exists(transaction, profile_exists_query, profile_id)? {
            return Err(not_found("provider profile"));
        }
        let current_route_id = legacy_profile_current_route_id_for_schema(
            transaction,
            profile_id,
            require_active_connection,
        )?;
        match normalized.selected_model_route_id.as_ref() {
            Some(route_id) if route_id != &current_route_id => {
                return Err(CoreError::invalid(
                    "legacy provider and model route selections must identify the same migrated provider",
                ));
            }
            None => {
                normalized.selected_model_route_id = Some(current_route_id.clone());
            }
            Some(_) => {}
        }
        let current_preset_id = GenerationPresetId::from(current_route_id.as_str());
        match normalized.selected_generation_preset_id.as_ref() {
            Some(preset_id) if preset_id != &current_preset_id => {
                return Err(CoreError::invalid(
                    "legacy provider and generation preset selections must identify the same migrated provider",
                ));
            }
            None => {
                normalized.selected_generation_preset_id = Some(current_preset_id);
            }
            Some(_) => {}
        }
    }

    match (
        normalized.selected_model_route_id.as_ref(),
        normalized.selected_generation_preset_id.as_ref(),
    ) {
        (None, None) => {}
        (Some(route_id), Some(preset_id)) => {
            let route_exists = row_exists(transaction, route_exists_query, route_id.as_str())?;
            if !route_exists {
                return Err(not_found("model route"));
            }
            let preset_route_id = transaction
                .query_row(
                    "SELECT model_route_id FROM generation_presets WHERE id = ?1",
                    [preset_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| not_found("generation preset"))?;
            if preset_route_id != route_id.as_str() {
                return Err(CoreError::invalid(
                    "selected generation preset does not belong to the selected model route",
                ));
            }
            if normalized.selected_provider_profile_id.is_none()
                && route_id.as_str() == preset_id.as_str()
                && row_exists(transaction, profile_exists_query, route_id.as_str())?
            {
                normalized.selected_provider_profile_id = Some(route_id.as_str().to_owned());
            }
        }
        _ => {
            return Err(CoreError::invalid(
                "model route and generation preset selections must be set or cleared together",
            ));
        }
    }
    Ok(normalized)
}

fn legacy_profile_current_route_id_for_schema(
    connection: &Connection,
    profile_id: &str,
    require_active_connection: bool,
) -> CoreResult<ModelRouteId> {
    let route_json = serde_json::to_string(&ModelRouteConfig::default())
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    let route_query = if require_active_connection {
        "SELECT model.id
         FROM provider_profiles AS profile
         JOIN provider_connections AS connection
           ON connection.id = profile.id
          AND connection.archived_at IS NULL
         JOIN provider_models AS model
           ON model.connection_id = profile.id
          AND model.api_family = 'openai_chat_completions'
          AND model.model_id = profile.model
          AND model.route_json = ?2
         WHERE profile.id = ?1
         ORDER BY model.first_seen_at, model.id
         LIMIT 1"
    } else {
        "SELECT model.id
         FROM provider_profiles AS profile
         JOIN provider_connections AS connection
           ON connection.id = profile.id
         JOIN provider_models AS model
           ON model.connection_id = profile.id
          AND model.api_family = 'openai_chat_completions'
          AND model.model_id = profile.model
          AND model.route_json = ?2
         WHERE profile.id = ?1
         ORDER BY model.first_seen_at, model.id
         LIMIT 1"
    };
    connection
        .query_row(route_query, params![profile_id, route_json], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(storage_db_error)?
        .map(ModelRouteId::from)
        .ok_or_else(|| {
            storage_corrupted("legacy provider profile has no route for its current model")
        })
}

fn active_retained_legacy_profile_exists(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<bool> {
    row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1
           FROM provider_profiles AS profile
           JOIN provider_connections AS connection
             ON connection.id = profile.id
            AND connection.archived_at IS NULL
           WHERE profile.id = ?1
         )",
        connection_id,
    )
}

fn current_migrated_legacy_route_exists(
    transaction: &rusqlite::Transaction<'_>,
    route_id: &str,
) -> CoreResult<bool> {
    let route_json = serde_json::to_string(&ModelRouteConfig::default())
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM provider_models AS model
               JOIN provider_profiles AS profile
                 ON profile.id = model.connection_id
                AND profile.model = model.model_id
               JOIN provider_connections AS connection
                 ON connection.id = profile.id
                AND connection.archived_at IS NULL
               WHERE model.id = ?1
                 AND model.api_family = 'openai_chat_completions'
                 AND model.route_json = ?2
             )",
            params![route_id, route_json],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)
}

pub(crate) fn archive_provider_connection_row(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
    archived_at: DateTime<Utc>,
) -> CoreResult<()> {
    let active = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1 FROM provider_connections
           WHERE id = ?1 AND archived_at IS NULL
         )",
        connection_id,
    )?;
    if !active {
        return Err(not_found("provider connection"));
    }
    ensure_provider_connection_has_no_unfinished_work(transaction, connection_id)?;
    clear_provider_selections_for_connection(transaction, connection_id)?;
    let changed = transaction
        .execute(
            "UPDATE provider_connections
             SET archived_at = ?2
             WHERE id = ?1 AND archived_at IS NULL",
            params![connection_id, archived_at.to_rfc3339()],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(CoreError::invalid(
            "provider connection changed while it was being archived",
        ));
    }
    Ok(())
}

pub(crate) fn ensure_provider_connection_has_no_unfinished_work(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<()> {
    ensure_provider_connection_has_no_unfinished_generation_attempt(transaction, connection_id)?;
    let unfinished_generation = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1
           FROM generations AS generation
           JOIN provider_models AS route ON route.id = generation.model_route_id
           WHERE route.connection_id = ?1
             AND generation.status = 'running'
         )",
        connection_id,
    )?;
    if unfinished_generation {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot be archived while generation is unfinished",
            true,
        ));
    }
    let unfinished_model_sync = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1
           FROM model_sync_jobs
           WHERE connection_id = ?1
             AND state NOT IN ('completed', 'failed', 'cancelled')
         )",
        connection_id,
    )?;
    if unfinished_model_sync {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot be archived while model synchronization is unfinished",
            true,
        ));
    }

    let unfinished_discovery = row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1
           FROM provider_discovery_sessions
           WHERE (
               json_extract(sanitized_input_json, '$.connection_id') = ?1
               OR committed_connection_id = ?1
             )
             AND state NOT IN ('ready', 'failed', 'cancelled')
         )",
        connection_id,
    )?;
    if unfinished_discovery {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot be archived while provider discovery is unfinished",
            true,
        ));
    }
    Ok(())
}

fn ensure_provider_connection_has_no_unfinished_generation_attempt(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<()> {
    let unfinished_generation_attempt = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM generation_attempt_intents AS attempt
               LEFT JOIN provider_models AS route
                 ON route.id = json_extract(
                   attempt.prompt_selection_authority_json,
                   '$.provider_target_authority.target.model_route_id'
                 )
               WHERE attempt.status NOT IN ('failed_before_dispatch', 'completed')
                 AND (
                   (
                     json_extract(
                       attempt.prompt_selection_authority_json,
                       '$.provider_target_authority.kind'
                     ) = 'provider_profile'
                     AND json_extract(
                       attempt.prompt_selection_authority_json,
                       '$.provider_target_authority.provider_profile_id'
                     ) = ?1
                   )
                   OR (
                     json_extract(
                       attempt.prompt_selection_authority_json,
                       '$.provider_target_authority.kind'
                     ) = 'generation_target'
                     AND route.connection_id = ?1
                   )
                 )
             )",
            [connection_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if unfinished_generation_attempt {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider connection cannot be archived while a generation attempt is unfinished",
            true,
        ));
    }
    Ok(())
}

fn ensure_generation_provider_credential_settled(
    transaction: &rusqlite::Transaction<'_>,
    generation: &GenerationRecord,
    credential_authority: Option<&crate::ProviderCredentialAccessAuthority>,
    require_exact_credential_authority: bool,
) -> CoreResult<()> {
    let Some(model_route_id) = generation.model_route_id.as_ref() else {
        return Ok(());
    };
    if require_exact_credential_authority {
        let connection_id = transaction
            .query_row(
                "SELECT connection_id FROM provider_models WHERE id = ?1",
                [model_route_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| CoreError::invalid("generation provider model route is unavailable"))?;
        return crate::generation_attempt::validate_provider_credential_access_authority_in_transaction(
            transaction,
            &ProviderConnectionId::from(connection_id),
            credential_authority,
        );
    }
    let unresolved = transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM provider_models AS route
               JOIN provider_credential_operations AS operation
                 ON operation.connection_id = route.connection_id
               WHERE route.id = ?1
                 AND operation.status IN (
                   'prepared', 'started', 'cleanup_required', 'outcome_unknown'
                 )
             )",
            [model_route_id.as_str()],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if unresolved {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "generation cannot start while provider credential recovery is unresolved",
            true,
        ));
    }
    Ok(())
}

pub(crate) fn clear_provider_selections_for_connection(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<()> {
    if clear_provider_selections_for_connection_without_revision(transaction, connection_id)? {
        advance_provider_selection_revision(transaction)?;
    }
    Ok(())
}

/// Clears a selection owned by the graph currently being compensated and
/// returns the exact durable revision produced by that internal clear.
///
/// `None` means graph removal did not change the selection, so a later restore
/// has no authority to treat an already-clear value as its own effect.
pub(crate) fn clear_provider_selections_for_discovery_compensation(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<Option<u64>> {
    if clear_provider_selections_for_connection_without_revision(transaction, connection_id)? {
        return advance_provider_selection_revision(transaction).map(Some);
    }
    Ok(None)
}

fn clear_provider_selections_for_connection_without_revision(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<bool> {
    update_stored_settings_without_selection_revision(transaction, |settings| {
        let selected_route_belongs =
            if let Some(route_id) = settings.selected_model_route_id.as_ref() {
                transaction
                    .query_row(
                        "SELECT EXISTS(
                           SELECT 1 FROM provider_models
                           WHERE id = ?1 AND connection_id = ?2
                         )",
                        params![route_id.as_str(), connection_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(storage_db_error)?
            } else {
                false
            };
        if settings.selected_provider_profile_id.as_deref() == Some(connection_id)
            || selected_route_belongs
        {
            settings.selected_provider_profile_id = None;
            settings.selected_model_route_id = None;
            settings.selected_generation_preset_id = None;
        }
        Ok(())
    })
}

pub(crate) fn load_discovery_previous_selection(
    connection: &Connection,
) -> CoreResult<DiscoveryPreviousSelection> {
    let settings_json = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    let settings = settings_json.map_or_else(
        || Ok(AppSettings::default()),
        |json| {
            serde_json::from_str::<AppSettings>(&json)
                .map_err(|error| storage_corrupted(format!("stored settings are invalid: {error}")))
        },
    )?;
    let (route_id, preset_id, selected_provider_profile_id) = match (
        settings.selected_model_route_id,
        settings.selected_generation_preset_id,
        settings.selected_provider_profile_id,
    ) {
        (Some(route_id), Some(preset_id), profile_id) => (route_id, preset_id, profile_id),
        (None, None, Some(profile_id)) => (
            ModelRouteId::from(profile_id.clone()),
            GenerationPresetId::from(profile_id.clone()),
            Some(profile_id),
        ),
        (None, None, None) => return Ok(DiscoveryPreviousSelection::None),
        _ => {
            return Err(storage_corrupted(
                "stored provider route and preset selection are incomplete",
            ));
        }
    };
    let preset_route_id = connection
        .query_row(
            "SELECT model_route_id
             FROM generation_presets
             WHERE id = ?1",
            [preset_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("selected generation preset is missing"))?;
    if preset_route_id != route_id.as_str()
        || !row_exists(
            connection,
            "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
            route_id.as_str(),
        )?
    {
        return Err(storage_corrupted(
            "stored provider route and preset selection do not match",
        ));
    }
    if let Some(profile_id) = selected_provider_profile_id.as_deref()
        && legacy_profile_current_route_id_for_schema(connection, profile_id, false)? != route_id
    {
        return Err(storage_corrupted(
            "stored legacy provider profile does not own its selected route",
        ));
    }
    Ok(DiscoveryPreviousSelection::RouteAndPreset {
        selected_provider_profile_id,
        model_route_id: route_id,
        generation_preset_id: preset_id,
    })
}

pub(crate) fn restore_discovery_provider_selection(
    transaction: &rusqlite::Transaction<'_>,
    previous_selection: &DiscoveryPreviousSelection,
    expected_selection_revision: Option<u64>,
) -> CoreResult<()> {
    let Some(expected_selection_revision) = expected_selection_revision else {
        return Ok(());
    };
    if load_provider_selection_revision(transaction)? != expected_selection_revision {
        // A later user or CRUD selection intent wins. Compensation still
        // completes because preserving that newer intent is the safe outcome.
        return Ok(());
    }
    if !matches!(previous_selection, DiscoveryPreviousSelection::None)
        && !row_exists(
            transaction,
            "SELECT EXISTS(
                 SELECT 1 FROM app_settings WHERE key = ?1
             )",
            "application",
        )?
    {
        return Err(CoreError::invalid(
            "previous discovery selection cannot be restored because settings are missing",
        ));
    }
    update_stored_settings_without_selection_revision(transaction, |settings| {
        let selection_is_clear = settings.selected_provider_profile_id.is_none()
            && settings.selected_model_route_id.is_none()
            && settings.selected_generation_preset_id.is_none();
        if !selection_is_clear {
            return Err(storage_corrupted(
                "discovery selection restore authority points to a non-clear selection",
            ));
        }
        match previous_selection {
            DiscoveryPreviousSelection::None => {}
            DiscoveryPreviousSelection::RouteAndPreset {
                selected_provider_profile_id,
                model_route_id,
                generation_preset_id,
            } => {
                let route_exists = row_exists(
                    transaction,
                    "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                    model_route_id.as_str(),
                )?;
                if !route_exists {
                    return Err(CoreError::invalid(
                        "previous discovery model route no longer exists",
                    ));
                }
                let preset_matches = transaction
                    .query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM generation_presets
                             WHERE id = ?1 AND model_route_id = ?2
                         )",
                        params![generation_preset_id.as_str(), model_route_id.as_str(),],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(storage_db_error)?;
                if !preset_matches {
                    return Err(CoreError::invalid(
                        "previous discovery generation preset no longer matches its route",
                    ));
                }
                let legacy_profile_id = if let Some(profile_id) = selected_provider_profile_id {
                    let current_route_id =
                        legacy_profile_current_route_id_for_schema(transaction, profile_id, false)?;
                    if current_route_id != *model_route_id {
                        return Err(CoreError::invalid(
                            "previous discovery legacy profile no longer owns its selected route",
                        ));
                    }
                    Some(profile_id.clone())
                } else {
                    (model_route_id.as_str() == generation_preset_id.as_str()
                        && row_exists(
                            transaction,
                            "SELECT EXISTS(SELECT 1 FROM provider_profiles WHERE id = ?1)",
                            model_route_id.as_str(),
                        )?)
                    .then(|| model_route_id.as_str().to_owned())
                };
                let already_restored = settings.selected_provider_profile_id == legacy_profile_id
                    && settings.selected_model_route_id.as_ref() == Some(model_route_id)
                    && settings.selected_generation_preset_id.as_ref()
                        == Some(generation_preset_id);
                if already_restored {
                    return Ok(());
                }
                settings.selected_provider_profile_id = legacy_profile_id;
                settings.selected_model_route_id = Some(model_route_id.clone());
                settings.selected_generation_preset_id = Some(generation_preset_id.clone());
            }
        }
        Ok(())
    })?;
    // Consume the CAS authority even when the previous value was already None.
    // This makes a replay observe a revision mismatch instead of reusing the
    // graph-removal decision.
    advance_provider_selection_revision(transaction)?;
    Ok(())
}

fn clear_provider_selections_for_route(
    transaction: &rusqlite::Transaction<'_>,
    route_id: &str,
    connection_id: &str,
) -> CoreResult<()> {
    update_stored_settings(transaction, |settings| {
        if settings
            .selected_model_route_id
            .as_ref()
            .is_some_and(|selected| selected.as_str() == route_id)
        {
            if settings.selected_provider_profile_id.as_deref() == Some(connection_id) {
                settings.selected_provider_profile_id = None;
            }
            settings.selected_model_route_id = None;
            settings.selected_generation_preset_id = None;
        }
        Ok(())
    })
}

fn clear_provider_selections_for_preset(
    transaction: &rusqlite::Transaction<'_>,
    preset_id: &str,
    route_id: &str,
) -> CoreResult<()> {
    update_stored_settings(transaction, |settings| {
        if settings
            .selected_generation_preset_id
            .as_ref()
            .is_some_and(|selected| selected.as_str() == preset_id)
        {
            if settings.selected_provider_profile_id.as_deref() == Some(route_id) {
                settings.selected_provider_profile_id = None;
            }
            settings.selected_model_route_id = None;
            settings.selected_generation_preset_id = None;
        }
        Ok(())
    })
}

fn update_stored_settings(
    transaction: &rusqlite::Transaction<'_>,
    update: impl FnOnce(&mut AppSettings) -> CoreResult<()>,
) -> CoreResult<()> {
    if update_stored_settings_without_selection_revision(transaction, update)? {
        advance_provider_selection_revision(transaction)?;
    }
    Ok(())
}

/// Applies an internal settings mutation and returns whether the provider
/// selection tuple changed. Callers must either advance the ordinary revision
/// or bind the resulting revision to discovery compensation before commit.
fn update_stored_settings_without_selection_revision(
    transaction: &rusqlite::Transaction<'_>,
    update: impl FnOnce(&mut AppSettings) -> CoreResult<()>,
) -> CoreResult<bool> {
    let settings_json = transaction
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some(settings_json) = settings_json else {
        return Ok(false);
    };
    let mut settings = serde_json::from_str::<AppSettings>(&settings_json)
        .map_err(|error| storage_corrupted(format!("stored settings are invalid: {error}")))?;
    let original = settings.clone();
    update(&mut settings)?;
    if settings == original {
        return Ok(false);
    }
    let selection_changed = provider_selection_changed(&original, &settings);
    write_application_settings(transaction, &settings)?;
    Ok(selection_changed)
}

fn provider_selection_changed(left: &AppSettings, right: &AppSettings) -> bool {
    left.selected_provider_profile_id != right.selected_provider_profile_id
        || left.selected_model_route_id != right.selected_model_route_id
        || left.selected_generation_preset_id != right.selected_generation_preset_id
}

fn write_application_settings(
    transaction: &rusqlite::Transaction<'_>,
    settings: &AppSettings,
) -> CoreResult<()> {
    let settings_json = serde_json::to_string(settings)
        .map_err(|error| CoreError::internal(format!("cannot encode settings: {error}")))?;
    transaction
        .execute(
            "INSERT INTO app_settings (key, value_json) VALUES ('application', ?1)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            [settings_json],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn load_provider_selection_revision(connection: &Connection) -> CoreResult<u64> {
    let revision = connection
        .query_row(
            "SELECT revision
             FROM provider_selection_state
             WHERE singleton_key = 'application'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("provider selection revision is missing"))?;
    i64_to_u64("provider selection revision", revision)
}

fn advance_provider_selection_revision(transaction: &rusqlite::Transaction<'_>) -> CoreResult<u64> {
    let changed = transaction
        .execute(
            "UPDATE provider_selection_state
             SET revision = revision + 1
             WHERE singleton_key = 'application'
               AND revision < 9223372036854775807",
            [],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "provider selection revision is missing or exhausted",
        ));
    }
    load_provider_selection_revision(transaction)
}

fn row_exists(connection: &Connection, query: &str, value: &str) -> CoreResult<bool> {
    connection
        .query_row(query, [value], |row| row.get::<_, bool>(0))
        .map_err(storage_db_error)
}

fn query_count<P: rusqlite::Params>(
    connection: &Connection,
    query: &str,
    params: P,
) -> CoreResult<u64> {
    let value = connection
        .query_row(query, params, |row| row.get::<_, i64>(0))
        .map_err(storage_db_error)?;
    u64::try_from(value).map_err(|_| storage_corrupted("database contains a negative row count"))
}

pub(crate) fn validate_provider_catalog_foreign_keys(connection: &Connection) -> CoreResult<()> {
    let violation = {
        let mut statement = connection
            .prepare("PRAGMA foreign_key_check")
            .map_err(storage_db_error)?;
        statement
            .query_row([], |_| Ok(()))
            .optional()
            .map_err(storage_db_error)?
            .is_some()
    };
    if violation {
        Err(storage_corrupted(
            "provider catalog contains a foreign-key violation",
        ))
    } else {
        Ok(())
    }
}

fn not_found(kind: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{kind} was not found"),
        false,
    )
}

fn validate_legacy_messages_for_branch_migration(connection: &Connection) -> CoreResult<()> {
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

fn recover_interrupted_work(root: &Path, connection: &mut Connection) -> CoreResult<()> {
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

fn reject_durable_committing_package_imports(connection: &Connection) -> CoreResult<()> {
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

fn recover_package_cas_promotions(root: &Path, connection: &mut Connection) -> CoreResult<()> {
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

fn load_recovery_settings(connection: &Connection) -> CoreResult<AppSettings> {
    let settings = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .map_or_else(
            || Ok(AppSettings::default()),
            |value| {
                serde_json::from_str::<AppSettings>(&value).map_err(|error| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        format!("stored settings are invalid: {error}"),
                        false,
                    )
                })
            },
        )?;
    validate_local_user_id(&settings.local_user_id)?;
    Ok(settings)
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

fn load_interrupted_generation_closures(
    transaction: &rusqlite::Transaction<'_>,
) -> CoreResult<Vec<InterruptedGenerationClosure>> {
    load_raw_interrupted_generation_closures(transaction)?
        .into_iter()
        .map(|raw| validate_interrupted_generation_closure(transaction, raw))
        .collect()
}

fn load_raw_interrupted_generation_closures(
    transaction: &rusqlite::Transaction<'_>,
) -> CoreResult<Vec<RawInterruptedGenerationClosure>> {
    let mut statement = transaction
        .prepare(
            "SELECT generation.id, generation.conversation_id,
                    generation.branch_id, generation.user_message_id,
                    generation.assistant_message_id, generation.provider_family,
                    EXISTS(
                      SELECT 1
                      FROM generation_attempt_intents AS attempt
                      WHERE attempt.generation_id = generation.id
                    )
             FROM generations AS generation
             WHERE generation.status = 'running'
             ORDER BY generation.id",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([], |row| {
            Ok(RawInterruptedGenerationClosure {
                generation_id: row.get(0)?,
                conversation: row.get(1)?,
                branch: row.get(2)?,
                user_message: row.get(3)?,
                assistant_message: row.get(4)?,
                provider_family: row.get(5)?,
                attempt_present: row.get(6)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn validate_interrupted_generation_closure(
    transaction: &rusqlite::Transaction<'_>,
    raw: RawInterruptedGenerationClosure,
) -> CoreResult<InterruptedGenerationClosure> {
    let generation_id = GenerationId(raw.generation_id);
    let route = StoredGenerationRoute {
        conversation: raw.conversation,
        branch: raw.branch,
        user_message: raw.user_message,
        assistant_message: raw.assistant_message,
        provider_family: raw
            .provider_family
            .map(|value| str_to_api_family(&value))
            .transpose()?,
    };
    let assistant = load_interrupted_generation_assistant(transaction, &route)?;
    if raw.attempt_present {
        validate_interrupted_generation_attempt(
            transaction,
            &generation_id,
            &route,
            assistant.as_ref(),
        )?;
    }
    if let Some(assistant) = assistant.as_ref()
        && (assistant.conversation_id.0 != route.conversation
            || assistant.parent_id.as_ref().map(|id| id.0.as_str())
                != Some(route.user_message.as_str())
            || assistant.role != MessageRole::Assistant
            || assistant.status != MessageStatus::Pending
            || assistant.generation_id.as_ref() != Some(&generation_id))
    {
        return Err(storage_corrupted(
            "running generation assistant route is inconsistent",
        ));
    }
    Ok(InterruptedGenerationClosure {
        generation_id,
        route,
        assistant,
        attempt_present: raw.attempt_present,
    })
}

fn load_interrupted_generation_assistant(
    transaction: &rusqlite::Transaction<'_>,
    route: &StoredGenerationRoute,
) -> CoreResult<Option<Message>> {
    route
        .assistant_message
        .as_deref()
        .map(|assistant_message_id| {
            transaction
                .query_row(
                    "SELECT id, conversation_id, parent_id, role, content, status,
                            generation_id, created_at
                     FROM messages
                     WHERE id = ?1",
                    [assistant_message_id],
                    map_message,
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| storage_corrupted("running generation assistant message is missing"))
        })
        .transpose()
}

fn validate_interrupted_generation_attempt(
    transaction: &rusqlite::Transaction<'_>,
    generation_id: &GenerationId,
    route: &StoredGenerationRoute,
    assistant: Option<&Message>,
) -> CoreResult<()> {
    let assistant = assistant.ok_or_else(|| {
        storage_corrupted("running generation attempt is missing its assistant message route")
    })?;
    let attempt = crate::generation_attempt::read_attempt(transaction, generation_id)?;
    if attempt.status != crate::generation_attempt::GenerationAttemptStatus::Running
        || attempt.input.conversation_id.0 != route.conversation
        || attempt.input.proposed_branch_id.0 != route.branch
    {
        return Err(storage_corrupted(
            "running generation attempt route or status is inconsistent",
        ));
    }
    let exact_head = transaction
        .query_row(
            "SELECT head_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![route.conversation, route.branch],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("running generation branch route is missing"))?;
    if exact_head.as_deref() != Some(assistant.id.0.as_str()) {
        return Err(storage_corrupted(
            "running generation assistant is not the exact branch head",
        ));
    }
    Ok(())
}

fn ensure_stable_local_user_settings(connection: &mut Connection) -> CoreResult<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let _ = load_or_create_settings_in_transaction(&transaction)?;
    transaction.commit().map_err(storage_db_error)
}

fn load_or_create_settings_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> CoreResult<AppSettings> {
    let stored_json = transaction
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    let (settings, requires_rewrite) = match stored_json {
        None => (AppSettings::default(), true),
        Some(json) => {
            let raw = serde_json::from_str::<serde_json::Value>(&json).map_err(|error| {
                storage_corrupted(format!("stored settings are invalid JSON: {error}"))
            })?;
            let object = raw.as_object().ok_or_else(|| {
                storage_corrupted("stored application settings must be a JSON object")
            })?;
            let requires_rewrite = !object.contains_key("local_user_id");
            let settings = serde_json::from_value::<AppSettings>(raw).map_err(|error| {
                storage_corrupted(format!("stored settings are invalid: {error}"))
            })?;
            (settings, requires_rewrite)
        }
    };
    validate_local_user_id(&settings.local_user_id)?;
    if requires_rewrite {
        let json = serde_json::to_string(&settings)
            .map_err(|error| CoreError::internal(format!("cannot encode settings: {error}")))?;
        transaction
            .execute(
                "INSERT INTO app_settings (key, value_json)
                 VALUES ('application', ?1)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                [json],
            )
            .map_err(storage_db_error)?;
    }
    Ok(settings)
}

fn validate_local_user_id(local_user_id: &LocalUserId) -> CoreResult<()> {
    let parsed = Uuid::parse_str(local_user_id.as_str())
        .map_err(|_| storage_corrupted("stored local user id is not a canonical UUID"))?;
    if parsed.get_version_num() != 4
        || parsed.hyphenated().to_string() != local_user_id.as_str().to_ascii_lowercase()
    {
        return Err(storage_corrupted(
            "stored local user id must be a canonical UUID v4",
        ));
    }
    Ok(())
}

fn apply_recovery_transaction(
    connection: &mut Connection,
    jobs: &[InterruptedImport],
    settings: &AppSettings,
) -> CoreResult<()> {
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let interrupted_generations = load_interrupted_generation_closures(&transaction)?;
    for job in jobs {
        transaction
            .execute("DELETE FROM import_jobs WHERE id = ?1", [&job.id])
            .map_err(storage_db_error)?;
    }
    let recovered_at = Utc::now();
    let recovered_at_text = recovered_at.to_rfc3339();
    close_interrupted_generation_rows(
        &transaction,
        interrupted_generations.len(),
        &recovered_at_text,
    )?;
    recover_attempt_backed_generation_messages(
        &transaction,
        &interrupted_generations,
        &recovered_at_text,
    )?;
    recover_pending_generation_messages(
        &transaction,
        settings.preserve_partial_generations,
        &recovered_at_text,
    )?;
    close_interrupted_generation_attempts(
        &transaction,
        &interrupted_generations,
        recovered_at,
        &recovered_at_text,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(())
}

fn recover_attempt_backed_generation_messages(
    transaction: &rusqlite::Transaction<'_>,
    interrupted_generations: &[InterruptedGenerationClosure],
    recovered_at: &str,
) -> CoreResult<()> {
    for interrupted in interrupted_generations
        .iter()
        .filter(|interrupted| interrupted.attempt_present)
    {
        let assistant = interrupted.assistant.as_ref().ok_or_else(|| {
            storage_corrupted("running generation attempt assistant route is missing")
        })?;
        let mut terminal = assistant.clone();
        terminal.status = MessageStatus::Cancelled;
        persist_terminal_assistant(
            transaction,
            &terminal,
            &interrupted.generation_id,
            &interrupted.route,
            recovered_at,
            interrupted.has_durable_partial_checkpoint(),
        )?;
    }
    Ok(())
}

fn close_interrupted_generation_rows(
    transaction: &rusqlite::Transaction<'_>,
    expected_generations: usize,
    recovered_at: &str,
) -> CoreResult<()> {
    let recovered_generations = transaction
        .execute(
            "UPDATE generations
             SET status = 'cancelled',
                 input_tokens = NULL,
                 cached_read_tokens = NULL,
                 cached_write_tokens = NULL,
                 output_tokens = NULL,
                 reasoning_tokens = NULL,
                 tool_tokens = NULL,
                 provider_raw_summary_json = NULL,
                 opaque_reasoning_state_json = NULL,
                 error_code = ?1,
                 finished_at = ?2
             WHERE status = 'running'",
            params![CoreErrorCode::Cancelled.as_str(), recovered_at],
        )
        .map_err(storage_db_error)?;
    if recovered_generations != expected_generations {
        return Err(storage_corrupted(
            "running generation recovery set changed inside its transaction",
        ));
    }
    Ok(())
}

fn recover_pending_generation_messages(
    transaction: &rusqlite::Transaction<'_>,
    preserve_partial_generations: bool,
    recovered_at: &str,
) -> CoreResult<()> {
    if preserve_partial_generations {
        transaction
            .execute(
                "UPDATE messages SET status = 'cancelled' WHERE status = 'pending'",
                [],
            )
            .map_err(storage_db_error)?;
    } else {
        transaction
            .execute(
                "UPDATE conversation_branches
                 SET head_message_id = CASE
                       WHEN head_message_id IN (
                         SELECT id
                         FROM messages
                         WHERE role = 'assistant' AND status = 'pending'
                       )
                       THEN (
                         SELECT parent_id
                         FROM messages
                         WHERE messages.id = conversation_branches.head_message_id
                       )
                       ELSE head_message_id
                     END,
                     fork_message_id = CASE
                       WHEN fork_message_id IN (
                         SELECT id
                         FROM messages
                         WHERE role = 'assistant' AND status = 'pending'
                       )
                       THEN (
                         SELECT parent_id
                         FROM messages
                         WHERE messages.id = conversation_branches.fork_message_id
                       )
                       ELSE fork_message_id
                     END,
                     updated_at = ?1
                 WHERE head_message_id IN (
                         SELECT id
                         FROM messages
                         WHERE role = 'assistant' AND status = 'pending'
                       )
                    OR fork_message_id IN (
                         SELECT id
                         FROM messages
                         WHERE role = 'assistant' AND status = 'pending'
                       )",
                [recovered_at],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "UPDATE messages AS child
                 SET parent_id = (
                   SELECT pending.parent_id
                   FROM messages AS pending
                   WHERE pending.id = child.parent_id
                     AND pending.conversation_id = child.conversation_id
                     AND pending.role = 'assistant'
                     AND pending.status = 'pending'
                 )
                 WHERE child.parent_id IN (
                   SELECT id
                   FROM messages
                   WHERE role = 'assistant' AND status = 'pending'
                 )",
                [],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "DELETE FROM messages WHERE role = 'assistant' AND status = 'pending'",
                [],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn close_interrupted_generation_attempts(
    transaction: &rusqlite::Transaction<'_>,
    interrupted_generations: &[InterruptedGenerationClosure],
    recovered_at: DateTime<Utc>,
    recovered_at_text: &str,
) -> CoreResult<()> {
    for interrupted in interrupted_generations {
        let attempt_present =
            crate::generation_attempt::mark_attempt_completed_if_present_in_transaction(
                transaction,
                &interrupted.generation_id,
                recovered_at,
            )?;
        if attempt_present != interrupted.attempt_present {
            return Err(storage_corrupted(
                "running generation attempt set changed inside its recovery transaction",
            ));
        }
        let updated_conversations = transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![interrupted.route.conversation, recovered_at_text],
            )
            .map_err(storage_db_error)?;
        if updated_conversations != 1 {
            return Err(storage_corrupted(
                "running generation conversation route is missing",
            ));
        }
        if attempt_present {
            let assistant = interrupted.assistant.as_ref().ok_or_else(|| {
                storage_corrupted("running generation attempt assistant route is missing")
            })?;
            Storage::insert_generation_terminal_occurrences(
                transaction,
                assistant,
                &interrupted.generation_id,
                &interrupted.route,
                interrupted.has_durable_partial_checkpoint(),
                recovered_at,
            )?;
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

fn remove_abandoned_staging_files(staging: &Path) -> CoreResult<()> {
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

fn validate_generation_append(
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    user: &Message,
    assistant: &Message,
    generation: &GenerationRecord,
) -> CoreResult<()> {
    if user.role != MessageRole::User
        || user.status != MessageStatus::Complete
        || user.generation_id.is_some()
        || user.parent_id.as_ref() != expected_head
    {
        return Err(CoreError::invalid(
            "branch append requires a complete user message parented to the expected head",
        ));
    }
    if assistant.role != MessageRole::Assistant
        || assistant.status != MessageStatus::Pending
        || assistant.parent_id.as_ref() != Some(&user.id)
        || assistant.conversation_id != user.conversation_id
    {
        return Err(CoreError::invalid(
            "branch append requires a pending assistant child of the user message",
        ));
    }
    if generation.status != GenerationStatus::Running
        || generation.finished_at.is_some()
        || !generation.opaque_reasoning_state.is_empty()
        || generation.id
            != assistant.generation_id.clone().ok_or_else(|| {
                CoreError::invalid("pending assistant message requires a generation id")
            })?
        || generation.conversation_id != user.conversation_id
        || &generation.branch_id != branch_id
        || generation.user_message_id != user.id
        || generation.assistant_message_id.as_ref() != Some(&assistant.id)
    {
        return Err(CoreError::invalid(
            "generation record does not own the appended user and assistant messages",
        ));
    }
    Ok(())
}

fn validate_generation_prompt_plan_link(
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    user: &Message,
    generation: &GenerationRecord,
    prompt_plan: Option<&crate::orchestration::GenerationPromptPlanRecord>,
) -> CoreResult<()> {
    let Some(prompt_plan) = prompt_plan else {
        return Ok(());
    };
    let provider_family_matches_route = if generation.model_route_id.is_some() {
        generation.provider_family == Some(prompt_plan.provider_request.api_family)
    } else {
        // Legacy credential-backed profiles intentionally have no catalog
        // route/family provenance on the generation row. The immutable prompt
        // request snapshot still records the concrete wire family used.
        generation.provider_family.is_none()
    };
    if prompt_plan.generation_id != generation.id
        || prompt_plan.conversation_id != generation.conversation_id
        || &prompt_plan.branch_id != branch_id
        || prompt_plan.head_message_id.as_ref() != expected_head
        || prompt_plan.latest_user_message_id != user.id
        || prompt_plan.model_route_id != generation.model_route_id
        || prompt_plan.generation_preset_id != generation.generation_preset_id
        || !provider_family_matches_route
    {
        return Err(CoreError::invalid(
            "generation prompt plan does not match the appended generation",
        ));
    }
    Ok(())
}

fn generation_prompt_module_plan_sha256(
    prompt_plan: &crate::orchestration::GenerationPromptPlanRecord,
) -> CoreResult<Sha256Digest> {
    let value = prompt_plan
        .provider_request
        .mapping_diagnostics
        .value
        .get("module_plan_sha256");
    match value {
        None | Some(serde_json::Value::Null) => {
            Ok(lorepia_orchestration::no_applied_module_runtime_plan_sha256())
        }
        Some(serde_json::Value::String(value)) => {
            Sha256Digest::parse(value.to_owned()).map_err(CoreError::invalid)
        }
        Some(_) => Err(CoreError::invalid(
            "generation module plan diagnostic must be a SHA-256 string or null",
        )),
    }
}

fn load_message_generation_action_context(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    target_message_id: &MessageId,
    action: MessageGenerationAction,
) -> CoreResult<MessageGenerationActionContext> {
    let target = load_branch_action_target(
        connection,
        conversation_id,
        branch_id,
        expected_head,
        target_message_id,
    )?;
    message_generation_action_context_from_target(connection, conversation_id, target, action)
}

fn load_message_generation_action_identity_context(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    target_message_id: &MessageId,
    action: MessageGenerationAction,
) -> CoreResult<MessageGenerationActionContext> {
    let branch_conversation_id = connection
        .query_row(
            "SELECT conversation_id FROM conversation_branches WHERE id = ?1",
            [&branch_id.0],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found",
                false,
            )
        })?;
    if branch_conversation_id != conversation_id.0 {
        return Err(CoreError::new(
            CoreErrorCode::NotFound,
            "conversation branch was not found in the conversation",
            false,
        ));
    }
    let target = connection
        .query_row(
            "SELECT id, conversation_id, parent_id, role, content, status,
                    generation_id, created_at
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0, target_message_id.0],
            map_message,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "message was not found in the conversation",
                false,
            )
        })?;
    message_generation_action_context_from_target(connection, conversation_id, target, action)
}

fn message_generation_action_context_from_target(
    connection: &Connection,
    conversation_id: &ConversationId,
    target: Message,
    action: MessageGenerationAction,
) -> CoreResult<MessageGenerationActionContext> {
    match action {
        MessageGenerationAction::EditUser => {
            if target.role != MessageRole::User || target.status != MessageStatus::Complete {
                return Err(CoreError::invalid(
                    "only a complete user message can be edited",
                ));
            }
            Ok(MessageGenerationActionContext {
                fork_message_id: target.parent_id,
                user_text: target.content,
            })
        }
        MessageGenerationAction::RegenerateAssistant => {
            if target.role != MessageRole::Assistant {
                return Err(CoreError::invalid(
                    "only an assistant message can be regenerated",
                ));
            }
            if target.status == MessageStatus::Pending {
                return Err(active_generation_action_error());
            }
            let user_message_id = target.parent_id.ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "assistant message is missing its user parent",
                    false,
                )
            })?;
            let user = connection
                .query_row(
                    "SELECT id, conversation_id, parent_id, role, content, status,
                            generation_id, created_at
                     FROM messages
                     WHERE conversation_id = ?1 AND id = ?2",
                    params![conversation_id.0, user_message_id.0],
                    map_message,
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "assistant message user parent was not found",
                        false,
                    )
                })?;
            if user.role != MessageRole::User || user.status != MessageStatus::Complete {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "assistant message parent is not a complete user message",
                    false,
                ));
            }
            Ok(MessageGenerationActionContext {
                fork_message_id: user.parent_id,
                user_text: user.content,
            })
        }
    }
}

fn load_branch_action_target(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    target_message_id: &MessageId,
) -> CoreResult<Message> {
    validate_branch_action_snapshot(connection, conversation_id, branch_id, expected_head)?;

    connection
        .query_row(
            "WITH RECURSIVE lineage(
               id, conversation_id, parent_id, role, content, status,
               generation_id, created_at
             ) AS (
               SELECT messages.id, messages.conversation_id, messages.parent_id,
                      messages.role, messages.content, messages.status,
                      messages.generation_id, messages.created_at
               FROM conversation_branches
               JOIN messages
                 ON messages.conversation_id = conversation_branches.conversation_id
                AND messages.id = conversation_branches.head_message_id
               WHERE conversation_branches.id = ?1
               UNION
               SELECT parent.id, parent.conversation_id, parent.parent_id,
                      parent.role, parent.content, parent.status,
                      parent.generation_id, parent.created_at
               FROM messages AS parent
               JOIN lineage
                 ON parent.conversation_id = lineage.conversation_id
                AND parent.id = lineage.parent_id
             )
             SELECT id, conversation_id, parent_id, role, content, status,
                    generation_id, created_at
             FROM lineage
             WHERE id = ?2
             LIMIT 1",
            params![branch_id.0, target_message_id.0],
            map_message,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "message was not found in the selected branch",
                false,
            )
        })
}

fn validate_branch_action_snapshot(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
) -> CoreResult<()> {
    let branch = connection
        .query_row(
            "SELECT branches.conversation_id, branches.head_message_id,
                    state.active_branch_id
             FROM conversation_branches AS branches
             JOIN conversation_state AS state
               ON state.conversation_id = branches.conversation_id
             WHERE branches.id = ?1",
            [&branch_id.0],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found",
                false,
            )
        })?;
    if branch.0 != conversation_id.0 {
        return Err(CoreError::new(
            CoreErrorCode::NotFound,
            "conversation branch was not found in the conversation",
            false,
        ));
    }
    if branch.1.as_deref() != expected_head.map(|message_id| message_id.0.as_str())
        || branch.2 != branch_id.0
    {
        return Err(stale_branch_error());
    }
    if let Some(head_message_id) = branch.1.as_deref() {
        let status = connection
            .query_row(
                "SELECT status
                 FROM messages
                 WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id.0, head_message_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "conversation branch head was not found",
                    false,
                )
            })?;
        if str_to_status(&status, 0).map_err(storage_db_error)? == MessageStatus::Pending {
            return Err(active_generation_action_error());
        }
    }
    Ok(())
}

fn active_generation_action_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "message actions are unavailable while the branch is generating",
        true,
    )
}

fn insert_message(transaction: &rusqlite::Transaction<'_>, message: &Message) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO messages
             (id, conversation_id, parent_id, role, content, status, generation_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                message.id.0,
                message.conversation_id.0,
                message.parent_id.as_ref().map(|value| value.0.as_str()),
                role_to_str(message.role),
                message.content,
                status_to_str(message.status),
                message.generation_id.as_ref().map(|value| value.0.as_str()),
                message.created_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_generation(
    transaction: &rusqlite::Transaction<'_>,
    generation: &GenerationRecord,
    prompt_plan: Option<&crate::orchestration::GenerationPromptPlanLink>,
) -> CoreResult<()> {
    let opaque_reasoning_state =
        serialize_opaque_reasoning_state(&generation.opaque_reasoning_state)?;
    transaction
        .execute(
            "INSERT INTO generations
             (id, conversation_id, branch_id, user_message_id, assistant_message_id,
              mode, model, status, input_tokens, output_tokens, error_code,
              started_at, finished_at, model_route_id, generation_preset_id,
              provider_family, cached_read_tokens, cached_write_tokens,
              reasoning_tokens, tool_tokens, provider_raw_summary_json,
              opaque_reasoning_state_json, resolved_prompt_plan_id,
              prompt_plan_sha256, provider_request_snapshot_id)
             VALUES (
                 ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
                 ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24,
                 ?25
             )",
            params![
                generation.id.0,
                generation.conversation_id.0,
                generation.branch_id.0,
                generation.user_message_id.0,
                generation
                    .assistant_message_id
                    .as_ref()
                    .map(|value| value.0.as_str()),
                mode_to_str(generation.mode),
                generation.model,
                generation_status_to_str(generation.status),
                generation.input_tokens.map(u64_to_i64).transpose()?,
                generation.output_tokens.map(u64_to_i64).transpose()?,
                generation.error_code,
                generation.started_at.to_rfc3339(),
                generation.finished_at.map(|value| value.to_rfc3339()),
                generation.model_route_id.as_ref().map(ModelRouteId::as_str),
                generation
                    .generation_preset_id
                    .as_ref()
                    .map(GenerationPresetId::as_str),
                generation.provider_family.map(api_family_to_str),
                generation.cached_read_tokens.map(u64_to_i64).transpose()?,
                generation.cached_write_tokens.map(u64_to_i64).transpose()?,
                generation.reasoning_tokens.map(u64_to_i64).transpose()?,
                generation.tool_tokens.map(u64_to_i64).transpose()?,
                generation
                    .provider_raw_summary
                    .as_ref()
                    .map(BoundedJson::as_str),
                opaque_reasoning_state,
                prompt_plan.map(|link| link.plan_id.as_str()),
                prompt_plan.map(|link| link.plan_sha256.as_str()),
                prompt_plan.map(|link| link.provider_request_snapshot_id.as_str()),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn load_running_generation(
    transaction: &rusqlite::Transaction<'_>,
    generation_id: &GenerationId,
) -> CoreResult<StoredGenerationRoute> {
    transaction
        .query_row(
            "SELECT conversation_id, branch_id, user_message_id, assistant_message_id,
                    provider_family
             FROM generations
             WHERE id = ?1 AND status = 'running'",
            [&generation_id.0],
            |row| {
                Ok(StoredGenerationRoute {
                    conversation: row.get(0)?,
                    branch: row.get(1)?,
                    user_message: row.get(2)?,
                    assistant_message: row.get(3)?,
                    provider_family: row
                        .get::<_, Option<String>>(4)?
                        .map(|value| str_to_api_family_sql(&value, 4))
                        .transpose()?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "running generation was not found",
                false,
            )
        })
}

fn validate_generation_assistant_ownership(
    generation: &StoredGenerationRoute,
    assistant: &Message,
) -> CoreResult<()> {
    if generation.conversation != assistant.conversation_id.0
        || generation.assistant_message.as_deref() != Some(assistant.id.0.as_str())
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation assistant ownership is inconsistent",
            false,
        ));
    }
    Ok(())
}

fn persist_terminal_assistant(
    transaction: &rusqlite::Transaction<'_>,
    assistant: &Message,
    generation_id: &GenerationId,
    generation: &StoredGenerationRoute,
    finished_at: &str,
    keep_assistant: bool,
) -> CoreResult<()> {
    if keep_assistant {
        let changed = transaction
            .execute(
                "UPDATE messages
                 SET content = ?3, status = ?4
                 WHERE id = ?1
                   AND generation_id = ?2
                   AND role = 'assistant'
                   AND status = 'pending'",
                params![
                    assistant.id.0,
                    generation_id.0,
                    assistant.content,
                    status_to_str(assistant.status)
                ],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            return Ok(());
        }
        return Err(CoreError::new(
            CoreErrorCode::NotFound,
            "pending assistant finalization target was not found",
            false,
        ));
    }
    transaction
        .execute(
            "UPDATE conversation_branches
             SET head_message_id = ?3, updated_at = ?4
             WHERE id = ?1
               AND conversation_id = ?2
               AND head_message_id = ?5",
            params![
                generation.branch,
                generation.conversation,
                generation.user_message,
                finished_at,
                assistant.id.0
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute("DELETE FROM messages WHERE id = ?1", [&assistant.id.0])
        .map_err(storage_db_error)?;
    Ok(())
}

fn compensate_terminal_assistant(
    transaction: &rusqlite::Transaction<'_>,
    assistant: &Message,
    generation_id: &GenerationId,
    generation: &StoredGenerationRoute,
    finished_at: &str,
    keep_assistant: bool,
) -> CoreResult<()> {
    if keep_assistant {
        let changed = transaction
            .execute(
                "UPDATE messages
                 SET content = ?3, status = 'failed'
                 WHERE id = ?1
                   AND generation_id = ?2
                   AND role = 'assistant'
                   AND status = 'pending'",
                params![assistant.id.0, generation_id.0, assistant.content],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            return Ok(());
        }
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation assistant compensation target was not found",
            false,
        ));
    }
    let changed = transaction
        .execute(
            "UPDATE conversation_branches
             SET head_message_id = ?3, updated_at = ?4
             WHERE id = ?1
               AND conversation_id = ?2
               AND head_message_id = ?5",
            params![
                generation.branch,
                generation.conversation,
                generation.user_message,
                finished_at,
                assistant.id.0
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation branch compensation target was not found",
            false,
        ));
    }
    let changed = transaction
        .execute(
            "DELETE FROM messages
             WHERE id = ?1
               AND generation_id = ?2
               AND role = 'assistant'
               AND status = 'pending'",
            params![assistant.id.0, generation_id.0],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation assistant compensation target was not found",
            false,
        ));
    }
    Ok(())
}

fn stale_branch_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "conversation branch head changed; refresh before retrying",
        true,
    )
}

fn prepare_owned_data_root(root: &Path) -> CoreResult<PathBuf> {
    if root.as_os_str().is_empty() {
        return Err(CoreError::invalid("data root must not be empty"));
    }
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(storage_corrupted(
                "data root must be a real directory, not a file or symbolic link",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(storage_io_error)?;
        }
        Err(error) => return Err(storage_io_error(error)),
    }
    let metadata = fs::symlink_metadata(root).map_err(storage_io_error)?;
    if !metadata.file_type().is_dir() {
        return Err(storage_corrupted(
            "data root must be a real directory, not a file or symbolic link",
        ));
    }
    harden_private_path(root, true)?;
    fs::canonicalize(root).map_err(storage_io_error)
}

fn validate_owner_lock_file(file: &File) -> CoreResult<()> {
    let metadata = file.metadata().map_err(storage_io_error)?;
    if !metadata.file_type().is_file() {
        return Err(storage_corrupted(
            "data root owner lock is not a regular file",
        ));
    }
    Ok(())
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn reject_non_regular_owner_lock(path: &Path) -> CoreResult<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(storage_corrupted(
            "data root owner lock is not a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage_io_error(error)),
    }
}

fn data_root_already_owned() -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        "data root is already owned by another LorePia process",
        true,
    )
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn acquire_data_root_owner_lock(root: &Path) -> CoreResult<File> {
    use rustix::fs::{FlockOperation, Mode, OFlags, flock, open, openat};

    let lock_path = root.join(".lorepia-owner.lock");
    reject_non_regular_owner_lock(&lock_path)?;
    let root_fd = open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(rustix_storage_io_error)?;
    let lock_fd = openat(
        &root_fd,
        ".lorepia-owner.lock",
        OFlags::CREATE | OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(rustix_storage_io_error)?;
    let file = File::from(lock_fd);
    validate_owner_lock_file(&file)?;
    flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
        let error = std::io::Error::from_raw_os_error(error.raw_os_error());
        if error.kind() == std::io::ErrorKind::WouldBlock {
            data_root_already_owned()
        } else {
            storage_io_error(error)
        }
    })?;
    Ok(file)
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn rustix_storage_io_error(error: rustix::io::Errno) -> CoreError {
    storage_io_error(std::io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(windows)]
fn acquire_data_root_owner_lock(root: &Path) -> CoreResult<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    let lock_path = root.join(".lorepia-owner.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .share_mode(0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&lock_path)
        .map_err(|error| {
            if matches!(error.raw_os_error(), Some(32 | 33)) {
                data_root_already_owned()
            } else if fs::symlink_metadata(&lock_path)
                .is_ok_and(|metadata| !metadata.file_type().is_file())
            {
                storage_corrupted("data root owner lock is not a regular file")
            } else {
                storage_io_error(error)
            }
        })?;
    validate_owner_lock_file(&file)?;
    Ok(file)
}

#[cfg(not(any(
    target_os = "android",
    target_os = "linux",
    target_vendor = "apple",
    windows
)))]
fn acquire_data_root_owner_lock(root: &Path) -> CoreResult<File> {
    let _ = root;
    Err(CoreError::new(
        CoreErrorCode::StorageUnavailable,
        "exclusive data root ownership is not supported on this platform",
        false,
    ))
}

fn create_owned_directory_tree(root: &Path, relative: &Path) -> CoreResult<()> {
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            return Err(CoreError::internal(
                "owned storage directory must use a relative normal path",
            ));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(storage_corrupted(format!(
                    "owned storage path is not a real directory: {}",
                    current.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(storage_io_error)?;
                if let Some(parent) = current.parent() {
                    sync_directory(parent)?;
                }
            }
            Err(error) => return Err(storage_io_error(error)),
        }
        harden_private_path(&current, true)?;
    }
    sync_directory(&current)
}

/// Stores an exact CAS object and returns whether this call published the
/// destination. Existing or concurrently won destinations return `false`.
fn store_verified_source(
    source: &Path,
    final_path: &Path,
    cas_root: &Path,
    expected_sha256: &str,
    expected_size: u64,
) -> CoreResult<bool> {
    store_verified_source_observed(
        source,
        final_path,
        cas_root,
        expected_sha256,
        expected_size,
        || {},
    )
}

fn store_verified_source_observed(
    source: &Path,
    final_path: &Path,
    cas_root: &Path,
    expected_sha256: &str,
    expected_size: u64,
    before_file_copy: impl FnOnce(),
) -> CoreResult<bool> {
    let parent = final_path
        .parent()
        .ok_or_else(|| CoreError::internal("source path has no parent"))?;
    create_and_sync_cas_directory(cas_root, parent)?;

    match fs::symlink_metadata(final_path) {
        Ok(metadata) if metadata.file_type().is_file() => {
            verify_file(final_path, expected_sha256, expected_size)?;
            sync_file_and_parent(final_path, parent)?;
            return Ok(false);
        }
        Ok(_) => {
            return Err(storage_corrupted(
                "content-addressed destination is not a regular file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(storage_io_error(error)),
    }

    let temp_path = parent.join(format!(".{}.partial", Uuid::new_v4()));
    before_file_copy();
    let copy_result = copy_and_hash(source, &temp_path);
    let (actual_sha256, actual_size) = match copy_result {
        Ok(result) => result,
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            return Err(error);
        }
    };
    if actual_sha256 != expected_sha256 || actual_size != expected_size {
        let _ = fs::remove_file(&temp_path);
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "staging source changed while it was being committed",
            false,
        ));
    }

    let published = match publish_temp_noclobber(&temp_path, final_path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            fs::remove_file(&temp_path).map_err(storage_io_error)?;
            ensure_regular_file(final_path)?;
            verify_file(final_path, expected_sha256, expected_size)?;
            false
        }
        Err(error) => {
            let _ = fs::remove_file(&temp_path);
            return Err(storage_io_error(error));
        }
    };

    sync_file_and_parent(final_path, parent)?;
    Ok(published)
}

fn validate_owned_staged_file(root: &Path, candidate: &Path) -> CoreResult<PathBuf> {
    let metadata = fs::symlink_metadata(candidate).map_err(storage_io_error)?;
    if !metadata.file_type().is_file() {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "package promotion source is not a regular staged file",
            false,
        ));
    }
    let staging = fs::canonicalize(root.join("staging")).map_err(storage_io_error)?;
    let candidate = fs::canonicalize(candidate).map_err(storage_io_error)?;
    if candidate == staging || !candidate.starts_with(&staging) {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "package promotion source escaped the owned staging directory",
            false,
        ));
    }
    ensure_regular_file(&candidate)?;
    Ok(candidate)
}

fn verify_media_type_signature(path: &Path, media_type: &str) -> CoreResult<()> {
    let mut file = File::open(path).map_err(storage_io_error)?;
    verify_open_file_media_type_signature(&mut file, media_type)
}

fn verify_open_file_media_type_signature(file: &mut File, media_type: &str) -> CoreResult<()> {
    let normalized = media_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return Err(CoreError::invalid("package asset media type is empty"));
    }
    let mut header = [0_u8; 64];
    let read = file.read(&mut header).map_err(storage_io_error)?;
    let header = &header[..read];
    let starts = |prefix: &[u8]| header.starts_with(prefix);
    let matches = match normalized.as_str() {
        "application/octet-stream" => true,
        "image/png" => starts(b"\x89PNG\r\n\x1a\n"),
        "image/jpeg" => starts(b"\xff\xd8\xff"),
        "image/gif" => starts(b"GIF87a") || starts(b"GIF89a"),
        "image/webp" => header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WEBP",
        "image/bmp" => starts(b"BM"),
        "image/avif" => header.len() >= 12 && &header[4..8] == b"ftyp" && &header[8..12] == b"avif",
        "image/heic" | "image/heif" => {
            header.len() >= 12
                && &header[4..8] == b"ftyp"
                && matches!(&header[8..12], b"heic" | b"heix" | b"heif" | b"mif1")
        }
        "audio/mpeg" => {
            starts(b"ID3") || (header.len() >= 2 && header[0] == 0xff && header[1] & 0xe0 == 0xe0)
        }
        "audio/wav" | "audio/x-wav" => {
            header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WAVE"
        }
        "audio/ogg" | "video/ogg" | "application/ogg" => starts(b"OggS"),
        "audio/flac" => starts(b"fLaC"),
        "video/mp4" | "audio/mp4" => header.len() >= 12 && &header[4..8] == b"ftyp",
        "video/webm" | "audio/webm" => starts(b"\x1a\x45\xdf\xa3"),
        "application/pdf" => starts(b"%PDF-"),
        "application/zip" => {
            starts(b"PK\x03\x04") || starts(b"PK\x05\x06") || starts(b"PK\x07\x08")
        }
        "application/json" | "text/plain" | "text/markdown" | "text/csv" => {
            std::str::from_utf8(header).is_ok()
        }
        "image/svg+xml" => std::str::from_utf8(header).is_ok_and(|text| {
            let text = text.trim_start_matches(|character: char| character.is_whitespace());
            text.starts_with("<svg") || text.starts_with("<?xml")
        }),
        _ => {
            return Err(CoreError::invalid(format!(
                "package asset media type cannot be signature-validated: {normalized}"
            )));
        }
    };
    if !matches {
        return Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "package asset bytes do not match the reviewed media type",
            false,
        ));
    }
    Ok(())
}

fn validate_renderer_media_type(media_type: &str) -> CoreResult<()> {
    if matches!(
        media_type,
        "image/png"
            | "image/jpeg"
            | "image/gif"
            | "image/webp"
            | "image/avif"
            | "audio/mpeg"
            | "audio/wav"
            | "audio/ogg"
            | "video/mp4"
            | "video/webm"
    ) {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::UnsafeArchive,
            "asset media type is not allowed in the renderer",
            false,
        ))
    }
}

#[cfg(any(target_os = "android", target_os = "linux", target_vendor = "apple"))]
fn publish_temp_noclobber(temp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};

    renameat_with(CWD, temp_path, CWD, final_path, RenameFlags::NOREPLACE)
        .map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
}

#[cfg(not(any(target_os = "android", target_os = "linux", target_vendor = "apple")))]
fn publish_temp_noclobber(temp_path: &Path, final_path: &Path) -> std::io::Result<()> {
    fs::hard_link(temp_path, final_path)?;
    fs::remove_file(temp_path)
}

fn create_and_sync_cas_directory(cas_root: &Path, path: &Path) -> CoreResult<()> {
    let relative = path
        .strip_prefix(cas_root)
        .map_err(|_| CoreError::internal("CAS destination escaped its owned root"))?;
    if relative.components().count() != 1 {
        return Err(CoreError::internal(
            "CAS destination must have exactly one hash-prefix directory",
        ));
    }
    ensure_real_directory(cas_root)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(storage_corrupted(
                "CAS hash-prefix path is not a real directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(storage_io_error)?;
            sync_directory(cas_root)?;
        }
        Err(error) => return Err(storage_io_error(error)),
    }
    ensure_real_directory(path)?;
    sync_directory(path)
}

fn ensure_real_directory(path: &Path) -> CoreResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(storage_io_error)?;
    if !metadata.file_type().is_dir() {
        return Err(storage_corrupted(format!(
            "owned CAS path is not a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_regular_file(path: &Path) -> CoreResult<()> {
    let metadata = fs::symlink_metadata(path).map_err(storage_io_error)?;
    if !metadata.file_type().is_file() {
        return Err(storage_corrupted(
            "content-addressed destination is not a regular file",
        ));
    }
    Ok(())
}

fn sync_file_and_parent(file_path: &Path, parent: &Path) -> CoreResult<()> {
    sync_file(file_path).map_err(storage_io_error)?;
    sync_directory(parent)
}

#[cfg(not(windows))]
fn sync_file(path: &Path) -> std::io::Result<()> {
    File::open(path).and_then(|file| file.sync_all())
}

#[cfg(windows)]
fn sync_file(path: &Path) -> std::io::Result<()> {
    // FlushFileBuffers requires a handle with write access on Windows.
    OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> CoreResult<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(storage_io_error)
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> CoreResult<()> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let result = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .and_then(|directory| directory.sync_all());
    match result {
        Ok(()) => Ok(()),
        // Windows does not guarantee FlushFileBuffers support for directory
        // handles on every filesystem. The CAS file itself was synced above.
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::InvalidInput
                    | std::io::ErrorKind::PermissionDenied
                    | std::io::ErrorKind::Unsupported
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(storage_io_error(error)),
    }
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(_path: &Path) -> CoreResult<()> {
    Ok(())
}

fn copy_and_hash(source: &Path, destination: &Path) -> CoreResult<(String, u64)> {
    let source = File::open(source).map_err(storage_io_error)?;
    let mut destination_options = OpenOptions::new();
    destination_options.create_new(true).write(true);
    #[cfg(unix)]
    destination_options.mode(0o600);
    let destination = destination_options
        .open(destination)
        .map_err(storage_io_error)?;
    let mut reader = BufReader::new(source);
    let mut writer = destination;
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(storage_io_error)?;
        if read == 0 {
            break;
        }
        writer
            .write_all(&buffer[..read])
            .map_err(storage_io_error)?;
        digest.update(&buffer[..read]);
        size = size
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| CoreError::internal("source byte count overflow"))?,
            )
            .ok_or_else(|| CoreError::internal("source size overflow"))?;
    }
    writer.flush().map_err(storage_io_error)?;
    writer.sync_all().map_err(storage_io_error)?;
    Ok((hex::encode(digest.finalize()), size))
}

fn verify_file(path: &Path, expected_sha256: &str, expected_size: u64) -> CoreResult<()> {
    let (actual_sha256, actual_size) = hash_file(path)?;
    if actual_sha256 != expected_sha256 || actual_size != expected_size {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "content-addressed source does not match its recorded digest",
            false,
        ));
    }
    Ok(())
}

fn hash_file(path: &Path) -> CoreResult<(String, u64)> {
    let mut source = File::open(path).map_err(storage_io_error)?;
    hash_open_file(&mut source)
}

fn hash_open_file(source: &mut File) -> CoreResult<(String, u64)> {
    source.rewind().map_err(storage_io_error)?;
    let mut reader = BufReader::new(source);
    let mut digest = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(storage_io_error)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        size = size
            .checked_add(
                u64::try_from(read)
                    .map_err(|_| CoreError::internal("source byte count overflow"))?,
            )
            .ok_or_else(|| CoreError::internal("source size overflow"))?;
    }
    Ok((hex::encode(digest.finalize()), size))
}

fn content_relative_path(hash: &str) -> CoreResult<String> {
    if hash.len() != 64 || !hash.bytes().all(|value| value.is_ascii_hexdigit()) {
        return Err(CoreError::invalid(
            "source hash is not a SHA-256 hex digest",
        ));
    }
    Ok(format!("sha256/{}/{}", &hash[..2], &hash[2..]))
}

fn map_character(row: &rusqlite::Row<'_>) -> rusqlite::Result<Character> {
    Ok(Character {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        source_hash: row.get(3)?,
        avatar_asset_hash: row.get(4)?,
        created_at: parse_datetime_sql(row.get::<_, String>(5)?, 5)?,
    })
}

fn map_provider_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderProfile> {
    let timeout_seconds = row.get::<_, i64>(4)?;
    let timeout_seconds = u32::try_from(timeout_seconds).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    Ok(ProviderProfile {
        id: row.get(0)?,
        display_name: row.get(1)?,
        base_url: row.get(2)?,
        model: row.get(3)?,
        timeout_seconds,
    })
}

fn decode_provider_template_row(
    row: (String, i64, String, String, String, String),
) -> CoreResult<ProviderTemplate> {
    let (id, version, display_name, source, manifest_json, manifest_sha256) = row;
    let version = u32::try_from(version)
        .map_err(|_| storage_corrupted("stored provider template version is invalid"))?;
    let actual_sha256 = hex::encode(Sha256::digest(manifest_json.as_bytes()));
    if actual_sha256 != manifest_sha256 {
        return Err(storage_corrupted(
            "stored provider template manifest hash does not match its content",
        ));
    }
    let template = serde_json::from_str::<ProviderTemplate>(&manifest_json).map_err(|error| {
        storage_corrupted(format!("stored provider template is invalid: {error}"))
    })?;
    let source = str_to_template_source(&source)?;
    if template.id.as_str() != id
        || template.manifest_version != version
        || template.display_name != display_name
        || template.source != source
        || template.api_family != template.default_manifest.api_family
    {
        return Err(storage_corrupted(
            "stored provider template columns do not match its typed manifest",
        ));
    }
    Ok(template)
}

pub(crate) fn provider_connection_columns(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ProviderConnectionRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

pub(crate) fn decode_provider_connection_row(
    row: ProviderConnectionRow,
) -> CoreResult<ProviderConnection> {
    let (
        id,
        template_id,
        template_version,
        display_name,
        api_origin,
        config_json,
        credential_ref,
        credential_scope_json,
        timeout_seconds,
        status,
        created_at,
        updated_at,
    ) = row;
    let template_version = u32::try_from(template_version)
        .map_err(|_| storage_corrupted("stored provider template version is invalid"))?;
    let timeout_seconds = u32::try_from(timeout_seconds)
        .map_err(|_| storage_corrupted("stored provider timeout is invalid"))?;
    if !(1..=600).contains(&timeout_seconds) {
        return Err(storage_corrupted(
            "stored provider timeout is outside the supported range",
        ));
    }
    let api_origin = CanonicalOrigin::parse(&api_origin).map_err(|error| {
        storage_corrupted(format!("stored provider API origin is invalid: {error}"))
    })?;
    let config = serde_json::from_str::<ConnectionConfig>(&config_json).map_err(|error| {
        storage_corrupted(format!(
            "stored provider connection config is invalid: {error}"
        ))
    })?;
    validate_connection_config(&config).map_err(stored_catalog_error)?;
    validate_provider_network_contract(&api_origin, &config).map_err(stored_catalog_error)?;
    let credential_scope = credential_scope_json
        .map(|json| {
            serde_json::from_str::<CredentialScope>(&json).map_err(|error| {
                storage_corrupted(format!("stored credential scope is invalid: {error}"))
            })
        })
        .transpose()?;
    if credential_ref.is_some() != credential_scope.is_some() {
        return Err(storage_corrupted(
            "stored credential reference and scope are inconsistent",
        ));
    }
    if let Some(scope) = credential_scope.as_ref()
        && (scope.allowed_origins.is_empty()
            || !scope
                .allowed_origins
                .iter()
                .any(|origin| origin == &api_origin))
    {
        return Err(storage_corrupted(
            "stored credential scope does not include the provider API origin",
        ));
    }
    let created_at = parse_stored_datetime(&created_at, "provider connection created_at")?;
    let updated_at = parse_stored_datetime(&updated_at, "provider connection updated_at")?;
    if updated_at < created_at {
        return Err(storage_corrupted(
            "stored provider connection timestamps are inconsistent",
        ));
    }
    Ok(ProviderConnection {
        id: ProviderConnectionId::from(id),
        template_id: ProviderTemplateId::from(template_id),
        template_version,
        display_name,
        api_origin,
        config,
        credential_ref: credential_ref.map(CredentialRef),
        credential_scope,
        timeout_seconds,
        status: str_to_connection_status(&status)?,
        created_at,
        updated_at,
    })
}

fn model_route_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelRouteRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
    ))
}

pub(crate) fn load_model_routes_for_reconciliation(
    transaction: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<Vec<ModelRoute>> {
    let mut statement = transaction
        .prepare(
            "SELECT id, connection_id, api_family, model_id, display_name, route_json,
                    availability, raw_metadata_json, miss_count, metadata_source_kind,
                    metadata_observed_at, last_reconciled_sync_job_id,
                    metadata_sync_job_id, first_seen_at, last_seen_at
             FROM provider_models WHERE connection_id = ?1
             ORDER BY id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([connection_id.as_str()], model_route_columns)
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter().map(decode_model_route_row).collect()
}

fn decode_model_route_row(row: ModelRouteRow) -> CoreResult<ModelRoute> {
    let (
        id,
        connection_id,
        api_family,
        model_id,
        display_name,
        route_json,
        availability,
        raw_metadata_json,
        miss_count,
        metadata_source_kind,
        metadata_observed_at,
        last_reconciled_sync_job_id,
        metadata_sync_job_id,
        first_seen_at,
        last_seen_at,
    ) = row;
    validate_nonempty("stored model id", &model_id).map_err(stored_catalog_error)?;
    if display_name
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(storage_corrupted(
            "stored model route display name is empty",
        ));
    }
    let route_config = serde_json::from_str::<ModelRouteConfig>(&route_json).map_err(|error| {
        storage_corrupted(format!("stored model route config is invalid: {error}"))
    })?;
    let raw_metadata = raw_metadata_json
        .map(lorepia_domain::BoundedJson::parse)
        .transpose()
        .map_err(|error| {
            storage_corrupted(format!("stored model route metadata is invalid: {error}"))
        })?;
    let miss_count = u32::try_from(miss_count)
        .map_err(|_| storage_corrupted("stored model route miss count is invalid"))?;
    let metadata_source = str_to_model_metadata_source(&metadata_source_kind)?;
    let metadata_observed_at = metadata_observed_at
        .map(|value| parse_stored_datetime(&value, "model route metadata_observed_at"))
        .transpose()?;
    validate_route_config(&route_config).map_err(stored_catalog_error)?;
    let first_seen_at = parse_stored_datetime(&first_seen_at, "model route first_seen_at")?;
    let last_seen_at = last_seen_at
        .map(|value| parse_stored_datetime(&value, "model route last_seen_at"))
        .transpose()?;
    if last_seen_at.is_some_and(|value| value < first_seen_at) {
        return Err(storage_corrupted(
            "stored model route timestamps are inconsistent",
        ));
    }
    Ok(ModelRoute {
        id: ModelRouteId::from(id),
        connection_id: ProviderConnectionId::from(connection_id),
        api_family: str_to_api_family(&api_family)?,
        model_id,
        display_name,
        route_config,
        status: str_to_model_availability(&availability)?,
        miss_count,
        raw_metadata,
        metadata_source,
        metadata_observed_at,
        last_reconciled_sync_job_id: last_reconciled_sync_job_id.map(ModelSyncJobId::from),
        metadata_sync_job_id: metadata_sync_job_id.map(ModelSyncJobId::from),
        first_seen_at,
        last_seen_at,
    })
}

fn capability_observation_columns(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<CapabilityObservationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
    ))
}

fn decode_capability_observation_row(
    row: CapabilityObservationRow,
) -> CoreResult<CapabilityObservation> {
    let (
        id,
        model_route_id,
        key,
        value_json,
        status,
        source,
        confidence,
        evidence_ref,
        observed_at,
        expires_at,
    ) = row;
    let id = ObservationId::from(id);
    let model_route_id = ModelRouteId::from(model_route_id);
    let key = str_to_capability_key(&key)?;
    let value = serde_json::from_str::<CapabilityValue>(&value_json).map_err(|error| {
        storage_corrupted(format!(
            "stored capability observation value is invalid: {error}"
        ))
    })?;
    let status = str_to_support_status(&status)?;
    let source = str_to_observation_source(&source)?;
    let confidence = str_to_confidence(&confidence)?;
    let observed_at = parse_stored_datetime(&observed_at, "capability observed_at")?;
    let expires_at = expires_at
        .map(|value| parse_stored_datetime(&value, "capability expires_at"))
        .transpose()?;
    let observation = CapabilityObservation {
        id,
        model_route_id,
        key,
        value,
        status,
        source,
        confidence,
        observed_at,
        expires_at,
        evidence_ref: evidence_ref.map(EvidenceId::from),
    };
    if observation
        .expires_at
        .is_some_and(|expires_at| expires_at <= observation.observed_at)
    {
        return Err(storage_corrupted(
            "stored capability observation timestamps are inconsistent",
        ));
    }
    if observation.status == SupportStatus::Unsupported
        && observation.value != CapabilityValue::Boolean(false)
    {
        return Err(storage_corrupted(
            "stored unsupported capability observation has a non-false value",
        ));
    }
    validate_capability_value(observation.key, &observation.value).map_err(stored_catalog_error)?;
    Ok(observation)
}

fn generation_preset_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<GenerationPresetRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn decode_generation_preset_row(row: GenerationPresetRow) -> CoreResult<GenerationPreset> {
    let (id, model_route_id, display_name, values_json, created_at, updated_at) = row;
    validate_nonempty("stored generation preset display name", &display_name)
        .map_err(stored_catalog_error)?;
    let (values, reasoning, prompt_cache) = decode_generation_preset_values(&values_json)?;
    let mut parameter_ids = std::collections::BTreeSet::new();
    if values
        .iter()
        .any(|value| !parameter_ids.insert(value.parameter_id.as_str()))
    {
        return Err(storage_corrupted(
            "stored generation preset contains duplicate parameter identifiers",
        ));
    }
    let created_at = parse_stored_datetime(&created_at, "generation preset created_at")?;
    let updated_at = parse_stored_datetime(&updated_at, "generation preset updated_at")?;
    if updated_at < created_at {
        return Err(storage_corrupted(
            "stored generation preset timestamps are inconsistent",
        ));
    }
    Ok(GenerationPreset {
        id: GenerationPresetId::from(id),
        model_route_id: ModelRouteId::from(model_route_id),
        display_name,
        values,
        reasoning,
        prompt_cache,
        created_at,
        updated_at,
    })
}

fn decode_generation_preset_values(
    values_json: &str,
) -> CoreResult<(
    Vec<ParameterValue>,
    GenerationReasoningSettings,
    GenerationPromptCacheSettings,
)> {
    let value = serde_json::from_str::<serde_json::Value>(values_json).map_err(|error| {
        storage_corrupted(format!(
            "stored generation preset values are invalid: {error}"
        ))
    })?;
    if value.is_array() {
        let values = serde_json::from_value(value).map_err(|error| {
            storage_corrupted(format!(
                "stored legacy generation preset values are invalid: {error}"
            ))
        })?;
        return Ok((
            values,
            GenerationReasoningSettings::default(),
            GenerationPromptCacheSettings::default(),
        ));
    }
    let object = value.as_object().ok_or_else(|| {
        storage_corrupted("stored generation preset values must be an object or legacy array")
    })?;
    let expected_keys = ["schema_version", "values", "reasoning", "prompt_cache"];
    if object.len() != expected_keys.len()
        || expected_keys.iter().any(|key| !object.contains_key(*key))
        || object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
    {
        return Err(storage_corrupted(
            "stored generation preset values use an unsupported schema",
        ));
    }
    let values = serde_json::from_value(object["values"].clone()).map_err(|error| {
        storage_corrupted(format!(
            "stored generation preset parameter values are invalid: {error}"
        ))
    })?;
    let reasoning = serde_json::from_value(object["reasoning"].clone()).map_err(|error| {
        storage_corrupted(format!(
            "stored generation preset reasoning settings are invalid: {error}"
        ))
    })?;
    let prompt_cache = serde_json::from_value(object["prompt_cache"].clone()).map_err(|error| {
        storage_corrupted(format!(
            "stored generation preset prompt-cache settings are invalid: {error}"
        ))
    })?;
    Ok((values, reasoning, prompt_cache))
}

fn parse_stored_datetime(value: &str, label: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| storage_corrupted(format!("stored {label} is invalid: {error}")))
}

fn stored_catalog_error(error: CoreError) -> CoreError {
    if error.code == CoreErrorCode::StorageCorrupted {
        error
    } else {
        storage_corrupted(format!(
            "stored provider catalog data is invalid: {}",
            error.message
        ))
    }
}

const fn template_source_to_str(source: TemplateSource) -> &'static str {
    match source {
        TemplateSource::BuiltIn => "built_in",
        TemplateSource::SignedCatalog => "signed_catalog",
        TemplateSource::UserDiscovered => "user_discovered",
    }
}

fn str_to_template_source(value: &str) -> CoreResult<TemplateSource> {
    match value {
        "built_in" => Ok(TemplateSource::BuiltIn),
        "signed_catalog" => Ok(TemplateSource::SignedCatalog),
        "user_discovered" => Ok(TemplateSource::UserDiscovered),
        _ => Err(storage_corrupted(format!(
            "stored provider template source is invalid: {value}"
        ))),
    }
}

const fn api_family_to_str(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

fn str_to_api_family(value: &str) -> CoreResult<ApiFamily> {
    match value {
        "openai_responses" => Ok(ApiFamily::OpenAiResponses),
        "openai_chat_completions" => Ok(ApiFamily::OpenAiChatCompletions),
        "anthropic_messages" => Ok(ApiFamily::AnthropicMessages),
        "gemini_generate_content" => Ok(ApiFamily::GeminiGenerateContent),
        "ollama_native" => Ok(ApiFamily::OllamaNative),
        _ => Err(storage_corrupted(format!(
            "stored provider API family is invalid: {value}"
        ))),
    }
}

const fn connection_status_to_str(status: ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Untested => "untested",
        ConnectionStatus::Connected => "connected",
        ConnectionStatus::AuthFailed => "auth_failed",
        ConnectionStatus::Unavailable => "unavailable",
    }
}

fn str_to_connection_status(value: &str) -> CoreResult<ConnectionStatus> {
    match value {
        "untested" => Ok(ConnectionStatus::Untested),
        "connected" => Ok(ConnectionStatus::Connected),
        "auth_failed" => Ok(ConnectionStatus::AuthFailed),
        "unavailable" => Ok(ConnectionStatus::Unavailable),
        _ => Err(storage_corrupted(format!(
            "stored provider connection status is invalid: {value}"
        ))),
    }
}

const fn model_availability_to_str(status: ModelAvailability) -> &'static str {
    match status {
        ModelAvailability::Available => "available",
        ModelAvailability::MissingTemporarily => "missing_temporarily",
        ModelAvailability::DocumentedOnly => "documented_only",
        ModelAvailability::AccessDenied => "access_denied",
        ModelAvailability::Deprecated => "deprecated",
        ModelAvailability::Retired => "retired",
        ModelAvailability::Unknown => "unknown",
    }
}

fn str_to_model_availability(value: &str) -> CoreResult<ModelAvailability> {
    match value {
        "available" => Ok(ModelAvailability::Available),
        "missing_temporarily" => Ok(ModelAvailability::MissingTemporarily),
        "documented_only" => Ok(ModelAvailability::DocumentedOnly),
        "access_denied" => Ok(ModelAvailability::AccessDenied),
        "deprecated" => Ok(ModelAvailability::Deprecated),
        "retired" => Ok(ModelAvailability::Retired),
        "unknown" => Ok(ModelAvailability::Unknown),
        _ => Err(storage_corrupted(format!(
            "stored model availability is invalid: {value}"
        ))),
    }
}

pub(crate) const fn model_metadata_source_to_str(source: ModelMetadataSource) -> &'static str {
    match source {
        ModelMetadataSource::Legacy => "legacy",
        ModelMetadataSource::ProviderApi => "provider_api",
        ModelMetadataSource::OfficialDocumentation => "official_documentation",
        ModelMetadataSource::SignedCatalog => "signed_catalog",
        ModelMetadataSource::CapabilityProbe => "capability_probe",
        ModelMetadataSource::UserOverride => "user_override",
    }
}

fn str_to_model_metadata_source(value: &str) -> CoreResult<ModelMetadataSource> {
    match value {
        "legacy" => Ok(ModelMetadataSource::Legacy),
        "provider_api" => Ok(ModelMetadataSource::ProviderApi),
        "official_documentation" => Ok(ModelMetadataSource::OfficialDocumentation),
        "signed_catalog" => Ok(ModelMetadataSource::SignedCatalog),
        "capability_probe" => Ok(ModelMetadataSource::CapabilityProbe),
        "user_override" => Ok(ModelMetadataSource::UserOverride),
        _ => Err(storage_corrupted(format!(
            "stored model metadata source is invalid: {value}"
        ))),
    }
}

const fn capability_key_to_str(key: CapabilityKey) -> &'static str {
    match key {
        CapabilityKey::Streaming => "streaming",
        CapabilityKey::Reasoning => "reasoning",
        CapabilityKey::PromptCaching => "prompt_caching",
        CapabilityKey::ToolCalling => "tool_calling",
        CapabilityKey::ParallelToolCalling => "parallel_tool_calling",
        CapabilityKey::StructuredOutput => "structured_output",
        CapabilityKey::JsonMode => "json_mode",
        CapabilityKey::ImageInput => "image_input",
        CapabilityKey::AudioInput => "audio_input",
        CapabilityKey::AudioOutput => "audio_output",
        CapabilityKey::Logprobs => "logprobs",
        CapabilityKey::Seed => "seed",
        CapabilityKey::Batch => "batch",
        CapabilityKey::Background => "background",
        CapabilityKey::ContextWindow => "context_window",
        CapabilityKey::MaxOutputTokens => "max_output_tokens",
    }
}

fn str_to_capability_key(value: &str) -> CoreResult<CapabilityKey> {
    match value {
        "streaming" => Ok(CapabilityKey::Streaming),
        "reasoning" => Ok(CapabilityKey::Reasoning),
        "prompt_caching" => Ok(CapabilityKey::PromptCaching),
        "tool_calling" => Ok(CapabilityKey::ToolCalling),
        "parallel_tool_calling" => Ok(CapabilityKey::ParallelToolCalling),
        "structured_output" => Ok(CapabilityKey::StructuredOutput),
        "json_mode" => Ok(CapabilityKey::JsonMode),
        "image_input" => Ok(CapabilityKey::ImageInput),
        "audio_input" => Ok(CapabilityKey::AudioInput),
        "audio_output" => Ok(CapabilityKey::AudioOutput),
        "logprobs" => Ok(CapabilityKey::Logprobs),
        "seed" => Ok(CapabilityKey::Seed),
        "batch" => Ok(CapabilityKey::Batch),
        "background" => Ok(CapabilityKey::Background),
        "context_window" => Ok(CapabilityKey::ContextWindow),
        "max_output_tokens" => Ok(CapabilityKey::MaxOutputTokens),
        _ => Err(storage_corrupted(format!(
            "stored capability key is invalid: {value}"
        ))),
    }
}

const fn support_status_to_str(status: SupportStatus) -> &'static str {
    match status {
        SupportStatus::Verified => "verified",
        SupportStatus::Documented => "documented",
        SupportStatus::Inferred => "inferred",
        SupportStatus::Unsupported => "unsupported",
        SupportStatus::Unknown => "unknown",
        SupportStatus::Conditional => "conditional",
    }
}

fn str_to_support_status(value: &str) -> CoreResult<SupportStatus> {
    match value {
        "verified" => Ok(SupportStatus::Verified),
        "documented" => Ok(SupportStatus::Documented),
        "inferred" => Ok(SupportStatus::Inferred),
        "unsupported" => Ok(SupportStatus::Unsupported),
        "unknown" => Ok(SupportStatus::Unknown),
        "conditional" => Ok(SupportStatus::Conditional),
        _ => Err(storage_corrupted(format!(
            "stored capability support status is invalid: {value}"
        ))),
    }
}

const fn observation_source_to_str(source: ObservationSource) -> &'static str {
    match source {
        ObservationSource::ProviderApi => "provider_api",
        ObservationSource::OfficialDocumentation => "official_documentation",
        ObservationSource::SignedLorepiaCatalog => "signed_lorepia_catalog",
        ObservationSource::CapabilityProbe => "capability_probe",
        ObservationSource::UserOverride => "user_override",
        ObservationSource::LlmInference => "llm_inference",
    }
}

fn str_to_observation_source(value: &str) -> CoreResult<ObservationSource> {
    match value {
        "provider_api" => Ok(ObservationSource::ProviderApi),
        "official_documentation" => Ok(ObservationSource::OfficialDocumentation),
        "signed_lorepia_catalog" => Ok(ObservationSource::SignedLorepiaCatalog),
        "capability_probe" => Ok(ObservationSource::CapabilityProbe),
        "user_override" => Ok(ObservationSource::UserOverride),
        "llm_inference" => Ok(ObservationSource::LlmInference),
        _ => Err(storage_corrupted(format!(
            "stored capability observation source is invalid: {value}"
        ))),
    }
}

const fn confidence_to_str(confidence: Confidence) -> &'static str {
    match confidence {
        Confidence::Low => "low",
        Confidence::Medium => "medium",
        Confidence::High => "high",
    }
}

fn str_to_confidence(value: &str) -> CoreResult<Confidence> {
    match value {
        "low" => Ok(Confidence::Low),
        "medium" => Ok(Confidence::Medium),
        "high" => Ok(Confidence::High),
        _ => Err(storage_corrupted(format!(
            "stored capability observation confidence is invalid: {value}"
        ))),
    }
}

fn map_conversation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: ConversationId(row.get(0)?),
        character_id: row.get(1)?,
        title: row.get(2)?,
        created_at: parse_datetime_sql(row.get::<_, String>(3)?, 3)?,
        updated_at: parse_datetime_sql(row.get::<_, String>(4)?, 4)?,
    })
}

fn map_conversation_branch(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationBranch> {
    Ok(ConversationBranch {
        id: ConversationBranchId(row.get(0)?),
        conversation_id: ConversationId(row.get(1)?),
        title: row.get(2)?,
        fork_message_id: row.get::<_, Option<String>>(3)?.map(MessageId),
        head_message_id: row.get::<_, Option<String>>(4)?.map(MessageId),
        created_at: parse_datetime_sql(row.get::<_, String>(5)?, 5)?,
        updated_at: parse_datetime_sql(row.get::<_, String>(6)?, 6)?,
    })
}

fn map_conversation_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationState> {
    let mode = row.get::<_, String>(2)?;
    Ok(ConversationState {
        conversation_id: ConversationId(row.get(0)?),
        active_branch_id: ConversationBranchId(row.get(1)?),
        selected_mode: str_to_mode(&mode, 2)?,
        updated_at: parse_datetime_sql(row.get::<_, String>(3)?, 3)?,
    })
}

fn map_generation(row: &rusqlite::Row<'_>) -> rusqlite::Result<GenerationRecord> {
    let mode = row.get::<_, String>(5)?;
    let status = row.get::<_, String>(7)?;
    let provider_family = row
        .get::<_, Option<String>>(15)?
        .map(|value| str_to_api_family_sql(&value, 15))
        .transpose()?;
    let provider_raw_summary = row
        .get::<_, Option<String>>(20)?
        .map(|value| {
            BoundedJson::parse(value)
                .map_err(|error| invalid_stored_text(20, "provider usage summary", &error))
        })
        .transpose()?;
    let opaque_reasoning_state = row
        .get::<_, Option<String>>(21)?
        .map(|value| deserialize_opaque_reasoning_state(&value, 21))
        .transpose()?
        .unwrap_or_default();
    if !opaque_reasoning_state_matches_provider_family(provider_family, &opaque_reasoning_state) {
        return Err(invalid_stored_text(
            21,
            "opaque reasoning state",
            "provider family binding is inconsistent",
        ));
    }
    Ok(GenerationRecord {
        id: GenerationId(row.get(0)?),
        conversation_id: ConversationId(row.get(1)?),
        branch_id: ConversationBranchId(row.get(2)?),
        user_message_id: MessageId(row.get(3)?),
        assistant_message_id: row.get::<_, Option<String>>(4)?.map(MessageId),
        mode: str_to_mode(&mode, 5)?,
        model: row.get(6)?,
        model_route_id: row.get::<_, Option<String>>(13)?.map(ModelRouteId::from),
        generation_preset_id: row
            .get::<_, Option<String>>(14)?
            .map(GenerationPresetId::from),
        provider_family,
        status: str_to_generation_status(&status, 7)?,
        input_tokens: optional_i64_to_u64_sql(row.get(8)?, 8)?,
        cached_read_tokens: optional_i64_to_u64_sql(row.get(16)?, 16)?,
        cached_write_tokens: optional_i64_to_u64_sql(row.get(17)?, 17)?,
        output_tokens: optional_i64_to_u64_sql(row.get(9)?, 9)?,
        reasoning_tokens: optional_i64_to_u64_sql(row.get(18)?, 18)?,
        tool_tokens: optional_i64_to_u64_sql(row.get(19)?, 19)?,
        provider_raw_summary,
        opaque_reasoning_state,
        error_code: row.get(10)?,
        started_at: parse_datetime_sql(row.get::<_, String>(11)?, 11)?,
        finished_at: row
            .get::<_, Option<String>>(12)?
            .map(|value| parse_datetime_sql(value, 12))
            .transpose()?,
    })
}

fn map_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let role: String = row.get(3)?;
    let status: String = row.get(5)?;
    Ok(Message {
        id: MessageId(row.get(0)?),
        conversation_id: ConversationId(row.get(1)?),
        parent_id: row.get::<_, Option<String>>(2)?.map(MessageId),
        role: str_to_role(&role, 3)?,
        content: row.get(4)?,
        status: str_to_status(&status, 5)?,
        generation_id: row.get::<_, Option<String>>(6)?.map(GenerationId),
        created_at: parse_datetime_sql(row.get::<_, String>(7)?, 7)?,
    })
}

fn active_character_content_revision(
    connection: &Connection,
    character_id: &str,
) -> CoreResult<Option<String>> {
    let revision_id = connection
        .query_row(
            "SELECT state.active_revision_id
             FROM character_content AS content
             JOIN content_objects AS object
               ON object.id = content.object_id
              AND object.object_kind = 'character_content'
              AND object.deleted_at IS NULL
             JOIN content_object_state AS state
               ON state.object_id = content.object_id
             JOIN character_content_revisions AS revision
               ON revision.object_id = content.object_id
              AND revision.revision_id = state.active_revision_id
             WHERE content.character_id = ?1",
            [character_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if revision_id.is_some() {
        return Ok(revision_id);
    }
    let character_exists = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM characters WHERE id = ?1)",
            [character_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if character_exists {
        Ok(None)
    } else {
        Err(CoreError::new(
            CoreErrorCode::NotFound,
            "character was not found",
            false,
        ))
    }
}

fn resolve_character_greeting(
    transaction: &rusqlite::Transaction<'_>,
    character_content_revision_id: Option<&str>,
    greeting_id: Option<&str>,
) -> CoreResult<Option<(String, String)>> {
    let Some(revision_id) = character_content_revision_id else {
        return if greeting_id.is_some() {
            Err(unavailable_character_greeting_error())
        } else {
            Ok(None)
        };
    };
    let selected = if let Some(greeting_id) = greeting_id {
        transaction
            .query_row(
                "SELECT greeting_id, content, enabled
                 FROM character_greetings
                 WHERE character_content_revision_id = ?1
                   AND greeting_id = ?2",
                params![revision_id, greeting_id],
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
    } else {
        transaction
            .query_row(
                "SELECT greeting_id, content, enabled
                 FROM character_greetings
                 WHERE character_content_revision_id = ?1
                   AND kind = 'default'
                   AND enabled = 1
                 ORDER BY ordinal, greeting_id
                 LIMIT 1",
                [revision_id],
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
    };
    match selected {
        Some((id, content, 1)) => Ok(Some((id, content))),
        Some(_) | None if greeting_id.is_some() => Err(unavailable_character_greeting_error()),
        Some(_) => Err(storage_corrupted(
            "enabled default character greeting projection is invalid",
        )),
        None => Ok(None),
    }
}

fn validate_character_greeting_id(greeting_id: &str) -> CoreResult<()> {
    if greeting_id.is_empty()
        || greeting_id.len() > 256
        || greeting_id.trim() != greeting_id
        || greeting_id.chars().any(char::is_control)
    {
        Err(CoreError::invalid("character greeting id is invalid"))
    } else {
        Ok(())
    }
}

fn stale_character_greeting_catalog_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "character greeting catalog is stale; refresh before creating the conversation",
        true,
    )
}

fn unavailable_character_greeting_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "character greeting is unavailable for the exact content revision",
        true,
    )
}

fn serialize_opaque_reasoning_state(states: &[OpaqueReasoningState]) -> CoreResult<Option<String>> {
    if states.is_empty() {
        return Ok(None);
    }
    validate_opaque_reasoning_states(states).map_err(CoreError::invalid)?;
    let json = serde_json::to_string(states)
        .map_err(|_| CoreError::invalid("opaque reasoning state could not be encoded"))?;
    if json.len() > MAX_OPAQUE_REASONING_SERIALIZED_BYTES {
        return Err(CoreError::invalid(
            "opaque reasoning state exceeds the stored JSON size limit",
        ));
    }
    Ok(Some(json))
}

fn serialize_opaque_reasoning_state_for_family(
    provider_family: Option<ApiFamily>,
    states: &[OpaqueReasoningState],
) -> CoreResult<Option<String>> {
    if !opaque_reasoning_state_matches_provider_family(provider_family, states) {
        return Err(CoreError::invalid(
            "opaque reasoning state does not match the generation provider family",
        ));
    }
    serialize_opaque_reasoning_state(states)
}

fn opaque_reasoning_state_matches_provider_family(
    provider_family: Option<ApiFamily>,
    states: &[OpaqueReasoningState],
) -> bool {
    states.is_empty()
        || provider_family.is_some_and(|provider_family| {
            states.iter().all(|state| {
                matches!(
                    (provider_family, state),
                    (
                        ApiFamily::OpenAiResponses,
                        OpaqueReasoningState::OpenAiResponses { .. }
                    ) | (
                        ApiFamily::OpenAiChatCompletions,
                        OpaqueReasoningState::OpenRouterReasoning { .. }
                    ) | (
                        ApiFamily::AnthropicMessages,
                        OpaqueReasoningState::AnthropicMessages { .. }
                    ) | (
                        ApiFamily::GeminiGenerateContent,
                        OpaqueReasoningState::GeminiThoughtSignature { .. }
                    )
                )
            })
        })
}

fn deserialize_opaque_reasoning_state(
    value: &str,
    column: usize,
) -> rusqlite::Result<Vec<OpaqueReasoningState>> {
    if value.len() > MAX_OPAQUE_REASONING_SERIALIZED_BYTES {
        return Err(invalid_stored_text(
            column,
            "opaque reasoning state",
            "stored JSON exceeds its size limit",
        ));
    }
    let states = serde_json::from_str::<Vec<OpaqueReasoningState>>(value).map_err(|_| {
        invalid_stored_text(
            column,
            "opaque reasoning state",
            "stored JSON failed typed validation",
        )
    })?;
    validate_opaque_reasoning_states(&states)
        .map_err(|error| invalid_stored_text(column, "opaque reasoning state", error.as_str()))?;
    Ok(states)
}

fn optional_i64_to_u64_sql(value: Option<i64>, column: usize) -> rusqlite::Result<Option<u64>> {
    value
        .map(|value| {
            u64::try_from(value).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    column,
                    rusqlite::types::Type::Integer,
                    Box::new(error),
                )
            })
        })
        .transpose()
}

fn str_to_api_family_sql(value: &str, column: usize) -> rusqlite::Result<ApiFamily> {
    match value {
        "openai_responses" => Ok(ApiFamily::OpenAiResponses),
        "openai_chat_completions" => Ok(ApiFamily::OpenAiChatCompletions),
        "anthropic_messages" => Ok(ApiFamily::AnthropicMessages),
        "gemini_generate_content" => Ok(ApiFamily::GeminiGenerateContent),
        "ollama_native" => Ok(ApiFamily::OllamaNative),
        other => Err(invalid_enum(column, "provider API family", other)),
    }
}

fn parse_datetime_sql(value: String, column: usize) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                column,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn role_to_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    }
}

fn str_to_role(value: &str, column: usize) -> rusqlite::Result<MessageRole> {
    match value {
        "system" => Ok(MessageRole::System),
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        other => Err(invalid_enum(column, "message role", other)),
    }
}

fn str_to_character_greeting_kind(
    value: &str,
    column: usize,
) -> rusqlite::Result<CharacterGreetingKind> {
    match value {
        "default" => Ok(CharacterGreetingKind::Default),
        "alternate" => Ok(CharacterGreetingKind::Alternate),
        other => Err(invalid_enum(column, "character greeting kind", other)),
    }
}

fn status_to_str(status: MessageStatus) -> &'static str {
    match status {
        MessageStatus::Pending => "pending",
        MessageStatus::Complete => "complete",
        MessageStatus::Cancelled => "cancelled",
        MessageStatus::Failed => "failed",
    }
}

const fn mode_to_str(mode: ConversationMode) -> &'static str {
    match mode {
        ConversationMode::Chat => "chat",
        ConversationMode::Story => "story",
    }
}

fn str_to_mode(value: &str, column: usize) -> rusqlite::Result<ConversationMode> {
    match value {
        "chat" => Ok(ConversationMode::Chat),
        "story" => Ok(ConversationMode::Story),
        other => Err(invalid_enum(column, "conversation mode", other)),
    }
}

const fn generation_status_to_str(status: GenerationStatus) -> &'static str {
    match status {
        GenerationStatus::Running => "running",
        GenerationStatus::Complete => "complete",
        GenerationStatus::Cancelled => "cancelled",
        GenerationStatus::Failed => "failed",
    }
}

fn str_to_generation_status(value: &str, column: usize) -> rusqlite::Result<GenerationStatus> {
    match value {
        "running" => Ok(GenerationStatus::Running),
        "complete" => Ok(GenerationStatus::Complete),
        "cancelled" => Ok(GenerationStatus::Cancelled),
        "failed" => Ok(GenerationStatus::Failed),
        other => Err(invalid_enum(column, "generation status", other)),
    }
}

const fn message_status_to_generation_status(status: MessageStatus) -> GenerationStatus {
    match status {
        MessageStatus::Pending => GenerationStatus::Running,
        MessageStatus::Complete => GenerationStatus::Complete,
        MessageStatus::Cancelled => GenerationStatus::Cancelled,
        MessageStatus::Failed => GenerationStatus::Failed,
    }
}

fn str_to_status(value: &str, column: usize) -> rusqlite::Result<MessageStatus> {
    match value {
        "pending" => Ok(MessageStatus::Pending),
        "complete" => Ok(MessageStatus::Complete),
        "cancelled" => Ok(MessageStatus::Cancelled),
        "failed" => Ok(MessageStatus::Failed),
        other => Err(invalid_enum(column, "message status", other)),
    }
}

fn invalid_enum(column: usize, kind: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        format!("invalid {kind}: {value}").into(),
    )
}

fn invalid_stored_text(column: usize, kind: &str, detail: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        rusqlite::types::Type::Text,
        format!("invalid stored {kind}: {detail}").into(),
    )
}

fn count(connection: &Connection, table: &str) -> CoreResult<u64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let value = connection
        .query_row(&sql, [], |row| row.get::<_, i64>(0))
        .map_err(storage_db_error)?;
    u64::try_from(value).map_err(|_| CoreError::internal("negative database row count"))
}

fn u64_to_i64(value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid("value exceeds SQLite integer range"))
}

fn i64_to_u64(label: &str, value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| storage_corrupted(format!("stored {label} is negative")))
}

fn storage_io_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("local storage operation failed: {error}"),
        true,
    )
}

fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

fn generation_read_error(error: rusqlite::Error) -> CoreError {
    if matches!(error, rusqlite::Error::FromSqlConversionFailure(_, _, _)) {
        storage_corrupted("stored generation data is invalid")
    } else {
        storage_db_error(error)
    }
}

pub(crate) fn storage_db_error(error: rusqlite::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("SQLite operation failed: {error}"),
        true,
    )
}

#[cfg(test)]
#[path = "database_legacy_recovery_tests.rs"]
mod legacy_recovery_tests;

#[cfg(test)]
mod tests {
    use std::{
        io::Write,
        process::Command,
        sync::{Arc, Barrier},
        thread,
    };

    use chrono::Duration;
    use tempfile::{NamedTempFile, tempdir};

    use crate::{ProviderCredentialObservedStatus, ProviderCredentialOperationKind};

    use super::*;

    const STORAGE_LOCK_PROBE_ROOT_ENV: &str = "LOREPIA_STORAGE_LOCK_PROBE_ROOT";
    const STORAGE_LOCK_PROBE_TEST_NAME: &str = "database::tests::storage_owner_lock_child_probe";
    const FROZEN_SCHEMA_ELEVEN_SQL: &str =
        include_str!("../../../testdata/tauri-upgrade/native-schema-11/schema-11.sql");
    const FROZEN_SOURCE_PACKAGE: &[u8] =
        include_bytes!("../../../testdata/packages/with-avatar.charx");
    const FROZEN_AVATAR_ASSET: &[u8] =
        include_bytes!("../../../testdata/tauri-upgrade/native-schema-11/assets/avatar.png");
    const FROZEN_SOURCE_SHA256: &str =
        "2c528a64fbf36a011e29c1a692cd13568b83f76e764ea03487393c28a2e666de";
    const FROZEN_AVATAR_SHA256: &str =
        "aa7bb0431aaeb198a77c26a14fe6dd714a75e4d7db94e3e1238a1fdcbfe1f8d4";

    fn staged_package_source(
        storage: &Storage,
        name: &str,
        bytes: &[u8],
        import_id: &str,
    ) -> (PathBuf, PackageCasPromotionIntent, PathBuf) {
        let staged = storage.staging_dir().join(name);
        fs::write(&staged, bytes).expect("write staged package source");
        let sha256 = hex::encode(Sha256::digest(bytes));
        let relative = content_relative_path(&sha256).expect("CAS relative path");
        let intent = PackageCasPromotionIntent {
            import_id: import_id.to_owned(),
            namespace: "source",
            sha256,
            size_bytes: u64::try_from(bytes.len()).expect("small package source"),
            media_type: None,
            relative_path: format!("sources/{relative}"),
        };
        let final_path = storage.data_root().join(&intent.relative_path);
        (staged, intent, final_path)
    }

    fn assert_abandoned_package_source_recovered(
        root: &Path,
        intent: &PackageCasPromotionIntent,
        final_path: &Path,
    ) {
        let reopened = Storage::open(root).expect("reopen storage for CAS recovery");
        assert!(!final_path.exists(), "abandoned CAS file must be removed");
        let connection = reopened.connection().expect("reopened database");
        let journal_count = connection
            .query_row(
                "SELECT COUNT(*) FROM package_cas_promotion_journal
                 WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
                params![intent.import_id, intent.namespace, intent.sha256],
                |row| row.get::<_, u64>(0),
            )
            .expect("journal count");
        assert_eq!(journal_count, 0);
        let source_count = connection
            .query_row(
                "SELECT COUNT(*) FROM content_sources WHERE sha256 = ?1",
                [intent.sha256.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .expect("source row count");
        assert_eq!(source_count, 0);
    }

    fn rollback_only_cas_promotion(
        namespace: &'static str,
        import_id: &str,
    ) -> (
        tempfile::TempDir,
        Storage,
        PackageCasPromotionIntent,
        PathBuf,
    ) {
        let root = tempdir().expect("temporary rollback-pinned CAS root");
        let database_path = root.path().join("db/lorepia.sqlite3");
        fs::create_dir_all(database_path.parent().expect("database parent"))
            .expect("create frozen database directory");
        let connection = Connection::open(&database_path).expect("create frozen database");
        connection
            .execute_batch(FROZEN_SCHEMA_ELEVEN_SQL)
            .expect("restore frozen schema-eleven fixture");

        for (directory, sha256, bytes) in [
            ("sources", FROZEN_SOURCE_SHA256, FROZEN_SOURCE_PACKAGE),
            ("assets", FROZEN_AVATAR_SHA256, FROZEN_AVATAR_ASSET),
        ] {
            let path = root
                .path()
                .join(directory)
                .join(content_relative_path(sha256).expect("frozen CAS path"));
            fs::create_dir_all(path.parent().expect("frozen CAS parent"))
                .expect("create frozen CAS directory");
            fs::write(path, bytes).expect("write frozen CAS object");
        }

        let bytes: &[u8] = match namespace {
            "source" => b"rollback-only package source",
            "asset" => b"\x89PNG\r\n\x1a\nrollback-only-asset",
            _ => panic!("unsupported rollback-only CAS namespace"),
        };
        let sha256 = hex::encode(Sha256::digest(bytes));
        let relative = content_relative_path(&sha256).expect("rollback-only CAS path");
        let (directory, media_type) = match namespace {
            "source" => ("sources", None),
            "asset" => ("assets", Some("image/png".to_owned())),
            _ => unreachable!("validated rollback-only CAS namespace"),
        };
        let relative_path = format!("{directory}/{relative}");
        let size_bytes = u64::try_from(bytes.len()).expect("small rollback-only object");
        match namespace {
            "source" => connection
                .execute(
                    "INSERT INTO content_sources
                     (sha256, relative_path, size_bytes, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        sha256,
                        relative_path,
                        u64_to_i64(size_bytes).expect("small source size"),
                        Utc::now().to_rfc3339(),
                    ],
                )
                .expect("insert rollback-only source row"),
            "asset" => connection
                .execute(
                    "INSERT INTO assets
                     (sha256, relative_path, media_type, size_bytes, created_at)
                     VALUES (?1, ?2, 'image/png', ?3, ?4)",
                    params![
                        sha256,
                        relative_path,
                        u64_to_i64(size_bytes).expect("small asset size"),
                        Utc::now().to_rfc3339(),
                    ],
                )
                .expect("insert rollback-only asset row"),
            _ => unreachable!("validated rollback-only CAS namespace"),
        };
        drop(connection);
        let final_path = root.path().join(&relative_path);
        fs::create_dir_all(final_path.parent().expect("rollback-only CAS parent"))
            .expect("create rollback-only CAS directory");
        fs::write(&final_path, bytes).expect("write rollback-only CAS object");
        let storage = Storage::open(root.path()).expect("cut over rollback-only CAS fixture");
        let intent = PackageCasPromotionIntent {
            import_id: import_id.to_owned(),
            namespace,
            sha256,
            size_bytes,
            media_type,
            relative_path,
        };
        (root, storage, intent, final_path)
    }

    fn approved_lan_connection(id: &str) -> (ProviderTemplate, ProviderConnection) {
        let profile = ProviderProfile {
            id: id.to_owned(),
            display_name: "Approved LAN".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "model".to_owned(),
            timeout_seconds: 30,
        };
        let (mut connection, _, _) =
            legacy_provider_graph(&profile, Utc::now()).expect("LAN connection fixture");
        let mut template = legacy_provider_template().expect("LAN template fixture");
        template.id = ProviderTemplateId::from("approved-lan-test-template");
        template.display_name = "Approved LAN test template".to_owned();
        template.source = TemplateSource::UserDiscovered;
        connection.template_id = template.id.clone();
        let api_origin = CanonicalOrigin::parse("https://192.168.10.20:11434").expect("LAN origin");
        connection.api_origin = api_origin.clone();
        connection.config.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
        connection.config.local_network_approval = Some(ProviderLocalNetworkApproval {
            origin: api_origin.clone(),
            addresses: vec!["192.168.10.20".parse().expect("LAN address")],
        });
        connection
            .credential_scope
            .as_mut()
            .expect("legacy credential scope")
            .allowed_origins = vec![api_origin];
        connection.config.values = vec![ConnectionConfigEntry {
            key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
            value: ConnectionConfigValue::Text("https://192.168.10.20:11434/v1".to_owned()),
        }];
        (template, connection)
    }

    fn save_modern_direct_provider_graph(
        storage: &Storage,
        profile: &ProviderProfile,
    ) -> (ProviderConnection, ModelRoute, GenerationPreset) {
        let (connection, mut route, preset) =
            legacy_provider_graph(profile, Utc::now()).expect("direct provider fixture");
        route.metadata_source = ModelMetadataSource::UserOverride;
        storage
            .save_provider_connection(&connection)
            .expect("save direct provider connection");
        storage
            .save_model_route(&route)
            .expect("save direct provider route");
        storage
            .save_generation_preset(&preset)
            .expect("save direct provider preset");
        assert_eq!(
            storage
                .get_provider_profile(&profile.id)
                .expect_err("direct provider fixture must not retain a legacy profile")
                .code,
            CoreErrorCode::NotFound
        );
        (connection, route, preset)
    }

    #[test]
    fn storage_owner_lock_child_probe() {
        let Some(root) = std::env::var_os(STORAGE_LOCK_PROBE_ROOT_ENV) else {
            return;
        };
        let Err(error) = Storage::open(PathBuf::from(root)) else {
            panic!("a second process unexpectedly acquired the data root");
        };
        assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
        assert_eq!(
            error.message,
            "data root is already owned by another LorePia process"
        );
    }

    #[test]
    fn approved_lan_connection_persists_exact_grant_and_reopens() {
        let root = tempdir().expect("temp root");
        let (template, connection) = approved_lan_connection("approved-lan-persisted");
        {
            let storage = Storage::open(root.path()).expect("open storage");
            storage
                .save_provider_template(&template)
                .expect("save LAN template");
            storage
                .save_provider_connection(&connection)
                .expect("save approved LAN connection");
            assert_eq!(
                storage
                    .get_provider_connection(&connection.id)
                    .expect("read approved LAN connection"),
                connection
            );
            let mirror = storage
                .connection()
                .expect("database")
                .query_row(
                    "SELECT origin, addresses_json
                     FROM provider_connection_local_network_approvals
                     WHERE connection_id = ?1",
                    [connection.id.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .expect("LAN approval mirror");
            assert_eq!(mirror.0, "https://192.168.10.20:11434");
            assert_eq!(mirror.1, r#"["192.168.10.20"]"#);
        }
        let reopened = Storage::open(root.path()).expect("reopen storage");
        assert_eq!(
            reopened
                .get_provider_connection(&connection.id)
                .expect("reopened approved LAN connection"),
            connection
        );
        reopened
            .connection()
            .expect("database")
            .execute(
                "DELETE FROM provider_connection_local_network_approvals
                 WHERE connection_id = ?1",
                [connection.id.as_str()],
            )
            .expect("simulate missing approval mirror");
        drop(reopened);
        let Err(error) = Storage::open(root.path()) else {
            panic!("missing LAN mirror must fail closed");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }

    #[test]
    fn provider_connection_storage_rejects_cleartext_credential_lan_grants() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let (template, mut connection) = approved_lan_connection("cleartext-credential-lan");
        let cleartext_origin =
            CanonicalOrigin::parse("http://192.168.10.20:11434").expect("cleartext LAN origin");
        connection.api_origin = cleartext_origin.clone();
        connection
            .config
            .local_network_approval
            .as_mut()
            .expect("LAN approval")
            .origin = cleartext_origin.clone();
        connection
            .credential_scope
            .as_mut()
            .expect("credential scope")
            .allowed_origins = vec![cleartext_origin];
        connection.config.values = vec![ConnectionConfigEntry {
            key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
            value: ConnectionConfigValue::Text("http://192.168.10.20:11434/v1".to_owned()),
        }];
        storage
            .save_provider_template(&template)
            .expect("save LAN template");

        let error = storage
            .save_provider_connection(&connection)
            .expect_err("credential-bearing LAN grants require authenticated transport");
        assert_eq!(error.code, CoreErrorCode::PermissionDenied);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn schema_ten_invalid_lan_grant_rolls_back_eleven_and_reopens_after_repair() {
        let root = tempdir().expect("temp root");
        let (template, connection) = approved_lan_connection("invalid-lan-v10");
        {
            fs::create_dir_all(root.path().join("db")).expect("create schema-ten database root");
            let mut database = Connection::open(root.path().join("db/lorepia.sqlite3"))
                .expect("open schema-ten database");
            database
                .pragma_update(None, "foreign_keys", true)
                .expect("enable schema-ten foreign keys");
            for (version, migration) in [
                (1, MIGRATION_0001),
                (2, MIGRATION_0002),
                (3, MIGRATION_0003),
                (4, MIGRATION_0004),
                (5, MIGRATION_0005),
                (6, MIGRATION_0006),
                (7, MIGRATION_0007),
                (8, MIGRATION_0008),
                (9, MIGRATION_0009),
                (10, MIGRATION_0010),
            ] {
                database
                    .execute_batch(migration)
                    .expect("apply historical migration through schema ten");
                database
                    .execute(
                        "INSERT INTO schema_migrations(version, applied_at)
                         VALUES (?1, '2026-07-31T00:00:00Z')",
                        [version],
                    )
                    .expect("record historical migration through schema ten");
            }
            let mut invalid_config = connection.config.clone();
            invalid_config
                .local_network_approval
                .as_mut()
                .expect("LAN approval")
                .addresses = vec!["8.8.8.8".parse().expect("public address")];
            let invalid_config_json =
                serde_json::to_string(&invalid_config).expect("encode invalid v10 config");
            let transaction = database.transaction().expect("schema-ten seed transaction");
            save_provider_template_row(&transaction, &template).expect("save schema-ten template");
            transaction
                .execute(
                    "INSERT INTO provider_connections
                     (id, template_id, template_version, display_name, api_origin,
                      config_json, credential_ref, credential_scope_json, timeout_seconds,
                      status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                    params![
                        connection.id.as_str(),
                        connection.template_id.as_str(),
                        connection.template_version,
                        connection.display_name,
                        connection.api_origin.as_str(),
                        invalid_config_json,
                        connection
                            .credential_ref
                            .as_ref()
                            .map(CredentialRef::as_str),
                        connection
                            .credential_scope
                            .as_ref()
                            .map(serde_json::to_string)
                            .transpose()
                            .expect("encode schema-ten credential scope"),
                        connection.timeout_seconds,
                        connection_status_to_str(connection.status),
                        connection.created_at.to_rfc3339(),
                        connection.updated_at.to_rfc3339(),
                    ],
                )
                .expect("seed semantically invalid schema-ten LAN grant");
            transaction.commit().expect("commit schema-ten fixture");
        }

        let Err(error) = Storage::open(root.path()) else {
            panic!("invalid schema-ten LAN grant must fail migration");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

        {
            let database =
                Connection::open(root.path().join("db/lorepia.sqlite3")).expect("database");
            assert_eq!(
                database
                    .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                        row.get::<_, u32>(0)
                    })
                    .expect("schema version"),
                10
            );
            assert_eq!(
                database
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'table'
                           AND name = 'provider_connection_local_network_approvals'",
                        [],
                        |row| row.get::<_, u32>(0),
                    )
                    .expect("schema-eleven table count"),
                0
            );
            assert_eq!(
                database
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'trigger'
                           AND name = 'provider_connection_local_network_approval_guard'",
                        [],
                        |row| row.get::<_, u32>(0),
                    )
                    .expect("schema-eleven trigger count"),
                0
            );
            database
                .execute(
                    "UPDATE provider_connections
                     SET config_json = ?2
                     WHERE id = ?1",
                    params![
                        connection.id.as_str(),
                        serde_json::to_string(&connection.config)
                            .expect("encode repaired v10 config")
                    ],
                )
                .expect("repair schema-ten LAN grant");
        }

        let reopened = Storage::open(root.path()).expect("migrate repaired schema-ten storage");
        assert_eq!(
            reopened
                .schema_version()
                .expect("read durable schema version"),
            SCHEMA_VERSION
        );
        assert_eq!(
            reopened
                .get_provider_connection(&connection.id)
                .expect("reopened LAN connection"),
            connection
        );
        assert_eq!(
            reopened
                .connection()
                .expect("database")
                .query_row(
                    "SELECT addresses_json
                     FROM provider_connection_local_network_approvals
                     WHERE connection_id = ?1",
                    [connection.id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .expect("recreated approval mirror"),
            r#"["192.168.10.20"]"#
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn provider_connection_storage_rejects_noncanonical_network_grants() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let (template, valid) = approved_lan_connection("approved-lan-invalid");
        storage
            .save_provider_template(&template)
            .expect("save LAN template");

        let mut mismatch = valid.clone();
        mismatch
            .config
            .local_network_approval
            .as_mut()
            .expect("approval")
            .origin =
            CanonicalOrigin::parse("http://192.168.10.21:11434").expect("other LAN origin");
        assert_eq!(
            storage
                .save_provider_connection(&mismatch)
                .expect_err("origin mismatch")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut empty = valid.clone();
        empty
            .config
            .local_network_approval
            .as_mut()
            .expect("approval")
            .addresses
            .clear();
        assert!(
            storage
                .save_provider_connection(&empty)
                .expect_err("empty address approval")
                .message
                .contains("1 to 16")
        );

        let mut oversized = valid.clone();
        oversized
            .config
            .local_network_approval
            .as_mut()
            .expect("approval")
            .addresses = (1..=17)
            .map(|last| IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)))
            .collect();
        assert!(
            storage
                .save_provider_connection(&oversized)
                .expect_err("oversized address approval")
                .message
                .contains("1 to 16")
        );

        let mut unsorted = valid.clone();
        unsorted
            .config
            .local_network_approval
            .as_mut()
            .expect("approval")
            .addresses = vec![
            "192.168.10.21".parse().expect("LAN address"),
            "192.168.10.20".parse().expect("LAN address"),
        ];
        assert!(
            storage
                .save_provider_connection(&unsorted)
                .expect_err("unsorted address approval")
                .message
                .contains("sorted")
        );

        let mut public_address = valid.clone();
        public_address
            .config
            .local_network_approval
            .as_mut()
            .expect("approval")
            .addresses = vec!["8.8.8.8".parse().expect("public address")];
        assert!(
            storage
                .save_provider_connection(&public_address)
                .expect_err("public address approval")
                .message
                .contains("RFC1918")
        );

        let mut loopback_with_grant = valid;
        let loopback_origin =
            CanonicalOrigin::parse("http://127.0.0.1:11434").expect("loopback origin");
        loopback_with_grant.api_origin = loopback_origin.clone();
        loopback_with_grant.config.network_mode = ProviderNetworkMode::LocalLoopback;
        loopback_with_grant
            .config
            .local_network_approval
            .as_mut()
            .expect("approval")
            .origin = loopback_origin.clone();
        loopback_with_grant
            .credential_scope
            .as_mut()
            .expect("credential scope")
            .allowed_origins = vec![loopback_origin];
        assert!(
            storage
                .save_provider_connection(&loopback_with_grant)
                .expect_err("loopback mode with LAN approval")
                .message
                .contains("only valid")
        );
    }

    #[test]
    fn provider_connection_catalog_state_compare_and_swap_rejects_stale_review() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let mut template = legacy_provider_template().expect("template");
        template.id = ProviderTemplateId::from("signed-catalog-cas-template");
        template.manifest_version = 7;
        template.source = TemplateSource::SignedCatalog;

        let profile = ProviderProfile {
            id: "catalog-cas-connection".to_owned(),
            display_name: "Catalog CAS".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "model".to_owned(),
            timeout_seconds: 30,
        };
        let (mut connection, _, _) =
            legacy_provider_graph(&profile, Utc::now()).expect("connection");
        connection.template_id = template.id.clone();
        connection.template_version = template.manifest_version;
        storage
            .insert_provider_connection_for_catalog_state(&connection, &template, 0)
            .expect("save against reviewed state");

        let mut duplicate = connection.clone();
        duplicate.display_name = "Retargeted duplicate".to_owned();
        let duplicate_error = storage
            .insert_provider_connection_for_catalog_state(&duplicate, &template, 0)
            .expect_err("catalog create must not overwrite an occupied connection ID");
        assert_eq!(duplicate_error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            storage
                .get_provider_connection(&connection.id)
                .expect("original catalog connection"),
            connection
        );

        storage
            .connection()
            .expect("database")
            .execute(
                "UPDATE provider_catalog_state
                 SET state_version = 1, updated_at = ?1
                 WHERE singleton = 1",
                [Utc::now().to_rfc3339()],
            )
            .expect("advance catalog state");
        connection.id = ProviderConnectionId::from("catalog-cas-stale");
        connection.credential_ref = Some(CredentialRef("catalog-cas-stale".to_owned()));
        let error = storage
            .insert_provider_connection_for_catalog_state(&connection, &template, 0)
            .expect_err("stale catalog review must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(
            storage.get_provider_connection(&connection.id).is_err(),
            "stale connection must not be inserted"
        );
    }

    #[test]
    fn data_root_owner_lock_blocks_recovery_in_a_second_process_until_drop() {
        let root = tempdir().expect("temp root");
        let owner = Storage::open(root.path()).expect("open owner");
        let active_staging = root.path().join("staging/active-import.partial");
        fs::write(&active_staging, b"owned by the first process").expect("active staging");

        let output = Command::new(std::env::current_exe().expect("current test executable"))
            .arg(STORAGE_LOCK_PROBE_TEST_NAME)
            .arg("--exact")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(STORAGE_LOCK_PROBE_ROOT_ENV, root.path())
            .output()
            .expect("run second-process probe");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "second-process probe failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let expected_success = format!("test {STORAGE_LOCK_PROBE_TEST_NAME} ... ok");
        assert!(
            stdout.lines().any(|line| line == expected_success),
            "second-process probe did not execute the exact child test\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            active_staging.exists(),
            "a rejected second process must not run staging recovery"
        );

        drop(owner);
        let reopened = Storage::open(root.path()).expect("reopen after owner drop");
        assert!(
            !active_staging.exists(),
            "the next owner must run normal staging recovery"
        );
        drop(reopened);
    }

    #[cfg(unix)]
    #[test]
    fn data_root_and_owner_lock_must_not_be_symbolic_links() {
        use std::os::unix::fs::symlink;

        let parent = tempdir().expect("temp parent");
        let real_root = parent.path().join("real");
        let linked_root = parent.path().join("linked");
        fs::create_dir(&real_root).expect("real root");
        symlink(&real_root, &linked_root).expect("data root symlink");
        let Err(error) = Storage::open(&linked_root) else {
            panic!("symbolic-link data root must be rejected");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

        let root = tempdir().expect("temp root");
        let outside = parent.path().join("outside-lock");
        fs::write(&outside, b"not a LorePia lock").expect("outside lock");
        symlink(&outside, root.path().join(".lorepia-owner.lock")).expect("owner lock symlink");
        let Err(error) = Storage::open(root.path()) else {
            panic!("symbolic-link owner lock must be rejected");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert_eq!(error.message, "data root owner lock is not a regular file");
    }

    fn append_pending_generation(
        storage: &Storage,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user_text: &str,
    ) -> (Message, Message, GenerationRecord) {
        let user = Message::user_after(conversation_id.clone(), expected_head.cloned(), user_text);
        let generation_id = GenerationId::new();
        let pending = Message::pending_assistant(
            conversation_id.clone(),
            user.id.clone(),
            generation_id.clone(),
        );
        let generation = GenerationRecord {
            id: generation_id,
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            user_message_id: user.id.clone(),
            assistant_message_id: Some(pending.id.clone()),
            mode: ConversationMode::Chat,
            model: "synthetic".to_owned(),
            model_route_id: None,
            generation_preset_id: None,
            provider_family: None,
            status: GenerationStatus::Running,
            input_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
            opaque_reasoning_state: Vec::new(),
            error_code: None,
            started_at: pending.created_at,
            finished_at: None,
        };
        storage
            .append_generation(branch_id, expected_head, &user, &pending, &generation)
            .expect("append generation");
        (user, pending, generation)
    }

    fn provider_generation_record(
        conversation: &Conversation,
        branch_id: &ConversationBranchId,
        route_id: ModelRouteId,
        preset_id: GenerationPresetId,
        model: &str,
        user_text: &str,
    ) -> (Message, Message, GenerationRecord) {
        let user = Message::user(conversation.id.clone(), user_text);
        let generation_id = GenerationId::new();
        let pending = Message::pending_assistant(
            conversation.id.clone(),
            user.id.clone(),
            generation_id.clone(),
        );
        let generation = GenerationRecord {
            id: generation_id,
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            user_message_id: user.id.clone(),
            assistant_message_id: Some(pending.id.clone()),
            mode: ConversationMode::Chat,
            model: model.to_owned(),
            model_route_id: Some(route_id),
            generation_preset_id: Some(preset_id),
            provider_family: Some(ApiFamily::OpenAiChatCompletions),
            status: GenerationStatus::Running,
            input_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
            opaque_reasoning_state: Vec::new(),
            error_code: None,
            started_at: pending.created_at,
            finished_at: None,
        };
        (user, pending, generation)
    }

    fn append_complete_generation(
        storage: &Storage,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        user_text: &str,
        assistant_text: &str,
    ) -> (Message, Message) {
        let (user, pending, _) = append_pending_generation(
            storage,
            conversation_id,
            branch_id,
            expected_head,
            user_text,
        );
        let mut assistant = pending;
        assistant.content = assistant_text.to_owned();
        assistant.status = MessageStatus::Complete;
        storage
            .finalize_generation(&assistant, None, None, true)
            .expect("finalize generation");
        (user, assistant)
    }

    fn imported_storage() -> (
        tempfile::TempDir,
        Storage,
        Conversation,
        ConversationBranchId,
    ) {
        let root = tempdir().expect("temp root");
        let mut staged = NamedTempFile::new_in(root.path()).expect("staging");
        staged.write_all(b"character").expect("source");
        let character = Character::new("Segu", "Guide", hex::encode(Sha256::digest(b"character")));
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .commit_character_import(
                staged.path(),
                &character,
                9,
                &Uuid::new_v4().to_string(),
                &[],
            )
            .expect("commit import");
        let conversation = Conversation::new(&character.id, &character.name);
        let (_, state) = storage
            .save_conversation_with_mode(&conversation, ConversationMode::Chat)
            .expect("save conversation");
        (root, storage, conversation, state.active_branch_id)
    }

    fn install_protocol_state_target(
        storage: &Storage,
        id: &str,
        family: ApiFamily,
    ) -> (ModelRouteId, GenerationPresetId, String) {
        let profile = ProviderProfile {
            id: id.to_owned(),
            display_name: format!("Protocol State {id}"),
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            model: format!("{id}-model"),
            timeout_seconds: 30,
        };
        let (mut connection, mut route, preset) =
            legacy_provider_graph(&profile, Utc::now()).expect("protocol-state graph");
        let mut template = legacy_provider_template().expect("protocol-state template");
        template.id = ProviderTemplateId::from(format!("{id}-template"));
        template.display_name = format!("Protocol State {id}");
        template.source = TemplateSource::UserDiscovered;
        template.api_family = family;
        template.default_manifest.api_family = family;
        connection.template_id = template.id.clone();
        connection.template_version = template.manifest_version;
        route.api_family = family;
        storage
            .save_provider_template(&template)
            .expect("save protocol-state template");
        storage
            .insert_provider_connection(&connection)
            .expect("insert protocol-state connection");
        storage
            .save_model_route(&route)
            .expect("save protocol-state route");
        storage
            .save_generation_preset(&preset)
            .expect("save protocol-state preset");
        (route.id, preset.id, profile.model)
    }

    fn gemini_states_with_serialized_len(target: usize) -> Vec<OpaqueReasoningState> {
        fn build(counts: [usize; 4], append_plain_byte: bool) -> Vec<OpaqueReasoningState> {
            counts
                .into_iter()
                .enumerate()
                .map(|(part_index, count)| {
                    let mut signature = "\\".repeat(count);
                    if append_plain_byte && part_index == 0 {
                        signature.push('a');
                    }
                    OpaqueReasoningState::GeminiThoughtSignature {
                        part_index: u32::try_from(part_index).expect("bounded part index"),
                        signature: lorepia_domain::OpaqueReasoningData::parse(signature)
                            .expect("bounded backslash-heavy signature"),
                    }
                })
                .collect()
        }

        let mut counts = [1_usize; 4];
        let baseline = serde_json::to_vec(&build(counts, false))
            .expect("serialize baseline opaque state")
            .len();
        let mut remaining = target
            .checked_sub(baseline)
            .expect("target must fit the fixed state envelope");
        let append_plain_byte = remaining % 2 == 1;
        remaining -= usize::from(append_plain_byte);
        let mut extra_backslashes = remaining / 2;
        for (index, count) in counts.iter_mut().enumerate() {
            let suffix_bytes = usize::from(append_plain_byte && index == 0);
            let capacity = lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES - *count - suffix_bytes;
            let added = capacity.min(extra_backslashes);
            *count += added;
            extra_backslashes -= added;
        }
        assert_eq!(extra_backslashes, 0, "target exceeds domain item bounds");
        let states = build(counts, append_plain_byte);
        assert_eq!(
            serde_json::to_vec(&states)
                .expect("serialize exact opaque state")
                .len(),
            target
        );
        states
    }

    fn raw_legacy_provider_identity_rows(storage: &Storage, id: &str) -> (String, String) {
        storage
            .connection()
            .expect("database")
            .query_row(
                "SELECT
                   hex(CAST(json_array(
                     profile.id, profile.display_name, profile.base_url,
                     profile.model, profile.timeout_seconds
                   ) AS BLOB)),
                   hex(CAST(json_array(
                     connection.id, connection.template_id, connection.template_version,
                     connection.display_name, connection.api_origin, connection.config_json,
                     connection.credential_ref, connection.credential_scope_json,
                     connection.timeout_seconds, connection.status,
                     connection.created_at, connection.updated_at, connection.archived_at
                   ) AS BLOB))
                 FROM provider_profiles AS profile
                 JOIN provider_connections AS connection ON connection.id = profile.id
                 WHERE profile.id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("raw legacy provider rows")
    }

    fn version_three_provider_database(root: &std::path::Path) -> Connection {
        fs::create_dir_all(root.join("db")).expect("db directory");
        let connection =
            Connection::open(root.join("db/lorepia.sqlite3")).expect("legacy database");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("foreign keys");
        connection
            .execute_batch(MIGRATION_0001)
            .expect("initial schema");
        connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?1)",
                ["2026-01-01T00:00:00Z"],
            )
            .expect("version one");
        connection
            .execute_batch(MIGRATION_0002)
            .expect("second migration");
        connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (2, ?1)",
                ["2026-01-01T00:00:01Z"],
            )
            .expect("version two");
        connection
            .execute_batch(MIGRATION_0003)
            .expect("third migration");
        connection
            .execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (3, ?1)",
                ["2026-01-01T00:00:02Z"],
            )
            .expect("version three");
        connection
    }

    fn insert_legacy_provider_profile(
        connection: &Connection,
        profile: (&str, &str, &str, &str, i64),
    ) {
        connection
            .execute(
                "INSERT INTO provider_profiles
                 (id, display_name, base_url, model, timeout_seconds)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![profile.0, profile.1, profile.2, profile.3, profile.4],
            )
            .expect("legacy provider profile");
    }

    #[test]
    fn usage_overflow_can_be_compensated_and_the_branch_accepts_another_generation() {
        let (_root, storage, conversation, branch_id) = imported_storage();
        let (_user, pending, generation) =
            append_pending_generation(&storage, &conversation.id, &branch_id, None, "first");
        let mut assistant = pending.clone();
        assistant.content = "response before invalid usage".to_owned();
        assistant.status = MessageStatus::Complete;
        let error = storage
            .finalize_generation(
                &assistant,
                Some(&lorepia_domain::GenerationUsage {
                    input_tokens: Some(1),
                    cached_write_tokens: Some(i64::MAX as u64 + 1),
                    output_tokens: Some(1),
                    ..lorepia_domain::GenerationUsage::default()
                }),
                None,
                true,
            )
            .expect_err("overflow usage must reject normal finalization");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            storage
                .get_generation(&generation.id)
                .expect("running generation")
                .status,
            GenerationStatus::Running
        );
        assert_eq!(
            storage
                .list_branch_messages(&branch_id)
                .expect("pending lineage")[1]
                .status,
            MessageStatus::Pending
        );

        assistant.status = MessageStatus::Failed;
        storage
            .fail_generation_after_finalize_error(&assistant, true)
            .expect("compensate overflow");
        let failed = storage
            .get_generation(&generation.id)
            .expect("failed generation");
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert_eq!(failed.input_tokens, None);
        assert_eq!(failed.cached_write_tokens, None);
        assert_eq!(failed.output_tokens, None);
        assert_eq!(
            failed.error_code.as_deref(),
            Some(CoreErrorCode::StorageUnavailable.as_str())
        );
        assert!(failed.finished_at.is_some());
        let messages = storage
            .list_branch_messages(&branch_id)
            .expect("failed lineage");
        assert_eq!(messages[1].status, MessageStatus::Failed);

        let (_, retry) = append_complete_generation(
            &storage,
            &conversation.id,
            &branch_id,
            Some(&assistant.id),
            "retry",
            "retry succeeded",
        );
        assert_eq!(retry.status, MessageStatus::Complete);
        assert!(
            storage
                .list_branch_messages(&branch_id)
                .expect("retried lineage")
                .iter()
                .all(|message| message.status != MessageStatus::Pending)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn expanded_usage_and_opaque_reasoning_state_survive_reopen() {
        let (root, storage, conversation, branch_id) = imported_storage();
        let cases = vec![
            (
                "openai-responses",
                ApiFamily::OpenAiResponses,
                serde_json::from_value(serde_json::json!({
                "kind": "open_ai_responses",
                "item": {
                    "id": "opaque-openai-item-canary",
                    "type": "reasoning",
                    "summary": [{
                        "type": "summary_text",
                        "text": "opaque-openai-summary-canary"
                    }],
                    "content": [{
                        "type": "reasoning_text",
                        "text": "opaque-openai-reasoning-text-canary"
                    }],
                    "encrypted_content": "opaque-openai-content-canary",
                    "status": "completed"
                }
                }))
                .expect("bounded OpenAI Responses reasoning item"),
            ),
            (
                "openrouter",
                ApiFamily::OpenAiChatCompletions,
                serde_json::from_value(serde_json::json!({
                    "kind": "open_router_reasoning",
                    "topology": {
                        "reasoning": "opaque-openrouter-reasoning-canary",
                        "reasoning_details": [concat!(
                            "{\"type\":\"reasoning.encrypted\",",
                            "\"data\":\"opaque-openrouter-data-canary\",",
                            "\"id\":\"opaque-openrouter-id-canary\",",
                            "\"format\":\"openai-responses-v1\"}"
                        ), concat!(
                            "{\"type\":\"reasoning.text\",",
                            "\"signature\":\"opaque-openrouter-signature-only-canary\"}"
                        ), concat!(
                            "{\"type\":\"reasoning.text\",",
                            "\"text\":null,",
                            "\"signature\":\"opaque-openrouter-null-text-canary\",",
                            "\"id\":null,",
                            "\"format\":null,",
                            "\"index\":null}"
                        )]
                    }
                }))
                .expect("bounded OpenRouter reasoning topology"),
            ),
            (
                "anthropic",
                ApiFamily::AnthropicMessages,
                OpaqueReasoningState::AnthropicMessages {
                    content_blocks: lorepia_domain::AnthropicContentBlockTopology::new(vec![
                        lorepia_domain::AnthropicContentBlock::Thinking {
                            thinking: lorepia_domain::AnthropicBlockText::parse(
                                "opaque-anthropic-thinking-canary",
                            )
                            .expect("bounded Anthropic thinking"),
                            signature: lorepia_domain::OpaqueReasoningData::parse(
                                "opaque-anthropic-signature-canary",
                            )
                            .expect("bounded Anthropic signature"),
                        },
                        lorepia_domain::AnthropicContentBlock::RedactedThinking {
                            data: lorepia_domain::OpaqueReasoningData::parse(
                                "opaque-anthropic-redacted-canary",
                            )
                            .expect("bounded Anthropic redacted thinking"),
                        },
                        lorepia_domain::AnthropicContentBlock::Text {
                            text: lorepia_domain::AnthropicBlockText::parse(
                                "opaque-anthropic-text-canary",
                            )
                            .expect("bounded Anthropic text"),
                        },
                        lorepia_domain::AnthropicContentBlock::ToolUse {
                            id: lorepia_domain::ToolCallId::parse(
                                "opaque-anthropic-tool-id-canary",
                            )
                            .expect("bounded Anthropic tool ID"),
                            name: lorepia_domain::ToolName::parse("lookup")
                                .expect("bounded Anthropic tool name"),
                            input: lorepia_domain::AnthropicToolInput::from_value(
                                &serde_json::json!({
                                    "query": "opaque-anthropic-tool-input-canary"
                                }),
                            )
                            .expect("bounded Anthropic tool input"),
                        },
                    ])
                    .expect("bounded Anthropic content topology"),
                },
            ),
            (
                "gemini",
                ApiFamily::GeminiGenerateContent,
                OpaqueReasoningState::GeminiThoughtSignature {
                    part_index: 0,
                    signature: lorepia_domain::OpaqueReasoningData::parse(
                        "opaque-gemini-signature-canary",
                    )
                    .expect("bounded signature"),
                },
            ),
        ];
        let opaque_debug = format!("{cases:?}");
        for canary in [
            "opaque-openai-item-canary",
            "opaque-openai-summary-canary",
            "opaque-openai-reasoning-text-canary",
            "opaque-openai-content-canary",
            "opaque-gemini-signature-canary",
            "opaque-openrouter-reasoning-canary",
            "opaque-openrouter-data-canary",
            "opaque-openrouter-id-canary",
            "opaque-openrouter-signature-only-canary",
            "opaque-openrouter-null-text-canary",
            "opaque-anthropic-text-canary",
            "opaque-anthropic-thinking-canary",
            "opaque-anthropic-signature-canary",
            "opaque-anthropic-redacted-canary",
            "opaque-anthropic-tool-id-canary",
            "opaque-anthropic-tool-input-canary",
        ] {
            assert!(
                !opaque_debug.contains(canary),
                "opaque state Debug output exposed {canary}"
            );
        }

        let usage = GenerationUsage {
            input_tokens: Some(101),
            cached_read_tokens: Some(11),
            cached_write_tokens: Some(12),
            output_tokens: Some(202),
            reasoning_tokens: Some(21),
            tool_tokens: Some(22),
            provider_raw_summary: Some(
                BoundedJson::parse(r#"{"total_tokens":303}"#).expect("bounded summary"),
            ),
        };
        let mut expected_head = None;
        let mut persisted = Vec::new();
        for (id, family, state) in cases {
            let (route_id, preset_id, model) = install_protocol_state_target(&storage, id, family);
            let user = Message::user_after(
                conversation.id.clone(),
                expected_head.clone(),
                format!("persist {id} protocol state"),
            );
            let generation_id = GenerationId::new();
            let pending = Message::pending_assistant(
                conversation.id.clone(),
                user.id.clone(),
                generation_id.clone(),
            );
            let generation = GenerationRecord {
                id: generation_id.clone(),
                conversation_id: conversation.id.clone(),
                branch_id: branch_id.clone(),
                user_message_id: user.id.clone(),
                assistant_message_id: Some(pending.id.clone()),
                mode: ConversationMode::Chat,
                model,
                model_route_id: Some(route_id),
                generation_preset_id: Some(preset_id),
                provider_family: Some(family),
                status: GenerationStatus::Running,
                input_tokens: None,
                cached_read_tokens: None,
                cached_write_tokens: None,
                output_tokens: None,
                reasoning_tokens: None,
                tool_tokens: None,
                provider_raw_summary: None,
                opaque_reasoning_state: Vec::new(),
                error_code: None,
                started_at: pending.created_at,
                finished_at: None,
            };
            storage
                .append_generation(
                    &branch_id,
                    expected_head.as_ref(),
                    &user,
                    &pending,
                    &generation,
                )
                .expect("append protocol-state generation");
            let mut assistant = pending;
            assistant.content = "complete".to_owned();
            assistant.status = MessageStatus::Complete;
            storage
                .finalize_generation_with_protocol_state(
                    &assistant,
                    Some(&usage),
                    std::slice::from_ref(&state),
                    None,
                    true,
                )
                .expect("finalize protocol-state generation");
            expected_head = Some(assistant.id);
            persisted.push((generation_id, family, state));
        }

        drop(storage);
        let reopened = Storage::open(root.path()).expect("reopen storage");
        for (generation_id, family, state) in &persisted {
            let restored = reopened
                .get_generation(generation_id)
                .expect("restore generation");
            assert_eq!(restored.provider_family, Some(*family));
            assert_eq!(restored.input_tokens, usage.input_tokens);
            assert_eq!(restored.cached_read_tokens, usage.cached_read_tokens);
            assert_eq!(restored.cached_write_tokens, usage.cached_write_tokens);
            assert_eq!(restored.output_tokens, usage.output_tokens);
            assert_eq!(restored.reasoning_tokens, usage.reasoning_tokens);
            assert_eq!(restored.tool_tokens, usage.tool_tokens);
            assert_eq!(restored.provider_raw_summary, usage.provider_raw_summary);
            assert_eq!(restored.opaque_reasoning_state, std::slice::from_ref(state));
        }
        let first_generation_id = &persisted.first().expect("persisted generation").0;
        let error = reopened
            .connection()
            .expect("reopened connection")
            .execute(
                "UPDATE generations SET status = 'running' WHERE id = ?1",
                [&first_generation_id.0],
            )
            .expect_err("opaque state must remain terminal-only");
        assert!(
            error
                .to_string()
                .contains("generation protocol-state provenance is inconsistent")
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn exact_serialized_opaque_reasoning_limit_survives_storage_and_reopen() {
        let (root, storage, conversation, branch_id) = imported_storage();
        let (route_id, preset_id, model) = install_protocol_state_target(
            &storage,
            "opaque-serialized-envelope",
            ApiFamily::GeminiGenerateContent,
        );
        let user = Message::user(
            conversation.id.clone(),
            "persist the exact opaque-state JSON envelope",
        );
        let generation_id = GenerationId::new();
        let pending = Message::pending_assistant(
            conversation.id.clone(),
            user.id.clone(),
            generation_id.clone(),
        );
        let generation = GenerationRecord {
            id: generation_id.clone(),
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            user_message_id: user.id.clone(),
            assistant_message_id: Some(pending.id.clone()),
            mode: ConversationMode::Chat,
            model,
            model_route_id: Some(route_id),
            generation_preset_id: Some(preset_id),
            provider_family: Some(ApiFamily::GeminiGenerateContent),
            status: GenerationStatus::Running,
            input_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
            opaque_reasoning_state: Vec::new(),
            error_code: None,
            started_at: pending.created_at,
            finished_at: None,
        };
        storage
            .append_generation(&branch_id, None, &user, &pending, &generation)
            .expect("append exact-envelope generation");

        let states = gemini_states_with_serialized_len(MAX_OPAQUE_REASONING_SERIALIZED_BYTES);
        validate_opaque_reasoning_states(&states).expect("domain accepts the exact JSON envelope");
        let encoded = serde_json::to_string(&states).expect("serialize exact JSON envelope");
        assert_eq!(encoded.len(), MAX_OPAQUE_REASONING_SERIALIZED_BYTES);

        let mut assistant = pending;
        assistant.content = "complete".to_owned();
        assistant.status = MessageStatus::Complete;
        storage
            .finalize_generation_with_protocol_state(&assistant, None, &states, None, true)
            .expect("store the exact opaque-state JSON envelope");
        let stored_len = storage
            .connection()
            .expect("database")
            .query_row(
                "SELECT length(CAST(opaque_reasoning_state_json AS BLOB))
                 FROM generations
                 WHERE id = ?1",
                [&generation_id.0],
                |row| row.get::<_, usize>(0),
            )
            .expect("stored opaque-state byte length");
        assert_eq!(stored_len, MAX_OPAQUE_REASONING_SERIALIZED_BYTES);

        drop(storage);
        let reopened = Storage::open(root.path()).expect("reopen exact opaque-state envelope");
        assert_eq!(
            reopened
                .get_generation(&generation_id)
                .expect("restore exact opaque-state envelope")
                .opaque_reasoning_state,
            states
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn opaque_reasoning_bounds_and_corruption_fail_closed_without_payloads_in_errors() {
        let (root, storage, conversation, branch_id) = imported_storage();
        let profile = ProviderProfile {
            id: "opaque-state-validation-provider".to_owned(),
            display_name: "Opaque State Validation Provider".to_owned(),
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&profile)
            .expect("save opaque-state route");

        let user = Message::user(conversation.id.clone(), "validate opaque protocol state");
        let generation_id = GenerationId::new();
        let pending = Message::pending_assistant(
            conversation.id.clone(),
            user.id.clone(),
            generation_id.clone(),
        );
        let generation = GenerationRecord {
            id: generation_id.clone(),
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            user_message_id: user.id.clone(),
            assistant_message_id: Some(pending.id.clone()),
            mode: ConversationMode::Chat,
            model: profile.model.clone(),
            model_route_id: Some(ModelRouteId::from(profile.id.as_str())),
            generation_preset_id: Some(GenerationPresetId::from(profile.id.as_str())),
            provider_family: Some(ApiFamily::OpenAiChatCompletions),
            status: GenerationStatus::Running,
            input_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
            opaque_reasoning_state: Vec::new(),
            error_code: None,
            started_at: pending.created_at,
            finished_at: None,
        };
        storage
            .append_generation(&branch_id, None, &user, &pending, &generation)
            .expect("append opaque-state generation");

        let individual_canary = "opaque-individual-bound-canary";
        let oversized_individual = format!(
            "{individual_canary}{}",
            "i".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES)
        );
        let individual_error = lorepia_domain::OpaqueReasoningData::parse(oversized_individual)
            .expect_err("individual opaque payload must be bounded before storage");
        assert!(!individual_error.contains(individual_canary));

        let mut terminal = pending.clone();
        terminal.content = "complete".to_owned();
        terminal.status = MessageStatus::Complete;
        let mismatch_canary = "opaque-family-mismatch-canary";
        let mismatched_state = OpaqueReasoningState::GeminiThoughtSignature {
            part_index: 0,
            signature: lorepia_domain::OpaqueReasoningData::parse(mismatch_canary)
                .expect("bounded mismatch fixture"),
        };
        let matching_state: OpaqueReasoningState = serde_json::from_value(serde_json::json!({
            "kind": "open_router_reasoning",
            "topology": {
                "reasoning_details": [serde_json::json!({
                    "type": "reasoning.encrypted",
                    "data": "matching-state",
                    "id": "matching-detail",
                    "format": "openai-responses-v1"
                }).to_string()]
            }
        }))
        .expect("bounded matching fixture");
        let mismatch_error = storage
            .finalize_generation_with_protocol_state(
                &terminal,
                None,
                &[matching_state, mismatched_state],
                None,
                true,
            )
            .expect_err("mixed provider-family state must fail before persistence");
        assert_eq!(mismatch_error.code, CoreErrorCode::InvalidInput);
        assert!(!mismatch_error.message.contains(mismatch_canary));
        assert!(!format!("{mismatch_error:?}").contains(mismatch_canary));

        let aggregate_canary = "opaque-aggregate-bound-canary";
        let aggregate_item: OpaqueReasoningState = serde_json::from_value(serde_json::json!({
            "kind": "open_router_reasoning",
            "topology": {
                "reasoning_details": [serde_json::json!({
                    "type": "reasoning.encrypted",
                    "data": format!("{aggregate_canary}{}", "a".repeat(60 * 1024)),
                    "id": "aggregate-detail",
                    "format": "openai-responses-v1"
                }).to_string()]
            }
        }))
        .expect("individually bounded aggregate fixture");
        let aggregate_error = storage
            .finalize_generation_with_protocol_state(
                &terminal,
                None,
                &vec![aggregate_item.clone(); 5],
                None,
                true,
            )
            .expect_err("aggregate opaque payload must be rejected before write");
        assert_eq!(aggregate_error.code, CoreErrorCode::InvalidInput);
        assert!(!aggregate_error.message.contains(aggregate_canary));
        assert!(!format!("{aggregate_error:?}").contains(aggregate_canary));

        let count_item: OpaqueReasoningState = serde_json::from_value(serde_json::json!({
            "kind": "open_router_reasoning",
            "topology": {
                "reasoning_details": [serde_json::json!({
                    "type": "reasoning.encrypted",
                    "data": "bounded",
                    "id": "count-detail",
                    "format": "openai-responses-v1"
                }).to_string()]
            }
        }))
        .expect("bounded count fixture");
        let count_error = storage
            .finalize_generation_with_protocol_state(
                &terminal,
                None,
                &vec![count_item; lorepia_domain::MAX_OPAQUE_REASONING_STATE_COUNT + 1],
                None,
                true,
            )
            .expect_err("opaque item count must be rejected before write");
        assert_eq!(count_error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            storage
                .get_generation(&generation_id)
                .expect("generation remains running after rejected writes")
                .status,
            GenerationStatus::Running
        );
        assert_eq!(
            storage
                .list_branch_messages(&branch_id)
                .expect("messages remain unchanged after rejected writes")
                .last()
                .expect("pending assistant")
                .status,
            MessageStatus::Pending
        );

        let valid_canary = "opaque-valid-storage-canary";
        let valid_state = serde_json::from_value(serde_json::json!({
            "kind": "open_router_reasoning",
            "topology": {
                "reasoning_details": [serde_json::json!({
                    "type": "reasoning.encrypted",
                    "data": valid_canary,
                    "id": "valid-detail",
                    "format": "openai-responses-v1"
                }).to_string()]
            }
        }))
        .expect("valid stored state");
        storage
            .finalize_generation_with_protocol_state(
                &terminal,
                None,
                std::slice::from_ref(&valid_state),
                None,
                true,
            )
            .expect("store valid opaque state");

        let malformed = storage
            .connection()
            .expect("database")
            .execute(
                "UPDATE generations
                 SET opaque_reasoning_state_json = '[{\"kind\":'
                 WHERE id = ?1",
                [&generation_id.0],
            )
            .expect_err("schema must reject malformed opaque-state JSON");
        assert!(!malformed.to_string().contains(valid_canary));
        assert_eq!(
            storage
                .get_generation(&generation_id)
                .expect("malformed update did not replace valid state")
                .opaque_reasoning_state,
            vec![valid_state]
        );

        let unknown_canary = "opaque-unknown-payload-canary";
        let unknown_discriminator_canary = "opaque-unknown-kind-canary";
        let unknown_json = serde_json::json!([{
            "kind": unknown_discriminator_canary,
            "payload": unknown_canary
        }])
        .to_string();
        storage
            .connection()
            .expect("database")
            .execute(
                "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
                params![generation_id.0, unknown_json],
            )
            .expect("inject structurally valid unknown state");
        drop(storage);

        let reopened = Storage::open(root.path()).expect("reopen storage with unknown state");
        let unknown_error = reopened
            .get_generation(&generation_id)
            .expect_err("unknown opaque state must fail closed");
        assert_eq!(unknown_error.code, CoreErrorCode::StorageCorrupted);
        assert!(!unknown_error.message.contains(unknown_canary));
        assert!(!format!("{unknown_error:?}").contains(unknown_canary));
        assert!(!unknown_error.message.contains(unknown_discriminator_canary));
        assert!(!format!("{unknown_error:?}").contains(unknown_discriminator_canary));

        let anthropic_type_canary = "opaque-anthropic-unknown-type-canary";
        let anthropic_unknown_type_json = serde_json::json!([{
            "kind": "anthropic_messages",
            "content_blocks": [{
                "type": anthropic_type_canary,
                "text": "bounded"
            }]
        }])
        .to_string();
        reopened
            .connection()
            .expect("database")
            .execute(
                "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
                params![generation_id.0, anthropic_unknown_type_json],
            )
            .expect("inject Anthropic topology with unknown block type");
        let anthropic_type_error = reopened
            .get_generation(&generation_id)
            .expect_err("unknown Anthropic block type must fail closed");
        assert_eq!(anthropic_type_error.code, CoreErrorCode::StorageCorrupted);
        assert!(!anthropic_type_error.message.contains(anthropic_type_canary));
        assert!(!format!("{anthropic_type_error:?}").contains(anthropic_type_canary));

        let stored_family_canary = "opaque-stored-family-mismatch-canary";
        let stored_family_mismatch_json = serde_json::json!([{
            "kind": "gemini_thought_signature",
            "part_index": 0,
            "signature": stored_family_canary
        }])
        .to_string();
        reopened
            .connection()
            .expect("database")
            .execute(
                "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
                params![generation_id.0, stored_family_mismatch_json],
            )
            .expect("inject valid state bound to the wrong provider family");
        let stored_family_error = reopened
            .get_generation(&generation_id)
            .expect_err("stored provider-family mismatch must fail closed");
        assert_eq!(stored_family_error.code, CoreErrorCode::StorageCorrupted);
        assert!(!stored_family_error.message.contains(stored_family_canary));
        assert!(!format!("{stored_family_error:?}").contains(stored_family_canary));

        let openai_corruption_canary = "opaque-openai-corrupt-canary";
        let openai_corrupt_json = serde_json::json!([{
            "kind": "open_ai_responses",
            "item": {
                "id": "bounded-openai-item",
                "type": "reasoning",
                "summary": [{
                    "type": "summary_text",
                    "text": format!(
                        "{openai_corruption_canary}{}",
                        "o".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES)
                    )
                }],
                "status": "completed"
            }
        }])
        .to_string();
        reopened
            .connection()
            .expect("database")
            .execute(
                "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
                params![generation_id.0, openai_corrupt_json],
            )
            .expect("inject invalid OpenAI Responses reasoning item");
        let openai_corruption_error = reopened
            .get_generation(&generation_id)
            .expect_err("oversized OpenAI Responses reasoning item must fail closed");
        assert_eq!(
            openai_corruption_error.code,
            CoreErrorCode::StorageCorrupted
        );
        assert!(
            !openai_corruption_error
                .message
                .contains(openai_corruption_canary)
        );
        assert!(!format!("{openai_corruption_error:?}").contains(openai_corruption_canary));

        let anthropic_topology_canary = "opaque-anthropic-topology-canary";
        let anthropic_invalid_topology_json = serde_json::json!([{
            "kind": "anthropic_messages",
            "content_blocks": [{
                "type": "text",
                "text": anthropic_topology_canary
            }]
        }])
        .to_string();
        reopened
            .connection()
            .expect("database")
            .execute(
                "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
                params![generation_id.0, anthropic_invalid_topology_json],
            )
            .expect("inject Anthropic topology without thinking state");
        let anthropic_topology_error = reopened
            .get_generation(&generation_id)
            .expect_err("Anthropic topology without thinking must fail closed");
        assert!(
            !anthropic_topology_error
                .message
                .contains(anthropic_topology_canary)
        );
        assert!(!format!("{anthropic_topology_error:?}").contains(anthropic_topology_canary));

        let anthropic_corruption_canary = "opaque-anthropic-corrupt-canary";
        let anthropic_corrupt_json = serde_json::json!([{
            "kind": "anthropic_messages",
            "content_blocks": [{
                "type": "thinking",
                "thinking": "bounded",
                "signature": format!(
                    "{anthropic_corruption_canary}{}",
                    "s".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES)
                )
            }]
        }])
        .to_string();
        reopened
            .connection()
            .expect("database")
            .execute(
                "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
                params![generation_id.0, anthropic_corrupt_json],
            )
            .expect("inject invalid Anthropic opaque topology");
        let anthropic_corruption_error = reopened
            .get_generation(&generation_id)
            .expect_err("oversized Anthropic stored topology must fail closed");
        assert!(
            !anthropic_corruption_error
                .message
                .contains(anthropic_corruption_canary)
        );
        assert!(!format!("{anthropic_corruption_error:?}").contains(anthropic_corruption_canary));

        let corruption_canary = "opaque-corrupt-payload-canary";
        let corrupt_json = serde_json::json!([{
            "kind": "gemini_thought_signature",
            "part_index": 0,
            "signature": format!(
                "{corruption_canary}{}",
                "c".repeat(lorepia_domain::MAX_OPAQUE_REASONING_ITEM_BYTES)
            )
        }])
        .to_string();
        reopened
            .connection()
            .expect("database")
            .execute(
                "UPDATE generations
                 SET opaque_reasoning_state_json = ?2
                 WHERE id = ?1",
                params![generation_id.0, corrupt_json],
            )
            .expect("inject bounded-JSON but invalid opaque payload");
        let corruption_error = reopened
            .get_generation(&generation_id)
            .expect_err("oversized stored opaque payload must fail closed");
        assert!(!corruption_error.message.contains(corruption_canary));
        assert!(!format!("{corruption_error:?}").contains(corruption_canary));
    }

    #[test]
    fn terminal_database_failure_is_compensated_without_raw_error_text() {
        let (_root, storage, conversation, branch_id) = imported_storage();
        let (_user, pending, generation) = append_pending_generation(
            &storage,
            &conversation.id,
            &branch_id,
            None,
            "trigger failure",
        );
        storage
            .connection()
            .expect("connection")
            .execute_batch(
                "CREATE TEMP TRIGGER reject_complete_generation
                 BEFORE UPDATE OF status ON generations
                 WHEN NEW.status = 'complete'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic terminal database failure');
                 END;",
            )
            .expect("install synthetic failure");

        let mut assistant = pending;
        assistant.content = "completed provider response".to_owned();
        assistant.status = MessageStatus::Complete;
        let error = storage
            .finalize_generation(&assistant, None, None, true)
            .expect_err("synthetic terminal update must fail");
        assert_eq!(error.code, CoreErrorCode::StorageUnavailable);

        assistant.status = MessageStatus::Failed;
        storage
            .fail_generation_after_finalize_error(&assistant, true)
            .expect("compensate terminal database failure");
        let failed = storage
            .get_generation(&generation.id)
            .expect("failed generation");
        assert_eq!(failed.status, GenerationStatus::Failed);
        assert_eq!(
            failed.error_code.as_deref(),
            Some(CoreErrorCode::StorageUnavailable.as_str())
        );
        assert!(
            !failed
                .error_code
                .as_deref()
                .unwrap_or_default()
                .contains("synthetic")
        );
        let messages = storage
            .list_branch_messages(&branch_id)
            .expect("failed lineage");
        assert_eq!(messages[1].status, MessageStatus::Failed);
        assert_eq!(messages[1].content, "completed provider response");
    }

    #[test]
    fn compensation_never_regresses_an_already_terminal_generation() {
        let (_root, storage, conversation, branch_id) = imported_storage();
        let (_, complete) = append_complete_generation(
            &storage,
            &conversation.id,
            &branch_id,
            None,
            "already complete",
            "durable response",
        );
        let generation_id = complete
            .generation_id
            .clone()
            .expect("assistant generation id");
        let mut attempted_compensation = complete.clone();
        attempted_compensation.status = MessageStatus::Failed;
        let error = storage
            .fail_generation_after_finalize_error(&attempted_compensation, true)
            .expect_err("terminal generation must reject compensation");
        assert_eq!(error.code, CoreErrorCode::NotFound);

        let generation = storage
            .get_generation(&generation_id)
            .expect("terminal generation");
        assert_eq!(generation.status, GenerationStatus::Complete);
        assert_eq!(generation.error_code, None);
        let messages = storage
            .list_branch_messages(&branch_id)
            .expect("terminal lineage");
        assert_eq!(messages[1].status, MessageStatus::Complete);
        assert_eq!(messages[1].content, "durable response");
    }

    #[test]
    fn discarded_partial_compensation_rewinds_the_branch_head() {
        let (_root, storage, conversation, branch_id) = imported_storage();
        let (user, pending, generation) = append_pending_generation(
            &storage,
            &conversation.id,
            &branch_id,
            None,
            "discard partial",
        );
        let mut assistant = pending;
        assistant.content = "partial response".to_owned();
        assistant.status = MessageStatus::Failed;
        let error = storage
            .finalize_generation(
                &assistant,
                Some(&lorepia_domain::GenerationUsage {
                    input_tokens: Some(i64::MAX as u64 + 1),
                    output_tokens: None,
                    ..lorepia_domain::GenerationUsage::default()
                }),
                Some(CoreErrorCode::ProviderUnavailable.as_str()),
                false,
            )
            .expect_err("overflow usage must reject normal finalization");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        storage
            .fail_generation_after_finalize_error(&assistant, false)
            .expect("discard compensated partial");
        assert_eq!(
            storage
                .get_generation(&generation.id)
                .expect("failed generation")
                .status,
            GenerationStatus::Failed
        );
        let branch = storage
            .get_conversation_branch(&branch_id)
            .expect("compensated branch");
        assert_eq!(branch.head_message_id, Some(user.id.clone()));
        assert_eq!(
            storage
                .list_branch_messages(&branch_id)
                .expect("rewound lineage"),
            vec![user]
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn v3_provider_catalog_migrates_to_v11_with_profiles_selection_and_base_paths() {
        let root = tempdir().expect("temp root");
        let connection = version_three_provider_database(root.path());
        insert_legacy_provider_profile(
            &connection,
            (
                "remote",
                "Remote",
                "https://api.example.test/openai/v1",
                "remote-model",
                45,
            ),
        );
        insert_legacy_provider_profile(
            &connection,
            (
                "local",
                "Local",
                "http://127.0.0.1:11434/v1",
                "local-model",
                30,
            ),
        );
        connection
            .execute(
                "INSERT INTO app_settings(key, value_json)
                 VALUES ('application', ?1)",
                [
                    r#"{"preserve_partial_generations":false,"selected_provider_profile_id":"remote"}"#,
                ],
            )
            .expect("legacy settings");
        drop(connection);

        let storage = Storage::open(root.path()).expect("migrate provider catalog");
        assert_eq!(
            storage
                .schema_version()
                .expect("read durable schema version"),
            SCHEMA_VERSION
        );
        let templates = storage.list_provider_templates().expect("templates");
        assert_eq!(
            templates,
            vec![legacy_provider_template().expect("template")]
        );
        let stored_template = storage
            .get_provider_template(
                &ProviderTemplateId::from(LEGACY_PROVIDER_TEMPLATE_ID),
                LEGACY_PROVIDER_TEMPLATE_VERSION,
            )
            .expect("built-in template");
        assert_eq!(stored_template.source, TemplateSource::BuiltIn);

        let connections = storage.list_provider_connections().expect("connections");
        assert_eq!(connections.len(), 2);
        let remote = storage
            .get_provider_connection(&ProviderConnectionId::from("remote"))
            .expect("remote connection");
        assert_eq!(remote.api_origin.as_str(), "https://api.example.test");
        assert_eq!(remote.config.network_mode, ProviderNetworkMode::Public);
        assert_eq!(
            remote
                .config
                .api_base_path
                .as_ref()
                .map(EndpointPath::as_str),
            Some("/openai/v1")
        );
        assert_eq!(
            remote.config.values,
            vec![ConnectionConfigEntry {
                key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
                value: ConnectionConfigValue::Text("https://api.example.test/openai/v1".to_owned()),
            }]
        );
        assert_eq!(
            remote.credential_ref.as_ref().map(CredentialRef::as_str),
            Some("remote")
        );
        assert_eq!(
            remote
                .credential_scope
                .as_ref()
                .expect("credential scope")
                .allowed_origins,
            vec![CanonicalOrigin::parse("https://api.example.test").expect("origin")]
        );
        assert_eq!(
            storage
                .get_provider_connection(&ProviderConnectionId::from("local"))
                .expect("local connection")
                .config
                .network_mode,
            ProviderNetworkMode::LocalLoopback
        );

        let routes = storage
            .list_model_routes(&ProviderConnectionId::from("remote"))
            .expect("routes");
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].id.as_str(), "remote");
        assert_eq!(routes[0].model_id, "remote-model");
        assert_eq!(routes[0].status, ModelAvailability::Available);
        let presets = storage
            .list_generation_presets(&ModelRouteId::from("remote"))
            .expect("presets");
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].id.as_str(), "remote");
        assert_eq!(
            presets[0].values,
            vec![
                ParameterValue {
                    parameter_id: ParameterId::from(TEMPERATURE_PARAMETER_ID),
                    state: ParameterValueState::Explicit(ParameterLiteral::Number(1.0)),
                },
                ParameterValue {
                    parameter_id: ParameterId::from(MAX_OUTPUT_TOKENS_PARAMETER_ID),
                    state: ParameterValueState::Explicit(ParameterLiteral::Integer(4096)),
                },
            ]
        );
        let settings = storage.load_settings().expect("migrated settings");
        assert_eq!(
            settings.selected_provider_profile_id.as_deref(),
            Some("remote")
        );
        assert_eq!(
            settings
                .selected_model_route_id
                .as_ref()
                .map(ModelRouteId::as_str),
            Some("remote")
        );
        assert_eq!(
            settings
                .selected_generation_preset_id
                .as_ref()
                .map(GenerationPresetId::as_str),
            Some("remote")
        );
        {
            let connection = storage.connection().expect("connection");
            assert_eq!(
                count(&connection, "provider_profiles").expect("profiles"),
                2
            );
            let (manifest_json, manifest_sha256) = connection
                .query_row(
                    "SELECT manifest_json, manifest_sha256
                     FROM provider_templates WHERE id = ?1 AND version = ?2",
                    params![
                        LEGACY_PROVIDER_TEMPLATE_ID,
                        LEGACY_PROVIDER_TEMPLATE_VERSION
                    ],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .expect("manifest and hash");
            assert_eq!(
                manifest_sha256,
                hex::encode(Sha256::digest(manifest_json.as_bytes()))
            );
            validate_provider_catalog_foreign_keys(&connection).expect("foreign keys");
        }
        let before_reopen = (
            storage.list_provider_connections().expect("connections"),
            storage
                .list_model_routes(&ProviderConnectionId::from("remote"))
                .expect("routes"),
            storage
                .list_generation_presets(&ModelRouteId::from("remote"))
                .expect("presets"),
            storage.load_settings().expect("settings"),
        );
        drop(storage);

        let reopened = Storage::open(root.path()).expect("reopen migrated catalog");
        assert_eq!(
            (
                reopened
                    .list_provider_connections()
                    .expect("reopened connections"),
                reopened
                    .list_model_routes(&ProviderConnectionId::from("remote"))
                    .expect("reopened routes"),
                reopened
                    .list_generation_presets(&ModelRouteId::from("remote"))
                    .expect("reopened presets"),
                reopened.load_settings().expect("reopened settings"),
            ),
            before_reopen
        );
    }

    #[test]
    fn empty_provider_catalog_migration_seeds_only_the_builtin_template() {
        let root = tempdir().expect("temp root");
        drop(version_three_provider_database(root.path()));

        let storage = Storage::open(root.path()).expect("migrate empty database");
        assert_eq!(
            storage.list_provider_templates().expect("templates").len(),
            1
        );
        assert!(
            storage
                .list_provider_connections()
                .expect("connections")
                .is_empty()
        );
        let settings = storage.load_settings().expect("settings");
        assert!(!settings.local_user_id.as_str().is_empty());
        assert!(settings.preserve_partial_generations);
        assert!(settings.selected_provider_profile_id.is_none());
        assert!(settings.selected_model_route_id.is_none());
        assert!(settings.selected_generation_preset_id.is_none());
        let local_user_id = settings.local_user_id;
        drop(storage);
        assert_eq!(
            Storage::open(root.path())
                .expect("reopen migrated empty database")
                .load_settings()
                .expect("reopened settings")
                .local_user_id,
            local_user_id,
            "first-load local identity must be persisted instead of regenerated"
        );
    }

    #[test]
    fn provider_template_versions_are_hashed_idempotent_and_immutable() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let mut template = legacy_provider_template().expect("template");
        template.id = ProviderTemplateId::from("user-template");
        template.display_name = "User template".to_owned();
        template.source = TemplateSource::UserDiscovered;

        storage
            .save_provider_template(&template)
            .expect("save template");
        storage
            .save_provider_template(&template)
            .expect("idempotent save");
        assert_eq!(
            storage
                .get_provider_template(&template.id, template.manifest_version)
                .expect("roundtrip template"),
            template
        );
        assert_eq!(
            storage
                .connection()
                .expect("connection")
                .query_row(
                    "SELECT COUNT(*) FROM provider_templates
                     WHERE id = 'user-template' AND version = 1",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("template count"),
            1
        );

        let mut conflicting = template.clone();
        conflicting.display_name = "Conflicting payload".to_owned();
        let error = storage
            .save_provider_template(&conflicting)
            .expect_err("same version must be immutable");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            storage
                .get_provider_template(&template.id, template.manifest_version)
                .expect("unchanged template"),
            template
        );

        let mut next_version = conflicting;
        next_version.manifest_version = 2;
        storage
            .save_provider_template(&next_version)
            .expect("save next version");
        assert_eq!(
            storage
                .get_provider_template(&next_version.id, 2)
                .expect("next version"),
            next_version
        );
        let connection = storage.connection().expect("connection");
        let (manifest_json, manifest_sha256) = connection
            .query_row(
                "SELECT manifest_json, manifest_sha256
                 FROM provider_templates
                 WHERE id = 'user-template' AND version = 2",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("stored template");
        assert_eq!(
            manifest_sha256,
            hex::encode(Sha256::digest(manifest_json.as_bytes()))
        );
    }

    #[test]
    fn invalid_legacy_provider_catalog_data_rolls_back_schema_four() {
        for fixture in ["remote-http", "invalid-timeout", "dangling-selection"] {
            let root = tempdir().expect("temp root");
            let connection = version_three_provider_database(root.path());
            match fixture {
                "remote-http" => insert_legacy_provider_profile(
                    &connection,
                    (
                        "invalid",
                        "Invalid",
                        "http://api.example.test/v1",
                        "model",
                        30,
                    ),
                ),
                "invalid-timeout" => insert_legacy_provider_profile(
                    &connection,
                    (
                        "invalid",
                        "Invalid",
                        "https://api.example.test/v1",
                        "model",
                        0,
                    ),
                ),
                "dangling-selection" => {
                    connection
                        .execute(
                            "INSERT INTO app_settings(key, value_json)
                             VALUES ('application', ?1)",
                            [
                                r#"{"preserve_partial_generations":true,"selected_provider_profile_id":"missing"}"#,
                            ],
                        )
                        .expect("dangling settings");
                }
                _ => unreachable!(),
            }
            drop(connection);

            let Err(error) = Storage::open(root.path()) else {
                panic!("{fixture} must fail migration");
            };
            assert_eq!(error.code, CoreErrorCode::StorageCorrupted, "{fixture}");
            let connection =
                Connection::open(root.path().join("db/lorepia.sqlite3")).expect("database");
            assert_eq!(
                connection
                    .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                        row.get::<_, u32>(0)
                    })
                    .expect("schema version"),
                3,
                "{fixture}"
            );
            assert_eq!(
                connection
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'table' AND name = 'provider_templates'",
                        [],
                        |row| row.get::<_, u32>(0),
                    )
                    .expect("provider template table"),
                0,
                "{fixture}"
            );
        }
    }

    #[test]
    fn provider_catalog_integrity_checks_detect_row_and_foreign_key_mismatches() {
        let mut connection = Connection::open_in_memory().expect("database");
        connection
            .execute_batch(MIGRATION_0001)
            .expect("initial schema");
        connection
            .execute_batch(MIGRATION_0004)
            .expect("provider schema");
        {
            let transaction = connection.transaction().expect("transaction");
            insert_legacy_provider_template(&transaction).expect("template");
            transaction
                .execute(
                    "INSERT INTO provider_profiles
                     (id, display_name, base_url, model, timeout_seconds)
                     VALUES ('unmigrated', 'Unmigrated', 'https://api.example.test/v1',
                             'model', 30)",
                    [],
                )
                .expect("legacy profile");
            let error = validate_provider_catalog_migration(&transaction)
                .expect_err("row mismatch must fail");
            assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        }

        connection
            .pragma_update(None, "foreign_keys", false)
            .expect("disable enforcement for corruption fixture");
        connection
            .execute(
                "INSERT INTO provider_models
                 (id, connection_id, api_family, model_id, display_name, route_json,
                  availability, raw_metadata_json, first_seen_at, last_seen_at)
                 VALUES ('orphan', 'missing', 'openai_chat_completions', 'model',
                         NULL, '{}', 'available', NULL, ?1, NULL)",
                ["2026-01-01T00:00:00Z"],
            )
            .expect("orphan fixture");
        let error = validate_provider_catalog_foreign_keys(&connection)
            .expect_err("foreign key mismatch must fail");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }

    #[test]
    fn capability_observation_table_enforces_json_enums_and_route_foreign_keys() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .save_provider_profile(&ProviderProfile {
                id: "observed".to_owned(),
                display_name: "Observed".to_owned(),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "model".to_owned(),
                timeout_seconds: 30,
            })
            .expect("seed route");
        let connection = storage.connection().expect("connection");
        connection
            .execute(
                "INSERT INTO model_capability_observations
                 (id, model_route_id, capability_key, value_json, support_status,
                  source_kind, confidence, evidence_ref, observed_at, expires_at)
                 VALUES ('valid', 'observed', 'streaming', 'true', 'verified',
                         'provider_api', 'high', NULL, ?1, NULL)",
                ["2026-01-01T00:00:00Z"],
            )
            .expect("valid observation");
        for (id, route_id, capability_key, value_json) in [
            ("bad-route", "missing", "streaming", "true"),
            ("bad-capability", "observed", "arbitrary_script", "true"),
            ("bad-json", "observed", "streaming", "not-json"),
        ] {
            assert!(
                connection
                    .execute(
                        "INSERT INTO model_capability_observations
                         (id, model_route_id, capability_key, value_json, support_status,
                          source_kind, confidence, evidence_ref, observed_at, expires_at)
                         VALUES (?1, ?2, ?3, ?4, 'verified', 'provider_api', 'high',
                                 NULL, ?5, NULL)",
                        params![
                            id,
                            route_id,
                            capability_key,
                            value_json,
                            "2026-01-01T00:00:00Z"
                        ],
                    )
                    .is_err(),
                "{id} must violate table integrity"
            );
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM model_capability_observations",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("observation count"),
            1
        );
    }

    #[test]
    fn capability_observation_crud_is_typed_monotonic_and_secret_free() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .save_provider_profile(&ProviderProfile {
                id: "capability-crud".to_owned(),
                display_name: "Capability CRUD".to_owned(),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "model".to_owned(),
                timeout_seconds: 30,
            })
            .expect("seed model route");
        let observed_at = Utc::now();
        let mut observation = CapabilityObservation {
            id: ObservationId::from("provider-api:context-window"),
            model_route_id: ModelRouteId::from("capability-crud"),
            key: CapabilityKey::ContextWindow,
            value: CapabilityValue::Integer(32_768),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at,
            expires_at: Some(observed_at + Duration::hours(24)),
            evidence_ref: None,
        };
        storage
            .upsert_capability_observation(&observation)
            .expect("insert observation");
        storage
            .upsert_capability_observation(&observation)
            .expect("idempotent observation");
        assert_eq!(
            storage
                .get_capability_observation(&observation.id)
                .expect("stored observation"),
            observation
        );
        assert_eq!(
            storage
                .list_capability_observations_for_key(
                    &observation.model_route_id,
                    CapabilityKey::ContextWindow,
                )
                .expect("observations"),
            vec![observation.clone()]
        );

        let original = observation.clone();
        observation.observed_at += Duration::minutes(1);
        observation.expires_at = Some(observation.observed_at + Duration::hours(24));
        observation.value = CapabilityValue::Integer(65_536);
        storage
            .upsert_capability_observation(&observation)
            .expect("advance provider observation");
        assert_eq!(
            storage
                .get_capability_observation(&observation.id)
                .expect("updated observation"),
            observation
        );
        assert!(
            storage
                .upsert_capability_observation(&original)
                .expect_err("older observation must not overwrite current evidence")
                .message
                .contains("backwards")
        );

        let secret_metadata = CapabilityObservation {
            id: ObservationId::from("secret-metadata"),
            model_route_id: ModelRouteId::from("capability-crud"),
            key: CapabilityKey::Reasoning,
            value: CapabilityValue::Structured(serde_json::json!({
                "dialect": "open_ai_responses",
                "api_key": "must-not-persist",
            })),
            status: SupportStatus::Documented,
            source: ObservationSource::OfficialDocumentation,
            confidence: Confidence::High,
            observed_at,
            expires_at: None,
            evidence_ref: None,
        };
        assert!(
            storage
                .upsert_capability_observation(&secret_metadata)
                .expect_err("secret-like metadata must be rejected")
                .message
                .contains("credentials")
        );

        storage
            .delete_capability_observation(&observation.id)
            .expect("delete observation");
        assert_eq!(
            storage
                .list_capability_observations(&ModelRouteId::from("capability-crud"))
                .expect("empty observations"),
            Vec::<CapabilityObservation>::new()
        );
    }

    #[test]
    fn capability_observation_batch_is_atomic() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .save_provider_profile(&ProviderProfile {
                id: "capability-batch".to_owned(),
                display_name: "Capability Batch".to_owned(),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "model".to_owned(),
                timeout_seconds: 30,
            })
            .expect("seed model route");
        let observed_at = Utc::now();
        let valid = CapabilityObservation {
            id: ObservationId::from("valid-batch-observation"),
            model_route_id: ModelRouteId::from("capability-batch"),
            key: CapabilityKey::Streaming,
            value: CapabilityValue::Boolean(true),
            status: SupportStatus::Verified,
            source: ObservationSource::CapabilityProbe,
            confidence: Confidence::High,
            observed_at,
            expires_at: Some(observed_at + Duration::hours(1)),
            evidence_ref: None,
        };
        let mut invalid = valid.clone();
        invalid.id = ObservationId::from("invalid-batch-observation");
        invalid.model_route_id = ModelRouteId::from("missing-route");
        storage
            .upsert_capability_observations(&[valid, invalid])
            .expect_err("invalid batch must roll back");
        assert_eq!(
            storage
                .list_capability_observations(&ModelRouteId::from("capability-batch"))
                .expect("rolled back observations"),
            Vec::<CapabilityObservation>::new()
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn model_refresh_replaces_only_listed_provider_api_snapshots_atomically_and_reopens() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let (_, main_route, _) = save_modern_direct_provider_graph(
            &storage,
            &ProviderProfile {
                id: "observation-refresh".to_owned(),
                display_name: "Observation refresh".to_owned(),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "main-model".to_owned(),
                timeout_seconds: 30,
            },
        );
        let connection_id = ProviderConnectionId::from("observation-refresh");
        let main_route_id = ModelRouteId::from("observation-refresh");
        let mut omitted_route = main_route.clone();
        omitted_route.id = ModelRouteId::from("observation-refresh-omitted");
        omitted_route.model_id = "omitted-model".to_owned();
        omitted_route.display_name = Some("Omitted model".to_owned());
        storage
            .save_model_route(&omitted_route)
            .expect("seed route omitted by the second refresh");

        let seed_time = Utc::now();
        let preserved_observation =
            |id: &str, key: CapabilityKey, source: ObservationSource| CapabilityObservation {
                id: ObservationId::from(id),
                model_route_id: main_route_id.clone(),
                key,
                value: CapabilityValue::Boolean(true),
                status: SupportStatus::Verified,
                source,
                confidence: Confidence::High,
                observed_at: seed_time,
                expires_at: None,
                evidence_ref: None,
            };
        let signed = preserved_observation(
            "refresh:signed:reasoning",
            CapabilityKey::Reasoning,
            ObservationSource::SignedLorepiaCatalog,
        );
        let probe = preserved_observation(
            "refresh:probe:tool-calling",
            CapabilityKey::ToolCalling,
            ObservationSource::CapabilityProbe,
        );
        let user = preserved_observation(
            "refresh:user:seed",
            CapabilityKey::Seed,
            ObservationSource::UserOverride,
        );
        let legacy_prompt_caching = CapabilityObservation {
            id: ObservationId::from("refresh:provider-api:legacy-prompt-caching"),
            model_route_id: main_route_id.clone(),
            key: CapabilityKey::PromptCaching,
            value: CapabilityValue::Boolean(true),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at: seed_time,
            expires_at: None,
            evidence_ref: None,
        };
        storage
            .upsert_capability_observations(&[
                signed.clone(),
                probe.clone(),
                user.clone(),
                legacy_prompt_caching.clone(),
            ])
            .expect("seed observations from preserved sources");

        let first_observed_at = seed_time + Duration::minutes(1);
        let provider_observation =
            |id: &str, route_id: &ModelRouteId, key: CapabilityKey, value: CapabilityValue| {
                CapabilityObservation {
                    id: ObservationId::from(id),
                    model_route_id: route_id.clone(),
                    key,
                    value,
                    status: SupportStatus::Verified,
                    source: ObservationSource::ProviderApi,
                    confidence: Confidence::High,
                    observed_at: first_observed_at,
                    expires_at: Some(first_observed_at + Duration::hours(24)),
                    evidence_ref: None,
                }
            };
        let context_window = provider_observation(
            "refresh:provider-api:context-window",
            &main_route_id,
            CapabilityKey::ContextWindow,
            CapabilityValue::Integer(128_000),
        );
        let max_output = provider_observation(
            "refresh:provider-api:max-output",
            &main_route_id,
            CapabilityKey::MaxOutputTokens,
            CapabilityValue::Integer(8_192),
        );
        let unsupported_parallel_tools = CapabilityObservation {
            id: ObservationId::from("refresh:provider-api:parallel-tools"),
            model_route_id: main_route_id.clone(),
            key: CapabilityKey::ParallelToolCalling,
            value: CapabilityValue::Boolean(false),
            status: SupportStatus::Unsupported,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at: first_observed_at,
            expires_at: Some(first_observed_at + Duration::hours(24)),
            evidence_ref: None,
        };
        let omitted_route_context = provider_observation(
            "refresh:provider-api:omitted-route-context",
            &omitted_route.id,
            CapabilityKey::ContextWindow,
            CapabilityValue::Integer(32_768),
        );
        let expected_first = storage
            .get_provider_connection(&connection_id)
            .expect("connection before first refresh");
        let mut refreshed_first = expected_first.clone();
        refreshed_first.status = ConnectionStatus::Connected;
        refreshed_first.updated_at = first_observed_at;
        storage
            .commit_model_refresh(
                &expected_first,
                &refreshed_first,
                &[main_route.clone(), omitted_route.clone()],
                &[],
                &[
                    context_window.clone(),
                    max_output.clone(),
                    unsupported_parallel_tools.clone(),
                    omitted_route_context.clone(),
                ],
                first_observed_at,
            )
            .expect("commit first provider API snapshot");

        let after_first = storage
            .list_capability_observations(&main_route_id)
            .expect("main observations after first refresh");
        for expected in [
            &context_window,
            &max_output,
            &unsupported_parallel_tools,
            &signed,
            &probe,
            &user,
        ] {
            assert!(after_first.contains(expected));
        }
        assert!(!after_first.contains(&legacy_prompt_caching));

        let expected_second = storage
            .get_provider_connection(&connection_id)
            .expect("connection before second refresh");
        let mut refreshed_second = expected_second.clone();
        let second_observed_at = first_observed_at + Duration::minutes(1);
        refreshed_second.updated_at = second_observed_at;
        let listed_main = storage
            .get_model_route(&main_route_id)
            .expect("listed route before second refresh");
        let invalid_unlisted_observation = CapabilityObservation {
            id: ObservationId::from("refresh:provider-api:unlisted"),
            model_route_id: omitted_route.id.clone(),
            key: CapabilityKey::MaxOutputTokens,
            value: CapabilityValue::Integer(1_024),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at: second_observed_at,
            expires_at: Some(second_observed_at + Duration::hours(24)),
            evidence_ref: None,
        };
        storage
            .commit_model_refresh(
                &expected_second,
                &refreshed_second,
                std::slice::from_ref(&listed_main),
                &[],
                &[invalid_unlisted_observation],
                second_observed_at,
            )
            .expect_err("unlisted observation must roll back snapshot deletion");
        let after_rollback = storage
            .list_capability_observations(&main_route_id)
            .expect("main observations after rollback");
        for expected in [&context_window, &max_output, &unsupported_parallel_tools] {
            assert!(
                after_rollback.contains(expected),
                "provider API observation must survive a rolled-back refresh"
            );
        }
        assert_eq!(
            storage
                .get_provider_connection(&connection_id)
                .expect("connection after rollback"),
            expected_second
        );
        let foreign_source_observation = CapabilityObservation {
            id: ObservationId::from("refresh:signed:foreign-source"),
            model_route_id: main_route_id.clone(),
            key: CapabilityKey::ToolCalling,
            value: CapabilityValue::Boolean(true),
            status: SupportStatus::Verified,
            source: ObservationSource::SignedLorepiaCatalog,
            confidence: Confidence::High,
            observed_at: second_observed_at,
            expires_at: Some(second_observed_at + Duration::hours(24)),
            evidence_ref: None,
        };
        storage
            .commit_model_refresh(
                &expected_second,
                &refreshed_second,
                std::slice::from_ref(&listed_main),
                &[],
                &[foreign_source_observation],
                second_observed_at,
            )
            .expect_err("direct provider API snapshot must reject a foreign source");
        let after_foreign_source = storage
            .list_capability_observations(&main_route_id)
            .expect("observations after foreign-source rejection");
        for expected in [&context_window, &max_output, &unsupported_parallel_tools] {
            assert!(after_foreign_source.contains(expected));
        }
        assert_eq!(
            storage
                .get_provider_connection(&connection_id)
                .expect("connection after foreign-source rejection"),
            expected_second
        );

        storage
            .connection()
            .expect("database")
            .execute_batch(
                "CREATE TEMP TRIGGER reject_direct_snapshot_publish
                 BEFORE UPDATE ON provider_connections
                 WHEN OLD.id = 'observation-refresh'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic direct refresh publish failure');
                 END;",
            )
            .expect("install rollback trigger");
        storage
            .commit_model_refresh(
                &expected_second,
                &refreshed_second,
                std::slice::from_ref(&listed_main),
                &[],
                &[],
                second_observed_at,
            )
            .expect_err("post-delete direct refresh failure must roll back the snapshot");
        let after_publish_rollback = storage
            .list_capability_observations(&main_route_id)
            .expect("observations after publish rollback");
        for expected in [&context_window, &max_output, &unsupported_parallel_tools] {
            assert!(
                after_publish_rollback.contains(expected),
                "provider API observation must survive a post-delete rollback"
            );
        }
        storage
            .connection()
            .expect("database")
            .execute_batch("DROP TRIGGER reject_direct_snapshot_publish;")
            .expect("remove rollback trigger");

        storage
            .commit_model_refresh(
                &expected_second,
                &refreshed_second,
                std::slice::from_ref(&listed_main),
                &[],
                &[],
                second_observed_at,
            )
            .expect("commit provider API snapshot with omitted numeric limits");
        let after_second = storage
            .list_capability_observations(&main_route_id)
            .expect("main observations after second refresh");
        assert!(after_second.contains(&signed));
        assert!(after_second.contains(&probe));
        assert!(after_second.contains(&user));
        assert!(
            after_second
                .iter()
                .all(|observation| observation.source != ObservationSource::ProviderApi),
            "the listed route must not retain omitted provider API observations"
        );
        assert_eq!(
            storage
                .list_capability_observations(&omitted_route.id)
                .expect("omitted route observations"),
            vec![omitted_route_context.clone()]
        );

        drop(storage);
        let reopened = Storage::open(root.path()).expect("reopen refreshed catalog");
        let reopened_main = reopened
            .list_capability_observations(&main_route_id)
            .expect("main observations after reopen");
        assert!(reopened_main.contains(&signed));
        assert!(reopened_main.contains(&probe));
        assert!(reopened_main.contains(&user));
        assert!(
            reopened_main
                .iter()
                .all(|observation| observation.source != ObservationSource::ProviderApi)
        );
        assert_eq!(
            reopened
                .list_capability_observations(&omitted_route.id)
                .expect("omitted route observations after reopen"),
            vec![omitted_route_context]
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn provider_profile_writes_dual_write_the_catalog_atomically() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let original = ProviderProfile {
            id: "dual".to_owned(),
            display_name: "Original".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "model-one".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&original)
            .expect("save original");
        assert_eq!(
            storage
                .get_provider_connection(&ProviderConnectionId::from("dual"))
                .expect("connection")
                .display_name,
            "Original"
        );
        assert_eq!(
            storage
                .get_model_route(&ModelRouteId::from("dual"))
                .expect("route")
                .model_id,
            "model-one"
        );
        assert_eq!(
            storage
                .get_generation_preset(&GenerationPresetId::from("dual"))
                .expect("preset")
                .values
                .len(),
            2
        );

        let raw_identity_before = raw_legacy_provider_identity_rows(&storage, "dual");
        let endpoint_mutation = ProviderProfile {
            base_url: "https://api.example.test/openai/v2".to_owned(),
            ..original.clone()
        };
        let error = storage
            .save_provider_profile(&endpoint_mutation)
            .expect_err("stable legacy connection ID must not retarget its endpoint");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(
            error.message.contains("create a new connection"),
            "endpoint mutation error must direct callers to a new identity"
        );
        assert_eq!(
            storage
                .get_provider_profile("dual")
                .expect("profile after rejected endpoint mutation"),
            original
        );
        assert_eq!(
            storage
                .list_model_routes(&ProviderConnectionId::from("dual"))
                .expect("routes after rejected endpoint mutation")
                .len(),
            1
        );
        assert_eq!(
            raw_legacy_provider_identity_rows(&storage, "dual"),
            raw_identity_before,
            "the rejected endpoint mutation must leave both identity rows byte-exact"
        );

        drop(storage);
        let storage = Storage::open(root.path()).expect("reopen after rejected endpoint mutation");
        assert_eq!(
            raw_legacy_provider_identity_rows(&storage, "dual"),
            raw_identity_before,
            "the exact identity rows must remain unchanged after rollback and reopen"
        );
        assert_eq!(
            storage
                .get_provider_profile("dual")
                .expect("profile after rejected endpoint mutation and reopen"),
            original
        );

        storage
            .connection()
            .expect("connection")
            .execute_batch(
                "CREATE TEMP TRIGGER reject_connection_update
                 BEFORE UPDATE ON provider_connections
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic catalog failure');
                 END;",
            )
            .expect("failure trigger");

        let updated = ProviderProfile {
            display_name: "Updated".to_owned(),
            model: "model-two".to_owned(),
            timeout_seconds: 60,
            ..original.clone()
        };
        let error = storage
            .save_provider_profile(&updated)
            .expect_err("dual write must roll back");
        assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
        assert_eq!(
            storage
                .get_provider_profile("dual")
                .expect("legacy profile after rollback"),
            original
        );
        assert_eq!(
            storage
                .get_model_route(&ModelRouteId::from("dual"))
                .expect("route after rollback")
                .model_id,
            "model-one"
        );
        storage
            .connection()
            .expect("connection")
            .execute_batch("DROP TRIGGER reject_connection_update;")
            .expect("drop failure trigger");

        storage
            .save_provider_profile(&updated)
            .expect("save updated profile");
        let connection = storage
            .get_provider_connection(&ProviderConnectionId::from("dual"))
            .expect("updated connection");
        assert_eq!(connection.display_name, "Updated");
        assert_eq!(
            connection
                .config
                .api_base_path
                .as_ref()
                .map(EndpointPath::as_str),
            Some("/v1")
        );
        assert_eq!(
            storage
                .get_model_route(&ModelRouteId::from("dual"))
                .expect("preserved original route")
                .model_id,
            "model-one"
        );
        assert_eq!(
            storage
                .get_generation_preset(&GenerationPresetId::from("dual"))
                .expect("preserved original preset")
                .values
                .len(),
            2
        );
        let updated_route = storage
            .list_model_routes(&ProviderConnectionId::from("dual"))
            .expect("updated model routes")
            .into_iter()
            .find(|route| route.model_id == "model-two")
            .expect("new stable route for updated model");
        assert_ne!(updated_route.id.as_str(), "dual");
        let updated_preset_id = GenerationPresetId::from(updated_route.id.as_str());
        assert_eq!(
            storage
                .get_generation_preset(&updated_preset_id)
                .expect("new model preset")
                .model_route_id,
            updated_route.id.clone()
        );

        storage
            .save_settings(&AppSettings {
                local_user_id: LocalUserId::default(),
                preserve_partial_generations: true,
                selected_provider_profile_id: Some("dual".to_owned()),
                selected_model_route_id: None,
                selected_generation_preset_id: None,
            })
            .expect("dual-write selection");
        let settings = storage.load_settings().expect("settings");
        assert_eq!(
            settings
                .selected_model_route_id
                .as_ref()
                .map(ModelRouteId::as_str),
            Some(updated_route.id.as_str())
        );
        assert_eq!(
            settings
                .selected_generation_preset_id
                .as_ref()
                .map(GenerationPresetId::as_str),
            Some(updated_preset_id.as_str())
        );

        drop(storage);
        let reopened = Storage::open(root.path()).expect("reopen stable legacy routes");
        assert_eq!(
            reopened
                .get_model_route(&ModelRouteId::from("dual"))
                .expect("original route after reopen")
                .model_id,
            "model-one"
        );
        assert_eq!(
            reopened
                .get_model_route(&updated_route.id)
                .expect("updated route after reopen")
                .model_id,
            "model-two"
        );
        reopened
            .save_provider_profile(&updated)
            .expect("idempotently reuse updated route");
        assert_eq!(
            reopened
                .list_model_routes(&ProviderConnectionId::from("dual"))
                .expect("routes after idempotent update")
                .len(),
            2
        );
    }

    #[test]
    fn discovery_compensation_restores_a_retained_legacy_profile_on_its_sibling_route() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let original = ProviderProfile {
            id: "legacy-discovery-compensation".to_owned(),
            display_name: "Legacy discovery compensation".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "model-one".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&original)
            .expect("save original legacy profile");
        storage
            .save_provider_profile(&ProviderProfile {
                model: "model-two".to_owned(),
                ..original.clone()
            })
            .expect("move legacy profile to a sibling route");
        storage
            .save_settings(&AppSettings {
                selected_provider_profile_id: Some(original.id.clone()),
                ..AppSettings::default()
            })
            .expect("select retained legacy profile");

        let selected = storage
            .load_settings()
            .expect("normalized legacy selection");
        let selected_route = selected
            .selected_model_route_id
            .clone()
            .expect("selected sibling route");
        let selected_preset = selected
            .selected_generation_preset_id
            .clone()
            .expect("selected sibling preset");
        assert_ne!(selected_route.as_str(), original.id);
        let previous = storage
            .current_discovery_previous_selection()
            .expect("capture discovery previous selection");
        assert_eq!(
            previous,
            DiscoveryPreviousSelection::RouteAndPreset {
                selected_provider_profile_id: Some(original.id.clone()),
                model_route_id: selected_route.clone(),
                generation_preset_id: selected_preset.clone(),
            }
        );

        let expected_selection_revision = {
            let mut connection = storage.connection().expect("graph-clear connection");
            let transaction = connection.transaction().expect("graph-clear transaction");
            let revision = clear_provider_selections_for_discovery_compensation(
                &transaction,
                original.id.as_str(),
            )
            .expect("clear graph-owned legacy selection")
            .expect("legacy graph clear changed the selected target");
            transaction.commit().expect("commit graph-owned clear");
            revision
        };
        {
            let mut connection = storage.connection().expect("compensation connection");
            let transaction = connection.transaction().expect("compensation transaction");
            restore_discovery_provider_selection(
                &transaction,
                &previous,
                Some(expected_selection_revision),
            )
            .expect("restore exact legacy selection");
            transaction.commit().expect("commit compensation");
        }

        let restored = storage.load_settings().expect("restored settings");
        assert_eq!(
            restored.selected_provider_profile_id.as_deref(),
            Some(original.id.as_str())
        );
        assert_eq!(restored.selected_model_route_id, Some(selected_route));
        assert_eq!(
            restored.selected_generation_preset_id,
            Some(selected_preset)
        );
    }

    #[test]
    fn discovery_graph_clear_cannot_overwrite_a_later_explicit_clear() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let previous_profile = ProviderProfile {
            id: "selection-before-discovery".to_owned(),
            display_name: "Selection before discovery".to_owned(),
            base_url: "https://previous.example.test/v1".to_owned(),
            model: "previous-model".to_owned(),
            timeout_seconds: 30,
        };
        let discovered_profile = ProviderProfile {
            id: "selection-owned-by-discovery".to_owned(),
            display_name: "Selection owned by discovery".to_owned(),
            base_url: "https://discovered.example.test/v1".to_owned(),
            model: "discovered-model".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&previous_profile)
            .expect("save previous provider graph");
        storage
            .save_provider_profile(&discovered_profile)
            .expect("save discovered provider graph");
        storage
            .save_settings(&AppSettings {
                selected_provider_profile_id: Some(previous_profile.id.clone()),
                ..AppSettings::default()
            })
            .expect("select provider that predates discovery");
        let previous_selection = storage
            .current_discovery_previous_selection()
            .expect("capture pre-discovery selection");
        storage
            .save_settings(&AppSettings {
                selected_provider_profile_id: Some(discovered_profile.id.clone()),
                ..AppSettings::default()
            })
            .expect("select graph later owned by discovery");

        let graph_clear_committed = Arc::new(Barrier::new(2));
        let explicit_clear_committed = Arc::new(Barrier::new(2));
        let discovered_profile_id = discovered_profile.id.clone();
        let storage_ref = &storage;
        thread::scope(|scope| {
            let graph_thread_clear = Arc::clone(&graph_clear_committed);
            let graph_thread_explicit = Arc::clone(&explicit_clear_committed);
            let previous_selection = previous_selection.clone();
            scope.spawn(move || {
                let expected_selection_revision = {
                    let mut connection = storage_ref.connection().expect("graph-clear connection");
                    let transaction = connection.transaction().expect("graph-clear transaction");
                    let revision = clear_provider_selections_for_discovery_compensation(
                        &transaction,
                        discovered_profile_id.as_str(),
                    )
                    .expect("clear selection owned by removed discovery graph")
                    .expect("discovery graph clear changed the selected target");
                    transaction.commit().expect("commit graph-owned clear");
                    revision
                };
                graph_thread_clear.wait();
                graph_thread_explicit.wait();
                let mut connection = storage_ref
                    .connection()
                    .expect("selection-restore connection");
                let transaction = connection
                    .transaction()
                    .expect("selection-restore transaction");
                restore_discovery_provider_selection(
                    &transaction,
                    &previous_selection,
                    Some(expected_selection_revision),
                )
                .expect("finish discovery selection compensation");
                transaction.commit().expect("commit selection restoration");
            });

            let user_thread_clear = Arc::clone(&graph_clear_committed);
            let user_thread_explicit = Arc::clone(&explicit_clear_committed);
            scope.spawn(move || {
                user_thread_clear.wait();
                storage_ref
                    .save_settings(&AppSettings::default())
                    .expect("persist explicit clear after graph removal");
                user_thread_explicit.wait();
            });
        });

        let final_settings = storage.load_settings().expect("load final settings");
        assert!(
            final_settings.selected_provider_profile_id.is_none()
                && final_settings.selected_model_route_id.is_none()
                && final_settings.selected_generation_preset_id.is_none(),
            "the latest explicit clear must win over stale discovery compensation"
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the four serialized delete/profile-switch orders keep their graph assertions adjacent"
    )]
    fn migrated_legacy_graph_crud_is_atomic_across_profile_switch_orders() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let seed_sibling = |id: &str| {
            let model_a = ProviderProfile {
                id: id.to_owned(),
                display_name: format!("{id} A"),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "model-a".to_owned(),
                timeout_seconds: 30,
            };
            let model_b = ProviderProfile {
                display_name: format!("{id} B"),
                model: "model-b".to_owned(),
                ..model_a.clone()
            };
            storage
                .save_provider_profile(&model_a)
                .expect("seed legacy model A");
            storage
                .save_provider_profile(&model_b)
                .expect("create legacy model B sibling");
            let route_b = storage
                .list_model_routes(&ProviderConnectionId::from(id))
                .expect("legacy sibling routes")
                .into_iter()
                .find(|route| route.model_id == "model-b")
                .expect("legacy model B sibling");
            let preset_b = GenerationPresetId::from(route_b.id.as_str());
            storage
                .save_provider_profile(&model_a)
                .expect("make model A current again");
            (model_b, route_b, preset_b)
        };

        let (route_delete_first_profile, route_delete_first, route_delete_first_preset) =
            seed_sibling("legacy-route-delete-first");
        storage
            .delete_model_route(&route_delete_first.id)
            .expect("an old sibling route may be deleted before the profile switches to it");
        assert_eq!(
            storage
                .get_model_route(&route_delete_first.id)
                .expect_err("old sibling route was deleted")
                .code,
            CoreErrorCode::NotFound
        );
        storage
            .save_provider_profile(&route_delete_first_profile)
            .expect("profile switch recreates its missing current route atomically");
        storage
            .get_generation_preset(&route_delete_first_preset)
            .expect("profile switch recreates its default preset");

        let (route_switch_first_profile, route_switch_first, route_switch_first_preset) =
            seed_sibling("legacy-route-switch-first");
        storage
            .save_provider_profile(&route_switch_first_profile)
            .expect("switch profile before route delete");
        let route_error = storage
            .delete_model_route(&route_switch_first.id)
            .expect_err("the transaction revalidates the newly-current sibling route");
        assert_eq!(route_error.code, CoreErrorCode::InvalidInput);
        storage
            .get_model_route(&route_switch_first.id)
            .expect("current sibling route remains");
        storage
            .get_generation_preset(&route_switch_first_preset)
            .expect("current sibling preset remains");

        let (preset_delete_first_profile, preset_delete_first_route, preset_delete_first) =
            seed_sibling("legacy-preset-delete-first");
        storage
            .delete_generation_preset(&preset_delete_first)
            .expect("an old sibling preset may be deleted before the profile switches to it");
        storage
            .save_provider_profile(&preset_delete_first_profile)
            .expect("profile switch recreates its missing default preset atomically");
        storage
            .get_generation_preset(&preset_delete_first)
            .expect("current default preset was recreated");
        storage
            .get_model_route(&preset_delete_first_route.id)
            .expect("preset recreation preserves its route");

        let (preset_switch_first_profile, preset_switch_first_route, preset_switch_first) =
            seed_sibling("legacy-preset-switch-first");
        storage
            .save_provider_profile(&preset_switch_first_profile)
            .expect("switch profile before preset delete");
        let preset_error = storage
            .delete_generation_preset(&preset_switch_first)
            .expect_err("the transaction revalidates the newly-current sibling preset");
        assert_eq!(preset_error.code, CoreErrorCode::InvalidInput);
        storage
            .get_generation_preset(&preset_switch_first)
            .expect("current sibling preset remains");

        let mut route_update = preset_switch_first_route.clone();
        route_update.display_name = Some("ordinary mutation".to_owned());
        assert_eq!(
            storage
                .save_model_route(&route_update)
                .expect_err("ordinary route save revalidates the retained profile transaction")
                .code,
            CoreErrorCode::InvalidInput
        );
        let mut extra_preset = storage
            .get_generation_preset(&preset_switch_first)
            .expect("current preset for ordinary save rejection");
        extra_preset.id = GenerationPresetId::from("ordinary-legacy-alias-preset");
        assert_eq!(
            storage
                .save_generation_preset(&extra_preset)
                .expect_err("ordinary preset save revalidates the retained profile transaction")
                .code,
            CoreErrorCode::InvalidInput
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn provider_catalog_crud_roundtrips_and_rejects_secret_or_dangling_data() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let (mut connection, _, _) = save_modern_direct_provider_graph(
            &storage,
            &ProviderProfile {
                id: "catalog".to_owned(),
                display_name: "Catalog".to_owned(),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "default-model".to_owned(),
                timeout_seconds: 30,
            },
        );
        let mut duplicate_connection = connection.clone();
        duplicate_connection.display_name = "Duplicate overwrite".to_owned();
        let duplicate_error = storage
            .insert_provider_connection(&duplicate_connection)
            .expect_err("create must not overwrite an occupied connection ID");
        assert_eq!(duplicate_error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            storage
                .get_provider_connection(&ProviderConnectionId::from("catalog"))
                .expect("connection after rejected duplicate"),
            connection
        );
        let mut retargeted_connection = connection.clone();
        retargeted_connection.config.api_base_path =
            Some(EndpointPath::parse("/v2").expect("retargeted base path"));
        retargeted_connection.config.values = vec![ConnectionConfigEntry {
            key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
            value: ConnectionConfigValue::Text("https://api.example.test/v2".to_owned()),
        }];
        let retarget_error = storage
            .save_provider_connection(&retargeted_connection)
            .expect_err("stable connection ID must not change endpoint config");
        assert_eq!(retarget_error.code, CoreErrorCode::InvalidInput);

        let mut rebound_connection = connection.clone();
        rebound_connection.credential_ref = Some(CredentialRef("other-vault-entry".to_owned()));
        let rebound_error = storage
            .save_provider_connection(&rebound_connection)
            .expect_err("stable connection ID must not change credential binding");
        assert_eq!(rebound_error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            storage
                .get_provider_connection(&ProviderConnectionId::from("catalog"))
                .expect("connection after rejected identity mutations"),
            connection
        );

        connection.status = ConnectionStatus::Connected;
        connection.updated_at = Utc::now();
        storage
            .save_provider_connection(&connection)
            .expect("update connection");
        assert_eq!(
            storage
                .get_provider_connection(&ProviderConnectionId::from("catalog"))
                .expect("roundtrip connection"),
            connection
        );
        let mut unsafe_connection = connection.clone();
        unsafe_connection.config.values.push(ConnectionConfigEntry {
            key: "api_key".to_owned(),
            value: ConnectionConfigValue::Text("must-not-be-persisted".to_owned()),
        });
        let error = storage
            .save_provider_connection(&unsafe_connection)
            .expect_err("secret-like config must be rejected");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(
            storage
                .connection()
                .expect("connection")
                .query_row(
                    "SELECT CAST(config_json AS TEXT) NOT LIKE '%must-not-be-persisted%'
                     FROM provider_connections WHERE id = 'catalog'",
                    [],
                    |row| row.get::<_, bool>(0),
                )
                .expect("secret absence")
        );

        let now = Utc::now();
        let route = ModelRoute {
            id: ModelRouteId::from("extra-route"),
            connection_id: ProviderConnectionId::from("catalog"),
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "extra-model".to_owned(),
            display_name: Some("Extra model".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: now,
            last_seen_at: Some(now),
        };
        storage.save_model_route(&route).expect("save route");
        assert_eq!(
            storage
                .get_model_route(&ModelRouteId::from("extra-route"))
                .expect("roundtrip route"),
            route
        );
        let preset = GenerationPreset {
            id: GenerationPresetId::from("extra-preset"),
            model_route_id: route.id.clone(),
            display_name: "Creative".to_owned(),
            values: vec![ParameterValue {
                parameter_id: ParameterId::from(TEMPERATURE_PARAMETER_ID),
                state: ParameterValueState::Explicit(ParameterLiteral::Number(1.5)),
            }],
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: now,
            updated_at: now,
        };
        storage
            .save_generation_preset(&preset)
            .expect("save preset");
        assert_eq!(
            storage
                .get_generation_preset(&GenerationPresetId::from("extra-preset"))
                .expect("roundtrip preset"),
            preset
        );
        assert_eq!(
            storage
                .list_generation_presets(&ModelRouteId::from("extra-route"))
                .expect("listed presets"),
            vec![preset.clone()]
        );
        storage
            .save_settings(&AppSettings {
                local_user_id: LocalUserId::default(),
                preserve_partial_generations: true,
                selected_provider_profile_id: None,
                selected_model_route_id: Some(route.id.clone()),
                selected_generation_preset_id: Some(preset.id.clone()),
            })
            .expect("select route and preset");
        storage
            .delete_generation_preset(&preset.id)
            .expect("delete preset");
        let settings = storage.load_settings().expect("settings after delete");
        assert!(settings.selected_model_route_id.is_none());
        assert!(settings.selected_generation_preset_id.is_none());
        storage.delete_model_route(&route.id).expect("delete route");

        let dangling = ModelRoute {
            id: ModelRouteId::from("dangling"),
            connection_id: ProviderConnectionId::from("missing"),
            ..route
        };
        let error = storage
            .save_model_route(&dangling)
            .expect_err("dangling connection must be rejected");
        assert_eq!(error.code, CoreErrorCode::NotFound);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn model_route_reconciliation_preserves_missing_rows_presets_and_rolls_back_atomically() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let (_, default_route, _) = save_modern_direct_provider_graph(
            &storage,
            &ProviderProfile {
                id: "sync".to_owned(),
                display_name: "Sync".to_owned(),
                base_url: "https://api.example.test/v1".to_owned(),
                model: "default-model".to_owned(),
                timeout_seconds: 30,
            },
        );
        let connection_id = ProviderConnectionId::from("sync");
        let first_seen_at = Utc::now() - chrono::Duration::hours(1);
        let old_route = ModelRoute {
            id: ModelRouteId::from("old-route"),
            connection_id: connection_id.clone(),
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "old-model".to_owned(),
            display_name: Some("Old model".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at,
            last_seen_at: Some(first_seen_at),
        };
        storage
            .save_model_route(&old_route)
            .expect("save old route");
        let old_preset = GenerationPreset {
            id: GenerationPresetId::from("old-preset"),
            model_route_id: old_route.id.clone(),
            display_name: "Old preset".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: first_seen_at,
            updated_at: first_seen_at,
        };
        storage
            .save_generation_preset(&old_preset)
            .expect("save old preset");

        let observed_at = Utc::now();
        let new_route = ModelRoute {
            id: ModelRouteId::from("new-route"),
            connection_id: connection_id.clone(),
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "new-model".to_owned(),
            display_name: Some("New model".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Unknown,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: first_seen_at - chrono::Duration::days(1),
            last_seen_at: None,
        };
        storage
            .reconcile_model_routes(
                &connection_id,
                &[default_route, new_route.clone()],
                observed_at,
            )
            .expect("reconcile models");
        let reconciled_default = storage
            .get_model_route(&ModelRouteId::from("sync"))
            .expect("reconciled default");
        assert_eq!(reconciled_default.status, ModelAvailability::Available);
        assert_eq!(reconciled_default.last_seen_at, Some(observed_at));
        let missing = storage
            .get_model_route(&old_route.id)
            .expect("missing route retained");
        assert_eq!(missing.status, ModelAvailability::MissingTemporarily);
        assert_eq!(missing.first_seen_at, first_seen_at);
        assert_eq!(
            storage
                .get_generation_preset(&old_preset.id)
                .expect("preset retained"),
            old_preset
        );
        let inserted = storage.get_model_route(&new_route.id).expect("new route");
        assert_eq!(inserted.status, ModelAvailability::Available);
        assert_eq!(inserted.first_seen_at, observed_at);
        assert_eq!(inserted.last_seen_at, Some(observed_at));

        let before_rollback = storage
            .list_model_routes(&connection_id)
            .expect("routes before rollback");
        storage
            .connection()
            .expect("connection")
            .execute_batch(
                "CREATE TEMP TRIGGER reject_missing_route_update
                 BEFORE UPDATE ON provider_models
                 WHEN OLD.id = 'old-route'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic reconciliation failure');
                 END;",
            )
            .expect("rollback trigger");
        let next_observation = observed_at + chrono::Duration::minutes(1);
        let listed = before_rollback
            .iter()
            .filter(|route| route.id.as_str() != "old-route")
            .cloned()
            .collect::<Vec<_>>();
        let error = storage
            .reconcile_model_routes(&connection_id, &listed, next_observation)
            .expect_err("reconciliation must roll back");
        assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
        assert_eq!(
            storage
                .list_model_routes(&connection_id)
                .expect("routes after rollback"),
            before_rollback
        );
        assert_eq!(
            storage
                .get_generation_preset(&old_preset.id)
                .expect("preset after rollback"),
            old_preset
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn provider_profile_archive_and_selection_clear_are_atomic() {
        let root = tempdir().expect("temp root");
        let storage = Storage::open(root.path()).expect("open storage");
        let profile = ProviderProfile {
            id: "selected".to_owned(),
            display_name: "Selected".to_owned(),
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&profile)
            .expect("save provider");
        storage
            .save_settings(&AppSettings {
                preserve_partial_generations: true,
                selected_provider_profile_id: Some(profile.id.clone()),
                ..AppSettings::default()
            })
            .expect("select provider");
        storage
            .connection()
            .expect("connection")
            .execute_batch(
                "CREATE TEMP TRIGGER reject_provider_archive
                 BEFORE UPDATE OF archived_at ON provider_connections
                 WHEN OLD.id = 'selected'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic provider archive failure');
                 END;",
            )
            .expect("install synthetic failure");

        let error = storage
            .delete_provider_profile(&profile.id)
            .expect_err("archive trigger must abort");
        assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
        assert_eq!(
            storage
                .load_settings()
                .expect("settings after rollback")
                .selected_provider_profile_id
                .as_deref(),
            Some(profile.id.as_str())
        );
        assert_eq!(
            storage
                .get_provider_profile(&profile.id)
                .expect("provider after rollback"),
            profile
        );
        assert!(
            storage
                .get_provider_connection(&ProviderConnectionId::from(profile.id.as_str()))
                .is_ok()
        );
        assert!(
            storage
                .get_generation_preset(&GenerationPresetId::from(profile.id.as_str()))
                .is_ok()
        );

        storage
            .connection()
            .expect("connection")
            .execute_batch("DROP TRIGGER reject_provider_archive;")
            .expect("remove synthetic failure");
        storage
            .delete_provider_profile(&profile.id)
            .expect("delete provider");
        assert!(
            storage
                .list_provider_profiles()
                .expect("providers")
                .is_empty()
        );
        assert_eq!(
            storage
                .load_settings()
                .expect("settings after delete")
                .selected_provider_profile_id,
            None
        );
        assert_eq!(
            storage
                .get_provider_connection(&ProviderConnectionId::from(profile.id.as_str()))
                .expect_err("archived connection must be hidden")
                .code,
            CoreErrorCode::NotFound
        );
        assert!(
            storage
                .get_model_route(&ModelRouteId::from(profile.id.as_str()))
                .is_ok(),
            "archiving must preserve route provenance"
        );
        assert!(
            storage
                .get_generation_preset(&GenerationPresetId::from(profile.id.as_str()))
                .is_ok(),
            "archiving must preserve preset provenance"
        );
        let (archived_at, profile_rows) = storage
            .connection()
            .expect("connection")
            .query_row(
                "SELECT connection.archived_at,
                        (SELECT COUNT(*) FROM provider_profiles WHERE id = ?1)
                 FROM provider_connections AS connection
                 WHERE connection.id = ?1",
                [profile.id.as_str()],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, u32>(1)?)),
            )
            .expect("tombstoned provider rows");
        assert!(archived_at.is_some());
        assert_eq!(profile_rows, 1);
        let reuse = storage
            .save_provider_profile(&profile)
            .expect_err("archived provider id must not be reused");
        assert_eq!(reuse.code, CoreErrorCode::InvalidInput);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn archived_connection_preserves_generation_and_discovery_history_after_reopen() {
        let (root, storage, conversation, branch_id) = imported_storage();
        let profile = ProviderProfile {
            id: "archived-history".to_owned(),
            display_name: "Archived history".to_owned(),
            base_url: "https://history.example.test/v1".to_owned(),
            model: "historical-model".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&profile)
            .expect("save historical provider");
        let connection_id = ProviderConnectionId::from(profile.id.as_str());
        let historical_connection = storage
            .get_provider_connection(&connection_id)
            .expect("historical connection");
        let route = storage
            .get_model_route(&ModelRouteId::from(profile.id.as_str()))
            .expect("historical route");
        let preset = storage
            .get_generation_preset(&GenerationPresetId::from(profile.id.as_str()))
            .expect("historical preset");

        let user = Message::user(conversation.id.clone(), "historical request");
        let generation_id = GenerationId::new();
        let pending = Message::pending_assistant(
            conversation.id.clone(),
            user.id.clone(),
            generation_id.clone(),
        );
        let generation = GenerationRecord {
            id: generation_id.clone(),
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            user_message_id: user.id.clone(),
            assistant_message_id: Some(pending.id.clone()),
            mode: ConversationMode::Chat,
            model: route.model_id.clone(),
            model_route_id: Some(route.id.clone()),
            generation_preset_id: Some(preset.id.clone()),
            provider_family: Some(ApiFamily::OpenAiChatCompletions),
            status: GenerationStatus::Running,
            input_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
            opaque_reasoning_state: Vec::new(),
            error_code: None,
            started_at: pending.created_at,
            finished_at: None,
        };
        storage
            .append_generation(&branch_id, None, &user, &pending, &generation)
            .expect("append provider-target generation");
        let mut assistant = pending;
        assistant.content = "historical response".to_owned();
        assistant.status = MessageStatus::Complete;
        storage
            .finalize_generation(&assistant, None, None, true)
            .expect("finalize provider-target generation");

        let discovery_session_id = "archived-provider-discovery";
        let now = Utc::now().to_rfc3339();
        let sanitized_input = serde_json::json!({
            "connection_id": profile.id.clone(),
            "display_name": "Archived history",
        })
        .to_string();
        {
            let mut connection = storage.connection().expect("connection");
            let transaction = connection
                .transaction()
                .expect("historical discovery fixture transaction");
            let initial_state_guard_sql = transaction
                .query_row(
                    "SELECT sql FROM sqlite_schema
                     WHERE type = 'trigger'
                       AND name = 'provider_discovery_session_initial_state_guard'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .expect("load discovery initial-state guard");
            transaction
                .execute_batch("DROP TRIGGER provider_discovery_session_initial_state_guard;")
                .expect("suspend initial-state guard for synthetic historical fixture");
            transaction
                .execute(
                    "INSERT INTO provider_discovery_sessions
                     (id, state, sanitized_input_json, committed_connection_id,
                      created_at, updated_at)
                     VALUES (?1, 'ready', ?2, ?3, ?4, ?4)",
                    params![
                        discovery_session_id,
                        sanitized_input,
                        profile.id.as_str(),
                        now
                    ],
                )
                .expect("seed discovery session reference");
            transaction
                .execute_batch(&initial_state_guard_sql)
                .expect("restore discovery initial-state guard");
            transaction
                .execute(
                    "INSERT INTO provider_discovery_audit_log
                     (session_id, audit_sequence, session_revision, audit_kind,
                      action_id, subject_id, summary_key, created_at)
                     VALUES (
                       ?1, 1, 0, 'session_created', NULL, ?2,
                       'discovery.audit.session_created', ?3
                     )",
                    params![discovery_session_id, profile.id.as_str(), now],
                )
                .expect("seed discovery audit reference");
            transaction
                .commit()
                .expect("commit synthetic historical discovery fixture");
        }
        storage
            .save_settings(&AppSettings {
                selected_provider_profile_id: Some(profile.id.clone()),
                ..AppSettings::default()
            })
            .expect("select provider before archive");

        let archive = storage
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::RemoveForArchive,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("prepare credential-aware provider archive");
        storage
            .finish_provider_credential_archive(
                &archive.plan.operation_id,
                &archive.plan_sha256,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("archive provider connection");
        assert!(
            storage
                .list_provider_connections()
                .expect("active list")
                .is_empty()
        );
        assert_eq!(
            storage
                .get_provider_connection(&connection_id)
                .expect_err("archived connection is inactive")
                .code,
            CoreErrorCode::NotFound
        );
        assert_eq!(
            storage
                .create_model_sync_job(&historical_connection)
                .expect_err("archived connection cannot start model sync")
                .code,
            CoreErrorCode::NotFound
        );
        assert_eq!(
            storage
                .load_settings()
                .expect("settings after archive")
                .selected_model_route_id,
            None
        );
        let stored_generation = storage.get_generation(&generation_id).expect("generation");
        assert_eq!(stored_generation.status, GenerationStatus::Complete);
        assert_eq!(stored_generation.model_route_id, Some(route.id.clone()));
        assert_eq!(
            stored_generation.generation_preset_id,
            Some(preset.id.clone())
        );
        assert_eq!(
            storage
                .list_branch_messages(&branch_id)
                .expect("conversation history")
                .len(),
            2
        );
        drop(storage);

        let reopened = Storage::open(root.path()).expect("reopen archived history");
        assert_eq!(
            reopened
                .schema_version()
                .expect("read durable schema version"),
            SCHEMA_VERSION
        );
        assert!(
            reopened
                .list_provider_connections()
                .expect("reopened active list")
                .is_empty()
        );
        assert_eq!(
            reopened
                .get_provider_connection(&connection_id)
                .expect_err("reopened archived connection is inactive")
                .code,
            CoreErrorCode::NotFound
        );
        assert!(reopened.get_model_route(&route.id).is_ok());
        assert!(reopened.get_generation_preset(&preset.id).is_ok());
        assert!(reopened.get_generation(&generation_id).is_ok());
        assert_eq!(
            reopened
                .list_branch_messages(&branch_id)
                .expect("reopened conversation history")
                .len(),
            2
        );
        let (archived_rows, discovery_rows, audit_rows) = reopened
            .connection()
            .expect("connection")
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM provider_connections
                    WHERE id = ?1 AND archived_at IS NOT NULL),
                   (SELECT COUNT(*) FROM provider_discovery_sessions
                    WHERE id = ?2 AND committed_connection_id = ?1),
                   (SELECT COUNT(*) FROM provider_discovery_audit_log
                    WHERE session_id = ?2)",
                params![connection_id.as_str(), discovery_session_id],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, u32>(2)?,
                    ))
                },
            )
            .expect("historical row counts");
        assert_eq!((archived_rows, discovery_rows, audit_rows), (1, 1, 1));
    }

    #[test]
    fn credential_archive_prepare_blocks_provider_generation_append_without_rows() {
        let (_root, storage, conversation, branch_id) = imported_storage();
        let profile = ProviderProfile {
            id: "archive-before-generation".to_owned(),
            display_name: "Archive before generation".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "blocked-model".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&profile)
            .expect("save provider profile");
        let connection_id = ProviderConnectionId::from(profile.id.as_str());
        let archive = storage
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::RemoveForArchive,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("prepare credential archive");
        assert_eq!(
            archive.status,
            crate::ProviderCredentialOperationStatus::Prepared
        );
        let (user, pending, generation) = provider_generation_record(
            &conversation,
            &branch_id,
            ModelRouteId::from(profile.id.as_str()),
            GenerationPresetId::from(profile.id.as_str()),
            &profile.model,
            "must remain transient",
        );
        let before_message_count = storage
            .list_messages(&conversation.id)
            .expect("messages before rejected append")
            .len();
        let before_branch_head = storage
            .get_conversation_branch(&branch_id)
            .expect("branch before rejected append")
            .head_message_id;

        let error = storage
            .append_generation(&branch_id, None, &user, &pending, &generation)
            .expect_err("credential archive must block provider generation append");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert_eq!(
            storage
                .list_messages(&conversation.id)
                .expect("messages after rejected append")
                .len(),
            before_message_count
        );
        assert_eq!(
            storage
                .connection()
                .expect("generation count connection")
                .query_row(
                    "SELECT COUNT(*) FROM generations WHERE id = ?1",
                    [generation.id.0.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("generation row count"),
            0
        );
        assert_eq!(
            storage
                .connection()
                .expect("message identity count connection")
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE id = ?1 OR id = ?2",
                    params![user.id.0.as_str(), pending.id.0.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("user and assistant row count"),
            0
        );
        assert_eq!(
            storage
                .get_conversation_branch(&branch_id)
                .expect("branch after rejected append")
                .head_message_id,
            before_branch_head
        );
        assert_eq!(
            storage
                .get_conversation_state(&conversation.id)
                .expect("conversation state after rejected append")
                .active_branch_id,
            branch_id
        );
        assert_eq!(
            storage
                .get_provider_credential_operation(&archive.plan.operation_id)
                .expect("archive after rejected append")
                .status,
            crate::ProviderCredentialOperationStatus::Prepared
        );
    }

    #[test]
    fn running_provider_generation_blocks_credential_archive() {
        let (_root, storage, conversation, branch_id) = imported_storage();
        let profile = ProviderProfile {
            id: "generation-before-archive".to_owned(),
            display_name: "Generation before archive".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "running-model".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&profile)
            .expect("save provider profile");
        let connection_id = ProviderConnectionId::from(profile.id.as_str());
        let (user, pending, generation) = provider_generation_record(
            &conversation,
            &branch_id,
            ModelRouteId::from(profile.id.as_str()),
            GenerationPresetId::from(profile.id.as_str()),
            &profile.model,
            "remain running",
        );
        storage
            .append_generation(&branch_id, None, &user, &pending, &generation)
            .expect("append running provider generation");

        let error = storage
            .prepare_provider_credential_operation(
                &connection_id,
                ProviderCredentialOperationKind::RemoveForArchive,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect_err("running provider generation must block credential archive");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(
            storage
                .list_unresolved_provider_credential_operations()
                .expect("list credential operations")
                .is_empty()
        );
        assert_eq!(
            storage
                .get_generation(&generation.id)
                .expect("running generation remains durable")
                .status,
            GenerationStatus::Running
        );
    }

    #[test]
    fn concurrent_provider_selection_and_delete_cannot_leave_dangling_settings() {
        let root = tempdir().expect("temp root");
        let storage = Arc::new(Storage::open(root.path()).expect("open storage"));

        for index in 0..32 {
            let profile = ProviderProfile {
                id: format!("provider-{index}"),
                display_name: format!("Provider {index}"),
                base_url: "http://127.0.0.1:11434/v1".to_owned(),
                model: "synthetic".to_owned(),
                timeout_seconds: 30,
            };
            storage
                .save_provider_profile(&profile)
                .expect("save provider");
            storage
                .save_settings(&AppSettings {
                    preserve_partial_generations: true,
                    selected_provider_profile_id: None,
                    ..AppSettings::default()
                })
                .expect("reset settings");

            let barrier = Arc::new(Barrier::new(3));
            let selecting_storage = Arc::clone(&storage);
            let selecting_barrier = Arc::clone(&barrier);
            let selected_id = profile.id.clone();
            let selection = thread::spawn(move || {
                selecting_barrier.wait();
                selecting_storage.save_settings(&AppSettings {
                    preserve_partial_generations: true,
                    selected_provider_profile_id: Some(selected_id),
                    ..AppSettings::default()
                })
            });
            let deleting_storage = Arc::clone(&storage);
            let deleting_barrier = Arc::clone(&barrier);
            let deleted_id = profile.id.clone();
            let deletion = thread::spawn(move || {
                deleting_barrier.wait();
                deleting_storage.delete_provider_profile(&deleted_id)
            });
            barrier.wait();

            let selection = selection.join().expect("selection thread");
            deletion
                .join()
                .expect("deletion thread")
                .expect("delete provider");
            if let Err(error) = selection {
                assert_eq!(error.code, CoreErrorCode::NotFound);
            }
            assert_eq!(
                storage
                    .get_provider_profile(&profile.id)
                    .expect_err("provider must be deleted")
                    .code,
                CoreErrorCode::NotFound
            );
            assert_eq!(
                storage
                    .load_settings()
                    .expect("settings after concurrent operations")
                    .selected_provider_profile_id,
                None
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn message_actions_preserve_rows_and_guard_branch_snapshots() {
        let (_root, storage, conversation, source_branch_id) = imported_storage();
        let (original_user, original_assistant) = append_complete_generation(
            &storage,
            &conversation.id,
            &source_branch_id,
            None,
            "original",
            "original response",
        );
        let context = storage
            .prepare_message_generation_action(
                &conversation.id,
                &source_branch_id,
                Some(&original_assistant.id),
                &original_user.id,
                MessageGenerationAction::EditUser,
            )
            .expect("prepare edit");
        assert!(context.fork_message_id.is_none());
        assert_eq!(context.user_text, "original");

        let edited_user = Message::user(conversation.id.clone(), "edited");
        let action_generation_id = GenerationId::new();
        let pending = Message::pending_assistant(
            conversation.id.clone(),
            edited_user.id.clone(),
            action_generation_id.clone(),
        );
        let now = Utc::now();
        let action_branch = ConversationBranch {
            id: ConversationBranchId::new(),
            conversation_id: conversation.id.clone(),
            title: None,
            fork_message_id: None,
            head_message_id: Some(pending.id.clone()),
            created_at: now,
            updated_at: now,
        };
        let generation = GenerationRecord {
            id: action_generation_id,
            conversation_id: conversation.id.clone(),
            branch_id: action_branch.id.clone(),
            user_message_id: edited_user.id.clone(),
            assistant_message_id: Some(pending.id.clone()),
            mode: ConversationMode::Chat,
            model: "synthetic".to_owned(),
            model_route_id: None,
            generation_preset_id: None,
            provider_family: None,
            status: GenerationStatus::Running,
            input_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
            opaque_reasoning_state: Vec::new(),
            error_code: None,
            started_at: pending.created_at,
            finished_at: None,
        };
        storage
            .append_message_generation_action(
                &source_branch_id,
                Some(&original_assistant.id),
                &original_user.id,
                MessageGenerationAction::EditUser,
                &action_branch,
                &edited_user,
                &pending,
                &generation,
            )
            .expect("append edit branch");
        assert_eq!(
            storage
                .get_conversation_state(&conversation.id)
                .expect("state")
                .active_branch_id,
            action_branch.id
        );
        assert_eq!(
            storage
                .list_branch_messages(&source_branch_id)
                .expect("source lineage")
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["original", "original response"]
        );

        let pending_error = storage
            .remove_message_from_branch(
                &conversation.id,
                &action_branch.id,
                Some(&pending.id),
                &pending.id,
            )
            .expect_err("pending branch must reject removal");
        assert_eq!(pending_error.code, CoreErrorCode::InvalidInput);
        assert!(pending_error.recoverable);

        let mut terminal = pending.clone();
        terminal.content = "edited response".to_owned();
        terminal.status = MessageStatus::Complete;
        storage
            .finalize_generation(&terminal, None, None, true)
            .expect("finalize edited response");
        let message_count = storage
            .list_messages(&conversation.id)
            .expect("all rows")
            .len();
        let rewound = storage
            .remove_message_from_branch(
                &conversation.id,
                &action_branch.id,
                Some(&terminal.id),
                &terminal.id,
            )
            .expect("rewind assistant");
        assert_eq!(rewound.head_message_id, Some(edited_user.id.clone()));
        assert_eq!(
            storage
                .list_branch_messages(&action_branch.id)
                .expect("rewound lineage"),
            vec![edited_user]
        );
        assert_eq!(
            storage
                .list_messages(&conversation.id)
                .expect("preserved rows")
                .len(),
            message_count,
            "logical removal must not delete immutable message rows"
        );

        let stale = storage
            .remove_message_from_branch(
                &conversation.id,
                &action_branch.id,
                Some(&terminal.id),
                &original_user.id,
            )
            .expect_err("stale head");
        assert_eq!(stale.code, CoreErrorCode::InvalidInput);
        assert!(stale.recoverable);
    }

    #[test]
    fn message_action_lineage_validation_is_deep_and_cycle_safe() {
        let (_root, storage, conversation, branch_id) = imported_storage();
        let (first_message_id, last_message_id) = {
            let mut connection = storage.connection().expect("connection");
            let transaction = connection.transaction().expect("transaction");
            let mut parent_id = None;
            let mut first_message_id = None;
            let mut last_message_id = None;
            for index in 0..4_105 {
                let message = Message::user_after(
                    conversation.id.clone(),
                    parent_id.clone(),
                    format!("message {index}"),
                );
                first_message_id.get_or_insert_with(|| message.id.clone());
                parent_id = Some(message.id.clone());
                last_message_id = Some(message.id.clone());
                insert_message(&transaction, &message).expect("insert deep message");
            }
            let last_message_id = last_message_id.expect("last message");
            transaction
                .execute(
                    "UPDATE conversation_branches
                     SET head_message_id = ?2
                     WHERE id = ?1",
                    params![branch_id.0, last_message_id.0],
                )
                .expect("update branch head");
            transaction.commit().expect("commit deep lineage");
            (first_message_id.expect("first message"), last_message_id)
        };

        let context = storage
            .prepare_message_generation_action(
                &conversation.id,
                &branch_id,
                Some(&last_message_id),
                &first_message_id,
                MessageGenerationAction::EditUser,
            )
            .expect("find a visible message beyond the former depth cutoff");
        assert!(context.fork_message_id.is_none());

        storage
            .connection()
            .expect("connection")
            .execute(
                "UPDATE messages
                 SET parent_id = ?2
                 WHERE conversation_id = ?1 AND id = ?3",
                params![conversation.id.0, last_message_id.0, first_message_id.0],
            )
            .expect("create synthetic corrupted cycle");
        let error = storage
            .prepare_message_generation_action(
                &conversation.id,
                &branch_id,
                Some(&last_message_id),
                &MessageId("missing-from-cycle".to_owned()),
                MessageGenerationAction::EditUser,
            )
            .expect_err("cycle-safe lookup must terminate");
        assert_eq!(error.code, CoreErrorCode::NotFound);
    }

    #[test]
    fn persists_character_and_settings_across_reopen() {
        let root = tempdir().expect("temp root");
        let mut staged = NamedTempFile::new_in(root.path()).expect("staging");
        staged.write_all(b"character").expect("source");
        let character = Character::new("Segu", "Guide", hex::encode(Sha256::digest(b"character")));

        {
            let storage = Storage::open(root.path()).expect("open storage");
            storage
                .commit_character_import(
                    staged.path(),
                    &character,
                    9,
                    &Uuid::new_v4().to_string(),
                    &[],
                )
                .expect("commit import");
            let conversation = Conversation::new(&character.id, &character.name);
            storage
                .save_conversation(&conversation)
                .expect("save conversation");
            let user = Message::user(conversation.id.clone(), "Hello");
            storage.save_message(&user).expect("save user");
            let pending = Message::pending_assistant(
                conversation.id.clone(),
                user.id.clone(),
                GenerationId::new(),
            );
            storage.save_message(&pending).expect("save pending");
            storage
                .save_provider_profile(&ProviderProfile {
                    id: "local".to_owned(),
                    display_name: "Local model".to_owned(),
                    base_url: "http://127.0.0.1:11434/v1".to_owned(),
                    model: "test".to_owned(),
                    timeout_seconds: 30,
                })
                .expect("save provider");
            storage
                .save_settings(&AppSettings {
                    preserve_partial_generations: false,
                    selected_provider_profile_id: Some("local".to_owned()),
                    ..AppSettings::default()
                })
                .expect("save settings");
        }

        let reopened = Storage::open(root.path()).expect("reopen storage");
        assert_eq!(reopened.list_characters().expect("list").len(), 1);
        assert!(
            !reopened
                .load_settings()
                .expect("load settings")
                .preserve_partial_generations
        );
        assert_eq!(
            reopened
                .list_provider_profiles()
                .expect("provider profiles")
                .len(),
            1
        );
        assert_eq!(
            reopened
                .list_messages(&reopened.list_conversations().expect("conversations")[0].id)
                .expect("messages")
                .len(),
            1,
            "discard policy removes interrupted assistant messages"
        );
    }

    #[test]
    fn partial_checkpoint_cannot_overwrite_a_terminal_assistant_message() {
        let root = tempdir().expect("temp root");
        let mut staged = NamedTempFile::new_in(root.path()).expect("staging");
        staged.write_all(b"character").expect("source");
        let character = Character::new("Segu", "Guide", hex::encode(Sha256::digest(b"character")));
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .commit_character_import(
                staged.path(),
                &character,
                9,
                &Uuid::new_v4().to_string(),
                &[],
            )
            .expect("commit import");
        let conversation = Conversation::new(&character.id, &character.name);
        storage
            .save_conversation(&conversation)
            .expect("save conversation");
        let user = Message::user(conversation.id.clone(), "Hello");
        storage.save_message(&user).expect("save user");
        let mut pending =
            Message::pending_assistant(conversation.id.clone(), user.id, GenerationId::new());
        storage.save_message(&pending).expect("save pending");

        pending.content = "checkpoint".to_owned();
        storage
            .checkpoint_pending_assistant(&pending)
            .expect("checkpoint pending");
        let mut terminal = pending.clone();
        terminal.content = "final".to_owned();
        terminal.status = MessageStatus::Complete;
        storage.save_message(&terminal).expect("save terminal");

        pending.content = "stale checkpoint".to_owned();
        let error = storage
            .checkpoint_pending_assistant(&pending)
            .expect_err("terminal row must reject a stale checkpoint");
        assert_eq!(error.code, CoreErrorCode::NotFound);
        let messages = storage.list_messages(&conversation.id).expect("messages");
        assert_eq!(messages[1].content, "final");
        assert_eq!(messages[1].status, MessageStatus::Complete);
    }

    #[test]
    fn restart_recovers_import_journal_cas_files_and_staging() {
        let root = tempdir().expect("temp root");
        let source_bytes = b"orphan source";
        let asset_bytes = b"orphan asset";
        let source_hash = hex::encode(Sha256::digest(source_bytes));
        let asset_hash = hex::encode(Sha256::digest(asset_bytes));
        let source_path = root
            .path()
            .join("sources")
            .join(content_relative_path(&source_hash).expect("source path"));
        let asset_path = root
            .path()
            .join("assets")
            .join(content_relative_path(&asset_hash).expect("asset path"));
        let staging_path = root.path().join("staging/inspection-recovery.json");
        let staged_asset = root
            .path()
            .join("staging/inspection-recovery-asset.partial");

        {
            let storage = Storage::open(root.path()).expect("open storage");
            fs::create_dir_all(source_path.parent().expect("source parent"))
                .expect("source directory");
            fs::create_dir_all(asset_path.parent().expect("asset parent"))
                .expect("asset directory");
            fs::write(&source_path, source_bytes).expect("source CAS");
            fs::write(&asset_path, asset_bytes).expect("asset CAS");
            fs::write(&staging_path, b"staging").expect("source staging");
            fs::write(&staged_asset, b"asset staging").expect("asset staging");
            storage
                .connection()
                .expect("connection")
                .execute(
                    "INSERT INTO import_jobs
                     (id, source_hash, staging_path, state, updated_at, asset_hashes_json)
                     VALUES (?1, ?2, ?3, 'file_stored', ?4, ?5)",
                    params![
                        "recovery-job",
                        source_hash,
                        staging_path.to_string_lossy(),
                        Utc::now().to_rfc3339(),
                        serde_json::to_string(&vec![asset_hash.clone()]).expect("asset hashes")
                    ],
                )
                .expect("insert recovery journal");
        }

        let reopened = Storage::open(root.path()).expect("recover storage");
        assert!(!source_path.exists());
        assert!(!asset_path.exists());
        assert!(!staging_path.exists());
        assert!(!staged_asset.exists());
        assert!(!reopened.recovery_pending().expect("recovery status"));
        assert_eq!(
            reopened
                .schema_version()
                .expect("read durable schema version"),
            SCHEMA_VERSION
        );
    }

    #[test]
    fn prompt_history_query_bounds_rows_and_multibyte_content_before_loading() {
        let root = tempdir().expect("temp root");
        let mut staged = NamedTempFile::new_in(root.path()).expect("staging");
        staged.write_all(b"character").expect("source");
        let character = Character::new("Segu", "Guide", hex::encode(Sha256::digest(b"character")));
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .commit_character_import(
                staged.path(),
                &character,
                9,
                &Uuid::new_v4().to_string(),
                &[],
            )
            .expect("commit import");
        let conversation = Conversation::new(&character.id, &character.name);
        storage
            .save_conversation(&conversation)
            .expect("save conversation");

        let base = Utc::now();
        for (index, content) in ["old", "😀😀", "😀😀😀", "latest"].into_iter().enumerate()
        {
            let mut message = Message::user(conversation.id.clone(), content);
            message.created_at =
                base + Duration::seconds(i64::try_from(index).expect("small fixture index"));
            storage.save_message(&message).expect("save message");
        }

        let history = storage
            .list_recent_messages_for_prompt(&conversation.id, 3, 8, 8)
            .expect("bounded history");
        let contents = history
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>();
        assert_eq!(contents, vec!["old", "😀😀", "latest"]);

        let recent = storage
            .list_recent_messages_for_prompt(&conversation.id, 2, 64, 64)
            .expect("recent history");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[1].content, "latest");
    }

    #[test]
    fn import_commit_observer_proves_cas_durability_precedes_sqlite_commit() {
        let root = tempdir().expect("temp root");
        let mut source = NamedTempFile::new_in(root.path()).expect("source staging");
        source.write_all(b"character").expect("source");
        let mut asset = NamedTempFile::new_in(root.path()).expect("asset staging");
        asset.write_all(b"avatar").expect("asset");
        let source_hash = hex::encode(Sha256::digest(b"character"));
        let asset_hash = hex::encode(Sha256::digest(b"avatar"));
        let mut character = Character::new("Segu", "Guide", &source_hash);
        character.avatar_asset_hash = Some(asset_hash.clone());
        let staged_assets = vec![StagedAssetImport {
            staged_path: asset.path().to_path_buf(),
            sha256: asset_hash.clone(),
            media_type: "image/png".to_owned(),
            size_bytes: 6,
        }];
        let storage = Storage::open(root.path()).expect("open storage");
        let source_cas = root
            .path()
            .join("sources")
            .join(content_relative_path(&source_hash).expect("source path"));
        let asset_cas = root
            .path()
            .join("assets")
            .join(content_relative_path(&asset_hash).expect("asset path"));
        let mut phases = Vec::new();

        storage
            .commit_character_import_observed(
                source.path(),
                &character,
                9,
                "observed-import",
                &staged_assets,
                |phase| {
                    let stats = storage.stats().expect("stats at phase");
                    match phase {
                        ImportCommitPhase::JournalCreated
                        | ImportCommitPhase::JournalMarkedFileStored => {
                            assert_eq!(stats.pending_imports, 1);
                            assert_eq!(stats.characters, 0);
                        }
                        ImportCommitPhase::CasFilesDurable => {
                            assert!(source_cas.is_file());
                            assert!(asset_cas.is_file());
                            assert_eq!(stats.pending_imports, 1);
                            assert_eq!(stats.characters, 0);
                        }
                        ImportCommitPhase::RecordsCommitted => {
                            assert_eq!(stats.pending_imports, 0);
                            assert_eq!(stats.characters, 1);
                        }
                    }
                    phases.push(phase);
                },
            )
            .expect("observed import");

        assert_eq!(
            phases,
            vec![
                ImportCommitPhase::JournalCreated,
                ImportCommitPhase::CasFilesDurable,
                ImportCommitPhase::JournalMarkedFileStored,
                ImportCommitPhase::RecordsCommitted,
            ]
        );
    }

    #[test]
    fn approved_asset_delivery_revalidates_descriptor_hash_signature_and_range() {
        let root = tempdir().expect("temp root");
        let source_bytes = b"synthetic character";
        let image_bytes = b"\x89PNG\r\n\x1a\nsynthetic-image";
        let mut source = NamedTempFile::new_in(root.path()).expect("source staging");
        source.write_all(source_bytes).expect("source");
        let mut asset = NamedTempFile::new_in(root.path()).expect("asset staging");
        asset.write_all(image_bytes).expect("asset");
        let source_hash = hex::encode(Sha256::digest(source_bytes));
        let asset_hash = hex::encode(Sha256::digest(image_bytes));
        let asset_digest = Sha256Digest::parse(&asset_hash).expect("asset digest");
        let descriptor = AssetDescriptor {
            id: AssetId::from("avatar"),
            sha256: asset_digest.clone(),
            media_type: "image/png".to_owned(),
            role: lorepia_domain::AssetRole::Avatar,
            name: "avatar.png".to_owned(),
            size_bytes: u64::try_from(image_bytes.len()).expect("small image"),
            width: Some(1),
            height: Some(1),
            duration_ms: None,
            source: lorepia_domain::AssetSource {
                kind: lorepia_domain::AssetSourceKind::CharxPackage,
                source_sha256: Some(Sha256Digest::parse(&source_hash).expect("source digest")),
                logical_path: Some("assets/avatar.png".to_owned()),
            },
        };
        let mut content = CharacterContentV1::default();
        content.assets.push(descriptor.clone());
        let mut character = Character::new("Segu", "Guide", &source_hash);
        character.avatar_asset_hash = Some(asset_hash.clone());
        let staged_assets = [StagedAssetImport {
            staged_path: asset.path().to_path_buf(),
            sha256: asset_hash.clone(),
            media_type: "image/png".to_owned(),
            size_bytes: descriptor.size_bytes,
        }];
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .commit_character_import_with_content(
                source.path(),
                &character,
                &content,
                &"ab".repeat(32),
                u64::try_from(source_bytes.len()).expect("small source"),
                "approved-asset-import",
                &staged_assets,
            )
            .expect("commit approved asset");

        assert_eq!(
            storage
                .resolve_approved_asset_by_id(&descriptor.id)
                .expect("resolve by id"),
            descriptor
        );
        let hash_verifications = storage.approved_asset_hash_verification_count();
        assert_eq!(hash_verifications, 1);
        assert_eq!(
            storage
                .resolve_approved_asset_by_sha256(&asset_digest)
                .expect("resolve by digest"),
            descriptor
        );
        let range = storage
            .read_approved_asset_range(&asset_digest, 1, 4)
            .expect("read exact range");
        assert_eq!(range.start, 1);
        assert_eq!(range.bytes, image_bytes[1..5]);
        let second_range = storage
            .read_approved_asset_range(&asset_digest, 8, 5)
            .expect("read second range");
        assert_eq!(second_range.start, 8);
        assert_eq!(second_range.bytes, image_bytes[8..13]);
        assert_eq!(
            storage.approved_asset_hash_verification_count(),
            hash_verifications,
            "repeated ranges must reuse the verified handle"
        );

        let cas_path = root
            .path()
            .join("assets")
            .join(content_relative_path(&asset_hash).expect("asset path"));
        #[cfg(windows)]
        {
            // Windows deliberately opens verified assets without write sharing,
            // so the live cache lease must be closed before an external mutation.
            drop(storage);
            fs::write(&cas_path, b"\x89PNG\r\n\x1a\nchanged").expect("tamper CAS");
            let reopened = Storage::open(root.path()).expect("reopen tampered storage");
            let error = reopened
                .resolve_approved_asset_by_sha256(&asset_digest)
                .expect_err("tampered CAS must fail closed after reopen");
            assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        }
        #[cfg(not(windows))]
        {
            fs::write(&cas_path, b"\x89PNG\r\n\x1a\nchanged").expect("tamper CAS");
            let error = storage
                .resolve_approved_asset_by_sha256(&asset_digest)
                .expect_err("tampered CAS must fail closed");
            assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        }
    }

    #[test]
    fn approved_asset_delivery_rejects_non_renderer_media() {
        let root = tempdir().expect("temp root");
        let source_bytes = b"synthetic package";
        let attachment_bytes = b"%PDF-synthetic";
        let mut source = NamedTempFile::new_in(root.path()).expect("source staging");
        source.write_all(source_bytes).expect("source");
        let mut asset = NamedTempFile::new_in(root.path()).expect("asset staging");
        asset.write_all(attachment_bytes).expect("asset");
        let source_hash = hex::encode(Sha256::digest(source_bytes));
        let asset_hash = hex::encode(Sha256::digest(attachment_bytes));
        let descriptor = AssetDescriptor {
            id: AssetId::from("attachment"),
            sha256: Sha256Digest::parse(&asset_hash).expect("asset digest"),
            media_type: "application/pdf".to_owned(),
            role: lorepia_domain::AssetRole::Attachment,
            name: "attachment.pdf".to_owned(),
            size_bytes: u64::try_from(attachment_bytes.len()).expect("small attachment"),
            width: None,
            height: None,
            duration_ms: None,
            source: lorepia_domain::AssetSource {
                kind: lorepia_domain::AssetSourceKind::CharxPackage,
                source_sha256: Some(Sha256Digest::parse(&source_hash).expect("source digest")),
                logical_path: Some("assets/attachment.pdf".to_owned()),
            },
        };
        let mut content = CharacterContentV1::default();
        content.assets.push(descriptor.clone());
        let character = Character::new("Segu", "Guide", &source_hash);
        let staged_assets = [StagedAssetImport {
            staged_path: asset.path().to_path_buf(),
            sha256: asset_hash,
            media_type: descriptor.media_type.clone(),
            size_bytes: descriptor.size_bytes,
        }];
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .commit_character_import_with_content(
                source.path(),
                &character,
                &content,
                &"cd".repeat(32),
                u64::try_from(source_bytes.len()).expect("small source"),
                "attachment-import",
                &staged_assets,
            )
            .expect("commit inert attachment");

        let error = storage
            .resolve_approved_asset_by_id(&descriptor.id)
            .expect_err("attachments must not reach the renderer protocol");
        assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
    }

    struct DurableCommittingPackageFixture {
        root: tempfile::TempDir,
        import_id: String,
        database_path: PathBuf,
        promoted_asset_path: PathBuf,
        partial_document: Option<(String, String)>,
    }

    #[allow(clippy::too_many_lines)] // The fixture follows every guarded durable transition.
    fn durable_committing_package_fixture(
        import_id: &str,
        with_partial_document: bool,
    ) -> DurableCommittingPackageFixture {
        let root = tempdir().expect("temp root");
        let source_sha256 = "ab".repeat(32);
        let snapshot_sha256 = "cd".repeat(32);
        let audit_payload = r#"{"schema_version":1,"value":{}}"#;
        let audit_payload_sha256 = hex::encode(Sha256::digest(audit_payload.as_bytes()));
        let created_at = "2026-08-16T00:00:00Z";
        let storage = Storage::open(root.path()).expect("open storage");
        let asset_bytes = b"\x89PNG\r\n\x1a\ninterrupted-package-asset";
        let asset_sha256 = hex::encode(Sha256::digest(asset_bytes));
        let staged_asset_path = storage.staging_dir().join("interrupted-asset.png");
        fs::write(&staged_asset_path, asset_bytes).expect("write staged package asset");
        let promoted_asset_path = storage
            .promote_package_assets(
                import_id,
                &[StagedAssetImport {
                    staged_path: staged_asset_path,
                    sha256: asset_sha256,
                    media_type: "image/png".to_owned(),
                    size_bytes: u64::try_from(asset_bytes.len()).expect("small package asset"),
                }],
            )
            .expect("promote package asset before commit")
            .into_iter()
            .next()
            .expect("one promoted package asset");
        let partial_document = with_partial_document.then(|| {
            let component_sha256 = "ef".repeat(32);
            let book: lorepia_domain::KnowledgeBook = serde_json::from_value(serde_json::json!({
                "id": format!("{import_id}-knowledge"),
                "name": "Interrupted package knowledge",
                "schema_version": 1,
                "entries": [],
                "scan_depth": 1,
                "token_budget": {"max_tokens": 1024},
                "recursive": false,
                "max_recursion_depth": 0,
                "provenance": {
                    "source_kind": "imported_package",
                    "source_id": "interrupted-package",
                    "source_hash": source_sha256,
                    "author": null,
                    "license": null,
                    "imported_at": null
                }
            }))
            .expect("valid interrupted knowledge document");
            let descriptor = lorepia_orchestration::PackageComponentDescriptor {
                id: format!("{import_id}-component"),
                kind: lorepia_orchestration::PackageComponentKind::KnowledgeBook,
                logical_path: "knowledge/interrupted.json".to_owned(),
                sha256: Sha256Digest::parse(component_sha256).expect("component digest"),
                dependencies: Vec::new(),
                conflicts_with: Vec::new(),
                required_capabilities: Vec::new(),
                asset_ids: Vec::new(),
                disposition: lorepia_orchestration::PackageComponentDisposition::Importable,
            };
            let document_json = serde_json::to_string(&book).expect("encode knowledge document");
            let document_sha256 = hex::encode(Sha256::digest(document_json.as_bytes()));
            let review_json = serde_json::to_string(&descriptor).expect("encode component review");
            let review_sha256 = hex::encode(Sha256::digest(review_json.as_bytes()));
            let revision_id = format!("{import_id}-knowledge-revision-1");
            (
                book,
                descriptor,
                document_json,
                document_sha256,
                review_json,
                review_sha256,
                revision_id,
            )
        });
        let database_path = {
            let connection = storage.connection().expect("database connection");
            let database_path = connection
                .query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
                .map(PathBuf::from)
                .expect("active database path");
            connection
                .execute(
                    "INSERT INTO content_sources (
                         sha256, relative_path, size_bytes, created_at
                     ) VALUES (?1, 'sources/synthetic-interrupted-package', 1, ?2)",
                    params![source_sha256, created_at],
                )
                .expect("insert package source CAS row");
            connection
                .execute(
                    "INSERT INTO package_sources (
                         id, source_hash, format, format_version, package_id, name,
                         version, author, manifest_json, manifest_sha256,
                         license_expression, license_status, redistribution_status,
                         required_app_version, signature_json, signature_status,
                         created_at
                     ) VALUES (
                         'interrupted-package-source', ?1, 'lorepia_content_package', 1,
                         'interrupted-package', 'Interrupted package', '1.0.0', NULL,
                         '{}', ?2, NULL, 'unknown', 'unknown', NULL, NULL, 'unsigned', ?3
                     )",
                    params![source_sha256, snapshot_sha256, created_at],
                )
                .expect("insert package source");
            connection
                .execute(
                    "INSERT INTO package_imports (
                         id, package_source_id, inspection_schema_version, state,
                         revision, inspection_json, inspection_sha256,
                         selection_json, selection_sha256,
                         capability_review_sha256, approved_selection_sha256,
                         approved_at, failure_json, created_at, updated_at, completed_at
                     ) VALUES (
                         ?1, 'interrupted-package-source', 1, 'inspected', 1,
                         '{}', ?2, NULL, NULL, ?2, NULL, NULL, NULL, ?3, ?3, NULL
                     )",
                    params![import_id, snapshot_sha256, created_at],
                )
                .expect("insert inspected package import");
            if let Some((book, descriptor, _, document_sha256, review_json, review_sha256, _)) =
                partial_document.as_ref()
            {
                connection
                    .execute(
                        "INSERT INTO package_import_components (
                             import_id, ordinal, source_component_key, component_kind,
                             disposition, selected, target_object_id, target_revision_id,
                             review_json, review_sha256
                         ) VALUES (?1, 0, ?2, 'knowledge_book', 'create', 1,
                                   NULL, NULL, ?3, ?4)",
                        params![import_id, descriptor.id, review_json, review_sha256],
                    )
                    .expect("insert interrupted package component");
                connection
                    .execute(
                        "INSERT INTO package_import_document_target_reviews (
                             import_id, component_ordinal, document_ordinal,
                             document_index, document_kind, target_object_id,
                             disposition, expected_target_revision_id,
                             expected_target_state_revision, source_component_sha256,
                             document_sha256
                         ) VALUES (?1, 0, 0, 0, 'knowledge_book', ?2, 'create',
                                   NULL, NULL, ?3, ?4)",
                        params![
                            import_id,
                            book.id.as_str(),
                            descriptor.sha256.as_str(),
                            document_sha256,
                        ],
                    )
                    .expect("insert interrupted package target review");
            }
            connection
                .execute(
                    "INSERT INTO package_import_audit_events (
                         import_id, sequence, import_revision, event_kind,
                         payload_json, payload_sha256, created_at
                     ) VALUES (?1, 1, 1, 'inspected', ?2, ?3, ?4)",
                    params![import_id, audit_payload, audit_payload_sha256, created_at],
                )
                .expect("insert inspection audit");
            connection
                .execute(
                    "INSERT INTO package_import_audit_events (
                         import_id, sequence, import_revision, event_kind,
                         payload_json, payload_sha256, created_at
                     ) VALUES (?1, 2, 2, 'selection_changed', ?2, ?3, ?4)",
                    params![import_id, audit_payload, audit_payload_sha256, created_at],
                )
                .expect("insert selection audit");
            connection
                .execute(
                    "UPDATE package_imports
                     SET state = 'awaiting_review', revision = 2,
                         selection_json = '{}', selection_sha256 = ?2, updated_at = ?3
                     WHERE id = ?1",
                    params![import_id, snapshot_sha256, created_at],
                )
                .expect("select package import");
            connection
                .execute(
                    "INSERT INTO package_import_approvals (
                         id, import_id, inspection_sha256, selection_sha256,
                         capability_review_sha256, approval_payload_json, approved_at
                     ) VALUES (
                         'interrupted-package-approval', ?1, ?2, ?2, ?2, '{}', ?3
                     )",
                    params![import_id, snapshot_sha256, created_at],
                )
                .expect("insert package approval");
            connection
                .execute(
                    "INSERT INTO package_import_audit_events (
                         import_id, sequence, import_revision, event_kind,
                         payload_json, payload_sha256, created_at
                     ) VALUES (?1, 3, 3, 'approved', ?2, ?3, ?4)",
                    params![import_id, audit_payload, audit_payload_sha256, created_at],
                )
                .expect("insert approval audit");
            connection
                .execute(
                    "UPDATE package_imports
                     SET state = 'approved', revision = 3,
                         approved_selection_sha256 = ?2, approved_at = ?3, updated_at = ?3
                     WHERE id = ?1",
                    params![import_id, snapshot_sha256, created_at],
                )
                .expect("approve package import");
            connection
                .execute(
                    "INSERT INTO package_import_audit_events (
                         import_id, sequence, import_revision, event_kind,
                         payload_json, payload_sha256, created_at
                     ) VALUES (?1, 4, 4, 'commit_started', ?2, ?3, ?4)",
                    params![import_id, audit_payload, audit_payload_sha256, created_at],
                )
                .expect("insert commit-started audit");
            connection
                .execute(
                    "UPDATE package_imports
                     SET state = 'committing', revision = 4, updated_at = ?2
                     WHERE id = ?1",
                    params![import_id, created_at],
                )
                .expect("persist interrupted commit state");
            if let Some((book, descriptor, document_json, document_sha256, _, _, revision_id)) =
                partial_document.as_ref()
            {
                let provenance_json =
                    serde_json::to_string(&book.provenance).expect("encode provenance");
                connection
                    .execute(
                        "INSERT INTO content_objects
                         (id, object_kind, created_at, deleted_at)
                         VALUES (?1, 'knowledge_book', ?2, NULL)",
                        params![book.id.as_str(), created_at],
                    )
                    .expect("insert partially committed content object");
                connection
                    .execute(
                        "INSERT INTO content_revisions (
                             id, object_id, object_kind, revision_no, parent_revision_id,
                             schema_version, document_json, document_sha256, source_kind,
                             source_hash, provenance_json, local_override_of_revision_id,
                             created_at
                         ) VALUES (
                             ?1, ?2, 'knowledge_book', 1, NULL, 1, ?3, ?4,
                             'imported_package', ?5, ?6, NULL, ?7
                         )",
                        params![
                            revision_id,
                            book.id.as_str(),
                            document_json,
                            document_sha256,
                            source_sha256,
                            provenance_json,
                            created_at,
                        ],
                    )
                    .expect("insert partially committed content revision");
                connection
                    .execute(
                        "INSERT INTO content_object_state
                         (object_id, active_revision_id, state_version, updated_at)
                         VALUES (?1, ?2, 1, ?3)",
                        params![book.id.as_str(), revision_id, created_at],
                    )
                    .expect("publish partial content state inside the fixture");
                connection
                    .execute(
                        "INSERT INTO content_revision_events (
                             id, object_id, event_kind, from_revision_id, to_revision_id,
                             diff_json, diff_sha256, plan_sha256, idempotency_key, created_at
                         ) VALUES (?1, ?2, 'import', NULL, ?3, NULL, NULL, NULL, ?4, ?5)",
                        params![
                            format!("{import_id}-content-event"),
                            book.id.as_str(),
                            revision_id,
                            format!("{import_id}-content-idempotency"),
                            created_at,
                        ],
                    )
                    .expect("insert partial content revision event");
                connection
                    .execute(
                        "INSERT INTO knowledge_books (
                             id, name, schema_version, revision, scan_depth, token_budget,
                             recursive, max_recursion_depth, document_json, provenance_json,
                             source_kind, source_hash, created_at, updated_at, deleted_at
                         ) VALUES (?1, ?2, 1, 1, ?3, ?4, ?5, ?6, ?7, ?8,
                                   'imported_package', ?9, ?10, ?10, NULL)",
                        params![
                            book.id.as_str(),
                            book.name,
                            book.scan_depth,
                            book.token_budget.max_tokens,
                            book.recursive,
                            book.max_recursion_depth,
                            document_json,
                            provenance_json,
                            source_sha256,
                            created_at,
                        ],
                    )
                    .expect("insert partial knowledge projection");
                connection
                    .execute(
                        "INSERT INTO knowledge_book_revisions (
                             revision_id, knowledge_book_id, revision_no, name,
                             description, token_budget, scan_depth, recursive,
                             max_recursion_depth, document_json
                         ) VALUES (?1, ?2, 1, ?3, '', ?4, ?5, ?6, ?7, ?8)",
                        params![
                            revision_id,
                            book.id.as_str(),
                            book.name,
                            book.token_budget.max_tokens,
                            book.scan_depth,
                            book.recursive,
                            book.max_recursion_depth,
                            document_json,
                        ],
                    )
                    .expect("insert partial knowledge revision projection");
                let result_json = serde_json::to_string(&serde_json::json!({
                    "schema_version": 1,
                    "value": {
                        "source_component_key": descriptor.id,
                        "component_document_ordinal": 0,
                        "source_component_sha256": descriptor.sha256.as_str(),
                        "document_sha256": document_sha256,
                        "target_object_id": book.id.as_str(),
                        "target_revision_id": revision_id,
                        "target_state_revision": 1,
                    }
                }))
                .expect("encode interrupted component result");
                let result_sha256 = hex::encode(Sha256::digest(result_json.as_bytes()));
                connection
                    .execute(
                        "INSERT INTO package_import_component_commits (
                             import_id, component_ordinal, document_ordinal,
                             target_object_id, target_revision_id, result_json,
                             result_sha256, committed_at
                         ) VALUES (?1, 0, 0, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            import_id,
                            book.id.as_str(),
                            revision_id,
                            result_json,
                            result_sha256,
                            created_at,
                        ],
                    )
                    .expect("insert partial package component result");
            }
            assert_eq!(
                connection
                    .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                    .expect("fixture integrity check"),
                "ok"
            );
            assert!(
                connection
                    .prepare("PRAGMA foreign_key_check")
                    .expect("foreign key check")
                    .query([])
                    .expect("foreign key rows")
                    .next()
                    .expect("foreign key result")
                    .is_none(),
                "fixture must satisfy every foreign key"
            );
            database_path
        };
        drop(storage);

        DurableCommittingPackageFixture {
            root,
            import_id: import_id.to_owned(),
            database_path,
            promoted_asset_path,
            partial_document: partial_document.map(|(book, _, _, _, _, _, revision_id)| {
                (book.id.as_str().to_owned(), revision_id)
            }),
        }
    }

    fn assert_durable_committing_fixture_unchanged(fixture: &DurableCommittingPackageFixture) {
        let connection =
            Connection::open(&fixture.database_path).expect("inspect rejected database");
        assert_eq!(
            connection
                .query_row(
                    "SELECT state, revision, failure_json, completed_at
                     FROM package_imports WHERE id = ?1",
                    [fixture.import_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, u64>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                        ))
                    },
                )
                .expect("load rejected package import"),
            ("committing".to_owned(), 4, None, None),
            "fail-closed open must not reinterpret or mutate impossible durable state"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM package_import_audit_events WHERE import_id = ?1",
                    [fixture.import_id.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("load unchanged package audit count"),
            4
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT event_kind FROM package_import_audit_events
                     WHERE import_id = ?1 ORDER BY sequence DESC LIMIT 1",
                    [fixture.import_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .expect("load unchanged final package audit"),
            "commit_started"
        );
        assert!(
            fixture.promoted_asset_path.exists(),
            "fail-closed open must preserve forensic CAS evidence"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM package_cas_promotion_journal
                     WHERE import_id = ?1 AND namespace = 'asset'",
                    [fixture.import_id.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("load unchanged package CAS journal"),
            1
        );
    }

    #[test]
    fn startup_rejects_impossible_durable_package_committing_state_without_mutation() {
        let fixture = durable_committing_package_fixture("package-impossible-committing", false);

        let Err(error) = Storage::open(fixture.root.path()) else {
            panic!("durable committing state must fail closed");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert_eq!(
            error.message,
            "durable package import remained in the impossible committing state"
        );
        assert_durable_committing_fixture_unchanged(&fixture);
    }

    #[test]
    fn startup_rejects_partial_package_commit_effects_without_publishing_or_mutating_them() {
        let fixture = durable_committing_package_fixture("package-partial-committing", true);
        let (document_id, revision_id) = fixture
            .partial_document
            .as_ref()
            .expect("partial document fixture");

        let Err(error) = Storage::open(fixture.root.path()) else {
            panic!("schema-valid partial package commit must fail closed");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert_eq!(
            error.message,
            "durable package import remained in the impossible committing state"
        );
        assert_durable_committing_fixture_unchanged(&fixture);

        let connection = Connection::open(&fixture.database_path).expect("inspect partial effects");
        assert_eq!(
            connection
                .query_row(
                    "SELECT state.active_revision_id, revision.source_kind
                     FROM content_objects AS object
                     JOIN content_object_state AS state ON state.object_id = object.id
                     JOIN content_revisions AS revision
                       ON revision.object_id = object.id
                      AND revision.id = state.active_revision_id
                     WHERE object.id = ?1 AND object.deleted_at IS NULL",
                    [document_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .expect("load preserved partial content"),
            (revision_id.clone(), "imported_package".to_owned())
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM package_import_component_commits
                     WHERE import_id = ?1 AND target_object_id = ?2
                       AND target_revision_id = ?3",
                    params![fixture.import_id, document_id, revision_id],
                    |row| row.get::<_, u64>(0),
                )
                .expect("load preserved component effect"),
            1
        );
        assert_eq!(
            connection
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .expect("post-rejection integrity check"),
            "ok"
        );
    }

    #[test]
    fn package_cas_recovery_removes_intent_before_file_publish() {
        let root = tempdir().expect("temp root");
        let (intent, final_path) = {
            let storage = Storage::open(root.path()).expect("open storage");
            let (_, intent, final_path) = staged_package_source(
                &storage,
                "intent.snapshot",
                b"package intent crash",
                "package-crash-intent",
            );
            ensure_package_cas_promotion_intents(
                &mut storage.connection().expect("database"),
                std::slice::from_ref(&intent),
            )
            .expect("persist intent");
            (intent, final_path)
        };
        assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
    }

    #[test]
    fn package_cas_recovery_removes_file_published_before_phase_update() {
        let root = tempdir().expect("temp root");
        let (intent, final_path) = {
            let storage = Storage::open(root.path()).expect("open storage");
            let (staged, intent, final_path) = staged_package_source(
                &storage,
                "published.snapshot",
                b"package published crash",
                "package-crash-published",
            );
            ensure_package_cas_promotion_intents(
                &mut storage.connection().expect("database"),
                std::slice::from_ref(&intent),
            )
            .expect("persist intent");
            store_verified_source(
                &staged,
                &final_path,
                &storage.data_root().join("sources/sha256"),
                &intent.sha256,
                intent.size_bytes,
            )
            .expect("publish CAS file");
            (intent, final_path)
        };
        assert!(final_path.is_file());
        assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
    }

    #[test]
    fn package_cas_recovery_removes_file_durable_before_row_registration() {
        let root = tempdir().expect("temp root");
        let (intent, final_path) = {
            let storage = Storage::open(root.path()).expect("open storage");
            let (staged, intent, final_path) = staged_package_source(
                &storage,
                "durable.snapshot",
                b"package durable crash",
                "package-crash-file-durable",
            );
            ensure_package_cas_promotion_intents(
                &mut storage.connection().expect("database"),
                std::slice::from_ref(&intent),
            )
            .expect("persist intent");
            store_verified_source(
                &staged,
                &final_path,
                &storage.data_root().join("sources/sha256"),
                &intent.sha256,
                intent.size_bytes,
            )
            .expect("publish CAS file");
            mark_package_cas_file_durable(&storage.connection().expect("database"), &intent)
                .expect("mark file durable");
            (intent, final_path)
        };
        assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
    }

    #[test]
    fn package_cas_recovery_removes_row_registered_before_product_claim() {
        let root = tempdir().expect("temp root");
        let (intent, final_path) = {
            let storage = Storage::open(root.path()).expect("open storage");
            let (staged, intent, final_path) = staged_package_source(
                &storage,
                "registered.snapshot",
                b"package row registered crash",
                "package-crash-row-registered",
            );
            storage
                .promote_package_source(
                    &intent.import_id,
                    &staged,
                    &intent.sha256,
                    intent.size_bytes,
                )
                .expect("register source CAS");
            (intent, final_path)
        };
        assert!(final_path.is_file());
        assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
    }

    #[test]
    fn package_cas_recovery_finishes_cleanup_pending_after_row_delete() {
        let root = tempdir().expect("temp root");
        let (intent, final_path) = {
            let storage = Storage::open(root.path()).expect("open storage");
            let (staged, intent, final_path) = staged_package_source(
                &storage,
                "cleanup.snapshot",
                b"package cleanup crash",
                "package-crash-cleanup-pending",
            );
            storage
                .promote_package_source(
                    &intent.import_id,
                    &staged,
                    &intent.sha256,
                    intent.size_bytes,
                )
                .expect("register source CAS");
            {
                let mut connection = storage.connection().expect("database");
                let transaction = connection.transaction().expect("cleanup transaction");
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
                    .expect("mark cleanup pending");
                transaction
                    .execute(
                        "DELETE FROM content_sources WHERE sha256 = ?1",
                        [intent.sha256.as_str()],
                    )
                    .expect("delete CAS row");
                transaction.commit().expect("commit cleanup phase");
            }
            (intent, final_path)
        };
        assert!(final_path.is_file());
        assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
    }

    #[test]
    fn package_cleanup_preserves_same_digest_pinned_by_canonical_generation() {
        for namespace in ["source", "asset"] {
            let (_root, storage, intent, final_path) =
                rollback_only_cas_promotion(namespace, &format!("rollback-cleanup-{namespace}"));
            ensure_package_cas_promotion_intents(
                &mut storage.connection().expect("active database"),
                std::slice::from_ref(&intent),
            )
            .expect("persist same-digest cleanup intent");
            let cas_mutation = storage.cas_mutation().expect("CAS mutation guard");

            assert!(
                !cleanup_package_cas_promotion(&storage, &cas_mutation, &intent)
                    .expect("clean up active-only package promotion"),
                "a canonical rollback pin is retained rather than physically removed"
            );
            assert!(
                final_path.is_file(),
                "{namespace} bytes pinned by the canonical generation must survive normal cleanup"
            );
        }
    }

    #[test]
    fn startup_recovery_preserves_same_digest_pinned_by_canonical_generation() {
        for namespace in ["source", "asset"] {
            let (root, storage, intent, final_path) =
                rollback_only_cas_promotion(namespace, &format!("rollback-recovery-{namespace}"));
            ensure_package_cas_promotion_intents(
                &mut storage.connection().expect("active database"),
                std::slice::from_ref(&intent),
            )
            .expect("persist abandoned same-digest promotion");
            drop(storage);

            let reopened = Storage::open(root.path()).expect("recover abandoned package promotion");
            assert!(
                final_path.is_file(),
                "{namespace} bytes pinned by the canonical generation must survive startup recovery"
            );
            assert_eq!(
                reopened
                    .connection()
                    .expect("recovered database")
                    .query_row(
                        "SELECT COUNT(*) FROM package_cas_promotion_journal
                         WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
                        params![intent.import_id, intent.namespace, intent.sha256],
                        |row| row.get::<_, u64>(0),
                    )
                    .expect("recovered promotion journal count"),
                0
            );
        }
    }

    #[test]
    fn package_asset_cas_recovery_removes_registered_orphan() {
        let root = tempdir().expect("temp root");
        let (sha256, size_bytes, final_path) = {
            let storage = Storage::open(root.path()).expect("open storage");
            let bytes = b"\x89PNG\r\n\x1a\npackage-asset-crash";
            let sha256 = hex::encode(Sha256::digest(bytes));
            let staged_path = storage.staging_dir().join("asset-crash.png");
            fs::write(&staged_path, bytes).expect("write staged asset");
            let staged = StagedAssetImport {
                staged_path,
                sha256: sha256.clone(),
                media_type: "image/png".to_owned(),
                size_bytes: u64::try_from(bytes.len()).expect("small asset"),
            };
            let paths = storage
                .promote_package_assets("package-asset-crash", &[staged])
                .expect("promote package asset");
            (
                sha256,
                u64::try_from(bytes.len()).expect("small asset"),
                paths[0].clone(),
            )
        };
        assert!(final_path.is_file());
        let reopened = Storage::open(root.path()).expect("reopen asset CAS storage");
        assert!(!final_path.exists());
        assert_eq!(
            reopened
                .connection()
                .expect("database")
                .query_row(
                    "SELECT COUNT(*) FROM assets WHERE sha256 = ?1",
                    [sha256.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("asset row count"),
            0
        );
        assert_eq!(size_bytes, 27);
    }

    #[test]
    fn package_cas_shared_digest_survives_other_operation_cleanup_and_restart() {
        let root = tempdir().expect("temp root");
        let (second_intent, final_path) = {
            let storage = Storage::open(root.path()).expect("open storage");
            let bytes = b"shared package CAS digest";
            let (first_staged, first_intent, final_path) =
                staged_package_source(&storage, "shared-a.snapshot", bytes, "package-shared-a");
            let (second_staged, second_intent, _) =
                staged_package_source(&storage, "shared-b.snapshot", bytes, "package-shared-b");
            storage
                .promote_package_source(
                    &first_intent.import_id,
                    &first_staged,
                    &first_intent.sha256,
                    first_intent.size_bytes,
                )
                .expect("promote first shared source");
            storage
                .promote_package_source(
                    &second_intent.import_id,
                    &second_staged,
                    &second_intent.sha256,
                    second_intent.size_bytes,
                )
                .expect("promote second shared source");
            assert!(
                !storage
                    .cleanup_package_source_promotion(
                        &first_intent.import_id,
                        &first_intent.sha256,
                        first_intent.size_bytes,
                    )
                    .expect("cleanup first shared promotion")
            );
            assert!(final_path.is_file());
            storage
                .connection()
                .expect("database")
                .execute(
                    "INSERT INTO characters (
                        id, name, description, source_hash,
                        avatar_asset_hash, created_at
                     ) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                    params![
                        "shared-cas-character",
                        "Shared CAS",
                        "Synthetic reference",
                        second_intent.sha256,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .expect("claim shared source from product data");
            (second_intent, final_path)
        };
        let reopened = Storage::open(root.path()).expect("reopen shared CAS storage");
        assert!(final_path.is_file(), "referenced shared CAS must survive");
        reopened
            .package_source_path(&second_intent.sha256, second_intent.size_bytes)
            .expect("shared source remains registered");
        assert_eq!(
            reopened
                .connection()
                .expect("database")
                .query_row(
                    "SELECT COUNT(*) FROM package_cas_promotion_journal
                     WHERE sha256 = ?1",
                    [second_intent.sha256.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .expect("journal count"),
            0
        );
    }

    #[test]
    fn atomic_cas_publish_never_replaces_an_existing_destination() {
        let root = tempdir().expect("temp root");
        let temp_path = root.path().join("new.partial");
        let final_path = root.path().join("final");
        fs::write(&temp_path, b"new").expect("temporary content");
        fs::write(&final_path, b"existing").expect("existing content");

        let error =
            publish_temp_noclobber(&temp_path, &final_path).expect_err("must not overwrite");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&final_path).expect("final content"), b"existing");
        assert_eq!(fs::read(&temp_path).expect("temporary content"), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn recovery_rejects_a_symlinked_cas_hash_prefix() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("temp root");
        let source_hash = format!("aa{}", "0".repeat(62));
        let staging_path = root.path().join("staging/inspection-symlink.json");
        {
            let storage = Storage::open(root.path()).expect("open storage");
            fs::write(&staging_path, b"staging").expect("staging");
            storage
                .connection()
                .expect("connection")
                .execute(
                    "INSERT INTO import_jobs
                     (id, source_hash, staging_path, state, updated_at, asset_hashes_json)
                     VALUES (?1, ?2, ?3, 'file_stored', ?4, '[]')",
                    params![
                        "symlink-job",
                        source_hash,
                        staging_path.to_string_lossy(),
                        Utc::now().to_rfc3339()
                    ],
                )
                .expect("journal");
        }
        let outside = root.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        symlink(&outside, root.path().join("sources/sha256/aa")).expect("prefix symlink");

        let Err(error) = Storage::open(root.path()) else {
            panic!("symlinked CAS prefix must be rejected");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert_eq!(
            error.message,
            "CAS recovery hash-prefix path is not a real directory"
        );
    }
}
