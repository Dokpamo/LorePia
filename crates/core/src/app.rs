use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use chrono::{DateTime, Utc};
#[cfg(test)]
use lorepia_chat::MAX_GENERATED_OUTPUT_CHARS;
use lorepia_chat::{
    ChatEvent, ChatEventKind, GenerationFailure, GenerationOutcome, MAX_HISTORY_MESSAGE_BYTES,
    MAX_HISTORY_MESSAGE_CHARS, MAX_PROMPT_MESSAGES, run_generation,
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
    runtime::Handle,
    sync::{broadcast, mpsc, watch},
    time::{self, MissedTickBehavior},
};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::{
    CoreConfig, DiscoveryRecoveryOwner, Revisioned,
    catalog::{CatalogRouteProjection, PendingProviderCatalogImportPlan},
    core_version,
    orchestration::{
        GenerationPlanInput, GenerationPromptAuthorityCapture, deterministic_prompt_user_message_id,
    },
    revision::project_revision,
};

mod generation_events;
mod generation_workflow;
mod model_sync;
mod portable_runtime_state;
mod runtime_control;
mod runtime_generation;

pub use generation_events::GenerationEventSubscription;
#[cfg(test)]
use generation_events::GenerationLivePrefix;
use generation_events::{
    GenerationDeliveryPhase, GenerationProviderAdmissionKey, GenerationRegistry,
    generation_subscription_unavailable,
};
use generation_workflow::execute_generation_task;
#[cfg(test)]
use generation_workflow::{
    apply_generation_output_transforms, apply_generation_result, partial_checkpoint_due,
    transform_content_sha256,
};
use runtime_control::RuntimeControl;

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
// Admission belongs to Core rather than renderer stream registrations so a
// detached or failed consumer cannot recycle a slot while provider work keeps
// running. The per-conversation allowance preserves bounded background branch
// generation while preventing one conversation from consuming the process.
const MAX_ACTIVE_GENERATIONS_PER_PROCESS: usize = 32;
const MAX_ACTIVE_GENERATIONS_PER_PROVIDER: usize = 8;
const MAX_ACTIVE_GENERATIONS_PER_CONVERSATION: usize = 4;
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
            // Built-in adapters tear down in-flight transport on this signal; briefly await
            // local confirmation before dropping futures. The remote outcome remains unknown.
            let _ = time::timeout(
                AUXILIARY_PROVIDER_TEARDOWN_GRACE,
                &mut provider_attempt,
            )
            .await;
            return unknown_task_outcome("auxiliary task was cancelled after provider dispatch began");
        }
        () = &mut timeout => {
            let _ = attempt_cancel_sender.send(true);
            // Apply the same bounded local teardown handshake on timeout. A
            // provider which ignores cancellation still has its local attempt
            // force-dropped when this grace period expires.
            let _ = time::timeout(
                AUXILIARY_PROVIDER_TEARDOWN_GRACE,
                &mut provider_attempt,
            )
            .await;
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
mod tests;
