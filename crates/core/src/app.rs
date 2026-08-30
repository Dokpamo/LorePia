use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
#[cfg(test)]
use std::{fs::File, path::PathBuf};

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
#[cfg(test)]
use lorepia_domain::{
    ApiFamily, BoundedJson, CanonicalOrigin, CapabilityKey, CapabilityObservation, CapabilityValue,
    Confidence, ConnectionStatus, CredentialRef, EndpointPath, GenerationId, GenerationPreset,
    GenerationPresetId, GenerationProviderProvenance, GenerationRecord, GenerationRequest,
    GenerationTarget, MessageRole, ModelAvailability, ModelMetadataSource, ModelRoute,
    ModelRouteConfig, ModelRouteId, ObservationId, ObservationSource, OpaqueReasoningContext,
    ParameterId, ParameterType, ProviderConnection, ProviderConnectionDraft, ProviderConnectionId,
    ProviderLocalNetworkApproval, ProviderNetworkMode, ProviderProfile, ProviderTemplate,
    SupportStatus, TransformSet, UiParameterLevel, VariableMap,
};
use lorepia_domain::{
    AppSettings, Character, CharacterContentV1, Conversation, ConversationBranch,
    ConversationBranchId, ConversationId, ConversationMode, ConversationState, CoreError,
    CoreErrorCode, CoreResult, GenerationStatus, HealthReport, Message, MessageId, MessageStatus,
    OpaqueReasoningState, Sha256Digest, TransformPhase,
};
#[cfg(test)]
use lorepia_providers::parameter_mapping::ReasoningWireDialect;
#[cfg(test)]
use lorepia_providers::parameter_mapping::{
    GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR, PromptCacheWireDialect,
};
use lorepia_providers::{AdapterRegistry, ModelRecordSource};
#[cfg(test)]
use lorepia_providers::{
    BuiltInTemplateId, ListedModel, ListedModelCapabilities, ListedModelCapability,
    ListedModelReasoningCapability, OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR,
    OpenRouterReasoningEffortSupport, OpenRouterSupportedParameter,
    OpenRouterSupportedParameterSupport, Provider,
};
use lorepia_storage::{
    DatabaseStats, MessageDisplayProjectionWrite, MessageTransformApplicationWrite,
    MessageTransformDisposition, MessageTransformPipelineFailureWrite, MessageTransformStage,
    Storage,
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
mod imports;
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
use imports::PendingImportRegistry;
use runtime_control::RuntimeControl;

#[allow(
    unused_imports,
    reason = "preserve the existing crate::app type path for in-crate callers"
)]
pub(crate) use providers::ReconciledModelRoutes;
#[cfg(test)]
use providers::{
    MAX_PROVIDER_BASE_URL_BYTES, MAX_PROVIDER_BASE_URL_CHARS, MAX_PROVIDER_DISPLAY_NAME_BYTES,
    MAX_PROVIDER_DISPLAY_NAME_CHARS, MAX_PROVIDER_ID_BYTES, MAX_PROVIDER_ID_CHARS,
    MAX_PROVIDER_MODEL_BYTES, MAX_PROVIDER_MODEL_CHARS, deterministic_model_route_id,
    listed_model_metadata,
};
use providers::{
    PROVIDER_API_CAPABILITY_FRESHNESS, ensure_model_list_does_not_reflect_credential,
    model_record_source_name, openrouter_reasoning_dialect_from_capabilities,
    record_model_refresh_failure, validate_provider_template, validate_settings_generation_target,
};
pub use providers::{
    ProviderModelRefreshProvenance, ProviderModelRefreshResult, ProviderTemplateView,
};
pub(crate) use providers::{
    initial_generation_preset, provider_api_capability_observations, reconcile_input_routes,
    template_accepts_empty_preset,
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
    pending_imports: PendingImportRegistry,
    pending_catalog_import_plans: Mutex<HashMap<String, PendingProviderCatalogImportPlan>>,
    pending_discovery_credential_reservations: Mutex<HashSet<String>>,
    active_generations: Arc<GenerationRegistry>,
    active_model_syncs: Arc<model_sync::ModelSyncRegistry>,
    event_bus: broadcast::Sender<ChatEvent>,
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
                pending_imports: PendingImportRegistry::new(HashMap::new()),
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

    pub fn database_stats(&self) -> CoreResult<DatabaseStats> {
        self.inner.storage.stats()
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

#[cfg(test)]
mod tests;
