use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use crate::{
    CoreConfig, DiscoveryRecoveryOwner, Revisioned, catalog::PendingProviderCatalogImportPlan,
    core_version, revision::project_revision,
};
use chrono::{DateTime, Utc};
#[cfg(test)]
use lorepia_chat::MAX_GENERATED_OUTPUT_CHARS;
use lorepia_chat::{
    ChatEvent, ChatEventKind, GenerationFailure, GenerationOutcome, run_generation,
};
use lorepia_content::{StagedAsset, prepare_import};
use lorepia_domain::{
    ApiFamily, AppSettings, BoundedJson, CanonicalOrigin, Character, CharacterContentV1,
    ConnectionStatus, Conversation, ConversationBranch, ConversationBranchId, ConversationId,
    ConversationMode, ConversationState, CoreError, CoreErrorCode, CoreResult, EndpointPath,
    GenerationPreset, GenerationPresetId, GenerationStatus, HealthReport, ImportInspection,
    ImportLimits, InspectionId, Message, MessageId, MessageStatus, ModelMetadataSource, ModelRoute,
    ModelRouteConfig, ModelRouteId, OpaqueReasoningState, ProviderConnection, ProviderConnectionId,
    ProviderTemplate, Sha256Digest, TransformPhase,
};
#[cfg(test)]
use lorepia_domain::{
    CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, CredentialRef, GenerationId,
    GenerationProviderProvenance, GenerationRecord, GenerationRequest, GenerationTarget,
    MessageRole, ModelAvailability, ObservationId, ObservationSource, OpaqueReasoningContext,
    ParameterId, ParameterType, ProviderConnectionDraft, ProviderLocalNetworkApproval,
    ProviderNetworkMode, ProviderProfile, SupportStatus, TransformSet, UiParameterLevel,
    VariableMap,
};
use lorepia_providers::parameter_mapping::ParameterEngine;
#[cfg(test)]
use lorepia_providers::parameter_mapping::ReasoningWireDialect;
#[cfg(test)]
use lorepia_providers::parameter_mapping::{
    GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR, PromptCacheWireDialect,
};
use lorepia_providers::{AdapterRegistry, ListedModel, ModelListResult, ModelRecordSource};
#[cfg(test)]
use lorepia_providers::{
    BuiltInTemplateId, ListedModelCapabilities, ListedModelCapability,
    ListedModelReasoningCapability, OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR,
    OpenRouterReasoningEffortSupport, OpenRouterSupportedParameter,
    OpenRouterSupportedParameterSupport, Provider,
};
use lorepia_storage::{
    DatabaseStats, MessageDisplayProjectionWrite, MessageTransformApplicationWrite,
    MessageTransformDisposition, MessageTransformPipelineFailureWrite, MessageTransformStage,
    StagedAssetImport, Storage,
};
#[cfg(test)]
use lorepia_storage::{MessageGenerationAction, ProviderCredentialAccessAuthority, StoredRevision};
use serde::Serialize;
use sha2::{Digest, Sha256};
#[cfg(test)]
use tokio::sync::watch;
use tokio::{
    runtime::Handle,
    sync::{broadcast, mpsc},
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;

mod generation;
mod generation_events;
mod generation_workflow;
mod model_sync;
mod portable_runtime_state;
mod providers;
mod runtime_control;
mod runtime_generation;

use generation::{
    ActiveGenerationGuard, GenerationCompletionContext, GenerationCredential,
    GenerationEventForwardingContext, GenerationTask, GenerationTransformContext,
    MAX_ACTIVE_GENERATIONS_PER_CONVERSATION, MAX_ACTIVE_GENERATIONS_PER_PROCESS,
    MAX_ACTIVE_GENERATIONS_PER_PROVIDER, PreparedSameBranchGenerationAttempt,
    SameBranchGenerationAttempt, TerminalPersistenceContext, dispatch_auxiliary_task_provider,
    effective_capability_at, effective_route_parameter_specs, generation_attempt_prompt_authority,
    preflight_generation_target_connection_credential, validate_capability_wire_metadata,
    validate_generation_preset_candidate_plan, validate_generation_target_plan,
};
pub(crate) use generation::{
    BoundedTaskPrompt, PromptRouteWireContract, TaskDispatchClassification, TaskExecutionOutcome,
    generation_attempt_module_authority, prompt_route_supports_temperature,
    prompt_route_wire_contract, prompt_route_wire_contract_with_reasoning_effort,
    resolve_generation_target,
};
pub use generation::{
    ConnectionBoundCredential, EffectiveCapability, GenerationCredentialAdmissionLease,
    GenerationOperationContext, MAX_GENERATION_OPERATION_NONCE_BYTES,
    MAX_GENERATION_OPERATION_NONCE_CHARS,
};
#[cfg(test)]
use generation::{
    GenerationActionTargetIdentity, GenerationLaunchPermit, MessageGenerationActionIdentityInput,
    SameBranchGenerationAttemptIdentity, compiled_openrouter_parameter_spec,
    direct_model_temporal_context, load_opaque_reasoning_context, new_generation_operation_id,
    openrouter_safe_signed_parameter_specs, resolve_generation_target_with_connection_credential,
    reviewed_prompt_session_seed, same_branch_generation_semantic_fingerprint,
    unknown_task_outcome, validate_connection_credential_binding,
};
pub use generation_events::GenerationEventSubscription;
#[cfg(test)]
use generation_events::{GenerationDeliveryPhase, GenerationLivePrefix};
use generation_events::{GenerationProviderAdmissionKey, GenerationRegistry};
#[cfg(test)]
use generation_workflow::{
    apply_generation_output_transforms, apply_generation_result, partial_checkpoint_due,
    transform_content_sha256,
};
use runtime_control::RuntimeControl;

pub use providers::ProviderTemplateView;
pub(crate) use providers::provider_api_capability_observations;
#[cfg(test)]
use providers::{
    MAX_PROVIDER_BASE_URL_BYTES, MAX_PROVIDER_BASE_URL_CHARS, MAX_PROVIDER_DISPLAY_NAME_BYTES,
    MAX_PROVIDER_DISPLAY_NAME_CHARS, MAX_PROVIDER_ID_BYTES, MAX_PROVIDER_ID_CHARS,
    MAX_PROVIDER_MODEL_BYTES, MAX_PROVIDER_MODEL_CHARS,
};
use providers::{
    PROVIDER_API_CAPABILITY_FRESHNESS, openrouter_reasoning_dialect_from_capabilities,
    validate_provider_template, validate_settings_generation_target,
};

pub use portable_runtime_state::{
    PortableRuntimeStatePayload, PortableRuntimeStateRecord, PortableRuntimeStateSaveResult,
    PortableRuntimeStateScope, PortableRuntimeStateSnapshot, PortableRuntimeStateWrite,
};
#[cfg(test)]
use runtime_generation::{
    RUNTIME_MAX_OUTPUT_TOKENS, runtime_generation_request, runtime_generation_result,
};
pub use runtime_generation::{
    RuntimeGenerationAuditContext, RuntimeGenerationCapability, RuntimePromptMessage,
};

const CORE_MAX_OUTPUT_TOKENS: u32 = 4_096;
const GENERATION_SHUTDOWN_GRACE: Duration = Duration::from_millis(750);
const AUXILIARY_PROVIDER_TEARDOWN_GRACE: Duration = Duration::from_millis(750);
const PARTIAL_CHECKPOINT_INTERVAL: Duration = Duration::from_millis(500);
const PARTIAL_CHECKPOINT_BYTES: usize = 64 * 1024;
// A live catch-up snapshot may contain the provider's bounded reasoning plus
// the separately bounded DisplayOnly projection. UTF-8 uses at most four bytes
// per Unicode scalar, so these caps cover every valid transformed display.
const MAX_LIVE_DISPLAY_PREFIX_CHARS: usize = lorepia_orchestration::DEFAULT_MAX_OUTPUT_CHARS;
const MAX_LIVE_DISPLAY_PREFIX_BYTES: usize = match MAX_LIVE_DISPLAY_PREFIX_CHARS.checked_mul(4) {
    Some(bytes) => bytes,
    None => panic!("live display prefix byte bound overflowed"),
};
const MAX_USER_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_USER_MESSAGE_CHARS: usize = 16 * 1024;
const MAX_TASK_PROMPT_BYTES: usize = 512 * 1024;
const MAX_TASK_PROMPT_CHARS: usize = 128 * 1024;
const MAX_RUNTIME_PROMPT_MESSAGES: usize = 128;
const RUNTIME_GENERATION_TIMEOUT_MS: u64 = 180_000;
const MAX_TASK_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_TASK_OUTPUT_CHARS: usize = 512 * 1024;
const MAX_CONVERSATION_TITLE_BYTES: usize = 1_024;
const MAX_CONVERSATION_TITLE_CHARS: usize = 256;
const MAX_BRANCH_TITLE_BYTES: usize = 1_024;
const MAX_BRANCH_TITLE_CHARS: usize = 256;
const GENERATION_PERSISTENCE_FAILURE_MESSAGE: &str =
    "generation state could not be saved; retry the message";
const INTERACTION_DERIVED_SUPERVISOR_IDLE_POLL: Duration = Duration::from_secs(30);
const INTERACTION_DERIVED_SUPERVISOR_ERROR_POLL: Duration = Duration::from_secs(1);
const INTERACTION_DERIVED_SUPERVISOR_MIN_DELAY: Duration = Duration::from_millis(10);

#[derive(Clone)]
pub struct Core {
    inner: Arc<CoreInner>,
}

struct CoreInner {
    storage: Arc<Storage>,
    discovery_recovery_owner: DiscoveryRecoveryOwner,
    runtime: RuntimeControl,
    pending_imports: RwLock<HashMap<InspectionId, PendingImport>>,
    pending_catalog_import_plans: Mutex<HashMap<String, PendingProviderCatalogImportPlan>>,
    pending_discovery_credential_reservations: Mutex<HashSet<String>>,
    active_generations: Arc<GenerationRegistry>,
    active_model_syncs: Arc<model_sync::ModelSyncRegistry>,
    event_bus: broadcast::Sender<ChatEvent>,
}

#[derive(Clone)]
struct PendingImport {
    path: PathBuf,
    inspection: ImportInspection,
    character_content: CharacterContentV1,
    plan_hash: String,
    staged_assets: Vec<StagedAsset>,
}

/// Non-secret provenance for one successful provider model-list request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelRefreshProvenance {
    pub source: String,
    pub api_family: ApiFamily,
    pub api_origin: CanonicalOrigin,
    pub endpoint_path: EndpointPath,
}

