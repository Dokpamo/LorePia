mod asset_delivery;
mod bootstrap;
mod capability_observations;
mod cas_filesystem;
mod character_catalog;
mod connection;
mod connection_metrics;
mod data_root;
mod generation_presets;
mod health;
mod migration_provider_v4;
mod migration_registry;
mod migration_runner;
mod migration_special;
mod migration_verification;
mod model_routes;
mod pragmas;
mod private_path;
mod provider_catalog;
mod provider_connections;
mod provider_validation;
mod schema;
mod settings;
mod stats;

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

#[cfg(test)]
use std::sync::atomic::AtomicUsize;

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

pub(crate) use connection_metrics::DatabaseConnectionGuard;
use connection_metrics::DatabaseConnectionMetricState;
pub use connection_metrics::DatabaseConnectionMetrics;
pub use stats::DatabaseStats;

use capability_observations::{
    capability_observation_columns, decode_capability_observation_row,
    validate_provider_api_snapshot_observations_for_routes,
};
pub(crate) use capability_observations::{
    upsert_capability_observation_row, validate_provider_api_snapshot_observation,
};
use character_catalog::{
    active_character_content_revision, resolve_character_greeting,
    stale_character_greeting_catalog_error, validate_character_greeting_id,
};
pub(crate) use generation_presets::upsert_generation_preset_row;
use generation_presets::{
    decode_generation_preset_row, encode_generation_preset_values, generation_preset_columns,
    validate_generation_preset_for_schema, validate_parameter_value,
};
use model_routes::{
    api_family_to_str, model_availability_to_str, str_to_api_family,
    validate_model_route_for_schema,
};
pub(crate) use model_routes::{load_model_routes_for_reconciliation, upsert_model_route_row};
use provider_catalog::{
    active_retained_legacy_profile_exists, canonical_origin_for_legacy_base_url,
    current_migrated_legacy_route_exists, decode_provider_template_row, is_loopback_host,
    legacy_api_base_path, legacy_network_mode, legacy_provider_graph, legacy_provider_template,
    save_provider_template_row,
};
pub(crate) use provider_connections::{
    archive_provider_connection_row, decode_provider_connection_row,
    ensure_provider_connection_has_no_unfinished_work, provider_connection_columns,
    upsert_provider_connection_row,
};
use provider_connections::{
    connection_status_to_str, ensure_generation_provider_credential_settled,
};
use provider_validation::{
    is_sensitive_configuration_key, validate_connection_config, validate_nonempty,
    validate_provider_connection, validate_provider_local_network_approval_integrity,
    validate_provider_network_contract, validate_route_config,
};
use settings::{
    advance_provider_selection_revision, clear_provider_selections_for_preset,
    clear_provider_selections_for_route, ensure_stable_local_user_settings,
    legacy_profile_current_route_id_for_schema, load_provider_selection_revision,
    load_recovery_settings, normalize_settings_for_schema, update_stored_settings,
    update_stored_settings_without_selection_revision,
};

#[cfg(test)]
use cas_filesystem::publish_temp_noclobber;
use cas_filesystem::{
    content_relative_path, ensure_real_directory, ensure_regular_file, hash_open_file,
    store_verified_source, store_verified_source_observed, sync_directory, verify_file,
};
pub(crate) use connection::{
    open_backup_destination, open_configured, open_cutover_source, reserve_source_writes,
};
use pragmas::register_integrity_functions;
pub(crate) use pragmas::validate_database_integrity;

#[cfg(test)]
use migration_provider_v4::{insert_legacy_provider_template, validate_provider_catalog_migration};
pub(crate) use migration_registry::FROZEN_NATIVE_MIGRATIONS;
#[cfg(test)]
use migration_registry::{
    MIGRATION_0001, MIGRATION_0002, MIGRATION_0003, MIGRATION_0004, MIGRATION_0005, MIGRATION_0006,
    MIGRATION_0007, MIGRATION_0008, MIGRATION_0009, MIGRATION_0010,
};
pub(crate) use migration_runner::apply_migrations;
pub(crate) use migration_special::truncate_sensitive_migration_wal;
pub(crate) use migration_verification::{
    read_current_schema_version, read_pre_migration_schema_version,
};
pub(crate) use schema::{FROZEN_NATIVE_SCHEMA_VERSION, SCHEMA_VERSION};

use crate::interaction_repository::{
    InteractionStateKey, clone_interaction_checkpoint_for_branch_transaction,
    interaction_state_key_for_branch, materialize_generation_attempt_interaction_for_append,
};
use crate::verified_asset_cache::VerifiedAssetCache;

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

    pub fn save_conversation(&self, conversation: &Conversation) -> CoreResult<()> {
        self.save_conversation_with_mode(conversation, ConversationMode::Chat)
            .map(|_| ())
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

    include!("database/tests/shared.rs");
    include!("database/tests/network_and_owner_lock.rs");
    include!("database/tests/generation_completion.rs");
    include!("database/tests/provider_catalog_migrations.rs");
    include!("database/tests/provider_capabilities.rs");
    include!("database/tests/provider_catalog.rs");
    include!("database/tests/provider_archive.rs");
    include!("database/tests/conversation_and_import.rs");
    include!("database/tests/asset_delivery.rs");
    include!("database/tests/package_cas_recovery.rs");
}
