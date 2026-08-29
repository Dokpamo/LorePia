use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    future::Future,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, RwLock},
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use lorepia_chat::{
    ChatEvent, ChatEventKind, GenerationFailure, GenerationOutcome, MAX_GENERATED_OUTPUT_BYTES,
    MAX_GENERATED_OUTPUT_CHARS, MAX_HISTORY_MESSAGE_BYTES, MAX_HISTORY_MESSAGE_CHARS,
    MAX_PROMPT_MESSAGES, run_generation,
};
use lorepia_content::{StagedAsset, prepare_import};
use lorepia_domain::{
    ApiFamily, AppSettings, AuthBinding, BoundedJson, CanonicalOrigin, CapabilityKey,
    CapabilityObservation, CapabilityValue, Character, CharacterContentV1, Confidence,
    ConnectionConfig, ConnectionStatus, Conversation, ConversationBranch, ConversationBranchId,
    ConversationId, ConversationMode, ConversationState, CoreError, CoreErrorCode, CoreResult,
    CredentialRedirectPolicy, CredentialRef, CredentialScope, EndpointPath, GenerationId,
    GenerationPreset, GenerationPresetId, GenerationProviderProvenance, GenerationReasoningEffort,
    GenerationReasoningMode, GenerationRecord, GenerationRequest, GenerationStatus,
    GenerationTarget, GenerationUsage, HealthReport, ImportInspection, ImportLimits, InspectionId,
    Message, MessageActionGeneration, MessageId, MessageRole, MessageStatus, ModelAvailability,
    ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId, ObservationId,
    ObservationSource, OpaqueReasoningContext, OpaqueReasoningState, ParameterDefaultMode,
    ParameterId, ParameterSpec, ParameterType, ProviderConnection, ProviderConnectionDraft,
    ProviderConnectionId, ProviderLocalNetworkApproval, ProviderNetworkMode,
    ProviderParameterMapping, ProviderParameterTarget, ProviderProfile, ProviderTemplate,
    Sha256Digest, SupportStatus, TaskProfile, TemplateSource, TransformPhase, TransformSet,
    UiParameterLevel, VariableMap, prompt_local_user_id_sha256, validate_opaque_reasoning_states,
};
use lorepia_providers::parameter_mapping::{
    GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR, OpenRouterReasoningWireStyle, ParameterEngine,
    PromptCacheControlModel, PromptCacheSettings, PromptCacheWireDialect, ProviderRequestPlan,
    ReasoningControlModel, ReasoningSettings, ReasoningWireDialect,
    parse_prompt_cache_wire_dialect_metadata, parse_reasoning_wire_dialect_metadata,
    render_prompt_cache_control, render_reasoning_control,
    validate_and_build_provider_request_plan,
};
use lorepia_providers::url_policy::{ApprovedLocalNetworkOrigin, UrlPolicy};
use lorepia_providers::{
    AdapterRegistry, BuiltInTemplateId, DeveloperRoleCapability, ListedModel,
    ListedModelCapabilities, ListedModelCapability, ListedModelReasoningCapability,
    ModelListResult, ModelRecordSource, OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR,
    OpenAiCompatibleProvider, OpenRouterReasoningEffortSupport, OpenRouterSupportedParameter,
    OpenRouterSupportedParameterSupport, Provider, ProviderEvent, RequestPreview,
    merge_capability_observations, validate_connection_fields, validate_manifest,
};
use lorepia_storage::{
    DatabaseStats, GenerationProviderTargetAuthority, MessageDisplayProjectionWrite,
    MessageGenerationAction, MessageGenerationActionContext, MessageTransformApplicationWrite,
    MessageTransformDisposition, MessageTransformPipelineFailureWrite, MessageTransformStage,
    ProviderCredentialAccessAuthority, StagedAssetImport, Storage, StoredRevision,
    deterministic_proposed_branch_id, validate_provider_api_route_metadata,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{
    runtime::{Builder, Handle},
    sync::{broadcast, mpsc, watch},
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    CoreConfig, DiscoveryRecoveryOwner,
    catalog::{CatalogRouteProjection, PendingProviderCatalogImportPlan},
    core_version,
    orchestration::{
        GenerationPlanInput, GenerationPromptAuthorityCapture, deterministic_prompt_user_message_id,
    },
};

mod model_sync;
mod runtime_generation;

#[cfg(test)]
use runtime_generation::{
    RUNTIME_MAX_OUTPUT_TOKENS, runtime_generation_request, runtime_generation_result,
};
pub use runtime_generation::{
    RuntimeGenerationAuditContext, RuntimeGenerationCapability, RuntimePromptMessage,
};

const CORE_MAX_OUTPUT_TOKENS: u32 = 4_096;
// Admission belongs to Core rather than renderer stream registrations so a
// detached or failed consumer cannot recycle a slot while provider work keeps
// running. The per-conversation allowance preserves bounded background branch
// generation while preventing one conversation from consuming the process.
const MAX_ACTIVE_GENERATIONS_PER_PROCESS: usize = 32;
const MAX_ACTIVE_GENERATIONS_PER_PROVIDER: usize = 8;
const MAX_ACTIVE_GENERATIONS_PER_CONVERSATION: usize = 4;
const GENERATION_SHUTDOWN_GRACE: Duration = Duration::from_millis(750);
const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(1);
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
pub const MAX_GENERATION_OPERATION_NONCE_BYTES: usize = 128;
pub const MAX_GENERATION_OPERATION_NONCE_CHARS: usize = 64;

/// Caller-owned boundary for a new generation operation or an exact durable
/// attempt selected for restart-safe resume. The variants are intentionally
/// exclusive so callers cannot ambiguously rotate and resume at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationOperationContext<'a> {
    New {
        operation_nonce: &'a str,
    },
    Resume {
        generation_attempt_id: &'a GenerationId,
    },
}
const MAX_TASK_PROMPT_BYTES: usize = 512 * 1024;
const MAX_TASK_PROMPT_CHARS: usize = 128 * 1024;
const MAX_RUNTIME_PROMPT_MESSAGES: usize = 128;
const RUNTIME_GENERATION_TIMEOUT_MS: u64 = 180_000;
const MAX_TASK_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const MAX_TASK_OUTPUT_CHARS: usize = 512 * 1024;
const MAX_PROVIDER_ID_BYTES: usize = 256;
const MAX_PROVIDER_ID_CHARS: usize = 64;
const MAX_PROVIDER_DISPLAY_NAME_BYTES: usize = 512;
const MAX_PROVIDER_DISPLAY_NAME_CHARS: usize = 128;
const MAX_PROVIDER_BASE_URL_BYTES: usize = 4 * 1024;
const MAX_PROVIDER_BASE_URL_CHARS: usize = 1_024;
const MAX_PROVIDER_MODEL_BYTES: usize = 1_024;
const MAX_PROVIDER_MODEL_CHARS: usize = 256;
const MAX_CONVERSATION_TITLE_BYTES: usize = 1_024;
const MAX_CONVERSATION_TITLE_CHARS: usize = 256;
const MAX_BRANCH_TITLE_BYTES: usize = 1_024;
const MAX_BRANCH_TITLE_CHARS: usize = 256;
const PROVIDER_API_CAPABILITY_FRESHNESS: chrono::Duration = chrono::Duration::hours(24);
const GENERATION_PERSISTENCE_FAILURE_MESSAGE: &str =
    "generation state could not be saved; retry the message";
const INTERACTION_DERIVED_SUPERVISOR_IDLE_POLL: Duration = Duration::from_secs(30);
const INTERACTION_DERIVED_SUPERVISOR_ERROR_POLL: Duration = Duration::from_secs(1);
const INTERACTION_DERIVED_SUPERVISOR_MIN_DELAY: Duration = Duration::from_millis(10);

#[derive(Clone)]
pub struct Core {
    inner: Arc<CoreInner>,
}

/// Atomic process-local subscription state for one running generation.
///
/// The receiver, sequence watermark, and bounded display/reasoning prefixes are
/// captured under the same delivery mutex used by generation publishers.
/// Callers therefore either observe a durable terminal status or can rebuild
/// the exact live presentation through the returned watermark before receiving
/// every later event. This process-local snapshot exists only while the
/// generation is registered as live; terminal recovery reads the durable
/// message/projection instead of subscribing again.
pub struct GenerationEventSubscription {
    receiver: broadcast::Receiver<ChatEvent>,
    assistant_message_id: MessageId,
    sequence_watermark: u64,
    display_prefix: String,
    reasoning_prefix: String,
}

impl GenerationEventSubscription {
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        broadcast::Receiver<ChatEvent>,
        MessageId,
        u64,
        String,
        String,
    ) {
        (
            self.receiver,
            self.assistant_message_id,
            self.sequence_watermark,
            self.display_prefix,
            self.reasoning_prefix,
        )
    }
}

/// Request-scoped credential material bound to one provider connection.
///
/// This type intentionally implements neither `Clone`, `Serialize`, nor
/// `Display`. The credential allocation is zeroized on drop.
pub struct ConnectionBoundCredential {
    connection_id: ProviderConnectionId,
    value: Option<String>,
    access_authority: Option<lorepia_storage::ProviderCredentialAccessAuthority>,
    dispatch_lease: Option<Box<dyn Send + Sync>>,
}

/// Secret material owned for one primary generation dispatch.
///
/// Bound credentials retain their complete carrier so its native dispatch
/// lease and zeroizing drop remain coupled to the provider future. Legacy raw
/// credentials receive the same zeroizing task-owned lifetime.
enum GenerationCredential {
    Raw(Option<String>),
    Bound(ConnectionBoundCredential),
}

impl GenerationCredential {
    fn as_deref(&self) -> Option<&str> {
        match self {
            Self::Raw(value) => value.as_deref(),
            Self::Bound(credential) => credential.value.as_deref(),
        }
    }
}

impl From<Option<String>> for GenerationCredential {
    fn from(value: Option<String>) -> Self {
        Self::Raw(value)
    }
}

impl From<ConnectionBoundCredential> for GenerationCredential {
    fn from(value: ConnectionBoundCredential) -> Self {
        Self::Bound(value)
    }
}

impl Drop for GenerationCredential {
    fn drop(&mut self) {
        if let Self::Raw(Some(value)) = self {
            value.zeroize();
        }
    }
}

/// Opaque process-local reservation retained only until a durable generation
/// attempt has been admitted.
///
/// Native hosts use this for legacy raw credentials which predate durable
/// credential authority epochs. The reservation has no serialized or debug
/// representation and is dropped before prompt-time auxiliary work begins.
pub struct GenerationCredentialAdmissionLease(Box<dyn Send + Sync>);

impl GenerationCredentialAdmissionLease {
    pub fn new(value: impl Send + Sync + 'static) -> Self {
        Self(Box::new(value))
    }

    fn release(self) {
        drop(self.0);
    }
}

impl ConnectionBoundCredential {
    pub fn new(connection_id: ProviderConnectionId, value: Option<String>) -> Self {
        Self {
            connection_id,
            value,
            access_authority: None,
            dispatch_lease: None,
        }
    }

    /// Binds credential material to the exact durable ownership authority
    /// observed by the native vault read which released it.
    pub fn new_with_access_authority(
        connection_id: ProviderConnectionId,
        value: Option<String>,
        access_authority: lorepia_storage::ProviderCredentialAccessAuthority,
    ) -> Self {
        Self {
            connection_id,
            value,
            access_authority: Some(access_authority),
            dispatch_lease: None,
        }
    }

    /// Retains one process-local native credential lease for the full provider
    /// dispatch lifetime. The lease has no serialized or debug representation.
    pub fn new_with_dispatch_lease(
        connection_id: ProviderConnectionId,
        value: Option<String>,
        dispatch_lease: impl Send + Sync + 'static,
    ) -> Self {
        Self {
            connection_id,
            value,
            access_authority: None,
            dispatch_lease: Some(Box::new(dispatch_lease)),
        }
    }

    /// Attaches a native provider-operation lease without changing the
    /// credential's durable access authority. The carrier releases the lease
    /// only after zeroizing its credential value.
    #[must_use]
    pub fn with_dispatch_lease(mut self, dispatch_lease: impl Send + Sync + 'static) -> Self {
        self.dispatch_lease = Some(Box::new(dispatch_lease));
        self
    }

    pub(crate) fn access_authority(
        &self,
    ) -> Option<&lorepia_storage::ProviderCredentialAccessAuthority> {
        self.access_authority.as_ref()
    }

    pub(crate) fn value_for_connection<'a>(
        &'a self,
        connection: &ProviderConnection,
    ) -> CoreResult<Option<&'a str>> {
        validate_connection_credential_binding(connection, self)?;
        Ok(self.value.as_deref())
    }
}

impl std::fmt::Debug for ConnectionBoundCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ConnectionBoundCredential([REDACTED])")
    }
}

impl Drop for ConnectionBoundCredential {
    fn drop(&mut self) {
        if let Some(value) = &mut self.value {
            value.zeroize();
        }
        drop(self.dispatch_lease.take());
    }
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

struct RuntimeControl {
    handle: Handle,
    shutdown_sender: Option<tokio::sync::oneshot::Sender<()>>,
    owner_thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GenerationDeliveryPhase {
    Preparing,
    Running,
    Terminal,
}

struct GenerationRoute {
    conversation: ConversationId,
    branch: ConversationBranchId,
    assistant_message: MessageId,
}

#[derive(Clone, PartialEq, Eq)]
enum GenerationProviderAdmissionKey {
    Connection(ProviderConnectionId),
    ProviderProfile(String),
    #[cfg(test)]
    DirectModel(String),
}

struct GenerationDeliveryState {
    phase: GenerationDeliveryPhase,
    sequence_watermark: u64,
    live_prefix: Option<GenerationLivePrefix>,
}

#[derive(Default)]
struct GenerationLivePrefix {
    display: String,
    reasoning: String,
    display_chars: usize,
    reasoning_chars: usize,
}

impl GenerationLivePrefix {
    fn append(&mut self, kind: &ChatEventKind) -> bool {
        let (target, chars, max_bytes, max_chars, delta) = match kind {
            ChatEventKind::TextDelta(delta) => (
                &mut self.display,
                &mut self.display_chars,
                MAX_LIVE_DISPLAY_PREFIX_BYTES,
                MAX_LIVE_DISPLAY_PREFIX_CHARS,
                delta,
            ),
            ChatEventKind::ReasoningDelta(delta) => (
                &mut self.reasoning,
                &mut self.reasoning_chars,
                MAX_GENERATED_OUTPUT_BYTES,
                MAX_GENERATED_OUTPUT_CHARS,
                delta,
            ),
            _ => return true,
        };
        let Some(next_bytes) = target.len().checked_add(delta.len()) else {
            return false;
        };
        let Some(next_chars) = chars.checked_add(delta.chars().count()) else {
            return false;
        };
        if next_bytes > max_bytes || next_chars > max_chars {
            return false;
        }
        target.push_str(delta);
        *chars = next_chars;
        true
    }
}

struct ActiveGeneration {
    cancel: watch::Sender<bool>,
    route: GenerationRoute,
    provider_admission_key: GenerationProviderAdmissionKey,
    delivery: Mutex<GenerationDeliveryState>,
    #[cfg(test)]
    subscription_pause: Mutex<Option<GenerationSubscriptionPause>>,
}

#[cfg(test)]
struct GenerationSubscriptionPause {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

#[derive(Default)]
struct GenerationRegistry {
    active: Mutex<HashMap<GenerationId, Arc<ActiveGeneration>>>,
    drained: Condvar,
}

#[derive(Clone)]
struct PendingImport {
    path: PathBuf,
    inspection: ImportInspection,
    character_content: CharacterContentV1,
    plan_hash: String,
    staged_assets: Vec<StagedAsset>,
}

struct GenerationTask {
    storage: Arc<Storage>,
    active_generations: Arc<GenerationRegistry>,
    event_bus: broadcast::Sender<ChatEvent>,
    branch_id: ConversationBranchId,
    request: GenerationRequest,
    assistant: Message,
    provider: Arc<dyn Provider>,
    credential: GenerationCredential,
    cancel_receiver: watch::Receiver<bool>,
    preserve_partial: bool,
    transforms: GenerationTransformContext,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum GenerationActionTargetIdentity {
    GenerationTarget {
        model_route_id: ModelRouteId,
        generation_preset_id: GenerationPresetId,
    },
    ProviderProfile {
        provider_profile_id: String,
    },
    #[cfg(test)]
    DirectModel {
        model_sha256: String,
    },
}

#[derive(Serialize)]
struct GenerationActionSemanticSnapshot<'a> {
    schema_version: u32,
    action: &'static str,
    conversation_id: &'a ConversationId,
    source_branch_id: &'a ConversationBranchId,
    expected_source_head_message_id: Option<&'a MessageId>,
    target_message_id: &'a MessageId,
    context_head_message_id: Option<&'a MessageId>,
    replacement_text_sha256: &'a str,
    target: &'a GenerationActionTargetIdentity,
}

struct PreparedMessageGenerationAction {
    conversation_id: ConversationId,
    source_branch_id: ConversationBranchId,
    expected_source_head_message_id: Option<MessageId>,
    target_message_id: MessageId,
    action: MessageGenerationAction,
    context: MessageGenerationActionContext,
    text: String,
    target: GenerationActionTargetIdentity,
    semantic_base_fingerprint_sha256: Sha256Digest,
    operation_id: String,
    resume_generation_attempt_id: Option<GenerationId>,
    proposed_branch_id: ConversationBranchId,
    mode: ConversationMode,
}

struct MessageGenerationActionIdentityInput<'a> {
    conversation_id: &'a ConversationId,
    source_branch_id: &'a ConversationBranchId,
    expected_source_head_message_id: Option<&'a MessageId>,
    target_message_id: &'a MessageId,
    action: MessageGenerationAction,
    replacement_text: Option<&'a str>,
    operation_context: GenerationOperationContext<'a>,
    target: GenerationActionTargetIdentity,
}

#[derive(Clone, Copy)]
struct MessageGenerationAttemptConfiguration<'a> {
    generation_target: Option<&'a GenerationTarget>,
    temperature: Option<f64>,
    max_output_tokens: Option<u32>,
    prompt_wire_contract: Option<&'a PromptRouteWireContract>,
    provider_target_authority: &'a GenerationProviderTargetAuthority,
    credential_authority: Option<&'a ProviderCredentialAccessAuthority>,
    require_exact_credential_authority: bool,
}

#[derive(Serialize)]
struct GenerationSendSemanticSnapshot<'a> {
    /// This includes only caller-owned semantic request identity. Conversation
    /// mode, provider mapping, effective quick settings, and the operation
    /// nonce are sealed or scoped separately so none can alter prompt
    /// semantics after an approval pause.
    schema_version: u32,
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    expected_head_message_id: Option<&'a MessageId>,
    user_text_sha256: &'a str,
    target: &'a GenerationActionTargetIdentity,
    temperature: Option<f64>,
    max_output_tokens: Option<u32>,
    prompt_preset_id: Option<&'a lorepia_domain::PromptPresetId>,
    variable_overrides: &'a VariableMap,
}

#[derive(Serialize)]
struct GenerationOperationNonceEnvelope<'a> {
    schema_version: u32,
    domain: &'static str,
    semantic_base_fingerprint_sha256: &'a Sha256Digest,
    operation_nonce: &'a str,
}

struct SameBranchGenerationAttemptIdentity<'a> {
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    expected_head: Option<&'a MessageId>,
    text: &'a str,
    operation_context: GenerationOperationContext<'a>,
    target: &'a GenerationActionTargetIdentity,
    temperature: Option<f64>,
    max_output_tokens: Option<u32>,
    prompt_preset_id: Option<&'a lorepia_domain::PromptPresetId>,
    variable_overrides: &'a VariableMap,
}

struct SameBranchGenerationTargetInput<'a> {
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    expected_head: Option<&'a MessageId>,
    live_mode: ConversationMode,
    text: &'a str,
    operation_context: GenerationOperationContext<'a>,
    target: &'a GenerationTarget,
    prompt_preset_id: Option<&'a lorepia_domain::PromptPresetId>,
    variable_overrides: &'a VariableMap,
}

struct PreparedSameBranchGenerationTarget {
    mode: ConversationMode,
    validated: ValidatedGenerationTarget,
    provider_target_authority: GenerationProviderTargetAuthority,
}

struct GenerationProviderTemporalContext {
    operation_target: GenerationActionTargetIdentity,
    authority: GenerationProviderTargetAuthority,
}

struct ExistingSameBranchAttemptRequest<'a> {
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    expected_head: Option<&'a MessageId>,
    operation_id: &'a str,
    base_request_fingerprint_sha256: &'a Sha256Digest,
    provider_target_authority: &'a GenerationProviderTargetAuthority,
    resume_generation_attempt_id: Option<&'a GenerationId>,
}

struct ResolvedGenerationOperationIdentity {
    operation_id: String,
    base_request_fingerprint_sha256: Sha256Digest,
    resume_generation_attempt_id: Option<GenerationId>,
}

enum ExistingSameBranchAttempt {
    Missing,
    Prepared(Box<lorepia_storage::StoredGenerationAttempt>),
    Resolved(SameBranchGenerationAttempt),
}

enum SameBranchGenerationAttempt {
    Existing(GenerationId),
    Ready(Box<PreparedSameBranchGenerationAttempt>),
}

struct PreparedSameBranchGenerationAttempt {
    attempt: lorepia_storage::StoredGenerationAttempt,
    interaction_state: lorepia_storage::StoredInteractionState,
    applied_module_plan: Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
}

struct SameBranchGenerationDispatch<'a> {
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    expected_head: Option<&'a MessageId>,
    mode: ConversationMode,
    model: String,
    generation_target: Option<&'a GenerationTarget>,
    provider_family: Option<ApiFamily>,
    preserve_opaque_reasoning_state: bool,
    credential: GenerationCredential,
    credential_authority: Option<ProviderCredentialAccessAuthority>,
    require_exact_credential_authority: bool,
    provider: Arc<dyn Provider>,
    provider_target: GenerationActionTargetIdentity,
    user_message: Message,
    attempt: PreparedSameBranchGenerationAttempt,
    prepared: crate::orchestration::PreparedGenerationPlan,
}

struct PreparedMessageActionAttempt {
    attempt: lorepia_storage::StoredGenerationAttempt,
    interaction_state: lorepia_storage::StoredInteractionState,
    target_interaction_state_key: lorepia_storage::InteractionStateKey,
    applied_module_plan: Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
}

fn build_message_action_generation_records(
    action_request: &PreparedMessageGenerationAction,
    user_message: &Message,
    generation_id: &GenerationId,
    generation_started_at: DateTime<Utc>,
    model: String,
    generation_target: Option<&GenerationTarget>,
    provider_family: Option<ApiFamily>,
) -> (Message, ConversationBranch, GenerationRecord) {
    let mut assistant_message = Message::pending_assistant(
        action_request.conversation_id.clone(),
        user_message.id.clone(),
        generation_id.clone(),
    );
    assistant_message.created_at = generation_started_at;
    let branch = ConversationBranch {
        id: action_request.proposed_branch_id.clone(),
        conversation_id: action_request.conversation_id.clone(),
        title: None,
        fork_message_id: action_request.context.fork_message_id.clone(),
        head_message_id: Some(assistant_message.id.clone()),
        created_at: generation_started_at,
        updated_at: generation_started_at,
    };
    let generation = GenerationRecord {
        id: generation_id.clone(),
        conversation_id: action_request.conversation_id.clone(),
        branch_id: branch.id.clone(),
        user_message_id: user_message.id.clone(),
        assistant_message_id: Some(assistant_message.id.clone()),
        mode: action_request.mode,
        model,
        model_route_id: generation_target.map(|target| target.model_route_id.clone()),
        generation_preset_id: generation_target.map(|target| target.generation_preset_id.clone()),
        provider_family,
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
        started_at: generation_started_at,
        finished_at: None,
    };
    (assistant_message, branch, generation)
}

enum MessageActionAttempt {
    Existing(MessageActionGeneration),
    Ready(Box<PreparedMessageActionAttempt>),
}

struct ReviewedPromptSendContext {
    mode: ConversationMode,
    resolved: ResolvedGenerationTarget,
    credential: GenerationCredential,
    credential_authority: Option<ProviderCredentialAccessAuthority>,
    user_message: Message,
    attempt: PreparedSameBranchGenerationAttempt,
}

enum ReviewedPromptSendPreparation {
    Existing(GenerationId),
    Ready(Box<ReviewedPromptSendContext>),
}

#[derive(Clone)]
struct GenerationTransformContext {
    sets: Vec<TransformSet>,
    variables: VariableMap,
    supported_capabilities: Vec<CapabilityKey>,
    approved_import_source_ids: std::collections::BTreeSet<String>,
    display_context: Option<lorepia_domain::PromptResolutionContext>,
}

impl From<crate::orchestration::PreparedGenerationPlan> for GenerationTransformContext {
    fn from(prepared: crate::orchestration::PreparedGenerationPlan) -> Self {
        Self {
            sets: prepared.transform_sets,
            variables: prepared.variables,
            supported_capabilities: prepared.supported_capabilities,
            approved_import_source_ids: prepared.approved_import_source_ids,
            display_context: Some(prepared.display_context),
        }
    }
}

pub(crate) struct ResolvedGenerationTarget {
    pub(crate) model: String,
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) api_family: ApiFamily,
    pub(crate) connection_id: ProviderConnectionId,
    pub(crate) preserve_opaque_reasoning_state: bool,
    pub(crate) prompt_wire_contract: PromptRouteWireContract,
}

/// One provider-neutral message supplied by an imported character runtime.
///
/// Runtime scripts can ask the native host for a secondary generation, but
/// they cannot supply provider request JSON, credentials, URLs, or headers.
/// Core rebuilds the request through the same provider adapters used by the
/// Bounded, Core-owned input for an auxiliary provider task.
///
/// The task runner constructs this value from a trusted instruction and
/// already-inspected source text. It is never exposed as an arbitrary provider
/// body or native DTO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BoundedTaskPrompt {
    system_instruction: String,
    input: String,
}

impl BoundedTaskPrompt {
    pub(crate) fn new(
        system_instruction: impl Into<String>,
        input: impl Into<String>,
    ) -> CoreResult<Self> {
        let prompt = Self {
            system_instruction: system_instruction.into(),
            input: input.into(),
        };
        let total_bytes = prompt
            .system_instruction
            .len()
            .checked_add(prompt.input.len())
            .ok_or_else(|| CoreError::invalid("auxiliary task prompt size overflowed"))?;
        let total_chars = prompt
            .system_instruction
            .chars()
            .count()
            .checked_add(prompt.input.chars().count())
            .ok_or_else(|| CoreError::invalid("auxiliary task prompt size overflowed"))?;
        if prompt.system_instruction.trim().is_empty()
            || prompt.input.trim().is_empty()
            || prompt.system_instruction.contains('\0')
            || prompt.input.contains('\0')
            || total_bytes > MAX_TASK_PROMPT_BYTES
            || total_chars > MAX_TASK_PROMPT_CHARS
        {
            return Err(CoreError::invalid(
                "auxiliary task prompt is empty, unsafe, or exceeds its size limit",
            ));
        }
        Ok(prompt)
    }
}

/// Dispatch certainty for one auxiliary provider attempt.
///
/// Runtime fallback is allowed only for `BeforeDispatch` and
/// `KnownNoSideEffect`. A timeout, cancellation, or ambiguous transport error
/// after the provider future starts is always `UnknownOutcome`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskDispatchClassification {
    BeforeDispatch,
    KnownNoSideEffect,
    UnknownOutcome,
    ProviderRejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TaskExecutionOutcome {
    Completed {
        canonical_text: String,
        usage: GenerationUsage,
    },
    Failed {
        classification: TaskDispatchClassification,
        error: CoreError,
    },
}

struct ValidatedGenerationTarget {
    route: ModelRoute,
    connection: ProviderConnection,
    template: ProviderTemplate,
    request_plan: ProviderRequestPlan,
    prompt_wire_contract: PromptRouteWireContract,
}

enum MigratedLegacyTargetClassification {
    Ordinary,
    Current { profile_id: String },
    Alias,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PromptRouteWireContract {
    pub(crate) model_route_id: ModelRouteId,
    pub(crate) generation_preset_id: GenerationPresetId,
    pub(crate) model: String,
    pub(crate) api_family: ApiFamily,
    pub(crate) developer_capability: DeveloperRoleCapability,
    pub(crate) cache_dialect: PromptCacheWireDialect,
    pub(crate) request_plan_sha256: String,
    pub(crate) generation_preset_sha256: String,
    pub(crate) configured_max_output_tokens: Option<u32>,
    pub(crate) context_limit_tokens: Option<u32>,
    pub(crate) observed_max_output_tokens: Option<u32>,
    pub(crate) supports_temperature: bool,
    pub(crate) reasoning_effort_applied: Option<GenerationReasoningEffort>,
}

#[derive(Serialize)]
struct ProviderProfileDispatchAuthoritySnapshot<'a> {
    schema_version: u32,
    provider_profile_id: &'a str,
    base_url: &'a str,
    model: &'a str,
    timeout_seconds: u32,
}

#[derive(Serialize)]
struct GenerationTargetResolutionAuthoritySnapshot<'a> {
    schema_version: u32,
    target: &'a GenerationTarget,
    route: &'a ModelRoute,
    connection: &'a ProviderConnection,
    template: &'a ProviderTemplate,
    request_plan: &'a ProviderRequestPlan,
    prompt_wire_contract: &'a PromptRouteWireContract,
}

struct GenerationPresetControlContext {
    route: ModelRoute,
    connection: ProviderConnection,
    template: ProviderTemplate,
    parameter_engine: ParameterEngine,
    reasoning: ReasoningSettings,
    prompt_cache: PromptCacheSettings,
    reasoning_dialect: ReasoningWireDialect,
    cache_dialect: PromptCacheWireDialect,
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

/// Native-facing provider-template presentation derived by Rust.
///
/// `default_network_mode` comes from the compiled adapter descriptor rather
/// than from native inference or persisted template JSON. This keeps Ollama's
/// loopback boundary explicit while every other built-in family defaults to
/// the public network policy.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderTemplateView {
    pub template: ProviderTemplate,
    pub default_network_mode: ProviderNetworkMode,
}

/// Deterministically merged capability state for one route and key.
///
/// Alternatives remain visible so native UI can explain disagreements rather
/// than presenting the selected value as an unqualified fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveCapability {
    pub selected: CapabilityObservation,
    pub alternatives: Vec<CapabilityObservation>,
    pub evaluated_at: DateTime<Utc>,
    pub selected_is_stale: bool,
    pub has_conflict: bool,
}

struct TerminalPersistenceContext<'a> {
    storage: &'a Storage,
    generation_id: &'a GenerationId,
}

struct GenerationCompletionContext {
    storage: Arc<Storage>,
    active_generations: Arc<GenerationRegistry>,
    event_bus: broadcast::Sender<ChatEvent>,
    branch_id: ConversationBranchId,
    conversation_id: ConversationId,
    generation_id: GenerationId,
    assistant_message_id: MessageId,
    preserve_partial: bool,
    transforms: GenerationTransformContext,
}

struct GenerationEventForwardingContext {
    active_generations: Arc<GenerationRegistry>,
    event_bus: broadcast::Sender<ChatEvent>,
    storage: Arc<Storage>,
    checkpoint: Message,
    branch_id: ConversationBranchId,
    assistant_message_id: MessageId,
    preserve_partial: bool,
    defer_text_events: bool,
}

struct GenerationLaunchPermit {
    generation_id: GenerationId,
    active_generations: Arc<GenerationRegistry>,
    cancel_receiver: Option<watch::Receiver<bool>>,
    preserve_partial: bool,
}

struct ActiveGenerationGuard {
    generation_id: GenerationId,
    active_generations: Arc<GenerationRegistry>,
}

impl GenerationLaunchPermit {
    #[allow(clippy::too_many_arguments)]
    fn into_task(
        mut self,
        storage: Arc<Storage>,
        event_bus: broadcast::Sender<ChatEvent>,
        branch_id: ConversationBranchId,
        request: GenerationRequest,
        assistant: Message,
        provider: Arc<dyn Provider>,
        credential: GenerationCredential,
        transforms: GenerationTransformContext,
    ) -> CoreResult<GenerationTask> {
        self.active_generations.activate(&self.generation_id)?;
        let cancel_receiver = self
            .cancel_receiver
            .take()
            .expect("generation launch permit can be consumed only once");
        Ok(GenerationTask {
            storage,
            active_generations: Arc::clone(&self.active_generations),
            event_bus,
            branch_id,
            request,
            assistant,
            provider,
            credential,
            cancel_receiver,
            preserve_partial: self.preserve_partial,
            transforms,
        })
    }
}

impl Drop for GenerationLaunchPermit {
    fn drop(&mut self) {
        if self.cancel_receiver.is_some() {
            self.active_generations.remove(&self.generation_id);
        }
    }
}

impl Drop for ActiveGenerationGuard {
    fn drop(&mut self) {
        self.active_generations.remove(&self.generation_id);
    }
}

impl RuntimeControl {
    fn start() -> CoreResult<Self> {
        let (ready_sender, ready_receiver) =
            std::sync::mpsc::sync_channel::<Result<Handle, String>>(1);
        let (shutdown_sender, shutdown_receiver) = tokio::sync::oneshot::channel();
        let owner_thread = std::thread::Builder::new()
            .name("lorepia-core-owner".to_owned())
            .spawn(move || {
                let runtime = match Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .thread_name("lorepia-core-worker")
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = ready_sender
                            .send(Err(format!("cannot create core async runtime: {error}")));
                        return;
                    }
                };
                if ready_sender.send(Ok(runtime.handle().clone())).is_err() {
                    runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
                    return;
                }
                let _ = runtime.block_on(shutdown_receiver);
                runtime.shutdown_timeout(RUNTIME_SHUTDOWN_TIMEOUT);
            })
            .map_err(|error| {
                CoreError::internal(format!("cannot start core runtime owner: {error}"))
            })?;

        match ready_receiver.recv() {
            Ok(Ok(handle)) => Ok(Self {
                handle,
                shutdown_sender: Some(shutdown_sender),
                owner_thread: Some(owner_thread),
            }),
            Ok(Err(message)) => {
                let _ = owner_thread.join();
                Err(CoreError::internal(message))
            }
            Err(error) => {
                let _ = owner_thread.join();
                Err(CoreError::internal(format!(
                    "core runtime owner stopped during startup: {error}"
                )))
            }
        }
    }

    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        std::mem::drop(self.handle.spawn(future));
    }

    fn shutdown(&mut self) {
        if let Some(sender) = self.shutdown_sender.take() {
            let _ = sender.send(());
        }
        if let Some(owner_thread) = self.owner_thread.take() {
            let _ = owner_thread.join();
        }
    }
}

impl GenerationRegistry {
    fn register(
        &self,
        generation: &GenerationRecord,
        provider_admission_key: GenerationProviderAdmissionKey,
        cancel: watch::Sender<bool>,
    ) -> CoreResult<()> {
        let assistant_message_id = generation.assistant_message_id.clone().ok_or_else(|| {
            CoreError::internal("running generation is missing its assistant message route")
        })?;
        let mut active = self
            .active
            .lock()
            .map_err(|_| CoreError::internal("generation registry lock was poisoned"))?;
        if active.contains_key(&generation.id) {
            return Err(CoreError::internal(
                "generation is already registered for delivery",
            ));
        }
        if active.len() >= MAX_ACTIVE_GENERATIONS_PER_PROCESS {
            return Err(generation_admission_limit_reached("process"));
        }
        if active
            .values()
            .filter(|entry| entry.route.conversation == generation.conversation_id)
            .count()
            >= MAX_ACTIVE_GENERATIONS_PER_CONVERSATION
        {
            return Err(generation_admission_limit_reached("conversation"));
        }
        if active
            .values()
            .filter(|entry| entry.provider_admission_key == provider_admission_key)
            .count()
            >= MAX_ACTIVE_GENERATIONS_PER_PROVIDER
        {
            return Err(generation_admission_limit_reached("provider"));
        }
        active.insert(
            generation.id.clone(),
            Arc::new(ActiveGeneration {
                cancel,
                route: GenerationRoute {
                    conversation: generation.conversation_id.clone(),
                    branch: generation.branch_id.clone(),
                    assistant_message: assistant_message_id,
                },
                provider_admission_key,
                delivery: Mutex::new(GenerationDeliveryState {
                    phase: GenerationDeliveryPhase::Preparing,
                    sequence_watermark: 0,
                    live_prefix: Some(GenerationLivePrefix::default()),
                }),
                #[cfg(test)]
                subscription_pause: Mutex::new(None),
            }),
        );
        Ok(())
    }

    fn activate(&self, generation_id: &GenerationId) -> CoreResult<()> {
        let entry = self.entry(generation_id)?;
        let mut delivery = entry
            .delivery
            .lock()
            .map_err(|_| CoreError::internal("generation delivery lock was poisoned"))?;
        if delivery.phase != GenerationDeliveryPhase::Preparing {
            return Err(CoreError::internal(
                "generation delivery phase cannot be activated",
            ));
        }
        delivery.phase = GenerationDeliveryPhase::Running;
        Ok(())
    }

    fn entry(&self, generation_id: &GenerationId) -> CoreResult<Arc<ActiveGeneration>> {
        self.active
            .lock()
            .map_err(|_| CoreError::internal("generation registry lock was poisoned"))?
            .get(generation_id)
            .cloned()
            .ok_or_else(generation_subscription_unavailable)
    }

    fn publish(
        &self,
        event_bus: &broadcast::Sender<ChatEvent>,
        event: ChatEvent,
    ) -> CoreResult<()> {
        let entry = self.entry(&event.generation_id)?;
        let mut delivery = entry
            .delivery
            .lock()
            .map_err(|_| CoreError::internal("generation delivery lock was poisoned"))?;
        if delivery.phase != GenerationDeliveryPhase::Running {
            return Err(CoreError::internal(
                "generation event was published outside the running phase",
            ));
        }
        if event.conversation_id != entry.route.conversation
            || event.branch_id.as_ref() != Some(&entry.route.branch)
            || event.assistant_message_id.as_ref() != Some(&entry.route.assistant_message)
        {
            return Err(CoreError::internal(
                "generation event route does not match the registered route",
            ));
        }
        if event.sequence <= delivery.sequence_watermark {
            return Err(CoreError::internal(
                "generation event sequence is not strictly increasing",
            ));
        }
        let is_terminal = matches!(
            &event.kind,
            ChatEventKind::GenerationCancelled
                | ChatEventKind::GenerationFailed { .. }
                | ChatEventKind::GenerationFinished
        );
        if delivery
            .live_prefix
            .as_mut()
            .is_some_and(|prefix| !prefix.append(&event.kind))
        {
            // The normal provider stream is already bounded by these same
            // cumulative output limits. A larger post-commit display
            // projection may still be delivered to an existing receiver, but
            // cannot be used as a process-local reattachment snapshot.
            delivery.live_prefix = None;
        }
        let sequence = event.sequence;
        let _ = event_bus.send(event);
        delivery.sequence_watermark = sequence;
        if is_terminal {
            delivery.phase = GenerationDeliveryPhase::Terminal;
        }
        Ok(())
    }

    fn cancel(&self, generation_id: &GenerationId) -> CoreResult<()> {
        let entry = self.entry(generation_id)?;
        entry.cancel.send(true).map_err(|_| {
            CoreError::new(CoreErrorCode::Cancelled, "generation already stopped", true)
        })
    }

    fn remove(&self, generation_id: &GenerationId) {
        if let Ok(mut active) = self.active.lock() {
            active.remove(generation_id);
            if active.is_empty() {
                self.drained.notify_all();
            }
        }
    }

    fn len(&self) -> usize {
        self.active.lock().map_or(0, |active| active.len())
    }

    fn cancel_all_and_wait(&self, timeout: Duration) {
        let Ok(mut active) = self.active.lock() else {
            return;
        };
        for entry in active.values() {
            let _ = entry.cancel.send(true);
        }
        let deadline = Instant::now() + timeout;
        while !active.is_empty() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match self.drained.wait_timeout(active, remaining) {
                Ok((next, result)) => {
                    active = next;
                    if result.timed_out() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }

    #[cfg(test)]
    fn sequence_watermark_for_test(&self, generation_id: &GenerationId) -> Option<u64> {
        let entry = self.active.lock().ok()?.get(generation_id).cloned()?;
        entry
            .delivery
            .lock()
            .ok()
            .map(|delivery| delivery.sequence_watermark)
    }

    #[cfg(test)]
    fn phase_for_test(&self, generation_id: &GenerationId) -> Option<GenerationDeliveryPhase> {
        let entry = self.active.lock().ok()?.get(generation_id).cloned()?;
        entry.delivery.lock().ok().map(|delivery| delivery.phase)
    }

    #[cfg(test)]
    fn pause_next_subscription_for_test(
        &self,
        generation_id: &GenerationId,
        entered: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) -> CoreResult<()> {
        let entry = self.entry(generation_id)?;
        let mut pause = entry
            .subscription_pause
            .lock()
            .map_err(|_| CoreError::internal("generation subscription test lock was poisoned"))?;
        if pause.is_some() {
            return Err(CoreError::internal(
                "generation subscription test pause is already installed",
            ));
        }
        *pause = Some(GenerationSubscriptionPause { entered, release });
        Ok(())
    }
}

fn generation_subscription_unavailable() -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        "generation subscription is unavailable",
        false,
    )
}

fn generation_admission_limit_reached(scope: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::ProviderRateLimited,
        format!("active generation {scope} concurrency limit reached"),
        true,
    )
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

fn compiled_built_in_default_api_base_path(
    template: &ProviderTemplate,
) -> CoreResult<Option<EndpointPath>> {
    if template.source != TemplateSource::BuiltIn {
        return Ok(None);
    }
    let Some(id) = BuiltInTemplateId::ALL
        .into_iter()
        .find(|id| id.as_str() == template.id.as_str())
    else {
        return Ok(None);
    };
    let compiled = AdapterRegistry::built_in_template(id)?;
    if template != &compiled {
        return Ok(None);
    }
    EndpointPath::parse(id.default_api_base_path())
        .map(Some)
        .map_err(|error| {
            CoreError::internal(format!(
                "compiled provider API base path is invalid: {error}"
            ))
        })
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
        &self.inner.runtime.handle
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
    pub fn get_character_content(
        &self,
        id: &str,
    ) -> CoreResult<lorepia_storage::StoredRevision<CharacterContentV1>> {
        self.inner.storage.get_character_content(id)
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

    pub fn send_message(
        &self,
        conversation_id: &ConversationId,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
    ) -> CoreResult<GenerationId> {
        let profile = self
            .inner
            .storage
            .get_provider_profile(provider_profile_id)?;
        let state = self.inner.storage.get_conversation_state(conversation_id)?;
        let branch = self
            .inner
            .storage
            .get_conversation_branch(&state.active_branch_id)?;
        self.send_message_to_branch_with_provider_profile(
            conversation_id,
            &state.active_branch_id,
            branch.head_message_id.as_ref(),
            state.selected_mode,
            text,
            operation_context,
            &VariableMap::default(),
            &profile,
            credential,
        )
    }

    pub fn send_message_with_target(
        &self,
        conversation_id: &ConversationId,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: Option<String>,
    ) -> CoreResult<GenerationId> {
        let state = self.inner.storage.get_conversation_state(conversation_id)?;
        let branch = self
            .inner
            .storage
            .get_conversation_branch(&state.active_branch_id)?;
        let variable_overrides = VariableMap::default();
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id,
                branch_id: &state.active_branch_id,
                expected_head: branch.head_message_id.as_ref(),
                live_mode: state.selected_mode,
                text,
                operation_context,
                target,
                prompt_preset_id: None,
                variable_overrides: &variable_overrides,
            })?;
        let provider_temporal_context = GenerationProviderTemporalContext {
            operation_target: GenerationActionTargetIdentity::GenerationTarget {
                model_route_id: target.model_route_id.clone(),
                generation_preset_id: target.generation_preset_id.clone(),
            },
            authority: prepared_target.provider_target_authority.clone(),
        };
        let resolved = build_resolved_generation_target(prepared_target.validated)?;
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.send_message_to_branch_with_provider_options_and_contract(
            conversation_id,
            &state.active_branch_id,
            branch.head_message_id.as_ref(),
            prepared_target.mode,
            text,
            operation_context,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            &variable_overrides,
            credential,
            None,
            false,
            resolved.provider,
            Some(&prompt_wire_contract),
            provider_temporal_context,
        )
    }

    /// Resolves the explicit expert preview through the same provider and
    /// prompt preparation path used by a reviewed send.
    ///
    /// This is the only Core read surface that returns prompt bodies to a Rust
    /// caller. Shell and Tauri must replace it with a content-free allowlist
    /// projection before any `WebView` serialization. The provider snapshot is
    /// credential-free by contract and is rejected if it contains
    /// endpoint/header/credential/opaque-state fields or if the complete
    /// preview exceeds 2 MiB. Preparation may persist an isolated
    /// generation-attempt review and approval records so preview and send can
    /// share one temporal snapshot; it never applies those records to live
    /// branch interaction state, effects, messages, or generations.
    pub fn resolve_prompt_preview(
        &self,
        plan_request: &crate::PromptPlanRequest,
        operation_context: GenerationOperationContext<'_>,
    ) -> CoreResult<crate::ExpertPromptPreview> {
        let state = self
            .inner
            .storage
            .get_conversation_state(&plan_request.conversation_id)?;
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id: &plan_request.conversation_id,
                branch_id: &plan_request.branch_id,
                expected_head: plan_request.expected_head.as_ref(),
                live_mode: state.selected_mode,
                text: &plan_request.user_text,
                operation_context,
                target: &plan_request.generation_target,
                prompt_preset_id: plan_request.prompt_preset_id.as_ref(),
                variable_overrides: &plan_request.variable_overrides,
            })?;
        let initial_resolved = build_resolved_generation_target(prepared_target.validated)?;
        let attempt = match self.prepare_reviewed_prompt_generation_attempt(
            plan_request,
            operation_context,
            prepared_target.mode,
            &initial_resolved,
        )? {
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
            SameBranchGenerationAttempt::Existing(_) => {
                return Err(CoreError::invalid(
                    "reviewed generation attempt has already been dispatched",
                ));
            }
        };
        let validated = validate_generation_target_for_attempt(
            self,
            &plan_request.generation_target,
            &attempt.attempt,
        )?;
        let applied_parameters = validated
            .request_plan
            .body_patches()
            .iter()
            .map(|patch| {
                (
                    patch.path().to_owned(),
                    crate::PromptAppliedParameterPreview {
                        field: patch.path().to_owned(),
                        value: patch.value().clone(),
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let resolved = build_resolved_generation_target(validated)?;
        let prepared = self.prepare_prompt_plan_request_with_wire_contract(
            plan_request,
            crate::orchestration::PromptPlanPreparation {
                prompt_wire_contract: Some(&resolved.prompt_wire_contract),
                interaction_state_override: Some(&attempt.interaction_state),
                applied_module_plan_override: attempt.applied_module_plan.as_ref(),
                prompt_selection_authority: attempt
                    .attempt
                    .input
                    .prompt_selection_authority
                    .as_ref(),
                generation_attempt_id: Some(&attempt.attempt.generation_id),
                resolution_time: attempt.attempt.created_at,
                session_seed: reviewed_prompt_session_seed(
                    &attempt.attempt.input.base_request_fingerprint_sha256,
                ),
            },
        )?;
        self.finish_expert_prompt_preview(
            attempt.attempt.generation_id,
            plan_request,
            resolved,
            prepared,
            applied_parameters,
        )
    }

    /// Prepares the redacted preview/explanation path from the same isolated
    /// attempt and exact provider wire contract used by expert preview and
    /// reviewed send.
    pub(crate) fn prepare_reviewed_prompt_plan_for_core(
        &self,
        plan_request: &crate::PromptPlanRequest,
        operation_context: GenerationOperationContext<'_>,
    ) -> CoreResult<crate::orchestration::PreparedGenerationPlan> {
        let state = self
            .inner
            .storage
            .get_conversation_state(&plan_request.conversation_id)?;
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id: &plan_request.conversation_id,
                branch_id: &plan_request.branch_id,
                expected_head: plan_request.expected_head.as_ref(),
                live_mode: state.selected_mode,
                text: &plan_request.user_text,
                operation_context,
                target: &plan_request.generation_target,
                prompt_preset_id: plan_request.prompt_preset_id.as_ref(),
                variable_overrides: &plan_request.variable_overrides,
            })?;
        let initial_resolved = build_resolved_generation_target(prepared_target.validated)?;
        let attempt = match self.prepare_reviewed_prompt_generation_attempt(
            plan_request,
            operation_context,
            prepared_target.mode,
            &initial_resolved,
        )? {
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
            SameBranchGenerationAttempt::Existing(_) => {
                return Err(CoreError::invalid(
                    "reviewed generation attempt has already been dispatched",
                ));
            }
        };
        let resolved = build_resolved_generation_target(validate_generation_target_for_attempt(
            self,
            &plan_request.generation_target,
            &attempt.attempt,
        )?)?;
        self.prepare_prompt_plan_request_with_wire_contract(
            plan_request,
            crate::orchestration::PromptPlanPreparation {
                prompt_wire_contract: Some(&resolved.prompt_wire_contract),
                interaction_state_override: Some(&attempt.interaction_state),
                applied_module_plan_override: attempt.applied_module_plan.as_ref(),
                prompt_selection_authority: attempt
                    .attempt
                    .input
                    .prompt_selection_authority
                    .as_ref(),
                generation_attempt_id: Some(&attempt.attempt.generation_id),
                resolution_time: attempt.attempt.created_at,
                session_seed: reviewed_prompt_session_seed(
                    &attempt.attempt.input.base_request_fingerprint_sha256,
                ),
            },
        )
    }

    /// Async expert preview path used by native hosts.
    ///
    /// Provider-backed memory retrieval is admitted only through the durable
    /// query-embedding state machine owned by `prepare_generation_plan_async`.
    /// The selected generation credential is neither required nor reused for
    /// that auxiliary task; the broker resolves the exact task connection.
    pub async fn resolve_prompt_preview_async(
        &self,
        plan_request: &crate::PromptPlanRequest,
        operation_context: GenerationOperationContext<'_>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<crate::ExpertPromptPreview> {
        let state = self
            .inner
            .storage
            .get_conversation_state(&plan_request.conversation_id)?;
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id: &plan_request.conversation_id,
                branch_id: &plan_request.branch_id,
                expected_head: plan_request.expected_head.as_ref(),
                live_mode: state.selected_mode,
                text: &plan_request.user_text,
                operation_context,
                target: &plan_request.generation_target,
                prompt_preset_id: plan_request.prompt_preset_id.as_ref(),
                variable_overrides: &plan_request.variable_overrides,
            })?;
        let initial_resolved = build_resolved_generation_target(prepared_target.validated)?;
        let attempt = match self.prepare_reviewed_prompt_generation_attempt(
            plan_request,
            operation_context,
            prepared_target.mode,
            &initial_resolved,
        )? {
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
            SameBranchGenerationAttempt::Existing(_) => {
                return Err(CoreError::invalid(
                    "reviewed generation attempt has already been dispatched",
                ));
            }
        };
        let validated = validate_generation_target_for_attempt(
            self,
            &plan_request.generation_target,
            &attempt.attempt,
        )?;
        let applied_parameters = validated
            .request_plan
            .body_patches()
            .iter()
            .map(|patch| {
                (
                    patch.path().to_owned(),
                    crate::PromptAppliedParameterPreview {
                        field: patch.path().to_owned(),
                        value: patch.value().clone(),
                    },
                )
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        let resolved = build_resolved_generation_target(validated)?;
        let prepared = self
            .prepare_prompt_plan_request_with_wire_contract_async(
                plan_request,
                crate::orchestration::AsyncPromptPlanPreparation {
                    prompt_wire_contract: Some(&resolved.prompt_wire_contract),
                    interaction_state_override: Some(&attempt.interaction_state),
                    applied_module_plan_override: attempt.applied_module_plan.as_ref(),
                    prompt_selection_authority: attempt
                        .attempt
                        .input
                        .prompt_selection_authority
                        .as_ref(),
                    generation_attempt_id: Some(&attempt.attempt.generation_id),
                    resolution_time: attempt.attempt.created_at,
                    session_seed: reviewed_prompt_session_seed(
                        &attempt.attempt.input.base_request_fingerprint_sha256,
                    ),
                    credential_broker: task_credential_broker,
                    cancelled,
                },
            )
            .await?;
        self.finish_expert_prompt_preview(
            attempt.attempt.generation_id,
            plan_request,
            resolved,
            prepared,
            applied_parameters,
        )
    }

    fn finish_expert_prompt_preview(
        &self,
        generation_attempt_id: GenerationId,
        plan_request: &crate::PromptPlanRequest,
        resolved: ResolvedGenerationTarget,
        prepared: crate::orchestration::PreparedGenerationPlan,
        mut applied_parameters: std::collections::BTreeMap<
            String,
            crate::PromptAppliedParameterPreview,
        >,
    ) -> CoreResult<crate::ExpertPromptPreview> {
        const MAX_EXPERT_PREVIEW_BYTES: usize = 2 * 1024 * 1024;

        let mut request = prepared.materialized.request.clone();
        // Opaque continuity is intentionally excluded from this expert
        // snapshot. It is never plaintext preview material.
        configure_generation_protocol_request(
            &self.inner.storage,
            &mut request,
            Some(&plan_request.generation_target),
            Some(resolved.api_family),
            false,
        )?;
        if let Some(temperature) = request.temperature {
            applied_parameters.insert(
                "temperature".to_owned(),
                crate::PromptAppliedParameterPreview {
                    field: "temperature".to_owned(),
                    value: serde_json::Value::from(temperature),
                },
            );
        }
        if let Some(tokens) = request.max_output_tokens {
            applied_parameters.insert(
                "max_output_tokens".to_owned(),
                crate::PromptAppliedParameterPreview {
                    field: "max_output_tokens".to_owned(),
                    value: serde_json::Value::from(tokens),
                },
            );
        }
        let provider_request = resolved.provider.snapshot_request(&request)?;
        reject_sensitive_provider_preview_fields(&provider_request)?;
        let resolved_plan = request.resolved_prompt_plan.as_ref().ok_or_else(|| {
            CoreError::internal("expert preview is missing its resolved prompt plan")
        })?;
        let effective_messages = resolved_plan
            .effective_messages
            .iter()
            .map(|message| crate::PromptEffectiveMessageContentPreview {
                sequence: message.sequence,
                block_id: message.block_id.clone(),
                block_kind: message.block_kind,
                requested_role: message.requested_role,
                effective_role: message.effective_role,
                estimated_tokens: message.estimated_tokens,
                source_message_ids: message.source_message_ids.clone(),
                content: message.content.clone(),
            })
            .collect::<Vec<_>>();
        let prompt_diff = resolved_plan
            .effective_messages
            .iter()
            .filter_map(|message| {
                let provider_message = prepared
                    .preview
                    .provider_messages
                    .iter()
                    .find(|candidate| candidate.sequence == message.sequence)?;
                let mut changes = vec![format!(
                    "requested role {:?} resolved to {:?}",
                    message.requested_role, message.effective_role
                )];
                changes.push(format!(
                    "effective role {:?} maps to provider role {:?} at {:?}",
                    message.effective_role, provider_message.wire_role, provider_message.placement
                ));
                Some(crate::PromptDiffEntry {
                    sequence: message.sequence,
                    block_id: message.block_id.clone(),
                    changes,
                })
            })
            .collect();
        let expert = crate::ExpertPromptPreview {
            generation_attempt_id,
            plan: prepared.preview,
            effective_messages,
            provider_request,
            applied_parameters: applied_parameters.into_values().collect(),
            prompt_diff,
        };
        let encoded = serde_json::to_vec(&expert).map_err(|error| {
            CoreError::internal(format!("cannot encode expert prompt preview: {error}"))
        })?;
        if encoded.len() > MAX_EXPERT_PREVIEW_BYTES {
            return Err(CoreError::new(
                CoreErrorCode::UnsupportedContent,
                "expert prompt preview exceeds the 2 MiB response limit",
                false,
            ));
        }
        Ok(expert)
    }

    /// Sends exactly the prompt plan previously reviewed by
    /// [`Core::resolve_prompt_preview`]. The active branch head and the
    /// resolver-owned plan hash are both checked again before any message or
    /// generation row is committed.
    pub fn send_message_with_prompt_plan(
        &self,
        plan_request: &crate::PromptPlanRequest,
        expected_generation_attempt_id: &GenerationId,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<GenerationId> {
        let context = match self.prepare_reviewed_prompt_send(
            plan_request,
            expected_generation_attempt_id,
            credential,
        )? {
            ReviewedPromptSendPreparation::Existing(generation_id) => return Ok(generation_id),
            ReviewedPromptSendPreparation::Ready(context) => *context,
        };
        let mut prepared = self.prepare_prompt_plan_request_with_wire_contract(
            plan_request,
            crate::orchestration::PromptPlanPreparation {
                prompt_wire_contract: Some(&context.resolved.prompt_wire_contract),
                interaction_state_override: Some(&context.attempt.interaction_state),
                applied_module_plan_override: context.attempt.applied_module_plan.as_ref(),
                prompt_selection_authority: context
                    .attempt
                    .attempt
                    .input
                    .prompt_selection_authority
                    .as_ref(),
                generation_attempt_id: Some(&context.attempt.attempt.generation_id),
                resolution_time: context.attempt.attempt.created_at,
                session_seed: reviewed_prompt_session_seed(
                    &context
                        .attempt
                        .attempt
                        .input
                        .base_request_fingerprint_sha256,
                ),
            },
        )?;
        prepared.materialized.request.generation_id = context.attempt.attempt.generation_id.clone();
        self.launch_reviewed_prompt_send(plan_request, context, prepared)
    }

    /// Async reviewed-send path used when prompt resolution may require a
    /// provider-backed semantic query. The exact reviewed hash, generation
    /// attempt, and append contract are identical to the synchronous path.
    pub async fn send_message_with_prompt_plan_async(
        &self,
        plan_request: &crate::PromptPlanRequest,
        expected_generation_attempt_id: &GenerationId,
        credential: ConnectionBoundCredential,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        let context = match self.prepare_reviewed_prompt_send(
            plan_request,
            expected_generation_attempt_id,
            credential,
        )? {
            ReviewedPromptSendPreparation::Existing(generation_id) => return Ok(generation_id),
            ReviewedPromptSendPreparation::Ready(context) => *context,
        };
        let mut prepared = self
            .prepare_prompt_plan_request_with_wire_contract_async(
                plan_request,
                crate::orchestration::AsyncPromptPlanPreparation {
                    prompt_wire_contract: Some(&context.resolved.prompt_wire_contract),
                    interaction_state_override: Some(&context.attempt.interaction_state),
                    applied_module_plan_override: context.attempt.applied_module_plan.as_ref(),
                    prompt_selection_authority: context
                        .attempt
                        .attempt
                        .input
                        .prompt_selection_authority
                        .as_ref(),
                    generation_attempt_id: Some(&context.attempt.attempt.generation_id),
                    resolution_time: context.attempt.attempt.created_at,
                    session_seed: reviewed_prompt_session_seed(
                        &context
                            .attempt
                            .attempt
                            .input
                            .base_request_fingerprint_sha256,
                    ),
                    credential_broker: task_credential_broker,
                    cancelled,
                },
            )
            .await?;
        prepared.materialized.request.generation_id = context.attempt.attempt.generation_id.clone();
        self.launch_reviewed_prompt_send(plan_request, context, prepared)
    }

    fn prepare_reviewed_prompt_send(
        &self,
        plan_request: &crate::PromptPlanRequest,
        expected_generation_attempt_id: &GenerationId,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<ReviewedPromptSendPreparation> {
        let expected_plan_hash = plan_request
            .expected_plan_hash
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                CoreError::invalid("sending a reviewed prompt requires expected_plan_hash")
            })?;
        let sealed_attempt = match self
            .inner
            .storage
            .get_generation_attempt(expected_generation_attempt_id)
        {
            Ok(attempt) => attempt,
            Err(error) if error.code == CoreErrorCode::NotFound => {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "generation resume attempt is unavailable; start a new generation operation",
                    true,
                ));
            }
            Err(error) => return Err(error),
        };
        let text = validate_user_message_text(&plan_request.user_text)?;
        let operation_target = GenerationActionTargetIdentity::GenerationTarget {
            model_route_id: plan_request.generation_target.model_route_id.clone(),
            generation_preset_id: plan_request.generation_target.generation_preset_id.clone(),
        };
        self.resolve_same_branch_generation_operation_identity(
            SameBranchGenerationAttemptIdentity {
                conversation_id: &plan_request.conversation_id,
                branch_id: &plan_request.branch_id,
                expected_head: plan_request.expected_head.as_ref(),
                text,
                operation_context: GenerationOperationContext::Resume {
                    generation_attempt_id: expected_generation_attempt_id,
                },
                target: &operation_target,
                temperature: None,
                max_output_tokens: None,
                prompt_preset_id: plan_request.prompt_preset_id.as_ref(),
                variable_overrides: &plan_request.variable_overrides,
            },
        )?;
        let sealed_prompt_authority = generation_attempt_prompt_authority(&sealed_attempt)?;
        let mode = sealed_prompt_authority.mode;
        let validated = validate_generation_target_for_attempt(
            self,
            &plan_request.generation_target,
            &sealed_attempt,
        )?;
        validate_connection_credential_binding(&validated.connection, &credential)?;
        let resolved = build_resolved_generation_target(validated)?;
        let credential_authority = credential.access_authority().cloned();
        let credential: GenerationCredential = credential.into();
        let mut user_message = Message::user_after(
            plan_request.conversation_id.clone(),
            plan_request.expected_head.clone(),
            text,
        );
        user_message.id = deterministic_prompt_user_message_id(
            &plan_request.conversation_id,
            &plan_request.branch_id,
            plan_request.expected_head.as_ref(),
            text,
        );
        let attempt = match self.prepare_reviewed_prompt_generation_attempt(
            plan_request,
            GenerationOperationContext::Resume {
                generation_attempt_id: expected_generation_attempt_id,
            },
            mode,
            &resolved,
        )? {
            SameBranchGenerationAttempt::Existing(generation_id) => {
                let existing = self.validate_existing_reviewed_generation(
                    generation_id,
                    expected_generation_attempt_id,
                    expected_plan_hash,
                )?;
                return Ok(ReviewedPromptSendPreparation::Existing(existing));
            }
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
        };
        validate_reviewed_generation_attempt_id(
            expected_generation_attempt_id,
            &attempt.attempt.generation_id,
        )?;
        Ok(ReviewedPromptSendPreparation::Ready(Box::new(
            ReviewedPromptSendContext {
                mode,
                resolved,
                credential,
                credential_authority,
                user_message,
                attempt,
            },
        )))
    }

    fn launch_reviewed_prompt_send(
        &self,
        plan_request: &crate::PromptPlanRequest,
        context: ReviewedPromptSendContext,
        prepared: crate::orchestration::PreparedGenerationPlan,
    ) -> CoreResult<GenerationId> {
        let ReviewedPromptSendContext {
            mode,
            resolved,
            credential,
            credential_authority,
            user_message,
            attempt,
        } = context;
        let mut request = prepared.materialized.request.clone();
        let preserve_opaque_reasoning_state = resolved.preserve_opaque_reasoning_state
            && credential.as_deref().is_none_or(str::is_empty);
        configure_generation_protocol_request(
            &self.inner.storage,
            &mut request,
            Some(&plan_request.generation_target),
            Some(resolved.api_family),
            preserve_opaque_reasoning_state,
        )?;
        let provider_request_value = resolved.provider.snapshot_request(&request)?;
        let generation_id = request.generation_id.clone();
        let mut assistant_message = Message::pending_assistant(
            plan_request.conversation_id.clone(),
            user_message.id.clone(),
            generation_id.clone(),
        );
        assistant_message.created_at = attempt.attempt.created_at;
        let generation = reviewed_prompt_generation_record(
            plan_request,
            mode,
            &resolved,
            &generation_id,
            &user_message,
            &assistant_message,
        );
        let prompt_plan = prepared.generation_prompt_plan_record(
            generation_id.clone(),
            plan_request.conversation_id.clone(),
            plan_request.branch_id.clone(),
            plan_request.expected_head.clone(),
            user_message.id.clone(),
            Some(&plan_request.generation_target),
            provider_request_value,
            assistant_message.created_at,
        )?;
        let provider_admission_key = self.generation_provider_admission_key_for_model_route(
            &plan_request.generation_target.model_route_id,
        )?;
        let launch = self.prepare_generation_launch(&generation, provider_admission_key)?;
        self.seal_same_branch_generation_attempt(attempt.attempt, &prepared, &prompt_plan)?;
        self.inner
            .storage
            .append_generation_attempt_with_prompt_plan(
                &plan_request.branch_id,
                plan_request.expected_head.as_ref(),
                &user_message,
                &assistant_message,
                &generation,
                &prompt_plan,
                &prepared.knowledge_logs,
                credential_authority.as_ref(),
                true,
            )?;
        let transforms = GenerationTransformContext::from(prepared);
        self.start_generation_task(
            launch,
            plan_request.branch_id.clone(),
            request,
            assistant_message,
            resolved.provider,
            credential,
            transforms,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_message_to_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
    ) -> CoreResult<GenerationId> {
        self.send_message_to_branch_with_variables(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            &VariableMap::default(),
            provider_profile_id,
            credential,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_message_to_branch_with_variables(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        variable_overrides: &VariableMap,
        provider_profile_id: &str,
        credential: Option<String>,
    ) -> CoreResult<GenerationId> {
        let profile = self
            .inner
            .storage
            .get_provider_profile(provider_profile_id)?;
        self.send_message_to_branch_with_provider_profile(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            variable_overrides,
            &profile,
            credential,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_message_to_branch_with_target(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: Option<String>,
    ) -> CoreResult<GenerationId> {
        let variable_overrides = VariableMap::default();
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id,
                branch_id,
                expected_head,
                live_mode: mode,
                text,
                operation_context,
                target,
                prompt_preset_id: None,
                variable_overrides: &variable_overrides,
            })?;
        let provider_temporal_context = GenerationProviderTemporalContext {
            operation_target: GenerationActionTargetIdentity::GenerationTarget {
                model_route_id: target.model_route_id.clone(),
                generation_preset_id: target.generation_preset_id.clone(),
            },
            authority: prepared_target.provider_target_authority.clone(),
        };
        let resolved = build_resolved_generation_target(prepared_target.validated)?;
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.send_message_to_branch_with_provider_options_and_contract(
            conversation_id,
            branch_id,
            expected_head,
            prepared_target.mode,
            text,
            operation_context,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            &variable_overrides,
            credential,
            None,
            false,
            resolved.provider,
            Some(&prompt_wire_contract),
            provider_temporal_context,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_message_to_branch_with_connection_credential(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<GenerationId> {
        self.send_message_to_branch_with_connection_credential_and_variables(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            &VariableMap::default(),
            target,
            credential,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_message_to_branch_with_connection_credential_and_variables(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        variable_overrides: &VariableMap,
        target: &GenerationTarget,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<GenerationId> {
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id,
                branch_id,
                expected_head,
                live_mode: mode,
                text,
                operation_context,
                target,
                prompt_preset_id: None,
                variable_overrides,
            })?;
        let provider_temporal_context = GenerationProviderTemporalContext {
            operation_target: GenerationActionTargetIdentity::GenerationTarget {
                model_route_id: target.model_route_id.clone(),
                generation_preset_id: target.generation_preset_id.clone(),
            },
            authority: prepared_target.provider_target_authority.clone(),
        };
        validate_connection_credential_binding(&prepared_target.validated.connection, &credential)?;
        let resolved = build_resolved_generation_target(prepared_target.validated)?;
        let credential_authority = credential.access_authority().cloned();
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.send_message_to_branch_with_provider_options_and_contract(
            conversation_id,
            branch_id,
            expected_head,
            prepared_target.mode,
            text,
            operation_context,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            variable_overrides,
            credential,
            credential_authority,
            true,
            resolved.provider,
            Some(&prompt_wire_contract),
            provider_temporal_context,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_to_branch_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        self.send_message_to_branch_async_with_variables(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            &VariableMap::default(),
            provider_profile_id,
            credential,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_to_branch_async_with_variables(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        variable_overrides: &VariableMap,
        provider_profile_id: &str,
        credential: Option<String>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        self.send_message_to_branch_async_inner(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            variable_overrides,
            provider_profile_id,
            credential,
            None,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_to_branch_async_with_credential_admission_lease(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
        admission_lease: GenerationCredentialAdmissionLease,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        self.send_message_to_branch_async_with_credential_admission_lease_and_variables(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            &VariableMap::default(),
            provider_profile_id,
            credential,
            admission_lease,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_to_branch_async_with_credential_admission_lease_and_variables(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        variable_overrides: &VariableMap,
        provider_profile_id: &str,
        credential: Option<String>,
        admission_lease: GenerationCredentialAdmissionLease,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        self.send_message_to_branch_async_inner(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            variable_overrides,
            provider_profile_id,
            credential,
            Some(admission_lease),
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_message_to_branch_async_inner(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        variable_overrides: &VariableMap,
        provider_profile_id: &str,
        credential: Option<String>,
        admission_lease: Option<GenerationCredentialAdmissionLease>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        let profile = self
            .inner
            .storage
            .get_provider_profile(provider_profile_id)?;
        let provider_temporal_context = provider_profile_temporal_context(&profile)?;
        self.preflight_same_branch_provider_authority(
            SameBranchGenerationAttemptIdentity {
                conversation_id,
                branch_id,
                expected_head,
                text,
                operation_context,
                target: &provider_temporal_context.operation_target,
                temperature: Some(1.0),
                max_output_tokens: Some(CORE_MAX_OUTPUT_TOKENS),
                prompt_preset_id: None,
                variable_overrides,
            },
            &provider_temporal_context.authority,
        )?;
        let provider = Arc::new(OpenAiCompatibleProvider::new(
            &profile.base_url,
            Duration::from_secs(u64::from(profile.timeout_seconds.max(1))),
        )?);
        self.send_message_to_branch_with_provider_options_and_contract_async(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            profile.model,
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            variable_overrides,
            credential,
            None,
            false,
            admission_lease,
            provider,
            None,
            provider_temporal_context,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_to_branch_with_target_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: Option<String>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        let variable_overrides = VariableMap::default();
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id,
                branch_id,
                expected_head,
                live_mode: mode,
                text,
                operation_context,
                target,
                prompt_preset_id: None,
                variable_overrides: &variable_overrides,
            })?;
        let provider_temporal_context = GenerationProviderTemporalContext {
            operation_target: GenerationActionTargetIdentity::GenerationTarget {
                model_route_id: target.model_route_id.clone(),
                generation_preset_id: target.generation_preset_id.clone(),
            },
            authority: prepared_target.provider_target_authority.clone(),
        };
        let resolved = build_resolved_generation_target(prepared_target.validated)?;
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.send_message_to_branch_with_provider_options_and_contract_async(
            conversation_id,
            branch_id,
            expected_head,
            prepared_target.mode,
            text,
            operation_context,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            &variable_overrides,
            credential,
            None,
            false,
            None,
            resolved.provider,
            Some(&prompt_wire_contract),
            provider_temporal_context,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_to_branch_with_connection_credential_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: ConnectionBoundCredential,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        self.send_message_to_branch_with_connection_credential_and_variables_async(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            &VariableMap::default(),
            target,
            credential,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn send_message_to_branch_with_connection_credential_and_variables_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        variable_overrides: &VariableMap,
        target: &GenerationTarget,
        credential: ConnectionBoundCredential,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        let prepared_target =
            self.prepare_same_branch_generation_target(SameBranchGenerationTargetInput {
                conversation_id,
                branch_id,
                expected_head,
                live_mode: mode,
                text,
                operation_context,
                target,
                prompt_preset_id: None,
                variable_overrides,
            })?;
        let provider_temporal_context = GenerationProviderTemporalContext {
            operation_target: GenerationActionTargetIdentity::GenerationTarget {
                model_route_id: target.model_route_id.clone(),
                generation_preset_id: target.generation_preset_id.clone(),
            },
            authority: prepared_target.provider_target_authority.clone(),
        };
        validate_connection_credential_binding(&prepared_target.validated.connection, &credential)?;
        let resolved = build_resolved_generation_target(prepared_target.validated)?;
        let credential_authority = credential.access_authority().cloned();
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.send_message_to_branch_with_provider_options_and_contract_async(
            conversation_id,
            branch_id,
            expected_head,
            prepared_target.mode,
            text,
            operation_context,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            variable_overrides,
            credential,
            credential_authority,
            true,
            None,
            resolved.provider,
            Some(&prompt_wire_contract),
            provider_temporal_context,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub fn edit_user_message(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
    ) -> CoreResult<MessageActionGeneration> {
        let profile = self
            .inner
            .storage
            .get_provider_profile(provider_profile_id)?;
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::EditUser,
                replacement_text: Some(replacement_text),
                operation_context,
                target: GenerationActionTargetIdentity::ProviderProfile {
                    provider_profile_id: provider_profile_id.to_owned(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let provider_temporal_context = provider_profile_temporal_context(&profile)?;
        self.preflight_message_action_provider_authority(
            &action_request,
            &provider_temporal_context.authority,
        )?;
        let provider = Arc::new(OpenAiCompatibleProvider::new(
            &profile.base_url,
            Duration::from_secs(u64::from(profile.timeout_seconds.max(1))),
        )?);
        self.start_message_generation_action_with_provider_options_and_contract(
            action_request,
            profile.model,
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            credential,
            None,
            false,
            provider,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_user_message_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        self.edit_user_message_async_inner(
            conversation_id,
            branch_id,
            expected_head,
            message_id,
            replacement_text,
            operation_context,
            provider_profile_id,
            credential,
            None,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_user_message_async_with_credential_admission_lease(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
        admission_lease: GenerationCredentialAdmissionLease,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        self.edit_user_message_async_inner(
            conversation_id,
            branch_id,
            expected_head,
            message_id,
            replacement_text,
            operation_context,
            provider_profile_id,
            credential,
            Some(admission_lease),
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn edit_user_message_async_inner(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
        admission_lease: Option<GenerationCredentialAdmissionLease>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        let profile = self
            .inner
            .storage
            .get_provider_profile(provider_profile_id)?;
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::EditUser,
                replacement_text: Some(replacement_text),
                operation_context,
                target: GenerationActionTargetIdentity::ProviderProfile {
                    provider_profile_id: provider_profile_id.to_owned(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let provider_temporal_context = provider_profile_temporal_context(&profile)?;
        self.preflight_message_action_provider_authority(
            &action_request,
            &provider_temporal_context.authority,
        )?;
        let provider = Arc::new(OpenAiCompatibleProvider::new(
            &profile.base_url,
            Duration::from_secs(u64::from(profile.timeout_seconds.max(1))),
        )?);
        self.start_message_generation_action_with_provider_options_and_contract_async(
            action_request,
            profile.model,
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            credential,
            None,
            false,
            admission_lease,
            provider,
            None,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub fn edit_user_message_with_target(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: Option<String>,
    ) -> CoreResult<MessageActionGeneration> {
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::EditUser,
                replacement_text: Some(replacement_text),
                operation_context,
                target: GenerationActionTargetIdentity::GenerationTarget {
                    model_route_id: target.model_route_id.clone(),
                    generation_preset_id: target.generation_preset_id.clone(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let reasoning_effort = self.prompt_reasoning_effort_for_message_action(&action_request)?;
        let validated = self.validate_message_action_generation_target(
            &action_request,
            target,
            reasoning_effort,
        )?;
        let resolved = build_resolved_generation_target(validated)?;
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.start_message_generation_action_with_provider_options_and_contract(
            action_request,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            credential,
            None,
            false,
            resolved.provider,
            Some(&prompt_wire_contract),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_user_message_with_target_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: Option<String>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::EditUser,
                replacement_text: Some(replacement_text),
                operation_context,
                target: GenerationActionTargetIdentity::GenerationTarget {
                    model_route_id: target.model_route_id.clone(),
                    generation_preset_id: target.generation_preset_id.clone(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let reasoning_effort = self.prompt_reasoning_effort_for_message_action(&action_request)?;
        let validated = self.validate_message_action_generation_target(
            &action_request,
            target,
            reasoning_effort,
        )?;
        let resolved = build_resolved_generation_target(validated)?;
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.start_message_generation_action_with_provider_options_and_contract_async(
            action_request,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            credential,
            None,
            false,
            None,
            resolved.provider,
            Some(&prompt_wire_contract),
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub fn edit_user_message_with_connection_credential(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<MessageActionGeneration> {
        preflight_generation_target_connection_credential(self, target, &credential)?;
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::EditUser,
                replacement_text: Some(replacement_text),
                operation_context,
                target: GenerationActionTargetIdentity::GenerationTarget {
                    model_route_id: target.model_route_id.clone(),
                    generation_preset_id: target.generation_preset_id.clone(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let reasoning_effort = self.prompt_reasoning_effort_for_message_action(&action_request)?;
        let validated = self.validate_message_action_generation_target(
            &action_request,
            target,
            reasoning_effort,
        )?;
        validate_connection_credential_binding(&validated.connection, &credential)?;
        let resolved = build_resolved_generation_target(validated)?;
        let credential_authority = credential.access_authority().cloned();
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.start_message_generation_action_with_provider_options_and_contract(
            action_request,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            credential,
            credential_authority,
            true,
            resolved.provider,
            Some(&prompt_wire_contract),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn edit_user_message_with_connection_credential_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: ConnectionBoundCredential,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        preflight_generation_target_connection_credential(self, target, &credential)?;
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::EditUser,
                replacement_text: Some(replacement_text),
                operation_context,
                target: GenerationActionTargetIdentity::GenerationTarget {
                    model_route_id: target.model_route_id.clone(),
                    generation_preset_id: target.generation_preset_id.clone(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let reasoning_effort = self.prompt_reasoning_effort_for_message_action(&action_request)?;
        let validated = self.validate_message_action_generation_target(
            &action_request,
            target,
            reasoning_effort,
        )?;
        validate_connection_credential_binding(&validated.connection, &credential)?;
        let resolved = build_resolved_generation_target(validated)?;
        let credential_authority = credential.access_authority().cloned();
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.start_message_generation_action_with_provider_options_and_contract_async(
            action_request,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            credential,
            credential_authority,
            true,
            None,
            resolved.provider,
            Some(&prompt_wire_contract),
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub fn regenerate_assistant_message(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
    ) -> CoreResult<MessageActionGeneration> {
        let profile = self
            .inner
            .storage
            .get_provider_profile(provider_profile_id)?;
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::RegenerateAssistant,
                replacement_text: None,
                operation_context,
                target: GenerationActionTargetIdentity::ProviderProfile {
                    provider_profile_id: provider_profile_id.to_owned(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let provider_temporal_context = provider_profile_temporal_context(&profile)?;
        self.preflight_message_action_provider_authority(
            &action_request,
            &provider_temporal_context.authority,
        )?;
        let provider = Arc::new(OpenAiCompatibleProvider::new(
            &profile.base_url,
            Duration::from_secs(u64::from(profile.timeout_seconds.max(1))),
        )?);
        self.start_message_generation_action_with_provider_options_and_contract(
            action_request,
            profile.model,
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            credential,
            None,
            false,
            provider,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn regenerate_assistant_message_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        self.regenerate_assistant_message_async_inner(
            conversation_id,
            branch_id,
            expected_head,
            message_id,
            operation_context,
            provider_profile_id,
            credential,
            None,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn regenerate_assistant_message_async_with_credential_admission_lease(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
        admission_lease: GenerationCredentialAdmissionLease,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        self.regenerate_assistant_message_async_inner(
            conversation_id,
            branch_id,
            expected_head,
            message_id,
            operation_context,
            provider_profile_id,
            credential,
            Some(admission_lease),
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn regenerate_assistant_message_async_inner(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        operation_context: GenerationOperationContext<'_>,
        provider_profile_id: &str,
        credential: Option<String>,
        admission_lease: Option<GenerationCredentialAdmissionLease>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        let profile = self
            .inner
            .storage
            .get_provider_profile(provider_profile_id)?;
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::RegenerateAssistant,
                replacement_text: None,
                operation_context,
                target: GenerationActionTargetIdentity::ProviderProfile {
                    provider_profile_id: provider_profile_id.to_owned(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let provider_temporal_context = provider_profile_temporal_context(&profile)?;
        self.preflight_message_action_provider_authority(
            &action_request,
            &provider_temporal_context.authority,
        )?;
        let provider = Arc::new(OpenAiCompatibleProvider::new(
            &profile.base_url,
            Duration::from_secs(u64::from(profile.timeout_seconds.max(1))),
        )?);
        self.start_message_generation_action_with_provider_options_and_contract_async(
            action_request,
            profile.model,
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            credential,
            None,
            false,
            admission_lease,
            provider,
            None,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub fn regenerate_assistant_message_with_target(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: Option<String>,
    ) -> CoreResult<MessageActionGeneration> {
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::RegenerateAssistant,
                replacement_text: None,
                operation_context,
                target: GenerationActionTargetIdentity::GenerationTarget {
                    model_route_id: target.model_route_id.clone(),
                    generation_preset_id: target.generation_preset_id.clone(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let reasoning_effort = self.prompt_reasoning_effort_for_message_action(&action_request)?;
        let validated = self.validate_message_action_generation_target(
            &action_request,
            target,
            reasoning_effort,
        )?;
        let resolved = build_resolved_generation_target(validated)?;
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.start_message_generation_action_with_provider_options_and_contract(
            action_request,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            credential,
            None,
            false,
            resolved.provider,
            Some(&prompt_wire_contract),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn regenerate_assistant_message_with_target_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: Option<String>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::RegenerateAssistant,
                replacement_text: None,
                operation_context,
                target: GenerationActionTargetIdentity::GenerationTarget {
                    model_route_id: target.model_route_id.clone(),
                    generation_preset_id: target.generation_preset_id.clone(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let reasoning_effort = self.prompt_reasoning_effort_for_message_action(&action_request)?;
        let validated = self.validate_message_action_generation_target(
            &action_request,
            target,
            reasoning_effort,
        )?;
        let resolved = build_resolved_generation_target(validated)?;
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.start_message_generation_action_with_provider_options_and_contract_async(
            action_request,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            credential,
            None,
            false,
            None,
            resolved.provider,
            Some(&prompt_wire_contract),
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub fn regenerate_assistant_message_with_connection_credential(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: ConnectionBoundCredential,
    ) -> CoreResult<MessageActionGeneration> {
        preflight_generation_target_connection_credential(self, target, &credential)?;
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::RegenerateAssistant,
                replacement_text: None,
                operation_context,
                target: GenerationActionTargetIdentity::GenerationTarget {
                    model_route_id: target.model_route_id.clone(),
                    generation_preset_id: target.generation_preset_id.clone(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let reasoning_effort = self.prompt_reasoning_effort_for_message_action(&action_request)?;
        let validated = self.validate_message_action_generation_target(
            &action_request,
            target,
            reasoning_effort,
        )?;
        validate_connection_credential_binding(&validated.connection, &credential)?;
        let resolved = build_resolved_generation_target(validated)?;
        let credential_authority = credential.access_authority().cloned();
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.start_message_generation_action_with_provider_options_and_contract(
            action_request,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            credential,
            credential_authority,
            true,
            resolved.provider,
            Some(&prompt_wire_contract),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn regenerate_assistant_message_with_connection_credential_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        operation_context: GenerationOperationContext<'_>,
        target: &GenerationTarget,
        credential: ConnectionBoundCredential,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        preflight_generation_target_connection_credential(self, target, &credential)?;
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action: MessageGenerationAction::RegenerateAssistant,
                replacement_text: None,
                operation_context,
                target: GenerationActionTargetIdentity::GenerationTarget {
                    model_route_id: target.model_route_id.clone(),
                    generation_preset_id: target.generation_preset_id.clone(),
                },
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let reasoning_effort = self.prompt_reasoning_effort_for_message_action(&action_request)?;
        let validated = self.validate_message_action_generation_target(
            &action_request,
            target,
            reasoning_effort,
        )?;
        validate_connection_credential_binding(&validated.connection, &credential)?;
        let resolved = build_resolved_generation_target(validated)?;
        let credential_authority = credential.access_authority().cloned();
        let prompt_wire_contract = resolved.prompt_wire_contract.clone();
        self.start_message_generation_action_with_provider_options_and_contract_async(
            action_request,
            resolved.model,
            Some(target),
            Some(resolved.api_family),
            resolved.preserve_opaque_reasoning_state,
            None,
            None,
            credential,
            credential_authority,
            true,
            None,
            resolved.provider,
            Some(&prompt_wire_contract),
            task_credential_broker,
            cancelled,
        )
        .await
    }

    fn prepare_message_generation_action_identity(
        &self,
        input: MessageGenerationActionIdentityInput<'_>,
    ) -> CoreResult<PreparedMessageGenerationAction> {
        let MessageGenerationActionIdentityInput {
            conversation_id,
            source_branch_id,
            expected_source_head_message_id,
            target_message_id,
            action,
            replacement_text,
            operation_context,
            target,
        } = input;
        let replacement_text = validate_action_replacement(action, replacement_text)?;
        let context = self
            .inner
            .storage
            .load_message_generation_action_identity_context(
                conversation_id,
                source_branch_id,
                target_message_id,
                action,
            )?;
        let text = replacement_text.map_or_else(
            || validate_user_message_text(&context.user_text).map(str::to_owned),
            |text| Ok(text.to_owned()),
        )?;
        let mode = self
            .inner
            .storage
            .get_conversation_state(conversation_id)?
            .selected_mode;
        let replacement_text_sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
        let semantic_base_fingerprint_sha256 = Sha256Digest::parse(canonical_value_sha256(
            &GenerationActionSemanticSnapshot {
                schema_version: 1,
                action: generation_action_name(action),
                conversation_id,
                source_branch_id,
                expected_source_head_message_id,
                target_message_id,
                context_head_message_id: context.fork_message_id.as_ref(),
                replacement_text_sha256: &replacement_text_sha256,
                target: &target,
            },
            "generation action semantic request",
        )?)
        .map_err(CoreError::invalid)?;
        let (operation_id, resume_generation_attempt_id) = match operation_context {
            GenerationOperationContext::New { operation_nonce } => (
                new_generation_operation_id(
                    "generation-action-v5",
                    &semantic_base_fingerprint_sha256,
                    operation_nonce,
                )?,
                None,
            ),
            GenerationOperationContext::Resume {
                generation_attempt_id,
            } => {
                let attempt = self
                    .inner
                    .storage
                    .get_generation_attempt(generation_attempt_id)?;
                (
                    attempt.input.operation_id,
                    Some(generation_attempt_id.clone()),
                )
            }
        };
        let proposed_branch_id = deterministic_proposed_branch_id(
            &operation_id,
            conversation_id,
            source_branch_id,
            context.fork_message_id.as_ref(),
        )?;
        let mode = match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(conversation_id, &operation_id)
        {
            Ok(attempt) => generation_attempt_prompt_authority(&attempt)?.mode,
            Err(error) if error.code == CoreErrorCode::NotFound => mode,
            Err(error) => return Err(error),
        };
        let prepared = PreparedMessageGenerationAction {
            conversation_id: conversation_id.clone(),
            source_branch_id: source_branch_id.clone(),
            expected_source_head_message_id: expected_source_head_message_id.cloned(),
            target_message_id: target_message_id.clone(),
            action,
            context,
            text,
            target,
            semantic_base_fingerprint_sha256,
            operation_id,
            resume_generation_attempt_id,
            proposed_branch_id,
            mode,
        };
        self.validate_message_generation_action_identity(&prepared)?;
        Ok(prepared)
    }

    fn validate_message_generation_action_identity(
        &self,
        prepared: &PreparedMessageGenerationAction,
    ) -> CoreResult<()> {
        match self.inner.storage.get_generation_attempt_by_operation_id(
            &prepared.conversation_id,
            &prepared.operation_id,
        ) {
            Ok(attempt) => {
                let mismatched = prepared
                    .resume_generation_attempt_id
                    .as_ref()
                    .is_some_and(|generation_id| generation_id != &attempt.generation_id)
                    || attempt.input.conversation_id != prepared.conversation_id
                    || attempt.input.source_branch_id != prepared.source_branch_id
                    || attempt.input.proposed_branch_id != prepared.proposed_branch_id
                    || attempt.input.expected_head_message_id
                        != prepared.expected_source_head_message_id
                    || attempt.input.context_head_message_id != prepared.context.fork_message_id
                    || attempt.input.base_request_fingerprint_sha256
                        != prepared.semantic_base_fingerprint_sha256
                    || attempt.input.prompt_selection_authority.is_none();
                if mismatched {
                    return if prepared.resume_generation_attempt_id.is_some() {
                        Err(CoreError::new(
                            CoreErrorCode::InvalidInput,
                            "generation resume attempt does not match the caller-owned action; start a new generation operation",
                            true,
                        ))
                    } else {
                        Err(CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "stored generation action attempt differs from its immutable request",
                            false,
                        ))
                    };
                }
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {
                if prepared.resume_generation_attempt_id.is_some() {
                    return Err(CoreError::new(
                        CoreErrorCode::InvalidInput,
                        "generation resume attempt does not belong to this action; start a new generation operation",
                        true,
                    ));
                }
            }
            Err(error) => return Err(error),
        }
        // A completed append moves the active branch away from the immutable
        // source branch. Resolve an exact durable operation before rechecking
        // that live branch snapshot so a response-loss retry can replay after
        // restart without relaunching its provider.
        if self.existing_message_action_generation(prepared)?.is_some() {
            return Ok(());
        }
        let validated_context = match self.inner.storage.prepare_message_generation_action(
            &prepared.conversation_id,
            &prepared.source_branch_id,
            prepared.expected_source_head_message_id.as_ref(),
            &prepared.target_message_id,
            prepared.action,
        ) {
            Ok(context) => context,
            Err(error) => {
                // Close the narrow race where another caller atomically
                // materialized this exact operation after the first lookup.
                if self.existing_message_action_generation(prepared)?.is_some() {
                    return Ok(());
                }
                return Err(error);
            }
        };
        if validated_context != prepared.context {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "message action identity changed during live snapshot validation",
                false,
            ));
        }
        Ok(())
    }

    fn preflight_message_action_provider_authority(
        &self,
        action: &PreparedMessageGenerationAction,
        provider_target_authority: &GenerationProviderTargetAuthority,
    ) -> CoreResult<()> {
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(attempt) => {
                require_generation_provider_target_authority(&attempt, provider_target_authority)
            }
            Err(error)
                if error.code == CoreErrorCode::NotFound
                    && action.resume_generation_attempt_id.is_none() =>
            {
                Ok(())
            }
            Err(error) if error.code == CoreErrorCode::NotFound => Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation resume attempt is unavailable; start a new generation operation",
                true,
            )),
            Err(error) => Err(error),
        }
    }

    fn validate_message_action_generation_target(
        &self,
        action: &PreparedMessageGenerationAction,
        target: &GenerationTarget,
        requested_reasoning_effort: Option<GenerationReasoningEffort>,
    ) -> CoreResult<ValidatedGenerationTarget> {
        let validated = validate_generation_target_plan_with_reasoning_effort(
            self,
            target,
            requested_reasoning_effort,
        )?;
        let provider_target_authority = generation_target_provider_authority(target, &validated)?;
        self.preflight_message_action_provider_authority(action, &provider_target_authority)?;
        Ok(validated)
    }

    fn prepare_message_generation_attempt(
        &self,
        action: &PreparedMessageGenerationAction,
        configuration: MessageGenerationAttemptConfiguration<'_>,
        module_runtime_review: &lorepia_orchestration::ModuleMergeReview,
        applied_module_plan: Option<&lorepia_orchestration::AppliedModuleRuntimePlan>,
        prepared_at: DateTime<Utc>,
    ) -> CoreResult<lorepia_storage::StoredGenerationAttempt> {
        let applied_module_plan_sha256 = applied_module_plan.map_or_else(
            lorepia_orchestration::no_applied_module_runtime_plan_sha256,
            |plan| plan.applied_plan_sha256.clone(),
        );
        let base_request_fingerprint_sha256 = action.semantic_base_fingerprint_sha256.clone();
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(existing) => {
                require_generation_provider_target_authority(
                    &existing,
                    configuration.provider_target_authority,
                )?;
                if existing.input.source_branch_id != action.source_branch_id
                    || existing.input.proposed_branch_id != action.proposed_branch_id
                    || existing.input.expected_head_message_id
                        != action.expected_source_head_message_id
                    || existing.input.context_head_message_id != action.context.fork_message_id
                    || existing.input.module_plan_sha256 != applied_module_plan_sha256
                    || existing.input.base_request_fingerprint_sha256
                        != base_request_fingerprint_sha256
                    || existing.input.prompt_selection_authority.is_none()
                    || existing.input.module_runtime_review_authority.as_ref()
                        != Some(module_runtime_review)
                    || existing.input.applied_runtime_plan_authority.as_ref() != applied_module_plan
                {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "stored generation action attempt differs from its immutable request",
                        false,
                    ));
                }
                if configuration.require_exact_credential_authority {
                    return self
                        .inner
                        .storage
                        .prepare_generation_attempt_with_credential_authority(
                            &existing.input,
                            existing.created_at,
                            configuration.credential_authority,
                        );
                }
                return Ok(existing);
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        let conversation = self
            .inner
            .storage
            .get_conversation(&action.conversation_id)?;
        let character = self
            .inner
            .storage
            .get_character(&conversation.character_id)?;
        let prompt_selection_authority =
            self.capture_generation_prompt_selection_authority(GenerationPromptAuthorityCapture {
                character: &character,
                conversation_id: &action.conversation_id,
                branch_id: &action.source_branch_id,
                mode: action.mode,
                explicit_preset_id: None,
                generation_target: configuration.generation_target,
                temperature: configuration.temperature,
                max_output_tokens: configuration.max_output_tokens,
                prompt_wire_contract: configuration.prompt_wire_contract,
                provider_target_authority: configuration.provider_target_authority.clone(),
            })?;
        let input = lorepia_storage::GenerationAttemptInput {
            operation_id: action.operation_id.clone(),
            conversation_id: action.conversation_id.clone(),
            source_branch_id: action.source_branch_id.clone(),
            proposed_branch_id: action.proposed_branch_id.clone(),
            expected_head_message_id: action.expected_source_head_message_id.clone(),
            context_head_message_id: action.context.fork_message_id.clone(),
            module_plan_sha256: applied_module_plan_sha256,
            base_request_fingerprint_sha256,
            prompt_selection_authority: Some(prompt_selection_authority),
            module_runtime_review_authority: Some(module_runtime_review.clone()),
            applied_runtime_plan_authority: applied_module_plan.cloned(),
        };
        if configuration.require_exact_credential_authority {
            self.inner
                .storage
                .prepare_generation_attempt_with_credential_authority(
                    &input,
                    prepared_at,
                    configuration.credential_authority,
                )
        } else {
            self.inner
                .storage
                .prepare_generation_attempt(&input, prepared_at)
        }
    }

    fn existing_message_action_generation(
        &self,
        action: &PreparedMessageGenerationAction,
    ) -> CoreResult<Option<MessageActionGeneration>> {
        let attempt = match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(attempt) => attempt,
            Err(error) if error.code == CoreErrorCode::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if !matches!(
            attempt.status,
            lorepia_storage::GenerationAttemptStatus::Running
                | lorepia_storage::GenerationAttemptStatus::Completed
        ) {
            return Ok(None);
        }
        if attempt.input.source_branch_id != action.source_branch_id
            || attempt.input.proposed_branch_id != action.proposed_branch_id
            || attempt.input.context_head_message_id != action.context.fork_message_id
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored generation action identity differs from its canonical operation",
                false,
            ));
        }
        let branch = self
            .inner
            .storage
            .get_conversation_branch(&attempt.input.proposed_branch_id)?;
        if branch.conversation_id != action.conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored generation action branch belongs to another conversation",
                false,
            ));
        }
        Ok(Some(MessageActionGeneration {
            branch,
            generation_id: attempt.generation_id,
        }))
    }

    fn prepare_message_action_attempt(
        &self,
        action: &PreparedMessageGenerationAction,
        configuration: MessageGenerationAttemptConfiguration<'_>,
    ) -> CoreResult<MessageActionAttempt> {
        self.ensure_interaction_state_available(&action.conversation_id, &action.source_branch_id)?;
        let existing_attempt = match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(existing) => Some(existing),
            Err(error) if error.code == CoreErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        let (module_runtime_review, mut applied_module_plan) =
            if let Some(existing) = existing_attempt.as_ref() {
                let (review, plan) = generation_attempt_module_authority(existing)?;
                (review.clone(), plan.cloned())
            } else {
                self.preview_module_runtime_authority_for_proposed_branch(
                    &action.conversation_id,
                    &action.proposed_branch_id,
                )?
            };
        let mut attempt = self.prepare_message_generation_attempt(
            action,
            configuration,
            &module_runtime_review,
            applied_module_plan.as_ref(),
            Utc::now(),
        )?;

        if attempt.status != lorepia_storage::GenerationAttemptStatus::Prepared {
            let before = self
                .inner
                .storage
                .get_generation_attempt_before_review(&attempt.generation_id)?
                .ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "generation action attempt is missing its immutable review",
                        false,
                    )
                })?;
            applied_module_plan = before.applied_runtime_plan;
        }

        if attempt.status == lorepia_storage::GenerationAttemptStatus::Prepared {
            let boundary = self
                .inner
                .storage
                .get_generation_attempt_interaction_boundary(&attempt.generation_id)?;
            let review = self.prepare_generation_attempt_before_review(
                &attempt,
                &boundary.state,
                &boundary.context_checkpoint_sha256,
                &module_runtime_review,
                applied_module_plan.as_ref(),
                attempt.created_at,
            )?;
            self.inner
                .storage
                .commit_generation_attempt_before_review(&review)?;
            attempt = self
                .inner
                .storage
                .get_generation_attempt(&attempt.generation_id)?;
        }

        self.finish_prepared_message_action_attempt(action, attempt, applied_module_plan)
    }

    fn finish_prepared_message_action_attempt(
        &self,
        action: &PreparedMessageGenerationAction,
        attempt: lorepia_storage::StoredGenerationAttempt,
        applied_module_plan: Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
    ) -> CoreResult<MessageActionAttempt> {
        match attempt.status {
            lorepia_storage::GenerationAttemptStatus::BeforeGenerationApplied
            | lorepia_storage::GenerationAttemptStatus::DispatchReady => {}
            lorepia_storage::GenerationAttemptStatus::AwaitingApproval => {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "generation is waiting for an interaction approval",
                    true,
                ));
            }
            lorepia_storage::GenerationAttemptStatus::FailedBeforeDispatch => {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "generation attempt requires an explicit pre-dispatch retry",
                    true,
                ));
            }
            lorepia_storage::GenerationAttemptStatus::Prepared => {
                return Err(CoreError::new(
                    CoreErrorCode::StorageUnavailable,
                    "generation attempt remained unreviewed",
                    true,
                ));
            }
            lorepia_storage::GenerationAttemptStatus::Running
            | lorepia_storage::GenerationAttemptStatus::Completed => {
                return self
                    .existing_message_action_generation(action)?
                    .map(MessageActionAttempt::Existing)
                    .ok_or_else(|| {
                        CoreError::new(
                            CoreErrorCode::StorageCorrupted,
                            "generation action attempt is terminal without its durable branch",
                            false,
                        )
                    });
            }
        }

        let boundary = self
            .inner
            .storage
            .get_generation_attempt_interaction_boundary(&attempt.generation_id)?;
        let aggregate = self
            .inner
            .storage
            .get_generation_attempt_interaction_aggregate(&attempt.generation_id)?;
        if aggregate.pending_proposal_count != 0 {
            return Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "generation is waiting for an interaction approval",
                true,
            ));
        }
        let interaction_state = lorepia_storage::StoredInteractionState {
            key: boundary.state.key,
            state: aggregate.state,
            knowledge: aggregate.knowledge,
        };
        Ok(MessageActionAttempt::Ready(Box::new(
            PreparedMessageActionAttempt {
                attempt,
                interaction_state,
                target_interaction_state_key: crate::orchestration_runtime::interaction_state_key(
                    &action.conversation_id,
                    &action.proposed_branch_id,
                )?,
                applied_module_plan,
            },
        )))
    }

    fn prompt_reasoning_effort_for_message_action(
        &self,
        action: &PreparedMessageGenerationAction,
    ) -> CoreResult<Option<GenerationReasoningEffort>> {
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(&action.conversation_id, &action.operation_id)
        {
            Ok(attempt) => {
                return Ok(generation_attempt_prompt_authority(&attempt)?
                    .quick_settings
                    .reasoning_effort);
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        let state = self
            .inner
            .storage
            .get_conversation_state(&action.conversation_id)?;
        // Edit/regenerate creates a new branch. Resolve against an unbound
        // branch identity so the same conversation/character/user/app scope
        // precedence used by the eventual new branch determines the provider
        // overlay without inheriting a source-branch-only binding.
        self.prompt_reasoning_effort_for_context(
            &action.conversation_id,
            &action.proposed_branch_id,
            state.selected_mode,
            None,
        )
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

    pub fn cancel_generation(&self, generation_id: &GenerationId) -> CoreResult<()> {
        self.inner.active_generations.cancel(generation_id)
    }

    /// Atomically validates a live generation route and subscribes at its
    /// authoritative event watermark.
    pub fn subscribe_generation_events(
        &self,
        generation_id: &GenerationId,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<GenerationEventSubscription> {
        let entry = self.inner.active_generations.entry(generation_id)?;
        let delivery = entry
            .delivery
            .lock()
            .map_err(|_| CoreError::internal("generation delivery lock was poisoned"))?;
        if delivery.phase == GenerationDeliveryPhase::Terminal
            || entry.route.conversation != *conversation_id
            || entry.route.branch != *branch_id
        {
            return Err(generation_subscription_unavailable());
        }

        let generation = self.inner.storage.get_generation(generation_id)?;
        if generation.status != GenerationStatus::Running
            || generation.conversation_id != *conversation_id
            || generation.branch_id != *branch_id
            || generation.assistant_message_id.as_ref() != Some(&entry.route.assistant_message)
        {
            return Err(generation_subscription_unavailable());
        }

        #[cfg(test)]
        if let Some(pause) = entry
            .subscription_pause
            .lock()
            .map_err(|_| CoreError::internal("generation subscription test lock was poisoned"))?
            .take()
        {
            pause
                .entered
                .send(())
                .map_err(|_| CoreError::internal("generation subscription test did not start"))?;
            pause
                .release
                .recv_timeout(Duration::from_secs(2))
                .map_err(|_| CoreError::internal("generation subscription test timed out"))?;
        }

        let receiver = self.inner.event_bus.subscribe();
        let live_prefix = delivery.live_prefix.as_ref().ok_or_else(|| {
            CoreError::internal("live generation catch-up prefix exceeded its bounded contract")
        })?;
        Ok(GenerationEventSubscription {
            receiver,
            assistant_message_id: entry.route.assistant_message.clone(),
            sequence_watermark: delivery.sequence_watermark,
            display_prefix: live_prefix.display.clone(),
            reasoning_prefix: live_prefix.reasoning.clone(),
        })
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

    pub fn list_provider_templates(&self) -> CoreResult<Vec<ProviderTemplate>> {
        let active_catalog = self.operational_provider_catalog_projection_at(Utc::now())?;
        let active_templates = active_catalog.provider_templates();
        let active_ids = active_templates
            .iter()
            .map(|template| template.id.clone())
            .collect::<HashSet<_>>();
        let mut by_id = self
            .inner
            .storage
            .list_provider_templates()?
            .into_iter()
            // Signed template rows are retained only to keep already-created
            // connections pinned. Visibility is controlled by the atomic
            // active catalog pointer, never by these inert support rows.
            .filter(|template| {
                template.source != TemplateSource::SignedCatalog
                    && !active_ids.contains(&template.id)
            })
            .fold(HashMap::new(), |mut latest, template| {
                latest
                    .entry(template.id.clone())
                    .and_modify(|current: &mut ProviderTemplate| {
                        if template.manifest_version > current.manifest_version {
                            *current = template.clone();
                        }
                    })
                    .or_insert(template);
                latest
            });
        for template in active_templates {
            validate_provider_template(&template)?;
            by_id.insert(template.id.clone(), template);
        }
        let mut templates = by_id.into_values().collect::<Vec<_>>();
        templates.sort_by(|left, right| {
            left.display_name
                .to_lowercase()
                .cmp(&right.display_name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
                .then_with(|| right.manifest_version.cmp(&left.manifest_version))
        });
        Ok(templates)
    }

    /// Lists provider templates together with Rust-owned presentation defaults.
    pub fn list_provider_template_views(&self) -> CoreResult<Vec<ProviderTemplateView>> {
        self.list_provider_templates()?
            .into_iter()
            .map(|template| {
                validate_provider_template(&template)?;
                let descriptor = AdapterRegistry::descriptor(template.api_family)?;
                Ok(ProviderTemplateView {
                    template,
                    default_network_mode: descriptor.default_network_mode,
                })
            })
            .collect()
    }

    pub fn list_provider_connections(&self) -> CoreResult<Vec<ProviderConnection>> {
        self.inner.storage.list_provider_connections()
    }

    #[allow(
        clippy::too_many_lines,
        reason = "connection creation is one fail-closed validation and persistence boundary"
    )]
    pub fn create_provider_connection(
        &self,
        mut draft: ProviderConnectionDraft,
    ) -> CoreResult<ProviderConnection> {
        match self.inner.storage.get_provider_connection(&draft.id) {
            Ok(_) => {
                return Err(CoreError::invalid(
                    "provider connection identifier already exists; create a new connection identifier",
                ));
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        let (network_policy, local_network_approval) = match (
            draft.network_mode,
            draft.local_network_approval.as_ref(),
        ) {
            (ProviderNetworkMode::Public, None) => (UrlPolicy::public(), None),
            (ProviderNetworkMode::LocalLoopback, None) => (UrlPolicy::local_loopback(), None),
            (ProviderNetworkMode::ApprovedLocalNetwork, Some(approval)) => {
                if approval.origin != draft.api_origin {
                    return Err(CoreError::invalid(
                        "local-network approval origin must exactly match the provider API origin",
                    ));
                }
                let approval =
                    ApprovedLocalNetworkOrigin::new(approval.origin.as_str(), &approval.addresses)
                        .map_err(|error| {
                            CoreError::invalid(format!(
                                "provider local-network approval is invalid: {error}"
                            ))
                        })?;
                let normalized = ProviderLocalNetworkApproval {
                    origin: draft.api_origin.clone(),
                    addresses: approval.addresses().to_vec(),
                };
                (
                    UrlPolicy::approved_local_network(approval),
                    Some(normalized),
                )
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
        };
        let policy_url = network_policy
            .canonicalize(&format!(
                "{}/",
                draft.api_origin.as_str().trim_end_matches('/')
            ))
            .map_err(|error| {
                CoreError::invalid(format!("provider API origin is not allowed: {error}"))
            })?;
        if policy_url.origin().as_string() != draft.api_origin.as_str() {
            return Err(CoreError::invalid(
                "provider API origin is not in canonical form",
            ));
        }
        draft.local_network_approval = local_network_approval;
        let active_catalog = self.operational_provider_catalog_projection_at(Utc::now())?;
        let expected_catalog_state_version = active_catalog.state_version;
        let template = if let Some(template) =
            active_catalog.provider_template(&draft.template_id, draft.template_version)
        {
            template
        } else {
            let template = self
                .inner
                .storage
                .get_provider_template(&draft.template_id, draft.template_version)?;
            if template.source == TemplateSource::SignedCatalog {
                return Err(CoreError::new(
                    CoreErrorCode::NotFound,
                    "provider template is not active in the signed catalog",
                    false,
                ));
            }
            template
        };
        validate_provider_template(&template)?;
        if draft.api_base_path.is_none() {
            draft.api_base_path = compiled_built_in_default_api_base_path(&template)?;
        }
        let credential_scope = match &template.default_manifest.auth {
            AuthBinding::None => {
                if draft.approved_credential_origin.is_some() {
                    return Err(CoreError::invalid(
                        "credential-free provider must not declare a credential origin",
                    ));
                }
                None
            }
            auth_binding => {
                let approved_origin =
                    draft.approved_credential_origin.as_ref().ok_or_else(|| {
                        CoreError::invalid(
                            "credential origin approval is required before saving this connection",
                        )
                    })?;
                if approved_origin != &draft.api_origin {
                    return Err(CoreError::invalid(
                        "approved credential origin must exactly match the provider API origin",
                    ));
                }
                Some(CredentialScope {
                    allowed_origins: vec![approved_origin.clone()],
                    auth_binding: auth_binding.clone(),
                    redirect_policy: CredentialRedirectPolicy::Deny,
                })
            }
        };
        let now = Utc::now();
        let connection = ProviderConnection {
            credential_ref: credential_scope
                .as_ref()
                .map(|_| CredentialRef(draft.id.as_str().to_owned())),
            credential_scope,
            id: draft.id,
            template_id: draft.template_id,
            template_version: draft.template_version,
            display_name: draft.display_name,
            api_origin: draft.api_origin,
            config: ConnectionConfig {
                api_base_path: draft.api_base_path,
                network_mode: draft.network_mode,
                local_network_approval: draft.local_network_approval,
                values: draft.values,
            },
            timeout_seconds: draft.timeout_seconds,
            status: ConnectionStatus::Untested,
            created_at: now,
            updated_at: now,
        };
        if template.source == TemplateSource::SignedCatalog {
            self.inner
                .storage
                .insert_provider_connection_for_catalog_state(
                    &connection,
                    &template,
                    expected_catalog_state_version,
                )?;
        } else {
            self.inner.storage.insert_provider_connection(&connection)?;
        }
        Ok(connection)
    }

    pub fn upsert_provider_connection(
        &self,
        connection: ProviderConnection,
    ) -> CoreResult<ProviderConnection> {
        let template = self
            .inner
            .storage
            .get_provider_template(&connection.template_id, connection.template_version)?;
        validate_provider_template(&template)?;
        let current = self.inner.storage.get_provider_connection(&connection.id)?;
        if connection.template_id != current.template_id
            || connection.template_version != current.template_version
            || connection.api_origin != current.api_origin
            || connection.config != current.config
            || connection.credential_ref != current.credential_ref
            || connection.credential_scope != current.credential_scope
        {
            return Err(CoreError::invalid(
                "provider template, endpoint configuration, network approval, and credential binding are immutable; create a newly approved connection instead",
            ));
        }
        let updated = ProviderConnection {
            display_name: connection.display_name,
            timeout_seconds: connection.timeout_seconds,
            updated_at: Utc::now(),
            ..current
        };
        self.inner.storage.save_provider_connection(&updated)?;
        Ok(updated)
    }

    pub fn delete_provider_connection(&self, id: &ProviderConnectionId) -> CoreResult<()> {
        self.inner.storage.delete_provider_connection(id)
    }

    pub fn list_model_routes(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<Vec<ModelRoute>> {
        self.inner.storage.list_model_routes(connection_id)
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

    pub fn upsert_model_route(&self, mut route: ModelRoute) -> CoreResult<ModelRoute> {
        if self
            .retained_legacy_profile_for_connection(&route.connection_id)?
            .is_some()
        {
            return Err(CoreError::invalid(
                "migrated legacy model routes are managed through their retained provider profile",
            ));
        }
        match self.inner.storage.get_model_route(&route.id) {
            Ok(existing) => {
                if route.connection_id != existing.connection_id
                    || route.api_family != existing.api_family
                    || route.model_id != existing.model_id
                    || route.route_config != existing.route_config
                    || route.first_seen_at != existing.first_seen_at
                {
                    return Err(CoreError::invalid(
                        "an existing model route cannot be rebound to another provider, model, or route discriminator",
                    ));
                }
                // Refresh/catalog provenance is owned by trusted Rust
                // ingestion paths. A native edit may change only the
                // user-facing label and availability.
                route.miss_count = existing.miss_count;
                route.raw_metadata = existing.raw_metadata;
                route.metadata_source = existing.metadata_source;
                route.metadata_observed_at = existing.metadata_observed_at;
                route.last_reconciled_sync_job_id = existing.last_reconciled_sync_job_id;
                route.metadata_sync_job_id = existing.metadata_sync_job_id;
                route.last_seen_at = existing.last_seen_at;
            }
            Err(error) if error.code == CoreErrorCode::NotFound => {
                let connection = self
                    .inner
                    .storage
                    .get_provider_connection(&route.connection_id)?;
                let template = self
                    .inner
                    .storage
                    .get_provider_template(&connection.template_id, connection.template_version)?;
                if route.api_family != template.api_family {
                    return Err(CoreError::invalid(
                        "model route API family does not match its provider template",
                    ));
                }
                if route.miss_count != 0
                    || route.raw_metadata.is_some()
                    || !matches!(
                        route.metadata_source,
                        ModelMetadataSource::Legacy | ModelMetadataSource::UserOverride
                    )
                    || route.metadata_observed_at.is_some()
                    || route.last_reconciled_sync_job_id.is_some()
                    || route.metadata_sync_job_id.is_some()
                {
                    return Err(CoreError::invalid(
                        "a native-created model route cannot claim provider, catalog, probe, or synchronization provenance",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
        self.inner.storage.save_model_route(&route)?;
        Ok(route)
    }

    pub fn delete_model_route(&self, id: &ModelRouteId) -> CoreResult<()> {
        let route = self.inner.storage.get_model_route(id)?;
        if self.is_current_migrated_legacy_route(&route)? {
            return Err(CoreError::invalid(
                "the migrated legacy profile's current model route cannot be deleted independently",
            ));
        }
        self.inner.storage.delete_model_route(id)
    }

    pub fn upsert_capability_observation(
        &self,
        observation: CapabilityObservation,
    ) -> CoreResult<CapabilityObservation> {
        if observation.source == ObservationSource::SignedLorepiaCatalog {
            return Err(CoreError::invalid(
                "signed catalog observations are derived from the active verified catalog and cannot be stored independently",
            ));
        }
        let route = self
            .inner
            .storage
            .get_model_route(&observation.model_route_id)?;
        let connection = self
            .inner
            .storage
            .get_provider_connection(&route.connection_id)?;
        let template = self
            .inner
            .storage
            .get_provider_template(&connection.template_id, connection.template_version)?;
        validate_capability_wire_metadata(&route, &template, &observation)?;
        self.inner
            .storage
            .upsert_capability_observation(&observation)?;
        Ok(observation)
    }

    /// Stores a capability override explicitly authored by the local user.
    ///
    /// Provider API, signed catalog, probe, documentation, and assistant
    /// observations have dedicated trusted ingestion paths and cannot be
    /// impersonated through a native binding.
    pub fn upsert_user_capability_override(
        &self,
        mut observation: CapabilityObservation,
    ) -> CoreResult<CapabilityObservation> {
        if observation.source != ObservationSource::UserOverride {
            return Err(CoreError::invalid(
                "the user override API only accepts user_override observations",
            ));
        }
        if matches!(observation.value, CapabilityValue::Structured(_)) {
            return Err(CoreError::invalid(
                "structured provider wire metadata cannot be authored as a user override",
            ));
        }
        if !matches!(
            observation.status,
            SupportStatus::Verified
                | SupportStatus::Unsupported
                | SupportStatus::Unknown
                | SupportStatus::Conditional
        ) {
            return Err(CoreError::invalid(
                "user override status must be verified, unsupported, unknown, or conditional",
            ));
        }
        observation.confidence = Confidence::High;
        observation.observed_at = Utc::now();
        observation.evidence_ref = None;
        if observation
            .expires_at
            .is_some_and(|expires_at| expires_at <= observation.observed_at)
        {
            return Err(CoreError::invalid(
                "a user capability override expiry must be in the future",
            ));
        }
        self.upsert_capability_observation(observation)
    }

    pub fn list_capability_observations(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        let now = Utc::now();
        let route = self.inner.storage.get_model_route(model_route_id)?;
        let catalog = self.catalog_route_projection_at(&route, now)?;
        let mut observations = self
            .inner
            .storage
            .list_capability_observations(model_route_id)?
            .into_iter()
            .filter(|observation| observation.source != ObservationSource::SignedLorepiaCatalog)
            .map(|observation| (observation.id.clone(), observation))
            .collect::<HashMap<_, _>>();
        for observation in catalog.capability_observations {
            observations.insert(observation.id.clone(), observation);
        }
        let mut observations = observations.into_values().collect::<Vec<_>>();
        observations.sort_by(|left, right| {
            capability_key_identity(left.key)
                .cmp(capability_key_identity(right.key))
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(observations)
    }

    pub fn delete_capability_observation(&self, id: &ObservationId) -> CoreResult<()> {
        self.inner.storage.delete_capability_observation(id)
    }

    pub fn delete_user_capability_override(
        &self,
        model_route_id: &ModelRouteId,
        id: &ObservationId,
    ) -> CoreResult<()> {
        let observation = self
            .inner
            .storage
            .list_capability_observations(model_route_id)?
            .into_iter()
            .find(|observation| observation.id == *id)
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "capability observation was not found",
                    false,
                )
            })?;
        if observation.source != ObservationSource::UserOverride {
            return Err(CoreError::invalid(
                "only user_override observations can be deleted through this API",
            ));
        }
        self.inner.storage.delete_capability_observation(id)
    }

    pub fn effective_capability(
        &self,
        model_route_id: &ModelRouteId,
        key: CapabilityKey,
    ) -> CoreResult<Option<EffectiveCapability>> {
        let now = Utc::now();
        let route = self.inner.storage.get_model_route(model_route_id)?;
        let catalog = self.catalog_route_projection_at(&route, now)?;
        effective_capability_at(
            &self.inner.storage,
            &catalog.capability_observations,
            model_route_id,
            key,
            now,
        )
    }

    /// Return the fresh model-specific parameter contract in effect now.
    ///
    /// Signed exact/glob entries override the family fallback by stable
    /// parameter ID. Stale signed mappings are not allowed to alter a request;
    /// expired layers have already been removed from the active projection.
    pub fn effective_parameter_specs(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<lorepia_domain::ParameterSpec>> {
        let now = Utc::now();
        let route = self.inner.storage.get_model_route(model_route_id)?;
        let connection = self
            .inner
            .storage
            .get_provider_connection(&route.connection_id)?;
        let template = self
            .inner
            .storage
            .get_provider_template(&connection.template_id, connection.template_version)?;
        let catalog = self
            .operational_provider_catalog_projection_at(now)?
            .route_projection(&route, &connection.template_id);
        let base = if catalog.matched {
            catalog.parameters
        } else {
            template.default_manifest.parameters.clone()
        };
        effective_route_parameter_specs(&route, &template, &base, &catalog.signed_parameters, now)
    }

    fn catalog_route_projection_at(
        &self,
        route: &ModelRoute,
        now: DateTime<Utc>,
    ) -> CoreResult<CatalogRouteProjection> {
        let connection = self
            .inner
            .storage
            .get_provider_connection(&route.connection_id)?;
        Ok(self
            .operational_provider_catalog_projection_at(now)?
            .route_projection(route, &connection.template_id))
    }

    /// Atomic ingestion point for direct provider model metadata.
    pub fn record_provider_api_capability_observations(
        &self,
        observations: Vec<CapabilityObservation>,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        self.record_capability_observations_from_source(
            observations,
            ObservationSource::ProviderApi,
        )
    }

    /// Atomic ingestion point for one-shot probe results.
    pub fn record_probe_capability_observations(
        &self,
        observations: Vec<CapabilityObservation>,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        self.record_capability_observations_from_source(
            observations,
            ObservationSource::CapabilityProbe,
        )
    }

    fn record_capability_observations_from_source(
        &self,
        observations: Vec<CapabilityObservation>,
        expected_source: ObservationSource,
    ) -> CoreResult<Vec<CapabilityObservation>> {
        let mut routes = HashMap::<ModelRouteId, (ModelRoute, ProviderTemplate)>::new();
        for observation in &observations {
            if observation.source != expected_source {
                return Err(CoreError::invalid(
                    "capability observation source does not match the ingestion path",
                ));
            }
            let (route, template) = if let Some(route) = routes.get(&observation.model_route_id) {
                route
            } else {
                let route = self
                    .inner
                    .storage
                    .get_model_route(&observation.model_route_id)?;
                let connection = self
                    .inner
                    .storage
                    .get_provider_connection(&route.connection_id)?;
                let template = self
                    .inner
                    .storage
                    .get_provider_template(&connection.template_id, connection.template_version)?;
                routes.insert(observation.model_route_id.clone(), (route, template));
                routes
                    .get(&observation.model_route_id)
                    .expect("inserted capability route")
            };
            validate_capability_wire_metadata(route, template, observation)?;
        }
        self.inner
            .storage
            .upsert_capability_observations(&observations)?;
        Ok(observations)
    }

    pub fn list_generation_presets(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<GenerationPreset>> {
        self.inner.storage.list_generation_presets(model_route_id)
    }

    pub fn upsert_generation_preset(
        &self,
        preset: GenerationPreset,
    ) -> CoreResult<GenerationPreset> {
        let route = self.inner.storage.get_model_route(&preset.model_route_id)?;
        if self
            .retained_legacy_profile_for_connection(&route.connection_id)?
            .is_some()
        {
            return Err(CoreError::invalid(
                "migrated legacy generation presets are managed through their retained provider profile",
            ));
        }
        self.validate_generation_preset_candidate(&preset)?;
        self.inner.storage.save_generation_preset(&preset)?;
        Ok(preset)
    }

    pub fn delete_generation_preset(&self, id: &GenerationPresetId) -> CoreResult<()> {
        let preset = self.inner.storage.get_generation_preset(id)?;
        let route = self.inner.storage.get_model_route(&preset.model_route_id)?;
        if preset.id.as_str() == route.id.as_str()
            && self.is_current_migrated_legacy_route(&route)?
        {
            return Err(CoreError::invalid(
                "the migrated legacy profile's current generation preset cannot be deleted independently",
            ));
        }
        self.inner.storage.delete_generation_preset(id)
    }

    /// Validates an unsaved preset candidate against the effective route
    /// catalog and capability dialects. Callers may safely use this before
    /// save; [`Self::upsert_generation_preset`] always applies the same gate.
    pub fn validate_generation_preset_candidate(
        &self,
        preset: &GenerationPreset,
    ) -> CoreResult<()> {
        validate_generation_preset_candidate_plan(self, preset).map(|_| ())
    }

    /// Returns the render-ready, model-specific reasoning controls for a
    /// stored or unsaved preset candidate. Native UI must not reconstruct
    /// these rules from an API-family name.
    pub fn render_reasoning_control_for_preset(
        &self,
        preset: &GenerationPreset,
    ) -> CoreResult<ReasoningControlModel> {
        let context = generation_preset_control_context(self, preset)?;
        let mut reasoning = context.reasoning;
        if context.connection.credential_ref.is_some()
            || !AdapterRegistry::template_supports_opaque_reasoning_state(&context.template)
        {
            reasoning.preserve_opaque_state = false;
        }
        Ok(render_reasoning_control(
            context.route.api_family,
            &context.reasoning_dialect,
            &reasoning,
        ))
    }

    /// Returns the render-ready, model-specific prompt-cache controls for a
    /// stored or unsaved preset candidate.
    pub fn render_prompt_cache_control_for_preset(
        &self,
        preset: &GenerationPreset,
    ) -> CoreResult<PromptCacheControlModel> {
        let context = generation_preset_control_context(self, preset)?;
        Ok(render_prompt_cache_control(
            context.route.api_family,
            context.cache_dialect,
            &context.prompt_cache,
        ))
    }

    /// Previews an unsaved preset through the same validation and adapter
    /// contract used by save and generation.
    pub fn preview_provider_request_candidate(
        &self,
        preset: &GenerationPreset,
    ) -> CoreResult<RequestPreview> {
        let validated = validate_generation_preset_candidate_plan(self, preset)?;
        AdapterRegistry::new().preview_provider_request(
            &validated.template,
            &validated.connection,
            &validated.route,
            Some(&validated.request_plan),
        )
    }

    /// Validates the same stored route/preset pair and family-specific request
    /// plan that generation will use, without constructing a provider or
    /// performing network work.
    pub fn validate_generation_preset(
        &self,
        model_route_id: &ModelRouteId,
        generation_preset_id: &GenerationPresetId,
    ) -> CoreResult<()> {
        validate_generation_target_plan(
            self,
            &GenerationTarget {
                model_route_id: model_route_id.clone(),
                generation_preset_id: generation_preset_id.clone(),
            },
        )
        .map(|_| ())
    }

    /// Returns a scalar-free, credential-free preview produced by the same
    /// family adapter and validated request plan used for generation.
    pub fn preview_provider_request(
        &self,
        model_route_id: &ModelRouteId,
        generation_preset_id: &GenerationPresetId,
    ) -> CoreResult<RequestPreview> {
        let validated = validate_generation_target_plan(
            self,
            &GenerationTarget {
                model_route_id: model_route_id.clone(),
                generation_preset_id: generation_preset_id.clone(),
            },
        )?;
        AdapterRegistry::new().preview_provider_request(
            &validated.template,
            &validated.connection,
            &validated.route,
            Some(&validated.request_plan),
        )
    }

    pub fn select_generation_target(
        &self,
        target: Option<GenerationTarget>,
    ) -> CoreResult<AppSettings> {
        let (selected_provider_profile_id, selected_model_route_id, selected_generation_preset_id) =
            if let Some(target) = target {
                validate_generation_target_plan(self, &target)?;
                let selected_provider_profile_id = match self
                    .classify_migrated_legacy_target(&target)?
                {
                    MigratedLegacyTargetClassification::Ordinary => None,
                    MigratedLegacyTargetClassification::Current { profile_id } => Some(profile_id),
                    MigratedLegacyTargetClassification::Alias => {
                        return Err(CoreError::invalid(
                            "select the retained legacy provider profile instead of a custom target from its migrated connection",
                        ));
                    }
                };
                (
                    selected_provider_profile_id,
                    Some(target.model_route_id),
                    Some(target.generation_preset_id),
                )
            } else {
                (None, None, None)
            };
        self.inner.storage.save_generation_target_selection(
            selected_provider_profile_id,
            selected_model_route_id,
            selected_generation_preset_id,
        )
    }

    fn retained_legacy_profile_for_connection(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<Option<ProviderProfile>> {
        match self
            .inner
            .storage
            .get_provider_profile(connection_id.as_str())
        {
            Ok(profile) => Ok(Some(profile)),
            Err(error) if error.code == CoreErrorCode::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn is_current_migrated_legacy_route(&self, route: &ModelRoute) -> CoreResult<bool> {
        Ok(self
            .retained_legacy_profile_for_connection(&route.connection_id)?
            .is_some_and(|profile| {
                route.api_family == ApiFamily::OpenAiChatCompletions
                    && route.model_id == profile.model
                    && route.route_config == ModelRouteConfig::default()
                    && route.metadata_source == ModelMetadataSource::Legacy
            }))
    }

    fn classify_migrated_legacy_target(
        &self,
        target: &GenerationTarget,
    ) -> CoreResult<MigratedLegacyTargetClassification> {
        let route = self.inner.storage.get_model_route(&target.model_route_id)?;
        let Some(profile) = self.retained_legacy_profile_for_connection(&route.connection_id)?
        else {
            return Ok(MigratedLegacyTargetClassification::Ordinary);
        };
        if route.api_family == ApiFamily::OpenAiChatCompletions
            && route.model_id == profile.model
            && route.route_config == ModelRouteConfig::default()
            && route.metadata_source == ModelMetadataSource::Legacy
            && target.generation_preset_id.as_str() == route.id.as_str()
        {
            return Ok(MigratedLegacyTargetClassification::Current {
                profile_id: profile.id,
            });
        }
        Ok(MigratedLegacyTargetClassification::Alias)
    }

    pub fn list_provider_profiles(&self) -> CoreResult<Vec<ProviderProfile>> {
        self.inner.storage.list_provider_profiles()
    }

    pub fn upsert_provider_profile(
        &self,
        mut profile: ProviderProfile,
    ) -> CoreResult<ProviderProfile> {
        profile.id = normalize_bounded_text(
            "provider profile id",
            std::mem::take(&mut profile.id),
            MAX_PROVIDER_ID_BYTES,
            MAX_PROVIDER_ID_CHARS,
        )?;
        profile.display_name = normalize_bounded_text(
            "provider display name",
            std::mem::take(&mut profile.display_name),
            MAX_PROVIDER_DISPLAY_NAME_BYTES,
            MAX_PROVIDER_DISPLAY_NAME_CHARS,
        )?;
        profile.base_url = normalize_bounded_text(
            "provider base URL",
            std::mem::take(&mut profile.base_url),
            MAX_PROVIDER_BASE_URL_BYTES,
            MAX_PROVIDER_BASE_URL_CHARS,
        )?;
        profile.model = normalize_bounded_text(
            "provider model",
            std::mem::take(&mut profile.model),
            MAX_PROVIDER_MODEL_BYTES,
            MAX_PROVIDER_MODEL_CHARS,
        )?;
        if profile.timeout_seconds == 0 || profile.timeout_seconds > 600 {
            return Err(CoreError::invalid(
                "provider profile requires an id, display name, model, and a timeout from 1 to 600 seconds",
            ));
        }
        OpenAiCompatibleProvider::new(
            &profile.base_url,
            Duration::from_secs(u64::from(profile.timeout_seconds)),
        )?;
        match self.inner.storage.get_provider_profile(&profile.id) {
            Ok(existing) if existing.base_url != profile.base_url => {
                return Err(CoreError::invalid(
                    "provider endpoint configuration is immutable; create a new provider connection instead",
                ));
            }
            Ok(_) => {}
            Err(error) if error.code == CoreErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
        self.inner.storage.save_provider_profile(&profile)?;
        Ok(profile)
    }

    pub fn delete_provider_profile(&self, id: &str) -> CoreResult<()> {
        self.inner.storage.delete_provider_profile(id)
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

    #[cfg(test)]
    fn send_message_with_provider(
        &self,
        conversation_id: &ConversationId,
        text: &str,
        model: String,
        credential: Option<String>,
        provider: Arc<dyn Provider>,
    ) -> CoreResult<GenerationId> {
        let state = self.inner.storage.get_conversation_state(conversation_id)?;
        let branch = self
            .inner
            .storage
            .get_conversation_branch(&state.active_branch_id)?;
        self.send_message_to_branch_with_provider(
            conversation_id,
            &state.active_branch_id,
            branch.head_message_id.as_ref(),
            state.selected_mode,
            text,
            GenerationOperationContext::New {
                operation_nonce: "core-direct-send-v1",
            },
            model,
            credential,
            provider,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn send_message_to_branch_with_provider_profile(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        variable_overrides: &VariableMap,
        profile: &ProviderProfile,
        credential: Option<String>,
    ) -> CoreResult<GenerationId> {
        let provider_temporal_context = provider_profile_temporal_context(profile)?;
        self.preflight_same_branch_provider_authority(
            SameBranchGenerationAttemptIdentity {
                conversation_id,
                branch_id,
                expected_head,
                text,
                operation_context,
                target: &provider_temporal_context.operation_target,
                temperature: Some(1.0),
                max_output_tokens: Some(CORE_MAX_OUTPUT_TOKENS),
                prompt_preset_id: None,
                variable_overrides,
            },
            &provider_temporal_context.authority,
        )?;
        let provider = Arc::new(OpenAiCompatibleProvider::new(
            &profile.base_url,
            Duration::from_secs(u64::from(profile.timeout_seconds.max(1))),
        )?);
        self.send_message_to_branch_with_provider_options_and_contract(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            profile.model.clone(),
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            variable_overrides,
            credential,
            None,
            false,
            provider,
            None,
            provider_temporal_context,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn send_message_to_branch_with_provider(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        model: String,
        credential: Option<String>,
        provider: Arc<dyn Provider>,
    ) -> CoreResult<GenerationId> {
        self.send_message_to_branch_with_provider_options(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            model,
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            credential,
            None,
            false,
            provider,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn send_message_to_branch_with_provider_options(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        model: String,
        generation_target: Option<&GenerationTarget>,
        provider_family: Option<ApiFamily>,
        preserve_opaque_reasoning_state: bool,
        temperature: Option<f64>,
        max_output_tokens: Option<u32>,
        credential: Option<String>,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
        provider: Arc<dyn Provider>,
    ) -> CoreResult<GenerationId> {
        let provider_temporal_context = match generation_target {
            Some(target) => {
                let validated = validate_generation_target_plan(self, target)?;
                generation_target_temporal_context(target, &validated)?
            }
            None => direct_model_temporal_context(&model)?,
        };
        self.send_message_to_branch_with_provider_options_and_contract(
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            model,
            generation_target,
            provider_family,
            preserve_opaque_reasoning_state,
            temperature,
            max_output_tokens,
            &VariableMap::default(),
            credential,
            credential_authority,
            require_exact_credential_authority,
            provider,
            None,
            provider_temporal_context,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn send_message_to_branch_with_provider_options_and_contract(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        model: String,
        generation_target: Option<&GenerationTarget>,
        provider_family: Option<ApiFamily>,
        preserve_opaque_reasoning_state: bool,
        temperature: Option<f64>,
        max_output_tokens: Option<u32>,
        variable_overrides: &VariableMap,
        credential: impl Into<GenerationCredential>,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
        provider: Arc<dyn Provider>,
        prompt_wire_contract: Option<&PromptRouteWireContract>,
        provider_temporal_context: GenerationProviderTemporalContext,
    ) -> CoreResult<GenerationId> {
        let credential = credential.into();
        let prompt_provider_family = provider_family.or_else(|| {
            generation_target
                .is_none()
                .then_some(ApiFamily::OpenAiChatCompletions)
        });
        let text = validate_user_message_text(text)?;
        let conversation = self.inner.storage.get_conversation(conversation_id)?;
        let character = self
            .inner
            .storage
            .get_character(&conversation.character_id)?;
        let branch = self.inner.storage.get_conversation_branch(branch_id)?;
        if branch.conversation_id != *conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        let mut user_message =
            Message::user_after(conversation_id.clone(), expected_head.cloned(), text);
        user_message.id =
            deterministic_prompt_user_message_id(conversation_id, branch_id, expected_head, text);
        let attempt = match self.prepare_same_branch_generation_attempt(
            &character,
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            generation_target,
            temperature,
            max_output_tokens,
            None,
            variable_overrides,
            prompt_wire_contract,
            &provider_temporal_context.operation_target,
            &provider_temporal_context.authority,
            credential_authority.as_ref(),
            require_exact_credential_authority,
        )? {
            SameBranchGenerationAttempt::Existing(generation_id) => return Ok(generation_id),
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
        };
        let mode = generation_attempt_prompt_authority(&attempt.attempt)?.mode;
        let mut history = self.inner.storage.list_recent_branch_messages_for_prompt(
            branch_id,
            MAX_PROMPT_MESSAGES.saturating_sub(2),
            MAX_HISTORY_MESSAGE_BYTES,
            MAX_HISTORY_MESSAGE_CHARS,
        )?;
        history.push(user_message.clone());
        let prepared = self.prepare_generation_plan(GenerationPlanInput {
            character: &character,
            conversation_id,
            branch_id,
            context_source_branch_id: &attempt.attempt.input.source_branch_id,
            context_head_message_id: attempt.attempt.input.context_head_message_id.as_ref(),
            interaction_state_branch_id: None,
            interaction_state_override: Some(&attempt.interaction_state),
            applied_module_plan_override: attempt.applied_module_plan.as_ref(),
            memory_lineage_branch_id: None,
            mode,
            history: &history,
            model: &model,
            generation_target,
            provider_family: prompt_provider_family,
            temperature,
            max_output_tokens,
            prompt_preset_id: None,
            prompt_selection_authority: attempt.attempt.input.prompt_selection_authority.as_ref(),
            generation_attempt_id: Some(&attempt.attempt.generation_id),
            variable_overrides,
            expected_plan_hash: None,
            prompt_wire_contract,
            resolution_time: attempt.attempt.created_at,
            session_seed: Some(reviewed_prompt_session_seed(
                &attempt.attempt.input.base_request_fingerprint_sha256,
            )),
        })?;
        self.finish_same_branch_generation_dispatch(SameBranchGenerationDispatch {
            conversation_id,
            branch_id,
            expected_head,
            mode,
            model,
            generation_target,
            provider_family,
            preserve_opaque_reasoning_state,
            credential,
            credential_authority,
            require_exact_credential_authority,
            provider,
            provider_target: provider_temporal_context.operation_target,
            user_message,
            attempt,
            prepared,
        })
    }

    fn finish_same_branch_generation_dispatch(
        &self,
        dispatch: SameBranchGenerationDispatch<'_>,
    ) -> CoreResult<GenerationId> {
        let SameBranchGenerationDispatch {
            conversation_id,
            branch_id,
            expected_head,
            mode,
            model,
            generation_target,
            provider_family,
            preserve_opaque_reasoning_state,
            credential,
            credential_authority,
            require_exact_credential_authority,
            provider,
            provider_target,
            user_message,
            attempt,
            mut prepared,
        } = dispatch;
        let generation_id = attempt.attempt.generation_id.clone();
        let generation_started_at = attempt.attempt.created_at;
        prepared.materialized.request.generation_id = generation_id.clone();
        let mut request = prepared.materialized.request.clone();
        let preserve_opaque_reasoning_state =
            preserve_opaque_reasoning_state && credential.as_deref().is_none_or(str::is_empty);
        configure_generation_protocol_request(
            &self.inner.storage,
            &mut request,
            generation_target,
            provider_family,
            preserve_opaque_reasoning_state,
        )?;
        let provider_request_value =
            snapshot_provider_request(provider.as_ref(), &request, generation_target)?;
        let generation_id = request.generation_id.clone();
        let mut assistant_message = Message::pending_assistant(
            conversation_id.clone(),
            user_message.id.clone(),
            generation_id.clone(),
        );
        assistant_message.created_at = generation_started_at;
        let generation = GenerationRecord {
            id: generation_id.clone(),
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            user_message_id: user_message.id.clone(),
            assistant_message_id: Some(assistant_message.id.clone()),
            mode,
            model,
            model_route_id: generation_target.map(|target| target.model_route_id.clone()),
            generation_preset_id: generation_target
                .map(|target| target.generation_preset_id.clone()),
            provider_family,
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
            started_at: assistant_message.created_at,
            finished_at: None,
        };
        let prompt_plan = prepared.generation_prompt_plan_record(
            generation_id.clone(),
            conversation_id.clone(),
            branch_id.clone(),
            expected_head.cloned(),
            user_message.id.clone(),
            generation_target,
            provider_request_value,
            assistant_message.created_at,
        )?;
        let launch = self.prepare_generation_launch_for_target(&generation, &provider_target)?;
        self.seal_same_branch_generation_attempt(attempt.attempt, &prepared, &prompt_plan)?;
        self.inner
            .storage
            .append_generation_attempt_with_prompt_plan(
                branch_id,
                expected_head,
                &user_message,
                &assistant_message,
                &generation,
                &prompt_plan,
                &prepared.knowledge_logs,
                credential_authority.as_ref(),
                require_exact_credential_authority,
            )?;
        let transforms = GenerationTransformContext::from(prepared);
        self.start_generation_task(
            launch,
            branch_id.clone(),
            request,
            assistant_message,
            provider,
            credential,
            transforms,
        )
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn send_message_to_branch_with_provider_options_and_contract_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        model: String,
        generation_target: Option<&GenerationTarget>,
        provider_family: Option<ApiFamily>,
        preserve_opaque_reasoning_state: bool,
        temperature: Option<f64>,
        max_output_tokens: Option<u32>,
        variable_overrides: &VariableMap,
        credential: impl Into<GenerationCredential> + Send,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
        admission_lease: Option<GenerationCredentialAdmissionLease>,
        provider: Arc<dyn Provider>,
        prompt_wire_contract: Option<&PromptRouteWireContract>,
        provider_temporal_context: GenerationProviderTemporalContext,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationId> {
        let credential = credential.into();
        let prompt_provider_family = provider_family.or_else(|| {
            generation_target
                .is_none()
                .then_some(ApiFamily::OpenAiChatCompletions)
        });
        let text = validate_user_message_text(text)?;
        let conversation = self.inner.storage.get_conversation(conversation_id)?;
        let character = self
            .inner
            .storage
            .get_character(&conversation.character_id)?;
        let branch = self.inner.storage.get_conversation_branch(branch_id)?;
        if branch.conversation_id != *conversation_id {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        let mut user_message =
            Message::user_after(conversation_id.clone(), expected_head.cloned(), text);
        user_message.id =
            deterministic_prompt_user_message_id(conversation_id, branch_id, expected_head, text);
        let attempt = match self.prepare_same_branch_generation_attempt(
            &character,
            conversation_id,
            branch_id,
            expected_head,
            mode,
            text,
            operation_context,
            generation_target,
            temperature,
            max_output_tokens,
            None,
            variable_overrides,
            prompt_wire_contract,
            &provider_temporal_context.operation_target,
            &provider_temporal_context.authority,
            credential_authority.as_ref(),
            require_exact_credential_authority,
        )? {
            SameBranchGenerationAttempt::Existing(generation_id) => return Ok(generation_id),
            SameBranchGenerationAttempt::Ready(attempt) => *attempt,
        };
        if let Some(admission_lease) = admission_lease {
            admission_lease.release();
        }
        let mode = generation_attempt_prompt_authority(&attempt.attempt)?.mode;
        let generation_id = attempt.attempt.generation_id.clone();
        let generation_started_at = attempt.attempt.created_at;
        let mut history = self.inner.storage.list_recent_branch_messages_for_prompt(
            branch_id,
            MAX_PROMPT_MESSAGES.saturating_sub(2),
            MAX_HISTORY_MESSAGE_BYTES,
            MAX_HISTORY_MESSAGE_CHARS,
        )?;
        history.push(user_message.clone());
        let mut prepared = self
            .prepare_generation_plan_async(
                GenerationPlanInput {
                    character: &character,
                    conversation_id,
                    branch_id,
                    context_source_branch_id: &attempt.attempt.input.source_branch_id,
                    context_head_message_id: attempt.attempt.input.context_head_message_id.as_ref(),
                    interaction_state_branch_id: None,
                    interaction_state_override: Some(&attempt.interaction_state),
                    applied_module_plan_override: attempt.applied_module_plan.as_ref(),
                    memory_lineage_branch_id: None,
                    mode,
                    history: &history,
                    model: &model,
                    generation_target,
                    provider_family: prompt_provider_family,
                    temperature,
                    max_output_tokens,
                    prompt_preset_id: None,
                    prompt_selection_authority: attempt
                        .attempt
                        .input
                        .prompt_selection_authority
                        .as_ref(),
                    generation_attempt_id: Some(&attempt.attempt.generation_id),
                    variable_overrides,
                    expected_plan_hash: None,
                    prompt_wire_contract,
                    resolution_time: attempt.attempt.created_at,
                    session_seed: Some(reviewed_prompt_session_seed(
                        &attempt.attempt.input.base_request_fingerprint_sha256,
                    )),
                },
                task_credential_broker,
                cancelled,
            )
            .await?;
        prepared.materialized.request.generation_id = generation_id.clone();
        let mut request = prepared.materialized.request.clone();
        let preserve_opaque_reasoning_state =
            preserve_opaque_reasoning_state && credential.as_deref().is_none_or(str::is_empty);
        configure_generation_protocol_request(
            &self.inner.storage,
            &mut request,
            generation_target,
            provider_family,
            preserve_opaque_reasoning_state,
        )?;
        let provider_request_value =
            snapshot_provider_request(provider.as_ref(), &request, generation_target)?;
        let generation_id = request.generation_id.clone();
        let mut assistant_message = Message::pending_assistant(
            conversation_id.clone(),
            user_message.id.clone(),
            generation_id.clone(),
        );
        assistant_message.created_at = generation_started_at;
        let generation = GenerationRecord {
            id: generation_id.clone(),
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            user_message_id: user_message.id.clone(),
            assistant_message_id: Some(assistant_message.id.clone()),
            mode,
            model,
            model_route_id: generation_target.map(|target| target.model_route_id.clone()),
            generation_preset_id: generation_target
                .map(|target| target.generation_preset_id.clone()),
            provider_family,
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
            started_at: assistant_message.created_at,
            finished_at: None,
        };
        let prompt_plan = prepared.generation_prompt_plan_record(
            generation_id.clone(),
            conversation_id.clone(),
            branch_id.clone(),
            expected_head.cloned(),
            user_message.id.clone(),
            generation_target,
            provider_request_value,
            assistant_message.created_at,
        )?;
        let launch = self.prepare_generation_launch_for_target(
            &generation,
            &provider_temporal_context.operation_target,
        )?;
        self.seal_same_branch_generation_attempt(attempt.attempt, &prepared, &prompt_plan)?;
        self.inner
            .storage
            .append_generation_attempt_with_prompt_plan(
                branch_id,
                expected_head,
                &user_message,
                &assistant_message,
                &generation,
                &prompt_plan,
                &prepared.knowledge_logs,
                credential_authority.as_ref(),
                require_exact_credential_authority,
            )?;
        let transforms = GenerationTransformContext {
            sets: prepared.transform_sets,
            variables: prepared.variables,
            supported_capabilities: prepared.supported_capabilities,
            approved_import_source_ids: prepared.approved_import_source_ids,
            display_context: Some(prepared.display_context),
        };
        self.start_generation_task(
            launch,
            branch_id.clone(),
            request,
            assistant_message,
            provider,
            credential,
            transforms,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn edit_user_message_with_provider(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        replacement_text: &str,
        model: String,
        credential: Option<String>,
        provider: Arc<dyn Provider>,
    ) -> CoreResult<MessageActionGeneration> {
        self.start_message_generation_action_with_provider(
            conversation_id,
            branch_id,
            expected_head,
            message_id,
            MessageGenerationAction::EditUser,
            Some(replacement_text),
            GenerationOperationContext::New {
                operation_nonce: "core-direct-edit-v1",
            },
            GenerationActionTargetIdentity::DirectModel {
                model_sha256: format!("{:x}", Sha256::digest(model.as_bytes())),
            },
            model,
            credential,
            provider,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn regenerate_assistant_message_with_provider(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        model: String,
        credential: Option<String>,
        provider: Arc<dyn Provider>,
    ) -> CoreResult<MessageActionGeneration> {
        self.start_message_generation_action_with_provider(
            conversation_id,
            branch_id,
            expected_head,
            message_id,
            MessageGenerationAction::RegenerateAssistant,
            None,
            GenerationOperationContext::New {
                operation_nonce: "core-direct-regenerate-v1",
            },
            GenerationActionTargetIdentity::DirectModel {
                model_sha256: format!("{:x}", Sha256::digest(model.as_bytes())),
            },
            model,
            credential,
            provider,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    fn start_message_generation_action_with_provider(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        action: MessageGenerationAction,
        replacement_text: Option<&str>,
        operation_context: GenerationOperationContext<'_>,
        operation_target: GenerationActionTargetIdentity,
        model: String,
        credential: Option<String>,
        provider: Arc<dyn Provider>,
    ) -> CoreResult<MessageActionGeneration> {
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action,
                replacement_text,
                operation_context,
                target: operation_target,
            },
        )?;
        self.start_message_generation_action_with_provider_options_and_contract(
            action_request,
            model,
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            credential,
            None,
            false,
            provider,
            None,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    async fn start_message_generation_action_with_provider_async(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        message_id: &MessageId,
        action: MessageGenerationAction,
        replacement_text: Option<&str>,
        operation_context: GenerationOperationContext<'_>,
        operation_target: GenerationActionTargetIdentity,
        model: String,
        credential: Option<String>,
        provider: Arc<dyn Provider>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        let action_request = self.prepare_message_generation_action_identity(
            MessageGenerationActionIdentityInput {
                conversation_id,
                source_branch_id: branch_id,
                expected_source_head_message_id: expected_head,
                target_message_id: message_id,
                action,
                replacement_text,
                operation_context,
                target: operation_target,
            },
        )?;
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        self.start_message_generation_action_with_provider_options_and_contract_async(
            action_request,
            model,
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            credential,
            None,
            false,
            None,
            provider,
            None,
            task_credential_broker,
            cancelled,
        )
        .await
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the atomic branch action keeps request planning and durable append in one boundary"
    )]
    fn start_message_generation_action_with_provider_options_and_contract(
        &self,
        action_request: PreparedMessageGenerationAction,
        model: String,
        generation_target: Option<&GenerationTarget>,
        provider_family: Option<ApiFamily>,
        preserve_opaque_reasoning_state: bool,
        temperature: Option<f64>,
        max_output_tokens: Option<u32>,
        credential: impl Into<GenerationCredential>,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
        provider: Arc<dyn Provider>,
        prompt_wire_contract: Option<&PromptRouteWireContract>,
    ) -> CoreResult<MessageActionGeneration> {
        let credential = credential.into();
        let prompt_provider_family = provider_family.or_else(|| {
            generation_target
                .is_none()
                .then_some(ApiFamily::OpenAiChatCompletions)
        });
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let conversation = self
            .inner
            .storage
            .get_conversation(&action_request.conversation_id)?;
        let character = self
            .inner
            .storage
            .get_character(&conversation.character_id)?;
        let mut user_message = Message::user_after(
            action_request.conversation_id.clone(),
            action_request.context.fork_message_id.clone(),
            &action_request.text,
        );
        user_message.id = deterministic_prompt_user_message_id(
            &action_request.conversation_id,
            &action_request.proposed_branch_id,
            action_request.context.fork_message_id.as_ref(),
            &action_request.text,
        );
        let provider_target_authority = message_action_provider_target_authority(
            self,
            &action_request,
            &model,
            generation_target,
            prompt_wire_contract,
        )?;
        let attempt = match self.prepare_message_action_attempt(
            &action_request,
            MessageGenerationAttemptConfiguration {
                generation_target,
                temperature,
                max_output_tokens,
                prompt_wire_contract,
                provider_target_authority: &provider_target_authority,
                credential_authority: credential_authority.as_ref(),
                require_exact_credential_authority,
            },
        )? {
            MessageActionAttempt::Existing(existing) => return Ok(existing),
            MessageActionAttempt::Ready(attempt) => *attempt,
        };
        let mut history = self.inner.storage.list_recent_message_lineage_for_prompt(
            &action_request.conversation_id,
            action_request.context.fork_message_id.as_ref(),
            MAX_PROMPT_MESSAGES.saturating_sub(2),
            MAX_HISTORY_MESSAGE_BYTES,
            MAX_HISTORY_MESSAGE_CHARS,
        )?;
        history.push(user_message.clone());
        let prepared = self.prepare_generation_plan(GenerationPlanInput {
            character: &character,
            conversation_id: &action_request.conversation_id,
            branch_id: &action_request.proposed_branch_id,
            context_source_branch_id: &attempt.attempt.input.source_branch_id,
            context_head_message_id: attempt.attempt.input.context_head_message_id.as_ref(),
            interaction_state_branch_id: Some(&action_request.source_branch_id),
            interaction_state_override: Some(&attempt.interaction_state),
            applied_module_plan_override: attempt.applied_module_plan.as_ref(),
            memory_lineage_branch_id: Some(&action_request.source_branch_id),
            mode: action_request.mode,
            history: &history,
            model: &model,
            generation_target,
            provider_family: prompt_provider_family,
            temperature,
            max_output_tokens,
            prompt_preset_id: None,
            prompt_selection_authority: attempt.attempt.input.prompt_selection_authority.as_ref(),
            generation_attempt_id: Some(&attempt.attempt.generation_id),
            variable_overrides: &lorepia_domain::VariableMap::default(),
            expected_plan_hash: None,
            prompt_wire_contract,
            resolution_time: attempt.attempt.created_at,
            session_seed: Some(reviewed_prompt_session_seed(
                &attempt.attempt.input.base_request_fingerprint_sha256,
            )),
        })?;
        self.finish_message_generation_action(
            action_request,
            attempt,
            user_message,
            model,
            generation_target,
            provider_family,
            preserve_opaque_reasoning_state,
            credential,
            credential_authority,
            require_exact_credential_authority,
            None,
            provider,
            prepared,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the asynchronous action path shares the exact durable append boundary"
    )]
    async fn start_message_generation_action_with_provider_options_and_contract_async(
        &self,
        action_request: PreparedMessageGenerationAction,
        model: String,
        generation_target: Option<&GenerationTarget>,
        provider_family: Option<ApiFamily>,
        preserve_opaque_reasoning_state: bool,
        temperature: Option<f64>,
        max_output_tokens: Option<u32>,
        credential: impl Into<GenerationCredential> + Send,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
        admission_lease: Option<GenerationCredentialAdmissionLease>,
        provider: Arc<dyn Provider>,
        prompt_wire_contract: Option<&PromptRouteWireContract>,
        task_credential_broker: &dyn crate::TaskCredentialBroker,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<MessageActionGeneration> {
        let credential = credential.into();
        let prompt_provider_family = provider_family.or_else(|| {
            generation_target
                .is_none()
                .then_some(ApiFamily::OpenAiChatCompletions)
        });
        if let Some(existing) = self.existing_message_action_generation(&action_request)? {
            return Ok(existing);
        }
        let conversation = self
            .inner
            .storage
            .get_conversation(&action_request.conversation_id)?;
        let character = self
            .inner
            .storage
            .get_character(&conversation.character_id)?;
        let mut user_message = Message::user_after(
            action_request.conversation_id.clone(),
            action_request.context.fork_message_id.clone(),
            &action_request.text,
        );
        user_message.id = deterministic_prompt_user_message_id(
            &action_request.conversation_id,
            &action_request.proposed_branch_id,
            action_request.context.fork_message_id.as_ref(),
            &action_request.text,
        );
        let provider_target_authority = message_action_provider_target_authority(
            self,
            &action_request,
            &model,
            generation_target,
            prompt_wire_contract,
        )?;
        let attempt = match self.prepare_message_action_attempt(
            &action_request,
            MessageGenerationAttemptConfiguration {
                generation_target,
                temperature,
                max_output_tokens,
                prompt_wire_contract,
                provider_target_authority: &provider_target_authority,
                credential_authority: credential_authority.as_ref(),
                require_exact_credential_authority,
            },
        )? {
            MessageActionAttempt::Existing(existing) => return Ok(existing),
            MessageActionAttempt::Ready(attempt) => *attempt,
        };
        let mut history = self.inner.storage.list_recent_message_lineage_for_prompt(
            &action_request.conversation_id,
            action_request.context.fork_message_id.as_ref(),
            MAX_PROMPT_MESSAGES.saturating_sub(2),
            MAX_HISTORY_MESSAGE_BYTES,
            MAX_HISTORY_MESSAGE_CHARS,
        )?;
        history.push(user_message.clone());
        let prepared = self
            .prepare_generation_plan_async(
                GenerationPlanInput {
                    character: &character,
                    conversation_id: &action_request.conversation_id,
                    branch_id: &action_request.proposed_branch_id,
                    context_source_branch_id: &attempt.attempt.input.source_branch_id,
                    context_head_message_id: attempt.attempt.input.context_head_message_id.as_ref(),
                    interaction_state_branch_id: Some(&action_request.source_branch_id),
                    interaction_state_override: Some(&attempt.interaction_state),
                    applied_module_plan_override: attempt.applied_module_plan.as_ref(),
                    memory_lineage_branch_id: Some(&action_request.source_branch_id),
                    mode: action_request.mode,
                    history: &history,
                    model: &model,
                    generation_target,
                    provider_family: prompt_provider_family,
                    temperature,
                    max_output_tokens,
                    prompt_preset_id: None,
                    prompt_selection_authority: attempt
                        .attempt
                        .input
                        .prompt_selection_authority
                        .as_ref(),
                    generation_attempt_id: Some(&attempt.attempt.generation_id),
                    variable_overrides: &VariableMap::default(),
                    expected_plan_hash: None,
                    prompt_wire_contract,
                    resolution_time: attempt.attempt.created_at,
                    session_seed: Some(reviewed_prompt_session_seed(
                        &attempt.attempt.input.base_request_fingerprint_sha256,
                    )),
                },
                task_credential_broker,
                cancelled,
            )
            .await?;
        self.finish_message_generation_action(
            action_request,
            attempt,
            user_message,
            model,
            generation_target,
            provider_family,
            preserve_opaque_reasoning_state,
            credential,
            credential_authority,
            require_exact_credential_authority,
            admission_lease,
            provider,
            prepared,
        )
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "dispatch seals and appends the complete action generation atomically"
    )]
    fn finish_message_generation_action(
        &self,
        action_request: PreparedMessageGenerationAction,
        attempt: PreparedMessageActionAttempt,
        user_message: Message,
        model: String,
        generation_target: Option<&GenerationTarget>,
        provider_family: Option<ApiFamily>,
        preserve_opaque_reasoning_state: bool,
        credential: GenerationCredential,
        credential_authority: Option<ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
        admission_lease: Option<GenerationCredentialAdmissionLease>,
        provider: Arc<dyn Provider>,
        mut prepared: crate::orchestration::PreparedGenerationPlan,
    ) -> CoreResult<MessageActionGeneration> {
        let generation_id = attempt.attempt.generation_id.clone();
        let generation_started_at = attempt.attempt.created_at;
        prepared.materialized.request.generation_id = generation_id.clone();
        let mut request = prepared.materialized.request.clone();
        let preserve_opaque_reasoning_state =
            preserve_opaque_reasoning_state && credential.as_deref().is_none_or(str::is_empty);
        configure_generation_protocol_request(
            &self.inner.storage,
            &mut request,
            generation_target,
            provider_family,
            preserve_opaque_reasoning_state,
        )?;
        let provider_request_value =
            snapshot_provider_request(provider.as_ref(), &request, generation_target)?;
        let generation_id = request.generation_id.clone();
        let (assistant_message, branch, generation) = build_message_action_generation_records(
            &action_request,
            &user_message,
            &generation_id,
            generation_started_at,
            model,
            generation_target,
            provider_family,
        );
        let prompt_plan = prepared.generation_prompt_plan_record(
            generation_id.clone(),
            action_request.conversation_id.clone(),
            branch.id.clone(),
            branch.fork_message_id.clone(),
            user_message.id.clone(),
            generation_target,
            provider_request_value,
            assistant_message.created_at,
        )?;
        let target_interaction_state_key = attempt.target_interaction_state_key.clone();
        let launch =
            self.prepare_generation_launch_for_target(&generation, &action_request.target)?;
        self.seal_same_branch_generation_attempt(attempt.attempt, &prepared, &prompt_plan)?;
        self.inner
            .storage
            .append_message_generation_action_attempt_with_prompt_plan(
                &action_request.source_branch_id,
                action_request.expected_source_head_message_id.as_ref(),
                &action_request.target_message_id,
                action_request.action,
                &branch,
                &target_interaction_state_key,
                &user_message,
                &assistant_message,
                &generation,
                &prompt_plan,
                &prepared.knowledge_logs,
                credential_authority.as_ref(),
                require_exact_credential_authority,
            )?;
        if let Some(admission_lease) = admission_lease {
            admission_lease.release();
        }
        let transforms = GenerationTransformContext::from(prepared);
        self.start_generation_task(
            launch,
            branch.id.clone(),
            request,
            assistant_message,
            provider,
            credential,
            transforms,
        )?;
        Ok(MessageActionGeneration {
            branch,
            generation_id,
        })
    }

    fn prepare_generation_launch(
        &self,
        generation: &GenerationRecord,
        provider_admission_key: GenerationProviderAdmissionKey,
    ) -> CoreResult<GenerationLaunchPermit> {
        let preserve_partial = self
            .inner
            .storage
            .load_settings()?
            .preserve_partial_generations;
        let (cancel_sender, cancel_receiver) = watch::channel(false);
        self.inner.active_generations.register(
            generation,
            provider_admission_key,
            cancel_sender,
        )?;
        Ok(GenerationLaunchPermit {
            generation_id: generation.id.clone(),
            active_generations: Arc::clone(&self.inner.active_generations),
            cancel_receiver: Some(cancel_receiver),
            preserve_partial,
        })
    }

    fn generation_provider_admission_key(
        &self,
        target: &GenerationActionTargetIdentity,
    ) -> CoreResult<GenerationProviderAdmissionKey> {
        match target {
            GenerationActionTargetIdentity::GenerationTarget { model_route_id, .. } => {
                self.generation_provider_admission_key_for_model_route(model_route_id)
            }
            GenerationActionTargetIdentity::ProviderProfile {
                provider_profile_id,
            } => Ok(GenerationProviderAdmissionKey::ProviderProfile(
                provider_profile_id.clone(),
            )),
            #[cfg(test)]
            GenerationActionTargetIdentity::DirectModel { model_sha256 } => Ok(
                GenerationProviderAdmissionKey::DirectModel(model_sha256.clone()),
            ),
        }
    }

    fn prepare_generation_launch_for_target(
        &self,
        generation: &GenerationRecord,
        target: &GenerationActionTargetIdentity,
    ) -> CoreResult<GenerationLaunchPermit> {
        let provider_admission_key = self.generation_provider_admission_key(target)?;
        self.prepare_generation_launch(generation, provider_admission_key)
    }

    fn generation_provider_admission_key_for_model_route(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<GenerationProviderAdmissionKey> {
        Ok(GenerationProviderAdmissionKey::Connection(
            self.inner
                .storage
                .get_model_route(model_route_id)?
                .connection_id,
        ))
    }

    fn ensure_interaction_state_available(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<()> {
        self.drain_available_core_lifecycle_occurrences()?;
        self.inner
            .storage
            .get_interaction_state_snapshot(conversation_id, branch_id)
            .map(|_| ())
            .map_err(|error| {
                if error.code == CoreErrorCode::NotFound {
                    CoreError::new(
                        CoreErrorCode::StorageUnavailable,
                        "interaction lifecycle initialization is backlogged; retry the request",
                        true,
                    )
                } else {
                    error
                }
            })
    }

    fn resolve_same_branch_generation_operation_identity(
        &self,
        input: SameBranchGenerationAttemptIdentity<'_>,
    ) -> CoreResult<ResolvedGenerationOperationIdentity> {
        let base_request_fingerprint_sha256 = same_branch_generation_semantic_fingerprint(&input)?;
        let (operation_id, resume_generation_attempt_id) = match input.operation_context {
            GenerationOperationContext::New { operation_nonce } => (
                new_generation_operation_id(
                    "generation-send-v5",
                    &base_request_fingerprint_sha256,
                    operation_nonce,
                )?,
                None,
            ),
            GenerationOperationContext::Resume {
                generation_attempt_id,
            } => {
                let attempt = self
                    .inner
                    .storage
                    .get_generation_attempt(generation_attempt_id)?;
                validate_same_branch_attempt_semantic_identity(
                    &attempt,
                    input.conversation_id,
                    input.branch_id,
                    input.expected_head,
                    &base_request_fingerprint_sha256,
                    Some(generation_attempt_id),
                )?;
                (
                    attempt.input.operation_id,
                    Some(generation_attempt_id.clone()),
                )
            }
        };
        Ok(ResolvedGenerationOperationIdentity {
            operation_id,
            base_request_fingerprint_sha256,
            resume_generation_attempt_id,
        })
    }

    fn preflight_same_branch_provider_authority(
        &self,
        input: SameBranchGenerationAttemptIdentity<'_>,
        provider_target_authority: &GenerationProviderTargetAuthority,
    ) -> CoreResult<()> {
        let conversation_id = input.conversation_id;
        let branch_id = input.branch_id;
        let expected_head = input.expected_head;
        let is_resume = matches!(
            input.operation_context,
            GenerationOperationContext::Resume { .. }
        );
        let operation = self.resolve_same_branch_generation_operation_identity(input)?;
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(conversation_id, &operation.operation_id)
        {
            Ok(attempt) => {
                validate_same_branch_attempt_semantic_identity(
                    &attempt,
                    conversation_id,
                    branch_id,
                    expected_head,
                    &operation.base_request_fingerprint_sha256,
                    operation.resume_generation_attempt_id.as_ref(),
                )?;
                require_generation_provider_target_authority(&attempt, provider_target_authority)
            }
            Err(error) if error.code == CoreErrorCode::NotFound && !is_resume => Ok(()),
            Err(error) if error.code == CoreErrorCode::NotFound => Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation resume attempt is unavailable; start a new generation operation",
                true,
            )),
            Err(error) => Err(error),
        }
    }

    fn prepare_same_branch_generation_target(
        &self,
        input: SameBranchGenerationTargetInput<'_>,
    ) -> CoreResult<PreparedSameBranchGenerationTarget> {
        let SameBranchGenerationTargetInput {
            conversation_id,
            branch_id,
            expected_head,
            live_mode,
            text,
            operation_context,
            target,
            prompt_preset_id,
            variable_overrides,
        } = input;
        let operation_target = GenerationActionTargetIdentity::GenerationTarget {
            model_route_id: target.model_route_id.clone(),
            generation_preset_id: target.generation_preset_id.clone(),
        };
        let is_resume = matches!(operation_context, GenerationOperationContext::Resume { .. });
        let operation = self.resolve_same_branch_generation_operation_identity(
            SameBranchGenerationAttemptIdentity {
                conversation_id,
                branch_id,
                expected_head,
                text,
                operation_context,
                target: &operation_target,
                temperature: None,
                max_output_tokens: None,
                prompt_preset_id,
                variable_overrides,
            },
        )?;
        match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(conversation_id, &operation.operation_id)
        {
            Ok(attempt) => {
                validate_same_branch_attempt_semantic_identity(
                    &attempt,
                    conversation_id,
                    branch_id,
                    expected_head,
                    &operation.base_request_fingerprint_sha256,
                    operation.resume_generation_attempt_id.as_ref(),
                )?;
                let validated = validate_generation_target_for_attempt(self, target, &attempt)?;
                let provider_target_authority =
                    generation_target_provider_authority(target, &validated)?;
                require_generation_provider_target_authority(&attempt, &provider_target_authority)?;
                Ok(PreparedSameBranchGenerationTarget {
                    mode: generation_attempt_prompt_authority(&attempt)?.mode,
                    validated,
                    provider_target_authority,
                })
            }
            Err(error) if error.code == CoreErrorCode::NotFound && !is_resume => {
                let reasoning_effort = self.prompt_reasoning_effort_for_context(
                    conversation_id,
                    branch_id,
                    live_mode,
                    prompt_preset_id,
                )?;
                let validated = validate_generation_target_plan_with_reasoning_effort(
                    self,
                    target,
                    reasoning_effort,
                )?;
                let provider_target_authority =
                    generation_target_provider_authority(target, &validated)?;
                Ok(PreparedSameBranchGenerationTarget {
                    mode: live_mode,
                    validated,
                    provider_target_authority,
                })
            }
            Err(error) if error.code == CoreErrorCode::NotFound => Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation resume attempt is unavailable; start a new generation operation",
                true,
            )),
            Err(error) => Err(error),
        }
    }

    /// Prepares or resumes the isolated attempt shared by expert preview and
    /// reviewed send. `expected_plan_hash` is intentionally absent from the
    /// operation identity; it is validated later against the resolved plan.
    fn prepare_reviewed_prompt_generation_attempt(
        &self,
        plan_request: &crate::PromptPlanRequest,
        operation_context: GenerationOperationContext<'_>,
        mode: ConversationMode,
        resolved: &ResolvedGenerationTarget,
    ) -> CoreResult<SameBranchGenerationAttempt> {
        let text = validate_user_message_text(&plan_request.user_text)?;
        let conversation = self
            .inner
            .storage
            .get_conversation(&plan_request.conversation_id)?;
        let character = self
            .inner
            .storage
            .get_character(&conversation.character_id)?;
        let validated = validate_generation_target_plan_with_reasoning_effort(
            self,
            &plan_request.generation_target,
            resolved.prompt_wire_contract.reasoning_effort_applied,
        )?;
        let provider_target_authority =
            generation_target_provider_authority(&plan_request.generation_target, &validated)?;
        let operation_target = GenerationActionTargetIdentity::GenerationTarget {
            model_route_id: plan_request.generation_target.model_route_id.clone(),
            generation_preset_id: plan_request.generation_target.generation_preset_id.clone(),
        };
        self.prepare_same_branch_generation_attempt(
            &character,
            &plan_request.conversation_id,
            &plan_request.branch_id,
            plan_request.expected_head.as_ref(),
            mode,
            text,
            operation_context,
            Some(&plan_request.generation_target),
            None,
            None,
            plan_request.prompt_preset_id.as_ref(),
            &plan_request.variable_overrides,
            Some(&resolved.prompt_wire_contract),
            &operation_target,
            &provider_target_authority,
            None,
            false,
        )
    }

    fn validate_existing_reviewed_generation(
        &self,
        generation_id: GenerationId,
        expected_generation_attempt_id: &GenerationId,
        expected_plan_hash: &str,
    ) -> CoreResult<GenerationId> {
        validate_reviewed_generation_attempt_id(expected_generation_attempt_id, &generation_id)?;
        let stored_plan = self.get_generation_prompt_plan(&generation_id)?;
        if stored_plan.id != expected_plan_hash {
            return Err(CoreError::invalid(
                "prompt plan changed after preview; resolve a new preview before sending",
            ));
        }
        Ok(generation_id)
    }

    fn existing_same_branch_generation_attempt(
        &self,
        request: ExistingSameBranchAttemptRequest<'_>,
    ) -> CoreResult<ExistingSameBranchAttempt> {
        let existing = match self
            .inner
            .storage
            .get_generation_attempt_by_operation_id(request.conversation_id, request.operation_id)
        {
            Ok(existing) => existing,
            Err(error) if error.code == CoreErrorCode::NotFound => {
                return Ok(ExistingSameBranchAttempt::Missing);
            }
            Err(error) => return Err(error),
        };
        validate_same_branch_attempt_semantic_identity(
            &existing,
            request.conversation_id,
            request.branch_id,
            request.expected_head,
            request.base_request_fingerprint_sha256,
            request.resume_generation_attempt_id,
        )?;
        require_generation_provider_target_authority(&existing, request.provider_target_authority)?;
        if matches!(
            existing.status,
            lorepia_storage::GenerationAttemptStatus::Running
                | lorepia_storage::GenerationAttemptStatus::Completed
        ) {
            return Ok(ExistingSameBranchAttempt::Resolved(
                SameBranchGenerationAttempt::Existing(existing.generation_id),
            ));
        }
        if existing.status == lorepia_storage::GenerationAttemptStatus::Prepared {
            return Ok(ExistingSameBranchAttempt::Prepared(Box::new(existing)));
        }
        let before = self
            .inner
            .storage
            .get_generation_attempt_before_review(&existing.generation_id)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation attempt is missing its immutable review",
                    false,
                )
            })?;
        self.advance_same_branch_generation_attempt(
            existing,
            request.conversation_id,
            request.branch_id,
            None,
            before.applied_runtime_plan,
        )
        .map(ExistingSameBranchAttempt::Resolved)
    }

    fn validate_new_same_branch_module_authority(
        &self,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        prompt_preset_id: Option<&lorepia_domain::PromptPresetId>,
        module_plan_sha256: &Sha256Digest,
    ) -> CoreResult<()> {
        let prompt_module_plan_sha256 = self.resolve_generation_module_plan_sha256(
            character,
            conversation_id,
            branch_id,
            mode,
            prompt_preset_id,
        )?;
        if prompt_module_plan_sha256 != *module_plan_sha256 {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "prompt and interaction module runtime authorities diverged",
                false,
            ));
        }
        Ok(())
    }

    fn revalidate_prepared_same_branch_credential_authority(
        &self,
        existing: Option<&lorepia_storage::StoredGenerationAttempt>,
        provider_target_authority: &GenerationProviderTargetAuthority,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<()> {
        let Some(existing) = existing else {
            return Ok(());
        };
        require_generation_provider_target_authority(existing, provider_target_authority)?;
        if require_exact_credential_authority {
            self.inner
                .storage
                .prepare_generation_attempt_with_credential_authority(
                    &existing.input,
                    existing.created_at,
                    credential_authority,
                )?;
        }
        Ok(())
    }

    fn resolve_same_branch_module_authority(
        &self,
        existing: Option<&lorepia_storage::StoredGenerationAttempt>,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        prompt_preset_id: Option<&lorepia_domain::PromptPresetId>,
    ) -> CoreResult<(
        lorepia_orchestration::ModuleMergeReview,
        Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
        Sha256Digest,
    )> {
        let (module_runtime_review, applied_module_plan) = if let Some(existing) = existing {
            let (review, plan) = generation_attempt_module_authority(existing)?;
            (review.clone(), plan.cloned())
        } else {
            self.preview_module_runtime_authority_for_proposed_branch(conversation_id, branch_id)?
        };
        let module_plan_sha256 = applied_module_plan.as_ref().map_or_else(
            lorepia_orchestration::no_applied_module_runtime_plan_sha256,
            |plan| plan.applied_plan_sha256.clone(),
        );
        if existing.is_none() {
            self.validate_new_same_branch_module_authority(
                character,
                conversation_id,
                branch_id,
                mode,
                prompt_preset_id,
                &module_plan_sha256,
            )?;
        }
        Ok((
            module_runtime_review,
            applied_module_plan,
            module_plan_sha256,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_same_branch_generation_attempt(
        &self,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        mode: ConversationMode,
        text: &str,
        operation_context: GenerationOperationContext<'_>,
        generation_target: Option<&GenerationTarget>,
        temperature: Option<f64>,
        max_output_tokens: Option<u32>,
        prompt_preset_id: Option<&lorepia_domain::PromptPresetId>,
        variable_overrides: &VariableMap,
        prompt_wire_contract: Option<&PromptRouteWireContract>,
        operation_target: &GenerationActionTargetIdentity,
        provider_target_authority: &GenerationProviderTargetAuthority,
        credential_authority: Option<&ProviderCredentialAccessAuthority>,
        require_exact_credential_authority: bool,
    ) -> CoreResult<SameBranchGenerationAttempt> {
        self.ensure_interaction_state_available(conversation_id, branch_id)?;
        let operation = self.resolve_same_branch_generation_operation_identity(
            SameBranchGenerationAttemptIdentity {
                conversation_id,
                branch_id,
                expected_head,
                text,
                operation_context,
                target: operation_target,
                temperature,
                max_output_tokens,
                prompt_preset_id,
                variable_overrides,
            },
        )?;
        let existing_attempt = match self.existing_same_branch_generation_attempt(
            ExistingSameBranchAttemptRequest {
                conversation_id,
                branch_id,
                expected_head,
                operation_id: &operation.operation_id,
                base_request_fingerprint_sha256: &operation.base_request_fingerprint_sha256,
                provider_target_authority,
                resume_generation_attempt_id: operation.resume_generation_attempt_id.as_ref(),
            },
        )? {
            ExistingSameBranchAttempt::Missing => None,
            ExistingSameBranchAttempt::Prepared(existing) => Some(*existing),
            ExistingSameBranchAttempt::Resolved(result) => return Ok(result),
        };
        self.revalidate_prepared_same_branch_credential_authority(
            existing_attempt.as_ref(),
            provider_target_authority,
            credential_authority,
            require_exact_credential_authority,
        )?;
        let (module_runtime_review, applied_module_plan, module_plan_sha256) = self
            .resolve_same_branch_module_authority(
                existing_attempt.as_ref(),
                character,
                conversation_id,
                branch_id,
                mode,
                prompt_preset_id,
            )?;
        let attempt = if let Some(existing) = existing_attempt {
            require_generation_attempt_module_plan(&existing, &module_plan_sha256)?;
            existing
        } else {
            let prompt_selection_authority = self.capture_generation_prompt_selection_authority(
                GenerationPromptAuthorityCapture {
                    character,
                    conversation_id,
                    branch_id,
                    mode,
                    explicit_preset_id: prompt_preset_id,
                    generation_target,
                    temperature,
                    max_output_tokens,
                    prompt_wire_contract,
                    provider_target_authority: provider_target_authority.clone(),
                },
            )?;
            let input = lorepia_storage::GenerationAttemptInput {
                operation_id: operation.operation_id,
                conversation_id: conversation_id.clone(),
                source_branch_id: branch_id.clone(),
                proposed_branch_id: branch_id.clone(),
                expected_head_message_id: expected_head.cloned(),
                context_head_message_id: expected_head.cloned(),
                module_plan_sha256,
                base_request_fingerprint_sha256: operation.base_request_fingerprint_sha256,
                prompt_selection_authority: Some(prompt_selection_authority),
                module_runtime_review_authority: Some(module_runtime_review.clone()),
                applied_runtime_plan_authority: applied_module_plan.clone(),
            };
            if require_exact_credential_authority {
                self.inner
                    .storage
                    .prepare_generation_attempt_with_credential_authority(
                        &input,
                        Utc::now(),
                        credential_authority,
                    )?
            } else {
                self.inner
                    .storage
                    .prepare_generation_attempt(&input, Utc::now())?
            }
        };
        self.advance_same_branch_generation_attempt(
            attempt,
            conversation_id,
            branch_id,
            Some(&module_runtime_review),
            applied_module_plan,
        )
    }

    fn advance_same_branch_generation_attempt(
        &self,
        mut attempt: lorepia_storage::StoredGenerationAttempt,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        module_runtime_review: Option<&lorepia_orchestration::ModuleMergeReview>,
        applied_module_plan: Option<lorepia_orchestration::AppliedModuleRuntimePlan>,
    ) -> CoreResult<SameBranchGenerationAttempt> {
        if attempt.status == lorepia_storage::GenerationAttemptStatus::Prepared {
            if !self
                .inner
                .storage
                .list_interaction_proposals(
                    conversation_id,
                    branch_id,
                    lorepia_domain::InteractionProposalStatus::Pending,
                    1,
                )?
                .is_empty()
            {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "generation is blocked by an existing interaction approval",
                    true,
                ));
            }
            let boundary = self
                .inner
                .storage
                .get_generation_attempt_interaction_boundary(&attempt.generation_id)?;
            let review = self.prepare_generation_attempt_before_review(
                &attempt,
                &boundary.state,
                &boundary.context_checkpoint_sha256,
                module_runtime_review.ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "prepared generation attempt is missing its module review",
                        false,
                    )
                })?,
                applied_module_plan.as_ref(),
                attempt.created_at,
            )?;
            self.inner
                .storage
                .commit_generation_attempt_before_review(&review)?;
            attempt = self
                .inner
                .storage
                .get_generation_attempt(&attempt.generation_id)?;
        }
        match attempt.status {
            lorepia_storage::GenerationAttemptStatus::BeforeGenerationApplied
            | lorepia_storage::GenerationAttemptStatus::DispatchReady => {
                let boundary = self
                    .inner
                    .storage
                    .get_generation_attempt_interaction_boundary(&attempt.generation_id)?;
                let aggregate = self
                    .inner
                    .storage
                    .get_generation_attempt_interaction_aggregate(&attempt.generation_id)?;
                if aggregate.pending_proposal_count != 0 {
                    return Err(CoreError::new(
                        CoreErrorCode::PermissionDenied,
                        "generation is waiting for an interaction approval",
                        true,
                    ));
                }
                Ok(SameBranchGenerationAttempt::Ready(Box::new(
                    PreparedSameBranchGenerationAttempt {
                        attempt,
                        interaction_state: lorepia_storage::StoredInteractionState {
                            key: boundary.state.key,
                            state: aggregate.state,
                            knowledge: aggregate.knowledge,
                        },
                        applied_module_plan,
                    },
                )))
            }
            lorepia_storage::GenerationAttemptStatus::Running
            | lorepia_storage::GenerationAttemptStatus::Completed => {
                Ok(SameBranchGenerationAttempt::Existing(attempt.generation_id))
            }
            lorepia_storage::GenerationAttemptStatus::Prepared => Err(CoreError::new(
                CoreErrorCode::StorageUnavailable,
                "generation attempt remained unreviewed",
                true,
            )),
            lorepia_storage::GenerationAttemptStatus::AwaitingApproval => Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "generation is waiting for an interaction approval",
                true,
            )),
            lorepia_storage::GenerationAttemptStatus::FailedBeforeDispatch => Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "generation attempt requires an explicit pre-dispatch retry",
                true,
            )),
        }
    }

    fn seal_same_branch_generation_attempt(
        &self,
        attempt: lorepia_storage::StoredGenerationAttempt,
        prepared: &crate::orchestration::PreparedGenerationPlan,
        prompt_plan: &lorepia_storage::GenerationPromptPlanRecord,
    ) -> CoreResult<lorepia_storage::StoredGenerationAttempt> {
        if attempt.status == lorepia_storage::GenerationAttemptStatus::DispatchReady {
            return Ok(attempt);
        }
        if attempt.status != lorepia_storage::GenerationAttemptStatus::BeforeGenerationApplied {
            return Err(CoreError::invalid(
                "generation attempt is not ready for prompt sealing",
            ));
        }
        let applied_module_plan_sha256 = match prepared.module_plan_sha256.as_ref() {
            Some(value) => Sha256Digest::parse(value.clone()).map_err(CoreError::invalid)?,
            None => lorepia_orchestration::no_applied_module_runtime_plan_sha256(),
        };
        if applied_module_plan_sha256 != attempt.input.module_plan_sha256 {
            return Err(CoreError::invalid(
                "applied module plan changed after BeforeGeneration",
            ));
        }
        let interaction_aggregate = self
            .inner
            .storage
            .get_generation_attempt_interaction_aggregate(&attempt.generation_id)?;
        let before = attempt.before_generation_evidence.as_ref().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt is missing BeforeGeneration evidence",
                false,
            )
        })?;
        let before_generation_evidence_sha256 = attempt
            .before_generation_evidence_sha256
            .clone()
            .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt is missing its BeforeGeneration evidence hash",
                false,
            )
        })?;
        let (final_interaction_state_revision, final_interaction_state_sha256) =
            attempt.approval_evidence.as_ref().map_or_else(
                || {
                    (
                        before.context_state_revision,
                        before.context_state_sha256.clone(),
                    )
                },
                |approval| {
                    (
                        approval.resulting_state_revision,
                        approval.resulting_state_sha256.clone(),
                    )
                },
            );
        self.inner.storage.seal_generation_attempt_dispatch_ready(
            &attempt.generation_id,
            attempt.revision,
            &lorepia_storage::GenerationDispatchSeal {
                final_prompt_plan_sha256: Sha256Digest::parse(prompt_plan.plan_sha256.clone())
                    .map_err(CoreError::invalid)?,
                final_prompt_input_fingerprint_sha256: Sha256Digest::parse(
                    prompt_plan.input_fingerprint_sha256.clone(),
                )
                .map_err(CoreError::invalid)?,
                final_interaction_state_revision,
                final_interaction_state_sha256,
                applied_module_plan_sha256,
                before_generation_evidence_sha256,
                approval_evidence_sha256: attempt.approval_evidence_sha256.clone(),
                derived_chain_sha256: Some(interaction_aggregate.derived_chain_sha256),
                derived_event_count: Some(interaction_aggregate.derived_event_count),
                derived_guard_count: Some(interaction_aggregate.derived_guard_count),
            },
            Utc::now(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_generation_task(
        &self,
        launch: GenerationLaunchPermit,
        branch_id: ConversationBranchId,
        request: GenerationRequest,
        assistant_message: Message,
        provider: Arc<dyn Provider>,
        credential: GenerationCredential,
        transforms: GenerationTransformContext,
    ) -> CoreResult<GenerationId> {
        let generation_id = request.generation_id.clone();
        let task = launch.into_task(
            Arc::clone(&self.inner.storage),
            self.inner.event_bus.clone(),
            branch_id,
            request,
            assistant_message,
            provider,
            credential,
            transforms,
        )?;
        self.inner.runtime.spawn(execute_generation_task(task));
        Ok(generation_id)
    }

    /// Executes one exact auxiliary-task target without exposing its prompt,
    /// request body, credential, or provider events across the Rust boundary.
    ///
    /// The caller owns fallback policy. In particular, it must never retry or
    /// select a fallback after `UnknownOutcome` or `ProviderRejected`.
    pub(crate) async fn execute_task_profile_target(
        &self,
        task_profile: &StoredRevision<TaskProfile>,
        target: &GenerationTarget,
        resolved: ResolvedGenerationTarget,
        prompt: BoundedTaskPrompt,
        credential: ConnectionBoundCredential,
        cancelled: watch::Receiver<bool>,
    ) -> TaskExecutionOutcome {
        let before_dispatch = |error| TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::BeforeDispatch,
            error,
        };
        if let Err(error) =
            self.validate_task_profile_dispatch(task_profile, target, &resolved, &credential)
        {
            return before_dispatch(error);
        }
        if *cancelled.borrow() {
            return TaskExecutionOutcome::Failed {
                classification: TaskDispatchClassification::KnownNoSideEffect,
                error: CoreError::new(
                    CoreErrorCode::Cancelled,
                    "auxiliary task was cancelled before provider dispatch",
                    true,
                ),
            };
        }

        let request = auxiliary_task_generation_request(target, &resolved, prompt);
        if let Err(error) = resolved.provider.snapshot_request(&request) {
            return before_dispatch(error);
        }
        dispatch_auxiliary_task_provider(
            Arc::clone(&resolved.provider),
            request,
            credential,
            task_profile.value.timeout_ms,
            cancelled,
        )
        .await
    }

    fn validate_task_profile_dispatch(
        &self,
        task_profile: &StoredRevision<TaskProfile>,
        target: &GenerationTarget,
        resolved: &ResolvedGenerationTarget,
        credential: &ConnectionBoundCredential,
    ) -> CoreResult<()> {
        let current_profile = self.storage().get_task_profile(&task_profile.value.id)?;
        if current_profile.revision != task_profile.revision
            || current_profile.revision_id != task_profile.revision_id
            || current_profile.value != task_profile.value
            || current_profile.deleted_at.is_some()
            || task_profile.revision_id.is_none()
        {
            return Err(CoreError::invalid(
                "auxiliary task profile changed before provider dispatch",
            ));
        }
        let target_plan = self.resolve_task_generation_targets(&task_profile.value.id)?;
        if !target_plan
            .targets
            .iter()
            .any(|candidate| candidate == target)
        {
            return Err(CoreError::invalid(
                "auxiliary task target is not part of the immutable task profile",
            ));
        }
        let current_target = validate_generation_target_plan(self, target)?;
        if current_target.connection.id != resolved.connection_id
            || current_target.route.model_id != resolved.model
            || current_target.prompt_wire_contract != resolved.prompt_wire_contract
        {
            return Err(CoreError::invalid(
                "auxiliary generation target changed before provider dispatch",
            ));
        }
        validate_connection_credential_binding(&current_target.connection, credential)
    }

    fn active_generation_count(&self) -> usize {
        self.inner.active_generations.len()
    }
}

fn same_branch_generation_semantic_fingerprint(
    input: &SameBranchGenerationAttemptIdentity<'_>,
) -> CoreResult<Sha256Digest> {
    let user_text_sha256 = format!("{:x}", Sha256::digest(input.text.as_bytes()));
    Sha256Digest::parse(canonical_value_sha256(
        &GenerationSendSemanticSnapshot {
            schema_version: 1,
            conversation_id: input.conversation_id,
            branch_id: input.branch_id,
            expected_head_message_id: input.expected_head,
            user_text_sha256: &user_text_sha256,
            target: input.target,
            temperature: input.temperature,
            max_output_tokens: input.max_output_tokens,
            prompt_preset_id: input.prompt_preset_id,
            variable_overrides: input.variable_overrides,
        },
        "generation semantic base request",
    )?)
    .map_err(CoreError::invalid)
}

fn new_generation_operation_id(
    domain: &'static str,
    base_request_fingerprint_sha256: &Sha256Digest,
    operation_nonce: &str,
) -> CoreResult<String> {
    let operation_nonce = validate_generation_operation_nonce(operation_nonce)?;
    let operation_sha256 = canonical_value_sha256(
        &GenerationOperationNonceEnvelope {
            schema_version: 1,
            domain,
            semantic_base_fingerprint_sha256: base_request_fingerprint_sha256,
            operation_nonce,
        },
        "generation operation",
    )?;
    Ok(format!("{domain}-{operation_sha256}"))
}

fn validate_same_branch_attempt_semantic_identity(
    attempt: &lorepia_storage::StoredGenerationAttempt,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    base_request_fingerprint_sha256: &Sha256Digest,
    resume_generation_attempt_id: Option<&GenerationId>,
) -> CoreResult<()> {
    let mismatched = resume_generation_attempt_id
        .is_some_and(|generation_id| generation_id != &attempt.generation_id)
        || attempt.input.conversation_id != *conversation_id
        || attempt.input.source_branch_id != *branch_id
        || attempt.input.proposed_branch_id != *branch_id
        || attempt.input.expected_head_message_id != expected_head.cloned()
        || attempt.input.context_head_message_id != expected_head.cloned()
        || attempt.input.base_request_fingerprint_sha256 != *base_request_fingerprint_sha256
        || attempt.input.prompt_selection_authority.is_none();
    if mismatched {
        return if resume_generation_attempt_id.is_some() {
            Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation resume attempt does not match the caller-owned request; start a new generation operation",
                true,
            ))
        } else {
            Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored same-branch generation attempt differs from its immutable request",
                false,
            ))
        };
    }
    Ok(())
}

fn generation_attempt_prompt_authority(
    attempt: &lorepia_storage::StoredGenerationAttempt,
) -> CoreResult<&lorepia_storage::GenerationPromptSelectionAuthority> {
    attempt
        .input
        .prompt_selection_authority
        .as_ref()
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt is missing its sealed prompt authority",
                false,
            )
        })
}

fn require_generation_attempt_module_plan(
    attempt: &lorepia_storage::StoredGenerationAttempt,
    expected: &Sha256Digest,
) -> CoreResult<()> {
    if attempt.input.module_plan_sha256 != *expected {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "stored generation module plan differs from its immutable request",
            false,
        ));
    }
    Ok(())
}

pub(crate) fn generation_attempt_module_authority(
    attempt: &lorepia_storage::StoredGenerationAttempt,
) -> CoreResult<(
    &lorepia_orchestration::ModuleMergeReview,
    Option<&lorepia_orchestration::AppliedModuleRuntimePlan>,
)> {
    let review = attempt
        .input
        .module_runtime_review_authority
        .as_ref()
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt is missing its sealed module review authority",
                false,
            )
        })?;
    let prompt_authority = generation_attempt_prompt_authority(attempt)?;
    review.verify().map_err(|_| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation attempt module review authority is invalid",
            false,
        )
    })?;
    if review.context.conversation_id.as_deref() != Some(attempt.input.conversation_id.0.as_str())
        || review.context.branch_id.as_deref() != Some(attempt.input.proposed_branch_id.0.as_str())
        || review.context.character_id.as_deref() != Some(prompt_authority.character.id.as_str())
        || review.context.persona_id.as_ref()
            != prompt_authority
                .persona_selection
                .as_ref()
                .map(|selection| &selection.value.persona_id)
        || prompt_local_user_id_sha256(&review.context.local_user_id)
            != prompt_authority.local_user_id_sha256
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation attempt module review authority differs from its target lineage",
            false,
        ));
    }
    if let Some(plan) = attempt.input.applied_runtime_plan_authority.as_ref() {
        plan.verify().map_err(|_| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt applied module authority is invalid",
                false,
            )
        })?;
        if plan.review != *review
            || plan.applied_plan_sha256 != attempt.input.module_plan_sha256
            || review.ordered_bindings.is_empty()
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt applied module authority differs from its review",
                false,
            ));
        }
        return Ok((review, Some(plan)));
    }
    if !review.ordered_bindings.is_empty()
        || attempt.input.module_plan_sha256
            != lorepia_orchestration::no_applied_module_runtime_plan_sha256()
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation attempt no-module authority differs from its review",
            false,
        ));
    }
    Ok((review, None))
}

fn validate_generation_target_for_attempt(
    core: &Core,
    target: &GenerationTarget,
    attempt: &lorepia_storage::StoredGenerationAttempt,
) -> CoreResult<ValidatedGenerationTarget> {
    let authority = generation_attempt_prompt_authority(attempt)?;
    let validated = validate_generation_target_plan_with_reasoning_effort(
        core,
        target,
        authority.quick_settings.reasoning_effort,
    )?;
    let current = generation_target_provider_authority(target, &validated)?;
    require_generation_provider_target_authority(attempt, &current)?;
    Ok(validated)
}

fn auxiliary_task_generation_request(
    target: &GenerationTarget,
    resolved: &ResolvedGenerationTarget,
    prompt: BoundedTaskPrompt,
) -> GenerationRequest {
    let conversation_id = ConversationId::new();
    let created_at = Utc::now();
    let system_message = Message {
        id: MessageId::new(),
        conversation_id: conversation_id.clone(),
        parent_id: None,
        role: MessageRole::System,
        content: prompt.system_instruction,
        status: MessageStatus::Complete,
        generation_id: None,
        created_at,
    };
    let user_message = Message {
        id: MessageId::new(),
        conversation_id: conversation_id.clone(),
        parent_id: Some(system_message.id.clone()),
        role: MessageRole::User,
        content: prompt.input,
        status: MessageStatus::Complete,
        generation_id: None,
        created_at,
    };
    GenerationRequest {
        generation_id: GenerationId::new(),
        conversation_id,
        model: resolved.model.clone(),
        messages: vec![system_message, user_message],
        resolved_prompt_plan: None,
        provider_execution_plan_hash: None,
        temperature: None,
        max_output_tokens: resolved.prompt_wire_contract.configured_max_output_tokens,
        provider_provenance: Some(GenerationProviderProvenance {
            api_family: resolved.api_family,
            model_route_id: target.model_route_id.clone(),
            generation_preset_id: target.generation_preset_id.clone(),
        }),
        preserve_opaque_reasoning_state: false,
        opaque_reasoning_context: Vec::new(),
    }
}

async fn dispatch_auxiliary_task_provider(
    provider: Arc<dyn Provider>,
    request: GenerationRequest,
    credential: ConnectionBoundCredential,
    timeout_ms: u64,
    mut cancelled: watch::Receiver<bool>,
) -> TaskExecutionOutcome {
    if *cancelled.borrow() {
        return TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::KnownNoSideEffect,
            error: CoreError::new(
                CoreErrorCode::Cancelled,
                "auxiliary task was cancelled before provider dispatch",
                true,
            ),
        };
    }
    let (event_sender, event_receiver) = mpsc::channel(128);
    let (attempt_cancel_sender, attempt_cancel_receiver) = watch::channel(false);
    let provider_attempt = async {
        tokio::join!(
            provider.generate(
                request,
                credential.value.as_deref(),
                event_sender,
                attempt_cancel_receiver,
            ),
            collect_task_provider_events(event_receiver),
        )
    };
    tokio::pin!(provider_attempt);
    let timeout = time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    let cancellation = async {
        loop {
            if *cancelled.borrow() {
                break;
            }
            if cancelled.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    };
    tokio::pin!(cancellation);

    let (provider_result, output_result) = tokio::select! {
        result = &mut provider_attempt => result,
        () = &mut cancellation => {
            let _ = attempt_cancel_sender.send(true);
            return unknown_task_outcome("auxiliary task was cancelled after provider dispatch began");
        }
        () = &mut timeout => {
            let _ = attempt_cancel_sender.send(true);
            return unknown_task_outcome("auxiliary task timed out after provider dispatch began");
        }
    };
    classify_task_provider_result(provider_result, output_result)
}

fn unknown_task_outcome(message: &'static str) -> TaskExecutionOutcome {
    TaskExecutionOutcome::Failed {
        classification: TaskDispatchClassification::UnknownOutcome,
        error: CoreError::new(CoreErrorCode::Cancelled, message, false),
    }
}

fn classify_task_provider_result(
    provider_result: CoreResult<GenerationUsage>,
    output_result: CoreResult<String>,
) -> TaskExecutionOutcome {
    match (provider_result, output_result) {
        (Ok(usage), Ok(canonical_text)) if !canonical_text.trim().is_empty() => {
            TaskExecutionOutcome::Completed {
                canonical_text,
                usage,
            }
        }
        (Ok(_), Ok(mut canonical_text)) => {
            canonical_text.zeroize();
            TaskExecutionOutcome::Failed {
                classification: TaskDispatchClassification::ProviderRejected,
                error: CoreError::new(
                    CoreErrorCode::UnsupportedContent,
                    "auxiliary provider returned no canonical text",
                    false,
                ),
            }
        }
        (Ok(_), Err(error)) => TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::ProviderRejected,
            error,
        },
        (Err(error), output_result) => {
            if let Ok(mut output) = output_result {
                output.zeroize();
            }
            TaskExecutionOutcome::Failed {
                classification: task_provider_error_classification(error.code),
                error,
            }
        }
    }
}

fn task_provider_error_classification(code: CoreErrorCode) -> TaskDispatchClassification {
    match code {
        CoreErrorCode::InvalidInput
        | CoreErrorCode::UnsupportedContent
        | CoreErrorCode::NotFound
        | CoreErrorCode::PermissionDenied
        | CoreErrorCode::ProviderAuthFailed
        | CoreErrorCode::ProviderRateLimited => TaskDispatchClassification::ProviderRejected,
        CoreErrorCode::UnsafeArchive
        | CoreErrorCode::StorageUnavailable
        | CoreErrorCode::StorageCorrupted
        | CoreErrorCode::ProviderUnavailable
        | CoreErrorCode::NetworkUnavailable
        | CoreErrorCode::Cancelled
        | CoreErrorCode::Internal => TaskDispatchClassification::UnknownOutcome,
    }
}

async fn collect_task_provider_events(
    mut receiver: mpsc::Receiver<ProviderEvent>,
) -> CoreResult<String> {
    let mut output = String::new();
    let mut rejected = None;
    while let Some(event) = receiver.recv().await {
        match event {
            ProviderEvent::TextDelta(mut delta) => {
                if rejected.is_some() {
                    delta.zeroize();
                    continue;
                }
                let next_bytes = output.len().checked_add(delta.len());
                let next_chars = output.chars().count().checked_add(delta.chars().count());
                if next_bytes.is_none_or(|bytes| bytes > MAX_TASK_OUTPUT_BYTES)
                    || next_chars.is_none_or(|chars| chars > MAX_TASK_OUTPUT_CHARS)
                {
                    output.zeroize();
                    delta.zeroize();
                    rejected = Some(CoreError::new(
                        CoreErrorCode::UnsupportedContent,
                        "auxiliary provider output exceeded its size limit",
                        false,
                    ));
                } else {
                    output.push_str(&delta);
                    delta.zeroize();
                }
            }
            ProviderEvent::ReasoningDelta(mut reasoning) => reasoning.zeroize(),
            ProviderEvent::OpaqueReasoningState(mut state) => {
                state.zeroize_sensitive_payloads();
                rejected.get_or_insert_with(|| {
                    CoreError::new(
                        CoreErrorCode::UnsupportedContent,
                        "auxiliary provider returned unsupported opaque reasoning state",
                        false,
                    )
                });
            }
            ProviderEvent::ToolCallStarted { .. }
            | ProviderEvent::ToolCallArgumentsDelta { .. }
            | ProviderEvent::ToolCallCompleted { .. } => {
                rejected.get_or_insert_with(|| {
                    CoreError::new(
                        CoreErrorCode::UnsupportedContent,
                        "auxiliary provider returned an unsupported tool call",
                        false,
                    )
                });
            }
        }
    }
    if let Some(error) = rejected {
        output.zeroize();
        Err(error)
    } else {
        Ok(output)
    }
}

pub(crate) fn configure_generation_protocol_request(
    storage: &Storage,
    request: &mut GenerationRequest,
    generation_target: Option<&GenerationTarget>,
    provider_family: Option<ApiFamily>,
    mut preserve_opaque_reasoning_state: bool,
) -> CoreResult<()> {
    if preserve_opaque_reasoning_state && let Some(target) = generation_target {
        let route = storage.get_model_route(&target.model_route_id)?;
        let connection = storage.get_provider_connection(&route.connection_id)?;
        if connection.credential_ref.is_some() {
            preserve_opaque_reasoning_state = false;
        }
    }
    let (generation_target, provider_family) = match (generation_target, provider_family) {
        (None, None) if !preserve_opaque_reasoning_state => {
            request.provider_provenance = None;
            request.preserve_opaque_reasoning_state = false;
            request.opaque_reasoning_context.clear();
            return Ok(());
        }
        (Some(target), Some(family)) => (target, family),
        _ => {
            return Err(CoreError::internal(
                "generation provider protocol provenance is inconsistent",
            ));
        }
    };

    let opaque_reasoning_context = if preserve_opaque_reasoning_state {
        load_opaque_reasoning_context(
            storage,
            &request.messages,
            provider_family,
            &request.model,
            generation_target,
        )?
    } else {
        Vec::new()
    };
    request.provider_provenance = Some(GenerationProviderProvenance {
        api_family: provider_family,
        model_route_id: generation_target.model_route_id.clone(),
        generation_preset_id: generation_target.generation_preset_id.clone(),
    });
    request.preserve_opaque_reasoning_state = preserve_opaque_reasoning_state;
    request.opaque_reasoning_context = opaque_reasoning_context;
    Ok(())
}

fn snapshot_provider_request(
    provider: &dyn Provider,
    request: &GenerationRequest,
    generation_target: Option<&GenerationTarget>,
) -> CoreResult<serde_json::Value> {
    match provider.snapshot_request(request) {
        Ok(value) => Ok(value),
        Err(error) => {
            #[cfg(test)]
            if generation_target.is_none()
                && error.code == CoreErrorCode::UnsupportedContent
                && !request.preserve_opaque_reasoning_state
                && request.opaque_reasoning_context.is_empty()
            {
                return serde_json::to_value(request).map_err(|encode_error| {
                    CoreError::internal(format!(
                        "cannot encode synthetic provider request snapshot: {encode_error}"
                    ))
                });
            }
            let _ = generation_target;
            Err(error)
        }
    }
}

fn reject_sensitive_provider_preview_fields(value: &serde_json::Value) -> CoreResult<()> {
    const FORBIDDEN_KEYS: [&str; 12] = [
        "api_key",
        "apikey",
        "authorization",
        "base_url",
        "credential",
        "credentials",
        "endpoint",
        "headers",
        "opaque_reasoning_context",
        "opaque_reasoning_state",
        "token",
        "url",
    ];
    match value {
        serde_json::Value::Object(values) => {
            for (key, value) in values {
                if FORBIDDEN_KEYS.contains(&key.to_ascii_lowercase().as_str()) {
                    return Err(CoreError::new(
                        CoreErrorCode::PermissionDenied,
                        "provider preview contained a security-sensitive field",
                        false,
                    ));
                }
                reject_sensitive_provider_preview_fields(value)?;
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                reject_sensitive_provider_preview_fields(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn load_opaque_reasoning_context(
    storage: &Storage,
    history: &[Message],
    provider_family: ApiFamily,
    model: &str,
    generation_target: &GenerationTarget,
) -> CoreResult<Vec<OpaqueReasoningContext>> {
    let mut contexts = Vec::new();
    let mut states = Vec::<OpaqueReasoningState>::new();
    for message in history {
        if message.role != MessageRole::Assistant || message.status != MessageStatus::Complete {
            continue;
        }
        let Some(generation_id) = message.generation_id.as_ref() else {
            continue;
        };
        if generation_id.is_character_greeting() {
            continue;
        }
        let generation = storage.get_generation(generation_id).map_err(|error| {
            if error.code == CoreErrorCode::NotFound {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "assistant message references a missing generation",
                    false,
                )
            } else {
                error
            }
        })?;
        if generation.opaque_reasoning_state.is_empty() {
            continue;
        }
        if generation.status != GenerationStatus::Complete
            || generation.conversation_id != message.conversation_id
            || generation.assistant_message_id.as_ref() != Some(&message.id)
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored opaque reasoning state has inconsistent message ownership",
                false,
            ));
        }
        if generation.provider_family != Some(provider_family)
            || generation.model != model
            || generation.model_route_id.as_ref() != Some(&generation_target.model_route_id)
        {
            continue;
        }
        let generation_preset_id = generation.generation_preset_id.ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored opaque reasoning state is missing preset provenance",
                false,
            )
        })?;
        for state in generation.opaque_reasoning_state {
            states.push(state.clone());
            contexts.push(OpaqueReasoningContext {
                source_message_id: message.id.clone(),
                api_family: provider_family,
                model: model.to_owned(),
                model_route_id: generation_target.model_route_id.clone(),
                generation_preset_id: generation_preset_id.clone(),
                state,
            });
        }
    }
    validate_opaque_reasoning_states(&states).map_err(CoreError::invalid)?;
    Ok(contexts)
}

async fn execute_generation_task(task: GenerationTask) {
    let GenerationTask {
        storage,
        active_generations,
        event_bus,
        branch_id,
        request,
        assistant,
        provider,
        credential,
        cancel_receiver,
        preserve_partial,
        transforms,
    } = task;
    let generation_id = request.generation_id.clone();
    let _active_generation = ActiveGenerationGuard {
        generation_id: generation_id.clone(),
        active_generations: Arc::clone(&active_generations),
    };
    let conversation_id = request.conversation_id.clone();
    let assistant_message_id = assistant.id.clone();
    let defer_text_events = generation_has_output_transforms(&transforms);
    let (event_sender, event_receiver) = mpsc::channel(128);
    let forward_events = tokio::spawn(forward_generation_events(
        event_receiver,
        GenerationEventForwardingContext {
            active_generations: Arc::clone(&active_generations),
            event_bus: event_bus.clone(),
            storage: Arc::clone(&storage),
            checkpoint: assistant.clone(),
            branch_id: branch_id.clone(),
            assistant_message_id: assistant_message_id.clone(),
            preserve_partial,
            defer_text_events,
        },
    ));
    let generation_result = run_generation(
        provider.as_ref(),
        request,
        credential.as_deref(),
        event_sender,
        cancel_receiver,
    )
    .await;
    drop(credential);
    drop(provider);
    let forwarding_result = forward_events
        .await
        .map_err(|error| {
            CoreError::internal(format!(
                "generation event forwarder stopped unexpectedly: {error}"
            ))
        })
        .and_then(std::convert::identity);
    let result = merge_generation_and_forwarding_results(generation_result, forwarding_result);
    finish_generation_task(
        GenerationCompletionContext {
            storage,
            active_generations,
            event_bus,
            branch_id,
            conversation_id,
            generation_id,
            assistant_message_id,
            preserve_partial,
            transforms,
        },
        assistant,
        result,
    );
}

fn finish_generation_task(
    context: GenerationCompletionContext,
    mut assistant: Message,
    result: Result<GenerationOutcome, GenerationFailure>,
) {
    let GenerationCompletionContext {
        storage,
        active_generations,
        event_bus,
        branch_id,
        conversation_id,
        generation_id,
        assistant_message_id,
        preserve_partial,
        transforms,
    } = context;
    let (result, display_projection) = apply_generation_output_transforms(result, &transforms);
    let usage = result.as_ref().ok().map(|outcome| outcome.usage.clone());
    let opaque_reasoning_state = result
        .as_ref()
        .ok()
        .map(|outcome| outcome.opaque_reasoning_state.clone())
        .unwrap_or_default();
    let error_code = result
        .as_ref()
        .err()
        .map(|failure| failure.error.code.as_str().to_owned());

    let (mut sequence, terminal_kind, should_commit) =
        apply_generation_result(&mut assistant, result, preserve_partial);
    let (terminal_kind, committed, projection_committed) = persist_generation_terminal(
        TerminalPersistenceContext {
            storage: &storage,
            generation_id: &generation_id,
        },
        &mut assistant,
        usage.as_ref(),
        &opaque_reasoning_state,
        error_code.as_deref(),
        should_commit,
        display_projection.as_ref(),
        terminal_kind,
    );
    let deferred_display_text = display_projection.as_ref().and_then(|projection| {
        committed.then(|| {
            if projection_committed {
                projection.display_content.clone()
            } else {
                assistant.content.clone()
            }
        })
    });
    if let Some(display_text) = deferred_display_text.filter(|text| !text.is_empty()) {
        let _ = active_generations.publish(
            &event_bus,
            ChatEvent::new(
                generation_id.clone(),
                conversation_id.clone(),
                sequence,
                ChatEventKind::TextDelta(display_text),
            )
            .with_route(branch_id.clone(), assistant_message_id.clone()),
        );
        sequence = sequence.saturating_add(1);
    }
    if committed {
        let _ = active_generations.publish(
            &event_bus,
            ChatEvent::new(
                generation_id.clone(),
                conversation_id.clone(),
                sequence,
                ChatEventKind::MessageCommitted {
                    message_id: assistant.id.clone(),
                    status: assistant.status,
                },
            )
            .with_route(branch_id.clone(), assistant_message_id.clone()),
        );
        sequence = sequence.saturating_add(1);
    }
    let _ = active_generations.publish(
        &event_bus,
        ChatEvent::new(
            generation_id.clone(),
            conversation_id,
            sequence,
            terminal_kind,
        )
        .with_route(branch_id, assistant_message_id),
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "terminal persistence keeps the complete transaction and compensation inputs explicit"
)]
fn persist_generation_terminal(
    context: TerminalPersistenceContext<'_>,
    assistant: &mut Message,
    usage: Option<&lorepia_domain::GenerationUsage>,
    opaque_reasoning_state: &[OpaqueReasoningState],
    error_code: Option<&str>,
    should_commit: bool,
    display_projection: Option<&MessageDisplayProjectionWrite>,
    mut terminal_kind: ChatEventKind,
) -> (ChatEventKind, bool, bool) {
    let original_status = assistant.status;
    let display_projection = should_commit.then_some(display_projection).flatten();
    let persistence = context
        .storage
        .finalize_generation_with_protocol_state_and_display(
            assistant,
            usage,
            opaque_reasoning_state,
            error_code,
            should_commit,
            display_projection,
        );
    let persistence_succeeded = persistence.is_ok();
    let committed = if persistence_succeeded {
        should_commit
    } else {
        assistant.status = MessageStatus::Failed;
        let compensation = context
            .storage
            .fail_generation_after_finalize_error(assistant, should_commit);
        if compensation.is_ok() {
            terminal_kind = generation_persistence_failure();
            should_commit
        } else if context
            .storage
            .get_generation(context.generation_id)
            .is_ok_and(|generation| {
                generation.status == generation_status_for_message(original_status)
            })
        {
            assistant.status = original_status;
            should_commit
        } else {
            terminal_kind = generation_persistence_failure();
            false
        }
    };
    let projection_committed = committed
        && display_projection.is_some_and(|expected| {
            persistence_succeeded
                || context
                    .storage
                    .get_message_display_projection(assistant)
                    .is_ok_and(|stored| {
                        stored.is_some_and(|stored| {
                            stored.display_content == expected.display_content
                        })
                    })
        });
    (terminal_kind, committed, projection_committed)
}

const fn generation_status_for_message(status: MessageStatus) -> GenerationStatus {
    match status {
        MessageStatus::Pending => GenerationStatus::Running,
        MessageStatus::Complete => GenerationStatus::Complete,
        MessageStatus::Cancelled => GenerationStatus::Cancelled,
        MessageStatus::Failed => GenerationStatus::Failed,
    }
}

fn generation_persistence_failure() -> ChatEventKind {
    ChatEventKind::GenerationFailed {
        code: CoreErrorCode::StorageUnavailable.as_str().to_owned(),
        message: GENERATION_PERSISTENCE_FAILURE_MESSAGE.to_owned(),
    }
}

async fn forward_generation_events(
    mut event_receiver: mpsc::Receiver<ChatEvent>,
    context: GenerationEventForwardingContext,
) -> CoreResult<()> {
    let GenerationEventForwardingContext {
        active_generations,
        event_bus,
        storage,
        mut checkpoint,
        branch_id,
        assistant_message_id,
        preserve_partial,
        defer_text_events,
    } = context;
    let start = time::Instant::now() + PARTIAL_CHECKPOINT_INTERVAL;
    let mut interval = time::interval_at(start, PARTIAL_CHECKPOINT_INTERVAL);
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_checkpoint_bytes = 0;
    let mut dirty = false;

    loop {
        tokio::select! {
            event = event_receiver.recv() => {
                let Some(event) = event else {
                    if preserve_partial && dirty {
                        storage.checkpoint_pending_assistant(&checkpoint)?;
                    }
                    return Ok(());
                };
                let is_text_delta = matches!(&event.kind, ChatEventKind::TextDelta(_));
                if !defer_text_events {
                    if preserve_partial
                        && let ChatEventKind::TextDelta(delta) = &event.kind
                    {
                        checkpoint.content.push_str(delta);
                        dirty = true;
                    }
                    active_generations.publish(
                        &event_bus,
                        event.with_route(branch_id.clone(), assistant_message_id.clone())
                    )?;
                } else if !is_text_delta {
                    active_generations.publish(
                        &event_bus,
                        event.with_route(branch_id.clone(), assistant_message_id.clone())
                    )?;
                }
                if preserve_partial
                    && dirty
                    && partial_checkpoint_due(checkpoint.content.len(), last_checkpoint_bytes)
                {
                    storage.checkpoint_pending_assistant(&checkpoint)?;
                    last_checkpoint_bytes = checkpoint.content.len();
                    dirty = false;
                }
            }
            _ = interval.tick(), if preserve_partial => {
                if dirty {
                    storage.checkpoint_pending_assistant(&checkpoint)?;
                    last_checkpoint_bytes = checkpoint.content.len();
                    dirty = false;
                }
            }
        }
    }
}

fn partial_checkpoint_due(current_bytes: usize, last_checkpoint_bytes: usize) -> bool {
    current_bytes.saturating_sub(last_checkpoint_bytes) >= PARTIAL_CHECKPOINT_BYTES
}

fn merge_generation_and_forwarding_results(
    generation: Result<GenerationOutcome, GenerationFailure>,
    forwarding: CoreResult<()>,
) -> Result<GenerationOutcome, GenerationFailure> {
    match (generation, forwarding) {
        (result, Ok(())) => result,
        (Ok(outcome), Err(error)) => Err(GenerationFailure {
            error,
            partial_text: outcome.text,
            last_sequence: outcome.last_sequence,
        }),
        (Err(mut failure), Err(error)) => {
            failure.error = error;
            Err(failure)
        }
    }
}

fn generation_has_output_transforms(context: &GenerationTransformContext) -> bool {
    context.sets.iter().any(|set| {
        set.enabled
            && set.rules.iter().any(|rule| {
                rule.enabled
                    && matches!(
                        rule.phase,
                        TransformPhase::ProviderOutputCanonical | TransformPhase::DisplayOnly
                    )
            })
    })
}

fn apply_generation_output_transforms(
    mut result: Result<GenerationOutcome, GenerationFailure>,
    context: &GenerationTransformContext,
) -> (
    Result<GenerationOutcome, GenerationFailure>,
    Option<MessageDisplayProjectionWrite>,
) {
    if !generation_has_output_transforms(context) {
        return (result, None);
    }
    let text = match &result {
        Ok(outcome) => outcome.text.as_str(),
        Err(failure) => failure.partial_text.as_str(),
    };
    let canonical_phase = apply_generation_transform_phase(
        context,
        TransformPhase::ProviderOutputCanonical,
        text,
        MessageTransformStage::ProviderOutputCanonical,
    );
    let display_phase = apply_generation_transform_phase(
        context,
        TransformPhase::DisplayOnly,
        &canonical_phase.output,
        MessageTransformStage::DisplayOnly,
    );
    let canonical = canonical_phase.output;
    let display = context.display_context.as_ref().map_or_else(
        || display_phase.output.clone(),
        |base_context| {
            let mut display_context = base_context.clone();
            display_context
                .messages
                .push(lorepia_domain::PromptConversationMessage {
                    id: MessageId("portable-display-output".to_owned()),
                    branch_id: display_context.branch_id.clone(),
                    role: lorepia_domain::PromptMessageRole::Assistant,
                    content: canonical.clone(),
                    turn_index: u32::try_from(display_context.messages.len()).unwrap_or(u32::MAX),
                });
            lorepia_orchestration::render_portable_text(&display_phase.output, &display_context)
        },
    );
    match &mut result {
        Ok(outcome) => outcome.text.clone_from(&canonical),
        Err(failure) => failure.partial_text.clone_from(&canonical),
    }
    let mut applications = canonical_phase.applications;
    applications.extend(display_phase.applications);
    let pipeline_failures = canonical_phase
        .pipeline_failure
        .into_iter()
        .chain(display_phase.pipeline_failure)
        .collect();
    (
        result,
        Some(MessageDisplayProjectionWrite {
            display_content: display,
            applications,
            pipeline_failures,
        }),
    )
}

struct GenerationTransformPhaseResult {
    output: String,
    applications: Vec<MessageTransformApplicationWrite>,
    pipeline_failure: Option<MessageTransformPipelineFailureWrite>,
}

fn apply_generation_transform_phase(
    context: &GenerationTransformContext,
    phase: TransformPhase,
    input: &str,
    stage: MessageTransformStage,
) -> GenerationTransformPhaseResult {
    let transformed = crate::orchestration::apply_transform_sets_with_import_approvals(
        &context.sets,
        phase,
        input,
        &context.variables,
        &context.supported_capabilities,
        &context.approved_import_source_ids,
    );
    let Ok(transformed) = transformed else {
        return GenerationTransformPhaseResult {
            output: input.to_owned(),
            applications: Vec::new(),
            pipeline_failure: Some(MessageTransformPipelineFailureWrite {
                stage,
                code: "pipeline_invalid".to_owned(),
                before_sha256: transform_content_sha256(input),
            }),
        };
    };
    let mut diagnostic_invalid = false;
    let applications = transformed
        .reports
        .iter()
        .filter_map(|report| {
            let application = map_generation_transform_report(report, stage);
            diagnostic_invalid |= application.is_none();
            application
        })
        .collect::<Vec<_>>();
    let pipeline_failure =
        transformed
            .error
            .as_ref()
            .map(|error| MessageTransformPipelineFailureWrite {
                stage,
                code: error.code.as_str().to_owned(),
                before_sha256: transform_content_sha256(input),
            });
    GenerationTransformPhaseResult {
        output: transformed.output,
        applications: if diagnostic_invalid {
            Vec::new()
        } else {
            applications
        },
        pipeline_failure: pipeline_failure.or_else(|| {
            diagnostic_invalid.then(|| MessageTransformPipelineFailureWrite {
                stage,
                code: "diagnostic_invalid".to_owned(),
                before_sha256: transform_content_sha256(input),
            })
        }),
    }
}

fn map_generation_transform_report(
    report: &lorepia_orchestration::TransformRuleReport,
    stage: MessageTransformStage,
) -> Option<MessageTransformApplicationWrite> {
    let audit = report.execution_audit.as_ref()?;
    let before_sha256 = Sha256Digest::parse(audit.before_sha256.clone()).ok()?;
    let after_sha256 = audit
        .after_sha256
        .as_ref()
        .map(|value| Sha256Digest::parse(value.clone()))
        .transpose()
        .ok()?;
    let (disposition, code) = match report.status {
        lorepia_orchestration::TransformRuleStatus::Applied => {
            (MessageTransformDisposition::Applied, None)
        }
        lorepia_orchestration::TransformRuleStatus::NoMatch => {
            (MessageTransformDisposition::NoMatch, None)
        }
        lorepia_orchestration::TransformRuleStatus::Disabled => {
            (MessageTransformDisposition::Disabled, None)
        }
        lorepia_orchestration::TransformRuleStatus::PendingImportApproval => {
            (MessageTransformDisposition::PendingImportApproval, None)
        }
        lorepia_orchestration::TransformRuleStatus::ResolvedPromptDisabled => {
            (MessageTransformDisposition::ResolvedPromptDisabled, None)
        }
        lorepia_orchestration::TransformRuleStatus::ConditionFalse => {
            (MessageTransformDisposition::ConditionFalse, None)
        }
        lorepia_orchestration::TransformRuleStatus::Failed => {
            let failure_code = audit.failure_code?;
            let disposition = if matches!(
                failure_code,
                lorepia_orchestration::TransformFailureCode::InputLimitExceeded
                    | lorepia_orchestration::TransformFailureCode::OutputLimitExceeded
            ) {
                MessageTransformDisposition::LimitRejected
            } else {
                MessageTransformDisposition::Failed
            };
            (disposition, Some(failure_code.as_str().to_owned()))
        }
    };
    Some(MessageTransformApplicationWrite {
        set_id: audit.set_id.as_str().to_owned(),
        rule_id: report.trace.rule_id.as_str().to_owned(),
        stage,
        disposition,
        code,
        before_sha256,
        after_sha256,
        replacement_count: report.trace.replacements,
        input_chars: report.trace.input_chars,
        output_chars: report.trace.output_chars,
    })
}

fn transform_content_sha256(value: &str) -> Sha256Digest {
    match Sha256Digest::parse(format!("{:x}", Sha256::digest(value.as_bytes()))) {
        Ok(digest) => digest,
        Err(error) => unreachable!("SHA-256 formatter produced an invalid digest: {error}"),
    }
}

fn apply_generation_result(
    assistant: &mut Message,
    result: Result<GenerationOutcome, GenerationFailure>,
    preserve_partial: bool,
) -> (u64, ChatEventKind, bool) {
    match result {
        Ok(outcome) => {
            assistant.content = outcome.text;
            assistant.status = MessageStatus::Complete;
            (
                outcome.last_sequence.saturating_add(1),
                ChatEventKind::GenerationFinished,
                true,
            )
        }
        Err(failure) => {
            let cancelled = failure.error.code == CoreErrorCode::Cancelled;
            assistant.content = failure.partial_text;
            assistant.status = if cancelled {
                MessageStatus::Cancelled
            } else {
                MessageStatus::Failed
            };
            let terminal = if cancelled {
                ChatEventKind::GenerationCancelled
            } else {
                ChatEventKind::GenerationFailed {
                    code: failure.error.code.as_str().to_owned(),
                    message: failure.error.message,
                }
            };
            (
                failure.last_sequence.saturating_add(1),
                terminal,
                preserve_partial && !assistant.content.is_empty(),
            )
        }
    }
}

pub(crate) type ReconciledModelRoutes = (Vec<ModelRoute>, Vec<ModelRouteId>, Vec<ModelRouteId>);

pub(crate) fn provider_api_capability_observations(
    routes: &[ModelRoute],
    listed_models: &[ListedModel],
    observed_at: DateTime<Utc>,
) -> CoreResult<Vec<CapabilityObservation>> {
    let routes_by_model = routes
        .iter()
        .map(|route| (route.model_id.as_str(), route))
        .collect::<HashMap<_, _>>();
    let expires_at = observed_at.checked_add_signed(PROVIDER_API_CAPABILITY_FRESHNESS);
    let mut observations = Vec::new();
    for model in listed_models {
        let route = routes_by_model
            .get(model.model_id.as_str())
            .ok_or_else(|| {
                CoreError::internal("reconciled model route is missing from capability ingestion")
            })?;
        for (key, value) in [
            (CapabilityKey::ContextWindow, model.max_input_tokens),
            (CapabilityKey::MaxOutputTokens, model.max_output_tokens),
        ] {
            let Some(value) = value else {
                continue;
            };
            if value == 0 {
                return Err(CoreError::new(
                    CoreErrorCode::ProviderUnavailable,
                    "provider model metadata contains a zero token limit",
                    false,
                ));
            }
            observations.push(CapabilityObservation {
                id: deterministic_capability_observation_id(
                    &route.id,
                    key,
                    ObservationSource::ProviderApi,
                ),
                model_route_id: route.id.clone(),
                key,
                value: CapabilityValue::Integer(value),
                status: SupportStatus::Verified,
                source: ObservationSource::ProviderApi,
                confidence: Confidence::High,
                observed_at,
                expires_at,
                evidence_ref: None,
            });
        }
        append_listed_model_capability_observations(
            model,
            &route.id,
            observed_at,
            expires_at,
            &mut observations,
        )?;
    }
    observations.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(observations)
}

fn append_listed_model_capability_observations(
    model: &ListedModel,
    route_id: &ModelRouteId,
    observed_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    observations: &mut Vec<CapabilityObservation>,
) -> CoreResult<()> {
    let mut supported = model.capabilities.supported.clone();
    supported.sort();
    supported.dedup();
    let authoritative = matches!(
        model.capabilities.parameters,
        OpenRouterSupportedParameterSupport::Exact(_)
    );
    let capabilities = if authoritative {
        vec![
            ListedModelCapability::Reasoning,
            ListedModelCapability::ToolCalling,
            ListedModelCapability::ParallelToolCalling,
            ListedModelCapability::StructuredOutput,
            ListedModelCapability::JsonMode,
            ListedModelCapability::Logprobs,
            ListedModelCapability::Seed,
        ]
    } else {
        supported.clone()
    };
    for capability in capabilities {
        let key = match capability {
            ListedModelCapability::Reasoning => CapabilityKey::Reasoning,
            ListedModelCapability::ToolCalling => CapabilityKey::ToolCalling,
            ListedModelCapability::ParallelToolCalling => CapabilityKey::ParallelToolCalling,
            ListedModelCapability::StructuredOutput => CapabilityKey::StructuredOutput,
            ListedModelCapability::JsonMode => CapabilityKey::JsonMode,
            ListedModelCapability::Logprobs => CapabilityKey::Logprobs,
            ListedModelCapability::Seed => CapabilityKey::Seed,
        };
        let is_supported = supported.contains(&capability);
        let value = if !is_supported {
            CapabilityValue::Boolean(false)
        } else if capability == ListedModelCapability::Reasoning {
            openrouter_reasoning_capability_value(model)?
        } else {
            CapabilityValue::Boolean(true)
        };
        observations.push(CapabilityObservation {
            id: deterministic_capability_observation_id(
                route_id,
                key,
                ObservationSource::ProviderApi,
            ),
            model_route_id: route_id.clone(),
            key,
            value,
            status: if is_supported {
                SupportStatus::Verified
            } else {
                SupportStatus::Unsupported
            },
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at,
            expires_at,
            evidence_ref: None,
        });
    }
    Ok(())
}

fn openrouter_reasoning_capability_value(model: &ListedModel) -> CoreResult<CapabilityValue> {
    let Some(dialect) = openrouter_reasoning_dialect_from_capabilities(&model.capabilities) else {
        return Ok(CapabilityValue::Boolean(true));
    };
    serialize_reasoning_capability(dialect)
}

fn openrouter_reasoning_dialect_from_capabilities(
    capabilities: &ListedModelCapabilities,
) -> Option<ReasoningWireDialect> {
    let parameters = match &capabilities.parameters {
        OpenRouterSupportedParameterSupport::Exact(parameters) => parameters,
        OpenRouterSupportedParameterSupport::NotExposed => return None,
    };
    if parameters.contains(&OpenRouterSupportedParameter::Reasoning) {
        let reasoning = capabilities
            .reasoning
            .clone()
            .unwrap_or(ListedModelReasoningCapability {
                supported_efforts: OpenRouterReasoningEffortSupport::NotExposed,
                default_effort: None,
                default_enabled: None,
                supports_max_tokens: None,
                mandatory: None,
            });
        return Some(ReasoningWireDialect::OpenRouter {
            style: OpenRouterReasoningWireStyle::Unified,
            supported_efforts: reasoning.supported_efforts,
            default_effort: reasoning.default_effort,
            default_enabled: reasoning.default_enabled,
            supports_max_tokens: reasoning.supports_max_tokens,
            mandatory: reasoning.mandatory,
        });
    }
    if !parameters.contains(&OpenRouterSupportedParameter::ReasoningEffort) {
        return None;
    }
    let reasoning = capabilities.reasoning.as_ref()?;
    if matches!(
        reasoning.supported_efforts,
        OpenRouterReasoningEffortSupport::NotExposed
    ) || matches!(
        &reasoning.supported_efforts,
        OpenRouterReasoningEffortSupport::Exact(efforts) if efforts.is_empty()
    ) {
        return None;
    }
    Some(ReasoningWireDialect::OpenRouter {
        style: OpenRouterReasoningWireStyle::LegacyReasoningEffort,
        supported_efforts: reasoning.supported_efforts.clone(),
        default_effort: reasoning.default_effort,
        default_enabled: reasoning.default_enabled,
        supports_max_tokens: reasoning.supports_max_tokens,
        mandatory: reasoning.mandatory,
    })
}

fn serialize_reasoning_capability(dialect: ReasoningWireDialect) -> CoreResult<CapabilityValue> {
    serde_json::to_value(dialect)
        .map(CapabilityValue::Structured)
        .map_err(|error| {
            CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                format!("OpenRouter reasoning metadata could not be normalized: {error}"),
                false,
            )
        })
}

fn deterministic_capability_observation_id(
    model_route_id: &ModelRouteId,
    key: CapabilityKey,
    source: ObservationSource,
) -> ObservationId {
    let identity = format!(
        "lorepia:capability-observation:v1\u{0}{}\u{0}{}\u{0}{}",
        model_route_id.as_str(),
        capability_key_identity(key),
        observation_source_identity(source),
    );
    ObservationId::from(Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string())
}

const fn capability_key_identity(key: CapabilityKey) -> &'static str {
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

const fn observation_source_identity(source: ObservationSource) -> &'static str {
    match source {
        ObservationSource::ProviderApi => "provider_api",
        ObservationSource::OfficialDocumentation => "official_documentation",
        ObservationSource::SignedLorepiaCatalog => "signed_lorepia_catalog",
        ObservationSource::CapabilityProbe => "capability_probe",
        ObservationSource::UserOverride => "user_override",
        ObservationSource::LlmInference => "llm_inference",
    }
}

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

fn validate_provider_template(template: &ProviderTemplate) -> CoreResult<()> {
    if template.manifest_version == 0 {
        return Err(CoreError::invalid(
            "provider template version must be positive",
        ));
    }
    if template.api_family != template.default_manifest.api_family {
        return Err(CoreError::invalid(
            "provider template API family does not match its manifest",
        ));
    }
    validate_connection_fields(&template.connection_fields)?;
    validate_manifest(&template.default_manifest)?;
    Ok(())
}

fn validate_generation_route(
    storage: &Storage,
    model_route_id: &ModelRouteId,
) -> CoreResult<(ModelRoute, ProviderConnection, ProviderTemplate)> {
    let route = storage.get_model_route(model_route_id)?;
    if matches!(
        route.status,
        ModelAvailability::MissingTemporarily
            | ModelAvailability::AccessDenied
            | ModelAvailability::Deprecated
            | ModelAvailability::Retired
    ) {
        return Err(CoreError::invalid(
            "selected model route is not currently available for generation",
        ));
    }
    let connection = storage.get_provider_connection(&route.connection_id)?;
    let template =
        storage.get_provider_template(&connection.template_id, connection.template_version)?;
    validate_provider_template(&template)?;
    if route.api_family != template.api_family {
        return Err(CoreError::invalid(
            "model route API family does not match its provider template",
        ));
    }
    Ok((route, connection, template))
}

fn effective_capability_at(
    storage: &Storage,
    catalog_observations: &[CapabilityObservation],
    model_route_id: &ModelRouteId,
    key: CapabilityKey,
    now: DateTime<Utc>,
) -> CoreResult<Option<EffectiveCapability>> {
    let mut observations = storage
        .list_capability_observations_for_key(model_route_id, key)?
        .into_iter()
        .filter(|observation| observation.source != ObservationSource::SignedLorepiaCatalog)
        .map(|observation| (observation.id.clone(), observation))
        .collect::<HashMap<_, _>>();
    for observation in catalog_observations.iter().filter(|observation| {
        observation.model_route_id == *model_route_id && observation.key == key
    }) {
        observations.insert(observation.id.clone(), observation.clone());
    }
    let observations = observations.into_values().collect::<Vec<_>>();
    if observations.is_empty() {
        return Ok(None);
    }
    let merged = merge_capability_observations(&observations, now)?;
    Ok(Some(EffectiveCapability {
        selected: merged.selected().clone(),
        alternatives: merged.alternatives().to_vec(),
        evaluated_at: now,
        selected_is_stale: merged.selected_is_stale(),
        has_conflict: merged.has_conflict(),
    }))
}

fn validate_capability_wire_metadata(
    route: &ModelRoute,
    template: &ProviderTemplate,
    observation: &CapabilityObservation,
) -> CoreResult<()> {
    let CapabilityValue::Structured(value) = &observation.value else {
        return Ok(());
    };
    match observation.key {
        CapabilityKey::Reasoning => {
            let dialect = parse_reasoning_wire_dialect_metadata(route.api_family, value).map_err(
                |error| {
                    CoreError::invalid(format!(
                        "reasoning capability metadata is invalid for this model route: {error}"
                    ))
                },
            )?;
            if matches!(dialect, ReasoningWireDialect::OpenRouter { .. })
                && !is_exact_built_in_openrouter_template(template)?
            {
                return Err(CoreError::invalid(
                    "OpenRouter reasoning metadata requires the exact built-in OpenRouter template",
                ));
            }
            if dialect == ReasoningWireDialect::Unsupported
                && matches!(
                    observation.status,
                    SupportStatus::Verified | SupportStatus::Documented
                )
            {
                return Err(CoreError::invalid(
                    "a supported reasoning observation requires a concrete wire dialect",
                ));
            }
        }
        CapabilityKey::PromptCaching => {
            let dialect = parse_prompt_cache_wire_dialect_metadata(route.api_family, value)
                .map_err(|error| {
                    CoreError::invalid(format!(
                        "prompt-cache capability metadata is invalid for this model route: {error}"
                    ))
                })?;
            if dialect == PromptCacheWireDialect::Unsupported
                && matches!(
                    observation.status,
                    SupportStatus::Verified | SupportStatus::Documented
                )
            {
                return Err(CoreError::invalid(
                    "a supported prompt-cache observation requires a concrete wire dialect",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn observation_can_drive_wire_mapping(effective: &EffectiveCapability) -> bool {
    !effective.selected_is_stale
        && !effective.has_conflict
        && effective.selected.confidence != Confidence::Low
        && effective.selected.source != ObservationSource::LlmInference
        && matches!(
            effective.selected.status,
            SupportStatus::Verified | SupportStatus::Documented
        )
}

fn effective_reasoning_dialect(
    family: ApiFamily,
    effective: Option<&EffectiveCapability>,
) -> ReasoningWireDialect {
    let Some(effective) = effective.filter(|value| observation_can_drive_wire_mapping(value))
    else {
        return ReasoningWireDialect::Unsupported;
    };
    let CapabilityValue::Structured(value) = &effective.selected.value else {
        return ReasoningWireDialect::Unsupported;
    };
    parse_reasoning_wire_dialect_metadata(family, value)
        .ok()
        .filter(|dialect| *dialect != ReasoningWireDialect::Unsupported)
        .unwrap_or(ReasoningWireDialect::Unsupported)
}

fn effective_prompt_cache_dialect(
    family: ApiFamily,
    effective: Option<&EffectiveCapability>,
) -> PromptCacheWireDialect {
    let Some(effective) = effective.filter(|value| observation_can_drive_wire_mapping(value))
    else {
        return PromptCacheWireDialect::Unsupported;
    };
    let CapabilityValue::Structured(value) = &effective.selected.value else {
        return PromptCacheWireDialect::Unsupported;
    };
    parse_prompt_cache_wire_dialect_metadata(family, value)
        .ok()
        .filter(|dialect| *dialect != PromptCacheWireDialect::Unsupported)
        .unwrap_or(PromptCacheWireDialect::Unsupported)
}

fn is_exact_built_in_openrouter_template(template: &ProviderTemplate) -> CoreResult<bool> {
    let canonical = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)?;
    Ok(template.source == TemplateSource::BuiltIn
        && template.id == canonical.id
        && template.manifest_version == canonical.manifest_version)
}

fn effective_route_parameter_specs(
    route: &ModelRoute,
    template: &ProviderTemplate,
    base_specs: &[ParameterSpec],
    signed_model_specs: &[ParameterSpec],
    evaluated_at: DateTime<Utc>,
) -> CoreResult<Vec<ParameterSpec>> {
    if !is_exact_built_in_openrouter_template(template)? {
        return Ok(base_specs.to_vec());
    }
    if route.status != ModelAvailability::Available {
        return Ok(Vec::new());
    }
    let Some(metadata) = fresh_openrouter_route_metadata(route, template, evaluated_at)? else {
        return Ok(openrouter_safe_signed_parameter_specs(signed_model_specs));
    };
    let OpenRouterSupportedParameterSupport::Exact(supported) = metadata.capabilities.parameters
    else {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "fresh OpenRouter provider metadata lacks exact supported parameters",
            false,
        ));
    };
    Ok(intersect_openrouter_parameter_specs(
        base_specs,
        &supported,
        metadata.max_output_tokens,
    ))
}

struct FreshOpenRouterRouteMetadata {
    capabilities: ListedModelCapabilities,
    max_output_tokens: Option<u64>,
    observed_at: DateTime<Utc>,
}

fn fresh_openrouter_route_metadata(
    route: &ModelRoute,
    template: &ProviderTemplate,
    evaluated_at: DateTime<Utc>,
) -> CoreResult<Option<FreshOpenRouterRouteMetadata>> {
    if !is_exact_built_in_openrouter_template(template)?
        || route.status != ModelAvailability::Available
        || route.metadata_source != ModelMetadataSource::ProviderApi
    {
        return Ok(None);
    }
    let Some(observed_at) = route.metadata_observed_at else {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "available ProviderApi route lacks a metadata observation time",
            false,
        ));
    };
    if observed_at > evaluated_at {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider model metadata has a future observation time",
            false,
        ));
    }
    match (
        route.last_reconciled_sync_job_id.as_ref(),
        route.metadata_sync_job_id.as_ref(),
    ) {
        (None, None) => {}
        (Some(reconciled), Some(metadata)) if reconciled == metadata => {}
        _ => {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider model metadata synchronization provenance is inconsistent",
                false,
            ));
        }
    }
    let Some(metadata) = route.raw_metadata.as_ref() else {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "available ProviderApi route lacks normalized model metadata",
            false,
        ));
    };
    validate_provider_api_route_metadata(Some(metadata)).map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!(
                "provider model metadata is not canonical: {}",
                error.message
            ),
            false,
        )
    })?;
    let value = serde_json::from_str::<serde_json::Value>(metadata.as_str()).map_err(|_| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider model metadata is invalid JSON",
            false,
        )
    })?;
    let capabilities = serde_json::from_value::<ListedModelCapabilities>(
        value.get("capabilities").cloned().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "provider model metadata lacks capabilities",
                false,
            )
        })?,
    )
    .map_err(|_| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "provider model capability metadata is invalid",
            false,
        )
    })?;
    if !matches!(
        capabilities.parameters,
        OpenRouterSupportedParameterSupport::Exact(_)
    ) {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "ProviderApi OpenRouter route lacks exact supported parameters",
            false,
        ));
    }
    let max_output_tokens = value
        .get("max_output_tokens")
        .and_then(serde_json::Value::as_u64);
    if observed_at
        .checked_add_signed(PROVIDER_API_CAPABILITY_FRESHNESS)
        .is_none_or(|expires_at| expires_at <= evaluated_at)
    {
        return Ok(None);
    }
    Ok(Some(FreshOpenRouterRouteMetadata {
        capabilities,
        max_output_tokens,
        observed_at,
    }))
}

fn openrouter_safe_signed_parameter_specs(specs: &[ParameterSpec]) -> Vec<ParameterSpec> {
    let mut safe_specs = Vec::new();
    let mut output_spec = None::<ParameterSpec>;
    let mut output_uses_completion_alias = false;
    for spec in specs {
        if spec.provider_mapping.target != ProviderParameterTarget::RequestBody {
            continue;
        }
        match spec.provider_mapping.field_name.as_str() {
            "max_tokens" | "max_completion_tokens" => {
                let uses_completion_alias =
                    spec.provider_mapping.field_name == "max_completion_tokens";
                let replace = output_spec.as_ref().is_none_or(|current| {
                    (uses_completion_alias && !output_uses_completion_alias)
                        || (uses_completion_alias == output_uses_completion_alias
                            && current.id.as_str() != "max_output_tokens"
                            && spec.id.as_str() == "max_output_tokens")
                });
                if replace {
                    output_spec = Some(spec.clone());
                    output_uses_completion_alias = uses_completion_alias;
                }
            }
            "temperature" | "top_p" | "frequency_penalty" | "presence_penalty" | "stop"
            | "seed"
                if !safe_specs.iter().any(|existing: &ParameterSpec| {
                    existing.id == spec.id || existing.provider_mapping == spec.provider_mapping
                }) =>
            {
                safe_specs.push(spec.clone());
            }
            _ => {}
        }
    }
    if let Some(mut output) = output_spec {
        let safe_maximum = f64::from(u32::MAX);
        output.id = ParameterId::from("max_output_tokens");
        output.label_key.clear();
        output
            .label_key
            .push_str("provider.parameter.max_output_tokens");
        output.description_key =
            Some("provider.parameter.max_output_tokens.description".to_owned());
        let output_field = if output_uses_completion_alias {
            "max_completion_tokens"
        } else {
            "max_tokens"
        };
        output.provider_mapping.field_name.clear();
        output.provider_mapping.field_name.push_str(output_field);
        output.maximum = Some(
            output
                .maximum
                .map_or(safe_maximum, |maximum| maximum.min(safe_maximum)),
        );
        if output.minimum.is_none_or(|minimum| minimum <= safe_maximum) {
            safe_specs.push(output);
        }
    }
    safe_specs
}

fn intersect_openrouter_parameter_specs(
    base_specs: &[ParameterSpec],
    supported: &[OpenRouterSupportedParameter],
    max_output_tokens: Option<u64>,
) -> Vec<ParameterSpec> {
    let mut specs = Vec::new();
    for spec in base_specs
        .iter()
        .filter(|spec| {
            !matches!(
                spec.provider_mapping.field_name.as_str(),
                "max_tokens" | "max_completion_tokens"
            )
        })
        .filter_map(|spec| openrouter_supported_parameter_spec(spec, supported))
    {
        if let Some(existing) = specs.iter_mut().find(|existing: &&mut ParameterSpec| {
            existing.provider_mapping == spec.provider_mapping
        }) {
            if spec.id.as_str() == "max_output_tokens"
                && existing.id.as_str() != "max_output_tokens"
            {
                *existing = spec;
            }
        } else {
            specs.push(spec);
        }
    }
    if let Some(spec) =
        select_openrouter_output_token_spec(base_specs, supported, max_output_tokens)
    {
        specs.push(spec);
    }
    for spec in openrouter_compiled_parameter_specs(supported) {
        if !specs.iter().any(|existing| {
            existing.id == spec.id || existing.provider_mapping == spec.provider_mapping
        }) {
            specs.push(spec);
        }
    }
    specs
}

fn select_openrouter_output_token_spec(
    base_specs: &[ParameterSpec],
    supported: &[OpenRouterSupportedParameter],
    max_output_tokens: Option<u64>,
) -> Option<ParameterSpec> {
    let preferred_field = if supported.contains(&OpenRouterSupportedParameter::MaxCompletionTokens)
    {
        "max_completion_tokens"
    } else if supported.contains(&OpenRouterSupportedParameter::MaxTokens) {
        "max_tokens"
    } else {
        return None;
    };
    let candidates = base_specs.iter().filter(|spec| {
        spec.provider_mapping.target == ProviderParameterTarget::RequestBody
            && matches!(
                spec.provider_mapping.field_name.as_str(),
                "max_tokens" | "max_completion_tokens"
            )
    });
    let selected = candidates
        .clone()
        .filter(|spec| spec.provider_mapping.field_name == preferred_field)
        .min_by_key(|spec| spec.id.as_str() != "max_output_tokens")
        .or_else(|| candidates.min_by_key(|spec| spec.id.as_str() != "max_output_tokens"))?;
    openrouter_output_token_spec(selected, supported, max_output_tokens)
}

fn openrouter_supported_parameter_spec(
    spec: &ParameterSpec,
    supported: &[OpenRouterSupportedParameter],
) -> Option<ParameterSpec> {
    if spec.provider_mapping.target != ProviderParameterTarget::RequestBody {
        return None;
    }
    let field = spec.provider_mapping.field_name.as_str();
    let parameter = match field {
        "temperature" => OpenRouterSupportedParameter::Temperature,
        "top_p" => OpenRouterSupportedParameter::TopP,
        "frequency_penalty" => OpenRouterSupportedParameter::FrequencyPenalty,
        "presence_penalty" => OpenRouterSupportedParameter::PresencePenalty,
        "stop" => OpenRouterSupportedParameter::Stop,
        "seed" => OpenRouterSupportedParameter::Seed,
        _ => return None,
    };
    supported.contains(&parameter).then(|| spec.clone())
}

fn openrouter_output_token_spec(
    spec: &ParameterSpec,
    supported: &[OpenRouterSupportedParameter],
    max_output_tokens: Option<u64>,
) -> Option<ParameterSpec> {
    let supports_max_tokens = supported.contains(&OpenRouterSupportedParameter::MaxTokens);
    let supports_max_completion =
        supported.contains(&OpenRouterSupportedParameter::MaxCompletionTokens);
    let field = match (supports_max_tokens, supports_max_completion) {
        (_, true) => "max_completion_tokens",
        (true, false) => "max_tokens",
        (false, false) => return None,
    };
    let mut normalized = spec.clone();
    normalized.id = ParameterId::from("max_output_tokens");
    normalized.label_key.clear();
    normalized
        .label_key
        .push_str("provider.parameter.max_output_tokens");
    normalized.description_key =
        Some("provider.parameter.max_output_tokens.description".to_owned());
    normalized.provider_mapping.field_name.clear();
    normalized.provider_mapping.field_name.push_str(field);
    let provider_maximum = f64::from(
        max_output_tokens
            .and_then(|maximum| u32::try_from(maximum).ok())
            .unwrap_or(u32::MAX),
    );
    normalized.maximum = Some(
        normalized
            .maximum
            .map_or(provider_maximum, |maximum| maximum.min(provider_maximum)),
    );
    if normalized
        .minimum
        .is_some_and(|minimum| minimum > provider_maximum)
    {
        return None;
    }
    Some(normalized)
}

fn openrouter_compiled_parameter_specs(
    supported: &[OpenRouterSupportedParameter],
) -> Vec<ParameterSpec> {
    [
        (
            OpenRouterSupportedParameter::FrequencyPenalty,
            compiled_openrouter_parameter_spec(
                "frequency_penalty",
                "frequency_penalty",
                ParameterType::Number,
                Some(-2.0),
                Some(2.0),
                None,
                UiParameterLevel::Advanced,
            ),
        ),
        (
            OpenRouterSupportedParameter::PresencePenalty,
            compiled_openrouter_parameter_spec(
                "presence_penalty",
                "presence_penalty",
                ParameterType::Number,
                Some(-2.0),
                Some(2.0),
                None,
                UiParameterLevel::Advanced,
            ),
        ),
        (
            OpenRouterSupportedParameter::Stop,
            compiled_openrouter_parameter_spec(
                "stop",
                "stop",
                ParameterType::StopSequenceList,
                None,
                None,
                None,
                UiParameterLevel::Advanced,
            ),
        ),
        (
            OpenRouterSupportedParameter::Seed,
            compiled_openrouter_parameter_spec(
                "seed",
                "seed",
                ParameterType::Integer,
                None,
                None,
                Some(1.0),
                UiParameterLevel::Advanced,
            ),
        ),
    ]
    .into_iter()
    .filter_map(|(parameter, spec)| supported.contains(&parameter).then_some(spec))
    .collect()
}

#[allow(clippy::too_many_arguments)]
fn compiled_openrouter_parameter_spec(
    id: &str,
    field_name: &str,
    value_type: ParameterType,
    minimum: Option<f64>,
    maximum: Option<f64>,
    step: Option<f64>,
    level: UiParameterLevel,
) -> ParameterSpec {
    ParameterSpec {
        id: ParameterId::from(id),
        label_key: format!("provider.parameter.{id}"),
        description_key: Some(format!("provider.parameter.{id}.description")),
        value_type,
        allowed_values: Vec::new(),
        minimum,
        maximum,
        step,
        default_mode: ParameterDefaultMode::ProviderDefault,
        visibility: None,
        conflicts: Vec::new(),
        provider_mapping: ProviderParameterMapping {
            target: ProviderParameterTarget::RequestBody,
            field_name: field_name.to_owned(),
        },
        level,
    }
}

fn generation_preset_control_context(
    core: &Core,
    preset: &GenerationPreset,
) -> CoreResult<GenerationPresetControlContext> {
    let storage = &core.inner.storage;
    let (route, connection, template) = validate_generation_route(storage, &preset.model_route_id)?;
    let evaluated_at = Utc::now();
    let catalog = core
        .operational_provider_catalog_projection_at(evaluated_at)?
        .route_projection(&route, &connection.template_id);
    let base_parameter_specs = if catalog.matched {
        catalog.parameters.clone()
    } else {
        template.default_manifest.parameters.clone()
    };
    let parameter_specs = effective_route_parameter_specs(
        &route,
        &template,
        &base_parameter_specs,
        &catalog.signed_parameters,
        evaluated_at,
    )?;
    let parameter_engine =
        ParameterEngine::from_manifest_specs_for_family(route.api_family, &parameter_specs)
            .map_err(|error| {
                CoreError::invalid(format!(
                    "provider parameter manifest is invalid for this model route: {error}"
                ))
            })?;
    let reasoning = ReasoningSettings::from(&preset.reasoning);
    let prompt_cache = PromptCacheSettings::from(&preset.prompt_cache);
    let reasoning_capability = effective_capability_at(
        storage,
        &catalog.capability_observations,
        &route.id,
        CapabilityKey::Reasoning,
        evaluated_at,
    )?;
    let cache_capability = effective_capability_at(
        storage,
        &catalog.capability_observations,
        &route.id,
        CapabilityKey::PromptCaching,
        evaluated_at,
    )?;
    let mut reasoning_dialect =
        effective_reasoning_dialect(route.api_family, reasoning_capability.as_ref());
    if matches!(reasoning_dialect, ReasoningWireDialect::OpenRouter { .. }) {
        let exact_template = is_exact_built_in_openrouter_template(&template)?;
        let metadata_matches_route =
            fresh_openrouter_route_metadata(&route, &template, evaluated_at)?.is_some_and(
                |metadata| {
                    let observation_time_matches =
                        reasoning_capability.as_ref().is_some_and(|capability| {
                            capability.selected.source != ObservationSource::ProviderApi
                                || capability.selected.observed_at == metadata.observed_at
                        });
                    observation_time_matches
                        && openrouter_reasoning_dialect_from_capabilities(&metadata.capabilities)
                            .is_some_and(|dialect| dialect == reasoning_dialect)
                },
            );
        if !exact_template || !metadata_matches_route {
            reasoning_dialect = ReasoningWireDialect::Unsupported;
        }
    }
    let cache_dialect = effective_prompt_cache_dialect(route.api_family, cache_capability.as_ref());

    Ok(GenerationPresetControlContext {
        route,
        connection,
        template,
        parameter_engine,
        reasoning,
        prompt_cache,
        reasoning_dialect,
        cache_dialect,
    })
}

fn validate_generation_preset_candidate_plan(
    core: &Core,
    preset: &GenerationPreset,
) -> CoreResult<ValidatedGenerationTarget> {
    let context = generation_preset_control_context(core, preset)?;
    validate_opaque_reasoning_state_support(
        &context.template,
        &context.connection,
        &context.reasoning,
    )?;
    // A family name alone is not evidence that a particular model supports a
    // reasoning or cache control. Only a fresh, non-conflicting, sufficiently
    // confident observation with an exact structured dialect can enable those
    // controls. Provider-default remains the only lossless fallback.
    let request_plan = validate_and_build_provider_request_plan(
        &context.parameter_engine,
        context.route.api_family,
        &preset.values,
        &context.reasoning,
        &context.reasoning_dialect,
        &context.prompt_cache,
        context.cache_dialect,
    )
    .map_err(|error| {
        CoreError::invalid(format!(
            "generation preset cannot be represented by this model route: {error}"
        ))
    })?;
    let developer_capability = match context.route.api_family {
        ApiFamily::OpenAiResponses => DeveloperRoleCapability::Supported,
        ApiFamily::OpenAiChatCompletions => DeveloperRoleCapability::Unknown,
        ApiFamily::AnthropicMessages
        | ApiFamily::GeminiGenerateContent
        | ApiFamily::OllamaNative => DeveloperRoleCapability::Unsupported,
    };
    let parameter_evaluation = context.parameter_engine.evaluate(&preset.values);
    let supports_temperature = parameter_evaluation
        .editor
        .basic
        .iter()
        .chain(&parameter_evaluation.editor.advanced)
        .chain(&parameter_evaluation.editor.expert)
        .any(|control| {
            control.id.as_str().eq_ignore_ascii_case("temperature")
                && control.visible
                && control.enabled
        });
    let prompt_wire_contract = PromptRouteWireContract {
        model_route_id: context.route.id.clone(),
        generation_preset_id: preset.id.clone(),
        model: context.route.model_id.clone(),
        api_family: context.route.api_family,
        developer_capability,
        cache_dialect: context.cache_dialect,
        request_plan_sha256: canonical_value_sha256(&request_plan, "provider request plan")?,
        generation_preset_sha256: canonical_value_sha256(preset, "generation preset")?,
        configured_max_output_tokens: configured_max_output_tokens(&request_plan),
        context_limit_tokens: observed_positive_integer_capability(
            core,
            &context.route.id,
            CapabilityKey::ContextWindow,
        )?,
        observed_max_output_tokens: observed_positive_integer_capability(
            core,
            &context.route.id,
            CapabilityKey::MaxOutputTokens,
        )?,
        supports_temperature,
        reasoning_effort_applied: None,
    };

    Ok(ValidatedGenerationTarget {
        route: context.route,
        connection: context.connection,
        template: context.template,
        request_plan,
        prompt_wire_contract,
    })
}

fn validate_opaque_reasoning_state_support(
    template: &ProviderTemplate,
    connection: &ProviderConnection,
    reasoning: &ReasoningSettings,
) -> CoreResult<()> {
    if !reasoning.preserve_opaque_state {
        return Ok(());
    }
    if !AdapterRegistry::template_supports_opaque_reasoning_state(template) {
        let message = if template.api_family == ApiFamily::GeminiGenerateContent {
            GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR
        } else {
            OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR
        };
        return Err(CoreError::invalid(message));
    }
    if connection.credential_ref.is_some() {
        return Err(CoreError::invalid(OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR));
    }
    Ok(())
}

fn validate_generation_target_plan(
    core: &Core,
    target: &GenerationTarget,
) -> CoreResult<ValidatedGenerationTarget> {
    validate_generation_target_plan_with_reasoning_effort(core, target, None)
}

fn validate_generation_target_plan_with_reasoning_effort(
    core: &Core,
    target: &GenerationTarget,
    requested_reasoning_effort: Option<GenerationReasoningEffort>,
) -> CoreResult<ValidatedGenerationTarget> {
    let mut preset = core
        .inner
        .storage
        .get_generation_preset(&target.generation_preset_id)?;
    if preset.model_route_id != target.model_route_id {
        return Err(CoreError::invalid(
            "generation preset does not belong to the selected model route",
        ));
    }
    let (_, connection, _) =
        validate_generation_route(&core.inner.storage, &preset.model_route_id)?;
    if connection.credential_ref.is_some() {
        preset.reasoning.preserve_opaque_state = false;
    }
    let validated = validate_generation_preset_candidate_plan(core, &preset)?;
    let Some(effort) = requested_reasoning_effort else {
        return Ok(validated);
    };

    let original_mode = preset.reasoning.mode;
    let mut candidate_modes = Vec::with_capacity(3);
    if matches!(
        original_mode,
        GenerationReasoningMode::Enabled | GenerationReasoningMode::Automatic
    ) {
        candidate_modes.push(original_mode);
    }
    for mode in [
        GenerationReasoningMode::Enabled,
        GenerationReasoningMode::Automatic,
    ] {
        if !candidate_modes.contains(&mode) {
            candidate_modes.push(mode);
        }
    }
    for mode in candidate_modes {
        let mut candidate = preset.clone();
        candidate.reasoning.mode = mode;
        candidate.reasoning.effort = Some(effort);
        if let Ok(mut candidate) = validate_generation_preset_candidate_plan(core, &candidate) {
            candidate.prompt_wire_contract.reasoning_effort_applied = Some(effort);
            return Ok(candidate);
        }
    }

    // A quick setting is a bounded overlay, not an unvalidated generic
    // parameter patch. Retain the original exact request plan when this route
    // cannot represent the requested effort; prompt diagnostics report the
    // omission.
    Ok(validated)
}

pub(crate) fn resolve_generation_target(
    core: &Core,
    target: &GenerationTarget,
) -> CoreResult<ResolvedGenerationTarget> {
    let validated = validate_generation_target_plan(core, target)?;
    build_resolved_generation_target(validated)
}

pub(crate) fn prompt_route_wire_contract(
    core: &Core,
    target: &GenerationTarget,
) -> CoreResult<PromptRouteWireContract> {
    let validated = validate_generation_target_plan(core, target)?;
    Ok(validated.prompt_wire_contract)
}

pub(crate) fn prompt_route_wire_contract_with_reasoning_effort(
    core: &Core,
    target: &GenerationTarget,
    requested_reasoning_effort: Option<GenerationReasoningEffort>,
) -> CoreResult<PromptRouteWireContract> {
    let validated = validate_generation_target_plan_with_reasoning_effort(
        core,
        target,
        requested_reasoning_effort,
    )?;
    Ok(validated.prompt_wire_contract)
}

pub(crate) fn prompt_route_supports_temperature(
    core: &Core,
    target: &GenerationTarget,
) -> CoreResult<bool> {
    Ok(validate_generation_target_plan(core, target)?
        .prompt_wire_contract
        .supports_temperature)
}

#[cfg(test)]
fn resolve_generation_target_with_connection_credential(
    core: &Core,
    target: &GenerationTarget,
    credential: ConnectionBoundCredential,
) -> CoreResult<(ResolvedGenerationTarget, GenerationCredential)> {
    let validated = validate_generation_target_plan(core, target)?;
    validate_connection_credential_binding(&validated.connection, &credential)?;
    let resolved = build_resolved_generation_target(validated)?;
    Ok((resolved, credential.into()))
}

fn preflight_generation_target_connection_credential(
    core: &Core,
    target: &GenerationTarget,
    credential: &ConnectionBoundCredential,
) -> CoreResult<()> {
    let validated = validate_generation_target_plan(core, target)?;
    validate_connection_credential_binding(&validated.connection, credential)
}

fn validate_connection_credential_binding(
    connection: &ProviderConnection,
    credential: &ConnectionBoundCredential,
) -> CoreResult<()> {
    let credential_reference = connection.credential_ref.as_ref();
    if credential.connection_id != connection.id
        || credential_reference
            .is_some_and(|reference| reference.as_str() != credential.connection_id.as_str())
    {
        return Err(CoreError::invalid(
            "credential does not belong to the selected provider connection",
        ));
    }
    let has_credential = credential
        .value
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    match (credential_reference.is_some(), has_credential) {
        (true, false) => {
            return Err(CoreError::new(
                CoreErrorCode::ProviderAuthFailed,
                "provider credential is required",
                false,
            ));
        }
        (false, true) => {
            return Err(CoreError::invalid(
                "this provider connection does not permit a credential",
            ));
        }
        _ => {}
    }
    Ok(())
}

fn build_resolved_generation_target(
    validated: ValidatedGenerationTarget,
) -> CoreResult<ResolvedGenerationTarget> {
    let preserve_opaque_reasoning_state = validated.connection.credential_ref.is_none()
        && validated.request_plan.preserves_opaque_reasoning_state();
    let prompt_wire_contract = validated.prompt_wire_contract;
    let provider = AdapterRegistry::new().build_provider_for_route_with_plan(
        &validated.template,
        &validated.connection,
        &validated.route,
        Some(validated.request_plan),
    )?;

    Ok(ResolvedGenerationTarget {
        model: validated.route.model_id,
        provider,
        api_family: validated.route.api_family,
        connection_id: validated.connection.id,
        preserve_opaque_reasoning_state,
        prompt_wire_contract,
    })
}

fn canonical_value_sha256(value: &impl Serialize, label: &str) -> CoreResult<String> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| CoreError::internal(format!("cannot encode {label}: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn validate_generation_operation_nonce(value: &str) -> CoreResult<&str> {
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_GENERATION_OPERATION_NONCE_BYTES
        || value.chars().count() > MAX_GENERATION_OPERATION_NONCE_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(
            "generation operation nonce is empty, unsafe, or exceeds its size limit",
        ));
    }
    Ok(value)
}

#[cfg(test)]
fn direct_model_provider_target_authority(
    model: &str,
) -> CoreResult<GenerationProviderTargetAuthority> {
    let digest = format!("{:x}", Sha256::digest(model.as_bytes()));
    Ok(GenerationProviderTargetAuthority::DirectModel {
        model_sha256: Sha256Digest::parse(digest).map_err(CoreError::invalid)?,
    })
}

#[cfg(test)]
fn direct_model_temporal_context(model: &str) -> CoreResult<GenerationProviderTemporalContext> {
    let authority = direct_model_provider_target_authority(model)?;
    let GenerationProviderTargetAuthority::DirectModel { model_sha256 } = &authority else {
        unreachable!("direct-model authority constructor returned another variant");
    };
    Ok(GenerationProviderTemporalContext {
        operation_target: GenerationActionTargetIdentity::DirectModel {
            model_sha256: model_sha256.as_str().to_owned(),
        },
        authority,
    })
}

fn provider_profile_target_authority(
    profile: &ProviderProfile,
) -> CoreResult<GenerationProviderTargetAuthority> {
    let digest = canonical_value_sha256(
        &ProviderProfileDispatchAuthoritySnapshot {
            schema_version: 1,
            provider_profile_id: &profile.id,
            base_url: &profile.base_url,
            model: &profile.model,
            timeout_seconds: profile.timeout_seconds,
        },
        "provider profile dispatch authority",
    )?;
    Ok(GenerationProviderTargetAuthority::ProviderProfile {
        provider_profile_id: profile.id.clone(),
        dispatch_snapshot_sha256: Sha256Digest::parse(digest).map_err(CoreError::invalid)?,
    })
}

fn provider_profile_temporal_context(
    profile: &ProviderProfile,
) -> CoreResult<GenerationProviderTemporalContext> {
    Ok(GenerationProviderTemporalContext {
        operation_target: GenerationActionTargetIdentity::ProviderProfile {
            provider_profile_id: profile.id.clone(),
        },
        authority: provider_profile_target_authority(profile)?,
    })
}

fn generation_target_provider_authority(
    target: &GenerationTarget,
    validated: &ValidatedGenerationTarget,
) -> CoreResult<GenerationProviderTargetAuthority> {
    let digest = canonical_value_sha256(
        &GenerationTargetResolutionAuthoritySnapshot {
            schema_version: 1,
            target,
            route: &validated.route,
            connection: &validated.connection,
            template: &validated.template,
            request_plan: &validated.request_plan,
            prompt_wire_contract: &validated.prompt_wire_contract,
        },
        "generation target resolution authority",
    )?;
    Ok(GenerationProviderTargetAuthority::GenerationTarget {
        target: target.clone(),
        resolved_snapshot_sha256: Sha256Digest::parse(digest).map_err(CoreError::invalid)?,
    })
}

#[cfg(test)]
fn generation_target_temporal_context(
    target: &GenerationTarget,
    validated: &ValidatedGenerationTarget,
) -> CoreResult<GenerationProviderTemporalContext> {
    Ok(GenerationProviderTemporalContext {
        operation_target: GenerationActionTargetIdentity::GenerationTarget {
            model_route_id: target.model_route_id.clone(),
            generation_preset_id: target.generation_preset_id.clone(),
        },
        authority: generation_target_provider_authority(target, validated)?,
    })
}

fn require_generation_provider_target_authority(
    attempt: &lorepia_storage::StoredGenerationAttempt,
    current: &GenerationProviderTargetAuthority,
) -> CoreResult<()> {
    let sealed = generation_attempt_prompt_authority(attempt)?
        .provider_target_authority
        .as_ref()
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::InvalidInput,
                "legacy generation attempt has no provider target authority; start a new generation operation",
                true,
            )
        })?;
    if sealed != current {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "provider configuration changed after generation review; start a new generation operation",
            true,
        ));
    }
    Ok(())
}

fn message_action_provider_target_authority(
    core: &Core,
    action: &PreparedMessageGenerationAction,
    model: &str,
    generation_target: Option<&GenerationTarget>,
    prompt_wire_contract: Option<&PromptRouteWireContract>,
) -> CoreResult<GenerationProviderTargetAuthority> {
    #[cfg(not(test))]
    let _ = model;
    match &action.target {
        GenerationActionTargetIdentity::ProviderProfile {
            provider_profile_id,
        } => {
            if generation_target.is_some() {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "provider-profile action unexpectedly carried a catalog target",
                    false,
                ));
            }
            let profile = core
                .inner
                .storage
                .get_provider_profile(provider_profile_id)?;
            provider_profile_target_authority(&profile)
        }
        GenerationActionTargetIdentity::GenerationTarget {
            model_route_id,
            generation_preset_id,
        } => {
            let target = generation_target.ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "catalog-target action lost its provider target",
                    false,
                )
            })?;
            if &target.model_route_id != model_route_id
                || &target.generation_preset_id != generation_preset_id
            {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "catalog-target action differs from its operation identity",
                    false,
                ));
            }
            let validated = validate_generation_target_plan_with_reasoning_effort(
                core,
                target,
                prompt_wire_contract.and_then(|contract| contract.reasoning_effort_applied),
            )?;
            generation_target_provider_authority(target, &validated)
        }
        #[cfg(test)]
        GenerationActionTargetIdentity::DirectModel { model_sha256 } => {
            let authority = direct_model_provider_target_authority(model)?;
            let GenerationProviderTargetAuthority::DirectModel {
                model_sha256: current,
            } = &authority
            else {
                unreachable!("direct-model authority constructor returned another variant");
            };
            if current.as_str() != model_sha256 {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "direct-model action differs from its operation identity",
                    false,
                ));
            }
            Ok(authority)
        }
    }
}

fn reviewed_prompt_session_seed(base_request_fingerprint_sha256: &Sha256Digest) -> u64 {
    const SQLITE_SIGNED_INTEGER_MAX: u64 = 0x7fff_ffff_ffff_ffff;
    let digest = Sha256::digest(
        format!(
            "reviewed-prompt-session-seed-v2:{}",
            base_request_fingerprint_sha256.as_str()
        )
        .as_bytes(),
    );
    let raw_seed = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 always contains eight seed bytes"),
    );
    raw_seed & SQLITE_SIGNED_INTEGER_MAX
}

fn validate_reviewed_generation_attempt_id(
    expected: &GenerationId,
    actual: &GenerationId,
) -> CoreResult<()> {
    if expected != actual {
        return Err(CoreError::invalid(
            "reviewed generation attempt changed; resolve a new preview before sending",
        ));
    }
    Ok(())
}

fn configured_max_output_tokens(plan: &ProviderRequestPlan) -> Option<u32> {
    const OUTPUT_TOKEN_PATHS: [&str; 5] = [
        "max_output_tokens",
        "max_tokens",
        "max_completion_tokens",
        "generationConfig.maxOutputTokens",
        "options.num_predict",
    ];
    plan.body_patches()
        .iter()
        .find(|patch| OUTPUT_TOKEN_PATHS.contains(&patch.path()))
        .and_then(|patch| patch.value().as_u64())
        .and_then(|tokens| u32::try_from(tokens).ok())
        .filter(|tokens| *tokens > 0)
}

fn observed_positive_integer_capability(
    core: &Core,
    model_route_id: &ModelRouteId,
    key: CapabilityKey,
) -> CoreResult<Option<u32>> {
    Ok(core
        .effective_capability(model_route_id, key)?
        .filter(|capability| !capability.has_conflict && !capability.selected_is_stale)
        .and_then(|capability| match capability.selected.value {
            CapabilityValue::Integer(value) => u32::try_from(value).ok(),
            _ => None,
        })
        .filter(|value| *value > 0))
}

fn validate_settings_generation_target(core: &Core, settings: &AppSettings) -> CoreResult<()> {
    match (
        settings.selected_model_route_id.as_ref(),
        settings.selected_generation_preset_id.as_ref(),
    ) {
        (None, None) => Ok(()),
        (Some(model_route_id), Some(generation_preset_id)) => {
            validate_generation_target_plan(
                core,
                &GenerationTarget {
                    model_route_id: model_route_id.clone(),
                    generation_preset_id: generation_preset_id.clone(),
                },
            )?;
            Ok(())
        }
        _ => Err(CoreError::invalid(
            "model route and generation preset must be selected together",
        )),
    }
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

fn validate_action_replacement(
    action: MessageGenerationAction,
    replacement_text: Option<&str>,
) -> CoreResult<Option<&str>> {
    match replacement_text {
        Some(text) => validate_user_message_text(text).map(Some),
        None if action == MessageGenerationAction::EditUser => {
            Err(CoreError::invalid("message text cannot be empty"))
        }
        None => Ok(None),
    }
}

const fn generation_action_name(action: MessageGenerationAction) -> &'static str {
    match action {
        MessageGenerationAction::EditUser => "edit_user",
        MessageGenerationAction::RegenerateAssistant => "regenerate_assistant",
    }
}

fn reviewed_prompt_generation_record(
    plan_request: &crate::PromptPlanRequest,
    mode: ConversationMode,
    resolved: &ResolvedGenerationTarget,
    generation_id: &GenerationId,
    user_message: &Message,
    assistant_message: &Message,
) -> GenerationRecord {
    GenerationRecord {
        id: generation_id.clone(),
        conversation_id: plan_request.conversation_id.clone(),
        branch_id: plan_request.branch_id.clone(),
        user_message_id: user_message.id.clone(),
        assistant_message_id: Some(assistant_message.id.clone()),
        mode,
        model: resolved.model.clone(),
        model_route_id: Some(plan_request.generation_target.model_route_id.clone()),
        generation_preset_id: Some(plan_request.generation_target.generation_preset_id.clone()),
        provider_family: Some(resolved.api_family),
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
        started_at: assistant_message.created_at,
        finished_at: None,
    }
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
mod tests {
    use std::{
        io::{Read, Write},
        net::{IpAddr, TcpListener, TcpStream},
        path::Path,
        process::Command,
        sync::{Arc, Barrier, mpsc as std_mpsc},
        thread,
        time::{Duration, Instant},
    };

    use async_trait::async_trait;
    use lorepia_domain::{
        ActivationRule, AuxiliaryTaskKind, BlockSource, BuiltInTemplateValue,
        ConnectionConfigEntry, ConnectionConfigValue, ContentCapability, ContentModule,
        ContentModuleId, ControlId, ControlKind, ControlSpec, DiceExpression,
        GenerationPromptCacheMode, GenerationPromptCacheSettings, GenerationPromptCacheTtl,
        GenerationReasoningEffort, GenerationReasoningMode, GenerationReasoningSettings,
        GenerationReasoningSummary, GenerationUsage, HistorySelector, InstructionAuthority,
        InteractionAction, InteractionEffect, InteractionEvent, InteractionProposalDecision,
        InteractionProposalStatus, InteractionRule, InteractionRuleId, InteractionRuleSet,
        InteractionRuleSetId, KnowledgeBook, KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId,
        KnowledgePlacement, MemoryKind, MemoryProfile, MemoryProfileId, MemoryRecord,
        MemoryRecordId, MergePolicy, ModelSyncState, ModuleBindingId, ModuleRevisionResolutionMode,
        ModuleScope, OpenRouterReasoningDetail, OpenRouterReasoningTopology, OverflowPolicy,
        PackageMetadata, PlacementZone, PresetMetadata, PromptBlock, PromptBlockId,
        PromptBlockKind, PromptContextSnapshotV1, PromptPreset, ProposalSpec, Provenance,
        ProviderCapabilities, RateLimit, ResolvedPromptPlan, RoleHint, SafeRegex, SafeTemplate,
        SourceKind, SummarySchemaId, TaskProfile, TaskProfileId, TemplatePart, TemplateSlot,
        TokenBudget, TokenPolicy, TransformRule, TransformRuleId, TransformSetId, ValueExpr,
        VariableId, VariableRef, VariableScope, VariableType, VariableValue, VersionedJson,
    };
    use lorepia_providers::{EmbeddingPurpose, ProviderEvent, ProviderEventSender, StaticProvider};
    use lorepia_storage::{
        GenerationAttemptStatus, KnowledgeEmbeddingWrite, LifecycleOccurrenceKind,
        MemoryQueryEmbeddingIntent, PromptPresetBinding, PromptResponseLength,
        ProviderCredentialObservedStatus, ProviderCredentialOperationKind,
    };
    use tempfile::{NamedTempFile, TempDir, tempdir};

    use super::*;
    use crate::{
        ContentModuleActivationRequest, ContentModuleBindingDraft, ContentModuleRuntimeTarget,
        MessagePresentation, ModuleActivationApproval, ModuleMergeResolutionSet,
    };

    fn new_test_generation_operation(nonce: &str) -> GenerationOperationContext<'_> {
        GenerationOperationContext::New {
            operation_nonce: nonce,
        }
    }

    fn open_core_after_drop(data_root: &Path) -> Core {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match Core::open(CoreConfig::new(data_root)) {
                Ok(core) => return core,
                Err(error)
                    if error.code == CoreErrorCode::StorageUnavailable
                        && error.message
                            == "data root is already owned by another LorePia process"
                        && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("open Core after prior owner drop: {error:?}"),
            }
        }
    }

    struct StallingProvider {
        partial: String,
        started: Mutex<Option<std_mpsc::Sender<()>>>,
    }

    struct CatchupSnapshotProvider {
        started: Mutex<Option<std_mpsc::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    struct CapturingProvider {
        response: String,
        captured: Mutex<Option<std_mpsc::Sender<Vec<String>>>>,
        captured_temperature: Mutex<Option<std_mpsc::Sender<Option<f64>>>>,
    }

    type OpaqueRequestCapture = (
        bool,
        Vec<OpaqueReasoningContext>,
        Option<GenerationProviderProvenance>,
    );

    struct OpaqueContinuityProvider {
        response: String,
        emitted_state: Option<OpaqueReasoningState>,
        captured_request: Mutex<Option<std_mpsc::Sender<OpaqueRequestCapture>>>,
    }

    struct OverflowUsageProvider;
    struct SnapshotFailingProvider;
    struct RejectingTaskCredentialBroker;

    struct LeaseBarrierProvider {
        entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    }

    impl crate::TaskCredentialBroker for RejectingTaskCredentialBroker {
        fn credential_for<'a>(
            &'a self,
            _connection_id: &'a ProviderConnectionId,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<Output = CoreResult<ConnectionBoundCredential>> + Send + 'a,
            >,
        > {
            Box::pin(async {
                Err(CoreError::internal(
                    "credential broker was called without an embedding task",
                ))
            })
        }
    }

    fn read_http_headers(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set model-list read timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let read = stream.read(&mut buffer).expect("read model-list request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(request).expect("model-list request is UTF-8")
    }

    fn spawn_model_list_provider(
        response_bodies: Vec<String>,
    ) -> (CanonicalOrigin, std_mpsc::Receiver<String>) {
        spawn_model_list_http_provider(
            response_bodies
                .into_iter()
                .map(|body| ("200 OK".to_owned(), body))
                .collect(),
        )
    }

    fn spawn_chat_completion_provider() -> (CanonicalOrigin, std_mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind chat provider");
        let address = listener.local_addr().expect("chat provider address");
        let (request_sender, request_receiver) = std_mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept chat request");
            let request = read_http_headers(&mut stream);
            request_sender
                .send(request)
                .expect("send captured chat request");
            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"fresh authority reply\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write chat response");
        });
        (
            CanonicalOrigin::parse(&format!("http://{address}")).expect("canonical chat origin"),
            request_receiver,
        )
    }

    fn spawn_blocking_chat_completion_provider() -> (
        CanonicalOrigin,
        std_mpsc::Receiver<String>,
        std_mpsc::Sender<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind blocking chat provider");
        let address = listener
            .local_addr()
            .expect("blocking chat provider address");
        let (request_sender, request_receiver) = std_mpsc::channel();
        let (release_sender, release_receiver) = std_mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept blocking chat request");
            let request = read_http_headers(&mut stream);
            request_sender
                .send(request)
                .expect("send captured blocking chat request");
            release_receiver
                .recv()
                .expect("release blocking chat response");
            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"leased reply\"},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            );
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write blocking chat response");
        });
        (
            CanonicalOrigin::parse(&format!("http://{address}"))
                .expect("canonical blocking chat origin"),
            request_receiver,
            release_sender,
        )
    }

    fn spawn_model_list_http_provider(
        responses: Vec<(String, String)>,
    ) -> (CanonicalOrigin, std_mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind model-list provider");
        let address = listener.local_addr().expect("model-list provider address");
        let (request_sender, request_receiver) = std_mpsc::channel();
        thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().expect("accept model-list request");
                let request = read_http_headers(&mut stream);
                request_sender
                    .send(request)
                    .expect("send captured model-list request");
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write model-list response");
            }
        });
        (
            CanonicalOrigin::parse(&format!("http://{address}"))
                .expect("canonical model-list origin"),
            request_receiver,
        )
    }

    fn create_openai_chat_connection(
        core: &Core,
        api_origin: &CanonicalOrigin,
    ) -> (ProviderTemplate, ProviderConnection) {
        let template = core
            .list_provider_templates()
            .expect("provider templates")
            .into_iter()
            .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
            .expect("OpenAI-compatible template");
        let api_base_url = format!("{}/v1", api_origin.as_str());
        let connection = core
            .create_provider_connection(ProviderConnectionDraft {
                id: ProviderConnectionId::from(format!("connection-{}", Uuid::new_v4())),
                template_id: template.id.clone(),
                template_version: template.manifest_version,
                display_name: "Synthetic OpenAI-compatible".to_owned(),
                api_origin: api_origin.clone(),
                api_base_path: Some(EndpointPath::parse("/v1").expect("API base path")),
                network_mode: ProviderNetworkMode::LocalLoopback,
                values: vec![ConnectionConfigEntry {
                    key: "api_base_url".to_owned(),
                    value: ConnectionConfigValue::Text(api_base_url),
                }],
                approved_credential_origin: Some(api_origin.clone()),
                local_network_approval: None,
                timeout_seconds: 5,
            })
            .expect("create model-list connection");
        (template, connection)
    }

    fn create_built_in_public_route(
        core: &Core,
        template_id: &str,
        api_base_path: &str,
        model_id: &str,
    ) -> (ProviderTemplate, ModelRoute) {
        let template = core
            .list_provider_templates()
            .expect("provider templates")
            .into_iter()
            .find(|template| template.id.as_str() == template_id)
            .expect("requested built-in template");
        let api_origin = template
            .default_manifest
            .default_api_origin
            .clone()
            .expect("built-in public origin");
        let connection = core
            .create_provider_connection(ProviderConnectionDraft {
                id: ProviderConnectionId::from(format!("connection-{}", Uuid::new_v4())),
                template_id: template.id.clone(),
                template_version: template.manifest_version,
                display_name: format!("Synthetic {template_id}"),
                api_origin: api_origin.clone(),
                api_base_path: Some(
                    EndpointPath::parse(api_base_path).expect("built-in API base path"),
                ),
                network_mode: ProviderNetworkMode::Public,
                values: Vec::new(),
                approved_credential_origin: Some(api_origin),
                local_network_approval: None,
                timeout_seconds: 5,
            })
            .expect("create built-in public connection");
        let now = Utc::now();
        let route = ModelRoute {
            id: ModelRouteId::from(format!("route-{}", Uuid::new_v4())),
            connection_id: connection.id,
            api_family: template.api_family,
            model_id: model_id.to_owned(),
            display_name: Some(model_id.to_owned()),
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
        core.upsert_model_route(route.clone())
            .expect("save built-in model route");
        (template, route)
    }

    fn install_provider_credential_authority(
        core: &Core,
        connection_id: &ProviderConnectionId,
    ) -> ProviderCredentialAccessAuthority {
        let authority = core
            .inner
            .storage
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose credential install authority");
        let install = core
            .inner
            .storage
            .prepare_provider_credential_operation_with_install_authority(
                connection_id,
                ProviderCredentialOperationKind::Install,
                ProviderCredentialObservedStatus::Missing,
                Some(&authority),
            )
            .expect("prepare credential install");
        core.inner
            .storage
            .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
            .expect("start credential install");
        core.inner
            .storage
            .finish_provider_credential_operation(
                &install.plan.operation_id,
                &install.plan_sha256,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("finish credential install");
        core.inner
            .storage
            .ensure_provider_credential_access_settled(connection_id)
            .expect("read credential authority")
    }

    fn install_then_remove_provider_credential(
        core: &Core,
        connection_id: &ProviderConnectionId,
    ) -> ProviderCredentialAccessAuthority {
        let cached_authority = install_provider_credential_authority(core, connection_id);
        let removal = core
            .inner
            .storage
            .prepare_provider_credential_operation(
                connection_id,
                ProviderCredentialOperationKind::RemoveCredential,
                ProviderCredentialObservedStatus::Available,
            )
            .expect("prepare credential removal");
        core.inner
            .storage
            .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
            .expect("start credential removal");
        core.inner
            .storage
            .finish_provider_credential_operation(
                &removal.plan.operation_id,
                &removal.plan_sha256,
                ProviderCredentialObservedStatus::Missing,
            )
            .expect("finish credential removal");
        cached_authority
    }

    struct DurableAttemptDropProbe {
        storage: Arc<Storage>,
        conversation_id: ConversationId,
        operation_id: String,
        sender: Option<std_mpsc::SyncSender<bool>>,
    }

    impl Drop for DurableAttemptDropProbe {
        fn drop(&mut self) {
            let durable = self
                .storage
                .get_generation_attempt_by_operation_id(&self.conversation_id, &self.operation_id)
                .is_ok();
            if let Some(sender) = self.sender.take() {
                let _ = sender.send(durable);
            }
        }
    }

    fn listed_openrouter_model(
        model_id: &str,
        mut parameters: Vec<OpenRouterSupportedParameter>,
        reasoning: Option<ListedModelReasoningCapability>,
        max_output_tokens: Option<u64>,
    ) -> ListedModel {
        parameters.sort();
        parameters.dedup();
        let mut supported = Vec::new();
        if parameters.iter().any(|parameter| {
            matches!(
                parameter,
                OpenRouterSupportedParameter::Reasoning
                    | OpenRouterSupportedParameter::ReasoningEffort
            )
        }) {
            supported.push(ListedModelCapability::Reasoning);
        }
        if parameters.contains(&OpenRouterSupportedParameter::Tools) {
            supported.push(ListedModelCapability::ToolCalling);
        }
        if parameters.contains(&OpenRouterSupportedParameter::ParallelToolCalls) {
            supported.push(ListedModelCapability::ParallelToolCalling);
        }
        if parameters.contains(&OpenRouterSupportedParameter::StructuredOutputs) {
            supported.push(ListedModelCapability::StructuredOutput);
        }
        if parameters.contains(&OpenRouterSupportedParameter::ResponseFormat) {
            supported.push(ListedModelCapability::JsonMode);
        }
        if parameters.contains(&OpenRouterSupportedParameter::Logprobs) {
            supported.push(ListedModelCapability::Logprobs);
        }
        if parameters.contains(&OpenRouterSupportedParameter::Seed) {
            supported.push(ListedModelCapability::Seed);
        }
        supported.sort();
        ListedModel {
            model_id: model_id.to_owned(),
            display_name: Some(model_id.to_owned()),
            max_input_tokens: Some(128_000),
            max_output_tokens,
            supported_generation_methods: Vec::new(),
            capabilities: ListedModelCapabilities {
                supported,
                parameters: OpenRouterSupportedParameterSupport::Exact(parameters),
                reasoning,
            },
            source: ModelRecordSource::ProviderApi,
            availability: ModelAvailability::Available,
        }
    }

    fn provider_api_openrouter_route(
        connection_id: ProviderConnectionId,
        model: &ListedModel,
        observed_at: DateTime<Utc>,
    ) -> ModelRoute {
        ModelRoute {
            id: ModelRouteId::from(format!("route-{}", Uuid::new_v4())),
            connection_id,
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: model.model_id.clone(),
            display_name: model.display_name.clone(),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: Some(listed_model_metadata(model).expect("listed model metadata")),
            metadata_source: ModelMetadataSource::ProviderApi,
            metadata_observed_at: Some(observed_at),
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: observed_at,
            last_seen_at: Some(observed_at),
        }
    }

    fn refresh_models_with_review(
        core: &Core,
        connection_id: &ProviderConnectionId,
        credential: Option<&str>,
    ) -> CoreResult<ProviderModelRefreshResult> {
        let job_id =
            core.start_provider_model_sync(connection_id, credential.map(str::to_owned))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let job = core.get_provider_model_sync(&job_id)?;
            match job.state {
                ModelSyncState::DiffReadyAwaitingReview => {
                    let review = job.review.ok_or_else(|| {
                        CoreError::internal("review-ready model sync has no review")
                    })?;
                    core.approve_provider_model_sync(&job_id, &review.sha256)?;
                    let diff = review.diff;
                    return Ok(ProviderModelRefreshResult {
                        connection_id: diff.connection_id.clone(),
                        model_routes: core.list_model_routes(&diff.connection_id)?,
                        newly_seen_model_route_ids: diff.newly_seen_model_route_ids,
                        missing_model_route_ids: diff.missing_model_route_ids,
                        created_generation_preset_ids: diff
                            .initial_presets
                            .into_iter()
                            .map(|preset| preset.id)
                            .collect(),
                        routes_requiring_preset_configuration: diff
                            .routes_requiring_preset_configuration,
                        provenance: ProviderModelRefreshProvenance {
                            source: diff.provenance.source,
                            api_family: diff.provenance.api_family,
                            api_origin: diff.provenance.api_origin,
                            endpoint_path: diff.provenance.endpoint_path,
                        },
                        pages_fetched: diff.provenance.pages_fetched,
                        response_bytes: diff.provenance.response_bytes,
                        observed_at: diff.observed_at,
                    });
                }
                ModelSyncState::Failed => {
                    let failure = job
                        .failure
                        .ok_or_else(|| CoreError::internal("failed model sync has no failure"))?;
                    let failure_code = match failure.code.as_str() {
                        "invalid_input" => CoreErrorCode::InvalidInput,
                        "unsupported_content" => CoreErrorCode::UnsupportedContent,
                        "unsafe_archive" => CoreErrorCode::UnsafeArchive,
                        "not_found" => CoreErrorCode::NotFound,
                        "permission_denied" => CoreErrorCode::PermissionDenied,
                        "storage_unavailable" => CoreErrorCode::StorageUnavailable,
                        "storage_corrupted" => CoreErrorCode::StorageCorrupted,
                        "provider_auth_failed" => CoreErrorCode::ProviderAuthFailed,
                        "provider_rate_limited" => CoreErrorCode::ProviderRateLimited,
                        "provider_unavailable" => CoreErrorCode::ProviderUnavailable,
                        "network_unavailable" => CoreErrorCode::NetworkUnavailable,
                        "cancelled" => CoreErrorCode::Cancelled,
                        _ => CoreErrorCode::Internal,
                    };
                    return Err(CoreError::new(
                        failure_code,
                        failure.message_key,
                        failure.recoverable,
                    ));
                }
                ModelSyncState::Cancelled => {
                    return Err(CoreError::new(
                        CoreErrorCode::Cancelled,
                        "model synchronization was cancelled",
                        true,
                    ));
                }
                ModelSyncState::Interrupted => {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageUnavailable,
                        "model synchronization was interrupted",
                        true,
                    ));
                }
                ModelSyncState::Created
                | ModelSyncState::Fetching
                | ModelSyncState::Committing
                | ModelSyncState::Completed => {}
            }
            if Instant::now() >= deadline {
                return Err(CoreError::internal(
                    "model synchronization did not reach review state",
                ));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn create_openai_chat_generation_target(
        core: &Core,
        api_origin: &CanonicalOrigin,
    ) -> (GenerationTarget, ModelRoute) {
        let (template, connection) = create_openai_chat_connection(core, api_origin);
        let now = Utc::now();
        let route = ModelRoute {
            id: ModelRouteId::from(format!("route-{}", Uuid::new_v4())),
            connection_id: connection.id,
            api_family: template.api_family,
            model_id: "reasoning-model".to_owned(),
            display_name: Some("Reasoning Model".to_owned()),
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
        core.upsert_model_route(route.clone())
            .expect("save model route");
        let preset = GenerationPreset {
            id: GenerationPresetId::from(format!("preset-{}", Uuid::new_v4())),
            model_route_id: route.id.clone(),
            display_name: "Reasoning and cache".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings {
                mode: GenerationReasoningMode::Enabled,
                effort: Some(GenerationReasoningEffort::High),
                budget_tokens: None,
                summary: GenerationReasoningSummary::ProviderDefault,
                preserve_opaque_state: false,
            },
            prompt_cache: GenerationPromptCacheSettings {
                mode: GenerationPromptCacheMode::Automatic,
                ttl: GenerationPromptCacheTtl::ProviderDefault,
                context_reference: None,
            },
            created_at: now,
            updated_at: now,
        };
        // Seed a pre-gate stored candidate so the tests below can exercise
        // generation-time repair behavior. Public Core upserts now reject this
        // unsupported reasoning/cache combination before persistence.
        core.inner
            .storage
            .save_generation_preset(&preset)
            .expect("seed legacy generation preset");
        (
            GenerationTarget {
                model_route_id: route.id.clone(),
                generation_preset_id: preset.id,
            },
            route,
        )
    }

    #[test]
    fn generation_operation_nonce_validation_is_bounded_and_core_owned() {
        let semantic_base_fingerprint_sha256 = Sha256Digest::parse(
            "b58c8a55aa6f52703d8c7c98f80690fb401e9867f30ed59ca4d4899749d50525".to_owned(),
        )
        .expect("valid semantic fingerprint fixture");
        let valid = "a".repeat(MAX_GENERATION_OPERATION_NONCE_CHARS);
        let operation_id = new_generation_operation_id(
            "generation-send-v5",
            &semantic_base_fingerprint_sha256,
            &valid,
        )
        .expect("accept a nonce at the Core character bound");
        assert!(operation_id.starts_with("generation-send-v5-"));

        for invalid in [
            String::new(),
            " padded".to_owned(),
            "control\nvalue".to_owned(),
            "a".repeat(MAX_GENERATION_OPERATION_NONCE_CHARS + 1),
            "가".repeat(43),
        ] {
            let error = new_generation_operation_id(
                "generation-send-v5",
                &semantic_base_fingerprint_sha256,
                &invalid,
            )
            .expect_err("Core must reject an invalid generation operation nonce");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
        }
    }

    #[test]
    fn reviewed_prompt_session_seed_is_sqlite_safe_for_a_high_bit_digest() {
        const SQLITE_SIGNED_INTEGER_MAX: u64 = 0x7fff_ffff_ffff_ffff;
        let base_request_fingerprint_sha256 = Sha256Digest::parse(
            "1ac4b8f106727907443ce712070c9aa78bf9cd5b99a97af24efacc61a1276fb3".to_owned(),
        )
        .expect("valid SHA-256 fixture");
        let digest = Sha256::digest(
            format!(
                "reviewed-prompt-session-seed-v2:{}",
                base_request_fingerprint_sha256.as_str()
            )
            .as_bytes(),
        );
        let raw_seed = u64::from_be_bytes(
            digest[..8]
                .try_into()
                .expect("SHA-256 always contains eight seed bytes"),
        );
        assert!(
            raw_seed > SQLITE_SIGNED_INTEGER_MAX,
            "fixture must exercise the formerly rejected upper-half seed"
        );
        let bounded = reviewed_prompt_session_seed(&base_request_fingerprint_sha256);
        assert_eq!(bounded, raw_seed & SQLITE_SIGNED_INTEGER_MAX);
        assert!(bounded <= SQLITE_SIGNED_INTEGER_MAX);
        assert_eq!(
            bounded,
            reviewed_prompt_session_seed(&base_request_fingerprint_sha256)
        );
    }

    #[test]
    fn connection_bound_credential_rejects_rebound_target_before_chat_mutation() {
        let (_root, core, character) = imported_core();
        let conversation = core
            .create_conversation(&character.id, "Bound credential", ConversationMode::Chat)
            .expect("conversation");
        let branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let (template, route) =
            create_built_in_public_route(&core, "openai-responses-v1", "/v1", "gpt-bound-fixture");
        let preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
            .expect("generation preset");
        let expected_connection_id = route.connection_id.clone();
        let target = GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: preset.id,
        };
        let credential_canary = "synthetic-bound-credential";
        let wrong_connection_id = ProviderConnectionId::from("different-connection");

        let send_error = core
            .send_message_to_branch_with_connection_credential(
                &conversation.id,
                &branch,
                None,
                ConversationMode::Chat,
                "must not be stored",
                new_test_generation_operation("bound-send-v1"),
                &target,
                ConnectionBoundCredential::new(
                    wrong_connection_id.clone(),
                    Some(credential_canary.to_owned()),
                ),
            )
            .expect_err("send must reject a credential bound to another connection");
        let edit_error = core
            .edit_user_message_with_connection_credential(
                &conversation.id,
                &branch,
                None,
                &MessageId("missing-user-message".to_owned()),
                "must not be stored",
                new_test_generation_operation("bound-edit-v1"),
                &target,
                ConnectionBoundCredential::new(
                    wrong_connection_id.clone(),
                    Some(credential_canary.to_owned()),
                ),
            )
            .expect_err("edit must reject a credential bound to another connection");
        let regenerate_error = core
            .regenerate_assistant_message_with_connection_credential(
                &conversation.id,
                &branch,
                None,
                &MessageId("missing-assistant-message".to_owned()),
                new_test_generation_operation("bound-regenerate-v1"),
                &target,
                ConnectionBoundCredential::new(
                    wrong_connection_id,
                    Some(credential_canary.to_owned()),
                ),
            )
            .expect_err("regenerate must reject a credential bound to another connection");

        for error in [send_error, edit_error, regenerate_error] {
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert_eq!(
                error.message,
                "credential does not belong to the selected provider connection"
            );
        }
        assert!(
            core.list_branch_messages(&branch)
                .expect("messages after rejected operations")
                .is_empty()
        );
        assert!(
            core.inner
                .active_generations
                .active
                .lock()
                .expect("generation registry")
                .is_empty()
        );

        let (resolved, credential) = resolve_generation_target_with_connection_credential(
            &core,
            &target,
            ConnectionBoundCredential::new(
                expected_connection_id,
                Some(credential_canary.to_owned()),
            ),
        )
        .expect("matching connection binding resolves");
        assert_eq!(resolved.model, "gpt-bound-fixture");
        assert_eq!(credential.as_deref(), Some(credential_canary));
    }

    #[test]
    fn terminal_credential_removal_rejects_cached_generation_before_provider_work() {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "Stale generation credential",
                ConversationMode::Chat,
            )
            .expect("conversation");
        let branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let (template, route) = create_built_in_public_route(
            &core,
            "openai-responses-v1",
            "/v1",
            "gpt-stale-authority",
        );
        let preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
            .expect("generation preset");
        let connection_id = route.connection_id.clone();
        let target = GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: preset.id,
        };
        let cached_authority = install_then_remove_provider_credential(&core, &connection_id);

        let error = core
            .send_message_to_branch_with_connection_credential(
                &conversation.id,
                &branch,
                None,
                ConversationMode::Chat,
                "must remain transient",
                new_test_generation_operation("stale-generation-authority-v1"),
                &target,
                ConnectionBoundCredential::new_with_access_authority(
                    connection_id,
                    Some("cached-secret".to_owned()),
                    cached_authority,
                ),
            )
            .expect_err("terminal removal must reject cached generation authority");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(
            core.list_branch_messages(&branch)
                .expect("messages after rejected generation")
                .is_empty()
        );
        assert!(
            core.inner
                .active_generations
                .active
                .lock()
                .expect("generation registry")
                .is_empty()
        );
        let connection = rusqlite::Connection::open(root.path().join("db/lorepia.sqlite3"))
            .expect("open generation database");
        let (attempt_count, generation_count) = connection
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM generation_attempt_intents),
                   (SELECT COUNT(*) FROM generations)",
                [],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
            )
            .expect("count rejected generation rows");
        assert_eq!((attempt_count, generation_count), (0, 0));
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one vertical proves exact credential authority epochs before generation admission"
    )]
    fn reinstalled_credential_rejects_cached_generation_authority_before_provider_work() {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "Reinstalled generation credential",
                ConversationMode::Chat,
            )
            .expect("conversation");
        let branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let (api_origin, requests) = spawn_chat_completion_provider();
        let (template, connection) = create_openai_chat_connection(&core, &api_origin);
        let connection_id = connection.id.clone();
        let now = Utc::now();
        let route = core
            .upsert_model_route(ModelRoute {
                id: ModelRouteId::from("reinstalled-credential-generation-route"),
                connection_id: connection_id.clone(),
                api_family: template.api_family,
                model_id: "reinstalled-credential-generation-model".to_owned(),
                display_name: Some("Reinstalled credential model".to_owned()),
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
            })
            .expect("save generation route");
        let preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
            .expect("save generation preset");
        let target = GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: preset.id,
        };

        let cached_authority = install_then_remove_provider_credential(&core, &connection_id);
        let current_authority = install_provider_credential_authority(&core, &connection_id);
        assert_ne!(
            cached_authority.authority_id,
            current_authority.authority_id
        );
        assert_eq!(
            cached_authority.connection_binding_sha256, current_authority.connection_binding_sha256,
            "reinstall must retain the same immutable connection binding"
        );

        let stale_error = core
            .send_message_to_branch_with_connection_credential(
                &conversation.id,
                &branch,
                None,
                ConversationMode::Chat,
                "must remain transient",
                new_test_generation_operation("cached-reinstalled-authority-v1"),
                &target,
                ConnectionBoundCredential::new_with_access_authority(
                    connection_id.clone(),
                    Some("cached-secret".to_owned()),
                    cached_authority,
                ),
            )
            .expect_err("cached pre-removal authority must not admit generation");
        assert_eq!(stale_error.code, CoreErrorCode::InvalidInput);
        assert!(stale_error.recoverable);
        assert_eq!(
            requests.recv_timeout(Duration::from_millis(250)),
            Err(std_mpsc::RecvTimeoutError::Timeout),
            "stale authority must not reach provider work"
        );
        assert!(
            core.list_branch_messages(&branch)
                .expect("messages after stale authority rejection")
                .is_empty()
        );
        let database = rusqlite::Connection::open(hard_crash_database_path(root.path()))
            .expect("open active generation database");
        let (attempt_count, generation_count, message_count) = database
            .query_row(
                "SELECT
                   (SELECT COUNT(*) FROM generation_attempt_intents),
                   (SELECT COUNT(*) FROM generations),
                   (SELECT COUNT(*) FROM messages)",
                [],
                |row| {
                    Ok((
                        row.get::<_, u64>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, u64>(2)?,
                    ))
                },
            )
            .expect("count rejected generation rows");
        assert_eq!((attempt_count, generation_count, message_count), (0, 0, 0));
        drop(database);

        let generation_id = core
            .send_message_to_branch_with_connection_credential(
                &conversation.id,
                &branch,
                None,
                ConversationMode::Chat,
                "use the current authority",
                new_test_generation_operation("current-reinstalled-authority-v1"),
                &target,
                ConnectionBoundCredential::new_with_access_authority(
                    connection_id,
                    Some("fresh-secret".to_owned()),
                    current_authority,
                ),
            )
            .expect("current authority admits generation");
        let request = requests
            .recv_timeout(Duration::from_secs(2))
            .expect("current authority reaches provider");
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer fresh-secret\r\n")
        );
        wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let messages = core
            .list_branch_messages(&branch)
            .expect("completed generation messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, "fresh authority reply");
    }

    #[tokio::test]
    async fn legacy_admission_lease_releases_after_durable_attempt_before_async_planning() {
        struct DropProbe(Option<std_mpsc::SyncSender<()>>);

        impl Drop for DropProbe {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    let _ = sender.send(());
                }
            }
        }

        let (_root, core, character) = imported_core();
        let provider_profile_id = "legacy-admission-lease-profile";
        core.upsert_provider_profile(ProviderProfile {
            id: provider_profile_id.to_owned(),
            display_name: "Legacy admission lease".to_owned(),
            base_url: "http://127.0.0.1:9/v1".to_owned(),
            model: "lease-model".to_owned(),
            timeout_seconds: 1,
        })
        .expect("legacy provider profile");
        let conversation = core
            .create_conversation(
                &character.id,
                "Legacy admission lease",
                ConversationMode::Chat,
            )
            .expect("conversation");
        let branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let (dropped_sender, dropped_receiver) = std_mpsc::sync_channel(1);
        let generation_id = core
            .send_message_to_branch_async_with_credential_admission_lease(
                &conversation.id,
                &branch,
                None,
                ConversationMode::Chat,
                "admit before prompt tasks",
                new_test_generation_operation("legacy-admission-lease-v1"),
                provider_profile_id,
                None,
                GenerationCredentialAdmissionLease::new(DropProbe(Some(dropped_sender))),
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("start legacy generation");
        dropped_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("admission lease released before async call returned");
        assert!(
            core.inner
                .storage
                .get_generation_attempt(&generation_id)
                .is_ok()
        );
    }

    #[tokio::test]
    async fn legacy_message_action_releases_admission_only_after_durable_attempt() {
        let (_root, core, character) = imported_core();
        let provider_profile_id = "legacy-action-admission-profile";
        core.upsert_provider_profile(ProviderProfile {
            id: provider_profile_id.to_owned(),
            display_name: "Legacy action admission".to_owned(),
            base_url: "http://127.0.0.1:9/v1".to_owned(),
            model: "lease-action-model".to_owned(),
            timeout_seconds: 1,
        })
        .expect("legacy provider profile");
        let conversation = core
            .create_conversation(
                &character.id,
                "Legacy action admission lease",
                ConversationMode::Chat,
            )
            .expect("conversation");
        let source_generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "original action message",
                "source-model".to_owned(),
                None,
                Arc::new(StaticProvider::new("source reply")),
            )
            .expect("source generation");
        wait_for_generation_status(&core, &source_generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let source_branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let source_messages = core
            .list_branch_messages(&source_branch)
            .expect("source messages");
        let operation_nonce = "legacy-action-admission-v1";
        let action_identity = core
            .prepare_message_generation_action_identity(MessageGenerationActionIdentityInput {
                conversation_id: &conversation.id,
                source_branch_id: &source_branch,
                expected_source_head_message_id: Some(&source_messages[1].id),
                target_message_id: &source_messages[0].id,
                action: MessageGenerationAction::EditUser,
                replacement_text: Some("edited through legacy admission"),
                operation_context: new_test_generation_operation(operation_nonce),
                target: GenerationActionTargetIdentity::ProviderProfile {
                    provider_profile_id: provider_profile_id.to_owned(),
                },
            })
            .expect("resolve action operation identity");
        let (drop_sender, drop_receiver) = std_mpsc::sync_channel(1);
        let action = core
            .edit_user_message_async_with_credential_admission_lease(
                &conversation.id,
                &source_branch,
                Some(&source_messages[1].id),
                &source_messages[0].id,
                "edited through legacy admission",
                new_test_generation_operation(operation_nonce),
                provider_profile_id,
                None,
                GenerationCredentialAdmissionLease::new(DurableAttemptDropProbe {
                    storage: Arc::clone(&core.inner.storage),
                    conversation_id: conversation.id.clone(),
                    operation_id: action_identity.operation_id,
                    sender: Some(drop_sender),
                }),
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("start legacy edit generation");
        assert!(
            drop_receiver
                .recv_timeout(Duration::from_secs(1))
                .expect("observe admission lease release"),
            "message-action admission lease released before its attempt became durable"
        );
        assert!(
            core.inner
                .storage
                .get_generation_attempt(&action.generation_id)
                .is_ok()
        );
    }

    #[test]
    fn connection_credential_presence_and_reference_match_connection_policy() {
        let root = tempdir().expect("temporary core root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let api_origin = CanonicalOrigin::parse("http://127.0.0.1:39491").expect("loopback origin");
        let (_template, credential_connection) = create_openai_chat_connection(&core, &api_origin);
        let credential_canary = "synthetic-bound-credential";
        assert!(
            !format!(
                "{:?}",
                ConnectionBoundCredential::new(
                    credential_connection.id.clone(),
                    Some(credential_canary.to_owned()),
                )
            )
            .contains(credential_canary)
        );
        let credential_reference = credential_connection
            .credential_ref
            .as_ref()
            .expect("credential-requiring connection reference");
        assert_eq!(
            credential_reference.as_str(),
            credential_connection.id.as_str()
        );

        let missing = validate_connection_credential_binding(
            &credential_connection,
            &ConnectionBoundCredential::new(credential_connection.id.clone(), None),
        )
        .expect_err("credential-requiring connection rejects missing material");
        assert_eq!(missing.code, CoreErrorCode::ProviderAuthFailed);
        assert_eq!(missing.message, "provider credential is required");

        let mut mismatched_reference = credential_connection.clone();
        mismatched_reference.credential_ref =
            Some(CredentialRef("different-vault-reference".to_owned()));
        let mismatch = validate_connection_credential_binding(
            &mismatched_reference,
            &ConnectionBoundCredential::new(
                credential_connection.id,
                Some("synthetic-credential".to_owned()),
            ),
        )
        .expect_err("stored credential reference must match the bound connection");
        assert_eq!(mismatch.code, CoreErrorCode::InvalidInput);

        let no_auth_template = core
            .list_provider_templates()
            .expect("provider templates")
            .into_iter()
            .find(|template| template.id.as_str() == "ollama-native-v1")
            .expect("Ollama template");
        let no_auth_origin =
            CanonicalOrigin::parse("http://127.0.0.1:11434").expect("Ollama origin");
        let no_auth_connection = core
            .create_provider_connection(ProviderConnectionDraft {
                id: ProviderConnectionId::from("no-auth-bound-credential"),
                template_id: no_auth_template.id,
                template_version: no_auth_template.manifest_version,
                display_name: "No-auth bound credential".to_owned(),
                api_origin: no_auth_origin,
                api_base_path: Some(EndpointPath::parse("/api").expect("Ollama API base path")),
                network_mode: ProviderNetworkMode::LocalLoopback,
                values: Vec::new(),
                approved_credential_origin: None,
                local_network_approval: None,
                timeout_seconds: 5,
            })
            .expect("create no-auth connection");
        let unexpected = validate_connection_credential_binding(
            &no_auth_connection,
            &ConnectionBoundCredential::new(
                no_auth_connection.id.clone(),
                Some("synthetic-unexpected-credential".to_owned()),
            ),
        )
        .expect_err("credentialless connection rejects unexpected material");
        assert_eq!(unexpected.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            unexpected.message,
            "this provider connection does not permit a credential"
        );
        validate_connection_credential_binding(
            &no_auth_connection,
            &ConnectionBoundCredential::new(no_auth_connection.id.clone(), None),
        )
        .expect("credentialless connection accepts absent material");
    }

    #[tokio::test]
    async fn primary_generation_retains_credential_carrier_until_provider_finishes() {
        let (_root, core, character) = imported_core();
        let (api_origin, requests, release_provider) = spawn_blocking_chat_completion_provider();
        let (template, connection) = create_openai_chat_connection(&core, &api_origin);
        let connection_id = connection.id.clone();
        let now = Utc::now();
        let route = core
            .upsert_model_route(ModelRoute {
                id: ModelRouteId::from("primary-credential-carrier-route"),
                connection_id: connection_id.clone(),
                api_family: template.api_family,
                model_id: "primary-credential-carrier-model".to_owned(),
                display_name: Some("Primary credential carrier model".to_owned()),
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
            })
            .expect("save primary credential carrier route");
        let preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
            .expect("save primary credential carrier preset");
        let target = GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: preset.id,
        };
        let authority = install_provider_credential_authority(&core, &connection_id);
        let conversation = core
            .create_conversation(
                &character.id,
                "Primary credential dispatch lease",
                ConversationMode::Chat,
            )
            .expect("create credential dispatch conversation");
        let branch = core
            .get_conversation_state(&conversation.id)
            .expect("credential dispatch conversation state")
            .active_branch_id;
        let operation_lock = Arc::new(tokio::sync::Mutex::new(()));
        let dispatch_lease = Arc::clone(&operation_lock).lock_owned().await;
        let credential = ConnectionBoundCredential::new_with_access_authority(
            connection_id,
            Some("synthetic-primary-leased-secret".to_owned()),
            authority,
        )
        .with_dispatch_lease(dispatch_lease);

        let generation_id = core
            .send_message_to_branch_with_connection_credential(
                &conversation.id,
                &branch,
                None,
                ConversationMode::Chat,
                "retain the primary credential carrier",
                new_test_generation_operation("primary-credential-carrier-v1"),
                &target,
                credential,
            )
            .expect("start connection-bound primary generation");
        let request = requests
            .recv_timeout(Duration::from_secs(2))
            .expect("primary provider future starts");
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-primary-leased-secret\r\n")
        );
        let retained_during_provider = Arc::clone(&operation_lock).try_lock_owned().is_err();

        release_provider
            .send(())
            .expect("finish blocking primary provider");
        wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        assert!(
            retained_during_provider,
            "connection-bound carrier dropped before the primary provider future finished"
        );
        assert!(
            Arc::clone(&operation_lock).try_lock_owned().is_ok(),
            "primary provider completion must release the credential carrier"
        );
    }

    #[tokio::test]
    async fn provider_dispatch_retains_credential_lease_until_attempt_finishes() {
        for credential_value in [Some("synthetic-leased-secret".to_owned()), None] {
            let operation_lock = Arc::new(tokio::sync::Mutex::new(()));
            let dispatch_lease = Arc::clone(&operation_lock).lock_owned().await;
            let credential = ConnectionBoundCredential::new_with_dispatch_lease(
                ProviderConnectionId::from("leased-dispatch-connection"),
                credential_value,
                dispatch_lease,
            );
            let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
            let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
            let provider = Arc::new(LeaseBarrierProvider {
                entered: Mutex::new(Some(entered_sender)),
                release: Mutex::new(Some(release_receiver)),
            });
            let request = GenerationRequest {
                generation_id: GenerationId::new(),
                conversation_id: ConversationId::new(),
                model: "lease-barrier-model".to_owned(),
                messages: Vec::new(),
                resolved_prompt_plan: None,
                provider_execution_plan_hash: None,
                temperature: None,
                max_output_tokens: None,
                provider_provenance: None,
                preserve_opaque_reasoning_state: false,
                opaque_reasoning_context: Vec::new(),
            };
            let (_cancel_sender, cancelled) = watch::channel(false);
            let dispatch = tokio::spawn(dispatch_auxiliary_task_provider(
                provider, request, credential, 5_000, cancelled,
            ));

            entered_receiver.await.expect("provider dispatch entered");
            assert!(
                Arc::clone(&operation_lock).try_lock_owned().is_err(),
                "archive/delete operation lock must remain unavailable during provider dispatch"
            );
            release_sender.send(()).expect("release provider dispatch");
            let outcome = dispatch.await.expect("Send provider dispatch task");
            assert!(matches!(outcome, TaskExecutionOutcome::Completed { .. }));
            assert!(
                Arc::clone(&operation_lock).try_lock_owned().is_ok(),
                "provider completion must release the in-process credential lease"
            );
        }
    }

    #[tokio::test]
    async fn cancelled_provider_dispatch_drops_credential_before_releasing_mutation_gate() {
        let operation_lock = Arc::new(tokio::sync::Mutex::new(()));
        let dispatch_lease = Arc::clone(&operation_lock).lock_owned().await;
        let credential = ConnectionBoundCredential::new_with_dispatch_lease(
            ProviderConnectionId::from("cancelled-leased-dispatch-connection"),
            Some("synthetic-cancelled-leased-secret".to_owned()),
            dispatch_lease,
        );
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (_release_sender, release_receiver) = tokio::sync::oneshot::channel();
        let provider = Arc::new(LeaseBarrierProvider {
            entered: Mutex::new(Some(entered_sender)),
            release: Mutex::new(Some(release_receiver)),
        });
        let request = GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: ConversationId::new(),
            model: "cancelled-lease-barrier-model".to_owned(),
            messages: Vec::new(),
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: None,
            max_output_tokens: None,
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        };
        let (cancel_sender, cancelled) = watch::channel(false);
        let dispatch = tokio::spawn(dispatch_auxiliary_task_provider(
            provider, request, credential, 5_000, cancelled,
        ));

        entered_receiver.await.expect("provider dispatch entered");
        assert!(Arc::clone(&operation_lock).try_lock_owned().is_err());
        cancel_sender.send(true).expect("cancel provider dispatch");
        let outcome = dispatch.await.expect("join cancelled provider dispatch");
        assert!(matches!(
            outcome,
            TaskExecutionOutcome::Failed {
                classification: TaskDispatchClassification::UnknownOutcome,
                error: CoreError {
                    code: CoreErrorCode::Cancelled,
                    ..
                },
            }
        ));
        assert!(
            Arc::clone(&operation_lock).try_lock_owned().is_ok(),
            "cancellation must drop and zeroize the credential carrier before mutation resumes"
        );
    }

    #[tokio::test]
    async fn pre_cancelled_runtime_dispatch_never_enters_the_provider() {
        let (provider, captured) = CapturingProvider::new("must not be emitted");
        let request = runtime_generation_request(
            "runtime-model".to_owned(),
            vec![RuntimePromptMessage {
                role: MessageRole::User,
                content: "bounded prompt".to_owned(),
            }],
            Some(u32::MAX),
            None,
        );
        assert_eq!(request.max_output_tokens, Some(RUNTIME_MAX_OUTPUT_TOKENS));
        let (cancel_sender, cancelled) = watch::channel(false);
        cancel_sender
            .send(true)
            .expect("mark request cancelled before dispatch");

        let outcome = dispatch_auxiliary_task_provider(
            provider,
            request,
            ConnectionBoundCredential::new(
                ProviderConnectionId::from("pre-cancelled-runtime-connection"),
                Some("synthetic-pre-cancelled-secret".to_owned()),
            ),
            5_000,
            cancelled,
        )
        .await;

        assert!(matches!(
            outcome,
            TaskExecutionOutcome::Failed {
                classification: TaskDispatchClassification::KnownNoSideEffect,
                error: CoreError {
                    code: CoreErrorCode::Cancelled,
                    ..
                },
            }
        ));
        assert!(
            captured.recv_timeout(Duration::from_millis(50)).is_err(),
            "pre-cancelled runtime request reached the provider"
        );
    }

    #[test]
    fn runtime_unknown_provider_outcome_is_non_recoverable() {
        let error =
            runtime_generation_result(unknown_task_outcome("synthetic post-dispatch cancellation"))
                .expect_err("unknown provider outcome must not become a successful runtime result");

        assert_eq!(error.code, CoreErrorCode::Internal);
        assert!(!error.recoverable);
        assert!(error.message.contains("outcome is unknown after dispatch"));
    }

    #[test]
    fn provider_connection_update_cannot_rebind_endpoint_or_credential_identity() {
        let root = tempdir().expect("temporary core root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let (api_origin, _) = spawn_model_list_provider(Vec::new());
        let (template, connection) = create_openai_chat_connection(&core, &api_origin);

        let mut ordinary_update = connection.clone();
        ordinary_update.display_name = "Renamed connection".to_owned();
        ordinary_update.timeout_seconds = 9;
        ordinary_update.status = ConnectionStatus::Connected;
        ordinary_update.created_at -= chrono::Duration::days(1);
        let updated = core
            .upsert_provider_connection(ordinary_update)
            .expect("safe connection update");
        assert_eq!(updated.display_name, "Renamed connection");
        assert_eq!(updated.timeout_seconds, 9);
        assert_eq!(updated.status, connection.status);
        assert_eq!(updated.created_at, connection.created_at);

        let mut origin_rebind = updated.clone();
        origin_rebind.api_origin =
            CanonicalOrigin::parse("http://127.0.0.1:65534").expect("other loopback origin");
        let error = core
            .upsert_provider_connection(origin_rebind)
            .expect_err("origin rebinding must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        let mut base_path_rebind = updated.clone();
        base_path_rebind.config.api_base_path =
            Some(EndpointPath::parse("/alternate-v1").expect("alternate base path"));
        let error = core
            .upsert_provider_connection(base_path_rebind)
            .expect_err("base-path rebinding must require a new connection");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.message.contains("endpoint configuration"));

        let mut value_rebind = updated.clone();
        value_rebind.config.values = vec![ConnectionConfigEntry {
            key: "api_base_url".to_owned(),
            value: ConnectionConfigValue::Text(format!("{}/alternate", api_origin.as_str())),
        }];
        let error = core
            .upsert_provider_connection(value_rebind)
            .expect_err("endpoint-affecting config values must require a new connection");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.message.contains("endpoint configuration"));

        let duplicate_create = core
            .create_provider_connection(ProviderConnectionDraft {
                id: updated.id.clone(),
                template_id: template.id,
                template_version: template.manifest_version,
                display_name: "Duplicate endpoint".to_owned(),
                api_origin: api_origin.clone(),
                api_base_path: Some(
                    EndpointPath::parse("/alternate-v1").expect("alternate base path"),
                ),
                network_mode: ProviderNetworkMode::LocalLoopback,
                values: vec![ConnectionConfigEntry {
                    key: "api_base_url".to_owned(),
                    value: ConnectionConfigValue::Text(format!(
                        "{}/alternate-v1",
                        api_origin.as_str()
                    )),
                }],
                approved_credential_origin: Some(api_origin),
                local_network_approval: None,
                timeout_seconds: 5,
            })
            .expect_err("create cannot be used as an endpoint-identity upsert");
        assert_eq!(duplicate_create.code, CoreErrorCode::InvalidInput);
        assert!(
            duplicate_create
                .message
                .contains("identifier already exists")
        );

        let mut credential_rebind = updated.clone();
        credential_rebind.credential_ref = Some(CredentialRef("another-secret".to_owned()));
        let error = core
            .upsert_provider_connection(credential_rebind)
            .expect_err("credential rebinding must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        assert_eq!(
            core.inner
                .storage
                .get_provider_connection(&updated.id)
                .expect("unchanged provider identity")
                .config,
            updated.config
        );
    }

    #[test]
    fn legacy_provider_profile_keeps_endpoint_identity_but_can_select_a_new_model_route() {
        let root = tempdir().expect("temporary core root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let original = ProviderProfile {
            id: format!("legacy-{}", Uuid::new_v4()),
            display_name: "Legacy original".to_owned(),
            base_url: "http://127.0.0.1:65534/v1".to_owned(),
            model: "model-one".to_owned(),
            timeout_seconds: 30,
        };
        core.upsert_provider_profile(original.clone())
            .expect("create legacy provider");
        let connection_id = ProviderConnectionId::from(original.id.as_str());
        let original_route = core
            .list_model_routes(&connection_id)
            .expect("original routes")
            .into_iter()
            .find(|route| route.model_id == "model-one")
            .expect("original model route");

        let safe_update = ProviderProfile {
            display_name: "Legacy renamed".to_owned(),
            model: "model-two".to_owned(),
            timeout_seconds: 45,
            ..original.clone()
        };
        core.upsert_provider_profile(safe_update.clone())
            .expect("display, timeout, and selected model may change");
        let routes = core
            .list_model_routes(&connection_id)
            .expect("preserved legacy routes");
        let new_route = routes
            .iter()
            .find(|route| route.model_id == "model-two")
            .expect("new model route");
        assert_ne!(new_route.id, original_route.id);
        assert!(routes.iter().any(|route| route.id == original_route.id));

        let mut endpoint_rebind = safe_update.clone();
        endpoint_rebind.base_url = "http://127.0.0.1:65534/v2".to_owned();
        let error = core
            .upsert_provider_profile(endpoint_rebind)
            .expect_err("legacy endpoint mutation must require a new provider ID");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(
            error
                .message
                .contains("endpoint configuration is immutable")
        );
        assert_eq!(
            core.inner
                .storage
                .get_provider_profile(&safe_update.id)
                .expect("unchanged legacy profile"),
            safe_update
        );
        assert_eq!(
            core.inner
                .storage
                .get_provider_connection(&connection_id)
                .expect("unchanged legacy connection")
                .config
                .api_base_path
                .as_ref()
                .map(EndpointPath::as_str),
            Some("/v1")
        );
    }

    struct RetainedLegacyCrudFixture {
        _root: TempDir,
        core: Core,
        connection_id: ProviderConnectionId,
        route_id: ModelRouteId,
        preset_id: GenerationPresetId,
        routes_before: Vec<ModelRoute>,
        presets_before: Vec<GenerationPreset>,
        cleared_settings: AppSettings,
    }

    fn retained_legacy_crud_fixture() -> RetainedLegacyCrudFixture {
        let root = tempdir().expect("temporary core root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let original = ProviderProfile {
            id: format!("legacy-{}", Uuid::new_v4()),
            display_name: "Legacy protected target".to_owned(),
            base_url: "http://127.0.0.1:65534/v1".to_owned(),
            model: "model-one".to_owned(),
            timeout_seconds: 30,
        };
        core.upsert_provider_profile(original.clone())
            .expect("create legacy provider");
        let mut settings = core.get_settings().expect("initial settings");
        settings.selected_provider_profile_id = Some(original.id.clone());
        core.update_settings(&settings)
            .expect("select the retained legacy profile");

        core.upsert_provider_profile(ProviderProfile {
            model: "model-two".to_owned(),
            ..original.clone()
        })
        .expect("move the active legacy profile to a sibling route");
        let normalized = core.get_settings().expect("normalized legacy selection");
        let route_id = normalized
            .selected_model_route_id
            .clone()
            .expect("selected legacy route");
        let preset_id = normalized
            .selected_generation_preset_id
            .clone()
            .expect("selected legacy preset");
        assert_ne!(route_id.as_str(), original.id);
        assert_eq!(preset_id.as_str(), route_id.as_str());
        let connection_id = ProviderConnectionId::from(original.id.as_str());
        let routes_before = core
            .list_model_routes(&connection_id)
            .expect("legacy routes before rejected deletes");
        let presets_before = core
            .list_generation_presets(&route_id)
            .expect("legacy presets before rejected deletes");
        let selected = core
            .select_generation_target(None)
            .expect("clear the legacy selection without archiving its profile");
        assert!(selected.selected_provider_profile_id.is_none());

        RetainedLegacyCrudFixture {
            _root: root,
            core,
            connection_id,
            route_id,
            preset_id,
            routes_before,
            presets_before,
            cleared_settings: selected,
        }
    }

    fn assert_retained_legacy_fixture_unchanged(fixture: &RetainedLegacyCrudFixture) {
        assert_eq!(
            fixture
                .core
                .get_settings()
                .expect("settings after rejected operation"),
            fixture.cleared_settings
        );
        assert_eq!(
            fixture
                .core
                .list_model_routes(&fixture.connection_id)
                .expect("routes after rejected operation"),
            fixture.routes_before
        );
        assert_eq!(
            fixture
                .core
                .list_generation_presets(&fixture.route_id)
                .expect("presets after rejected operation"),
            fixture.presets_before
        );
    }

    #[test]
    fn retained_legacy_profile_current_sibling_deletes_are_rejected() {
        let fixture = retained_legacy_crud_fixture();

        let route_error = fixture
            .core
            .delete_model_route(&fixture.route_id)
            .expect_err("the active legacy sibling route must be protected");
        assert_eq!(route_error.code, CoreErrorCode::InvalidInput);
        assert_retained_legacy_fixture_unchanged(&fixture);

        let preset_error = fixture
            .core
            .delete_generation_preset(&fixture.preset_id)
            .expect_err("the active legacy sibling preset must be protected");
        assert_eq!(preset_error.code, CoreErrorCode::InvalidInput);
        assert_retained_legacy_fixture_unchanged(&fixture);
    }

    #[test]
    fn retained_legacy_profile_rejects_ordinary_route_and_preset_upserts() {
        let fixture = retained_legacy_crud_fixture();

        let mut route_update = fixture
            .routes_before
            .iter()
            .find(|route| route.id == fixture.route_id)
            .expect("current legacy route")
            .clone();
        route_update.display_name = Some("ordinary mutation".to_owned());
        let route_update_error = fixture
            .core
            .upsert_model_route(route_update)
            .expect_err("ordinary route mutation must not alter the legacy graph");
        assert_eq!(route_update_error.code, CoreErrorCode::InvalidInput);

        let mut extra_route = fixture
            .routes_before
            .iter()
            .find(|route| route.id == fixture.route_id)
            .expect("current legacy route")
            .clone();
        extra_route.id = ModelRouteId::from(format!("ordinary-{}", Uuid::new_v4()));
        extra_route.model_id = "ordinary-extra-model".to_owned();
        extra_route.display_name = Some("Ordinary extra route".to_owned());
        extra_route.metadata_source = ModelMetadataSource::UserOverride;
        let route_create_error = fixture
            .core
            .upsert_model_route(extra_route)
            .expect_err("ordinary route creation must not extend the legacy graph");
        assert_eq!(route_create_error.code, CoreErrorCode::InvalidInput);

        let mut extra_preset = fixture
            .presets_before
            .first()
            .expect("current legacy preset")
            .clone();
        extra_preset.id = GenerationPresetId::from(format!("ordinary-{}", Uuid::new_v4()));
        extra_preset.display_name = "Ordinary extra preset".to_owned();
        let preset_create_error = fixture
            .core
            .upsert_generation_preset(extra_preset)
            .expect_err("ordinary preset creation must not extend the legacy graph");
        assert_eq!(preset_create_error.code, CoreErrorCode::InvalidInput);
        assert_retained_legacy_fixture_unchanged(&fixture);
    }

    #[test]
    fn active_legacy_profile_reselection_preserves_its_sibling_target_family() {
        let root = tempdir().expect("temporary core root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let original = ProviderProfile {
            id: format!("legacy-{}", Uuid::new_v4()),
            display_name: "Legacy reselection".to_owned(),
            base_url: "http://127.0.0.1:65534/v1".to_owned(),
            model: "model-one".to_owned(),
            timeout_seconds: 30,
        };
        core.upsert_provider_profile(original.clone())
            .expect("create legacy provider");
        let connection_id = ProviderConnectionId::from(original.id.as_str());
        let original_route = core
            .list_model_routes(&connection_id)
            .expect("original legacy routes")
            .into_iter()
            .find(|route| route.model_id == original.model)
            .expect("original legacy route");
        let original_target = GenerationTarget {
            model_route_id: original_route.id.clone(),
            generation_preset_id: GenerationPresetId::from(original_route.id.as_str()),
        };
        let mut settings = core.get_settings().expect("initial settings");
        settings.selected_provider_profile_id = Some(original.id.clone());
        core.update_settings(&settings)
            .expect("select the retained legacy profile");
        core.upsert_provider_profile(ProviderProfile {
            model: "model-two".to_owned(),
            ..original.clone()
        })
        .expect("move the active legacy profile to a sibling route");
        let selected = core.get_settings().expect("normalized legacy selection");
        let current_target = GenerationTarget {
            model_route_id: selected
                .selected_model_route_id
                .clone()
                .expect("selected legacy route"),
            generation_preset_id: selected
                .selected_generation_preset_id
                .clone()
                .expect("selected legacy preset"),
        };
        let cleared = core
            .select_generation_target(None)
            .expect("clear the legacy selection before generic reselection");
        assert!(cleared.selected_provider_profile_id.is_none());

        let reselected = core
            .select_generation_target(Some(current_target))
            .expect("reselect the exact normalized legacy target");
        assert_eq!(
            reselected.selected_provider_profile_id.as_deref(),
            Some(original.id.as_str())
        );

        let error = core
            .select_generation_target(Some(original_target))
            .expect_err("a retained sibling cannot replace the active legacy target");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            core.get_settings().expect("selection after rejection"),
            reselected
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the reopen scenario keeps one connection fixture and its durable assertions linear"
    )]
    fn approved_lan_connection_reopens_and_drives_preview_and_generation_validation() {
        let root = tempdir().expect("temporary core root");
        let connection_id = ProviderConnectionId::from("approved-lan-core");
        let route_id = ModelRouteId::from("approved-lan-route");
        let preset_id = GenerationPresetId::from("approved-lan-preset");
        {
            let core = Core::open(CoreConfig::new(root.path())).expect("open core");
            let template = core
                .list_provider_templates()
                .expect("provider templates")
                .into_iter()
                .find(|template| template.id.as_str() == "ollama-native-v1")
                .expect("Ollama template");
            let api_origin = CanonicalOrigin::parse("http://ollama.lan:11434").expect("LAN origin");
            let connection = core
                .create_provider_connection(ProviderConnectionDraft {
                    id: connection_id.clone(),
                    template_id: template.id.clone(),
                    template_version: template.manifest_version,
                    display_name: "Approved LAN Ollama".to_owned(),
                    api_origin: api_origin.clone(),
                    api_base_path: Some(EndpointPath::parse("/api").expect("API base path")),
                    network_mode: ProviderNetworkMode::ApprovedLocalNetwork,
                    local_network_approval: Some(ProviderLocalNetworkApproval {
                        origin: api_origin,
                        addresses: vec![
                            "192.168.10.21".parse().expect("LAN address"),
                            "192.168.10.20".parse().expect("LAN address"),
                            "192.168.10.21".parse().expect("duplicate LAN address"),
                        ],
                    }),
                    values: Vec::new(),
                    approved_credential_origin: None,
                    timeout_seconds: 5,
                })
                .expect("create approved LAN connection");
            assert_eq!(
                connection
                    .config
                    .local_network_approval
                    .as_ref()
                    .expect("normalized LAN approval")
                    .addresses,
                vec![
                    "192.168.10.20".parse::<IpAddr>().expect("LAN address"),
                    "192.168.10.21".parse::<IpAddr>().expect("LAN address"),
                ]
            );
            assert_eq!(
                core.list_provider_connections()
                    .expect("provider connections")
                    .into_iter()
                    .find(|candidate| candidate.id == connection_id)
                    .expect("approved LAN connection"),
                connection
            );
            let now = Utc::now();
            core.upsert_model_route(ModelRoute {
                id: route_id.clone(),
                connection_id: connection_id.clone(),
                api_family: template.api_family,
                model_id: "llama-lan".to_owned(),
                display_name: Some("LAN Llama".to_owned()),
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
            })
            .expect("save LAN model route");
            core.upsert_generation_preset(GenerationPreset {
                id: preset_id.clone(),
                model_route_id: route_id.clone(),
                display_name: "LAN defaults".to_owned(),
                values: Vec::new(),
                reasoning: GenerationReasoningSettings {
                    preserve_opaque_state: false,
                    ..GenerationReasoningSettings::default()
                },
                prompt_cache: GenerationPromptCacheSettings::default(),
                created_at: now,
                updated_at: now,
            })
            .expect("save LAN generation preset");
            core.preview_provider_request(&route_id, &preset_id)
                .expect("preview reconstructs persisted LAN policy");
            core.validate_generation_preset(&route_id, &preset_id)
                .expect("generation validation reconstructs persisted LAN policy");
        }
        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
        let connection = reopened
            .list_provider_connections()
            .expect("reopened provider connections")
            .into_iter()
            .find(|candidate| candidate.id == connection_id)
            .expect("reopened approved LAN connection");
        assert_eq!(
            connection.config.network_mode,
            ProviderNetworkMode::ApprovedLocalNetwork
        );
        reopened
            .preview_provider_request(&route_id, &preset_id)
            .expect("reopened preview reconstructs persisted LAN policy");
        reopened
            .validate_generation_preset(&route_id, &preset_id)
            .expect("reopened generation validation reconstructs persisted LAN policy");
    }

    fn assert_directory_does_not_contain(root: &Path, needle: &[u8]) {
        for entry in fs::read_dir(root).expect("read data directory") {
            let entry = entry.expect("data directory entry");
            let path = entry.path();
            if path.is_dir() {
                assert_directory_does_not_contain(&path, needle);
            } else if path.is_file() {
                let contents = fs::read(&path).expect("read persisted data");
                assert!(
                    !contents
                        .windows(needle.len())
                        .any(|window| window == needle),
                    "secret material was persisted in {}",
                    path.display()
                );
            }
        }
    }

    impl CapturingProvider {
        fn new(response: impl Into<String>) -> (Arc<Self>, std_mpsc::Receiver<Vec<String>>) {
            let (sender, receiver) = std_mpsc::channel();
            (
                Arc::new(Self {
                    response: response.into(),
                    captured: Mutex::new(Some(sender)),
                    captured_temperature: Mutex::new(None),
                }),
                receiver,
            )
        }

        fn new_with_temperature_capture(
            response: impl Into<String>,
        ) -> (
            Arc<Self>,
            std_mpsc::Receiver<Vec<String>>,
            std_mpsc::Receiver<Option<f64>>,
        ) {
            let (message_sender, message_receiver) = std_mpsc::channel();
            let (temperature_sender, temperature_receiver) = std_mpsc::channel();
            (
                Arc::new(Self {
                    response: response.into(),
                    captured: Mutex::new(Some(message_sender)),
                    captured_temperature: Mutex::new(Some(temperature_sender)),
                }),
                message_receiver,
                temperature_receiver,
            )
        }
    }

    #[async_trait]
    impl Provider for CapturingProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            if let Some(sender) = self
                .captured_temperature
                .lock()
                .expect("temperature capture lock")
                .take()
            {
                let _ = sender.send(request.temperature);
            }
            if let Some(sender) = self.captured.lock().expect("capture lock").take() {
                let _ = sender.send(
                    request
                        .messages
                        .into_iter()
                        .map(|message| message.content)
                        .collect(),
                );
            }
            sink.send(ProviderEvent::TextDelta(self.response.clone()))
                .await
                .map_err(|_| CoreError::internal("chat event receiver closed"))?;
            Ok(GenerationUsage::default())
        }
    }

    #[async_trait]
    impl Provider for OpaqueContinuityProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: true,
                max_context_tokens: None,
            }
        }

        fn snapshot_request(&self, request: &GenerationRequest) -> CoreResult<serde_json::Value> {
            if request.preserve_opaque_reasoning_state
                || !request.opaque_reasoning_context.is_empty()
            {
                return Err(CoreError::new(
                    CoreErrorCode::UnsupportedContent,
                    "opaque reasoning continuity cannot be stored in a plaintext request snapshot",
                    false,
                ));
            }
            serde_json::to_value(request).map_err(|error| {
                CoreError::internal(format!(
                    "cannot encode synthetic opaque-continuity request snapshot: {error}"
                ))
            })
        }

        async fn generate(
            &self,
            request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            if let Some(sender) = self
                .captured_request
                .lock()
                .expect("opaque request capture lock")
                .take()
            {
                let _ = sender.send((
                    request.preserve_opaque_reasoning_state,
                    request.opaque_reasoning_context,
                    request.provider_provenance,
                ));
            }
            if let Some(state) = self.emitted_state.clone() {
                sink.send(ProviderEvent::OpaqueReasoningState(state))
                    .await
                    .map_err(|_| CoreError::internal("chat event receiver closed"))?;
            }
            sink.send(ProviderEvent::TextDelta(self.response.clone()))
                .await
                .map_err(|_| CoreError::internal("chat event receiver closed"))?;
            Ok(GenerationUsage::default())
        }
    }

    #[async_trait]
    impl Provider for OverflowUsageProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            sink.send(ProviderEvent::TextDelta(
                "response before invalid usage".to_owned(),
            ))
            .await
            .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            Ok(GenerationUsage {
                input_tokens: Some(i64::MAX as u64 + 1),
                output_tokens: Some(1),
                ..GenerationUsage::default()
            })
        }
    }

    #[async_trait]
    impl Provider for SnapshotFailingProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        fn snapshot_request(&self, _request: &GenerationRequest) -> CoreResult<serde_json::Value> {
            Err(CoreError::internal("injected provider snapshot failure"))
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            _sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            panic!("generation must not start after a request snapshot failure")
        }
    }

    impl StallingProvider {
        fn new(partial: impl Into<String>) -> (Arc<Self>, std_mpsc::Receiver<()>) {
            let (started_sender, started_receiver) = std_mpsc::channel();
            (
                Arc::new(Self {
                    partial: partial.into(),
                    started: Mutex::new(Some(started_sender)),
                }),
                started_receiver,
            )
        }
    }

    #[async_trait]
    impl Provider for StallingProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            sink.send(ProviderEvent::TextDelta(self.partial.clone()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            if let Some(sender) = self.started.lock().expect("started lock").take() {
                let _ = sender.send(());
            }
            std::future::pending().await
        }
    }

    impl CatchupSnapshotProvider {
        fn new() -> (
            Arc<Self>,
            std_mpsc::Receiver<()>,
            tokio::sync::oneshot::Sender<()>,
        ) {
            let (started_sender, started_receiver) = std_mpsc::channel();
            let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
            (
                Arc::new(Self {
                    started: Mutex::new(Some(started_sender)),
                    release: Mutex::new(Some(release_receiver)),
                }),
                started_receiver,
                release_sender,
            )
        }
    }

    #[async_trait]
    impl Provider for CatchupSnapshotProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: true,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            sink.send(ProviderEvent::ReasoningDelta("reasoning-prefix".to_owned()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            sink.send(ProviderEvent::TextDelta("text-prefix".to_owned()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            if let Some(sender) = self.started.lock().expect("catch-up started lock").take() {
                let _ = sender.send(());
            }
            let release = self
                .release
                .lock()
                .expect("catch-up release lock")
                .take()
                .expect("catch-up release receiver");
            release
                .await
                .map_err(|_| CoreError::internal("catch-up release sender dropped"))?;
            sink.send(ProviderEvent::ReasoningDelta(
                "+reasoning-suffix".to_owned(),
            ))
            .await
            .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            sink.send(ProviderEvent::TextDelta("+text-suffix".to_owned()))
                .await
                .map_err(|_| CoreError::internal("provider event receiver closed"))?;
            Ok(GenerationUsage::default())
        }
    }

    #[async_trait]
    impl Provider for LeaseBarrierProvider {
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                streaming: true,
                reasoning: false,
                max_context_tokens: None,
            }
        }

        async fn generate(
            &self,
            _request: GenerationRequest,
            _credential: Option<&str>,
            sink: ProviderEventSender,
            _cancelled: watch::Receiver<bool>,
        ) -> CoreResult<GenerationUsage> {
            if let Some(entered) = self.entered.lock().expect("lease entered lock").take() {
                let _ = entered.send(());
            }
            let release = self
                .release
                .lock()
                .expect("lease release lock")
                .take()
                .expect("lease release receiver");
            release
                .await
                .map_err(|_| CoreError::internal("lease test release dropped"))?;
            sink.send(ProviderEvent::TextDelta("completed".to_owned()))
                .await
                .map_err(|_| CoreError::internal("lease test event receiver closed"))?;
            Ok(GenerationUsage::default())
        }
    }

    fn imported_core() -> (tempfile::TempDir, Core, Character) {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let mut card = NamedTempFile::new_in(root.path()).expect("card");
        write!(
            card,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Segu","description":"Guide"}}}}"#
        )
        .expect("write card");
        let inspection = core.inspect_import(card.path()).expect("inspect");
        let character = core.commit_import(&inspection.id).expect("commit");
        (root, core, character)
    }

    const HARD_CRASH_GENERATION_ROOT_ENV: &str = "LOREPIA_TEST_HARD_CRASH_GENERATION_ROOT";
    const HARD_CRASH_GENERATION_PRESERVE_ENV: &str = "LOREPIA_TEST_HARD_CRASH_GENERATION_PRESERVE";
    const HARD_CRASH_GENERATION_REOPEN_PRESERVE_ENV: &str =
        "LOREPIA_TEST_HARD_CRASH_GENERATION_REOPEN_PRESERVE";
    const HARD_CRASH_GENERATION_PARTIAL_ENV: &str = "LOREPIA_TEST_HARD_CRASH_GENERATION_PARTIAL";
    const HARD_CRASH_GENERATION_EXIT_CODE: i32 = 86;

    #[derive(Debug, Serialize, serde::Deserialize)]
    struct HardCrashGenerationFixture {
        conversation_id: String,
        branch_id: String,
        user_message_id: String,
        assistant_message_id: String,
        generation_id: String,
        running_attempt_revision: u64,
        partial: String,
    }

    fn hard_crash_generation_fixture_path(root: &Path) -> PathBuf {
        root.join("hard-crash-generation-fixture.json")
    }

    fn run_hard_crash_generation_child(
        root: &Path,
        preserve_partial_generations: bool,
        reopen_preserve_partial_generations: bool,
        partial: &str,
    ) -> HardCrashGenerationFixture {
        let output = Command::new(std::env::current_exe().expect("current Core test executable"))
            .arg("--exact")
            .arg("app::tests::hard_crash_generation_fixture_child")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(HARD_CRASH_GENERATION_ROOT_ENV, root)
            .env(
                HARD_CRASH_GENERATION_PRESERVE_ENV,
                if preserve_partial_generations {
                    "true"
                } else {
                    "false"
                },
            )
            .env(
                HARD_CRASH_GENERATION_REOPEN_PRESERVE_ENV,
                if reopen_preserve_partial_generations {
                    "true"
                } else {
                    "false"
                },
            )
            .env(HARD_CRASH_GENERATION_PARTIAL_ENV, partial)
            .output()
            .expect("run hard-crash generation child");
        assert_eq!(
            output.status.code(),
            Some(HARD_CRASH_GENERATION_EXIT_CODE),
            "hard-crash child did not reach its deliberate process exit\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        let fixture = fs::read(hard_crash_generation_fixture_path(root))
            .expect("read hard-crash generation fixture");
        serde_json::from_slice(&fixture).expect("decode hard-crash generation fixture")
    }

    #[derive(Debug, PartialEq, Eq)]
    struct GenerationLifecycleRow {
        occurrence_id: String,
        event_kind: String,
        status: String,
        exact_head_message_id: Option<String>,
        owner_message_id: Option<String>,
    }

    fn hard_crash_database_path(root: &Path) -> PathBuf {
        fs::read_dir(root.join("db/schema-cutover"))
            .expect("read hard-crash database generations")
            .filter_map(|entry| {
                let entry = entry.expect("read hard-crash database generation");
                let manifest = fs::read(entry.path().join("generation-manifest.json")).ok()?;
                let manifest = serde_json::from_slice::<serde_json::Value>(&manifest)
                    .expect("decode hard-crash database manifest");
                Some((
                    manifest["activation_sequence"]
                        .as_u64()
                        .expect("hard-crash database activation sequence"),
                    root.join(
                        manifest["active_database_relative_path"]
                            .as_str()
                            .expect("hard-crash active database path"),
                    ),
                ))
            })
            .max_by_key(|(sequence, _)| *sequence)
            .map(|(_, path)| path)
            .expect("committed hard-crash database generation")
    }

    fn generation_lifecycle_rows(root: &Path, generation_id: &str) -> Vec<GenerationLifecycleRow> {
        let database = rusqlite::Connection::open(hard_crash_database_path(root))
            .expect("open lifecycle database");
        let mut statement = database
            .prepare(
                "SELECT occurrence_id, event_kind, status,
                        exact_head_message_id, owner_message_id
                 FROM core_lifecycle_outbox
                 WHERE generation_id = ?1
                   AND event_kind IN ('after_generation', 'message_committed')
                 ORDER BY occurrence_id",
            )
            .expect("prepare lifecycle query");
        statement
            .query_map([generation_id], |row| {
                Ok(GenerationLifecycleRow {
                    occurrence_id: row.get(0)?,
                    event_kind: row.get(1)?,
                    status: row.get(2)?,
                    exact_head_message_id: row.get(3)?,
                    owner_message_id: row.get(4)?,
                })
            })
            .expect("query lifecycle rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect lifecycle rows")
    }

    fn hard_crash_assistant_content(root: &Path, assistant_message_id: &str) -> String {
        rusqlite::Connection::open(hard_crash_database_path(root))
            .expect("open hard-crash message database")
            .query_row(
                "SELECT content FROM messages WHERE id = ?1",
                [assistant_message_id],
                |row| row.get(0),
            )
            .expect("read durable hard-crash assistant content")
    }

    fn prompt_attempt_test_provenance(source_id: &str) -> Provenance {
        Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: Some(source_id.to_owned()),
            source_hash: Some("a5".repeat(32)),
            author: Some("Synthetic prompt-attempt test".to_owned()),
            license: Some("LicenseRef-Synthetic-Test".to_owned()),
            imported_at: None,
        }
    }

    fn install_generation_transform_fixture(
        core: &Core,
        conversation_id: &ConversationId,
        transform_set: &TransformSet,
        preset_id: &str,
        binding_id: &str,
    ) -> String {
        let transform_revision = core
            .upsert_transform_set(transform_set, None)
            .expect("save generation transform set")
            .revision_id
            .expect("generation transform set revision id");
        let now = Utc::now();
        let mut prompt_preset = lorepia_orchestration::default_prompt_preset(
            lorepia_domain::PromptPresetId::from(preset_id),
            "Synthetic generation transform preset",
            PresetMetadata {
                description: "Synthetic terminal transform fixture".to_owned(),
                tags: vec!["synthetic".to_owned()],
                provenance: prompt_attempt_test_provenance(preset_id),
                created_at: now,
                updated_at: now,
                local_override_of: None,
            },
        );
        for block in &mut prompt_preset.blocks {
            block.provenance = prompt_attempt_test_provenance(block.id.as_str());
        }
        prompt_preset
            .transform_set_ids
            .push(transform_set.id.clone());
        core.upsert_prompt_preset(&prompt_preset, None)
            .expect("save generation transform prompt preset");
        core.bind_prompt_preset(
            &PromptPresetBinding {
                id: binding_id.to_owned(),
                prompt_preset_id: prompt_preset.id,
                scope: ModuleScope::Conversation,
                target_id: Some(conversation_id.0.clone()),
                conversation_id: None,
                pinned_revision_id: None,
                priority: 0,
                enabled: true,
                response_length: PromptResponseLength::Balanced,
                creativity: 50,
                reasoning_effort: None,
                memory_enabled: true,
                knowledge_enabled: true,
                variable_overrides: VariableMap::default(),
                generation_preset_override_id: None,
                user_name_override: None,
                author_note: None,
                group_context: None,
                template_slots: Vec::new(),
                created_at: now,
                updated_at: now,
            },
            None,
        )
        .expect("bind generation transform prompt preset");
        transform_revision
    }

    fn fail_open_generation_transform_set() -> TransformSet {
        let set_id = TransformSetId::from("synthetic.fail-open.set");
        TransformSet {
            id: set_id.clone(),
            name: "Synthetic fail-open transforms".to_owned(),
            schema_version: 1,
            enabled: true,
            imported_author_enabled: false,
            rules: vec![
                TransformRule {
                    id: TransformRuleId::from("synthetic.fail-open.invalid-regex"),
                    name: "Invalid regex".to_owned(),
                    enabled: true,
                    imported_enabled: false,
                    imported_author_enabled: false,
                    phase: TransformPhase::ProviderOutputCanonical,
                    order: 0,
                    pattern: SafeRegex {
                        pattern: "(".to_owned(),
                        case_insensitive: false,
                    },
                    replacement: "must-not-appear".to_owned(),
                    condition: None,
                    max_replacements: 8,
                    input_limit: 1_024,
                    output_limit: 1_024,
                    provenance: prompt_attempt_test_provenance("invalid-regex"),
                },
                TransformRule {
                    id: TransformRuleId::from("synthetic.fail-open.output-limit"),
                    name: "Output limit".to_owned(),
                    enabled: true,
                    imported_enabled: false,
                    imported_author_enabled: false,
                    phase: TransformPhase::ProviderOutputCanonical,
                    order: 1,
                    pattern: SafeRegex {
                        pattern: "Synthetic".to_owned(),
                        case_insensitive: false,
                    },
                    replacement: "X".repeat(64),
                    condition: None,
                    max_replacements: 8,
                    input_limit: 1_024,
                    output_limit: 32,
                    provenance: prompt_attempt_test_provenance("output-limit"),
                },
            ],
            max_rules_per_phase: 8,
            max_output_chars: 1_024,
            provenance: prompt_attempt_test_provenance(set_id.as_str()),
        }
    }

    fn display_only_generation_transform_set() -> TransformSet {
        let set_id = TransformSetId::from("synthetic.display-only.set");
        TransformSet {
            id: set_id.clone(),
            name: "Synthetic DisplayOnly".to_owned(),
            schema_version: 1,
            enabled: true,
            imported_author_enabled: false,
            rules: vec![TransformRule {
                id: TransformRuleId::from("synthetic.display-only.rule"),
                name: "Render-only wording".to_owned(),
                enabled: true,
                imported_enabled: false,
                imported_author_enabled: false,
                phase: TransformPhase::DisplayOnly,
                order: 0,
                pattern: SafeRegex {
                    pattern: "Synthetic".to_owned(),
                    case_insensitive: false,
                },
                replacement: "Rendered".to_owned(),
                condition: None,
                max_replacements: 8,
                input_limit: 1_024,
                output_limit: 1_024,
                provenance: prompt_attempt_test_provenance("synthetic.display-only.rule"),
            }],
            max_rules_per_phase: 8,
            max_output_chars: 1_024,
            provenance: prompt_attempt_test_provenance(set_id.as_str()),
        }
    }

    fn assert_display_only_events(events: &[ChatEvent], generation_id: &GenerationId) -> String {
        let events = events
            .iter()
            .filter(|event| event.generation_id == *generation_id)
            .collect::<Vec<_>>();
        assert_eq!(events.first().map(|event| event.sequence), Some(1));
        assert!(
            events
                .windows(2)
                .all(|events| events[1].sequence > events[0].sequence)
        );
        let streamed_display = events
            .iter()
            .filter_map(|event| match &event.kind {
                ChatEventKind::TextDelta(delta) => Some(delta.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(streamed_display, "Rendered reply");
        let display_delta = events
            .iter()
            .position(|event| matches!(event.kind, ChatEventKind::TextDelta(_)))
            .expect("deferred DisplayOnly text delta");
        let committed = events
            .iter()
            .position(|event| matches!(event.kind, ChatEventKind::MessageCommitted { .. }))
            .expect("message committed event");
        let finished = events
            .iter()
            .position(|event| matches!(event.kind, ChatEventKind::GenerationFinished))
            .expect("generation finished event");
        assert!(display_delta < committed && committed < finished);
        streamed_display
    }

    fn assert_display_only_projection(
        presentation: &MessagePresentation,
        transform_revision_id: &str,
        streamed_display: &str,
    ) {
        assert_eq!(presentation.message.content, "Synthetic reply");
        assert_eq!(presentation.display_content, streamed_display);
        assert!(presentation.projection_diagnostics_sha256.is_some());
        assert_eq!(presentation.transform_diagnostics.len(), 1);
        let diagnostic = &presentation.transform_diagnostics[0];
        assert_eq!(
            diagnostic.set_revision_id.as_deref(),
            Some(transform_revision_id)
        );
        assert_eq!(
            diagnostic.rule_id.as_deref(),
            Some("synthetic.display-only.rule")
        );
        assert_eq!(diagnostic.stage, MessageTransformStage::DisplayOnly);
        assert_eq!(diagnostic.disposition, MessageTransformDisposition::Applied);
        assert!(diagnostic.code.is_none());
        assert_eq!(
            diagnostic.before_sha256,
            transform_content_sha256("Synthetic reply")
        );
        assert_eq!(
            diagnostic.after_sha256.as_ref(),
            Some(&transform_content_sha256("Rendered reply"))
        );
        let diagnostic_json =
            serde_json::to_string(diagnostic).expect("serialize public transform diagnostic");
        for forbidden in ["Synthetic reply", "Rendered reply", "Synthetic", "Rendered"] {
            assert!(!diagnostic_json.contains(forbidden));
        }
    }

    fn prompt_source_test_block(
        preset_id: &lorepia_domain::PromptPresetId,
        suffix: &str,
        name: &str,
        kind: PromptBlockKind,
        source: BlockSource,
        placement_zone: PlacementZone,
    ) -> PromptBlock {
        PromptBlock {
            id: PromptBlockId::from(format!("{}.{}", preset_id.as_str(), suffix)),
            name: name.to_owned(),
            kind,
            enabled: true,
            role_hint: RoleHint::System,
            authority: InstructionAuthority::Creator,
            template: None,
            condition: None,
            source,
            placement_zone,
            history_selector: None,
            token_policy: TokenPolicy {
                priority: 900,
                min_tokens: None,
                max_tokens: Some(1_024),
                reserve_tokens: None,
            },
            overflow_policy: OverflowPolicy::TrimTail,
            merge_policy: MergePolicy::SeparateMessage,
            provenance: prompt_attempt_test_provenance(suffix),
        }
    }

    fn prompt_source_test_preset(summary_id: &MemoryRecordId) -> PromptPreset {
        let now = Utc::now();
        let preset_id = lorepia_domain::PromptPresetId::from("synthetic.prompt-source.preset");
        let mut preset = lorepia_orchestration::default_prompt_preset(
            preset_id.clone(),
            "Synthetic prompt sources",
            PresetMetadata {
                description: "Synthetic current-source materialization fixture".to_owned(),
                tags: vec!["synthetic".to_owned()],
                provenance: prompt_attempt_test_provenance(preset_id.as_str()),
                created_at: now,
                updated_at: now,
                local_override_of: None,
            },
        );
        for block in &mut preset.blocks {
            block.provenance = prompt_attempt_test_provenance(block.id.as_str());
        }
        let mut character = preset.blocks.remove(0);
        let mut history = preset.blocks.remove(0);
        let latest = preset.blocks.remove(0);
        character.name = "Synthetic character source".to_owned();
        history.history_selector = Some(HistorySelector::SinceSummary {
            summary_id: summary_id.clone(),
        });
        let mut user_and_slot = prompt_source_test_block(
            &preset_id,
            "user-slot",
            "User and slot",
            PromptBlockKind::StaticInstruction,
            BlockSource::Template,
            PlacementZone::PresetInstruction,
        );
        user_and_slot.template = Some(SafeTemplate {
            parts: vec![
                TemplatePart::Text {
                    value: "USER=".to_owned(),
                },
                TemplatePart::BuiltIn {
                    value: BuiltInTemplateValue::UserName,
                },
                TemplatePart::Text {
                    value: "; SLOT=".to_owned(),
                },
                TemplatePart::Slot {
                    name: "tone".to_owned(),
                },
            ],
            max_output_chars: 1_024,
        });
        let group = prompt_source_test_block(
            &preset_id,
            "group",
            "Group context",
            PromptBlockKind::GroupContext,
            BlockSource::GroupContext,
            PlacementZone::CharacterContext,
        );
        let summary = prompt_source_test_block(
            &preset_id,
            "summary",
            "Conversation summary",
            PromptBlockKind::ConversationSummary,
            BlockSource::ConversationSummary,
            PlacementZone::RetrievedContext,
        );
        let author = prompt_source_test_block(
            &preset_id,
            "author",
            "Author note",
            PromptBlockKind::AuthorNote,
            BlockSource::AuthorNote,
            PlacementZone::PostHistory,
        );
        preset.blocks = vec![
            user_and_slot,
            character,
            group,
            summary,
            history,
            author,
            latest,
        ];
        preset
    }

    fn save_prompt_source_summary(
        core: &Core,
        branch_id: &ConversationBranchId,
        messages: &[Message],
    ) -> StoredRevision<MemoryRecord> {
        let [user, assistant] = messages else {
            panic!("prompt-source summary fixture requires one complete turn");
        };
        let now = Utc::now();
        let summary_id = MemoryRecordId::from("synthetic.prompt-source.summary");
        core.inner
            .storage
            .save_memory_record(
                &MemoryRecord {
                    id: summary_id.clone(),
                    conversation_id: user.conversation_id.clone(),
                    branch_id: branch_id.clone(),
                    source_start_message_id: user.id.clone(),
                    source_end_message_id: assistant.id.clone(),
                    kind: MemoryKind::ConversationSummary,
                    title: "Synthetic exact prompt summary".to_owned(),
                    summary: "SUMMARY_SOURCE_CANARY_7A31".to_owned(),
                    structured_data: VersionedJson {
                        schema_version: 1,
                        value: serde_json::json!({"fixture": "prompt-source"}),
                    },
                    importance: 100,
                    keywords: vec!["synthetic".to_owned()],
                    embedding_ref: None,
                    pinned: false,
                    excluded_from_conversation: false,
                    excluded_from_character: false,
                    created_at: now,
                    updated_at: now,
                    invalidated_at: None,
                    provenance: prompt_attempt_test_provenance(summary_id.as_str()),
                },
                None,
            )
            .expect("save prompt-source summary")
    }

    fn bind_prompt_source_test_preset(
        core: &Core,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        summary_id: &MemoryRecordId,
    ) -> StoredRevision<PromptPresetBinding> {
        let preset = prompt_source_test_preset(summary_id);
        core.upsert_prompt_preset(&preset, None)
            .expect("save prompt-source preset");
        let now = Utc::now();
        core.bind_prompt_preset(
            &PromptPresetBinding {
                id: "synthetic.prompt-source.binding".to_owned(),
                prompt_preset_id: preset.id,
                scope: ModuleScope::Branch,
                target_id: Some(branch_id.0.clone()),
                conversation_id: Some(conversation_id.clone()),
                pinned_revision_id: None,
                priority: 0,
                enabled: true,
                response_length: PromptResponseLength::Balanced,
                creativity: 50,
                reasoning_effort: None,
                memory_enabled: true,
                knowledge_enabled: true,
                variable_overrides: VariableMap::default(),
                generation_preset_override_id: None,
                user_name_override: Some("USER_SOURCE_CANARY_2B64".to_owned()),
                author_note: Some("AUTHOR_SOURCE_CANARY_4C82".to_owned()),
                group_context: Some("GROUP_SOURCE_CANARY_1D53".to_owned()),
                template_slots: vec![TemplateSlot {
                    name: "tone".to_owned(),
                    value: "SLOT_SOURCE_CANARY_9E17".to_owned(),
                }],
                created_at: now,
                updated_at: now,
            },
            None,
        )
        .expect("bind prompt-source preset")
    }

    fn assert_prompt_source_preview(preview: &crate::ExpertPromptPreview) {
        let block_contents = |suffix: &str| {
            preview
                .effective_messages
                .iter()
                .filter(|message| message.block_id.as_str().ends_with(suffix))
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            block_contents(".user-slot"),
            ["USER=USER_SOURCE_CANARY_2B64; SLOT=SLOT_SOURCE_CANARY_9E17"]
        );
        assert_eq!(block_contents(".group"), ["GROUP_SOURCE_CANARY_1D53"]);
        assert_eq!(block_contents(".summary"), ["SUMMARY_SOURCE_CANARY_7A31"]);
        assert_eq!(block_contents(".author"), ["AUTHOR_SOURCE_CANARY_4C82"]);
        assert_eq!(
            block_contents(".history"),
            [
                "SINCE_SUMMARY_USER_CANARY_54C9",
                "SINCE_SUMMARY_ASSISTANT_CANARY_86E2"
            ]
        );
        assert_eq!(
            block_contents(".latest_user"),
            ["LATEST_USER_SOURCE_CANARY_03F8"]
        );
    }

    fn assert_prompt_source_snapshot(
        snapshot: &PromptContextSnapshotV1,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        messages: &[Message],
        summary: &StoredRevision<MemoryRecord>,
        binding: &StoredRevision<PromptPresetBinding>,
    ) {
        assert_eq!(snapshot.conversation_id, *conversation_id);
        assert_eq!(snapshot.source_branch_id, *branch_id);
        assert_eq!(
            snapshot.context_head_message_id.as_ref(),
            Some(&messages[3].id)
        );
        assert_eq!(
            snapshot.conversation_summary_id.as_ref(),
            Some(&summary.value.id)
        );
        assert_eq!(snapshot.summaries.len(), 1);
        assert_eq!(
            snapshot.summaries[0].source_start_message_id,
            messages[0].id
        );
        assert_eq!(snapshot.summaries[0].source_end_message_id, messages[1].id);
        assert_eq!(snapshot.summaries[0].state_revision, summary.revision);
        assert_eq!(
            snapshot.summaries[0].active_revision_id.as_str(),
            summary
                .revision_id
                .as_deref()
                .expect("summary revision identity")
        );
        let summary_json =
            serde_json::to_string(&summary.value).expect("encode exact summary revision");
        assert_eq!(
            snapshot.summaries[0].active_revision_sha256,
            format!("{:x}", Sha256::digest(summary_json.as_bytes()))
        );
        let snapshot_binding = snapshot.binding.as_ref().expect("binding evidence");
        assert_eq!(snapshot_binding.binding_id, binding.value.id);
        assert_eq!(snapshot_binding.binding_revision, binding.revision);
        assert_eq!(
            snapshot_binding.document_sha256,
            binding
                .value
                .canonical_document_sha256()
                .expect("hash exact prompt binding")
        );
        assert_eq!(
            snapshot.snapshot_sha256,
            lorepia_domain::prompt_context_snapshot_sha256(snapshot)
                .expect("rehash prompt context snapshot")
        );
        let snapshot_json = serde_json::to_string(snapshot).expect("encode prompt source evidence");
        for source_text in [
            "USER_SOURCE_CANARY_2B64",
            "AUTHOR_SOURCE_CANARY_4C82",
            "GROUP_SOURCE_CANARY_1D53",
            "SLOT_SOURCE_CANARY_9E17",
            "SUMMARY_SOURCE_CANARY_7A31",
            "SINCE_SUMMARY_USER_CANARY_54C9",
        ] {
            assert!(!snapshot_json.contains(source_text));
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the fixture keeps one exact module revision self-contained for parity validation"
    )]
    fn install_prompt_attempt_parity_module(
        core: &Core,
        runtime_target: ContentModuleRuntimeTarget,
    ) -> (VariableRef, VariableRef, KnowledgeEntryId, String) {
        let module_id = ContentModuleId::from("synthetic.prompt-attempt.parity-module");
        let variable = VariableRef {
            scope: VariableScope::Module,
            namespace: Some(module_id.clone()),
            id: VariableId::from("synthetic.prompt-attempt.parity-module.approval-state"),
        };
        let temporal_roll = VariableRef {
            scope: VariableScope::Module,
            namespace: Some(module_id.clone()),
            id: VariableId::from("synthetic.prompt-attempt.parity-module.temporal-roll"),
        };
        let knowledge_book_id = KnowledgeBookId::from("synthetic.prompt-attempt.knowledge");
        let knowledge_entry_id =
            KnowledgeEntryId::from("synthetic.prompt-attempt.knowledge.manual");
        core.upsert_knowledge_book(
            &KnowledgeBook {
                id: knowledge_book_id.clone(),
                name: "Synthetic attempt-owned knowledge".to_owned(),
                schema_version: 1,
                entries: vec![KnowledgeEntry {
                    id: knowledge_entry_id.clone(),
                    book_id: knowledge_book_id.clone(),
                    name: "Attempt-owned manual knowledge".to_owned(),
                    content: "SYNTHETIC_ATTEMPT_MANUAL_KNOWLEDGE_6A91".to_owned(),
                    enabled: true,
                    activation: ActivationRule::Manual,
                    priority: 100,
                    importance: 100,
                    placement: KnowledgePlacement::RetrievedContext,
                    token_policy: TokenPolicy {
                        priority: 100,
                        min_tokens: None,
                        max_tokens: Some(64),
                        reserve_tokens: None,
                    },
                    parent_id: None,
                    activation_probability_basis_points: 10_000,
                    provenance: prompt_attempt_test_provenance(
                        "synthetic.prompt-attempt.knowledge.manual",
                    ),
                }],
                scan_depth: 8,
                token_budget: TokenBudget { max_tokens: 64 },
                recursive: false,
                max_recursion_depth: 0,
                provenance: prompt_attempt_test_provenance("synthetic.prompt-attempt.knowledge"),
            },
            None,
        )
        .expect("save attempt-owned knowledge book");

        let proposal_id = "synthetic.prompt-attempt.approval".to_owned();
        let rule_set = InteractionRuleSet {
            id: InteractionRuleSetId::from("synthetic.prompt-attempt.rules"),
            name: "Synthetic prompt-attempt rules".to_owned(),
            schema_version: 1,
            rules: vec![
                InteractionRule {
                    id: InteractionRuleId::from("synthetic.prompt-attempt.rules.before"),
                    name: "Prepare prompt state before generation".to_owned(),
                    enabled: true,
                    imported_author_enabled: false,
                    event: InteractionEvent::BeforeGeneration,
                    condition: None,
                    actions: vec![
                        InteractionAction::SetVariable {
                            target: variable.clone(),
                            value: ValueExpr::Literal {
                                value: VariableValue::Text("before-approval".to_owned()),
                            },
                        },
                        InteractionAction::ActivateKnowledge {
                            entry_id: knowledge_entry_id.clone(),
                        },
                        InteractionAction::RollDice {
                            expression: DiceExpression {
                                count: 1,
                                sides: 10_000,
                                modifier: 0,
                            },
                            target: Some(temporal_roll.clone()),
                        },
                        InteractionAction::AppendVisibleSystemEvent {
                            text: SafeTemplate {
                                parts: vec![TemplatePart::BuiltIn {
                                    value: BuiltInTemplateValue::CurrentTime,
                                }],
                                max_output_chars: 64,
                            },
                        },
                        InteractionAction::RequestUserApproval {
                            proposal: ProposalSpec {
                                id: proposal_id.clone(),
                                title: "Approve synthetic prompt state".to_owned(),
                                body: SafeTemplate {
                                    parts: vec![TemplatePart::BuiltIn {
                                        value: BuiltInTemplateValue::CurrentTime,
                                    }],
                                    max_output_chars: 64,
                                },
                                expires_after_seconds: None,
                            },
                        },
                    ],
                    priority: 0,
                    stop_after_match: false,
                    provenance: prompt_attempt_test_provenance(
                        "synthetic.prompt-attempt.rules.before",
                    ),
                },
                InteractionRule {
                    id: InteractionRuleId::from("synthetic.prompt-attempt.rules.approved"),
                    name: "Apply approved prompt state".to_owned(),
                    enabled: true,
                    imported_author_enabled: false,
                    event: InteractionEvent::UserAction {
                        action_id: proposal_id.clone(),
                    },
                    condition: None,
                    actions: vec![InteractionAction::SetVariable {
                        target: variable.clone(),
                        value: ValueExpr::Literal {
                            value: VariableValue::Text("approved-for-prompt".to_owned()),
                        },
                    }],
                    priority: 0,
                    stop_after_match: false,
                    provenance: prompt_attempt_test_provenance(
                        "synthetic.prompt-attempt.rules.approved",
                    ),
                },
            ],
            max_actions_per_event: 8,
            provenance: prompt_attempt_test_provenance("synthetic.prompt-attempt.rules"),
        };
        core.upsert_interaction_rule_set(&rule_set, None)
            .expect("save prompt-attempt rule set");

        let module = ContentModule {
            id: module_id.clone(),
            name: "Synthetic prompt-attempt parity module".to_owned(),
            version: "1.0.0".to_owned(),
            schema_version: 1,
            prompt_fragments: vec![
                PromptBlock {
                    id: PromptBlockId::from("synthetic.prompt-attempt.variable-block"),
                    name: "Attempt-owned variable marker".to_owned(),
                    kind: PromptBlockKind::StaticInstruction,
                    enabled: true,
                    role_hint: RoleHint::System,
                    authority: InstructionAuthority::Creator,
                    template: Some(SafeTemplate {
                        parts: vec![
                            TemplatePart::Text {
                                value: "SYNTHETIC_ATTEMPT_VARIABLE=".to_owned(),
                            },
                            TemplatePart::Variable {
                                variable: variable.clone(),
                            },
                            TemplatePart::Text {
                                value: ";SYNTHETIC_ATTEMPT_TIME_ROLL=".to_owned(),
                            },
                            TemplatePart::Variable {
                                variable: temporal_roll.clone(),
                            },
                            TemplatePart::Text {
                                value: ";SYNTHETIC_ATTEMPT_DATE=".to_owned(),
                            },
                            TemplatePart::BuiltIn {
                                value: BuiltInTemplateValue::CurrentDate,
                            },
                            TemplatePart::Text {
                                value: ";SYNTHETIC_ATTEMPT_TIME=".to_owned(),
                            },
                            TemplatePart::BuiltIn {
                                value: BuiltInTemplateValue::CurrentTime,
                            },
                        ],
                        max_output_chars: 256,
                    }),
                    condition: None,
                    source: BlockSource::Template,
                    placement_zone: PlacementZone::AssistantPrefill,
                    history_selector: None,
                    token_policy: TokenPolicy {
                        priority: 1_000,
                        min_tokens: None,
                        max_tokens: Some(64),
                        reserve_tokens: None,
                    },
                    overflow_policy: OverflowPolicy::Reject,
                    merge_policy: MergePolicy::SeparateMessage,
                    provenance: prompt_attempt_test_provenance(
                        "synthetic.prompt-attempt.variable-block",
                    ),
                },
                PromptBlock {
                    id: PromptBlockId::from("synthetic.prompt-attempt.knowledge-block"),
                    name: "Attempt-owned selected knowledge".to_owned(),
                    kind: PromptBlockKind::WorldKnowledge,
                    enabled: true,
                    role_hint: RoleHint::System,
                    authority: InstructionAuthority::Creator,
                    template: None,
                    condition: None,
                    source: BlockSource::SelectedKnowledge,
                    placement_zone: PlacementZone::RetrievedContext,
                    history_selector: None,
                    token_policy: TokenPolicy {
                        priority: 1_000,
                        min_tokens: None,
                        max_tokens: Some(64),
                        reserve_tokens: None,
                    },
                    overflow_policy: OverflowPolicy::Reject,
                    merge_policy: MergePolicy::SeparateMessage,
                    provenance: prompt_attempt_test_provenance(
                        "synthetic.prompt-attempt.knowledge-block",
                    ),
                },
            ],
            knowledge_book_ids: vec![knowledge_book_id],
            control_specs: vec![
                ControlSpec {
                    id: ControlId::from("synthetic.prompt-attempt.approval-state"),
                    label: "Synthetic approval state".to_owned(),
                    description: "Synthetic test-only variable".to_owned(),
                    kind: ControlKind::Text,
                    value_type: Some(VariableType::Text),
                    variable: Some(variable.clone()),
                    default_value: Some(VariableValue::Text("initial".to_owned())),
                    options: Vec::new(),
                    minimum: None,
                    maximum: None,
                    step: None,
                    visible_when: None,
                    scope: VariableScope::Module,
                    sensitive: false,
                    requires_regeneration: true,
                },
                ControlSpec {
                    id: ControlId::from("synthetic.prompt-attempt.temporal-roll"),
                    label: "Synthetic temporal roll".to_owned(),
                    description: "Attempt-time seeded synthetic test variable".to_owned(),
                    kind: ControlKind::Number,
                    value_type: Some(VariableType::Integer),
                    variable: Some(temporal_roll.clone()),
                    default_value: Some(VariableValue::Integer(0)),
                    options: Vec::new(),
                    minimum: Some(0.0),
                    maximum: Some(10_000.0),
                    step: Some(1.0),
                    visible_when: None,
                    scope: VariableScope::Module,
                    sensitive: false,
                    requires_regeneration: true,
                },
            ],
            transform_set_ids: Vec::new(),
            interaction_rule_set_ids: vec![rule_set.id],
            asset_ids: Vec::new(),
            imported_components_enabled: true,
            required_capabilities: vec![
                ContentCapability::PromptFragments,
                ContentCapability::Knowledge,
                ContentCapability::Variables,
                ContentCapability::DeclarativeInteractions,
            ],
            metadata: PackageMetadata {
                author: Some("Synthetic prompt-attempt test".to_owned()),
                license: "LicenseRef-Synthetic-Test".to_owned(),
                redistribution_allowed: false,
                homepage: None,
                description: "Synthetic prompt-attempt parity fixture".to_owned(),
                tags: vec!["synthetic".to_owned()],
                provenance: prompt_attempt_test_provenance(
                    "synthetic.prompt-attempt.parity-module",
                ),
            },
        };
        core.upsert_content_module(&module, None)
            .expect("save prompt-attempt parity module");
        let mut initial_variables = VariableMap::default();
        initial_variables.insert(variable.clone(), VariableValue::Text("initial".to_owned()));
        initial_variables.insert(temporal_roll.clone(), VariableValue::Integer(0));
        let request = ContentModuleActivationRequest {
            runtime_target,
            expected_binding_revision: None,
            binding: ContentModuleBindingDraft {
                id: ModuleBindingId::from("synthetic.prompt-attempt.parity-binding"),
                module_id,
                scope: ModuleScope::App,
                target_id: None,
                conversation_id: None,
                priority: 0,
                resolution_mode: ModuleRevisionResolutionMode::Active,
                pinned_revision_id: None,
                package_import_approval_id: None,
                variable_overrides: initial_variables,
            },
        };
        let review = core
            .review_content_module_activation(&request)
            .expect("review prompt-attempt module activation");
        let resolutions = ModuleMergeResolutionSet {
            expected_review_sha256: review.review_sha256.clone(),
            resolutions: Vec::new(),
        };
        let plan = core
            .resolve_content_module_activation(&request, &resolutions)
            .expect("resolve prompt-attempt module activation");
        core.activate_content_module(
            &request,
            &resolutions,
            &ModuleActivationApproval {
                approval_id: "synthetic-prompt-attempt-activation".to_owned(),
                expected_review_sha256: review.review_sha256,
                expected_plan_sha256: plan.plan_sha256,
            },
        )
        .expect("activate prompt-attempt module")
        .verify()
        .expect("verify prompt-attempt activation receipt");
        (variable, temporal_roll, knowledge_entry_id, proposal_id)
    }

    fn poison_generation_registry(core: &Core) {
        let registry = Arc::clone(&core.inner.active_generations);
        let result = thread::spawn(move || {
            let _guard = registry.active.lock().expect("registry lock");
            panic!("synthetic generation registry failure");
        })
        .join();
        assert!(result.is_err(), "registry poison thread must panic");
    }

    fn wait_for_partial(core: &Core, conversation_id: &ConversationId, expected: &str) -> Message {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let messages = core.list_messages(conversation_id).expect("messages");
            if let Some(message) = messages.get(1)
                && message.content == expected
            {
                return message.clone();
            }
            assert!(
                Instant::now() < deadline,
                "partial checkpoint was not persisted"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_generation_status(
        core: &Core,
        generation_id: &GenerationId,
        expected: GenerationStatus,
    ) -> GenerationRecord {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let generation = core
                .inner
                .storage
                .get_generation(generation_id)
                .expect("generation snapshot");
            if generation.status == expected {
                return generation;
            }
            assert!(
                Instant::now() < deadline,
                "generation did not reach {expected:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_generation_registry_to_drain(core: &Core) {
        wait_for_active_generation_count(core, 0);
    }

    fn wait_for_active_generation_count(core: &Core, expected: usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while core.active_generation_count() != expected {
            assert!(
                Instant::now() < deadline,
                "generation registry did not reach {expected} active entries"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn health_reports_storage_state() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let expected_schema_version = core
            .inner
            .storage
            .schema_version()
            .expect("storage schema version");
        let health = core.health_check().expect("health");
        assert!(health.database_open);
        assert!(health.data_root_writable);
        assert_eq!(health.schema_version, expected_schema_version);
    }

    #[test]
    fn provider_template_listing_exposes_only_each_latest_manifest_version() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let built_in = core
            .list_provider_templates()
            .expect("built-in provider templates")
            .into_iter()
            .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
            .expect("OpenAI-compatible template");
        assert_eq!(built_in.manifest_version, 3);

        let mut version_one = built_in.clone();
        version_one.id = "synthetic-template-history".into();
        version_one.display_name = "Synthetic template history".to_owned();
        version_one.manifest_version = 1;
        let mut version_two = version_one.clone();
        version_two.manifest_version = 2;
        core.inner
            .storage
            .save_provider_template(&version_one)
            .expect("save historical template");
        core.inner
            .storage
            .save_provider_template(&version_two)
            .expect("save latest template");

        let stored_versions = core
            .inner
            .storage
            .list_provider_templates()
            .expect("stored template history")
            .into_iter()
            .filter(|template| template.id == version_one.id)
            .map(|template| template.manifest_version)
            .collect::<Vec<_>>();
        assert_eq!(stored_versions, vec![2, 1]);

        let exposed = core
            .list_provider_templates()
            .expect("latest provider templates")
            .into_iter()
            .filter(|template| template.id == version_one.id)
            .collect::<Vec<_>>();
        assert_eq!(exposed.len(), 1);
        assert_eq!(exposed[0].manifest_version, 2);
    }

    #[test]
    fn ollama_template_view_creates_a_loopback_connection_without_native_inference() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let ollama = core
            .list_provider_template_views()
            .expect("provider template views")
            .into_iter()
            .find(|view| view.template.id.as_str() == "ollama-native-v1")
            .expect("Ollama template view");
        assert_eq!(
            ollama.default_network_mode,
            ProviderNetworkMode::LocalLoopback
        );
        let api_origin = ollama
            .template
            .default_manifest
            .default_api_origin
            .clone()
            .expect("Ollama default origin");

        let connection = core
            .create_provider_connection(ProviderConnectionDraft {
                id: ProviderConnectionId::from("ollama-create-regression"),
                template_id: ollama.template.id,
                template_version: ollama.template.manifest_version,
                display_name: "Local Ollama".to_owned(),
                api_origin,
                api_base_path: Some(EndpointPath::parse("/api").expect("Ollama base path")),
                network_mode: ollama.default_network_mode,
                values: Vec::new(),
                approved_credential_origin: None,
                local_network_approval: None,
                timeout_seconds: 30,
            })
            .expect("create Ollama loopback connection");
        assert_eq!(
            connection.config.network_mode,
            ProviderNetworkMode::LocalLoopback
        );
        assert_eq!(connection.api_origin.as_str(), "http://localhost:11434");
        assert!(connection.credential_ref.is_none());
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one vertical proves generic archive blocking, visibility, and identifier reuse"
    )]
    fn archived_provider_is_hidden_and_rejected_by_generation_and_model_sync() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let template = core
            .list_provider_templates()
            .expect("provider templates")
            .into_iter()
            .find(|template| template.id.as_str() == "ollama-native-v1")
            .expect("credentialless Ollama template");
        let api_origin = template
            .default_manifest
            .default_api_origin
            .clone()
            .expect("Ollama default origin");
        let draft = ProviderConnectionDraft {
            id: ProviderConnectionId::from("archived-core-provider"),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: "Archived Core provider".to_owned(),
            api_origin,
            api_base_path: Some(EndpointPath::parse("/api").expect("Ollama API base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            values: Vec::new(),
            approved_credential_origin: None,
            local_network_approval: None,
            timeout_seconds: 30,
        };
        let connection = core
            .create_provider_connection(draft.clone())
            .expect("create credentialless provider");
        let connection_id = connection.id.clone();
        let now = Utc::now();
        let route = core
            .upsert_model_route(ModelRoute {
                id: ModelRouteId::from("archived-core-provider-route"),
                connection_id: connection_id.clone(),
                api_family: template.api_family,
                model_id: "historical-model".to_owned(),
                display_name: Some("Historical model".to_owned()),
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
            })
            .expect("save active route");
        let preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
            .expect("save active preset");
        core.validate_generation_preset(&route.id, &preset.id)
            .expect("active target");

        let unfinished_sync = core
            .inner
            .storage
            .create_model_sync_job(&connection)
            .expect("create durable model sync");
        let archive_error = core
            .delete_provider_connection(&connection_id)
            .expect_err("unfinished model sync must block Core archive");
        assert_eq!(archive_error.code, CoreErrorCode::InvalidInput);
        assert!(archive_error.recoverable);
        assert_eq!(
            archive_error.message,
            "provider connection cannot be archived while model synchronization is unfinished"
        );
        assert_eq!(
            core.list_provider_connections()
                .expect("active connections after rejected archive"),
            vec![connection]
        );
        core.cancel_provider_model_sync(&unfinished_sync.id)
            .expect("cancel durable model sync");
        core.delete_provider_connection(&connection_id)
            .expect("archive provider");
        assert!(
            core.list_provider_connections()
                .expect("active connections")
                .is_empty()
        );
        assert_eq!(
            core.inner
                .storage
                .get_provider_connection(&connection_id)
                .expect_err("archived provider is hidden")
                .code,
            CoreErrorCode::NotFound
        );
        assert_eq!(
            core.validate_generation_preset(&route.id, &preset.id)
                .expect_err("archived provider cannot generate")
                .code,
            CoreErrorCode::NotFound
        );
        assert_eq!(
            core.start_provider_model_sync(&connection_id, None)
                .expect_err("archived provider cannot synchronize")
                .code,
            CoreErrorCode::NotFound
        );
        assert_eq!(
            core.create_provider_connection(draft)
                .expect_err("archived provider id cannot be reused")
                .code,
            CoreErrorCode::InvalidInput
        );
    }

    #[test]
    fn provider_model_refresh_lists_routes_with_non_secret_provenance() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let body = r#"{"data":[{"id":"zeta-model"},{"id":"alpha-model"}]}"#.to_owned();
        let response_bytes = u64::try_from(body.len()).expect("response size");
        let (api_origin, requests) = spawn_model_list_provider(vec![body]);
        let (template, connection) = create_openai_chat_connection(&core, &api_origin);
        let secret = "model-refresh-listing-key";

        let result = refresh_models_with_review(&core, &connection.id, Some(secret))
            .expect("refresh provider models");

        let request = requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured model-list request");
        let request = request.to_ascii_lowercase();
        assert!(request.starts_with("get /v1/models http/1.1\r\n"));
        assert!(request.contains("authorization: bearer model-refresh-listing-key\r\n"));
        assert_eq!(result.connection_id, connection.id);
        assert_eq!(result.pages_fetched, 1);
        assert_eq!(result.response_bytes, response_bytes);
        assert_eq!(result.provenance.source, "provider_api");
        assert_eq!(result.provenance.api_family, template.api_family);
        assert_eq!(result.provenance.api_origin, api_origin);
        assert_eq!(result.provenance.endpoint_path.as_str(), "/v1/models");
        assert_eq!(
            result
                .model_routes
                .iter()
                .map(|route| route.model_id.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha-model", "zeta-model"]
        );
        assert!(result.model_routes.iter().all(|route| {
            route.status == ModelAvailability::Available
                && route.api_family == template.api_family
                && route.connection_id == connection.id
        }));
        assert_eq!(result.newly_seen_model_route_ids.len(), 2);
        assert_eq!(result.created_generation_preset_ids.len(), 2);
        assert!(result.routes_requiring_preset_configuration.is_empty());
        for route in &result.model_routes {
            let expected_id =
                deterministic_model_route_id(&connection.id, template.api_family, &route.model_id);
            assert_eq!(route.id, expected_id);
            let presets = core
                .list_generation_presets(&route.id)
                .expect("initial preset");
            assert_eq!(presets.len(), 1);
            assert!(presets[0].values.is_empty());
        }
        assert_eq!(
            core.inner
                .storage
                .get_provider_connection(&connection.id)
                .expect("refreshed connection")
                .status,
            ConnectionStatus::Connected
        );
        assert!(!format!("{result:?}").contains(secret));
    }

    #[test]
    fn provider_model_token_limits_become_bounded_route_observations() {
        let observed_at = Utc::now();
        let route = ModelRoute {
            id: ModelRouteId::from("token-route"),
            connection_id: ProviderConnectionId::from("token-connection"),
            api_family: ApiFamily::GeminiGenerateContent,
            model_id: "models/token-model".to_owned(),
            display_name: None,
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: observed_at,
            last_seen_at: Some(observed_at),
        };
        let listed = ListedModel {
            model_id: route.model_id.clone(),
            display_name: None,
            max_input_tokens: Some(1_000_000),
            max_output_tokens: Some(65_536),
            supported_generation_methods: vec!["generateContent".to_owned()],
            capabilities: lorepia_providers::ListedModelCapabilities::default(),
            source: ModelRecordSource::ProviderApi,
            availability: ModelAvailability::Available,
        };
        let observations = provider_api_capability_observations(
            std::slice::from_ref(&route),
            &[listed],
            observed_at,
        )
        .expect("provider API observations");
        assert_eq!(observations.len(), 2);
        assert!(observations.iter().all(|observation| {
            observation.model_route_id == route.id
                && observation.source == ObservationSource::ProviderApi
                && observation.status == SupportStatus::Verified
                && observation.confidence == Confidence::High
                && observation.expires_at == Some(observed_at + PROVIDER_API_CAPABILITY_FRESHNESS)
        }));
        assert_eq!(
            observations
                .iter()
                .find(|observation| observation.key == CapabilityKey::ContextWindow)
                .map(|observation| &observation.value),
            Some(&CapabilityValue::Integer(1_000_000))
        );
        assert_eq!(
            observations
                .iter()
                .find(|observation| observation.key == CapabilityKey::MaxOutputTokens)
                .map(|observation| &observation.value),
            Some(&CapabilityValue::Integer(65_536))
        );
        assert_eq!(
            provider_api_capability_observations(
                &[route],
                &[ListedModel {
                    model_id: "models/token-model".to_owned(),
                    display_name: None,
                    max_input_tokens: Some(0),
                    max_output_tokens: None,
                    supported_generation_methods: Vec::new(),
                    capabilities: lorepia_providers::ListedModelCapabilities::default(),
                    source: ModelRecordSource::ProviderApi,
                    availability: ModelAvailability::Available,
                }],
                observed_at,
            )
            .expect_err("zero token limits must fail closed")
            .code,
            CoreErrorCode::ProviderUnavailable
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one contract-matrix regression covers source, freshness, alias, and bound interactions"
    )]
    fn openrouter_parameter_specs_intersect_exact_metadata_and_fail_closed_by_source() {
        let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
            .expect("OpenRouter template");
        let now = Utc::now();
        let model = listed_openrouter_model(
            "openai/exact-parameter-model",
            vec![
                OpenRouterSupportedParameter::FrequencyPenalty,
                OpenRouterSupportedParameter::Logprobs,
                OpenRouterSupportedParameter::MaxCompletionTokens,
                OpenRouterSupportedParameter::MaxTokens,
                OpenRouterSupportedParameter::ParallelToolCalls,
                OpenRouterSupportedParameter::Stop,
                OpenRouterSupportedParameter::Temperature,
                OpenRouterSupportedParameter::ToolChoice,
                OpenRouterSupportedParameter::Tools,
            ],
            None,
            Some(8_192),
        );
        let mut route = provider_api_openrouter_route(
            ProviderConnectionId::from("openrouter-parameter-connection"),
            &model,
            now,
        );
        let mut base = template.default_manifest.parameters.clone();
        base.push(compiled_openrouter_parameter_spec(
            "alternate_output",
            "max_completion_tokens",
            ParameterType::Integer,
            Some(1.0),
            Some(16_384.0),
            Some(1.0),
            UiParameterLevel::Basic,
        ));
        base.push(compiled_openrouter_parameter_spec(
            "logprobs",
            "logprobs",
            ParameterType::Boolean,
            None,
            None,
            None,
            UiParameterLevel::Advanced,
        ));
        base.push(compiled_openrouter_parameter_spec(
            "parallel_tool_calls",
            "parallel_tool_calls",
            ParameterType::Boolean,
            None,
            None,
            None,
            UiParameterLevel::Advanced,
        ));
        base.push(compiled_openrouter_parameter_spec(
            "tool_choice",
            "tool_choice",
            ParameterType::ToolPolicy,
            None,
            None,
            None,
            UiParameterLevel::Advanced,
        ));
        let specs = effective_route_parameter_specs(&route, &template, &base, &[], now)
            .expect("fresh exact parameter specs");
        let ids = specs
            .iter()
            .map(|spec| spec.id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"temperature"));
        assert!(ids.contains(&"frequency_penalty"));
        assert!(ids.contains(&"stop"));
        assert!(!ids.contains(&"top_p"));
        assert!(!ids.contains(&"logprobs"));
        assert!(!ids.contains(&"parallel_tool_calls"));
        assert!(!ids.contains(&"tool_choice"));
        let output = specs
            .iter()
            .find(|spec| spec.id.as_str() == "max_output_tokens")
            .expect("stable output-token control");
        assert_eq!(output.provider_mapping.field_name, "max_completion_tokens");
        assert_eq!(output.maximum, Some(8_192.0));
        assert_eq!(
            specs
                .iter()
                .filter(|spec| {
                    matches!(
                        spec.provider_mapping.field_name.as_str(),
                        "max_tokens" | "max_completion_tokens"
                    )
                })
                .count(),
            1
        );
        for (parameters, expected_field) in [
            (
                vec![OpenRouterSupportedParameter::MaxTokens],
                Some("max_tokens"),
            ),
            (
                vec![OpenRouterSupportedParameter::MaxCompletionTokens],
                Some("max_completion_tokens"),
            ),
            (
                vec![
                    OpenRouterSupportedParameter::MaxTokens,
                    OpenRouterSupportedParameter::MaxCompletionTokens,
                ],
                Some("max_completion_tokens"),
            ),
            (Vec::new(), None),
        ] {
            let alias_model =
                listed_openrouter_model("openai/alias-model", parameters, None, Some(u64::MAX));
            let alias_route = provider_api_openrouter_route(
                ProviderConnectionId::from("openrouter-alias-connection"),
                &alias_model,
                now,
            );
            let alias_specs = effective_route_parameter_specs(
                &alias_route,
                &template,
                &template.default_manifest.parameters,
                &[],
                now,
            )
            .expect("alias parameter contract");
            let output = alias_specs
                .iter()
                .find(|spec| spec.id.as_str() == "max_output_tokens");
            assert_eq!(
                output.map(|spec| spec.provider_mapping.field_name.as_str()),
                expected_field
            );
            if let Some(output) = output {
                assert_eq!(output.maximum, Some(f64::from(u32::MAX)));
            }
        }
        let no_numeric_cap = listed_openrouter_model(
            "openai/no-numeric-cap",
            vec![OpenRouterSupportedParameter::MaxTokens],
            None,
            None,
        );
        let no_numeric_route = provider_api_openrouter_route(
            ProviderConnectionId::from("openrouter-no-numeric-cap"),
            &no_numeric_cap,
            now,
        );
        let no_numeric_specs = effective_route_parameter_specs(
            &no_numeric_route,
            &template,
            &template.default_manifest.parameters,
            &[],
            now,
        )
        .expect("missing numeric cap retains the local safe ceiling");
        assert_eq!(
            no_numeric_specs
                .iter()
                .find(|spec| spec.id.as_str() == "max_output_tokens")
                .expect("output control without provider numeric cap")
                .maximum,
            Some(f64::from(u32::MAX))
        );

        route.metadata_observed_at = Some(now - chrono::Duration::hours(25));
        assert!(
            effective_route_parameter_specs(&route, &template, &base, &[], now)
                .expect("stale bundled-only contract")
                .is_empty()
        );
        let signed_max_tokens = compiled_openrouter_parameter_spec(
            "signed_output",
            "max_tokens",
            ParameterType::Integer,
            Some(1.0),
            None,
            Some(1.0),
            UiParameterLevel::Basic,
        );
        let mut signed_max_completion = signed_max_tokens.clone();
        signed_max_completion.id = ParameterId::from("signed_completion");
        signed_max_completion.provider_mapping.field_name = "max_completion_tokens".to_owned();
        signed_max_completion.maximum = Some(12_345.0);
        let signed_unsafe = compiled_openrouter_parameter_spec(
            "signed_logprobs",
            "logprobs",
            ParameterType::Boolean,
            None,
            None,
            None,
            UiParameterLevel::Advanced,
        );
        let signed_parallel = compiled_openrouter_parameter_spec(
            "signed_parallel_tool_calls",
            "parallel_tool_calls",
            ParameterType::Boolean,
            None,
            None,
            None,
            UiParameterLevel::Advanced,
        );
        let signed_tool_choice = compiled_openrouter_parameter_spec(
            "signed_tool_choice",
            "tool_choice",
            ParameterType::ToolPolicy,
            None,
            None,
            None,
            UiParameterLevel::Advanced,
        );
        let signed = openrouter_safe_signed_parameter_specs(&[
            signed_max_tokens,
            signed_max_completion,
            signed_unsafe,
            signed_parallel,
            signed_tool_choice,
        ]);
        assert_eq!(signed.len(), 1);
        assert_eq!(signed[0].id.as_str(), "max_output_tokens");
        assert_eq!(
            signed[0].provider_mapping.field_name,
            "max_completion_tokens"
        );
        assert_eq!(signed[0].maximum, Some(12_345.0));
        assert_eq!(
            effective_route_parameter_specs(&route, &template, &base, &signed, now)
                .expect("fresh signed fallback"),
            signed
        );
        let canonical_raw = route.raw_metadata.clone();
        route.raw_metadata = Some(
            BoundedJson::from_value(&serde_json::json!({"malformed": true}))
                .expect("bounded malformed metadata fixture"),
        );
        assert_eq!(
            effective_route_parameter_specs(&route, &template, &base, &signed, now)
                .expect_err("stale malformed ProviderApi metadata cannot use signed fallback")
                .code,
            CoreErrorCode::StorageCorrupted
        );
        route.raw_metadata = canonical_raw;

        route.status = ModelAvailability::MissingTemporarily;
        assert!(
            effective_route_parameter_specs(&route, &template, &base, &signed, now)
                .expect("unavailable routes remain nonactionable")
                .is_empty()
        );
        route.status = ModelAvailability::Available;
        route.metadata_observed_at = Some(now);
        route.raw_metadata = None;
        assert_eq!(
            effective_route_parameter_specs(&route, &template, &base, &signed, now)
                .expect_err("fresh ProviderApi provenance without metadata is corrupt")
                .code,
            CoreErrorCode::StorageCorrupted
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the test keeps raw model metadata, its observation, UI, and request wire in one atomic matrix"
    )]
    fn openrouter_reasoning_requires_matching_fresh_raw_metadata_and_uses_exact_wire_style() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let (template, mut route) =
            create_built_in_public_route(&core, "openrouter-v1", "/api/v1", "openai/reasoning");
        let now = Utc::now();
        let reasoning = ListedModelReasoningCapability {
            supported_efforts: OpenRouterReasoningEffortSupport::Exact(vec![
                lorepia_providers::OpenRouterReasoningEffort::High,
            ]),
            default_effort: Some(lorepia_providers::OpenRouterReasoningEffort::High),
            default_enabled: Some(true),
            supports_max_tokens: Some(true),
            mandatory: Some(false),
        };
        let model = listed_openrouter_model(
            &route.model_id,
            vec![
                OpenRouterSupportedParameter::MaxCompletionTokens,
                OpenRouterSupportedParameter::Reasoning,
                OpenRouterSupportedParameter::ReasoningEffort,
                OpenRouterSupportedParameter::Temperature,
            ],
            Some(reasoning),
            Some(4_096),
        );
        route.raw_metadata = Some(listed_model_metadata(&model).expect("normalized metadata"));
        route.metadata_source = ModelMetadataSource::ProviderApi;
        route.metadata_observed_at = Some(now);
        route.last_seen_at = Some(now);
        core.inner
            .storage
            .save_model_route(&route)
            .expect("save trusted route fixture");
        let observations = provider_api_capability_observations(
            std::slice::from_ref(&route),
            std::slice::from_ref(&model),
            now,
        )
        .expect("provider observations");
        core.record_provider_api_capability_observations(observations)
            .expect("persist provider observations");

        let mut preset = initial_generation_preset(&route.id, &template, now);
        preset.reasoning.mode = GenerationReasoningMode::Enabled;
        let rendered = core
            .render_reasoning_control_for_preset(&preset)
            .expect("render default-effort adoption");
        assert_eq!(
            rendered.settings.effort,
            Some(lorepia_providers::parameter_mapping::ReasoningEffort::High)
        );
        assert_eq!(
            core.validate_generation_preset_candidate(&preset)
                .expect_err("render-only default must not become an implicit request")
                .code,
            CoreErrorCode::InvalidInput
        );

        preset.reasoning.effort = Some(GenerationReasoningEffort::High);
        preset.values = vec![lorepia_domain::ParameterValue {
            parameter_id: ParameterId::from("max_output_tokens"),
            state: lorepia_domain::ParameterValueState::Explicit(
                lorepia_domain::ParameterLiteral::Integer(2_048),
            ),
        }];
        let preview = core
            .preview_provider_request_candidate(&preset)
            .expect("preview unified OpenRouter request");
        let lorepia_providers::RequestBodyShape::Object { fields, .. } =
            preview.body().expect("preview body")
        else {
            panic!("OpenRouter preview body must be an object");
        };
        assert!(fields.iter().any(|field| {
            field.name() == "max_completion_tokens"
                && field.shape() == &lorepia_providers::RequestBodyShape::Number
        }));
        assert!(
            fields
                .iter()
                .all(|field| field.name() != "max_tokens" && field.name() != "reasoning_effort")
        );
        let reasoning = fields
            .iter()
            .find(|field| field.name() == "reasoning")
            .expect("nested reasoning field");
        let lorepia_providers::RequestBodyShape::Object {
            fields: reasoning_fields,
            ..
        } = reasoning.shape()
        else {
            panic!("reasoning preview shape must be an object");
        };
        assert!(
            reasoning_fields
                .iter()
                .any(|field| field.name() == "effort")
        );

        route.metadata_observed_at = Some(now - chrono::Duration::hours(25));
        core.inner
            .storage
            .save_model_route(&route)
            .expect("make raw metadata stale");
        assert_eq!(
            core.render_reasoning_control_for_preset(&preset)
                .expect("stale control renders hidden")
                .state,
            lorepia_providers::parameter_mapping::UiControlState::Hidden
        );
        assert_eq!(
            core.validate_generation_preset_candidate(&preset)
                .expect_err("stale raw metadata cannot drive reasoning")
                .code,
            CoreErrorCode::InvalidInput
        );

        route.metadata_observed_at = Some(now);
        let legacy_model = listed_openrouter_model(
            &route.model_id,
            vec![OpenRouterSupportedParameter::ReasoningEffort],
            Some(ListedModelReasoningCapability {
                supported_efforts: OpenRouterReasoningEffortSupport::AllGateway,
                default_effort: None,
                default_enabled: None,
                supports_max_tokens: None,
                mandatory: Some(false),
            }),
            None,
        );
        route.raw_metadata =
            Some(listed_model_metadata(&legacy_model).expect("legacy raw metadata"));
        core.inner
            .storage
            .save_model_route(&route)
            .expect("save mismatched raw style");
        assert_eq!(
            core.render_reasoning_control_for_preset(&preset)
                .expect("mismatched observation is hidden")
                .state,
            lorepia_providers::parameter_mapping::UiControlState::Hidden
        );

        route.raw_metadata = Some(listed_model_metadata(&model).expect("canonical raw metadata"));
        route.metadata_observed_at = Some(now - chrono::Duration::seconds(1));
        core.inner
            .storage
            .save_model_route(&route)
            .expect("save timestamp-mismatched raw metadata");
        assert_eq!(
            core.render_reasoning_control_for_preset(&preset)
                .expect("timestamp-mismatched observation is hidden")
                .state,
            lorepia_providers::parameter_mapping::UiControlState::Hidden
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the capability conflict scenario is clearer as one chronological state transition"
    )]
    fn effective_capabilities_gate_reasoning_and_cache_with_exact_fresh_metadata() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let api_origin = CanonicalOrigin::parse("http://127.0.0.1:39491").expect("loopback origin");
        let (target, route) = create_openai_chat_generation_target(&core, &api_origin);
        let preset = core
            .inner
            .storage
            .get_generation_preset(&target.generation_preset_id)
            .expect("seeded generation preset");
        assert_eq!(
            core.render_reasoning_control_for_preset(&preset)
                .expect("hidden reasoning controls")
                .state,
            lorepia_providers::parameter_mapping::UiControlState::Hidden
        );
        assert_eq!(
            core.render_prompt_cache_control_for_preset(&preset)
                .expect("hidden cache controls")
                .state,
            lorepia_providers::parameter_mapping::UiControlState::Hidden
        );

        let error = resolve_generation_target(&core, &target)
            .err()
            .expect("family alone must not enable reasoning or prompt caching");
        assert!(error.message.contains("no observed reasoning control"));

        let observed_at = Utc::now();
        let reasoning = CapabilityObservation {
            id: ObservationId::from("reasoning-provider-api"),
            model_route_id: route.id.clone(),
            key: CapabilityKey::Reasoning,
            value: CapabilityValue::Structured(
                serde_json::to_value(ReasoningWireDialect::OpenAiChatCompletions {
                    efforts: vec![
                        lorepia_providers::parameter_mapping::ReasoningEffort::Low,
                        lorepia_providers::parameter_mapping::ReasoningEffort::High,
                    ],
                    supports_disabled: true,
                })
                .expect("reasoning dialect JSON"),
            ),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at,
            expires_at: Some(observed_at + chrono::Duration::hours(1)),
            evidence_ref: None,
        };
        core.record_provider_api_capability_observations(vec![reasoning.clone()])
            .expect("store reasoning observation");
        let reasoning_control = core
            .render_reasoning_control_for_preset(&preset)
            .expect("render reasoning controls");
        assert_eq!(
            reasoning_control.state,
            lorepia_providers::parameter_mapping::UiControlState::Ready
        );
        assert_eq!(
            reasoning_control.allowed_efforts,
            vec![
                lorepia_providers::parameter_mapping::ReasoningEffort::Low,
                lorepia_providers::parameter_mapping::ReasoningEffort::High,
            ]
        );
        assert!(reasoning_control.issues.is_empty());
        let error = resolve_generation_target(&core, &target)
            .err()
            .expect("cache control must remain gated independently");
        assert!(
            error.message.contains("no provider prompt-cache control"),
            "{}",
            error.message
        );

        let prompt_cache = CapabilityObservation {
            id: ObservationId::from("cache-provider-api"),
            model_route_id: route.id.clone(),
            key: CapabilityKey::PromptCaching,
            value: CapabilityValue::Structured(
                serde_json::to_value(PromptCacheWireDialect::OpenAiAutomatic {
                    supports_24_hour_retention: false,
                })
                .expect("prompt-cache dialect JSON"),
            ),
            status: SupportStatus::Verified,
            source: ObservationSource::ProviderApi,
            confidence: Confidence::High,
            observed_at,
            expires_at: Some(observed_at + chrono::Duration::hours(1)),
            evidence_ref: None,
        };
        core.record_provider_api_capability_observations(vec![prompt_cache])
            .expect("store cache observation");
        let cache_control = core
            .render_prompt_cache_control_for_preset(&preset)
            .expect("render cache controls");
        assert_eq!(
            cache_control.state,
            lorepia_providers::parameter_mapping::UiControlState::Ready
        );
        assert!(
            cache_control
                .allowed_modes
                .contains(&lorepia_providers::parameter_mapping::PromptCacheMode::Automatic)
        );
        assert!(cache_control.issues.is_empty());
        resolve_generation_target(&core, &target)
            .expect("exact reasoning and cache metadata unlock request mapping");

        let mut invalid_preset = preset.clone();
        invalid_preset.reasoning.effort = Some(GenerationReasoningEffort::Minimal);
        let invalid_control = core
            .render_reasoning_control_for_preset(&invalid_preset)
            .expect("render invalid reasoning controls");
        assert_eq!(
            invalid_control.state,
            lorepia_providers::parameter_mapping::UiControlState::Invalid
        );
        assert!(!invalid_control.issues.is_empty());

        let conflicting = CapabilityObservation {
            id: ObservationId::from("reasoning-probe-conflict"),
            model_route_id: route.id.clone(),
            key: CapabilityKey::Reasoning,
            value: CapabilityValue::Boolean(false),
            status: SupportStatus::Unsupported,
            source: ObservationSource::CapabilityProbe,
            confidence: Confidence::High,
            observed_at: observed_at + chrono::Duration::seconds(1),
            expires_at: Some(observed_at + chrono::Duration::hours(1)),
            evidence_ref: None,
        };
        core.record_probe_capability_observations(vec![conflicting])
            .expect("store conflicting probe");
        let effective = core
            .effective_capability(&route.id, CapabilityKey::Reasoning)
            .expect("effective capability")
            .expect("reasoning capability");
        assert_eq!(
            effective.selected.source,
            ObservationSource::CapabilityProbe
        );
        assert!(!effective.selected_is_stale);
        assert!(effective.has_conflict);
        let error = resolve_generation_target(&core, &target)
            .err()
            .expect("fresh conflicts must fail closed");
        assert!(error.message.contains("no observed reasoning control"));

        core.delete_capability_observation(&effective.selected.id)
            .expect("remove conflicting observation");
        resolve_generation_target(&core, &target)
            .expect("removing conflict restores exact mapping");

        let mut wrong_family = reasoning;
        wrong_family.id = ObservationId::from("wrong-family-dialect");
        wrong_family.observed_at += chrono::Duration::seconds(2);
        wrong_family.value = CapabilityValue::Structured(
            serde_json::to_value(ReasoningWireDialect::GeminiThinkingBudget {
                minimum_budget_tokens: 1,
                maximum_budget_tokens: 1024,
                supports_zero_to_disable: true,
                supports_automatic: true,
                summaries: Vec::new(),
            })
            .expect("wrong-family dialect JSON"),
        );
        assert!(
            core.upsert_capability_observation(wrong_family)
                .expect_err("family-mismatched dialect must be rejected")
                .message
                .contains("does not match the API family")
        );
    }

    #[test]
    fn signed_catalog_observations_cannot_outlive_the_active_catalog_pointer() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let origin = CanonicalOrigin::parse("http://127.0.0.1:11434").expect("loopback origin");
        let (_target, route) = create_openai_chat_generation_target(&core, &origin);
        let observed_at = Utc::now();
        let observation = CapabilityObservation {
            id: ObservationId::from("detached-signed-catalog-observation"),
            model_route_id: route.id.clone(),
            key: CapabilityKey::Streaming,
            value: CapabilityValue::Boolean(true),
            status: SupportStatus::Documented,
            source: ObservationSource::SignedLorepiaCatalog,
            confidence: Confidence::High,
            observed_at,
            expires_at: Some(observed_at + chrono::Duration::days(1)),
            evidence_ref: None,
        };
        assert!(
            core.upsert_capability_observation(observation.clone())
                .expect_err("detached signed catalog facts must not be accepted")
                .message
                .contains("active verified catalog")
        );

        // Legacy rows from a pre-projection build are ignored as well. Only
        // the currently active, signature-verified snapshot may supply this
        // provenance, so rollback cannot leave a detached fact selected.
        core.inner
            .storage
            .upsert_capability_observation(&observation)
            .expect("inject legacy detached row");
        assert!(
            core.list_capability_observations(&route.id)
                .expect("effective observations")
                .iter()
                .all(|value| value.id != observation.id)
        );
        assert!(
            core.effective_capability(&route.id, CapabilityKey::Streaming)
                .expect("effective capability")
                .is_none()
        );
    }

    #[test]
    fn provider_model_refresh_preserves_missing_routes_and_their_presets() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let first = r#"{"data":[{"id":"keep-model"},{"id":"gone-model"}]}"#.to_owned();
        let second = r#"{"data":[{"id":"keep-model"}]}"#.to_owned();
        let (api_origin, requests) = spawn_model_list_provider(vec![first, second]);
        let (_template, connection) = create_openai_chat_connection(&core, &api_origin);

        let first_result = refresh_models_with_review(&core, &connection.id, Some("refresh-key"))
            .expect("initial model refresh");
        requests
            .recv_timeout(Duration::from_secs(2))
            .expect("initial model-list request");
        let keep_before = first_result
            .model_routes
            .iter()
            .find(|route| route.model_id == "keep-model")
            .expect("kept route")
            .clone();
        let gone_before = first_result
            .model_routes
            .iter()
            .find(|route| route.model_id == "gone-model")
            .expect("soon-missing route")
            .clone();
        let mut customized_preset = core
            .list_generation_presets(&gone_before.id)
            .expect("initial missing-route preset")
            .into_iter()
            .next()
            .expect("preset for soon-missing route");
        customized_preset.display_name = "Keep this preset".to_owned();
        customized_preset.updated_at = Utc::now();
        core.upsert_generation_preset(customized_preset.clone())
            .expect("customize missing-route preset");

        let second_result = refresh_models_with_review(&core, &connection.id, Some("refresh-key"))
            .expect("second model refresh");
        requests
            .recv_timeout(Duration::from_secs(2))
            .expect("second model-list request");

        assert!(second_result.newly_seen_model_route_ids.is_empty());
        assert!(second_result.created_generation_preset_ids.is_empty());
        assert_eq!(
            second_result.missing_model_route_ids,
            vec![gone_before.id.clone()]
        );
        let keep_after = second_result
            .model_routes
            .iter()
            .find(|route| route.model_id == "keep-model")
            .expect("kept route after refresh");
        assert_eq!(keep_after.id, keep_before.id);
        assert_eq!(keep_after.first_seen_at, keep_before.first_seen_at);
        assert_eq!(keep_after.status, ModelAvailability::Available);
        let gone_after = second_result
            .model_routes
            .iter()
            .find(|route| route.model_id == "gone-model")
            .expect("missing route remains");
        assert_eq!(gone_after.id, gone_before.id);
        assert_eq!(gone_after.first_seen_at, gone_before.first_seen_at);
        assert_eq!(gone_after.status, ModelAvailability::MissingTemporarily);
        for error in [
            core.validate_generation_preset_candidate(&customized_preset)
                .expect_err("missing route preset validation"),
            core.preview_provider_request_candidate(&customized_preset)
                .expect_err("missing route preview"),
            core.upsert_generation_preset(customized_preset.clone())
                .expect_err("missing route preset save"),
        ] {
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(error.message.contains("not currently available"));
        }
        assert_eq!(
            core.list_generation_presets(&gone_before.id)
                .expect("preserved missing-route presets"),
            vec![customized_preset]
        );
    }

    #[test]
    fn provider_model_refresh_never_persists_the_borrowed_credential() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let (api_origin, requests) = spawn_model_list_provider(vec![
            r#"{"data":[{"id":"credential-safe-model"}]}"#.to_owned(),
        ]);
        let (_template, connection) = create_openai_chat_connection(&core, &api_origin);
        let secret = format!("refresh-secret-{}", Uuid::new_v4());

        let result = refresh_models_with_review(&core, &connection.id, Some(&secret))
            .expect("refresh provider models");
        let request = requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured credential-bearing request");
        assert!(request.contains(&secret));
        assert!(!format!("{result:?}").contains(&secret));
        assert!(
            core.list_model_routes(&connection.id)
                .expect("persisted routes")
                .iter()
                .all(|route| !format!("{route:?}").contains(&secret))
        );

        drop(core);
        assert_directory_does_not_contain(root.path(), secret.as_bytes());
    }

    #[test]
    fn generation_preset_validation_and_preview_share_the_route_plan() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let (api_origin, requests) =
            spawn_model_list_provider(vec![r#"{"data":[{"id":"preview-safe-model"}]}"#.to_owned()]);
        let (_template, connection) = create_openai_chat_connection(&core, &api_origin);

        let result = refresh_models_with_review(&core, &connection.id, Some("request-only-key"))
            .expect("refresh provider models");
        requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured model-list request");
        let route = result.model_routes.first().expect("refreshed model route");
        let preset = core
            .list_generation_presets(&route.id)
            .expect("generation presets")
            .into_iter()
            .next()
            .expect("initial generation preset");

        core.validate_generation_preset(&route.id, &preset.id)
            .expect("family-aware generation validation");
        let preview = core
            .preview_provider_request(&route.id, &preset.id)
            .expect("safe provider request preview");
        assert_eq!(preview.method(), lorepia_domain::HttpMethod::Post);
        assert_eq!(preview.origin(), &api_origin);
        assert_eq!(preview.path().as_str(), "/v1/chat/completions");
        assert!(preview.body().is_some());
        assert!(!format!("{preview:?}").contains("request-only-key"));

        let mut invalid = preset.clone();
        invalid.id = GenerationPresetId::from(format!("invalid-{}", Uuid::new_v4()));
        invalid.values = vec![lorepia_domain::ParameterValue {
            parameter_id: lorepia_domain::ParameterId::from("unknown-parameter"),
            state: lorepia_domain::ParameterValueState::Explicit(
                lorepia_domain::ParameterLiteral::Integer(1),
            ),
        }];
        let error = core
            .upsert_generation_preset(invalid.clone())
            .expect_err("invalid candidate must fail before persistence");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(
            core.list_generation_presets(&route.id)
                .expect("presets after rejected candidate")
                .iter()
                .all(|stored| stored.id != invalid.id)
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the table-like cross-family policy assertions share one catalog fixture"
    )]
    fn unsupported_opaque_continuity_is_normalized_or_rejected_before_generation() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let now = Utc::now();

        let (gemini_template, gemini_route) = create_built_in_public_route(
            &core,
            "gemini-generate-content-v1",
            "/v1beta",
            "gemini-2.5-flash",
        );
        let gemini_default = initial_generation_preset(&gemini_route.id, &gemini_template, now);
        assert!(!gemini_default.reasoning.preserve_opaque_state);
        let saved = core
            .upsert_generation_preset(gemini_default.clone())
            .expect("Gemini default with opaque continuity disabled");
        let resolved = resolve_generation_target(
            &core,
            &GenerationTarget {
                model_route_id: gemini_route.id.clone(),
                generation_preset_id: saved.id.clone(),
            },
        )
        .expect("Gemini target resolves without deferred continuity failure");
        assert!(!resolved.preserve_opaque_reasoning_state);

        let mut direct = gemini_default.clone();
        direct.id = GenerationPresetId::from(format!("direct-{}", Uuid::new_v4()));
        direct.reasoning.preserve_opaque_state = true;
        let control = core
            .render_reasoning_control_for_preset(&direct)
            .expect("render normalized Gemini control");
        assert!(!control.settings.preserve_opaque_state);
        for error in [
            core.validate_generation_preset_candidate(&direct)
                .expect_err("direct Gemini continuity candidate"),
            core.preview_provider_request_candidate(&direct)
                .expect_err("Gemini preview must share the pre-network gate"),
            core.upsert_generation_preset(direct.clone())
                .expect_err("Gemini continuity must fail before persistence"),
        ] {
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert_eq!(error.message, GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR);
        }
        assert!(
            core.list_generation_presets(&gemini_route.id)
                .expect("Gemini presets")
                .iter()
                .all(|preset| preset.id != direct.id)
        );

        let mut legacy = gemini_default;
        legacy.reasoning.preserve_opaque_state = true;
        core.inner
            .storage
            .save_generation_preset(&legacy)
            .expect("seed legacy Gemini preset");
        core.validate_generation_preset(&gemini_route.id, &legacy.id)
            .expect("legacy credential-bound preset is normalized off");
        let legacy_resolved = resolve_generation_target(
            &core,
            &GenerationTarget {
                model_route_id: gemini_route.id.clone(),
                generation_preset_id: legacy.id,
            },
        )
        .expect("legacy credential-bound target resolves safely");
        assert!(!legacy_resolved.preserve_opaque_reasoning_state);

        let (responses_template, responses_route) =
            create_built_in_public_route(&core, "openai-responses-v1", "/v1", "gpt-5-fixture");
        let mut responses_default =
            initial_generation_preset(&responses_route.id, &responses_template, now);
        assert!(!responses_default.reasoning.preserve_opaque_state);
        let responses_saved = core
            .upsert_generation_preset(responses_default.clone())
            .expect("OpenAI Responses default disables lossy opaque continuity");
        let responses_resolved = resolve_generation_target(
            &core,
            &GenerationTarget {
                model_route_id: responses_route.id,
                generation_preset_id: responses_saved.id,
            },
        )
        .expect("OpenAI Responses target without opaque continuity");
        assert!(!responses_resolved.preserve_opaque_reasoning_state);
        responses_default.reasoning.preserve_opaque_state = true;
        let responses_error = core
            .validate_generation_preset_candidate(&responses_default)
            .expect_err("OpenAI Responses cannot replay incomplete response topology");
        assert_eq!(
            responses_error.message,
            OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR
        );

        let (openrouter_template, openrouter_route) =
            create_built_in_public_route(&core, "openrouter-v1", "/api/v1", "openai/gpt-fixture");
        let mut openrouter_default =
            initial_generation_preset(&openrouter_route.id, &openrouter_template, now);
        assert!(!openrouter_default.reasoning.preserve_opaque_state);
        assert!(
            !core
                .render_reasoning_control_for_preset(&openrouter_default)
                .expect("render credential-bound OpenRouter control")
                .settings
                .preserve_opaque_state
        );
        let openrouter_preset = core
            .upsert_generation_preset(openrouter_default.clone())
            .expect("credential-bound OpenRouter disables opaque continuity");
        let openrouter = resolve_generation_target(
            &core,
            &GenerationTarget {
                model_route_id: openrouter_route.id.clone(),
                generation_preset_id: openrouter_preset.id,
            },
        )
        .expect("credential-bound OpenRouter target");
        assert!(!openrouter.preserve_opaque_reasoning_state);
        openrouter_default.reasoning.preserve_opaque_state = true;
        for error in [
            core.validate_generation_preset_candidate(&openrouter_default)
                .expect_err("OpenRouter continuity candidate must fail closed"),
            core.preview_provider_request_candidate(&openrouter_default)
                .expect_err("OpenRouter continuity preview must fail closed"),
            core.upsert_generation_preset(openrouter_default.clone())
                .expect_err("OpenRouter continuity save must fail closed"),
        ] {
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert_eq!(error.message, OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR);
        }

        let loopback = CanonicalOrigin::parse("http://127.0.0.1:65534").expect("loopback origin");
        let (generic_template, generic_connection) =
            create_openai_chat_connection(&core, &loopback);
        let generic_route = ModelRoute {
            id: ModelRouteId::from(format!("route-{}", Uuid::new_v4())),
            connection_id: generic_connection.id,
            api_family: generic_template.api_family,
            model_id: "generic-chat".to_owned(),
            display_name: None,
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
        core.upsert_model_route(generic_route.clone())
            .expect("save generic Chat Completions route");
        let mut generic_default =
            initial_generation_preset(&generic_route.id, &generic_template, now);
        assert!(!generic_default.reasoning.preserve_opaque_state);
        let generic_saved = core
            .upsert_generation_preset(generic_default.clone())
            .expect("generic Chat Completions default");
        let generic_resolved = resolve_generation_target(
            &core,
            &GenerationTarget {
                model_route_id: generic_route.id.clone(),
                generation_preset_id: generic_saved.id,
            },
        )
        .expect("generic Chat Completions target");
        assert!(!generic_resolved.preserve_opaque_reasoning_state);
        generic_default.reasoning.preserve_opaque_state = true;
        let generic_error = core
            .validate_generation_preset_candidate(&generic_default)
            .expect_err("generic Chat Completions cannot advertise OpenRouter continuity");
        assert_eq!(
            generic_error.message,
            OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "credential rotation, a legacy row, and reopen assertions must share one fixture"
    )]
    fn opaque_preset_is_provenance_only_but_credential_targets_never_load_or_persist_it() {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(&character.id, "Opaque continuity", ConversationMode::Chat)
            .expect("conversation");
        let state = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state");
        let (template, route) = create_built_in_public_route(
            &core,
            "openrouter-v1",
            "/api/v1",
            "openrouter/test-model",
        );
        let model = route.model_id.clone();
        let route_id = route.id.clone();
        let source_preset = core
            .upsert_generation_preset(initial_generation_preset(&route_id, &template, Utc::now()))
            .expect("source preset");
        let source_preset_id = source_preset.id.clone();
        assert!(!source_preset.reasoning.preserve_opaque_state);
        let source_target = GenerationTarget {
            model_route_id: route_id.clone(),
            generation_preset_id: source_preset_id.clone(),
        };
        let retained_state = OpaqueReasoningState::OpenRouterReasoning {
            topology: OpenRouterReasoningTopology::new(
                None,
                Some(vec![
                    OpenRouterReasoningDetail::from_value(&serde_json::json!({
                        "type": "reasoning.encrypted",
                        "data": "opaque-state",
                        "id": "detail-1",
                        "format": "openrouter-v1",
                        "index": 0
                    }))
                    .expect("OpenRouter opaque detail"),
                ]),
            )
            .expect("OpenRouter opaque topology"),
        };
        let (source_capture_sender, source_capture_receiver) = std_mpsc::channel();
        let source_provider = Arc::new(OpaqueContinuityProvider {
            response: "source response".to_owned(),
            emitted_state: Some(retained_state.clone()),
            captured_request: Mutex::new(Some(source_capture_sender)),
        });

        // Even an internal caller asking to preserve state is overridden when
        // the actual borrowed credential is non-empty.
        let source_generation_id = core
            .send_message_to_branch_with_provider_options(
                &conversation.id,
                &state.active_branch_id,
                None,
                ConversationMode::Chat,
                "first",
                new_test_generation_operation("opaque-first-v1"),
                model.clone(),
                Some(&source_target),
                Some(ApiFamily::OpenAiChatCompletions),
                true,
                None,
                Some(128),
                Some("credential-a".to_owned()),
                None,
                false,
                source_provider,
            )
            .expect("source generation");
        let (source_preserve, source_contexts, _) = source_capture_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("captured source request");
        assert!(!source_preserve);
        assert!(source_contexts.is_empty());
        let source_generation =
            wait_for_generation_status(&core, &source_generation_id, GenerationStatus::Complete);
        assert!(source_generation.opaque_reasoning_state.is_empty());
        let source_assistant = core
            .list_branch_messages(&state.active_branch_id)
            .expect("source branch messages")
            .into_iter()
            .find(|message| {
                message.role == MessageRole::Assistant
                    && message.generation_id.as_ref() == Some(&source_generation_id)
            })
            .expect("source assistant");

        // Simulate a completed row written by an older release while key A was
        // active. Credentials are intentionally absent from generation rows,
        // so Core must never infer that this state is safe for key B.
        let legacy_generation_id = GenerationId::new();
        let legacy_user = Message::user_after(
            conversation.id.clone(),
            Some(source_assistant.id.clone()),
            "legacy credential-A turn",
        );
        let legacy_assistant = Message::pending_assistant(
            conversation.id.clone(),
            legacy_user.id.clone(),
            legacy_generation_id.clone(),
        );
        let legacy_generation = GenerationRecord {
            id: legacy_generation_id.clone(),
            conversation_id: conversation.id.clone(),
            branch_id: state.active_branch_id.clone(),
            user_message_id: legacy_user.id.clone(),
            assistant_message_id: Some(legacy_assistant.id.clone()),
            mode: ConversationMode::Chat,
            model: model.clone(),
            model_route_id: Some(route_id.clone()),
            generation_preset_id: Some(source_preset_id.clone()),
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
            started_at: Utc::now(),
            finished_at: None,
        };
        core.inner
            .storage
            .append_generation(
                &state.active_branch_id,
                Some(&source_assistant.id),
                &legacy_user,
                &legacy_assistant,
                &legacy_generation,
            )
            .expect("seed running legacy generation");
        let mut legacy_terminal = legacy_assistant;
        legacy_terminal.content = "legacy response".to_owned();
        legacy_terminal.status = MessageStatus::Complete;
        core.inner
            .storage
            .finalize_generation_with_protocol_state(
                &legacy_terminal,
                Some(&GenerationUsage::default()),
                std::slice::from_ref(&retained_state),
                None,
                true,
            )
            .expect("seed legacy credential-A opaque state");
        assert_eq!(
            core.inner
                .storage
                .get_generation(&legacy_generation_id)
                .expect("legacy generation")
                .opaque_reasoning_state,
            vec![retained_state.clone()]
        );

        // Preset ID remains source provenance rather than continuity identity.
        // This dormant loader may match the exact family/model/route/source
        // under a different current preset, while the credential gate below
        // ensures production requests never receive that context.
        let different_current_target = GenerationTarget {
            model_route_id: route_id.clone(),
            generation_preset_id: GenerationPresetId::from("different-current-preset"),
        };
        let dormant_context = load_opaque_reasoning_context(
            &core.inner.storage,
            std::slice::from_ref(&legacy_terminal),
            ApiFamily::OpenAiChatCompletions,
            &model,
            &different_current_target,
        )
        .expect("load dormant context under a different current preset");
        assert_eq!(dormant_context.len(), 1);
        assert_eq!(dormant_context[0].source_message_id, legacy_terminal.id);
        assert_eq!(dormant_context[0].model_route_id, route_id);
        assert_eq!(dormant_context[0].generation_preset_id, source_preset_id);
        assert_ne!(
            dormant_context[0].generation_preset_id,
            different_current_target.generation_preset_id
        );

        let resolved = resolve_generation_target(&core, &source_target)
            .expect("credential-bound target resolves with continuity disabled");
        assert!(!resolved.preserve_opaque_reasoning_state);
        let next_state = OpaqueReasoningState::OpenRouterReasoning {
            topology: OpenRouterReasoningTopology::new(
                Some("new key-B reasoning".to_owned()),
                Some(Vec::new()),
            )
            .expect("new OpenRouter topology"),
        };
        let (capture_sender, capture_receiver) = std_mpsc::channel();
        let next_provider = Arc::new(OpaqueContinuityProvider {
            response: "next response".to_owned(),
            emitted_state: Some(next_state),
            captured_request: Mutex::new(Some(capture_sender)),
        });
        let next_generation_id = core
            .send_message_to_branch_with_provider_options(
                &conversation.id,
                &state.active_branch_id,
                Some(&legacy_terminal.id),
                ConversationMode::Chat,
                "second",
                new_test_generation_operation("opaque-second-v1"),
                model.clone(),
                Some(&source_target),
                Some(ApiFamily::OpenAiChatCompletions),
                true,
                None,
                Some(128),
                Some("credential-b".to_owned()),
                None,
                false,
                next_provider,
            )
            .expect("next generation with a different credential");
        let (preserve, contexts, current_provenance) = capture_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("captured next request");
        assert!(!preserve);
        assert!(contexts.is_empty());
        let next_generation =
            wait_for_generation_status(&core, &next_generation_id, GenerationStatus::Complete);
        assert!(next_generation.opaque_reasoning_state.is_empty());

        assert_eq!(
            current_provenance,
            Some(GenerationProviderProvenance {
                api_family: ApiFamily::OpenAiChatCompletions,
                model_route_id: route_id.clone(),
                generation_preset_id: source_preset_id,
            })
        );
        wait_for_generation_registry_to_drain(&core);
        drop(core);

        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
        assert_eq!(
            reopened
                .inner
                .storage
                .get_generation(&legacy_generation_id)
                .expect("reopened legacy generation")
                .opaque_reasoning_state,
            vec![retained_state]
        );
        assert!(
            reopened
                .inner
                .storage
                .get_generation(&next_generation_id)
                .expect("reopened key-B generation")
                .opaque_reasoning_state
                .is_empty()
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "route construction and durable no-auth assertions intentionally share one fixture"
    )]
    fn nonempty_raw_credential_disables_opaque_state_on_a_no_auth_connection() {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "No-auth raw credential",
                ConversationMode::Chat,
            )
            .expect("conversation");
        let branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let template = core
            .list_provider_templates()
            .expect("provider templates")
            .into_iter()
            .find(|template| template.id.as_str() == "ollama-native-v1")
            .expect("Ollama template");
        let api_origin = CanonicalOrigin::parse("http://127.0.0.1:11434").expect("loopback origin");
        let connection = core
            .create_provider_connection(ProviderConnectionDraft {
                id: ProviderConnectionId::from(format!("no-auth-{}", Uuid::new_v4())),
                template_id: template.id.clone(),
                template_version: template.manifest_version,
                display_name: "No-auth Ollama".to_owned(),
                api_origin,
                api_base_path: Some(EndpointPath::parse("/api").expect("API base path")),
                network_mode: ProviderNetworkMode::LocalLoopback,
                values: Vec::new(),
                approved_credential_origin: None,
                local_network_approval: None,
                timeout_seconds: 5,
            })
            .expect("create no-auth connection");
        assert!(connection.credential_ref.is_none());
        let now = Utc::now();
        let route = ModelRoute {
            id: ModelRouteId::from(format!("no-auth-route-{}", Uuid::new_v4())),
            connection_id: connection.id,
            api_family: ApiFamily::OllamaNative,
            model_id: "llama-no-auth".to_owned(),
            display_name: None,
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
        core.upsert_model_route(route.clone())
            .expect("save no-auth route");
        let preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
            .expect("save no-auth preset");
        let target = GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: preset.id,
        };

        let (capture_sender, capture_receiver) = std_mpsc::channel();
        let provider = Arc::new(OpaqueContinuityProvider {
            response: "safe response".to_owned(),
            emitted_state: Some(OpaqueReasoningState::GeminiThoughtSignature {
                part_index: 0,
                signature: lorepia_domain::OpaqueReasoningData::parse("safe-signature")
                    .expect("signature"),
            }),
            captured_request: Mutex::new(Some(capture_sender)),
        });
        let generation_id = core
            .send_message_to_branch_with_provider_options(
                &conversation.id,
                &branch,
                None,
                ConversationMode::Chat,
                "hello",
                new_test_generation_operation("no-auth-raw-credential-v1"),
                "llama-no-auth".to_owned(),
                Some(&target),
                Some(ApiFamily::OllamaNative),
                true,
                None,
                Some(128),
                Some("unexpected-raw-credential".to_owned()),
                None,
                false,
                provider,
            )
            .expect("start no-auth generation");
        let (preserve, contexts, provenance) = capture_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("captured no-auth request");
        assert!(!preserve);
        assert!(contexts.is_empty());
        assert_eq!(
            provenance,
            Some(GenerationProviderProvenance {
                api_family: ApiFamily::OllamaNative,
                model_route_id: target.model_route_id,
                generation_preset_id: target.generation_preset_id,
            })
        );
        let generation =
            wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
        assert!(generation.opaque_reasoning_state.is_empty());
        wait_for_generation_registry_to_drain(&core);
        drop(core);

        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
        assert!(
            reopened
                .inner
                .storage
                .get_generation(&generation_id)
                .expect("reopened no-auth generation")
                .opaque_reasoning_state
                .is_empty()
        );
    }

    #[test]
    fn provider_model_sync_rejects_reflected_credential_without_persisting_it() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let secret = format!("reflected-secret-{}", Uuid::new_v4());
        let body = serde_json::json!({
            "data": [{"id": secret.clone()}],
        })
        .to_string();
        let (api_origin, requests) = spawn_model_list_provider(vec![body]);
        let (_template, connection) = create_openai_chat_connection(&core, &api_origin);

        let error = refresh_models_with_review(&core, &connection.id, Some(&secret))
            .expect_err("credential reflection must fail closed");
        requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured credential-bearing request");
        assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
        let jobs = core
            .list_provider_model_syncs(&connection.id, 4)
            .expect("durable failed job");
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].state, ModelSyncState::Failed);
        assert!(jobs[0].review.is_none());
        assert!(!format!("{jobs:?}").contains(&secret));

        drop(core);
        assert_directory_does_not_contain(root.path(), secret.as_bytes());
    }

    #[test]
    fn job_scoped_model_sync_event_poll_does_not_consume_another_job() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        for (id, origin) in [
            ("event-job-a", "https://events-a.example.com/v1"),
            ("event-job-b", "https://events-b.example.com/v1"),
        ] {
            core.upsert_provider_profile(ProviderProfile {
                id: id.to_owned(),
                display_name: id.to_owned(),
                base_url: origin.to_owned(),
                model: "existing-model".to_owned(),
                timeout_seconds: 30,
            })
            .expect("seed provider graph");
        }
        let first_connection = core
            .inner
            .storage
            .get_provider_connection(&ProviderConnectionId::from("event-job-a"))
            .expect("first connection");
        let second_connection = core
            .inner
            .storage
            .get_provider_connection(&ProviderConnectionId::from("event-job-b"))
            .expect("second connection");
        let first_job = core
            .inner
            .storage
            .create_model_sync_job(&first_connection)
            .expect("first model sync job");
        let second_job = core
            .inner
            .storage
            .create_model_sync_job(&second_connection)
            .expect("second model sync job");

        let first_events = core
            .poll_provider_model_sync_events(&first_job.id, 16)
            .expect("poll first job");
        assert_eq!(first_events.len(), 1);
        assert_eq!(first_events[0].job_id, first_job.id);
        assert!(
            core.ack_provider_model_sync_event(&first_job.id, first_events[0].sequence)
                .expect("ack first job")
        );

        let second_events = core
            .poll_provider_model_sync_events(&second_job.id, 16)
            .expect("poll second job");
        assert_eq!(second_events.len(), 1);
        assert_eq!(second_events[0].job_id, second_job.id);
        assert_eq!(
            core.poll_provider_model_sync_events(&second_job.id, 16)
                .expect("second event remains until acknowledged"),
            second_events
        );
        assert!(
            core.ack_provider_model_sync_event(&second_job.id, second_events[0].sequence)
                .expect("ack second job")
        );
    }

    #[test]
    fn provider_model_refresh_records_safe_failure_statuses() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let secret = format!("failure-secret-{}", Uuid::new_v4());

        let (auth_origin, auth_requests) = spawn_model_list_http_provider(vec![(
            "401 Unauthorized".to_owned(),
            r#"{"error":"invalid credential"}"#.to_owned(),
        )]);
        let (_template, auth_connection) = create_openai_chat_connection(&core, &auth_origin);
        let auth_error = refresh_models_with_review(&core, &auth_connection.id, Some(&secret))
            .expect_err("401 model refresh must fail");
        auth_requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured auth-failing request");
        assert_eq!(auth_error.code, CoreErrorCode::ProviderAuthFailed);
        assert!(!format!("{auth_error:?}").contains(&secret));
        assert_eq!(
            core.inner
                .storage
                .get_provider_connection(&auth_connection.id)
                .expect("auth-failed connection")
                .status,
            ConnectionStatus::AuthFailed
        );

        let (unavailable_origin, unavailable_requests) = spawn_model_list_http_provider(vec![(
            "503 Service Unavailable".to_owned(),
            r#"{"error":"temporarily unavailable"}"#.to_owned(),
        )]);
        let (_template, unavailable_connection) =
            create_openai_chat_connection(&core, &unavailable_origin);
        let unavailable_error =
            refresh_models_with_review(&core, &unavailable_connection.id, Some(&secret))
                .expect_err("503 model refresh must fail");
        unavailable_requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured unavailable request");
        assert_eq!(unavailable_error.code, CoreErrorCode::ProviderUnavailable);
        assert!(!format!("{unavailable_error:?}").contains(&secret));
        assert_eq!(
            core.inner
                .storage
                .get_provider_connection(&unavailable_connection.id)
                .expect("unavailable connection")
                .status,
            ConnectionStatus::Unavailable
        );
    }

    #[test]
    fn initial_model_preset_is_deferred_when_template_requires_an_explicit_value() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let templates = core.list_provider_templates().expect("provider templates");
        let anthropic = templates
            .iter()
            .find(|template| template.id.as_str() == "anthropic-messages-v1")
            .expect("Anthropic template");
        let openai_chat = templates
            .iter()
            .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
            .expect("OpenAI-compatible template");

        assert!(!template_accepts_empty_preset(anthropic).expect("Anthropic preset requirement"));
        assert!(
            template_accepts_empty_preset(openai_chat)
                .expect("OpenAI-compatible preset requirement")
        );
    }

    #[test]
    fn detached_generations_keep_process_admission_until_terminal() {
        let (_root, core, character) = imported_core();
        let mut generation_ids = Vec::with_capacity(MAX_ACTIVE_GENERATIONS_PER_PROCESS);

        for index in 0..MAX_ACTIVE_GENERATIONS_PER_PROCESS {
            let conversation = core.open_conversation(&character.id).expect("conversation");
            let (provider, provider_started) =
                StallingProvider::new(format!("detached partial {index}"));
            let generation_id = core
                .send_message_with_provider(
                    &conversation.id,
                    "start detached generation",
                    format!("detached-model-{index}"),
                    None,
                    provider,
                )
                .expect("start generation within process admission");
            provider_started
                .recv_timeout(Duration::from_secs(2))
                .expect("detached provider started");
            generation_ids.push(generation_id);
        }
        assert_eq!(
            core.active_generation_count(),
            MAX_ACTIVE_GENERATIONS_PER_PROCESS
        );

        let overflow_conversation = core.open_conversation(&character.id).expect("conversation");
        let (overflow_provider, overflow_started) = StallingProvider::new("must not dispatch");
        let overflow = core
            .send_message_with_provider(
                &overflow_conversation.id,
                "overflow detached generations",
                "overflow-model".to_owned(),
                None,
                overflow_provider,
            )
            .expect_err("a recycled renderer stream must not bypass Core admission");
        assert_eq!(overflow.code, CoreErrorCode::ProviderRateLimited);
        assert!(
            overflow_started
                .recv_timeout(Duration::from_millis(100))
                .is_err(),
            "an over-capacity generation must not reach provider dispatch"
        );

        core.cancel_generation(&generation_ids[0])
            .expect("cancel one admitted generation");
        wait_for_generation_status(&core, &generation_ids[0], GenerationStatus::Cancelled);
        wait_for_active_generation_count(&core, MAX_ACTIVE_GENERATIONS_PER_PROCESS - 1);
        let (replacement_provider, replacement_started) =
            StallingProvider::new("replacement partial");
        core.send_message_with_provider(
            &overflow_conversation.id,
            "overflow detached generations",
            "overflow-model".to_owned(),
            None,
            replacement_provider,
        )
        .expect("terminal generation releases Core admission for the exact retry");
        replacement_started
            .recv_timeout(Duration::from_secs(2))
            .expect("replacement provider started");
        assert_eq!(
            core.active_generation_count(),
            MAX_ACTIVE_GENERATIONS_PER_PROCESS
        );
    }

    #[test]
    fn dropping_last_core_from_a_runtime_worker_bounds_shutdown_and_releases_provider() {
        let (_root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let (provider, provider_started) = StallingProvider::new("partial before shutdown");
        let provider_weak = Arc::downgrade(&provider);
        core.send_message_with_provider(
            &conversation.id,
            "start",
            "stalling".to_owned(),
            Some("ephemeral-credential".to_owned()),
            provider,
        )
        .expect("start generation");
        provider_started
            .recv_timeout(Duration::from_secs(2))
            .expect("provider started");

        let runtime_handle = core.inner.runtime.handle.clone();
        let (dropped_sender, dropped_receiver) = std_mpsc::channel();
        std::mem::drop(runtime_handle.spawn(async move {
            let started = Instant::now();
            drop(core);
            let _ = dropped_sender.send(started.elapsed());
        }));

        let elapsed = dropped_receiver
            .recv_timeout(Duration::from_secs(4))
            .expect("core drop must not panic or deadlock on its runtime worker");
        assert!(
            elapsed < Duration::from_secs(3),
            "shutdown exceeded its cancellation and runtime bounds: {elapsed:?}"
        );
        assert!(
            provider_weak.upgrade().is_none(),
            "runtime shutdown must release the stalling provider and its captured state"
        );
    }

    #[test]
    fn hard_crash_generation_fixture_child() {
        let Some(root) = std::env::var_os(HARD_CRASH_GENERATION_ROOT_ENV) else {
            return;
        };
        let preserve_partial_generations =
            std::env::var(HARD_CRASH_GENERATION_PRESERVE_ENV).as_deref() == Ok("true");
        let reopen_preserve_partial_generations =
            std::env::var(HARD_CRASH_GENERATION_REOPEN_PRESERVE_ENV).as_deref() == Ok("true");
        let root = PathBuf::from(root);
        let core = Core::open(CoreConfig::new(&root)).expect("open hard-crash child Core");
        let mut settings = core.get_settings().expect("load hard-crash settings");
        settings.preserve_partial_generations = preserve_partial_generations;
        core.update_settings(&settings)
            .expect("configure hard-crash partial preservation");
        let mut card = NamedTempFile::new_in(&root).expect("hard-crash character card");
        write!(
            card,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Crash","description":"Fixture"}}}}"#,
        )
        .expect("write hard-crash character card");
        let inspection = core
            .inspect_import(card.path())
            .expect("inspect hard-crash card");
        let character = core
            .commit_import(&inspection.id)
            .expect("commit hard-crash card");
        let conversation = core
            .open_conversation(&character.id)
            .expect("open hard-crash conversation");
        let partial = std::env::var(HARD_CRASH_GENERATION_PARTIAL_ENV)
            .unwrap_or_else(|_| "durable hard-crash checkpoint".to_owned());
        let (provider, provider_started) = StallingProvider::new(&partial);
        let generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "start hard-crash fixture",
                "stalling".to_owned(),
                None,
                provider,
            )
            .expect("start hard-crash generation");
        provider_started
            .recv_timeout(Duration::from_secs(2))
            .expect("hard-crash provider started");
        if preserve_partial_generations {
            let checkpoint = wait_for_partial(&core, &conversation.id, &partial);
            assert_eq!(checkpoint.status, MessageStatus::Pending);
        }
        settings.preserve_partial_generations = reopen_preserve_partial_generations;
        core.update_settings(&settings)
            .expect("configure hard-crash reopen preservation");
        let generation = core
            .inner
            .storage
            .get_generation(&generation_id)
            .expect("read running hard-crash generation");
        let assistant_message_id = generation
            .assistant_message_id
            .clone()
            .expect("hard-crash generation assistant");
        let attempt = core
            .inner
            .storage
            .get_generation_attempt(&generation_id)
            .expect("read running hard-crash attempt");
        assert_eq!(attempt.status, GenerationAttemptStatus::Running);
        let fixture = HardCrashGenerationFixture {
            conversation_id: conversation.id.0,
            branch_id: generation.branch_id.0,
            user_message_id: generation.user_message_id.0,
            assistant_message_id: assistant_message_id.0,
            generation_id: generation_id.0,
            running_attempt_revision: attempt.revision,
            partial,
        };
        let encoded = serde_json::to_vec(&fixture).expect("encode hard-crash generation fixture");
        let mut sidecar = File::create(hard_crash_generation_fixture_path(&root))
            .expect("create hard-crash generation fixture");
        sidecar
            .write_all(&encoded)
            .expect("write hard-crash generation fixture");
        sidecar
            .flush()
            .expect("flush hard-crash generation fixture");
        sidecar
            .sync_all()
            .expect("sync hard-crash generation fixture");
        std::process::exit(HARD_CRASH_GENERATION_EXIT_CODE);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn hard_crash_recovery_closes_attempt_and_terminal_lifecycle_once() {
        for preserve_partial_generations in [true, false] {
            let root = tempdir().expect("hard-crash recovery root");
            let fixture = run_hard_crash_generation_child(
                root.path(),
                preserve_partial_generations,
                preserve_partial_generations,
                "durable hard-crash checkpoint",
            );
            let generation_id = GenerationId(fixture.generation_id.clone());
            let reopened = Core::open(CoreConfig::new(root.path())).expect("recover hard crash");
            let generation = reopened
                .inner
                .storage
                .get_generation(&generation_id)
                .expect("recovered generation");
            assert_eq!(generation.status, GenerationStatus::Cancelled);
            assert_eq!(
                generation.error_code.as_deref(),
                Some(CoreErrorCode::Cancelled.as_str())
            );
            assert!(generation.finished_at.is_some());
            assert_eq!(
                reopened
                    .get_conversation(&ConversationId(fixture.conversation_id.clone()))
                    .expect("recovered conversation")
                    .updated_at,
                generation.finished_at.expect("recovery finished_at")
            );
            let attempt = reopened
                .inner
                .storage
                .get_generation_attempt(&generation_id)
                .expect("recovered generation attempt");
            assert_eq!(attempt.status, GenerationAttemptStatus::Completed);
            assert_eq!(attempt.revision, fixture.running_attempt_revision + 1);

            let messages = reopened
                .list_messages(&ConversationId(fixture.conversation_id.clone()))
                .expect("recovered messages");
            let branch = reopened
                .inner
                .storage
                .get_conversation_branch(&ConversationBranchId(fixture.branch_id.clone()))
                .expect("recovered branch");
            if preserve_partial_generations {
                assert_eq!(messages.len(), 2);
                assert_eq!(messages[1].id.0, fixture.assistant_message_id);
                assert_eq!(messages[1].content, fixture.partial);
                assert_eq!(messages[1].status, MessageStatus::Cancelled);
                assert_eq!(
                    branch.head_message_id.as_ref().map(|id| id.0.as_str()),
                    Some(fixture.assistant_message_id.as_str())
                );
            } else {
                assert_eq!(messages.len(), 1);
                assert_eq!(messages[0].id.0, fixture.user_message_id);
                assert_eq!(
                    branch.head_message_id.as_ref().map(|id| id.0.as_str()),
                    Some(fixture.user_message_id.as_str())
                );
            }

            let rows = generation_lifecycle_rows(root.path(), &fixture.generation_id);
            let expected_count = if preserve_partial_generations { 2 } else { 1 };
            assert_eq!(rows.len(), expected_count);
            assert_eq!(
                rows[0],
                GenerationLifecycleRow {
                    occurrence_id: format!("after-generation:{}", fixture.generation_id),
                    event_kind: "after_generation".to_owned(),
                    status: "pending".to_owned(),
                    exact_head_message_id: Some(if preserve_partial_generations {
                        fixture.assistant_message_id.clone()
                    } else {
                        fixture.user_message_id.clone()
                    }),
                    owner_message_id: preserve_partial_generations
                        .then(|| fixture.assistant_message_id.clone()),
                }
            );
            if preserve_partial_generations {
                assert_eq!(
                    rows[1],
                    GenerationLifecycleRow {
                        occurrence_id: format!(
                            "message-committed:{}",
                            fixture.assistant_message_id
                        ),
                        event_kind: "message_committed".to_owned(),
                        status: "pending".to_owned(),
                        exact_head_message_id: Some(fixture.assistant_message_id.clone()),
                        owner_message_id: Some(fixture.assistant_message_id.clone()),
                    }
                );
            }

            let receipt = reopened
                .drain_core_lifecycle_occurrences(64)
                .expect("drain recovered lifecycle");
            let recovered_events = receipt
                .deliveries
                .iter()
                .filter(|delivery| {
                    delivery.generation_id.as_ref().map(|id| id.0.as_str())
                        == Some(fixture.generation_id.as_str())
                })
                .map(|delivery| delivery.event_kind)
                .collect::<Vec<_>>();
            let expected_events = if preserve_partial_generations {
                vec![
                    LifecycleOccurrenceKind::AfterGeneration,
                    LifecycleOccurrenceKind::MessageCommitted,
                ]
            } else {
                vec![LifecycleOccurrenceKind::AfterGeneration]
            };
            assert_eq!(recovered_events, expected_events);
            drop(reopened);

            let reopened =
                Core::open(CoreConfig::new(root.path())).expect("second hard-crash recovery open");
            assert_eq!(
                reopened
                    .inner
                    .storage
                    .get_generation_attempt(&generation_id)
                    .expect("idempotent attempt")
                    .revision,
                fixture.running_attempt_revision + 1
            );
            let rows = generation_lifecycle_rows(root.path(), &fixture.generation_id);
            assert_eq!(rows.len(), expected_count);
            assert!(rows.iter().all(|row| row.status == "acknowledged"));
            let second = reopened
                .drain_core_lifecycle_occurrences(64)
                .expect("second lifecycle drain");
            assert!(second.deliveries.iter().all(|delivery| {
                delivery.generation_id.as_ref().map(|id| id.0.as_str())
                    != Some(fixture.generation_id.as_str())
            }));
        }
    }

    #[test]
    fn hard_crash_recovery_uses_durable_checkpoint_instead_of_reopen_setting() {
        for (label, launch_preserve, reopen_preserve, partial, durable_content, keep_assistant) in [
            ("empty-before-first-checkpoint", true, true, "", "", false),
            (
                "checkpoint-survives-setting-disable",
                true,
                false,
                "durable checkpoint",
                "durable checkpoint",
                true,
            ),
            (
                "disabled-launch-cannot-be-enabled-on-reopen",
                false,
                true,
                "uncheckpointed delta",
                "",
                false,
            ),
        ] {
            let root = tempdir().expect("hard-crash policy root");
            let fixture = run_hard_crash_generation_child(
                root.path(),
                launch_preserve,
                reopen_preserve,
                partial,
            );
            assert_eq!(
                hard_crash_assistant_content(root.path(), &fixture.assistant_message_id),
                durable_content,
                "{label}: launch policy must determine the durable checkpoint fact"
            );
            let reopened = Core::open(CoreConfig::new(root.path())).expect("recover hard crash");
            let messages = reopened
                .list_messages(&ConversationId(fixture.conversation_id.clone()))
                .expect("recovered messages");
            let retained = messages
                .iter()
                .find(|message| message.id.0 == fixture.assistant_message_id);
            assert_eq!(
                retained.is_some(),
                keep_assistant,
                "{label}: reopen setting must not reinterpret the durable checkpoint"
            );
            if let Some(retained) = retained {
                assert_eq!(retained.content, durable_content);
                assert_eq!(retained.status, MessageStatus::Cancelled);
            }
            let rows = generation_lifecycle_rows(root.path(), &fixture.generation_id);
            assert_eq!(rows.len(), if keep_assistant { 2 } else { 1 });
            assert_eq!(
                rows.iter().any(|row| row.event_kind == "message_committed"),
                keep_assistant,
                "{label}: MessageCommitted must match retained durable content"
            );
        }
    }

    #[test]
    fn interrupted_generation_recovery_rolls_back_on_outbox_conflict() {
        let root = tempdir().expect("hard-crash rollback root");
        let fixture = run_hard_crash_generation_child(
            root.path(),
            true,
            true,
            "durable hard-crash checkpoint",
        );
        let database_path = hard_crash_database_path(root.path());
        let database = rusqlite::Connection::open(&database_path).expect("open crash database");
        database
            .execute_batch(
                "CREATE TRIGGER test_reject_recovered_message_committed
                 BEFORE INSERT ON core_lifecycle_outbox
                 WHEN NEW.event_kind = 'message_committed'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic recovered lifecycle failure');
                 END;",
            )
            .expect("inject recovered lifecycle failure");
        drop(database);

        let error = Core::open(CoreConfig::new(root.path()))
            .err()
            .expect("outbox conflict must reject recovery");
        assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
        let database =
            rusqlite::Connection::open(&database_path).expect("inspect rollback database");
        assert_eq!(
            database
                .query_row(
                    "SELECT status FROM generations WHERE id = ?1",
                    [&fixture.generation_id],
                    |row| row.get::<_, String>(0),
                )
                .expect("generation status after rollback"),
            "running"
        );
        assert_eq!(
            database
                .query_row(
                    "SELECT status FROM messages WHERE id = ?1",
                    [&fixture.assistant_message_id],
                    |row| row.get::<_, String>(0),
                )
                .expect("message status after rollback"),
            "pending"
        );
        assert_eq!(
            database
                .query_row(
                    "SELECT status, revision
                     FROM generation_attempt_intents WHERE generation_id = ?1",
                    [&fixture.generation_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
                )
                .expect("attempt after rollback"),
            ("running".to_owned(), fixture.running_attempt_revision)
        );
        assert_eq!(
            database
                .query_row(
                    "SELECT COUNT(*)
                     FROM core_lifecycle_outbox
                     WHERE generation_id = ?1
                       AND event_kind IN ('after_generation', 'message_committed')",
                    [&fixture.generation_id],
                    |row| row.get::<_, u64>(0),
                )
                .expect("terminal lifecycle count after rollback"),
            0,
            "the earlier AfterGeneration insert must roll back with MessageCommitted"
        );
        database
            .execute_batch("DROP TRIGGER test_reject_recovered_message_committed;")
            .expect("remove recovered lifecycle failure");
        drop(database);
        Core::open(CoreConfig::new(root.path())).expect("recover after removing outbox conflict");
    }

    #[test]
    fn timed_partial_checkpoint_survives_restart_when_preservation_is_enabled() {
        let (root, core, character) = imported_core();
        core.update_settings(&AppSettings {
            preserve_partial_generations: true,
            selected_provider_profile_id: None,
            selected_model_route_id: None,
            selected_generation_preset_id: None,
            ..AppSettings::default()
        })
        .expect("enable partial preservation");
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let partial = "latest timer checkpoint";
        let (provider, provider_started) = StallingProvider::new(partial);
        core.send_message_with_provider(
            &conversation.id,
            "start",
            "stalling".to_owned(),
            None,
            provider,
        )
        .expect("start generation");
        provider_started
            .recv_timeout(Duration::from_secs(2))
            .expect("provider started");

        let checkpoint = wait_for_partial(&core, &conversation.id, partial);
        assert_eq!(checkpoint.status, MessageStatus::Pending);
        drop(core);

        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
        let messages = reopened
            .list_messages(&conversation.id)
            .expect("restored messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, partial);
        assert_eq!(messages[1].status, MessageStatus::Cancelled);
    }

    #[test]
    fn partial_checkpoint_is_never_written_when_preservation_is_disabled() {
        let (root, core, character) = imported_core();
        core.update_settings(&AppSettings {
            preserve_partial_generations: false,
            selected_provider_profile_id: None,
            selected_model_route_id: None,
            selected_generation_preset_id: None,
            ..AppSettings::default()
        })
        .expect("disable partial preservation");
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let partial = "must not persist";
        let (provider, provider_started) = StallingProvider::new(partial);
        core.send_message_with_provider(
            &conversation.id,
            "start",
            "stalling".to_owned(),
            None,
            provider,
        )
        .expect("start generation");
        provider_started
            .recv_timeout(Duration::from_secs(2))
            .expect("provider started");
        thread::sleep(PARTIAL_CHECKPOINT_INTERVAL + Duration::from_millis(150));

        let messages = core.list_messages(&conversation.id).expect("messages");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].status, MessageStatus::Pending);
        assert!(messages[1].content.is_empty());
        drop(core);

        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
        let restored = reopened
            .list_messages(&conversation.id)
            .expect("restored messages");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].content, "start");
    }

    #[test]
    fn partial_checkpoint_byte_threshold_is_inclusive() {
        assert!(!partial_checkpoint_due(PARTIAL_CHECKPOINT_BYTES - 1, 0));
        assert!(partial_checkpoint_due(PARTIAL_CHECKPOINT_BYTES, 0));
        assert!(partial_checkpoint_due(
            PARTIAL_CHECKPOINT_BYTES * 2,
            PARTIAL_CHECKPOINT_BYTES
        ));
    }

    #[test]
    fn import_and_restart_restore_library() {
        let (root, core, _) = imported_core();
        drop(core);
        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
        assert_eq!(reopened.list_characters().expect("library").len(), 1);
    }

    #[test]
    fn import_uses_an_owned_snapshot_and_cleans_it_after_commit() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let mut card = NamedTempFile::new_in(root.path()).expect("card");
        write!(
            card,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Snapshot","description":"Safe"}}}}"#
        )
        .expect("write card");

        let inspection = core.inspect_import(card.path()).expect("inspect");
        fs::write(card.path(), b"changed after inspection").expect("mutate original");
        let character = core.commit_import(&inspection.id).expect("commit snapshot");

        assert_eq!(character.name, "Snapshot");
        assert!(
            fs::read_dir(core.inner.storage.staging_dir())
                .expect("staging directory")
                .next()
                .is_none(),
            "committed snapshots must be removed"
        );
    }

    #[test]
    fn discard_and_restart_cleanup_owned_staging_files() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let mut card = NamedTempFile::new_in(root.path()).expect("card");
        write!(
            card,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Discard","description":"Safe"}}}}"#
        )
        .expect("write card");
        let inspection = core.inspect_import(card.path()).expect("inspect");
        core.discard_import(&inspection.id).expect("discard");
        assert!(
            fs::read_dir(core.inner.storage.staging_dir())
                .expect("staging directory")
                .next()
                .is_none()
        );

        let abandoned = core
            .inner
            .storage
            .staging_dir()
            .join("inspection-abandoned.json");
        fs::write(&abandoned, b"abandoned").expect("abandoned staging file");
        drop(core);
        let _reopened = open_core_after_drop(root.path());
        assert!(
            !abandoned.exists(),
            "restart must clean abandoned snapshots"
        );
    }

    #[test]
    fn concurrent_commits_atomically_claim_one_inspection() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let mut card = NamedTempFile::new_in(root.path()).expect("card");
        write!(
            card,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Claim","description":"Safe"}}}}"#
        )
        .expect("write card");
        let inspection = core.inspect_import(card.path()).expect("inspect");
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let core = core.clone();
            let inspection_id = inspection.id.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(thread::spawn(move || {
                barrier.wait();
                core.commit_import(&inspection_id)
            }));
        }
        barrier.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("commit worker"))
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let loser = results
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("one losing commit");
        assert_eq!(loser.code, CoreErrorCode::NotFound);
        assert_eq!(core.list_characters().expect("characters").len(), 1);
    }

    #[test]
    fn concurrent_commit_and_discard_have_one_atomic_winner() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let mut card = NamedTempFile::new_in(root.path()).expect("card");
        write!(
            card,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Race","description":"Safe"}}}}"#
        )
        .expect("write card");
        let inspection = core.inspect_import(card.path()).expect("inspect");
        let barrier = Arc::new(Barrier::new(3));
        let commit_core = core.clone();
        let commit_id = inspection.id.clone();
        let commit_barrier = Arc::clone(&barrier);
        let commit = thread::spawn(move || {
            commit_barrier.wait();
            commit_core.commit_import(&commit_id)
        });
        let discard_core = core.clone();
        let discard_id = inspection.id.clone();
        let discard_barrier = Arc::clone(&barrier);
        let discard = thread::spawn(move || {
            discard_barrier.wait();
            discard_core.discard_import(&discard_id)
        });
        barrier.wait();
        let commit = commit.join().expect("commit worker");
        let discard = discard.join().expect("discard worker");

        assert_ne!(commit.is_ok(), discard.is_ok());
        let loser = commit
            .as_ref()
            .err()
            .or_else(|| discard.as_ref().err())
            .expect("one losing operation");
        assert_eq!(loser.code, CoreErrorCode::NotFound);
        assert_eq!(
            core.list_characters().expect("characters").len(),
            usize::from(commit.is_ok())
        );
    }

    #[test]
    fn precommit_failure_restores_the_claim_for_a_safe_retry() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let mut card = NamedTempFile::new_in(root.path()).expect("card");
        let card_bytes =
            br#"{"spec":"chara_card_v3","data":{"name":"Retry","description":"Safe"}}"#;
        card.write_all(card_bytes).expect("write card");
        let inspection = core.inspect_import(card.path()).expect("inspect");
        let database_path = hard_crash_database_path(root.path());
        let database = rusqlite::Connection::open(database_path).expect("open database");
        database
            .execute_batch(
                "CREATE TRIGGER test_reject_character_import_journal
                 BEFORE INSERT ON import_jobs
                 BEGIN
                     SELECT RAISE(ABORT, 'synthetic character import failure');
                 END;",
            )
            .expect("install precommit failure injector");

        let error = core
            .commit_import(&inspection.id)
            .expect_err("precommit failure");
        assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
        assert!(
            core.inner
                .pending_imports
                .read()
                .expect("pending imports")
                .contains_key(&inspection.id),
            "a definitely uncommitted claim must be restored"
        );

        database
            .execute("DROP TRIGGER test_reject_character_import_journal", [])
            .expect("remove precommit failure injector");
        let character = core.commit_import(&inspection.id).expect("safe retry");
        assert_eq!(character.name, "Retry");
        assert_eq!(core.list_characters().expect("characters").len(), 1);
    }

    #[test]
    fn user_message_and_provider_fields_have_utf8_safe_inclusive_bounds() {
        let exact_message = "😀".repeat(MAX_USER_MESSAGE_CHARS);
        assert_eq!(exact_message.len(), MAX_USER_MESSAGE_BYTES);
        validate_bounded_text(
            "message text",
            &exact_message,
            MAX_USER_MESSAGE_BYTES,
            MAX_USER_MESSAGE_CHARS,
        )
        .expect("exact message boundary");
        let message_error = validate_bounded_text(
            "message text",
            &format!("{exact_message}😀"),
            MAX_USER_MESSAGE_BYTES,
            MAX_USER_MESSAGE_CHARS,
        )
        .expect_err("message over boundary");
        assert_eq!(message_error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            message_error.message,
            "message text exceeds the 65536-byte or 16384-character limit"
        );

        for (field, max_bytes, max_chars) in [
            (
                "provider profile id",
                MAX_PROVIDER_ID_BYTES,
                MAX_PROVIDER_ID_CHARS,
            ),
            (
                "provider display name",
                MAX_PROVIDER_DISPLAY_NAME_BYTES,
                MAX_PROVIDER_DISPLAY_NAME_CHARS,
            ),
            (
                "provider base URL",
                MAX_PROVIDER_BASE_URL_BYTES,
                MAX_PROVIDER_BASE_URL_CHARS,
            ),
            (
                "provider model",
                MAX_PROVIDER_MODEL_BYTES,
                MAX_PROVIDER_MODEL_CHARS,
            ),
        ] {
            let exact = "😀".repeat(max_chars);
            assert_eq!(exact.len(), max_bytes);
            validate_bounded_text(field, &exact, max_bytes, max_chars)
                .expect("exact provider field boundary");
            assert!(
                validate_bounded_text(field, &format!("{exact}😀"), max_bytes, max_chars).is_err()
            );
        }
    }

    #[test]
    fn oversized_user_input_and_provider_fields_are_not_persisted() {
        let (_root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let error = core
            .send_message_with_provider(
                &conversation.id,
                &"😀".repeat(MAX_USER_MESSAGE_CHARS + 1),
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("unused")),
            )
            .expect_err("oversized message");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(
            core.list_messages(&conversation.id)
                .expect("messages")
                .is_empty()
        );

        let profile_error = core
            .upsert_provider_profile(ProviderProfile {
                id: "provider".to_owned(),
                display_name: "Provider".to_owned(),
                base_url: "http://127.0.0.1:11434/v1".to_owned(),
                model: "😀".repeat(MAX_PROVIDER_MODEL_CHARS + 1),
                timeout_seconds: 30,
            })
            .expect_err("oversized model");
        assert_eq!(profile_error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            profile_error.message,
            "provider model exceeds the 1024-byte or 256-character limit"
        );
        assert!(core.list_provider_profiles().expect("profiles").is_empty());
    }

    #[test]
    fn every_provider_profile_string_is_bounded_before_storage() {
        let root = tempdir().expect("temp root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let valid = || ProviderProfile {
            id: "provider".to_owned(),
            display_name: "Provider".to_owned(),
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            model: "model".to_owned(),
            timeout_seconds: 30,
        };
        let mut cases = Vec::new();
        let mut oversized_id = valid();
        oversized_id.id = "😀".repeat(MAX_PROVIDER_ID_CHARS + 1);
        cases.push(("provider profile id", oversized_id));
        let mut oversized_display = valid();
        oversized_display.display_name = "😀".repeat(MAX_PROVIDER_DISPLAY_NAME_CHARS + 1);
        cases.push(("provider display name", oversized_display));
        let mut oversized_url = valid();
        oversized_url.base_url = format!(
            "http://127.0.0.1/{}",
            "a".repeat(MAX_PROVIDER_BASE_URL_BYTES)
        );
        cases.push(("provider base URL", oversized_url));
        let mut oversized_model = valid();
        oversized_model.model = "😀".repeat(MAX_PROVIDER_MODEL_CHARS + 1);
        cases.push(("provider model", oversized_model));

        for (field, profile) in cases {
            let error = core
                .upsert_provider_profile(profile)
                .expect_err("oversized provider field");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(error.message.starts_with(field), "{:?}", error.message);
        }
        assert!(core.list_provider_profiles().expect("profiles").is_empty());
    }

    #[test]
    fn one_character_can_own_multiple_explicit_rooms_with_independent_modes() {
        let (_root, core, character) = imported_core();
        let chat = core
            .create_conversation(&character.id, "첫 번째 방", ConversationMode::Chat)
            .expect("chat room");
        let story = core
            .create_conversation(&character.id, "두 번째 방", ConversationMode::Story)
            .expect("story room");

        assert_ne!(chat.id, story.id);
        assert_eq!(
            core.list_conversations_for_character(&character.id)
                .expect("character rooms")
                .len(),
            2
        );
        assert_eq!(
            core.get_conversation_state(&chat.id)
                .expect("chat state")
                .selected_mode,
            ConversationMode::Chat
        );
        assert_eq!(
            core.get_conversation_state(&story.id)
                .expect("story state")
                .selected_mode,
            ConversationMode::Story
        );
        assert_eq!(
            core.list_conversation_branches(&chat.id)
                .expect("default branch")
                .len(),
            1
        );
    }

    #[test]
    fn generation_assembly_preserves_validated_temperature_and_default_omission() {
        let (_root, core, character) = imported_core();
        let finite_conversation = core
            .create_conversation(&character.id, "온도 검증", ConversationMode::Chat)
            .expect("finite-temperature conversation");
        let finite_state = core
            .get_conversation_state(&finite_conversation.id)
            .expect("finite-temperature state");
        let (provider, _messages, captured_temperature) =
            CapturingProvider::new_with_temperature_capture("응답");

        let invalid = core
            .send_message_to_branch_with_provider_options(
                &finite_conversation.id,
                &finite_state.active_branch_id,
                None,
                ConversationMode::Chat,
                "전송되면 안 됨",
                new_test_generation_operation("invalid-temperature-v1"),
                "model".to_owned(),
                None,
                None,
                false,
                Some(f64::NAN),
                Some(1),
                None,
                None,
                false,
                Arc::new(StaticProvider::new("unused")),
            )
            .expect_err("non-finite temperature must fail before persistence");
        assert_eq!(invalid.code, CoreErrorCode::InvalidInput);
        assert!(
            core.list_branch_messages(&finite_state.active_branch_id)
                .expect("unchanged branch")
                .is_empty()
        );

        // This synthetic direct-provider path has no compiled route schema.
        // Core therefore validates finiteness and preserves the exact value;
        // route-backed family-specific bounds are enforced before assembly.
        core.send_message_to_branch_with_provider_options(
            &finite_conversation.id,
            &finite_state.active_branch_id,
            None,
            ConversationMode::Chat,
            "유한 온도",
            new_test_generation_operation("finite-temperature-v1"),
            "model".to_owned(),
            None,
            None,
            false,
            Some(3.0),
            Some(1),
            None,
            None,
            false,
            provider,
        )
        .expect("finite-temperature generation");
        assert_eq!(
            captured_temperature
                .recv_timeout(Duration::from_secs(2))
                .expect("captured finite temperature"),
            Some(3.0)
        );

        let default_conversation = core
            .create_conversation(&character.id, "기본 온도", ConversationMode::Chat)
            .expect("default conversation");
        let default_state = core
            .get_conversation_state(&default_conversation.id)
            .expect("default state");
        let (provider, _messages, captured_temperature) =
            CapturingProvider::new_with_temperature_capture("응답");
        core.send_message_to_branch_with_provider_options(
            &default_conversation.id,
            &default_state.active_branch_id,
            None,
            ConversationMode::Chat,
            "기본값",
            new_test_generation_operation("default-temperature-v1"),
            "model".to_owned(),
            None,
            None,
            false,
            None,
            Some(1),
            None,
            None,
            false,
            provider,
        )
        .expect("default generation");
        assert_eq!(
            captured_temperature
                .recv_timeout(Duration::from_secs(2))
                .expect("captured omitted temperature"),
            None
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn forked_branch_uses_only_its_parent_lineage_and_rejects_a_stale_head() {
        let (_root, core, character) = imported_core();
        let conversation = core
            .create_conversation(&character.id, "분기 테스트", ConversationMode::Chat)
            .expect("conversation");
        core.send_message_with_provider(
            &conversation.id,
            "공통 시작",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("원본 답변")),
        )
        .expect("initial generation");
        let deadline = Instant::now() + Duration::from_secs(2);
        let original = loop {
            let messages = core.list_messages(&conversation.id).expect("messages");
            if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
                break messages;
            }
            assert!(Instant::now() < deadline, "initial generation timed out");
            thread::sleep(Duration::from_millis(10));
        };

        let fork = core
            .create_conversation_branch(
                &conversation.id,
                Some(&original[0].id),
                Some("다른 선택".to_owned()),
            )
            .expect("fork");
        let (provider, captured) = CapturingProvider::new("분기 답변");
        let generation_id = core
            .send_message_to_branch_with_provider(
                &conversation.id,
                &fork.id,
                Some(&original[0].id),
                ConversationMode::Story,
                "분기 질문",
                new_test_generation_operation("branch-question-v1"),
                "captured".to_owned(),
                None,
                provider,
            )
            .expect("branch generation");
        let request_messages = captured
            .recv_timeout(Duration::from_secs(2))
            .expect("captured prompt");
        assert!(
            request_messages
                .first()
                .is_some_and(|message| message.contains("Story mode:")),
            "the provider prompt must use the generation snapshot mode"
        );
        assert!(
            request_messages
                .iter()
                .any(|message| message == "공통 시작")
        );
        assert!(
            request_messages
                .iter()
                .any(|message| message == "분기 질문")
        );
        assert!(
            !request_messages
                .iter()
                .any(|message| message == "원본 답변"),
            "a sibling assistant response must not leak into the fork prompt"
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let forked = loop {
            let messages = core.list_branch_messages(&fork.id).expect("fork messages");
            if messages.len() == 3
                && messages
                    .last()
                    .is_some_and(|message| message.status == MessageStatus::Complete)
            {
                break messages;
            }
            assert!(Instant::now() < deadline, "branch generation timed out");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            forked
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["공통 시작", "분기 질문", "분기 답변"]
        );
        assert_eq!(
            core.inner
                .storage
                .get_generation(&generation_id)
                .expect("generation snapshot")
                .mode,
            ConversationMode::Story
        );
        assert_eq!(
            core.list_branch_messages(
                &core
                    .get_conversation_state(&conversation.id)
                    .expect("state")
                    .active_branch_id
            )
            .expect("original branch")
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
            ["공통 시작", "원본 답변"]
        );

        let stale = core
            .send_message_to_branch_with_provider(
                &conversation.id,
                &fork.id,
                Some(&original[0].id),
                ConversationMode::Story,
                "오래된 head",
                new_test_generation_operation("stale-branch-head-v1"),
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("should not run")),
            )
            .expect_err("stale branch head");
        assert_eq!(stale.code, CoreErrorCode::InvalidInput);
        assert!(stale.recoverable);
        assert_eq!(
            core.list_branch_messages(&fork.id)
                .expect("unchanged fork")
                .len(),
            3
        );

        core.select_conversation_branch(&conversation.id, &fork.id)
            .expect("select fork");
        assert_eq!(
            core.list_messages(&conversation.id)
                .expect("active branch messages"),
            forked
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn message_actions_fork_immutable_lineage_and_rewind_without_deleting_rows() {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(&character.id, "메시지 액션", ConversationMode::Chat)
            .expect("conversation");
        core.send_message_with_provider(
            &conversation.id,
            "원본 질문",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("원본 답변")),
        )
        .expect("initial generation");
        let deadline = Instant::now() + Duration::from_secs(2);
        let original = loop {
            let messages = core.list_messages(&conversation.id).expect("messages");
            if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
                break messages;
            }
            assert!(Instant::now() < deadline, "initial generation timed out");
            thread::sleep(Duration::from_millis(10));
        };
        let source_branch_id = core
            .get_conversation_state(&conversation.id)
            .expect("source state")
            .active_branch_id;
        core.set_conversation_mode(&conversation.id, ConversationMode::Story)
            .expect("story mode");

        let (edit_provider, edited_prompt) = CapturingProvider::new("수정 답변");
        let edited = core
            .edit_user_message_with_provider(
                &conversation.id,
                &source_branch_id,
                Some(&original[1].id),
                &original[0].id,
                "수정 질문",
                "edited-model".to_owned(),
                None,
                edit_provider,
            )
            .expect("edit user");
        let edited_request = edited_prompt
            .recv_timeout(Duration::from_secs(2))
            .expect("edited prompt");
        assert!(
            edited_request
                .first()
                .is_some_and(|message| message.contains("Story mode:"))
        );
        assert!(edited_request.iter().any(|message| message == "수정 질문"));
        assert!(!edited_request.iter().any(|message| message == "원본 질문"));
        let deadline = Instant::now() + Duration::from_secs(2);
        let edited_messages = loop {
            let messages = core
                .list_branch_messages(&edited.branch.id)
                .expect("edited branch");
            if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
                break messages;
            }
            assert!(Instant::now() < deadline, "edited generation timed out");
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(
            edited_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["수정 질문", "수정 답변"]
        );
        assert_eq!(
            core.get_conversation_state(&conversation.id)
                .expect("edited state")
                .active_branch_id,
            edited.branch.id
        );
        assert_eq!(
            core.inner
                .storage
                .get_generation(&edited.generation_id)
                .expect("edited generation")
                .mode,
            ConversationMode::Story
        );
        assert_eq!(
            core.list_branch_messages(&source_branch_id)
                .expect("original branch"),
            original
        );

        core.select_conversation_branch(&conversation.id, &source_branch_id)
            .expect("select original");
        let (regenerate_provider, regenerated_prompt) = CapturingProvider::new("새 답변");
        let regenerated = core
            .regenerate_assistant_message_with_provider(
                &conversation.id,
                &source_branch_id,
                Some(&original[1].id),
                &original[1].id,
                "regenerated-model".to_owned(),
                None,
                regenerate_provider,
            )
            .expect("regenerate assistant");
        let regenerated_request = regenerated_prompt
            .recv_timeout(Duration::from_secs(2))
            .expect("regenerated prompt");
        assert!(
            regenerated_request
                .iter()
                .any(|message| message == "원본 질문")
        );
        assert!(
            !regenerated_request
                .iter()
                .any(|message| message == "원본 답변")
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let regenerated_messages = loop {
            let messages = core
                .list_branch_messages(&regenerated.branch.id)
                .expect("regenerated branch");
            if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
                break messages;
            }
            assert!(
                Instant::now() < deadline,
                "regenerated generation timed out"
            );
            thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(regenerated_messages[0].content, "원본 질문");
        assert_ne!(regenerated_messages[0].id, original[0].id);
        assert_eq!(regenerated_messages[1].content, "새 답변");
        assert_eq!(
            core.list_branch_messages(&source_branch_id)
                .expect("preserved original"),
            original
        );

        let rows_before_remove = core.database_stats().expect("stats").messages;
        let rewound = core
            .remove_message_from_branch(
                &conversation.id,
                &regenerated.branch.id,
                Some(&regenerated_messages[1].id),
                &regenerated_messages[1].id,
            )
            .expect("remove regenerated assistant");
        assert_eq!(
            rewound.head_message_id,
            Some(regenerated_messages[0].id.clone())
        );
        assert_eq!(
            core.list_branch_messages(&regenerated.branch.id)
                .expect("rewound branch"),
            vec![regenerated_messages[0].clone()]
        );
        assert_eq!(
            core.database_stats().expect("stats").messages,
            rows_before_remove,
            "logical removal must preserve immutable message rows"
        );

        drop(core);
        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
        assert_eq!(
            reopened
                .get_conversation_state(&conversation.id)
                .expect("restored state")
                .active_branch_id,
            regenerated.branch.id
        );
        assert_eq!(
            reopened
                .list_branch_messages(&source_branch_id)
                .expect("restored original"),
            original
        );
        assert_eq!(
            reopened.database_stats().expect("restored stats").messages,
            rows_before_remove
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn message_actions_reject_wrong_roles_stale_context_foreign_rooms_and_pending_heads() {
        let (_root, core, character) = imported_core();
        let conversation = core
            .create_conversation(&character.id, "거절 테스트", ConversationMode::Chat)
            .expect("conversation");
        core.send_message_with_provider(
            &conversation.id,
            "질문",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("답변")),
        )
        .expect("initial generation");
        let deadline = Instant::now() + Duration::from_secs(2);
        let messages = loop {
            let messages = core.list_messages(&conversation.id).expect("messages");
            if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
                break messages;
            }
            assert!(Instant::now() < deadline, "initial generation timed out");
            thread::sleep(Duration::from_millis(10));
        };
        let branch_id = core
            .get_conversation_state(&conversation.id)
            .expect("state")
            .active_branch_id;

        let edit_assistant = core
            .edit_user_message_with_provider(
                &conversation.id,
                &branch_id,
                Some(&messages[1].id),
                &messages[1].id,
                "잘못된 편집",
                "unused".to_owned(),
                None,
                Arc::new(StaticProvider::new("unused")),
            )
            .expect_err("assistant cannot be edited");
        assert_eq!(edit_assistant.code, CoreErrorCode::InvalidInput);
        let regenerate_user = core
            .regenerate_assistant_message_with_provider(
                &conversation.id,
                &branch_id,
                Some(&messages[1].id),
                &messages[0].id,
                "unused".to_owned(),
                None,
                Arc::new(StaticProvider::new("unused")),
            )
            .expect_err("user cannot be regenerated");
        assert_eq!(regenerate_user.code, CoreErrorCode::InvalidInput);

        let stale = core
            .remove_message_from_branch(
                &conversation.id,
                &branch_id,
                Some(&messages[0].id),
                &messages[1].id,
            )
            .expect_err("stale expected head");
        assert_eq!(stale.code, CoreErrorCode::InvalidInput);
        assert!(stale.recoverable);

        let foreign = core
            .create_conversation(&character.id, "다른 방", ConversationMode::Chat)
            .expect("foreign conversation");
        let foreign_error = core
            .remove_message_from_branch(
                &foreign.id,
                &branch_id,
                Some(&messages[1].id),
                &messages[1].id,
            )
            .expect_err("foreign conversation");
        assert_eq!(foreign_error.code, CoreErrorCode::NotFound);

        let (stalling, started) = StallingProvider::new("생성 중");
        core.send_message_to_branch_with_provider(
            &conversation.id,
            &branch_id,
            Some(&messages[1].id),
            ConversationMode::Chat,
            "다음 질문",
            new_test_generation_operation("pending-generation-v1"),
            "stalling".to_owned(),
            None,
            stalling,
        )
        .expect("pending generation");
        started
            .recv_timeout(Duration::from_secs(2))
            .expect("provider started");
        let pending_head = core
            .list_branch_messages(&branch_id)
            .expect("pending lineage")
            .last()
            .expect("pending assistant")
            .id
            .clone();
        let pending_error = core
            .remove_message_from_branch(
                &conversation.id,
                &branch_id,
                Some(&pending_head),
                &pending_head,
            )
            .expect_err("pending generation");
        assert_eq!(pending_error.code, CoreErrorCode::InvalidInput);
        assert!(pending_error.recoverable);
    }

    #[test]
    fn provider_snapshot_failure_leaves_generation_tables_empty() {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(&character.id, "snapshot failure", ConversationMode::Chat)
            .expect("conversation");
        let state = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state");

        let error = core
            .send_message_to_branch_with_provider_options(
                &conversation.id,
                &state.active_branch_id,
                None,
                ConversationMode::Chat,
                "must remain transient",
                new_test_generation_operation("snapshot-failure-v1"),
                "snapshot-failure".to_owned(),
                None,
                None,
                false,
                None,
                Some(128),
                None,
                None,
                false,
                Arc::new(SnapshotFailingProvider),
            )
            .expect_err("snapshot preflight must fail");
        assert_eq!(error.code, CoreErrorCode::Internal);

        let connection =
            rusqlite::Connection::open(root.path().join("db/lorepia.sqlite3")).expect("database");
        for table in [
            "messages",
            "generations",
            "generation_prompt_plans",
            "provider_request_snapshots",
            "knowledge_activation_logs",
            "generation_prompt_plan_knowledge_selections",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .expect("table count");
            assert_eq!(count, 0, "{table} must remain empty");
        }
        assert!(
            core.inner
                .storage
                .get_conversation_branch(&state.active_branch_id)
                .expect("branch")
                .head_message_id
                .is_none()
        );
    }

    #[test]
    fn identical_send_retry_with_original_head_replays_the_existing_generation() {
        let (_root, core, character) = imported_core();
        let conversation = core
            .create_conversation(&character.id, "response loss", ConversationMode::Chat)
            .expect("conversation");
        let state = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state");
        let original_head = core
            .inner
            .storage
            .get_conversation_branch(&state.active_branch_id)
            .expect("original branch")
            .head_message_id;
        let (provider, started) = StallingProvider::new("in flight");
        let first_generation = core
            .send_message_to_branch_with_provider(
                &conversation.id,
                &state.active_branch_id,
                original_head.as_ref(),
                ConversationMode::Chat,
                "exact same request",
                new_test_generation_operation("same-branch-response-loss-v1"),
                "response-loss-model".to_owned(),
                None,
                provider,
            )
            .expect("first send");
        started
            .recv_timeout(Duration::from_secs(2))
            .expect("provider started");
        let first_messages = core
            .list_branch_messages(&state.active_branch_id)
            .expect("first append");
        assert_eq!(first_messages.len(), 2);

        let replayed_generation = core
            .send_message_to_branch_with_provider(
                &conversation.id,
                &state.active_branch_id,
                original_head.as_ref(),
                ConversationMode::Chat,
                "exact same request",
                new_test_generation_operation("same-branch-response-loss-v1"),
                "response-loss-model".to_owned(),
                None,
                Arc::new(SnapshotFailingProvider),
            )
            .expect("response-loss retry");

        assert_eq!(replayed_generation, first_generation);
        assert_eq!(
            core.list_branch_messages(&state.active_branch_id)
                .expect("replayed messages"),
            first_messages,
            "an identical retry must not append or relaunch"
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one restart fixture proves lost-response replay for both immutable action kinds"
    )]
    fn message_action_retries_survive_response_loss_and_reopen_without_relaunch() {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "Action response loss",
                ConversationMode::Chat,
            )
            .expect("conversation");
        let source_model = "action-source-model";
        let source_generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "original question",
                source_model.to_owned(),
                None,
                Arc::new(StaticProvider::new("original reply")),
            )
            .expect("initial generation");
        let source_branch = core
            .get_conversation_state(&conversation.id)
            .expect("source state")
            .active_branch_id;
        wait_for_generation_status(&core, &source_generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let source_generation = core
            .list_branch_messages(&source_branch)
            .expect("source messages");
        assert_eq!(source_generation.len(), 2);
        drop(core);

        let core = Core::open(CoreConfig::new(root.path()))
            .expect("reopen after lost same-branch send response");
        let replayed_source_generation_id = core
            .send_message_to_branch_with_provider(
                &conversation.id,
                &source_branch,
                None,
                ConversationMode::Chat,
                "original question",
                new_test_generation_operation("core-direct-send-v1"),
                source_model.to_owned(),
                None,
                Arc::new(SnapshotFailingProvider),
            )
            .expect("replay send after response loss");
        assert_eq!(replayed_source_generation_id, source_generation_id);
        assert_eq!(
            core.list_branch_messages(&source_branch)
                .expect("replayed source messages"),
            source_generation
        );

        let edit_model = "action-edit-response-loss-model";
        let edited = core
            .edit_user_message_with_provider(
                &conversation.id,
                &source_branch,
                Some(&source_generation[1].id),
                &source_generation[0].id,
                "edited question",
                edit_model.to_owned(),
                None,
                Arc::new(StaticProvider::new("edited reply")),
            )
            .expect("edit generation");
        wait_for_generation_status(&core, &edited.generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let branches_after_edit = core
            .list_conversation_branches(&conversation.id)
            .expect("branches after edit");
        let messages_after_edit = core
            .list_branch_messages(&edited.branch.id)
            .expect("edited messages");
        drop(core);

        let reopened =
            Core::open(CoreConfig::new(root.path())).expect("reopen after lost edit response");
        let replayed_edit = reopened
            .edit_user_message_with_provider(
                &conversation.id,
                &source_branch,
                Some(&source_generation[1].id),
                &source_generation[0].id,
                "edited question",
                edit_model.to_owned(),
                None,
                Arc::new(SnapshotFailingProvider),
            )
            .expect("replay edit after response loss");
        assert_eq!(replayed_edit, edited);
        assert_eq!(
            reopened
                .list_conversation_branches(&conversation.id)
                .expect("replayed edit branches"),
            branches_after_edit
        );
        assert_eq!(
            reopened
                .list_branch_messages(&edited.branch.id)
                .expect("replayed edit messages"),
            messages_after_edit
        );
        let changed_edit = reopened
            .edit_user_message_with_provider(
                &conversation.id,
                &source_branch,
                Some(&source_generation[1].id),
                &source_generation[0].id,
                "different edited question",
                edit_model.to_owned(),
                None,
                Arc::new(SnapshotFailingProvider),
            )
            .expect_err("changed edit input must not replay the completed operation");
        assert_eq!(changed_edit.code, CoreErrorCode::InvalidInput);

        reopened
            .select_conversation_branch(&conversation.id, &source_branch)
            .expect("restore source for regenerate");
        let regenerate_model = "action-regenerate-response-loss-model";
        let regenerated = reopened
            .regenerate_assistant_message_with_provider(
                &conversation.id,
                &source_branch,
                Some(&source_generation[1].id),
                &source_generation[1].id,
                regenerate_model.to_owned(),
                None,
                Arc::new(StaticProvider::new("regenerated reply")),
            )
            .expect("regenerate assistant");
        wait_for_generation_status(
            &reopened,
            &regenerated.generation_id,
            GenerationStatus::Complete,
        );
        wait_for_generation_registry_to_drain(&reopened);
        let branches_after_regenerate = reopened
            .list_conversation_branches(&conversation.id)
            .expect("branches after regenerate");
        let messages_after_regenerate = reopened
            .list_branch_messages(&regenerated.branch.id)
            .expect("regenerated messages");
        drop(reopened);

        let reopened = Core::open(CoreConfig::new(root.path()))
            .expect("reopen after lost regenerate response");
        let replayed_regenerate = reopened
            .regenerate_assistant_message_with_provider(
                &conversation.id,
                &source_branch,
                Some(&source_generation[1].id),
                &source_generation[1].id,
                regenerate_model.to_owned(),
                None,
                Arc::new(SnapshotFailingProvider),
            )
            .expect("replay regenerate after response loss");
        assert_eq!(replayed_regenerate, regenerated);
        assert_eq!(
            reopened
                .list_conversation_branches(&conversation.id)
                .expect("replayed regenerate branches"),
            branches_after_regenerate
        );
        assert_eq!(
            reopened
                .list_branch_messages(&regenerated.branch.id)
                .expect("replayed regenerate messages"),
            messages_after_regenerate
        );
        let changed_regenerate = reopened
            .regenerate_assistant_message_with_provider(
                &conversation.id,
                &source_branch,
                Some(&source_generation[1].id),
                &source_generation[1].id,
                "different-regenerate-model".to_owned(),
                None,
                Arc::new(SnapshotFailingProvider),
            )
            .expect_err("changed regenerate target must not replay the completed operation");
        assert_eq!(changed_regenerate.code, CoreErrorCode::InvalidInput);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn generation_launch_preflight_prevents_failed_sends_and_actions_from_mutating_storage() {
        let (_send_root, send_core, send_character) = imported_core();
        let send_conversation = send_core
            .create_conversation(&send_character.id, "전송 preflight", ConversationMode::Chat)
            .expect("send conversation");
        let send_state = send_core
            .get_conversation_state(&send_conversation.id)
            .expect("send state");
        poison_generation_registry(&send_core);
        let send_error = send_core
            .send_message_with_provider(
                &send_conversation.id,
                "저장되면 안 됨",
                "unused".to_owned(),
                None,
                Arc::new(StaticProvider::new("unused")),
            )
            .expect_err("launch preflight must fail");
        assert_eq!(send_error.code, CoreErrorCode::Internal);
        assert!(
            send_core
                .list_messages(&send_conversation.id)
                .expect("send messages")
                .is_empty()
        );
        assert!(
            send_core
                .inner
                .storage
                .get_conversation_branch(&send_state.active_branch_id)
                .expect("send branch")
                .head_message_id
                .is_none()
        );

        let (_action_root, action_core, action_character) = imported_core();
        let action_conversation = action_core
            .create_conversation(
                &action_character.id,
                "액션 preflight",
                ConversationMode::Chat,
            )
            .expect("action conversation");
        action_core
            .send_message_with_provider(
                &action_conversation.id,
                "원본",
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("답변")),
            )
            .expect("initial generation");
        let deadline = Instant::now() + Duration::from_secs(2);
        let original = loop {
            let messages = action_core
                .list_messages(&action_conversation.id)
                .expect("action messages");
            if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
                break messages;
            }
            assert!(Instant::now() < deadline, "initial generation timed out");
            thread::sleep(Duration::from_millis(10));
        };
        let action_state = action_core
            .get_conversation_state(&action_conversation.id)
            .expect("action state");
        let branch_count = action_core
            .list_conversation_branches(&action_conversation.id)
            .expect("action branches")
            .len();
        let message_count = action_core.database_stats().expect("action stats").messages;
        poison_generation_registry(&action_core);
        let action_error = action_core
            .edit_user_message_with_provider(
                &action_conversation.id,
                &action_state.active_branch_id,
                Some(&original[1].id),
                &original[0].id,
                "수정본",
                "unused".to_owned(),
                None,
                Arc::new(StaticProvider::new("unused")),
            )
            .expect_err("action launch preflight must fail");
        assert_eq!(action_error.code, CoreErrorCode::Internal);
        assert_eq!(
            action_core
                .get_conversation_state(&action_conversation.id)
                .expect("unchanged action state")
                .active_branch_id,
            action_state.active_branch_id
        );
        assert_eq!(
            action_core
                .list_conversation_branches(&action_conversation.id)
                .expect("unchanged action branches")
                .len(),
            branch_count
        );
        assert_eq!(
            action_core
                .database_stats()
                .expect("unchanged stats")
                .messages,
            message_count
        );
        assert_eq!(
            action_core
                .list_messages(&action_conversation.id)
                .expect("unchanged action messages"),
            original
        );
    }

    #[test]
    fn regenerate_revalidates_copied_user_text_before_creating_a_branch() {
        let (_root, core, character) = imported_core();
        for (index, invalid_text) in ["   ".to_owned(), "x".repeat(MAX_USER_MESSAGE_BYTES + 1)]
            .into_iter()
            .enumerate()
        {
            let conversation = core
                .create_conversation(
                    &character.id,
                    format!("비정상 원본 {index}"),
                    ConversationMode::Chat,
                )
                .expect("conversation");
            let state = core
                .get_conversation_state(&conversation.id)
                .expect("state");
            let user = Message::user(conversation.id.clone(), invalid_text);
            let generation_id = GenerationId::new();
            let pending = Message::pending_assistant(
                conversation.id.clone(),
                user.id.clone(),
                generation_id.clone(),
            );
            let generation = GenerationRecord {
                id: generation_id,
                conversation_id: conversation.id.clone(),
                branch_id: state.active_branch_id.clone(),
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
            core.inner
                .storage
                .append_generation(&state.active_branch_id, None, &user, &pending, &generation)
                .expect("append abnormal legacy generation");
            let mut assistant = pending;
            assistant.content = "legacy response".to_owned();
            assistant.status = MessageStatus::Complete;
            core.inner
                .storage
                .finalize_generation(&assistant, None, None, true)
                .expect("finalize abnormal legacy generation");

            let branches_before = core
                .list_conversation_branches(&conversation.id)
                .expect("branches before");
            let messages_before = core
                .list_messages(&conversation.id)
                .expect("messages before");
            let error = core
                .regenerate_assistant_message_with_provider(
                    &conversation.id,
                    &state.active_branch_id,
                    Some(&assistant.id),
                    &assistant.id,
                    "unused".to_owned(),
                    None,
                    Arc::new(StaticProvider::new("unused")),
                )
                .expect_err("invalid copied user text");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert_eq!(
                core.list_conversation_branches(&conversation.id)
                    .expect("unchanged branches"),
                branches_before
            );
            assert_eq!(
                core.list_messages(&conversation.id)
                    .expect("unchanged messages"),
                messages_before
            );
        }
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one operating-path test keeps the async send and immutable action sequence visible"
    )]
    async fn async_generation_operating_paths_send_edit_and_regenerate() {
        let (_root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "Async generation paths",
                ConversationMode::Chat,
            )
            .expect("conversation");
        let source_branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let broker = RejectingTaskCredentialBroker;

        let (send_provider, send_prompt) = CapturingProvider::new("async send reply");
        let send_temporal_context =
            direct_model_temporal_context("async-send-model").expect("direct async send authority");
        let generation_id = core
            .send_message_to_branch_with_provider_options_and_contract_async(
                &conversation.id,
                &source_branch,
                None,
                ConversationMode::Chat,
                "async send question",
                new_test_generation_operation("async-send-v1"),
                "async-send-model".to_owned(),
                None,
                None,
                false,
                Some(1.0),
                Some(CORE_MAX_OUTPUT_TOKENS),
                &VariableMap::default(),
                None,
                None,
                false,
                None,
                send_provider,
                None,
                send_temporal_context,
                &broker,
                watch::channel(false).1,
            )
            .await
            .expect("async send");
        let send_request = send_prompt
            .recv_timeout(Duration::from_secs(2))
            .expect("captured async send prompt");
        assert!(
            send_request
                .iter()
                .any(|message| message == "async send question")
        );
        wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let original = core
            .list_branch_messages(&source_branch)
            .expect("source messages");
        assert_eq!(original.len(), 2);
        assert_eq!(original[1].content, "async send reply");

        let edit_model = "async-edit-model";
        let (edit_provider, edit_prompt) = CapturingProvider::new("async edit reply");
        let edited = core
            .start_message_generation_action_with_provider_async(
                &conversation.id,
                &source_branch,
                Some(&original[1].id),
                &original[0].id,
                MessageGenerationAction::EditUser,
                Some("async edited question"),
                new_test_generation_operation("async-edit-v1"),
                GenerationActionTargetIdentity::DirectModel {
                    model_sha256: format!("{:x}", Sha256::digest(edit_model.as_bytes())),
                },
                edit_model.to_owned(),
                None,
                edit_provider,
                &broker,
                watch::channel(false).1,
            )
            .await
            .expect("async edit");
        let edit_request = edit_prompt
            .recv_timeout(Duration::from_secs(2))
            .expect("captured async edit prompt");
        assert!(
            edit_request
                .iter()
                .any(|message| message == "async edited question")
        );
        assert!(
            !edit_request
                .iter()
                .any(|message| message == "async send question")
        );
        wait_for_generation_status(&core, &edited.generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let edited_messages = core
            .list_branch_messages(&edited.branch.id)
            .expect("edited branch");
        assert_eq!(
            edited_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["async edited question", "async edit reply"]
        );

        core.select_conversation_branch(&conversation.id, &source_branch)
            .expect("restore source branch");
        let regenerate_model = "async-regenerate-model";
        let (regenerate_provider, regenerate_prompt) =
            CapturingProvider::new("async regenerated reply");
        let regenerated = core
            .start_message_generation_action_with_provider_async(
                &conversation.id,
                &source_branch,
                Some(&original[1].id),
                &original[1].id,
                MessageGenerationAction::RegenerateAssistant,
                None,
                new_test_generation_operation("async-regenerate-v1"),
                GenerationActionTargetIdentity::DirectModel {
                    model_sha256: format!("{:x}", Sha256::digest(regenerate_model.as_bytes())),
                },
                regenerate_model.to_owned(),
                None,
                regenerate_provider,
                &broker,
                watch::channel(false).1,
            )
            .await
            .expect("async regenerate");
        let regenerate_request = regenerate_prompt
            .recv_timeout(Duration::from_secs(2))
            .expect("captured async regenerate prompt");
        assert!(
            regenerate_request
                .iter()
                .any(|message| message == "async send question")
        );
        assert!(
            !regenerate_request
                .iter()
                .any(|message| message == "async send reply")
        );
        wait_for_generation_status(
            &core,
            &regenerated.generation_id,
            GenerationStatus::Complete,
        );
        wait_for_generation_registry_to_drain(&core);
        let regenerated_messages = core
            .list_branch_messages(&regenerated.branch.id)
            .expect("regenerated branch");
        assert_eq!(
            regenerated_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            ["async send question", "async regenerated reply"]
        );
        assert_eq!(
            core.list_branch_messages(&source_branch)
                .expect("preserved source branch"),
            original
        );
    }

    #[tokio::test]
    async fn async_prompt_preview_resolves_without_chat_mutation() {
        let (_root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "Async prompt preview",
                ConversationMode::Chat,
            )
            .expect("conversation");
        let branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let (template, route) = create_built_in_public_route(
            &core,
            "openai-responses-v1",
            "/v1",
            "gpt-async-preview-fixture",
        );
        let preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
            .expect("generation preset");
        let target = GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: preset.id,
        };
        let preview = core
            .resolve_prompt_preview_async(
                &crate::PromptPlanRequest {
                    conversation_id: conversation.id.clone(),
                    branch_id: branch.clone(),
                    expected_head: None,
                    user_text: "async preview question".to_owned(),
                    generation_target: target.clone(),
                    prompt_preset_id: None,
                    variable_overrides: VariableMap::default(),
                    expected_plan_hash: None,
                },
                new_test_generation_operation("async-preview-v1"),
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("async prompt preview");

        assert_eq!(preview.plan.generation_target.as_ref(), Some(&target));
        assert!(
            preview
                .effective_messages
                .iter()
                .any(|message| message.content == "async preview question")
        );
        assert!(
            core.list_branch_messages(&branch)
                .expect("preview must remain read-only")
                .is_empty()
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "one vertical proves approval, temporal replay, variable/knowledge resolution, and atomic materialization"
    )]
    fn expert_preview_reuses_attempt_owned_before_generation_for_reviewed_send() {
        let (_root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "Attempt-owned expert preview",
                ConversationMode::Chat,
            )
            .expect("create attempt-owned preview conversation");
        let branch_id = core
            .get_conversation_state(&conversation.id)
            .expect("load attempt-owned preview room state")
            .active_branch_id;
        let (variable, temporal_roll, knowledge_entry_id, proposal_id) =
            install_prompt_attempt_parity_module(
                &core,
                ContentModuleRuntimeTarget {
                    conversation_id: conversation.id.clone(),
                    branch_id: branch_id.clone(),
                },
            );

        let api_origin =
            CanonicalOrigin::parse("http://127.0.0.1:9").expect("synthetic loopback origin");
        let (template, connection) = create_openai_chat_connection(&core, &api_origin);
        let connection_id = connection.id.clone();
        let now = Utc::now();
        let route = core
            .upsert_model_route(ModelRoute {
                id: ModelRouteId::from("synthetic-prompt-attempt-route"),
                connection_id: connection.id,
                api_family: template.api_family,
                model_id: "synthetic-prompt-attempt-model".to_owned(),
                display_name: Some("Synthetic prompt-attempt model".to_owned()),
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
            })
            .expect("save prompt-attempt route");
        let generation_preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
            .expect("save prompt-attempt generation preset");
        let idle_branch_updated_at = core
            .inner
            .storage
            .get_conversation_branch(&branch_id)
            .expect("load idle branch before review")
            .updated_at;
        thread::sleep(Duration::from_millis(1_100));
        let request = crate::PromptPlanRequest {
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            expected_head: None,
            user_text: "Synthetic attempt-owned preview request".to_owned(),
            generation_target: GenerationTarget {
                model_route_id: route.id,
                generation_preset_id: generation_preset.id,
            },
            prompt_preset_id: None,
            variable_overrides: VariableMap::default(),
            expected_plan_hash: None,
        };

        let awaiting = core
            .resolve_prompt_preview(
                &request,
                new_test_generation_operation("attempt-owned-preview-v1"),
            )
            .expect_err("attempt-owned approval must block the first final preview");
        assert_eq!(
            awaiting.code,
            CoreErrorCode::PermissionDenied,
            "unexpected first-preview failure: {awaiting:?}"
        );
        assert!(
            core.list_branch_messages(&branch_id)
                .expect("messages before attempt approval")
                .is_empty(),
            "preview must not append chat rows"
        );
        assert!(
            core.list_interaction_effect_history(&conversation.id, &branch_id, None, 100)
                .expect("live effects before attempt approval")
                .is_empty(),
            "attempt-owned BeforeGeneration effects must remain isolated"
        );

        let proposals = core
            .list_generation_attempt_proposals_for_source_room(
                &conversation.id,
                &branch_id,
                InteractionProposalStatus::Pending,
                10,
            )
            .expect("list attempt-owned proposals");
        let [proposal] = proposals.as_slice() else {
            panic!("expected one attempt-owned proposal, got {proposals:?}");
        };
        assert_eq!(proposal.proposal.record.proposal_id, proposal_id);
        let generation_id = proposal.proposal.generation_id.clone();
        let attempt_created_at = core
            .inner
            .storage
            .get_generation_attempt(&generation_id)
            .expect("load isolated prompt attempt")
            .created_at;
        assert!(attempt_created_at > idle_branch_updated_at);
        let event_time_text = proposal.proposal.record.body.clone();
        assert!(
            event_time_text.ends_with("+00:00"),
            "approval body must retain the attempt's explicit UTC event time"
        );

        let live_before_send = core
            .inner
            .storage
            .get_interaction_state_snapshot(&conversation.id, &branch_id)
            .expect("live interaction state before attempt decision");
        assert_eq!(
            live_before_send.state.variables.get(&variable),
            Some(&VariableValue::Text("initial".to_owned()))
        );
        assert_eq!(
            live_before_send.state.variables.get(&temporal_roll),
            Some(&VariableValue::Integer(0))
        );
        assert!(
            !live_before_send
                .state
                .manually_active_knowledge
                .contains(&knowledge_entry_id)
        );

        let decision = core
            .decide_generation_attempt_proposal(
                &crate::orchestration_runtime::GenerationAttemptProposalDecisionRequest {
                    conversation_id: conversation.id.clone(),
                    source_branch_id: branch_id.clone(),
                    generation_id: generation_id.clone(),
                    proposal_record_id: proposal.proposal.record.id.clone(),
                    expected_aggregate_revision: proposal.aggregate_revision,
                    expected_proposal_revision: proposal.proposal.proposal_revision,
                    decision: InteractionProposalDecision::Approve,
                },
            )
            .expect("approve attempt-owned proposal");
        assert_eq!(decision.pending_proposal_count, 0);
        let approved_aggregate = core
            .inner
            .storage
            .get_generation_attempt_interaction_aggregate(&generation_id)
            .expect("load approved attempt-owned interaction aggregate");
        assert!(
            approved_aggregate
                .state
                .manually_active_knowledge
                .contains(&knowledge_entry_id),
            "approved attempt aggregate must retain manual knowledge activation"
        );
        assert!(
            core.list_interaction_effect_history(&conversation.id, &branch_id, None, 100)
                .expect("live effects after isolated approval")
                .is_empty(),
            "approval must still leave live state untouched before append"
        );

        let preview = core
            .resolve_prompt_preview(
                &request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &generation_id,
                },
            )
            .expect("resolve final attempt-owned expert preview");
        assert_eq!(preview.generation_attempt_id, generation_id);
        assert_eq!(
            preview,
            core.resolve_prompt_preview(
                &request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &generation_id,
                },
            )
            .expect("repeat exact attempt-owned expert preview"),
            "re-preview must reuse the same temporal interaction aggregate"
        );
        let preview_text = preview
            .effective_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(preview_text.contains("SYNTHETIC_ATTEMPT_VARIABLE=approved-for-prompt"));
        assert!(
            preview_text.contains("SYNTHETIC_ATTEMPT_MANUAL_KNOWLEDGE_6A91"),
            "expert preview omitted attempt-owned knowledge:\n{preview_text}"
        );
        assert!(preview_text.contains(&format!(
            "SYNTHETIC_ATTEMPT_DATE={}",
            attempt_created_at.format("%Y-%m-%d")
        )));
        assert!(preview_text.contains(&format!(
            "SYNTHETIC_ATTEMPT_TIME={}",
            attempt_created_at.format("%H:%M:%S%:z")
        )));
        assert!(!preview_text.contains(&format!(
            "SYNTHETIC_ATTEMPT_TIME={}",
            idle_branch_updated_at.format("%H:%M:%S%:z")
        )));
        let temporal_roll_value = preview_text
            .split("SYNTHETIC_ATTEMPT_TIME_ROLL=")
            .nth(1)
            .and_then(|suffix| suffix.lines().next())
            .and_then(|line| line.split(';').next())
            .and_then(|value| value.trim().parse::<i64>().ok())
            .expect("expert preview contains the attempt-time-seeded roll");
        assert!((1..=10_000).contains(&temporal_roll_value));

        let mut reviewed = request;
        reviewed.expected_plan_hash = Some(preview.plan.plan_hash.clone());
        let credential_authority = install_provider_credential_authority(&core, &connection_id);
        let stale_attempt = core
            .send_message_with_prompt_plan(
                &reviewed,
                &GenerationId::new(),
                ConnectionBoundCredential::new_with_access_authority(
                    connection_id.clone(),
                    Some("synthetic-attempt-credential".to_owned()),
                    credential_authority.clone(),
                ),
            )
            .expect_err("a reviewed send cannot substitute another attempt token");
        assert_eq!(stale_attempt.code, CoreErrorCode::InvalidInput);
        assert!(
            core.list_branch_messages(&branch_id)
                .expect("messages after stale attempt rejection")
                .is_empty()
        );
        let dispatched_generation_id = core
            .send_message_with_prompt_plan(
                &reviewed,
                &preview.generation_attempt_id,
                ConnectionBoundCredential::new_with_access_authority(
                    connection_id,
                    Some("synthetic-attempt-credential".to_owned()),
                    credential_authority,
                ),
            )
            .expect("send exact attempt-owned expert preview");
        assert_eq!(dispatched_generation_id, generation_id);

        let stored_plan = core
            .get_generation_prompt_plan(&dispatched_generation_id)
            .expect("load attempt-owned generation prompt plan");
        let stored_attempt = core
            .inner
            .storage
            .get_generation_attempt(&generation_id)
            .expect("load attempt-owned semantic fingerprint");
        assert_eq!(stored_plan.id, preview.plan.plan_id);
        assert_eq!(stored_plan.plan_sha256, preview.plan.neutral_plan_hash);
        assert_eq!(
            stored_plan.random_seed,
            Some(reviewed_prompt_session_seed(
                &stored_attempt.input.base_request_fingerprint_sha256,
            ))
        );
        assert_eq!(
            stored_plan.provider_request.request.value,
            preview.provider_request
        );
        let resolved: ResolvedPromptPlan = serde_json::from_value(stored_plan.plan.value)
            .expect("decode stored attempt-owned resolved plan");
        let stored_messages = resolved
            .effective_messages
            .iter()
            .map(|message| {
                (
                    message.sequence,
                    message.block_id.clone(),
                    message.content.clone(),
                )
            })
            .collect::<Vec<_>>();
        let preview_messages = preview
            .effective_messages
            .iter()
            .map(|message| {
                (
                    message.sequence,
                    message.block_id.clone(),
                    message.content.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(stored_messages, preview_messages);

        let live_after_send = core
            .inner
            .storage
            .get_interaction_state_snapshot(&conversation.id, &branch_id)
            .expect("live interaction state after atomic append");
        assert_eq!(
            live_after_send.state.variables.get(&variable),
            Some(&VariableValue::Text("approved-for-prompt".to_owned()))
        );
        assert_eq!(
            live_after_send.state.variables.get(&temporal_roll),
            Some(&VariableValue::Integer(temporal_roll_value))
        );
        assert!(
            live_after_send
                .state
                .manually_active_knowledge
                .contains(&knowledge_entry_id)
        );
        let visible_times = core
            .list_interaction_effect_history(&conversation.id, &branch_id, None, 100)
            .expect("materialized attempt-owned effects")
            .into_iter()
            .filter_map(|history| match history.stored.effect {
                InteractionEffect::VisibleSystemEvent { text } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(visible_times, vec![event_time_text]);
    }

    #[test]
    fn provider_output_limit_failure_obeys_the_partial_persistence_policy() {
        let conversation_id = ConversationId::new();
        let parent_id = lorepia_domain::MessageId::new();
        let generation_id = GenerationId::new();
        let failure = GenerationFailure {
            error: CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                lorepia_chat::OUTPUT_LIMIT_ERROR_MESSAGE,
                false,
            ),
            partial_text: "safe prefix 😀".to_owned(),
            last_sequence: 7,
        };

        let mut preserved = Message::pending_assistant(
            conversation_id.clone(),
            parent_id.clone(),
            generation_id.clone(),
        );
        let (sequence, terminal, should_commit) =
            apply_generation_result(&mut preserved, Err(failure.clone()), true);
        assert_eq!(sequence, 8);
        assert_eq!(preserved.status, MessageStatus::Failed);
        assert_eq!(preserved.content, "safe prefix 😀");
        assert!(should_commit);
        assert!(matches!(
            terminal,
            ChatEventKind::GenerationFailed { code, message }
                if code == "provider_unavailable"
                    && message == lorepia_chat::OUTPUT_LIMIT_ERROR_MESSAGE
        ));

        let mut discarded = Message::pending_assistant(conversation_id, parent_id, generation_id);
        let (_, _, should_commit) = apply_generation_result(&mut discarded, Err(failure), false);
        assert!(!should_commit);
    }

    #[test]
    fn static_provider_persists_assistant_message() {
        let (root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let mut events = core.subscribe_events();
        let generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "Hello",
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("Hi there")),
            )
            .expect("send");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let messages = core.list_messages(&conversation.id).expect("messages");
            if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
                assert_eq!(messages[1].content, "Hi there");
                break;
            }
            assert!(Instant::now() < deadline, "generation timed out");
            thread::sleep(Duration::from_millis(10));
        }

        wait_for_generation_registry_to_drain(&core);
        let events = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let committed = events
            .iter()
            .position(|event| matches!(event.kind, ChatEventKind::MessageCommitted { .. }))
            .expect("message committed event");
        let finished = events
            .iter()
            .position(|event| matches!(event.kind, ChatEventKind::GenerationFinished))
            .expect("generation finished event");
        assert!(committed < finished);
        assert!(events.windows(2).all(|events| {
            events[0].generation_id != events[1].generation_id
                || events[0].sequence < events[1].sequence
        }));
        let state = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state");
        let generation = core
            .inner
            .storage
            .get_generation(&generation_id)
            .expect("generation snapshot");
        assert_eq!(generation.mode, ConversationMode::Chat);
        assert!(events.iter().all(|event| {
            event.branch_id.as_ref() == Some(&state.active_branch_id)
                && event.assistant_message_id.as_ref() == generation.assistant_message_id.as_ref()
        }));

        drop(core);
        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
        let restored = reopened
            .list_messages(&conversation.id)
            .expect("restored messages");
        assert_eq!(restored.len(), 2);
        assert_eq!(restored[1].content, "Hi there");
    }

    #[test]
    fn display_only_terminal_stream_matches_hash_verified_reopen_projection() {
        let (root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let transform_set = display_only_generation_transform_set();
        let transform_revision_id = install_generation_transform_fixture(
            &core,
            &conversation.id,
            &transform_set,
            "synthetic.display-only.preset",
            "synthetic.display-only.binding",
        );

        let mut events = core.subscribe_events();
        let generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "Render the synthetic projection",
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("Synthetic reply")),
            )
            .expect("start DisplayOnly generation");
        wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let events = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let streamed_display = assert_display_only_events(&events, &generation_id);
        let canonical = core
            .list_messages(&conversation.id)
            .expect("canonical messages");
        assert_eq!(canonical[1].content, "Synthetic reply");
        let projected = core
            .list_message_presentations(&conversation.id)
            .expect("projected messages");
        assert_display_only_projection(&projected[1], &transform_revision_id, &streamed_display);

        drop(core);
        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
        let canonical_after_reopen = reopened
            .list_messages(&conversation.id)
            .expect("reopened canonical messages");
        assert_eq!(canonical_after_reopen[1].content, "Synthetic reply");
        let projected_after_reopen = reopened
            .list_message_presentations(&conversation.id)
            .expect("reopened projected messages");
        assert_eq!(projected_after_reopen[1].display_content, streamed_display);
        assert_eq!(
            projected_after_reopen[1].display_content_sha256,
            transform_content_sha256(&streamed_display)
        );
        assert_eq!(
            projected_after_reopen[1].projection_diagnostics_sha256,
            projected[1].projection_diagnostics_sha256
        );
        assert_eq!(
            projected_after_reopen[1].transform_diagnostics,
            projected[1].transform_diagnostics
        );
    }

    #[test]
    fn generation_transform_failures_preserve_provider_text_and_reopen_diagnostics() {
        const PROVIDER_TEXT: &str = "Synthetic reply";
        let (root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let transform_set = fail_open_generation_transform_set();
        install_generation_transform_fixture(
            &core,
            &conversation.id,
            &transform_set,
            "synthetic.fail-open.preset",
            "synthetic.fail-open.binding",
        );

        let mut events = core.subscribe_events();
        let generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "Exercise transform failure",
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new(PROVIDER_TEXT)),
            )
            .expect("start fail-open generation");
        wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let generation_events = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| event.generation_id == generation_id)
            .collect::<Vec<_>>();
        assert!(generation_events.iter().any(|event| {
            matches!(&event.kind, ChatEventKind::TextDelta(text) if text == PROVIDER_TEXT)
        }));
        assert!(
            generation_events
                .iter()
                .any(|event| matches!(event.kind, ChatEventKind::GenerationFinished))
        );

        let canonical = core
            .list_messages(&conversation.id)
            .expect("canonical messages");
        assert_eq!(canonical[1].content, PROVIDER_TEXT);
        assert_eq!(canonical[1].status, MessageStatus::Complete);
        let projected = core
            .list_message_presentations(&conversation.id)
            .expect("fail-open projection");
        assert_eq!(projected[1].display_content, PROVIDER_TEXT);
        assert!(projected[1].projection_diagnostics_sha256.is_some());
        assert_eq!(projected[1].transform_diagnostics.len(), 2);
        let invalid = projected[1]
            .transform_diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.rule_id.as_deref() == Some("synthetic.fail-open.invalid-regex")
            })
            .expect("invalid-regex diagnostic");
        assert_eq!(invalid.disposition, MessageTransformDisposition::Failed);
        assert_eq!(invalid.code.as_deref(), Some("invalid_regex"));
        let limited = projected[1]
            .transform_diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.rule_id.as_deref() == Some("synthetic.fail-open.output-limit")
            })
            .expect("output-limit diagnostic");
        assert_eq!(
            limited.disposition,
            MessageTransformDisposition::LimitRejected
        );
        assert_eq!(limited.code.as_deref(), Some("output_limit_exceeded"));
        assert!(projected[1].transform_diagnostics.iter().all(
            |diagnostic| diagnostic.before_sha256 == transform_content_sha256(PROVIDER_TEXT)
                && diagnostic.after_sha256.is_none()
        ));

        drop(core);
        let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen fail-open core");
        let canonical_after_reopen = reopened
            .list_messages(&conversation.id)
            .expect("reopened canonical messages");
        assert_eq!(canonical_after_reopen[1].content, PROVIDER_TEXT);
        assert_eq!(
            reopened
                .list_message_presentations(&conversation.id)
                .expect("reopened fail-open projection")[1],
            projected[1]
        );
    }

    #[test]
    fn prompt_preview_materializes_exact_current_room_sources_and_content_free_snapshot() {
        let (_root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let branch_id = core
            .get_conversation_state(&conversation.id)
            .expect("prompt-source room state")
            .active_branch_id;
        let first = core
            .send_message_with_provider(
                &conversation.id,
                "SUMMARY_RANGE_USER_CANARY_31A7",
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("SUMMARY_RANGE_ASSISTANT_CANARY_72D4")),
            )
            .expect("start summary-range generation");
        wait_for_generation_status(&core, &first, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let first_turn = core
            .list_branch_messages(&branch_id)
            .expect("summary-range messages");
        let summary = save_prompt_source_summary(&core, &branch_id, &first_turn);
        let second = core
            .send_message_with_provider(
                &conversation.id,
                "SINCE_SUMMARY_USER_CANARY_54C9",
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("SINCE_SUMMARY_ASSISTANT_CANARY_86E2")),
            )
            .expect("start since-summary generation");
        wait_for_generation_status(&core, &second, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let messages = core
            .list_branch_messages(&branch_id)
            .expect("complete prompt-source history");
        let binding =
            bind_prompt_source_test_preset(&core, &conversation.id, &branch_id, &summary.value.id);

        let (template, route) =
            create_built_in_public_route(&core, "openai-responses-v1", "/v1", "source-fixture");
        let generation_preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
            .expect("save prompt-source generation preset");
        let request = crate::PromptPlanRequest {
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            expected_head: Some(messages[3].id.clone()),
            user_text: "LATEST_USER_SOURCE_CANARY_03F8".to_owned(),
            generation_target: GenerationTarget {
                model_route_id: route.id,
                generation_preset_id: generation_preset.id,
            },
            prompt_preset_id: None,
            variable_overrides: VariableMap::default(),
            expected_plan_hash: None,
        };
        let preview = core
            .resolve_prompt_preview(
                &request,
                new_test_generation_operation("prompt-source-preview-v1"),
            )
            .expect("resolve exact prompt-source preview");
        assert_prompt_source_preview(&preview);

        let trace = core
            .explain_prompt_plan(
                &request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &preview.generation_attempt_id,
                },
                &preview.plan.plan_hash,
            )
            .expect("explain exact prompt-source plan");
        let snapshot = trace
            .context_snapshot
            .expect("typed prompt context snapshot");
        assert_prompt_source_snapshot(
            &snapshot,
            &conversation.id,
            &branch_id,
            &messages,
            &summary,
            &binding,
        );
    }

    struct SemanticReplayFixture {
        root: tempfile::TempDir,
        core: Core,
        connection_id: ProviderConnectionId,
        book: KnowledgeBook,
        book_revision: u64,
        prompt_preset: PromptPreset,
        prompt_preset_revision: u64,
        request: crate::PromptPlanRequest,
    }

    fn semantic_replay_probability_sample(
        book_id: &KnowledgeBookId,
        entry_id: &KnowledgeEntryId,
        seed: u64,
    ) -> u16 {
        let seed_bytes = seed.to_be_bytes();
        let mut hasher = Sha256::new();
        for value in [
            b"lorepia-knowledge-probability-v1".as_slice(),
            book_id.as_str().as_bytes(),
            entry_id.as_str().as_bytes(),
            seed_bytes.as_slice(),
        ] {
            hasher.update(
                u64::try_from(value.len())
                    .expect("field length fits u64")
                    .to_be_bytes(),
            );
            hasher.update(value);
        }
        let digest = hasher.finalize();
        u16::from_be_bytes([digest[0], digest[1]]) % 10_000
    }

    fn semantic_replay_entry(
        book_id: &KnowledgeBookId,
        id: KnowledgeEntryId,
        name: &str,
        content: &str,
        activation: ActivationRule,
        probability: u16,
    ) -> KnowledgeEntry {
        KnowledgeEntry {
            id,
            book_id: book_id.clone(),
            name: name.to_owned(),
            content: content.to_owned(),
            enabled: true,
            activation,
            priority: 100,
            importance: 100,
            placement: KnowledgePlacement::RetrievedContext,
            token_policy: TokenPolicy {
                priority: 100,
                min_tokens: None,
                max_tokens: Some(64),
                reserve_tokens: None,
            },
            parent_id: None,
            activation_probability_basis_points: probability,
            provenance: prompt_attempt_test_provenance("synthetic.semantic-replay.entry"),
        }
    }

    fn create_semantic_replay_book(core: &Core) -> (KnowledgeBook, u64) {
        let book_id = KnowledgeBookId::from("synthetic.semantic-replay.book");
        let semantic_entry_id = KnowledgeEntryId::from("synthetic.semantic-replay.cobalt-moon");
        let book = KnowledgeBook {
            id: book_id.clone(),
            name: "Synthetic semantic replay knowledge".to_owned(),
            schema_version: 1,
            entries: vec![semantic_replay_entry(
                &book_id,
                semantic_entry_id,
                "Cobalt moon",
                "SYNTHETIC_SEMANTIC_COBALT_MOON_41B7 cobalt moon",
                ActivationRule::Semantic {
                    threshold: 0.1,
                    top_k: 8,
                },
                10_000,
            )],
            scan_depth: 8,
            token_budget: TokenBudget { max_tokens: 512 },
            recursive: false,
            max_recursion_depth: 0,
            provenance: prompt_attempt_test_provenance("synthetic.semantic-replay.book"),
        };
        let stored = core
            .upsert_knowledge_book(&book, None)
            .expect("save initial semantic replay book");
        (book, stored.revision)
    }

    fn semantic_replay_knowledge_block() -> PromptBlock {
        PromptBlock {
            id: PromptBlockId::from("synthetic.semantic-replay.knowledge-block"),
            name: "Synthetic semantic knowledge".to_owned(),
            kind: PromptBlockKind::WorldKnowledge,
            enabled: true,
            role_hint: RoleHint::System,
            authority: InstructionAuthority::Creator,
            template: None,
            condition: None,
            source: BlockSource::SelectedKnowledge,
            placement_zone: PlacementZone::RetrievedContext,
            history_selector: None,
            token_policy: TokenPolicy {
                priority: 1_000,
                min_tokens: None,
                max_tokens: Some(512),
                reserve_tokens: None,
            },
            overflow_policy: OverflowPolicy::ReduceKnowledgeEntries,
            merge_policy: MergePolicy::SeparateMessage,
            provenance: prompt_attempt_test_provenance("synthetic.semantic-replay.knowledge-block"),
        }
    }

    fn create_semantic_replay_prompt_preset(
        core: &Core,
        book_id: &KnowledgeBookId,
        now: DateTime<Utc>,
    ) -> (PromptPreset, u64) {
        let mut preset = lorepia_orchestration::default_prompt_preset(
            lorepia_domain::PromptPresetId::from("synthetic.semantic-replay.preset"),
            "Synthetic semantic replay preset",
            PresetMetadata {
                description: "Synthetic semantic/probability replay fixture".to_owned(),
                tags: vec!["synthetic".to_owned()],
                provenance: prompt_attempt_test_provenance("synthetic.semantic-replay.preset"),
                created_at: now,
                updated_at: now,
                local_override_of: None,
            },
        );
        for block in &mut preset.blocks {
            block.provenance = prompt_attempt_test_provenance(block.id.as_str());
        }
        preset.blocks.push(semantic_replay_knowledge_block());
        preset.blocks.sort_by_key(|block| block.placement_zone);
        preset.knowledge_book_ids.push(book_id.clone());
        let stored = core
            .upsert_prompt_preset(&preset, None)
            .expect("save initial semantic replay preset");
        (preset, stored.revision)
    }

    fn bind_semantic_replay_prompt_preset(
        core: &Core,
        conversation_id: &ConversationId,
        prompt_preset_id: &lorepia_domain::PromptPresetId,
        now: DateTime<Utc>,
    ) {
        core.bind_prompt_preset(
            &PromptPresetBinding {
                id: "synthetic.semantic-replay.binding".to_owned(),
                prompt_preset_id: prompt_preset_id.clone(),
                scope: ModuleScope::Conversation,
                target_id: Some(conversation_id.0.clone()),
                conversation_id: None,
                pinned_revision_id: None,
                priority: 0,
                enabled: true,
                response_length: PromptResponseLength::Balanced,
                creativity: 50,
                reasoning_effort: None,
                memory_enabled: true,
                knowledge_enabled: true,
                variable_overrides: VariableMap::default(),
                generation_preset_override_id: None,
                user_name_override: None,
                author_note: None,
                group_context: None,
                template_slots: Vec::new(),
                created_at: now,
                updated_at: now,
            },
            None,
        )
        .expect("bind semantic replay preset");
    }

    fn create_semantic_replay_fixture() -> SemanticReplayFixture {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "Semantic attempt replay",
                ConversationMode::Chat,
            )
            .expect("create semantic replay conversation");
        let branch_id = core
            .get_conversation_state(&conversation.id)
            .expect("semantic replay state")
            .active_branch_id;
        let (template, route) = create_built_in_public_route(
            &core,
            "openai-responses-v1",
            "/v1",
            "gpt-semantic-replay-fixture",
        );
        let generation_preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
            .expect("save semantic replay generation preset");
        let (book, book_revision) = create_semantic_replay_book(&core);
        let now = Utc::now();
        let (prompt_preset, prompt_preset_revision) =
            create_semantic_replay_prompt_preset(&core, &book.id, now);
        bind_semantic_replay_prompt_preset(&core, &conversation.id, &prompt_preset.id, now);
        let request = crate::PromptPlanRequest {
            conversation_id: conversation.id,
            branch_id,
            expected_head: None,
            user_text: "Tell me about the cobalt moon".to_owned(),
            generation_target: GenerationTarget {
                model_route_id: route.id,
                generation_preset_id: generation_preset.id,
            },
            prompt_preset_id: Some(prompt_preset.id.clone()),
            variable_overrides: VariableMap::default(),
            expected_plan_hash: None,
        };
        SemanticReplayFixture {
            root,
            core,
            connection_id: route.connection_id,
            book,
            book_revision,
            prompt_preset,
            prompt_preset_revision,
            request,
        }
    }

    fn add_semantic_replay_probability_entry(
        fixture: &mut SemanticReplayFixture,
        session_seed: u64,
    ) -> bool {
        let probabilistic_entry_id = (0_u32..100_000)
            .map(|index| KnowledgeEntryId::from(format!("synthetic.semantic-replay.roll-{index}")))
            .find(|entry_id| {
                (semantic_replay_probability_sample(&fixture.book.id, entry_id, session_seed)
                    < 5_000)
                    != (semantic_replay_probability_sample(&fixture.book.id, entry_id, 0) < 5_000)
            })
            .expect("find entry distinguished from the legacy zero seed");
        let expected_probability_selection = semantic_replay_probability_sample(
            &fixture.book.id,
            &probabilistic_entry_id,
            session_seed,
        ) < 5_000;
        assert_ne!(
            expected_probability_selection,
            semantic_replay_probability_sample(&fixture.book.id, &probabilistic_entry_id, 0)
                < 5_000,
        );
        let entry = semantic_replay_entry(
            &fixture.book.id,
            probabilistic_entry_id,
            "Attempt-owned probability",
            "SYNTHETIC_ATTEMPT_PROBABILITY_92CF",
            ActivationRule::Always,
            5_000,
        );
        fixture.book.entries.push(entry);
        let stored_book = fixture
            .core
            .upsert_knowledge_book(&fixture.book, Some(fixture.book_revision))
            .expect("save probabilistic semantic replay revision");
        fixture.book_revision = stored_book.revision;
        assert_eq!(fixture.book_revision, 2);
        fixture.prompt_preset.metadata.updated_at = Utc::now();
        let stored_preset = fixture
            .core
            .upsert_prompt_preset(&fixture.prompt_preset, Some(fixture.prompt_preset_revision))
            .expect("seal revised knowledge dependency");
        fixture.prompt_preset_revision = stored_preset.revision;
        assert_eq!(fixture.prompt_preset_revision, 2);
        expected_probability_selection
    }

    fn prepare_final_semantic_replay_preview(
        fixture: &mut SemanticReplayFixture,
    ) -> (crate::ExpertPromptPreview, u64) {
        let operation_target = GenerationActionTargetIdentity::GenerationTarget {
            model_route_id: fixture.request.generation_target.model_route_id.clone(),
            generation_preset_id: fixture
                .request
                .generation_target
                .generation_preset_id
                .clone(),
        };
        let base_request_fingerprint_sha256 =
            same_branch_generation_semantic_fingerprint(&SameBranchGenerationAttemptIdentity {
                conversation_id: &fixture.request.conversation_id,
                branch_id: &fixture.request.branch_id,
                expected_head: fixture.request.expected_head.as_ref(),
                text: &fixture.request.user_text,
                operation_context: GenerationOperationContext::New {
                    operation_nonce: "semantic-final-replay-v1",
                },
                target: &operation_target,
                temperature: None,
                max_output_tokens: None,
                prompt_preset_id: fixture.request.prompt_preset_id.as_ref(),
                variable_overrides: &fixture.request.variable_overrides,
            })
            .expect("derive semantic replay base request fingerprint");
        let session_seed = reviewed_prompt_session_seed(&base_request_fingerprint_sha256);
        assert_ne!(
            session_seed, 0,
            "attempt seed cannot be the legacy constant"
        );
        let expected_probability_selection =
            add_semantic_replay_probability_entry(fixture, session_seed);
        let preview = fixture
            .core
            .resolve_prompt_preview(
                &fixture.request,
                GenerationOperationContext::New {
                    operation_nonce: "semantic-final-replay-v1",
                },
            )
            .expect("resolve final semantic replay preview");
        let attempt = fixture
            .core
            .inner
            .storage
            .get_generation_attempt(&preview.generation_attempt_id)
            .expect("load final semantic replay attempt");
        assert_eq!(
            attempt.input.base_request_fingerprint_sha256, base_request_fingerprint_sha256,
            "attempt must persist the nonce-free semantic seed authority"
        );
        let preview_text = preview
            .effective_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(preview_text.contains("SYNTHETIC_SEMANTIC_COBALT_MOON_41B7"));
        assert_eq!(
            preview_text.contains("SYNTHETIC_ATTEMPT_PROBABILITY_92CF"),
            expected_probability_selection,
        );
        (preview, session_seed)
    }

    fn assert_semantic_replay_restart_and_send(
        root: &Path,
        request: &crate::PromptPlanRequest,
        connection_id: &ProviderConnectionId,
        credential_authority: &ProviderCredentialAccessAuthority,
        preview: &crate::ExpertPromptPreview,
        session_seed: u64,
    ) {
        let reopened = Core::open(CoreConfig::new(root)).expect("reopen semantic replay core");
        let reopened_preview = reopened
            .resolve_prompt_preview(
                request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &preview.generation_attempt_id,
                },
            )
            .expect("resolve semantic replay preview after restart");
        assert_eq!(&reopened_preview, preview);

        let mut reviewed = request.clone();
        reviewed.expected_plan_hash = Some(reopened_preview.plan.plan_hash.clone());
        let generation_id = reopened
            .send_message_with_prompt_plan(
                &reviewed,
                &reopened_preview.generation_attempt_id,
                ConnectionBoundCredential::new_with_access_authority(
                    connection_id.clone(),
                    Some("synthetic-semantic-replay-credential".to_owned()),
                    credential_authority.clone(),
                ),
            )
            .expect("send exact restarted semantic replay preview");
        let stored_plan = reopened
            .get_generation_prompt_plan(&generation_id)
            .expect("load sent semantic replay plan");
        assert_eq!(stored_plan.id, reopened_preview.plan.plan_id);
        assert_eq!(stored_plan.random_seed, Some(session_seed));
        assert_eq!(
            stored_plan.provider_request.mapping_diagnostics.value["knowledge_semantic_evidence"]
                [0]["source"]["kind"],
            "lexical_v1",
        );
        let sent_plan: ResolvedPromptPlan = serde_json::from_value(stored_plan.plan.value)
            .expect("decode sent semantic replay plan");
        assert_eq!(
            sent_plan
                .effective_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
            reopened_preview
                .effective_messages
                .iter()
                .map(|message| message.content.as_str())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    fn semantic_knowledge_and_probability_replay_across_restart_preview_and_send() {
        let mut fixture = create_semantic_replay_fixture();
        let (preview, session_seed) = prepare_final_semantic_replay_preview(&mut fixture);
        let credential_authority =
            install_provider_credential_authority(&fixture.core, &fixture.connection_id);
        drop(fixture.core);
        assert_semantic_replay_restart_and_send(
            fixture.root.path(),
            &fixture.request,
            &fixture.connection_id,
            &credential_authority,
            &preview,
            session_seed,
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn provider_semantic_knowledge_reuses_one_durable_query_for_preview_and_send() {
        let (root, core, character) = imported_core();
        let conversation = core
            .create_conversation(
                &character.id,
                "Provider semantic replay",
                ConversationMode::Chat,
            )
            .expect("create provider semantic conversation");
        let first_generation = core
            .send_message_with_provider(
                &conversation.id,
                "durable first turn",
                "static-provider".to_owned(),
                None,
                Arc::new(StaticProvider::new("durable assistant anchor")),
            )
            .expect("create durable semantic anchor");
        wait_for_generation_status(&core, &first_generation, GenerationStatus::Complete);
        wait_for_generation_registry_to_drain(&core);
        let branch_id = core
            .get_conversation_state(&conversation.id)
            .expect("provider semantic state")
            .active_branch_id;
        let durable_messages = core
            .list_branch_messages(&branch_id)
            .expect("provider semantic anchor messages");
        let durable_head = durable_messages
            .last()
            .expect("durable assistant anchor")
            .id
            .clone();

        let (template, route) = create_built_in_public_route(
            &core,
            "openai-responses-v1",
            "/v1",
            "text-embedding-provider-semantic-fixture",
        );
        let generation_preset = core
            .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
            .expect("save provider semantic generation preset");
        let summary_task_id = TaskProfileId::from("synthetic.provider-semantic.summary-task");
        core.upsert_task_profile(
            &TaskProfile {
                id: summary_task_id.clone(),
                kind: AuxiliaryTaskKind::MemorySummary,
                route_id: route.id.clone(),
                generation_preset_id: generation_preset.id.clone(),
                fallback_route_ids: Vec::new(),
                embedding_dimensions: None,
                timeout_ms: 5_000,
                rate_limit: RateLimit {
                    requests: 100,
                    per_seconds: 60,
                },
                concurrency_limit: 1,
            },
            None,
        )
        .expect("save provider semantic summary task");
        let embedding_task_id = TaskProfileId::from("synthetic.provider-semantic.embedding-task");
        let embedding_task = core
            .upsert_task_profile(
                &TaskProfile {
                    id: embedding_task_id.clone(),
                    kind: AuxiliaryTaskKind::MemoryEmbedding,
                    route_id: route.id.clone(),
                    generation_preset_id: generation_preset.id.clone(),
                    fallback_route_ids: Vec::new(),
                    embedding_dimensions: Some(3),
                    timeout_ms: 5_000,
                    rate_limit: RateLimit {
                        requests: 100,
                        per_seconds: 60,
                    },
                    concurrency_limit: 1,
                },
                None,
            )
            .expect("save provider semantic embedding task");
        let memory_profile_id = MemoryProfileId::from("synthetic.provider-semantic.memory-profile");
        let memory_profile = core
            .upsert_memory_profile(
                &MemoryProfile {
                    id: memory_profile_id.clone(),
                    name: "Synthetic provider semantic memory".to_owned(),
                    schema_version: 1,
                    summary_task: summary_task_id,
                    embedding_task: Some(embedding_task_id),
                    turns_per_summary: 100,
                    recent_raw_budget: TokenBudget { max_tokens: 1_024 },
                    episodic_budget: TokenBudget { max_tokens: 1_024 },
                    semantic_budget: TokenBudget { max_tokens: 1_024 },
                    retrieval_count: 16,
                    recency_weight: 1.0,
                    similarity_weight: 1.0,
                    importance_weight: 1.0,
                    preserve_invalidated_records: true,
                    summary_schema: SummarySchemaId::from(
                        "synthetic.provider-semantic.summary-schema",
                    ),
                    provenance: prompt_attempt_test_provenance(
                        "synthetic.provider-semantic.memory-profile",
                    ),
                },
                None,
            )
            .expect("save provider semantic memory profile");

        let book_id = KnowledgeBookId::from("synthetic.provider-semantic.book");
        let entry_id = KnowledgeEntryId::from("synthetic.provider-semantic.entry");
        let book = KnowledgeBook {
            id: book_id.clone(),
            name: "Synthetic provider semantic knowledge".to_owned(),
            schema_version: 1,
            entries: vec![KnowledgeEntry {
                id: entry_id.clone(),
                book_id: book_id.clone(),
                name: "Provider-only vector match".to_owned(),
                content: "SYNTHETIC_PROVIDER_SEMANTIC_VECTOR_31AD".to_owned(),
                enabled: true,
                activation: ActivationRule::Semantic {
                    threshold: 0.9,
                    top_k: 1,
                },
                priority: 100,
                importance: 100,
                placement: KnowledgePlacement::RetrievedContext,
                token_policy: TokenPolicy {
                    priority: 100,
                    min_tokens: None,
                    max_tokens: Some(64),
                    reserve_tokens: None,
                },
                parent_id: None,
                activation_probability_basis_points: 10_000,
                provenance: prompt_attempt_test_provenance("synthetic.provider-semantic.entry"),
            }],
            scan_depth: 8,
            token_budget: TokenBudget { max_tokens: 128 },
            recursive: false,
            max_recursion_depth: 0,
            provenance: prompt_attempt_test_provenance("synthetic.provider-semantic.book"),
        };
        let stored_book = core
            .upsert_knowledge_book(&book, None)
            .expect("save provider semantic book");
        let book_revision_id = stored_book
            .revision_id
            .clone()
            .expect("provider semantic book revision id");

        let now = Utc::now();
        let mut prompt_preset = lorepia_orchestration::default_prompt_preset(
            lorepia_domain::PromptPresetId::from("synthetic.provider-semantic.preset"),
            "Synthetic provider semantic preset",
            PresetMetadata {
                description: "Synthetic provider semantic fixture".to_owned(),
                tags: vec!["synthetic".to_owned()],
                provenance: prompt_attempt_test_provenance("synthetic.provider-semantic.preset"),
                created_at: now,
                updated_at: now,
                local_override_of: None,
            },
        );
        for block in &mut prompt_preset.blocks {
            block.provenance = prompt_attempt_test_provenance(block.id.as_str());
        }
        prompt_preset.blocks.push(PromptBlock {
            id: PromptBlockId::from("synthetic.provider-semantic.knowledge-block"),
            name: "Synthetic provider semantic knowledge".to_owned(),
            kind: PromptBlockKind::WorldKnowledge,
            enabled: true,
            role_hint: RoleHint::System,
            authority: InstructionAuthority::Creator,
            template: None,
            condition: None,
            source: BlockSource::SelectedKnowledge,
            placement_zone: PlacementZone::RetrievedContext,
            history_selector: None,
            token_policy: TokenPolicy {
                priority: 1_000,
                min_tokens: None,
                max_tokens: Some(128),
                reserve_tokens: None,
            },
            overflow_policy: OverflowPolicy::ReduceKnowledgeEntries,
            merge_policy: MergePolicy::SeparateMessage,
            provenance: prompt_attempt_test_provenance(
                "synthetic.provider-semantic.knowledge-block",
            ),
        });
        prompt_preset
            .blocks
            .sort_by_key(|block| block.placement_zone);
        prompt_preset.knowledge_book_ids.push(book_id);
        prompt_preset.memory_profile_id = Some(memory_profile_id.clone());
        core.upsert_prompt_preset(&prompt_preset, None)
            .expect("save provider semantic prompt preset");
        core.bind_prompt_preset(
            &PromptPresetBinding {
                id: "synthetic.provider-semantic.binding".to_owned(),
                prompt_preset_id: prompt_preset.id.clone(),
                scope: ModuleScope::Conversation,
                target_id: Some(conversation.id.0.clone()),
                conversation_id: None,
                pinned_revision_id: None,
                priority: 0,
                enabled: true,
                response_length: PromptResponseLength::Balanced,
                creativity: 50,
                reasoning_effort: None,
                memory_enabled: true,
                knowledge_enabled: true,
                variable_overrides: VariableMap::default(),
                generation_preset_override_id: None,
                user_name_override: None,
                author_note: None,
                group_context: None,
                template_slots: Vec::new(),
                created_at: now,
                updated_at: now,
            },
            None,
        )
        .expect("bind provider semantic prompt preset");

        let connection = core
            .inner
            .storage
            .get_provider_connection(&route.connection_id)
            .expect("load provider semantic connection");
        let credential_authority = install_provider_credential_authority(&core, &connection.id);
        let embedding_provider = AdapterRegistry::new()
            .build_embedding_provider_for_route(&template, &connection, &route, 3)
            .expect("build provider semantic embedding contract");
        let embedding_contract = embedding_provider.contract();
        let vector_space_sha256 = embedding_contract.vector_space_sha256();
        assert_eq!(
            embedding_contract
                .execution_sha256(EmbeddingPurpose::RetrievalQuery)
                .len(),
            64
        );
        let task_profile_revision_id = embedding_task
            .revision_id
            .expect("provider semantic task revision id");
        let memory_profile_revision_id = memory_profile
            .revision_id
            .expect("provider semantic memory revision id");
        let query_text = {
            let mut texts = durable_messages
                .iter()
                .map(|message| message.content.clone())
                .collect::<Vec<_>>();
            texts.push("opaque provider vector query".to_owned());
            texts
                .iter()
                .rev()
                .filter(|text| !text.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join("\n\n")
        };
        let query_sha256 = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&("lorepia.memory-query.v1", &query_text))
                    .expect("encode provider semantic query")
            )
        );
        let intent_digest = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    "lorepia.memory-query-embedding-intent.v1",
                    memory_profile_id.as_str(),
                    memory_profile_revision_id.as_str(),
                    task_profile_revision_id.as_str(),
                    conversation.id.0.as_str(),
                    branch_id.0.as_str(),
                    durable_head.0.as_str(),
                    durable_head.0.as_str(),
                    query_sha256.as_str(),
                    vector_space_sha256.as_str(),
                    route.id.as_str(),
                    3_u32,
                ))
                .expect("encode provider semantic intent")
            )
        );
        let intent = MemoryQueryEmbeddingIntent {
            id: format!("memory-query-embedding-{intent_digest}"),
            idempotency_key: format!("memory-query-embedding:v1:{intent_digest}"),
            memory_profile_id,
            memory_profile_revision_id,
            task_profile_revision_id: task_profile_revision_id.clone(),
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            source_start_message_id: durable_head.clone(),
            source_end_message_id: durable_head.clone(),
            query_sha256,
            vector_space_sha256: vector_space_sha256.clone(),
            model_route_id: route.id.clone(),
            dimensions: 3,
            created_at: now,
        };
        let request = crate::PromptPlanRequest {
            conversation_id: conversation.id.clone(),
            branch_id: branch_id.clone(),
            expected_head: Some(durable_head),
            user_text: "opaque provider vector query".to_owned(),
            generation_target: GenerationTarget {
                model_route_id: route.id.clone(),
                generation_preset_id: generation_preset.id.clone(),
            },
            prompt_preset_id: Some(prompt_preset.id.clone()),
            variable_overrides: VariableMap::default(),
            expected_plan_hash: None,
        };
        let lexical_fallback_preview = core
            .resolve_prompt_preview_async(
                &request,
                new_test_generation_operation("provider-semantic-preview-v1"),
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("fall back lexically before exact knowledge vectors exist");
        assert!(
            lexical_fallback_preview
                .effective_messages
                .iter()
                .all(|message| {
                    !message
                        .content
                        .contains("SYNTHETIC_PROVIDER_SEMANTIC_VECTOR_31AD")
                })
        );
        assert_eq!(
            core.inner
                .storage
                .get_memory_query_embedding(&intent.id)
                .expect_err("lexical fallback must not enqueue a provider query")
                .code,
            CoreErrorCode::NotFound,
        );
        let queued = core
            .inner
            .storage
            .enqueue_memory_query_embedding(&intent)
            .expect("enqueue provider semantic query");
        let running = core
            .inner
            .storage
            .claim_memory_query_embedding(&intent.id, queued.entry.revision, now)
            .expect("claim provider semantic query");
        let completed = core
            .inner
            .storage
            .complete_memory_query_embedding(&intent.id, running.revision, &[1.0, 0.0, 0.0], now)
            .expect("complete provider semantic query");
        assert_eq!(completed.revision, 3);
        core.inner
            .storage
            .save_knowledge_embedding(&KnowledgeEmbeddingWrite {
                id: "synthetic-provider-semantic-embedding".to_owned(),
                book_revision_id,
                entry_id,
                task_profile_revision_id,
                model_route_id: route.id.clone(),
                dimensions: 3,
                vector_space_sha256,
                values: vec![1.0, 0.0, 0.0],
                created_at: now,
            })
            .expect("save provider semantic knowledge embedding");

        let preview = core
            .resolve_prompt_preview_async(
                &request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &lexical_fallback_preview.generation_attempt_id,
                },
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("resolve provider semantic preview from durable query");
        assert_eq!(
            preview.generation_attempt_id,
            lexical_fallback_preview.generation_attempt_id,
        );
        assert!(preview.effective_messages.iter().any(|message| {
            message
                .content
                .contains("SYNTHETIC_PROVIDER_SEMANTIC_VECTOR_31AD")
        }));
        drop(core);
        let core = Core::open(CoreConfig::new(root.path()))
            .expect("reopen provider semantic core before reviewed send");
        assert_eq!(
            core.resolve_prompt_preview_async(
                &request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &preview.generation_attempt_id,
                },
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("repeat provider semantic preview"),
            preview,
        );

        let mut reviewed = request;
        reviewed.expected_plan_hash = Some(preview.plan.plan_hash.clone());
        let generation_id = core
            .send_message_with_prompt_plan_async(
                &reviewed,
                &preview.generation_attempt_id,
                ConnectionBoundCredential::new_with_access_authority(
                    connection.id.clone(),
                    Some("synthetic-provider-semantic-credential".to_owned()),
                    credential_authority.clone(),
                ),
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("send provider semantic reviewed plan");
        let stored_plan = core
            .get_generation_prompt_plan(&generation_id)
            .expect("load provider semantic sent plan");
        assert_eq!(stored_plan.id, preview.plan.plan_id);
        assert_eq!(
            stored_plan.provider_request.mapping_diagnostics.value["knowledge_semantic_evidence"]
                [0]["source"]["kind"],
            "provider_embedding_v1",
        );
        let reused_query = core
            .inner
            .storage
            .get_memory_query_embedding(&intent.id)
            .expect("load reused provider semantic query");
        assert_eq!(reused_query.revision, 3);
        assert_eq!(reused_query.attempts, 1);

        let root_conversation = core
            .create_conversation(
                &character.id,
                "Provider semantic lexical root fallback",
                ConversationMode::Chat,
            )
            .expect("create provider semantic root conversation");
        let root_branch_id = core
            .get_conversation_state(&root_conversation.id)
            .expect("provider semantic root state")
            .active_branch_id;
        core.bind_prompt_preset(
            &PromptPresetBinding {
                id: "synthetic.provider-semantic.root-binding".to_owned(),
                prompt_preset_id: prompt_preset.id.clone(),
                scope: ModuleScope::Conversation,
                target_id: Some(root_conversation.id.0.clone()),
                conversation_id: None,
                pinned_revision_id: None,
                priority: 0,
                enabled: true,
                response_length: PromptResponseLength::Balanced,
                creativity: 50,
                reasoning_effort: None,
                memory_enabled: true,
                knowledge_enabled: true,
                variable_overrides: VariableMap::default(),
                generation_preset_override_id: None,
                user_name_override: None,
                author_note: None,
                group_context: None,
                template_slots: Vec::new(),
                created_at: now,
                updated_at: now,
            },
            None,
        )
        .expect("bind provider semantic root prompt preset");
        let root_request = crate::PromptPlanRequest {
            conversation_id: root_conversation.id,
            branch_id: root_branch_id,
            expected_head: None,
            user_text: "lexically unrelated first turn".to_owned(),
            generation_target: GenerationTarget {
                model_route_id: route.id,
                generation_preset_id: generation_preset.id,
            },
            prompt_preset_id: Some(prompt_preset.id),
            variable_overrides: VariableMap::default(),
            expected_plan_hash: None,
        };
        let root_preview = core
            .resolve_prompt_preview_async(
                &root_request,
                new_test_generation_operation("provider-semantic-root-preview-v1"),
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("resolve provider semantic root preview with lexical fallback");
        assert!(root_preview.effective_messages.iter().all(|message| {
            !message
                .content
                .contains("SYNTHETIC_PROVIDER_SEMANTIC_VECTOR_31AD")
        }));
        assert_eq!(
            core.resolve_prompt_preview_async(
                &root_request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &root_preview.generation_attempt_id,
                },
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("repeat provider semantic root preview"),
            root_preview,
        );
        let mut reviewed_root = root_request;
        reviewed_root.expected_plan_hash = Some(root_preview.plan.plan_hash.clone());
        let root_generation_id = core
            .send_message_with_prompt_plan_async(
                &reviewed_root,
                &root_preview.generation_attempt_id,
                ConnectionBoundCredential::new_with_access_authority(
                    connection.id,
                    Some("synthetic-provider-semantic-root-credential".to_owned()),
                    credential_authority,
                ),
                &RejectingTaskCredentialBroker,
                watch::channel(false).1,
            )
            .await
            .expect("send exact provider semantic root lexical preview");
        let root_plan = core
            .get_generation_prompt_plan(&root_generation_id)
            .expect("load provider semantic root plan");
        assert_eq!(root_plan.id, root_preview.plan.plan_id);
        assert_eq!(
            root_plan.provider_request.mapping_diagnostics.value["knowledge_semantic_evidence"][0]
                ["source"]["kind"],
            "lexical_v1",
        );
    }

    fn wait_for_generation_sequence_watermark(
        core: &Core,
        generation_id: &GenerationId,
        expected: u64,
    ) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while core
            .inner
            .active_generations
            .sequence_watermark_for_test(generation_id)
            != Some(expected)
        {
            assert!(
                Instant::now() < deadline,
                "initial live events did not reach the registry"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_cancelled_subscription_event(
        receiver: &mut broadcast::Receiver<ChatEvent>,
        generation_id: &GenerationId,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match receiver.try_recv() {
                Ok(event) => {
                    assert_eq!(&event.generation_id, generation_id);
                    assert_eq!(&event.conversation_id, conversation_id);
                    assert_eq!(event.branch_id.as_ref(), Some(branch_id));
                    if matches!(event.kind, ChatEventKind::GenerationCancelled) {
                        assert_eq!(event.sequence, 4);
                        break;
                    }
                }
                Err(broadcast::error::TryRecvError::Empty) => {
                    assert!(
                        Instant::now() < deadline,
                        "terminal event was lost at the subscription boundary"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("terminal subscription failed: {error:?}"),
            }
        }
    }

    struct PreparingGenerationFixture {
        launch: GenerationLaunchPermit,
        generation: GenerationRecord,
        user: Message,
        assistant: Message,
    }

    fn prepare_registered_generation(
        core: &Core,
        conversation: &Conversation,
    ) -> PreparingGenerationFixture {
        prepare_registered_generation_for_model(core, conversation, "synthetic")
            .expect("register preparing generation")
    }

    fn prepare_registered_generation_for_model(
        core: &Core,
        conversation: &Conversation,
        model: &str,
    ) -> CoreResult<PreparingGenerationFixture> {
        let branch = core
            .get_conversation_state(&conversation.id)
            .expect("conversation state")
            .active_branch_id;
        let user = Message::user(conversation.id.clone(), "prepare atomic subscription");
        let generation_id = GenerationId::new();
        let assistant = Message::pending_assistant(
            conversation.id.clone(),
            user.id.clone(),
            generation_id.clone(),
        );
        let generation = GenerationRecord {
            id: generation_id,
            conversation_id: conversation.id.clone(),
            branch_id: branch,
            user_message_id: user.id.clone(),
            assistant_message_id: Some(assistant.id.clone()),
            mode: ConversationMode::Chat,
            model: model.to_owned(),
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
            started_at: assistant.created_at,
            finished_at: None,
        };
        let provider_target = GenerationActionTargetIdentity::DirectModel {
            model_sha256: model.to_owned(),
        };
        let launch = core.prepare_generation_launch_for_target(&generation, &provider_target)?;
        Ok(PreparingGenerationFixture {
            launch,
            generation,
            user,
            assistant,
        })
    }

    #[test]
    fn generation_admission_scopes_provider_and_conversation() {
        let (_provider_root, provider_core, provider_character) = imported_core();
        let mut provider_permits = Vec::with_capacity(MAX_ACTIVE_GENERATIONS_PER_PROVIDER);
        for _ in 0..MAX_ACTIVE_GENERATIONS_PER_PROVIDER {
            let conversation = provider_core
                .open_conversation(&provider_character.id)
                .expect("provider-scope conversation");
            provider_permits.push(
                prepare_registered_generation_for_model(
                    &provider_core,
                    &conversation,
                    "shared-provider-model",
                )
                .expect("generation within provider admission"),
            );
        }
        let overflow_conversation = provider_core
            .open_conversation(&provider_character.id)
            .expect("provider overflow conversation");
        let provider_error = prepare_registered_generation_for_model(
            &provider_core,
            &overflow_conversation,
            "shared-provider-model",
        )
        .err()
        .expect("provider admission must be bounded");
        assert_eq!(provider_error.code, CoreErrorCode::ProviderRateLimited);
        assert!(provider_error.message.contains("provider"));
        drop(provider_permits.pop());
        prepare_registered_generation_for_model(
            &provider_core,
            &overflow_conversation,
            "shared-provider-model",
        )
        .expect("dropping an unlaunched permit releases provider admission");

        let (_conversation_root, conversation_core, conversation_character) = imported_core();
        let conversation = conversation_core
            .open_conversation(&conversation_character.id)
            .expect("conversation-scope conversation");
        let mut conversation_permits = Vec::with_capacity(MAX_ACTIVE_GENERATIONS_PER_CONVERSATION);
        for index in 0..MAX_ACTIVE_GENERATIONS_PER_CONVERSATION {
            conversation_permits.push(
                prepare_registered_generation_for_model(
                    &conversation_core,
                    &conversation,
                    &format!("conversation-model-{index}"),
                )
                .expect("generation within conversation admission"),
            );
        }
        let conversation_error = prepare_registered_generation_for_model(
            &conversation_core,
            &conversation,
            "conversation-overflow-model",
        )
        .err()
        .expect("conversation admission must be bounded");
        assert_eq!(conversation_error.code, CoreErrorCode::ProviderRateLimited);
        assert!(conversation_error.message.contains("conversation"));
        drop(conversation_permits.pop());
        prepare_registered_generation_for_model(
            &conversation_core,
            &conversation,
            "conversation-replacement-model",
        )
        .expect("dropping an unlaunched permit releases conversation admission");
    }

    #[test]
    fn generation_subscription_accepts_durable_running_before_local_activation() {
        let (_root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let fixture = prepare_registered_generation(&core, &conversation);
        assert_eq!(
            core.inner
                .active_generations
                .phase_for_test(&fixture.generation.id),
            Some(GenerationDeliveryPhase::Preparing)
        );
        assert_eq!(
            core.subscribe_generation_events(
                &fixture.generation.id,
                &conversation.id,
                &fixture.generation.branch_id,
            )
            .err()
            .expect("pre-append generation cannot be subscribed")
            .code,
            CoreErrorCode::NotFound
        );

        core.inner
            .storage
            .append_generation(
                &fixture.generation.branch_id,
                None,
                &fixture.user,
                &fixture.assistant,
                &fixture.generation,
            )
            .expect("durably append generation before local activation");
        assert_eq!(
            core.inner
                .storage
                .get_generation(&fixture.generation.id)
                .expect("durable running generation")
                .status,
            GenerationStatus::Running
        );
        assert_eq!(
            core.inner
                .active_generations
                .phase_for_test(&fixture.generation.id),
            Some(GenerationDeliveryPhase::Preparing)
        );

        let subscription = core
            .subscribe_generation_events(
                &fixture.generation.id,
                &conversation.id,
                &fixture.generation.branch_id,
            )
            .expect("durable running generation is authoritative");
        let (_receiver, assistant_message_id, sequence_watermark, display_prefix, reasoning_prefix) =
            subscription.into_parts();
        assert_eq!(sequence_watermark, 0);
        assert_eq!(assistant_message_id, fixture.assistant.id);
        assert!(display_prefix.is_empty());
        assert!(reasoning_prefix.is_empty());
        drop(fixture.launch);
    }

    #[test]
    fn generation_subscription_is_atomic_with_terminal_persistence_and_publish() {
        let (_root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let (provider, provider_started) = StallingProvider::new("in flight");
        let generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "start",
                "stalling".to_owned(),
                None,
                provider,
            )
            .expect("start generation");
        provider_started
            .recv_timeout(Duration::from_secs(2))
            .expect("provider started");
        wait_for_generation_sequence_watermark(&core, &generation_id, 2);

        let generation = core
            .inner
            .storage
            .get_generation(&generation_id)
            .expect("running generation");
        let branch_id = generation.branch_id.clone();
        let wrong_route = core
            .subscribe_generation_events(
                &generation_id,
                &conversation.id,
                &ConversationBranchId("wrong-branch".to_owned()),
            )
            .err()
            .expect("wrong route must not disclose a live generation");
        assert_eq!(wrong_route.code, CoreErrorCode::NotFound);
        let (subscription_entered, subscription_entered_receiver) = std_mpsc::channel();
        let (release_subscription, release_subscription_receiver) = std_mpsc::channel();
        core.inner
            .active_generations
            .pause_next_subscription_for_test(
                &generation_id,
                subscription_entered,
                release_subscription_receiver,
            )
            .expect("install subscription boundary pause");

        let subscribing_core = core.clone();
        let subscribing_generation_id = generation_id.clone();
        let subscribing_conversation_id = conversation.id.clone();
        let subscribing_branch_id = branch_id.clone();
        let subscription = thread::spawn(move || {
            subscribing_core.subscribe_generation_events(
                &subscribing_generation_id,
                &subscribing_conversation_id,
                &subscribing_branch_id,
            )
        });
        subscription_entered_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("subscription reached the receiver boundary");

        core.cancel_generation(&generation_id)
            .expect("cancel generation while subscription is paused");
        wait_for_generation_status(&core, &generation_id, GenerationStatus::Cancelled);
        release_subscription
            .send(())
            .expect("release subscription boundary");

        let subscription = subscription
            .join()
            .expect("subscription thread")
            .expect("atomic live subscription");
        let (
            mut receiver,
            assistant_message_id,
            sequence_watermark,
            display_prefix,
            reasoning_prefix,
        ) = subscription.into_parts();
        assert_eq!(sequence_watermark, 2);
        assert_eq!(display_prefix, "in flight");
        assert!(reasoning_prefix.is_empty());
        assert_eq!(
            Some(assistant_message_id),
            generation.assistant_message_id.clone()
        );
        assert_cancelled_subscription_event(
            &mut receiver,
            &generation_id,
            &conversation.id,
            &branch_id,
        );
        wait_for_generation_registry_to_drain(&core);
        let terminal = core
            .subscribe_generation_events(&generation_id, &conversation.id, &branch_id)
            .err()
            .expect("terminal generation cannot create an empty live subscription");
        assert_eq!(terminal.code, CoreErrorCode::NotFound);
    }

    #[test]
    fn generation_subscription_recovers_uncheckpointed_reasoning_and_text_prefixes() {
        for preserve_partial in [false, true] {
            let (_root, core, character) = imported_core();
            let mut settings = core.get_settings().expect("load settings");
            settings.preserve_partial_generations = preserve_partial;
            core.update_settings(&settings)
                .expect("configure durable partial checkpoints");
            let conversation = core.open_conversation(&character.id).expect("conversation");
            let (provider, provider_started, release_provider) = CatchupSnapshotProvider::new();
            let generation_id = core
                .send_message_with_provider(
                    &conversation.id,
                    "start",
                    "catch-up".to_owned(),
                    None,
                    provider,
                )
                .expect("start generation");
            let catchup_started_at = Instant::now();
            provider_started
                .recv_timeout(Duration::from_secs(2))
                .expect("provider emitted pre-subscription prefixes");
            wait_for_generation_sequence_watermark(&core, &generation_id, 3);
            if preserve_partial {
                assert!(
                    catchup_started_at.elapsed() < PARTIAL_CHECKPOINT_INTERVAL,
                    "the regression must subscribe before the 500 ms durable checkpoint"
                );
            }

            let generation = core
                .inner
                .storage
                .get_generation(&generation_id)
                .expect("running generation");
            let persisted_assistant = core
                .list_branch_messages(&generation.branch_id)
                .expect("durable branch messages")
                .into_iter()
                .find(|message| message.generation_id.as_ref() == Some(&generation_id))
                .expect("pending assistant");
            assert_eq!(persisted_assistant.status, MessageStatus::Pending);
            assert!(
                persisted_assistant.content.is_empty(),
                "the live prefixes must not rely on a durable partial checkpoint (preserve_partial={preserve_partial})"
            );

            let subscription = core
                .subscribe_generation_events(
                    &generation_id,
                    &conversation.id,
                    &generation.branch_id,
                )
                .expect("subscribe after live prefixes");
            let (
                mut receiver,
                assistant_message_id,
                sequence_watermark,
                display_prefix,
                reasoning_prefix,
            ) = subscription.into_parts();
            assert_eq!(assistant_message_id, persisted_assistant.id);
            assert_eq!(sequence_watermark, 3);
            assert_eq!(display_prefix, "text-prefix");
            assert_eq!(reasoning_prefix, "reasoning-prefix");
            release_provider.send(()).expect("release provider suffix");
            wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);

            let mut reasoning_suffix = String::new();
            let mut text_suffix = String::new();
            while let Ok(event) = receiver.try_recv() {
                match event.kind {
                    ChatEventKind::ReasoningDelta(delta) => reasoning_suffix.push_str(&delta),
                    ChatEventKind::TextDelta(delta) => text_suffix.push_str(&delta),
                    _ => {}
                }
            }
            assert_eq!(
                (
                    format!("{reasoning_prefix}{reasoning_suffix}"),
                    format!("{display_prefix}{text_suffix}"),
                ),
                (
                    "reasoning-prefix+reasoning-suffix".to_owned(),
                    "text-prefix+text-suffix".to_owned(),
                ),
                "reattachment must reconstruct the exact live prefix plus suffix (preserve_partial={preserve_partial})",
            );
        }
    }

    #[test]
    fn live_generation_prefix_accepts_the_maximum_valid_display_transform() {
        let mut transform_set = display_only_generation_transform_set();
        transform_set.rules[0].pattern = SafeRegex {
            pattern: "x".to_owned(),
            case_insensitive: false,
        };
        transform_set.rules[0].replacement =
            "😀".repeat(lorepia_orchestration::DEFAULT_MAX_REPLACEMENT_CHARS);
        transform_set.rules[0].max_replacements = 32;
        transform_set.rules[0].input_limit = 32;
        transform_set.rules[0].output_limit =
            u32::try_from(MAX_LIVE_DISPLAY_PREFIX_CHARS).expect("display char cap fits u32");
        transform_set.max_output_chars =
            u32::try_from(MAX_LIVE_DISPLAY_PREFIX_CHARS).expect("display char cap fits u32");
        let context = GenerationTransformContext {
            sets: vec![transform_set],
            variables: VariableMap::default(),
            supported_capabilities: Vec::new(),
            approved_import_source_ids: std::collections::BTreeSet::new(),
            display_context: None,
        };
        let (_, projection) = apply_generation_output_transforms(
            Ok(GenerationOutcome {
                text: "x".repeat(32),
                usage: GenerationUsage::default(),
                opaque_reasoning_state: Vec::new(),
                last_sequence: 2,
            }),
            &context,
        );
        let display = projection
            .expect("valid maximum DisplayOnly projection")
            .display_content;
        assert_eq!(display.chars().count(), MAX_LIVE_DISPLAY_PREFIX_CHARS);
        assert_eq!(display.len(), MAX_LIVE_DISPLAY_PREFIX_BYTES);

        let mut prefix = GenerationLivePrefix::default();
        let reasoning = "r".repeat(MAX_GENERATED_OUTPUT_CHARS);
        assert!(prefix.append(&ChatEventKind::ReasoningDelta(reasoning)));
        assert!(prefix.append(&ChatEventKind::TextDelta(display)));
        assert_eq!(prefix.reasoning_chars, MAX_GENERATED_OUTPUT_CHARS);
        assert_eq!(prefix.display_chars, MAX_LIVE_DISPLAY_PREFIX_CHARS);
        assert_eq!(prefix.display.len(), MAX_LIVE_DISPLAY_PREFIX_BYTES);
        assert!(!prefix.append(&ChatEventKind::TextDelta("overflow".to_owned())));
        assert!(!prefix.append(&ChatEventKind::ReasoningDelta("overflow".to_owned())));
    }

    #[test]
    fn usage_overflow_is_compensated_as_failed_and_allows_the_next_send() {
        let (root, core, character) = imported_core();
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let mut events = core.subscribe_events();
        let secret = "credential-must-not-leak";
        let failed_generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "first",
                "overflow".to_owned(),
                Some(secret.to_owned()),
                Arc::new(OverflowUsageProvider),
            )
            .expect("start overflow generation");

        let failed_generation =
            wait_for_generation_status(&core, &failed_generation_id, GenerationStatus::Failed);
        wait_for_generation_registry_to_drain(&core);
        assert_eq!(failed_generation.input_tokens, None);
        assert_eq!(failed_generation.output_tokens, None);
        assert_eq!(
            failed_generation.error_code.as_deref(),
            Some(CoreErrorCode::StorageUnavailable.as_str())
        );
        assert!(failed_generation.finished_at.is_some());

        let failed_messages = core
            .list_messages(&conversation.id)
            .expect("failed messages");
        assert_eq!(failed_messages.len(), 2);
        assert_eq!(failed_messages[1].status, MessageStatus::Failed);
        assert_eq!(failed_messages[1].content, "response before invalid usage");

        let observed = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        assert!(observed.iter().any(|event| {
            matches!(
                &event.kind,
                ChatEventKind::GenerationFailed { code, message }
                    if code == CoreErrorCode::StorageUnavailable.as_str()
                        && message == GENERATION_PERSISTENCE_FAILURE_MESSAGE
            )
        }));
        assert!(
            !format!("{observed:?}").contains(secret),
            "generation events must not expose credentials"
        );

        drop(core);
        let core = Core::open(CoreConfig::new(root.path())).expect("reopen core");
        assert_eq!(
            core.inner
                .storage
                .get_generation(&failed_generation_id)
                .expect("restored failed generation")
                .status,
            GenerationStatus::Failed
        );
        assert_eq!(
            core.list_messages(&conversation.id)
                .expect("restored failed messages")[1]
                .status,
            MessageStatus::Failed
        );

        let next_generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "second",
                "static".to_owned(),
                None,
                Arc::new(StaticProvider::new("retry succeeded")),
            )
            .expect("start retry generation");
        wait_for_generation_status(&core, &next_generation_id, GenerationStatus::Complete);
        let messages = core
            .list_messages(&conversation.id)
            .expect("messages after retry");
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[1].status, MessageStatus::Failed);
        assert_eq!(messages[3].status, MessageStatus::Complete);
        assert_eq!(messages[3].content, "retry succeeded");
        assert!(
            messages
                .iter()
                .all(|message| message.status != MessageStatus::Pending)
        );
    }
}