/// Reconciled model catalog state returned to native clients.
///
/// Raw provider responses and credentials are intentionally excluded. Missing
/// routes remain in `model_routes` with `MissingTemporarily` availability so
/// existing presets and selections can be repaired explicitly by native UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelRefreshResult {
    pub connection_id: ProviderConnectionId,
    pub model_routes: Vec<ModelRoute>,
    pub newly_seen_model_route_ids: Vec<ModelRouteId>,
    pub missing_model_route_ids: Vec<ModelRouteId>,
    pub created_generation_preset_ids: Vec<GenerationPresetId>,
    pub routes_requiring_preset_configuration: Vec<ModelRouteId>,
    pub provenance: ProviderModelRefreshProvenance,
    pub pages_fetched: u32,
    pub response_bytes: u64,
    pub observed_at: DateTime<Utc>,
}

impl Drop for CoreInner {
    fn drop(&mut self) {
        self.active_generations
            .cancel_all_and_wait(GENERATION_SHUTDOWN_GRACE);
        self.active_model_syncs.cancel_all();
        self.runtime.shutdown();
    }
}

fn interaction_derived_supervisor_delay(storage: &Storage, now: DateTime<Utc>) -> Duration {
    let Ok(status) = storage.interaction_derived_event_supervisor_status() else {
        return INTERACTION_DERIVED_SUPERVISOR_ERROR_POLL;
    };
    if status.pending_count == 0 {
        return INTERACTION_DERIVED_SUPERVISOR_IDLE_POLL;
    }
    let Some(next_available_at) = status.next_available_at else {
        return INTERACTION_DERIVED_SUPERVISOR_ERROR_POLL;
    };
    next_available_at
        .signed_duration_since(now)
        .to_std()
        .unwrap_or(INTERACTION_DERIVED_SUPERVISOR_MIN_DELAY)
        .clamp(
            INTERACTION_DERIVED_SUPERVISOR_MIN_DELAY,
            INTERACTION_DERIVED_SUPERVISOR_IDLE_POLL,
        )
}

