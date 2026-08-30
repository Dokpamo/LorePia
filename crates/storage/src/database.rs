mod action;
mod asset_delivery;
mod bootstrap;
mod branches;
mod capability_observations;
mod cas_filesystem;
mod character_catalog;
mod character_import;
mod connection;
mod connection_metrics;
mod conversations;
mod data_root;
mod finalize;
mod generation_append;
mod generation_presets;
mod health;
mod interrupted_generation_recovery;
mod messages;
mod migration_provider_v4;
mod migration_registry;
mod migration_runner;
mod migration_special;
mod migration_verification;
mod model_routes;
mod package_cas;
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

pub use asset_delivery::ApprovedAssetRange;
pub(crate) use connection_metrics::DatabaseConnectionGuard;
use connection_metrics::DatabaseConnectionMetricState;
pub use connection_metrics::DatabaseConnectionMetrics;
pub use messages::{MessageGenerationAction, MessageGenerationActionContext};
pub use stats::DatabaseStats;

pub(crate) use interrupted_generation_recovery::{
    InterruptedGenerationClosure, close_interrupted_generations_in_transaction,
    load_interrupted_generation_closures,
};

use branches::{map_conversation_branch, mode_to_str, str_to_mode};
use messages::{
    active_generation_action_error, insert_message, load_branch_action_target,
    load_message_generation_action_context, map_message, status_to_str, str_to_status,
};

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
#[cfg(test)]
use character_import::ImportCommitPhase;
pub use character_import::StagedAssetImport;
pub(crate) use character_import::prepare_cutover_candidate_for_open;
use character_import::{recover_interrupted_work, remove_abandoned_staging_files};
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
#[cfg(test)]
use package_cas::{
    PackageCasPromotionIntent, cleanup_package_cas_promotion, ensure_package_cas_promotion_intents,
    mark_package_cas_file_durable,
};
pub(crate) use package_cas::{claim_package_asset_promotions, claim_package_source_promotion};
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
    normalize_settings_for_schema, update_stored_settings,
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

struct StoredGenerationRoute {
    conversation: String,
    branch: String,
    user_message: String,
    assistant_message: Option<String>,
    provider_family: Option<ApiFamily>,
}

impl Storage {
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

fn stale_branch_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "conversation branch head changed; refresh before retrying",
        true,
    )
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