impl Core {
    pub fn open(config: CoreConfig) -> CoreResult<Self> {
        Self::open_with_discovery_recovery_owner(config, DiscoveryRecoveryOwner::Core)
    }

    /// Opens Core while assigning unfinished provider-discovery recovery to one
    /// exact owner.
    ///
    /// `NativePlatform` deliberately leaves provider-discovery WAL entries
    /// untouched. A caller selecting it must reconcile native credential state
    /// and then invoke normal Core recovery before exposing this instance.
    pub fn open_with_discovery_recovery_owner(
        config: CoreConfig,
        recovery_owner: DiscoveryRecoveryOwner,
    ) -> CoreResult<Self> {
        let storage = Arc::new(Storage::open_with_deferred_discovery_recovery(
            config.data_root,
        )?);
        for template in AdapterRegistry::built_in_templates()? {
            validate_provider_template(&template)?;
            storage.save_provider_template(&template)?;
        }
        if recovery_owner == DiscoveryRecoveryOwner::Core {
            let resumable_assistant_operations =
                crate::provider_discovery::resumable_assistant_operation_ids(&storage)?;
            storage.recover_unfinished_discovery_operations_except(
                Utc::now(),
                &resumable_assistant_operations,
            )?;
        }
        let runtime = RuntimeControl::start()?;
        let (event_bus, _) = broadcast::channel(256);
        let core = Self {
            inner: Arc::new(CoreInner {
                storage,
                discovery_recovery_owner: recovery_owner,
                runtime,
                pending_imports: RwLock::new(HashMap::new()),
                pending_catalog_import_plans: Mutex::new(HashMap::new()),
                pending_discovery_credential_reservations: Mutex::new(HashSet::new()),
                active_generations: Arc::new(GenerationRegistry::default()),
                active_model_syncs: Arc::new(model_sync::ModelSyncRegistry::default()),
                event_bus,
            }),
        };
        core.drain_interaction_derived_events()?;
        core.start_interaction_derived_supervisor();
        Ok(core)
    }

    pub fn health_check(&self) -> CoreResult<HealthReport> {
        let derived_status = self
            .inner
            .storage
            .interaction_derived_event_supervisor_status()?;
        Ok(HealthReport {
            core_version: core_version().to_owned(),
            database_open: true,
            schema_version: self.inner.storage.schema_version()?,
            data_root_writable: directory_is_writable(self.inner.storage.data_root()),
            staging_writable: directory_is_writable(&self.inner.storage.staging_dir()),
            recovery_pending: self.inner.storage.recovery_pending()?
                || derived_status.pending_count > 0,
            active_jobs: u32::try_from(
                self.active_generation_count()
                    .saturating_add(self.inner.active_model_syncs.len()),
            )
            .unwrap_or(u32::MAX),
        })
    }

    fn start_interaction_derived_supervisor(&self) {
        let weak_inner = Arc::downgrade(&self.inner);
        let mut delay = interaction_derived_supervisor_delay(&self.inner.storage, Utc::now());
        self.inner.runtime.spawn(async move {
            loop {
                time::sleep(delay).await;
                let Some(inner) = weak_inner.upgrade() else {
                    break;
                };
                let core = Core { inner };
                let _ = core.drain_interaction_derived_events();
                delay = interaction_derived_supervisor_delay(core.storage(), Utc::now());
            }
        });
    }

    pub(crate) fn storage(&self) -> &Storage {
        &self.inner.storage
    }

    pub(crate) fn discovery_recovery_owner(&self) -> DiscoveryRecoveryOwner {
        self.inner.discovery_recovery_owner
    }

    pub(crate) fn pending_catalog_import_plans(
        &self,
    ) -> &Mutex<HashMap<String, PendingProviderCatalogImportPlan>> {
        &self.inner.pending_catalog_import_plans
    }

    pub(crate) fn remember_discovery_credential_reservation(
        &self,
        physical_authority_id: &str,
    ) -> CoreResult<()> {
        let mut reservations = self
            .inner
            .pending_discovery_credential_reservations
            .lock()
            .map_err(|_| CoreError::internal("credential reservation registry is unavailable"))?;
        if !reservations.insert(physical_authority_id.to_owned()) {
            return Err(CoreError::invalid(
                "credential reservation is already active in this Core process",
            ));
        }
        Ok(())
    }

    pub(crate) fn consume_discovery_credential_reservation(
        &self,
        physical_authority_id: &str,
    ) -> CoreResult<()> {
        let mut reservations = self
            .inner
            .pending_discovery_credential_reservations
            .lock()
            .map_err(|_| CoreError::internal("credential reservation registry is unavailable"))?;
        if !reservations.remove(physical_authority_id) {
            return Err(CoreError::invalid(
                "credential reservation was not minted by this Core process",
            ));
        }
        Ok(())
    }

    pub(crate) fn forget_discovery_credential_reservation(
        &self,
        physical_authority_id: &str,
    ) -> CoreResult<bool> {
        self.inner
            .pending_discovery_credential_reservations
            .lock()
            .map(|mut reservations| reservations.remove(physical_authority_id))
            .map_err(|_| CoreError::internal("credential reservation registry is unavailable"))
    }

    #[cfg(test)]
    pub(crate) fn pending_discovery_credential_reservation_count(&self) -> usize {
        self.inner
            .pending_discovery_credential_reservations
            .lock()
            .expect("credential reservation registry")
            .len()
    }

    pub(crate) fn runtime_handle(&self) -> &Handle {
        self.inner.runtime.handle()
    }

    pub fn inspect_import(&self, staged_path: impl AsRef<Path>) -> CoreResult<ImportInspection> {
        let limits = ImportLimits::default();
        let snapshot = snapshot_import_source(
            staged_path.as_ref(),
            &self.inner.storage.staging_dir(),
            limits.max_source_bytes,
        )?;
        let prepared = match prepare_import(&snapshot, limits, &self.inner.storage.staging_dir()) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = fs::remove_file(&snapshot);
                return Err(error);
            }
        };
        let inspection = prepared.inspection;
        self.inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?
            .insert(
                inspection.id.clone(),
                PendingImport {
                    path: snapshot,
                    inspection: inspection.clone(),
                    character_content: prepared.character_content,
                    plan_hash: prepared.plan_hash,
                    staged_assets: prepared.staged_assets,
                },
            );
        Ok(inspection)
    }

    pub fn commit_import(&self, inspection_id: &InspectionId) -> CoreResult<Character> {
        let pending = self
            .inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?
            .remove(inspection_id)
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "inspection was not found", false)
            })?;
        if !pending.inspection.is_allowed() {
            let error = CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "blocked import cannot be committed",
                false,
            );
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(error);
        }
        let Ok(verified) = prepare_import(
            &pending.path,
            ImportLimits::default(),
            &self.inner.storage.staging_dir(),
        ) else {
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "import source changed or became unsafe after inspection",
                false,
            ));
        };
        let verification_matches = verified.plan_hash == pending.plan_hash
            && verified.character_content == pending.character_content
            && verified.inspection.source_sha256 == pending.inspection.source_sha256
            && verified.inspection.source_size == pending.inspection.source_size
            && verified.inspection.kind == pending.inspection.kind;
        for asset in &verified.staged_assets {
            let _ = remove_snapshot(&asset.staged_path, &self.inner.storage.staging_dir());
        }
        if !verification_matches {
            let error = CoreError::new(
                CoreErrorCode::UnsafeArchive,
                "import source or normalized inspection plan changed before commit",
                false,
            );
            self.restore_pending_import(inspection_id.clone(), pending)?;
            return Err(error);
        }
        let mut character = Character::new(
            &pending.inspection.display_name,
            &pending.inspection.description,
            &pending.inspection.source_sha256,
        );
        character.avatar_asset_hash =
            reviewed_avatar_asset_hash(&pending.inspection, &pending.staged_assets);
        let staged_assets = pending
            .staged_assets
            .iter()
            .map(|asset| StagedAssetImport {
                staged_path: asset.staged_path.clone(),
                sha256: asset.sha256.clone(),
                media_type: asset.media_type.clone(),
                size_bytes: asset.size_bytes,
            })
            .collect::<Vec<_>>();
        let commit = self.inner.storage.commit_character_import_with_content(
            &pending.path,
            &character,
            &pending.character_content,
            &pending.plan_hash,
            pending.inspection.source_size,
            &inspection_id.0,
            &staged_assets,
        );
        match commit {
            Ok(()) => {
                let _ = cleanup_pending_import(&pending, &self.inner.storage.staging_dir());
                Ok(character)
            }
            Err(error) => match self.inner.storage.get_character(&character.id) {
                Ok(committed) => {
                    let _ = cleanup_pending_import(&pending, &self.inner.storage.staging_dir());
                    Ok(committed)
                }
                Err(lookup) if lookup.code == CoreErrorCode::NotFound => {
                    self.restore_pending_import(inspection_id.clone(), pending)?;
                    Err(error)
                }
                Err(_) => Err(error),
            },
        }
    }

    pub fn discard_import(&self, inspection_id: &InspectionId) -> CoreResult<()> {
        let pending = self
            .inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?
            .remove(inspection_id)
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "inspection was not found", false)
            })?;
        cleanup_pending_import(&pending, &self.inner.storage.staging_dir())
    }

    pub fn list_characters(&self) -> CoreResult<Vec<Character>> {
        self.inner.storage.list_characters()
    }

    pub fn get_character(&self, id: &str) -> CoreResult<Character> {
        self.inner.storage.get_character(id)
    }

    /// Returns the normalized companion content persisted atomically with a
    /// character-card import.
    pub fn get_character_content(&self, id: &str) -> CoreResult<Revisioned<CharacterContentV1>> {
        self.inner
            .storage
            .get_character_content(id)
            .map(project_revision)
    }

    pub fn open_conversation(&self, character_id: &str) -> CoreResult<Conversation> {
        let character = self.get_character(character_id)?;
        let conversation =
            self.create_conversation(&character.id, character.name, ConversationMode::Chat)?;
        self.enqueue_conversation_opened(&conversation)?;
        Ok(conversation)
    }

    pub fn create_conversation(
        &self,
        character_id: &str,
        title: impl Into<String>,
        mode: ConversationMode,
    ) -> CoreResult<Conversation> {
        self.get_character(character_id)?;
        let title = normalize_bounded_text(
            "conversation title",
            title.into(),
            MAX_CONVERSATION_TITLE_BYTES,
            MAX_CONVERSATION_TITLE_CHARS,
        )?;
        let conversation = Conversation::new(character_id, title);
        self.inner
            .storage
            .save_conversation_with_mode(&conversation, mode)?;
        Ok(conversation)
    }

    /// Lists greeting identities for the exact active character-content
    /// revision without exposing greeting text.
    pub fn get_character_greeting_catalog(
        &self,
        character_id: &str,
    ) -> CoreResult<lorepia_domain::CharacterGreetingCatalog> {
        self.get_character(character_id)?;
        self.inner.storage.character_greeting_catalog(character_id)
    }

    /// Creates a room from one exact greeting selection.
    ///
    /// Storage validates the immutable character-content revision and commits
    /// the optional complete assistant greeting, branch head, conversation
    /// state, and `ConversationStarted` occurrence in one transaction.
    pub fn create_conversation_with_greeting(
        &self,
        character_id: &str,
        title: impl Into<String>,
        mode: ConversationMode,
        expected_character_content_revision_id: Option<&str>,
        greeting_id: Option<&str>,
    ) -> CoreResult<lorepia_domain::ConversationStart> {
        self.get_character(character_id)?;
        let title = normalize_bounded_text(
            "conversation title",
            title.into(),
            MAX_CONVERSATION_TITLE_BYTES,
            MAX_CONVERSATION_TITLE_CHARS,
        )?;
        let conversation = Conversation::new(character_id, title);
        self.inner.storage.save_conversation_with_greeting(
            &conversation,
            mode,
            expected_character_content_revision_id,
            greeting_id,
        )
    }

    /// Returns the immutable greeting/content revision provenance captured by
    /// the atomic conversation-start transaction.
    pub fn get_conversation_greeting_binding(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<lorepia_domain::ConversationGreetingBinding> {
        self.inner
            .storage
            .get_conversation_greeting_binding(conversation_id)
    }

    pub fn list_conversations(&self) -> CoreResult<Vec<Conversation>> {
        self.inner.storage.list_conversations()
    }

    pub fn list_conversations_for_character(
        &self,
        character_id: &str,
    ) -> CoreResult<Vec<Conversation>> {
        self.get_character(character_id)?;
        self.inner
            .storage
            .list_conversations_for_character(character_id)
    }

    pub fn get_conversation(&self, conversation_id: &ConversationId) -> CoreResult<Conversation> {
        self.inner.storage.get_conversation(conversation_id)
    }

    /// Selects an existing room and durably records its exact open snapshot.
    ///
    /// The frontend cannot submit a lifecycle event or a branch head. Core
    /// resolves both from storage and gives each real open action a fresh
    /// occurrence identity before the background lifecycle consumer runs.
    pub fn open_existing_conversation(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<Conversation> {
        let conversation = self.inner.storage.get_conversation(conversation_id)?;
        self.enqueue_conversation_opened(&conversation)?;
        Ok(conversation)
    }

    fn enqueue_conversation_opened(&self, conversation: &Conversation) -> CoreResult<()> {
        let state = self
            .inner
            .storage
            .get_conversation_state(&conversation.id)?;
        let branch = self
            .inner
            .storage
            .get_conversation_branch(&state.active_branch_id)?;
        if branch.conversation_id != conversation.id {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "active conversation branch belongs to another conversation",
                false,
            ));
        }
        self.inner.storage.enqueue_conversation_opened_occurrence(
            &Uuid::new_v4().to_string(),
            &conversation.id,
            &branch.id,
            branch.head_message_id.as_ref(),
            Utc::now(),
        )?;
        Ok(())
    }

    pub fn get_conversation_state(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<ConversationState> {
        self.inner.storage.get_conversation_state(conversation_id)
    }

    pub fn list_conversation_branches(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<Vec<ConversationBranch>> {
        self.inner.storage.get_conversation(conversation_id)?;
        self.inner
            .storage
            .list_conversation_branches(conversation_id)
    }

    pub fn create_conversation_branch(
        &self,
        conversation_id: &ConversationId,
        from_message_id: Option<&MessageId>,
        title: Option<String>,
    ) -> CoreResult<ConversationBranch> {
        let title = title
            .map(|title| {
                normalize_bounded_text(
                    "conversation branch title",
                    title,
                    MAX_BRANCH_TITLE_BYTES,
                    MAX_BRANCH_TITLE_CHARS,
                )
            })
            .transpose()?;
        let state = self.inner.storage.get_conversation_state(conversation_id)?;
        self.ensure_interaction_state_available(conversation_id, &state.active_branch_id)?;
        self.inner
            .storage
            .create_conversation_branch(conversation_id, from_message_id, title)
    }

    pub fn select_conversation_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ConversationState> {
        self.inner
            .storage
            .select_conversation_branch(conversation_id, branch_id)
    }

    pub fn set_conversation_mode(
        &self,
        conversation_id: &ConversationId,
        mode: ConversationMode,
    ) -> CoreResult<ConversationState> {
        self.inner
            .storage
            .set_conversation_mode(conversation_id, mode)
    }

    pub fn list_branch_messages(
        &self,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Vec<Message>> {
        self.inner.storage.list_branch_messages(branch_id)
    }

    pub fn list_messages(&self, conversation_id: &ConversationId) -> CoreResult<Vec<Message>> {
        let state = self.inner.storage.get_conversation_state(conversation_id)?;
        self.inner
            .storage
            .list_branch_messages(&state.active_branch_id)
    }

    pub fn remove_message_from_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
    ) -> CoreResult<ConversationBranch> {
        self.inner.storage.remove_message_from_branch(
            conversation_id,
            branch_id,
            expected_head,
            message_id,
        )
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ChatEvent> {
        self.inner.event_bus.subscribe()
    }

    pub fn get_settings(&self) -> CoreResult<AppSettings> {
        self.inner.storage.load_settings()
    }

    pub fn update_settings(&self, settings: &AppSettings) -> CoreResult<()> {
        validate_settings_generation_target(self, settings)?;
        self.inner.storage.save_settings(settings)
    }

    /// Legacy immediate-refresh entry point.
    ///
    /// Model catalog writes now require a durable diff and explicit hash
    /// approval. Call `start_provider_model_sync`, wait for
    /// `DiffReadyAwaitingReview`, then call `approve_provider_model_sync`.
    #[deprecated(
        since = "0.1.0",
        note = "use the durable start/get/approve model synchronization APIs"
    )]
    pub fn refresh_provider_models(
        &self,
        _connection_id: &ProviderConnectionId,
        _credential: Option<&str>,
    ) -> CoreResult<ProviderModelRefreshResult> {
        Err(CoreError::invalid(
            "immediate model refresh is disabled; start a durable model synchronization and approve its review hash",
        ))
    }

    pub fn database_stats(&self) -> CoreResult<DatabaseStats> {
        self.inner.storage.stats()
    }

    fn restore_pending_import(
        &self,
        inspection_id: InspectionId,
        pending: PendingImport,
    ) -> CoreResult<()> {
        let mut imports = self
            .inner
            .pending_imports
            .write()
            .map_err(|_| CoreError::internal("pending import lock was poisoned"))?;
        if let std::collections::hash_map::Entry::Vacant(entry) = imports.entry(inspection_id) {
            entry.insert(pending);
            Ok(())
        } else {
            Err(CoreError::internal(
                "inspection claim collided while restoring a retryable import",
            ))
        }
    }
}

pub(crate) type ReconciledModelRoutes = (Vec<ModelRoute>, Vec<ModelRouteId>, Vec<ModelRouteId>);

pub(crate) fn reconcile_input_routes(
    connection_id: &ProviderConnectionId,
    api_family: ApiFamily,
    existing_routes: &[ModelRoute],
    listed_models: &[ListedModel],
    observed_at: DateTime<Utc>,
) -> CoreResult<ReconciledModelRoutes> {
    let mut existing_by_identity = HashMap::with_capacity(existing_routes.len());
    let mut existing_by_id = HashMap::with_capacity(existing_routes.len());
    for route in existing_routes {
        let identity = (route.api_family, route.model_id.clone());
        if existing_by_identity.insert(identity, route).is_some() {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider connection contains duplicate model route identities",
                false,
            ));
        }
        existing_by_id.insert(route.id.clone(), route);
    }

    let mut routes = Vec::with_capacity(listed_models.len());
    let mut newly_seen = Vec::new();
    let mut listed_route_ids = HashSet::with_capacity(listed_models.len());
    for model in listed_models {
        if model.source != ModelRecordSource::ProviderApi {
            return Err(CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                "provider model list contained unsupported provenance",
                false,
            ));
        }
        let identity = (api_family, model.model_id.clone());
        let existing = existing_by_identity.get(&identity).copied();
        let route_id = existing.map_or_else(
            || deterministic_model_route_id(connection_id, api_family, &model.model_id),
            |route| route.id.clone(),
        );
        if !listed_route_ids.insert(route_id.clone()) {
            return Err(CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                "provider model list resolved to duplicate model routes",
                false,
            ));
        }
        if let Some(colliding) = existing_by_id.get(&route_id)
            && (colliding.api_family != api_family || colliding.model_id != model.model_id)
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "deterministic model route ID collides with different stored model data",
                false,
            ));
        }
        if existing.is_none() {
            newly_seen.push(route_id.clone());
        }
        routes.push(ModelRoute {
            id: route_id,
            connection_id: connection_id.clone(),
            api_family,
            model_id: model.model_id.clone(),
            // Provider listings cannot silently rename a stable local route.
            // A user-controlled catalog edit may still change this field.
            display_name: existing
                .and_then(|route| route.display_name.clone())
                .or_else(|| model.display_name.clone()),
            route_config: existing.map_or_else(ModelRouteConfig::default, |route| {
                route.route_config.clone()
            }),
            status: model.availability,
            miss_count: 0,
            raw_metadata: Some(listed_model_metadata(model)?),
            metadata_source: ModelMetadataSource::ProviderApi,
            metadata_observed_at: Some(observed_at),
            last_reconciled_sync_job_id: existing
                .and_then(|route| route.last_reconciled_sync_job_id.clone()),
            metadata_sync_job_id: existing.and_then(|route| route.metadata_sync_job_id.clone()),
            first_seen_at: existing.map_or(observed_at, |route| route.first_seen_at),
            last_seen_at: Some(observed_at),
        });
    }

    routes.sort_by(|left, right| {
        left.model_id
            .cmp(&right.model_id)
            .then_with(|| left.id.cmp(&right.id))
    });
    newly_seen.sort();
    let mut missing = existing_routes
        .iter()
        .filter(|route| !listed_route_ids.contains(&route.id))
        .map(|route| route.id.clone())
        .collect::<Vec<_>>();
    missing.sort();
    Ok((routes, newly_seen, missing))
}

fn listed_model_metadata(model: &ListedModel) -> CoreResult<BoundedJson> {
    let mut supported_generation_methods = model.supported_generation_methods.clone();
    supported_generation_methods.sort();
    supported_generation_methods.dedup();
    let mut capabilities = model.capabilities.clone();
    capabilities.supported.sort();
    capabilities.supported.dedup();
    BoundedJson::from_value(&serde_json::json!({
        "max_input_tokens": model.max_input_tokens,
        "max_output_tokens": model.max_output_tokens,
        "supported_generation_methods": supported_generation_methods,
        "capabilities": capabilities,
    }))
    .map_err(|error| {
        CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            format!("provider model metadata could not be normalized: {error}"),
            false,
        )
    })
}

fn deterministic_model_route_id(
    connection_id: &ProviderConnectionId,
    api_family: ApiFamily,
    model_id: &str,
) -> ModelRouteId {
    let identity = format!(
        "lorepia:model-route:v1\u{0}{}\u{0}{}\u{0}{model_id}",
        connection_id.as_str(),
        api_family_wire_name(api_family),
    );
    ModelRouteId::from(Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string())
}

fn deterministic_initial_preset_id(route_id: &ModelRouteId) -> GenerationPresetId {
    let identity = format!(
        "lorepia:initial-generation-preset:v1\u{0}{}",
        route_id.as_str()
    );
    GenerationPresetId::from(Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string())
}

pub(crate) fn initial_generation_preset(
    route_id: &ModelRouteId,
    template: &ProviderTemplate,
    observed_at: DateTime<Utc>,
) -> GenerationPreset {
    let reasoning = lorepia_domain::GenerationReasoningSettings {
        preserve_opaque_state: AdapterRegistry::template_supports_opaque_reasoning_state(template),
        ..lorepia_domain::GenerationReasoningSettings::default()
    };
    GenerationPreset {
        id: deterministic_initial_preset_id(route_id),
        model_route_id: route_id.clone(),
        display_name: "Default".to_owned(),
        values: Vec::new(),
        reasoning,
        prompt_cache: lorepia_domain::GenerationPromptCacheSettings::default(),
        created_at: observed_at,
        updated_at: observed_at,
    }
}

pub(crate) fn template_accepts_empty_preset(template: &ProviderTemplate) -> CoreResult<bool> {
    let parameter_engine =
        ParameterEngine::from_manifest_specs(&template.default_manifest.parameters).map_err(
            |error| CoreError::invalid(format!("provider parameter manifest is invalid: {error}")),
        )?;
    Ok(parameter_engine.validate_for_request(&[]).is_ok())
}

fn ensure_model_list_does_not_reflect_credential(
    result: &ModelListResult,
    credential: Option<&str>,
) -> CoreResult<()> {
    let Some(credential) = credential.filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    let reflected = result.models.iter().any(|model| {
        model.model_id.contains(credential)
            || model
                .display_name
                .as_deref()
                .is_some_and(|value| value.contains(credential))
            || model
                .supported_generation_methods
                .iter()
                .any(|value| value.contains(credential))
            || serde_json::to_string(&model.capabilities)
                .is_ok_and(|value| value.contains(credential))
    });
    if reflected {
        return Err(CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            "provider model list reflected credential material",
            false,
        ));
    }
    Ok(())
}

fn record_model_refresh_failure(
    storage: &Storage,
    attempted_connection: &ProviderConnection,
    error: &CoreError,
) -> CoreResult<()> {
    let status = match error.code {
        CoreErrorCode::ProviderAuthFailed => ConnectionStatus::AuthFailed,
        CoreErrorCode::ProviderRateLimited
        | CoreErrorCode::ProviderUnavailable
        | CoreErrorCode::NetworkUnavailable => ConnectionStatus::Unavailable,
        _ => return Ok(()),
    };
    let mut current = storage.get_provider_connection(&attempted_connection.id)?;
    if current != *attempted_connection {
        return Ok(());
    }
    current.status = status;
    current.updated_at = Utc::now();
    storage.save_provider_connection(&current)
}

const fn model_record_source_name(source: ModelRecordSource) -> &'static str {
    match source {
        ModelRecordSource::ProviderApi => "provider_api",
    }
}

const fn api_family_wire_name(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

fn canonical_value_sha256(value: &impl Serialize, label: &str) -> CoreResult<String> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| CoreError::internal(format!("cannot encode {label}: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn normalize_bounded_text(
    field: &str,
    value: String,
    max_bytes: usize,
    max_chars: usize,
) -> CoreResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::invalid(format!("{field} cannot be empty")));
    }
    validate_bounded_text(field, trimmed, max_bytes, max_chars)?;
    Ok(trimmed.to_owned())
}

fn validate_bounded_text(
    field: &str,
    value: &str,
    max_bytes: usize,
    max_chars: usize,
) -> CoreResult<()> {
    if value.len() > max_bytes || value.chars().count() > max_chars {
        return Err(CoreError::invalid(format!(
            "{field} exceeds the {max_bytes}-byte or {max_chars}-character limit"
        )));
    }
    Ok(())
}

fn validate_user_message_text(value: &str) -> CoreResult<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(CoreError::invalid("message text cannot be empty"));
    }
    validate_bounded_text(
        "message text",
        trimmed,
        MAX_USER_MESSAGE_BYTES,
        MAX_USER_MESSAGE_CHARS,
    )?;
    Ok(trimmed)
}

fn directory_is_writable(path: &Path) -> bool {
    if fs::create_dir_all(path).is_err() {
        return false;
    }
    let probe = path.join(format!(".health-{}", Uuid::new_v4()));
    let created = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|file| file.sync_all())
        .is_ok();
    let _ = fs::remove_file(probe);
    created
}

fn snapshot_import_source(
    source_path: &Path,
    staging_dir: &Path,
    max_source_bytes: u64,
) -> CoreResult<PathBuf> {
    let source_metadata = fs::symlink_metadata(source_path).map_err(import_io_error)?;
    if !source_metadata.file_type().is_file() {
        return Err(CoreError::invalid(
            "the import source must be a regular file and cannot be a symbolic link",
        ));
    }
    if source_metadata.len() > max_source_bytes {
        return Err(CoreError::new(
            CoreErrorCode::UnsupportedContent,
            format!(
                "source is {} bytes; maximum is {} bytes",
                source_metadata.len(),
                max_source_bytes
            ),
            false,
        ));
    }

    fs::create_dir_all(staging_dir).map_err(import_io_error)?;
    let extension = source_path
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default();
    let snapshot = staging_dir.join(format!("inspection-{}{extension}", Uuid::new_v4()));
    let result = (|| {
        let source = File::open(source_path).map_err(import_io_error)?;
        let opened_metadata = source.metadata().map_err(import_io_error)?;
        if !opened_metadata.is_file() {
            return Err(CoreError::invalid(
                "the import source is not a regular file",
            ));
        }
        let mut reader = BufReader::new(source);
        let mut destination = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&snapshot)
            .map_err(import_io_error)?;
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer).map_err(import_io_error)?;
            if read == 0 {
                break;
            }
            copied = copied
                .checked_add(
                    u64::try_from(read)
                        .map_err(|_| CoreError::internal("import byte count overflow"))?,
                )
                .ok_or_else(|| CoreError::internal("import size overflow"))?;
            if copied > max_source_bytes {
                return Err(CoreError::new(
                    CoreErrorCode::UnsupportedContent,
                    format!("source exceeds the {max_source_bytes} byte import limit"),
                    false,
                ));
            }
            destination
                .write_all(&buffer[..read])
                .map_err(import_io_error)?;
        }
        destination.flush().map_err(import_io_error)?;
        destination.sync_all().map_err(import_io_error)?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&snapshot);
        return Err(error);
    }
    Ok(snapshot)
}

fn remove_snapshot(snapshot: &Path, staging_dir: &Path) -> CoreResult<()> {
    if snapshot.parent() != Some(staging_dir) || snapshot.file_name().is_none() {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "pending import snapshot is outside the owned staging directory",
            false,
        ));
    }
    match fs::remove_file(snapshot) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(import_io_error(error)),
    }
}

fn reviewed_avatar_asset_hash(
    inspection: &ImportInspection,
    staged_assets: &[StagedAsset],
) -> Option<String> {
    let reviewed_representative = inspection.representative_image.as_ref().and_then(|image| {
        staged_assets.iter().find(|asset| {
            asset.original_path == image.logical_asset_id
                && asset.signature_valid
                && asset.media_type.starts_with("image/")
        })
    });
    reviewed_representative
        .or_else(|| {
            staged_assets
                .iter()
                .find(|asset| asset.signature_valid && asset.media_type.starts_with("image/"))
        })
        .map(|asset| asset.sha256.clone())
}

fn cleanup_pending_import(pending: &PendingImport, staging_dir: &Path) -> CoreResult<()> {
    let mut first_error = remove_snapshot(&pending.path, staging_dir).err();
    for asset in &pending.staged_assets {
        if let Err(error) = remove_snapshot(&asset.staged_path, staging_dir)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn import_io_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot stage import source: {error}"),
        true,
    )
}

#[cfg(test)]
mod tests;
