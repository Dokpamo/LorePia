//! High-level use cases for prompt orchestration and creator-owned content.
//!
//! Storage documents remain revisioned and all writes use explicit optimistic
//! concurrency. Prompt rendering stays in `lorepia-orchestration`; this module
//! coordinates that pure engine with conversation, branch, and provider state.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_chat::{
    MAX_HISTORY_MESSAGE_BYTES, MAX_HISTORY_MESSAGE_CHARS, MAX_PROMPT_MESSAGES,
    MaterializedPromptPlan, PromptPlanner,
};
use lorepia_domain::{
    ActivationRule, ApiFamily, BlockResolutionTrace, CapabilityKey, CapabilityValue, Character,
    CharacterContentV1, CharacterPromptContent, ContentCapability, ContentModule, ContentModuleId,
    ControlKind, ControlSpec, ConversationMode, GenerationPresetId, GenerationReasoningEffort,
    InteractionRuleSet, InteractionRuleSetId, KnowledgeBook, KnowledgeBookId, KnowledgeEntryId,
    MemoryJob, MemoryJobId, MemoryKind, MemoryProfile, MemoryProfileId, MemoryRecord,
    MemoryRecordId, Message, MessageId, MessageRole, ModuleBinding, ModuleBindingId,
    ModuleComponentRef, ModuleScope, OverflowTrace, PersonaId, PersonaPromptContent,
    PromptContextBindingEvidence, PromptContextPersonaEvidence, PromptContextSnapshotV1,
    PromptConversationMessage, PromptMemorySelectionEvidence, PromptMemorySelectionLane,
    PromptMemorySelectionReason, PromptMessageRole, PromptPreset, PromptPresetId,
    PromptResolutionContext, PromptResolutionTrace, PromptResolveRequest,
    PromptSummarySourceEvidence, ProviderMessageRole, ResolvedCacheDirective, ResolvedPromptPlan,
    RoleHint, RoleMappingTrace, SelectedKnowledge, SelectedMemory, SemanticKnowledgeScore,
    SourceKind, SummaryBoundary, TaskProfile, TaskProfileId, TemplateSlot, TransformRuleId,
    TransformSet, TransformSetId, ValidateOrchestration, VariableId, VariableMap, VariableRef,
    VariableScope, VariableType, VariableValue, VersionedJson, prompt_context_snapshot_sha256,
    prompt_local_user_id_sha256,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, KnowledgeEngine, KnowledgeSelection, KnowledgeSelectionContext,
    KnowledgeWorkBudget, MemoryEngine, MemorySelection, MemorySelectionContext,
    MemorySelectionLane, MemorySelectionReason, MemorySemanticScore, ModuleResolutionContext,
    TransformApplyOptions, TransformCompileOptions, TransformContext, TransformLimits,
    TransformPipeline, TransformResult, preview_transform_rule, reseal_prompt_resolution_evidence,
    reseal_resolved_prompt_plan, resolve_prompt_plan as resolve_prompt_plan_engine,
    validate_prompt_preset as validate_prompt_preset_document, verify_resolved_prompt_plan,
};
use lorepia_providers::parameter_mapping::PromptCacheWireDialect;
use lorepia_providers::{
    DeveloperRoleCapability, ProviderCacheBoundaryCompilation, ProviderCacheBoundaryDisposition,
    ProviderCompiledPromptPreview, ProviderPromptAdapterContract, ProviderPromptPlacement,
    ProviderWireRole,
};
use lorepia_storage::{
    ContentModuleRevisionDiff, GenerationPromptPlanRecord, GenerationPromptQuickSettingsAuthority,
    GenerationPromptSelectionAuthority, GenerationProviderTargetAuthority,
    InteractionKnowledgeBinding, KnowledgeActivationLog, KnowledgeEmbeddingMatch,
    KnowledgeEmbeddingQuery, MemoryInvalidationResult, ModuleRevisionComponentSnapshot,
    ObjectRevision, PromptPresetBinding, PromptPresetRevisionDiff, PromptPresetRollbackApproval,
    PromptPresetRollbackCommit, PromptPresetRollbackReview, PromptResponseLength,
    ProviderRequestSnapshotRecord, StoredInteractionState, StoredRevision, built_in_prompt_presets,
    generation_prompt_selection_authority_sha256, prompt_preset_rollback_approval_sha256,
};
use sha2::{Digest, Sha256};

use crate::{
    Core, Revisioned,
    orchestration_runtime::{
        MemorySemanticQueryEvidence, ResolvedMemorySemanticQuery, TaskCredentialBroker,
        apply_exact_transform_runtime_overlay, collect_exact_component_import_approvals,
    },
    revision::{project_revision, project_revisions},
};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId, GenerationTarget,
};
use uuid::Uuid;

/// Deterministic primary and fallback targets for one auxiliary task.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskGenerationTargetPlan {
    pub task_profile_id: TaskProfileId,
    pub targets: Vec<GenerationTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeTokenEstimate {
    pub entry_id: KnowledgeEntryId,
    pub tokens: u32,
}

/// Owned creator-tool input for deterministic knowledge activation simulation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeSimulationRequest {
    pub book_id: KnowledgeBookId,
    pub sample_texts: Vec<String>,
    pub manual_entry_ids: Vec<KnowledgeEntryId>,
    pub semantic_scores: Vec<SemanticKnowledgeScore>,
    pub variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub token_estimates: Vec<KnowledgeTokenEstimate>,
    pub activation_seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContentShareGate {
    pub module_id: ContentModuleId,
    pub local_use_allowed: bool,
    pub sharing_allowed: bool,
    pub reasons: Vec<String>,
}

/// Owned retrieval request; visible message ids define the complete active
/// branch lineage and are the authority for cross-branch ancestor sharing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemoryRetrievalRequest {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub profile_id: MemoryProfileId,
    pub visible_message_ids: Vec<MessageId>,
    /// Bounded local query text. Core derives lexical fallback scores or an
    /// exact configured embedding query; callers never provide scores.
    pub query_texts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TransformPreviewRequest {
    pub transform_set_id: TransformSetId,
    pub rule_id: TransformRuleId,
    pub input: String,
    pub variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub approved_import_source_ids: Vec<String>,
    pub allow_resolved_prompt: bool,
}

/// Safe, owned input used by both preview and generation preparation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PromptPlanRequest {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub expected_head: Option<MessageId>,
    pub user_text: String,
    pub generation_target: GenerationTarget,
    pub prompt_preset_id: Option<PromptPresetId>,
    pub variable_overrides: VariableMap,
    pub expected_plan_hash: Option<String>,
}

/// JSON-safe creator-control value. Core converts this high-level value only
/// through the selected preset's declared `ControlSpec -> VariableRef` binding;
/// callers never submit arbitrary variable references for room settings.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum CreatorControlValue {
    Bool(bool),
    Integer(i64),
    Decimal(f64),
    Text(String),
    StringList(Vec<String>),
}

/// Full desired room-scoped orchestration settings used by the CAS save.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOrchestrationConfigPatch {
    pub prompt_preset_id: Option<PromptPresetId>,
    pub generation_preset_id: Option<GenerationPresetId>,
    #[serde(default)]
    pub creator_values: BTreeMap<String, CreatorControlValue>,
    pub response_length: PromptResponseLength,
    pub creativity: u8,
    pub reasoning_effort: Option<GenerationReasoningEffort>,
    pub memory_enabled: bool,
    pub knowledge_enabled: bool,
    #[serde(default)]
    pub user_name_override: Option<String>,
    #[serde(default)]
    pub author_note: Option<String>,
    #[serde(default)]
    pub group_context: Option<String>,
    #[serde(default)]
    pub template_slots: Vec<TemplateSlot>,
}

/// Effective room settings. `binding_revision` is present only when this exact
/// branch owns a binding; inherited conversation/character/user/app settings
/// remain visible but save as a new branch binding with expected `None`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomOrchestrationConfig {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub prompt_preset_id: PromptPresetId,
    pub generation_preset_id: Option<GenerationPresetId>,
    /// Credential-free target that generation must use for this exact room.
    ///
    /// A prompt binding/default generation preset wins over the installation
    /// default. The route is read from the stored preset in Core so renderers
    /// never reproduce route/preset resolution rules.
    pub generation_target: Option<GenerationTarget>,
    pub creator_values: BTreeMap<String, CreatorControlValue>,
    /// Exact non-sensitive variable overrides represented by the effective
    /// binding. The renderer may review this value but cannot author arbitrary
    /// variable references through the room quick-settings API.
    pub variable_overrides: VariableMap,
    pub response_length: PromptResponseLength,
    pub creativity: u8,
    pub reasoning_effort: Option<GenerationReasoningEffort>,
    pub memory_enabled: bool,
    pub knowledge_enabled: bool,
    pub user_name_override: Option<String>,
    pub author_note: Option<String>,
    pub group_context: Option<String>,
    pub template_slots: Vec<TemplateSlot>,
    pub binding_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptPlanMessagePreview {
    pub sequence: u32,
    pub block_id: lorepia_domain::PromptBlockId,
    pub block_kind: lorepia_domain::PromptBlockKind,
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
    pub estimated_tokens: u32,
    pub source_message_ids: Vec<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptProviderMessagePreview {
    pub sequence: u32,
    pub block_id: lorepia_domain::PromptBlockId,
    pub effective_role: ProviderMessageRole,
    pub wire_role: ProviderWireRole,
    pub placement: ProviderPromptPlacement,
    pub estimated_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptEffectiveMessageContentPreview {
    pub sequence: u32,
    pub block_id: lorepia_domain::PromptBlockId,
    pub block_kind: lorepia_domain::PromptBlockKind,
    pub requested_role: RoleHint,
    pub effective_role: ProviderMessageRole,
    pub estimated_tokens: u32,
    pub source_message_ids: Vec<MessageId>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptAppliedParameterPreview {
    pub field: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptDiffEntry {
    pub sequence: u32,
    pub block_id: lorepia_domain::PromptBlockId,
    pub changes: Vec<String>,
}

/// Stable confirmation submitted after a user reviews an exact rollback.
///
/// `approval_id` is caller-stable so a retry after response loss can return
/// the already-applied revision. Core derives the approval hash and target
/// document; callers cannot submit either.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackApplyRequest {
    pub review: PromptPresetRollbackReview,
    pub approval_id: String,
    pub expected_review_sha256: String,
}

/// Durable rollback result. A rollback always appends a new immutable revision.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetRollbackReceipt {
    pub preset: Revisioned<PromptPreset>,
    pub approval: PromptPresetRollbackApproval,
}

/// Exact Rust-internal review material used to bind preview and send.
///
/// This type intentionally contains the final prompt and provider body, but
/// never an endpoint, header, credential, host path, or opaque provider state.
/// Application adapters must project it to an allowlisted, content-free DTO;
/// it is not a `WebView` or cross-process wire contract.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpertPromptPreview {
    /// Opaque durable attempt identity that must accompany a reviewed send.
    /// Core revalidates it against the exact request before dispatch.
    pub generation_attempt_id: GenerationId,
    pub plan: PromptPlanPreview,
    pub effective_messages: Vec<PromptEffectiveMessageContentPreview>,
    pub provider_request: serde_json::Value,
    pub applied_parameters: Vec<PromptAppliedParameterPreview>,
    pub prompt_diff: Vec<PromptDiffEntry>,
}

/// Redacted prompt preview. Prompt text, variable values, selected memory
/// content, credentials, and raw provider payloads are intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PromptPlanPreview {
    pub plan_id: String,
    /// Composite execution identity reviewed by the caller. This binds the
    /// neutral resolved prompt to exact preset revisions and provider mapping.
    pub plan_hash: String,
    /// Resolver-owned hash of `ResolvedPromptPlan`.
    pub neutral_plan_hash: String,
    pub prompt_preset_id: PromptPresetId,
    pub prompt_preset_revision: u64,
    pub prompt_preset_revision_id: String,
    pub generation_target: Option<GenerationTarget>,
    pub estimated_input_tokens: u32,
    pub available_input_tokens: u32,
    /// Exact estimator identity used by the neutral resolver.
    pub token_estimator_id: String,
    /// The current resolver estimator is a conservative fallback, not an
    /// exact model tokenizer.
    pub token_estimate_exact: bool,
    pub messages: Vec<PromptPlanMessagePreview>,
    pub provider_family: ApiFamily,
    pub provider_messages: Vec<PromptProviderMessagePreview>,
    pub provider_cache_boundaries: Vec<ProviderCacheBoundaryCompilation>,
    pub cache_directives: Vec<ResolvedCacheDirective>,
    pub blocks: Vec<BlockResolutionTrace>,
    pub role_mappings: Vec<RoleMappingTrace>,
    pub overflow: Vec<OverflowTrace>,
    pub warnings: Vec<String>,
}

pub(crate) struct GenerationPlanInput<'a> {
    pub character: &'a Character,
    pub conversation_id: &'a ConversationId,
    pub branch_id: &'a ConversationBranchId,
    /// Attempt-owned durable branch whose historical lineage is the authority
    /// for prompt context sources. A proposed action branch is intentionally
    /// distinct from this source branch until atomic materialization.
    pub context_source_branch_id: &'a ConversationBranchId,
    /// Attempt-owned exact pre-action head. This may be a historical ancestor
    /// after the source branch advances and must never be replaced by a newer
    /// mutable branch head during dispatch.
    pub context_head_message_id: Option<&'a MessageId>,
    /// Durable branch that owns the interaction state used during resolution.
    /// Message edit/regenerate prepares a not-yet-created action branch, so
    /// its source branch is used for the pre-generation interaction snapshot.
    pub interaction_state_branch_id: Option<&'a ConversationBranchId>,
    /// Exact read-only historical state used by a not-yet-materialized action
    /// branch. Storage rechecks this checkpoint during the atomic fork append.
    pub interaction_state_override: Option<&'a StoredInteractionState>,
    /// Exact context-specific module composition for a proposed branch.
    /// This plan remains read-only until the atomic action append persists it.
    pub applied_module_plan_override: Option<&'a AppliedModuleRuntimePlan>,
    /// Branch whose durable lineage bounds the memory candidates. The
    /// selection context still uses `branch_id`, so records copied from a
    /// source action branch are accepted only when their complete source range
    /// is visible at the fork point.
    pub memory_lineage_branch_id: Option<&'a ConversationBranchId>,
    pub mode: ConversationMode,
    pub history: &'a [Message],
    pub model: &'a str,
    pub generation_target: Option<&'a GenerationTarget>,
    pub provider_family: Option<ApiFamily>,
    pub temperature: Option<f64>,
    pub max_output_tokens: Option<u32>,
    pub prompt_preset_id: Option<&'a PromptPresetId>,
    /// Attempt-owned immutable prompt selection. Once present, prompt
    /// resolution must not re-read mutable room bindings, persona selection,
    /// or the current preset head.
    pub prompt_selection_authority: Option<&'a GenerationPromptSelectionAuthority>,
    /// Attempt whose immutable `BeforeGeneration` memory snapshot bounds
    /// summary materialization after approval or restart.
    pub generation_attempt_id: Option<&'a GenerationId>,
    pub variable_overrides: &'a VariableMap,
    pub expected_plan_hash: Option<&'a str>,
    /// Exact provider/request-plan snapshot used to build the provider that
    /// will receive this plan. When absent, preview resolves one read-only
    /// snapshot itself.
    pub prompt_wire_contract: Option<&'a crate::app::PromptRouteWireContract>,
    /// Stable attempt-owned temporal snapshot for current-date/time template
    /// variables.
    pub resolution_time: DateTime<Utc>,
    /// Stable attempt-owned seed recorded in the resolution trace and durable
    /// generation plan.
    pub session_seed: Option<u64>,
}

pub(crate) struct GenerationPromptAuthorityCapture<'a> {
    pub character: &'a Character,
    pub conversation_id: &'a ConversationId,
    pub branch_id: &'a ConversationBranchId,
    pub mode: ConversationMode,
    pub explicit_preset_id: Option<&'a PromptPresetId>,
    pub generation_target: Option<&'a GenerationTarget>,
    pub temperature: Option<f64>,
    pub max_output_tokens: Option<u32>,
    pub prompt_wire_contract: Option<&'a crate::app::PromptRouteWireContract>,
    pub provider_target_authority: GenerationProviderTargetAuthority,
}

pub(crate) struct AsyncPromptPlanPreparation<'a> {
    pub prompt_wire_contract: Option<&'a crate::app::PromptRouteWireContract>,
    pub interaction_state_override: Option<&'a StoredInteractionState>,
    pub applied_module_plan_override: Option<&'a AppliedModuleRuntimePlan>,
    pub prompt_selection_authority: Option<&'a GenerationPromptSelectionAuthority>,
    pub generation_attempt_id: Option<&'a GenerationId>,
    pub resolution_time: DateTime<Utc>,
    pub session_seed: u64,
    pub credential_broker: &'a dyn TaskCredentialBroker,
    pub cancelled: tokio::sync::watch::Receiver<bool>,
}

pub(crate) struct PromptPlanPreparation<'a> {
    pub prompt_wire_contract: Option<&'a crate::app::PromptRouteWireContract>,
    pub interaction_state_override: Option<&'a StoredInteractionState>,
    pub applied_module_plan_override: Option<&'a AppliedModuleRuntimePlan>,
    pub prompt_selection_authority: Option<&'a GenerationPromptSelectionAuthority>,
    pub generation_attempt_id: Option<&'a GenerationId>,
    pub resolution_time: DateTime<Utc>,
    pub session_seed: u64,
}

struct PreparedPromptPlanRequestContext {
    character: Character,
    mode: ConversationMode,
    history: Vec<Message>,
    model: String,
    api_family: ApiFamily,
    resolution_time: DateTime<Utc>,
    session_seed: u64,
}

impl PreparedPromptPlanRequestContext {
    fn generation_input<'a>(
        &'a self,
        request: &'a PromptPlanRequest,
        prompt_wire_contract: Option<&'a crate::app::PromptRouteWireContract>,
        interaction_state_override: Option<&'a StoredInteractionState>,
        applied_module_plan_override: Option<&'a AppliedModuleRuntimePlan>,
        prompt_selection_authority: Option<&'a GenerationPromptSelectionAuthority>,
        generation_attempt_id: Option<&'a GenerationId>,
    ) -> GenerationPlanInput<'a> {
        let character =
            prompt_selection_authority.map_or(&self.character, |authority| &authority.character);
        let mode = prompt_selection_authority.map_or(self.mode, |authority| authority.mode);
        GenerationPlanInput {
            character,
            conversation_id: &request.conversation_id,
            branch_id: &request.branch_id,
            context_source_branch_id: &request.branch_id,
            context_head_message_id: request.expected_head.as_ref(),
            interaction_state_branch_id: None,
            interaction_state_override,
            applied_module_plan_override,
            memory_lineage_branch_id: None,
            mode,
            history: &self.history,
            model: &self.model,
            generation_target: Some(&request.generation_target),
            provider_family: Some(self.api_family),
            temperature: None,
            max_output_tokens: None,
            prompt_preset_id: request.prompt_preset_id.as_ref(),
            prompt_selection_authority,
            generation_attempt_id,
            variable_overrides: &request.variable_overrides,
            expected_plan_hash: request.expected_plan_hash.as_deref(),
            prompt_wire_contract,
            resolution_time: self.resolution_time,
            session_seed: Some(self.session_seed),
        }
    }
}

pub(crate) struct PreparedGenerationPlan {
    pub materialized: MaterializedPromptPlan,
    pub preview: PromptPlanPreview,
    pub prompt_preset_revision_id: String,
    pub execution_hash: String,
    pub transform_sets: Vec<TransformSet>,
    /// Exact immutable set revisions used by every transform phase. This
    /// content-free map is sealed into the generation prompt-plan diagnostics
    /// so terminal application logs can satisfy their revision foreign keys
    /// without consulting a newer active revision.
    pub transform_set_revisions: BTreeMap<TransformSetId, String>,
    pub variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub knowledge_logs: Vec<KnowledgeActivationLog>,
    pub approved_import_source_ids: BTreeSet<String>,
    pub display_context: PromptResolutionContext,
    pub module_plan_sha256: Option<String>,
    pub cacheable_prefix_tokens: u32,
    pub tokenizer_id: String,
    pub tokenizer_version: String,
    pub memory_semantic_evidence: Option<MemorySemanticQueryEvidence>,
    pub knowledge_semantic_evidence: Vec<KnowledgeSemanticBookEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KnowledgeSemanticBookEvidence {
    pub book_id: KnowledgeBookId,
    pub book_revision_id: String,
    pub source: KnowledgeSemanticScoreSourceEvidence,
    pub semantic_entry_count: u32,
    pub scores_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum KnowledgeSemanticScoreSourceEvidence {
    LexicalV1 {
        query_sha256: String,
    },
    ProviderEmbeddingV1 {
        memory_profile_revision_id: String,
        task_profile_revision_id: String,
        model_route_id: lorepia_domain::ModelRouteId,
        dimensions: u32,
        vector_space_sha256: String,
        query_sha256: String,
        query_embedding_id: String,
        query_embedding_revision: u64,
        query_vector_sha256: String,
        matches_sha256: String,
    },
}

struct PromptProviderResolution {
    contract: lorepia_domain::ProviderPromptContract,
    adapter: ProviderPromptAdapterContract,
    developer_capability: DeveloperRoleCapability,
    cache_dialect: PromptCacheWireDialect,
    max_context_tokens: u32,
    reserved_output_tokens: u32,
    reasoning_effort_applied: Option<GenerationReasoningEffort>,
    request_plan_sha256: String,
    generation_preset_sha256: String,
}

struct PromptProviderWireMetadata {
    developer_capability: DeveloperRoleCapability,
    cache_dialect: PromptCacheWireDialect,
    request_plan_sha256: String,
    generation_preset_sha256: String,
}

struct PromptPersonaMaterialization {
    content: PersonaPromptContent,
    evidence: PromptContextPersonaEvidence,
}

type PromptPresetSelection = (
    PromptPreset,
    u64,
    String,
    Option<StoredRevision<PromptPresetBinding>>,
    Option<StoredRevision<lorepia_domain::ConversationPersonaSelection>>,
);

struct PromptSummaryMaterialization {
    boundaries: Vec<SummaryBoundary>,
    conversation_summary: Option<String>,
    conversation_summary_id: Option<MemoryRecordId>,
    evidence: Vec<PromptSummarySourceEvidence>,
}

#[derive(Clone)]
struct VisiblePromptSummary {
    record: MemoryRecord,
    evidence: PromptSummarySourceEvidence,
    end_depth: u64,
}

struct PromptContextMaterialization {
    user_name: String,
    author_note: Option<String>,
    group_context: Option<String>,
    slots: Vec<TemplateSlot>,
    summaries: PromptSummaryMaterialization,
    snapshot: PromptContextSnapshotV1,
}

struct PromptQuickSettings {
    temperature: Option<f64>,
    max_output_tokens: Option<u32>,
    response_length: PromptResponseLength,
    creativity: u8,
    reasoning_effort: Option<GenerationReasoningEffort>,
    memory_enabled: bool,
    knowledge_enabled: bool,
    warnings: Vec<String>,
}

struct PromptPresetPreparation {
    preset: PromptPreset,
    revision: u64,
    revision_id: String,
    binding: Option<StoredRevision<PromptPresetBinding>>,
    prompt_persona: Option<PromptPersonaMaterialization>,
    prompt_knowledge_books: Vec<ObjectRevision<KnowledgeBook>>,
    prompt_transform_sets: Vec<ObjectRevision<TransformSet>>,
    prompt_memory_profile: Option<ObjectRevision<MemoryProfile>>,
    block_source_revisions: BTreeMap<lorepia_domain::PromptBlockId, String>,
    module_overlay: PromptModuleOverlay,
    warnings: Vec<String>,
}

struct PromptVariableState {
    variables: VariableMap,
    manually_active_knowledge: BTreeSet<KnowledgeEntryId>,
}

struct PromptTransformPreparation {
    transform_sets: Vec<TransformSet>,
    transform_set_revisions: BTreeMap<TransformSetId, String>,
    approved_import_source_ids: BTreeSet<String>,
    supported_capabilities: Vec<CapabilityKey>,
    transformed_latest: TransformResult,
}

struct PromptConversationPreparation {
    character_content: CharacterContentV1,
    prompt_character: CharacterPromptContent,
    prompt_messages: Vec<PromptConversationMessage>,
    scan_texts: Vec<String>,
}

struct PromptSelectionInput<'a> {
    preset: &'a PromptPreset,
    character_content: &'a CharacterContentV1,
    prompt_knowledge_books: &'a [ObjectRevision<KnowledgeBook>],
    module_knowledge_books: &'a [ObjectRevision<KnowledgeBook>],
    exact_character_knowledge_book: Option<&'a StoredRevision<KnowledgeBook>>,
    memory_profile: Option<&'a ObjectRevision<MemoryProfile>>,
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    memory_lineage_branch_id: Option<&'a ConversationBranchId>,
    memory_context_head_message_id: Option<&'a MessageId>,
    generation_attempt_id: Option<&'a GenerationId>,
    prompt_messages: &'a [PromptConversationMessage],
    scan_texts: &'a [String],
    manually_active_knowledge: &'a BTreeSet<KnowledgeEntryId>,
    variables: &'a VariableMap,
    supported_capabilities: &'a [CapabilityKey],
    resolved_memory_semantics: Option<&'a ResolvedMemorySemanticQuery>,
    activation_seed: u64,
    resolution_time: DateTime<Utc>,
    knowledge_enabled: bool,
    memory_enabled: bool,
}

struct PromptSelectionPreparation {
    selected_knowledge: Vec<SelectedKnowledge>,
    knowledge_logs: Vec<KnowledgeActivationLog>,
    knowledge_semantic_evidence: Vec<KnowledgeSemanticBookEvidence>,
    selected_memory: Vec<SelectedMemory>,
    memory_evidence: Vec<PromptMemorySelectionEvidence>,
    warnings: Vec<String>,
}

struct PromptPlanSources {
    preset: PromptPresetPreparation,
    quick_settings: PromptQuickSettings,
    variables: PromptVariableState,
    transforms: PromptTransformPreparation,
    conversation: PromptConversationPreparation,
    selection: PromptSelectionPreparation,
}

struct PromptPlanAssembly {
    request: PromptResolveRequest,
    provider_resolution: PromptProviderResolution,
    prompt_preset_revision: u64,
    prompt_preset_revision_id: String,
    block_source_revisions: BTreeMap<lorepia_domain::PromptBlockId, String>,
    quick_settings: PromptQuickSettings,
    transform_sets: Vec<TransformSet>,
    transform_set_revisions: BTreeMap<TransformSetId, String>,
    approved_import_source_ids: BTreeSet<String>,
    variables: VariableMap,
    supported_capabilities: Vec<CapabilityKey>,
    knowledge_logs: Vec<KnowledgeActivationLog>,
    knowledge_semantic_evidence: Vec<KnowledgeSemanticBookEvidence>,
    memory_evidence: Vec<PromptMemorySelectionEvidence>,
    module_overlay: PromptModuleOverlay,
    preparation_warnings: Vec<String>,
}

struct ResolvedPromptAssembly {
    plan: ResolvedPromptPlan,
    provider_preview: ProviderCompiledPromptPreview,
    execution_hash: String,
    cacheable_prefix_tokens: u32,
}

struct PromptMemorySource {
    profile: MemoryProfile,
    records: Vec<MemoryRecord>,
}

#[derive(Default)]
struct PromptModuleOverlay {
    plan_sha256: Option<String>,
    prompt_blocks: Vec<lorepia_domain::PromptBlock>,
    prompt_block_source_revisions: BTreeMap<lorepia_domain::PromptBlockId, String>,
    controls: Vec<ControlSpec>,
    knowledge_books: Vec<ObjectRevision<KnowledgeBook>>,
    transform_sets: Vec<ObjectRevision<TransformSet>>,
    variables: VariableMap,
    approved_import_source_ids: BTreeSet<String>,
    warnings: Vec<String>,
}

struct PromptModuleOverlayInput<'a> {
    character: &'a Character,
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    persona_id: Option<&'a PersonaId>,
    applied_plan_override: Option<&'a AppliedModuleRuntimePlan>,
    sealed_local_user_id_sha256: Option<&'a str>,
    generation_attempt_id: Option<&'a GenerationId>,
}

pub(crate) struct KnowledgeSemanticProviderRequirement {
    pub book_revision_id: String,
    pub entry_ids: Vec<KnowledgeEntryId>,
}

impl PreparedGenerationPlan {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn generation_prompt_plan_record(
        &self,
        generation_id: GenerationId,
        conversation_id: ConversationId,
        branch_id: ConversationBranchId,
        head_message_id: Option<MessageId>,
        latest_user_message_id: MessageId,
        generation_target: Option<&GenerationTarget>,
        provider_request_value: serde_json::Value,
        created_at: DateTime<Utc>,
    ) -> CoreResult<GenerationPromptPlanRecord> {
        let plan = self
            .materialized
            .request
            .resolved_prompt_plan
            .as_ref()
            .ok_or_else(|| CoreError::internal("prepared generation is missing its prompt plan"))?;
        verify_resolved_prompt_plan(plan).map_err(orchestration_validation_error)?;
        let plan_value = serde_json::to_value(plan).map_err(|error| {
            CoreError::internal(format!("cannot encode resolved prompt plan: {error}"))
        })?;
        let diagnostics_value = serde_json::json!({
            "execution_hash": &self.execution_hash,
            "neutral_plan_hash": &plan.plan_hash,
            "provider_family": self.preview.provider_family,
            "provider_messages": &self.preview.provider_messages,
            "provider_cache_boundaries": &self.preview.provider_cache_boundaries,
            "module_plan_sha256": &self.module_plan_sha256,
            "transform_set_revisions": self.transform_set_revisions.iter().map(
                |(set_id, revision_id)| serde_json::json!({
                    "set_id": set_id.as_str(),
                    "revision_id": revision_id,
                })
            ).collect::<Vec<_>>(),
            "memory_semantic_evidence": &self.memory_semantic_evidence,
            "knowledge_semantic_evidence": &self.knowledge_semantic_evidence,
            "cacheable_prefix_tokens": self.cacheable_prefix_tokens,
            "warnings": &self.preview.warnings,
        });
        Ok(GenerationPromptPlanRecord {
            id: self.execution_hash.clone(),
            generation_id: generation_id.clone(),
            conversation_id,
            branch_id,
            head_message_id,
            latest_user_message_id,
            prompt_preset_id: plan.preset_id.clone(),
            prompt_preset_revision_id: self.prompt_preset_revision_id.clone(),
            model_route_id: generation_target.map(|target| target.model_route_id.clone()),
            generation_preset_id: generation_target
                .map(|target| target.generation_preset_id.clone()),
            task_profile_revision_id: None,
            random_seed: plan.trace.session_seed,
            tokenizer_id: self.tokenizer_id.clone(),
            tokenizer_version: self.tokenizer_version.clone(),
            plan: VersionedJson {
                schema_version: plan.schema_version,
                value: plan_value,
            },
            plan_sha256: plan.plan_hash.clone(),
            input_fingerprint_sha256: self.execution_hash.clone(),
            context_limit_tokens: plan.trace.max_context_tokens,
            estimated_input_tokens: plan.trace.estimated_input_tokens,
            reserved_output_tokens: plan.trace.reserved_output_tokens,
            final_input_tokens: plan.trace.estimated_input_tokens,
            cacheable_prefix_tokens: self.cacheable_prefix_tokens,
            provider_request: ProviderRequestSnapshotRecord {
                id: format!("provider-request:{}", generation_id.0),
                api_family: self.preview.provider_family,
                request_schema_version: 1,
                request: VersionedJson {
                    schema_version: 1,
                    value: provider_request_value,
                },
                mapping_diagnostics: VersionedJson {
                    schema_version: 1,
                    value: diagnostics_value,
                },
                created_at,
            },
            created_at,
        })
    }
}

impl Core {
    pub fn get_generation_prompt_plan(
        &self,
        generation_id: &GenerationId,
    ) -> CoreResult<GenerationPromptPlanRecord> {
        self.storage()
            .get_generation_prompt_plan_by_generation(generation_id)
    }

    /// Resolves a redacted preview through the same attempt-owned
    /// `BeforeGeneration`, provider-wire, and prompt preparation path used by
    /// reviewed send. No prompt body or retrieved private content crosses the
    /// Core API, and the staged attempt remains isolated from live branch
    /// interaction state until the atomic generation append.
    pub fn render_prompt_preview(
        &self,
        request: &PromptPlanRequest,
        operation_context: crate::GenerationOperationContext<'_>,
    ) -> CoreResult<PromptPlanPreview> {
        self.prepare_reviewed_prompt_plan_for_core(request, operation_context)
            .map(|prepared| prepared.preview)
    }

    /// Alias with execution-oriented naming for creator and diagnostics tools.
    pub fn resolve_prompt_plan(
        &self,
        request: &PromptPlanRequest,
        operation_context: crate::GenerationOperationContext<'_>,
    ) -> CoreResult<PromptPlanPreview> {
        self.render_prompt_preview(request, operation_context)
    }

    /// Re-resolves and explains an immutable plan identity.
    pub fn explain_prompt_plan(
        &self,
        request: &PromptPlanRequest,
        operation_context: crate::GenerationOperationContext<'_>,
        expected_plan_hash: &str,
    ) -> CoreResult<PromptResolutionTrace> {
        let mut request = request.clone();
        request.expected_plan_hash = Some(expected_plan_hash.to_owned());
        let prepared = self.prepare_reviewed_prompt_plan_for_core(&request, operation_context)?;
        let trace = &prepared
            .materialized
            .request
            .resolved_prompt_plan
            .as_ref()
            .ok_or_else(|| CoreError::internal("prepared prompt has no resolved plan"))?
            .trace;
        Ok(PromptResolutionTrace {
            estimator_id: trace.estimator_id.clone(),
            session_seed: trace.session_seed,
            context_snapshot: trace.context_snapshot.clone(),
            max_context_tokens: trace.max_context_tokens,
            reserved_output_tokens: trace.reserved_output_tokens,
            available_input_tokens: prepared.preview.available_input_tokens,
            estimated_input_tokens: prepared.preview.estimated_input_tokens,
            blocks: prepared.preview.blocks,
            role_mappings: prepared.preview.role_mappings,
            overflow: prepared.preview.overflow,
            warnings: prepared.preview.warnings,
        })
    }

    pub(crate) fn prepare_prompt_plan_request_with_wire_contract(
        &self,
        request: &PromptPlanRequest,
        preparation: PromptPlanPreparation<'_>,
    ) -> CoreResult<PreparedGenerationPlan> {
        let context = self.prepare_prompt_plan_request_context(
            request,
            preparation.prompt_wire_contract,
            preparation.prompt_selection_authority,
            preparation.resolution_time,
            preparation.session_seed,
        )?;
        self.prepare_generation_plan(context.generation_input(
            request,
            preparation.prompt_wire_contract,
            preparation.interaction_state_override,
            preparation.applied_module_plan_override,
            preparation.prompt_selection_authority,
            preparation.generation_attempt_id,
        ))
    }

    pub(crate) async fn prepare_prompt_plan_request_with_wire_contract_async(
        &self,
        request: &PromptPlanRequest,
        preparation: AsyncPromptPlanPreparation<'_>,
    ) -> CoreResult<PreparedGenerationPlan> {
        let context = self.prepare_prompt_plan_request_context(
            request,
            preparation.prompt_wire_contract,
            preparation.prompt_selection_authority,
            preparation.resolution_time,
            preparation.session_seed,
        )?;
        self.prepare_generation_plan_async(
            context.generation_input(
                request,
                preparation.prompt_wire_contract,
                preparation.interaction_state_override,
                preparation.applied_module_plan_override,
                preparation.prompt_selection_authority,
                preparation.generation_attempt_id,
            ),
            preparation.credential_broker,
            preparation.cancelled,
        )
        .await
    }

    fn prepare_prompt_plan_request_context(
        &self,
        request: &PromptPlanRequest,
        prompt_wire_contract: Option<&crate::app::PromptRouteWireContract>,
        prompt_selection_authority: Option<&GenerationPromptSelectionAuthority>,
        resolution_time: DateTime<Utc>,
        session_seed: u64,
    ) -> CoreResult<PreparedPromptPlanRequestContext> {
        let user_text = validate_prompt_user_text(&request.user_text)?;
        let conversation = self.storage().get_conversation(&request.conversation_id)?;
        let (character, mode) = if let Some(authority) = prompt_selection_authority {
            generation_prompt_selection_authority_sha256(authority)?;
            if authority.character.id != conversation.character_id {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "attempt prompt character differs from its conversation",
                    false,
                ));
            }
            (authority.character.clone(), authority.mode)
        } else {
            (
                self.storage().get_character(&conversation.character_id)?,
                self.storage()
                    .get_conversation_state(&request.conversation_id)?
                    .selected_mode,
            )
        };
        let branch = self.storage().get_conversation_branch(&request.branch_id)?;
        if branch.conversation_id != request.conversation_id {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        if branch.head_message_id != request.expected_head {
            return Err(CoreError::invalid(
                "conversation branch head changed before prompt resolution",
            ));
        }
        let (model, api_family) = if let Some(contract) = prompt_wire_contract {
            if contract.model_route_id != request.generation_target.model_route_id
                || contract.generation_preset_id != request.generation_target.generation_preset_id
            {
                return Err(CoreError::internal(
                    "provider snapshot does not match the requested generation target",
                ));
            }
            (contract.model.clone(), contract.api_family)
        } else {
            let route = self
                .storage()
                .get_model_route(&request.generation_target.model_route_id)?;
            let generation_preset = self
                .storage()
                .get_generation_preset(&request.generation_target.generation_preset_id)?;
            if generation_preset.model_route_id != route.id {
                return Err(CoreError::invalid(
                    "generation preset does not belong to the selected model route",
                ));
            }
            (route.model_id, route.api_family)
        };
        let user_message_id = deterministic_prompt_user_message_id(
            &request.conversation_id,
            &request.branch_id,
            request.expected_head.as_ref(),
            user_text,
        );
        let mut user_message = Message::user_after(
            request.conversation_id.clone(),
            request.expected_head.clone(),
            user_text,
        );
        user_message.id = user_message_id;
        let mut history = self.storage().list_recent_branch_messages_for_prompt(
            &request.branch_id,
            MAX_PROMPT_MESSAGES.saturating_sub(2),
            MAX_HISTORY_MESSAGE_BYTES,
            MAX_HISTORY_MESSAGE_CHARS,
        )?;
        history.push(user_message);
        Ok(PreparedPromptPlanRequestContext {
            character,
            mode,
            history,
            model,
            api_family,
            resolution_time,
            session_seed,
        })
    }

    pub(crate) fn prepare_generation_plan(
        &self,
        input: GenerationPlanInput<'_>,
    ) -> CoreResult<PreparedGenerationPlan> {
        let mut knowledge_work_budget = KnowledgeWorkBudget::default();
        self.prepare_generation_plan_with_memory(input, None, &mut knowledge_work_budget)
    }

    /// Provider-aware prepare path shared by preview and every generation
    /// action. The only asynchronous pre-stage is the durable, exactly-once
    /// query-embedding intent; final prompt resolution still executes once
    /// through `prepare_generation_plan_with_memory`.
    pub(crate) async fn prepare_generation_plan_async(
        &self,
        input: GenerationPlanInput<'_>,
        credential_broker: &dyn TaskCredentialBroker,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> CoreResult<PreparedGenerationPlan> {
        let mut knowledge_work_budget = KnowledgeWorkBudget::default();
        let semantic_query = self
            .prepare_generation_memory_semantic_query(
                &input,
                credential_broker,
                cancelled,
                &mut knowledge_work_budget,
            )
            .await?;
        self.prepare_generation_plan_with_memory(
            input,
            semantic_query.as_ref(),
            &mut knowledge_work_budget,
        )
    }

    #[allow(clippy::too_many_lines)]
    async fn prepare_generation_memory_semantic_query(
        &self,
        input: &GenerationPlanInput<'_>,
        credential_broker: &dyn TaskCredentialBroker,
        cancelled: tokio::sync::watch::Receiver<bool>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<Option<ResolvedMemorySemanticQuery>> {
        let latest = input
            .history
            .last()
            .filter(|message| message.role == MessageRole::User)
            .ok_or_else(|| CoreError::invalid("prompt history must end with a user message"))?;
        let (preset, _, prompt_preset_revision_id, binding, persona_selection) =
            self.resolve_generation_prompt_selection(input)?;
        let memory_enabled = input.prompt_selection_authority.map_or_else(
            || {
                binding
                    .as_ref()
                    .is_none_or(|binding| binding.value.memory_enabled)
            },
            |authority| authority.quick_settings.memory_enabled,
        );
        let knowledge_enabled = input.prompt_selection_authority.map_or_else(
            || {
                binding
                    .as_ref()
                    .is_none_or(|binding| binding.value.knowledge_enabled)
            },
            |authority| authority.quick_settings.knowledge_enabled,
        );
        let semantic_requirements = if knowledge_enabled {
            self.prompt_semantic_knowledge_requirements(
                input,
                &preset,
                &prompt_preset_revision_id,
                persona_selection
                    .as_ref()
                    .map(|selection| &selection.value.persona_id),
            )?
        } else {
            Vec::new()
        };
        if (!memory_enabled && semantic_requirements.is_empty())
            || preset.memory_profile_id.is_none()
        {
            return Ok(None);
        }
        let exact_profile = self
            .storage()
            .get_prompt_preset_memory_profile_revision(&prompt_preset_revision_id)?
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "prompt preset memory profile dependency is missing its exact revision",
                    false,
                )
            })?;
        if preset.memory_profile_id.as_ref() != Some(&exact_profile.value.id) {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "prompt preset memory profile identity differs from its exact revision",
                false,
            ));
        }
        if exact_profile.value.embedding_task.is_none() {
            return Ok(None);
        }
        let lineage_branch_id = input.memory_lineage_branch_id.unwrap_or(input.branch_id);
        let records = if memory_enabled {
            let selection = match input.generation_attempt_id {
                Some(generation_id) => self.load_generation_attempt_memory_selection(
                    generation_id,
                    input.conversation_id,
                    lineage_branch_id,
                    input.context_head_message_id,
                )?,
                None => self.storage().list_memory_records_at_head(
                    input.conversation_id,
                    lineage_branch_id,
                    input.context_head_message_id,
                    false,
                )?,
            };
            selection
                .records
                .into_iter()
                .map(|stored| stored.value)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let query_texts = input
            .history
            .iter()
            .filter(|message| message.role != MessageRole::System)
            .rev()
            .take(32)
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        // Preview's deterministic user message is not yet durable. Both
        // preview and send therefore bind the query intent to the same
        // pre-action lineage anchor (the user message's parent), while the
        // canonical query hash still binds the complete hypothetical input.
        let Some(lineage_anchor) = latest.parent_id.as_ref() else {
            // A read-only first-turn preview has no durable message authority
            // to own an exactly-once provider intent. Return exact provider
            // profile evidence with an empty memory result so memory remains
            // valid, while knowledge receives the bounded deterministic
            // lexical scorer because no raw query vector is present.
            if !records.is_empty() {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "root prompt history unexpectedly has visible memory records",
                    false,
                ));
            }
            return self
                .resolve_memory_semantic_scores(
                    &exact_profile,
                    input.conversation_id,
                    lineage_branch_id,
                    &latest.id,
                    &latest.id,
                    &records,
                    &query_texts,
                    &[],
                    credential_broker,
                    cancelled,
                    knowledge_work_budget,
                )
                .await
                .map(Some);
        };
        self.resolve_memory_semantic_scores(
            &exact_profile,
            input.conversation_id,
            lineage_branch_id,
            lineage_anchor,
            lineage_anchor,
            &records,
            &query_texts,
            &semantic_requirements,
            credential_broker,
            cancelled,
            knowledge_work_budget,
        )
        .await
        .map(Some)
    }

    fn prompt_semantic_knowledge_requirements(
        &self,
        input: &GenerationPlanInput<'_>,
        preset: &PromptPreset,
        prompt_preset_revision_id: &str,
        persona_id: Option<&PersonaId>,
    ) -> CoreResult<Vec<KnowledgeSemanticProviderRequirement>> {
        let prompt_books = self
            .storage()
            .get_prompt_preset_knowledge_book_revisions(prompt_preset_revision_id)?
            .into_iter()
            .map(|revision| (revision.value.id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        let module_books = self
            .resolve_prompt_module_overlay(
                preset,
                prompt_preset_revision_id,
                PromptModuleOverlayInput {
                    character: input
                        .prompt_selection_authority
                        .map_or(input.character, |authority| &authority.character),
                    conversation_id: input.conversation_id,
                    branch_id: input.branch_id,
                    persona_id,
                    applied_plan_override: input.applied_module_plan_override,
                    sealed_local_user_id_sha256: input
                        .prompt_selection_authority
                        .map(|authority| authority.local_user_id_sha256.as_str()),
                    generation_attempt_id: input.generation_attempt_id,
                },
            )?
            .knowledge_books
            .into_iter()
            .map(|revision| (revision.value.id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        let character_content = if let Some(authority) = input.prompt_selection_authority {
            authority
                .character_content
                .as_ref()
                .map_or_else(CharacterContentV1::default, |stored| stored.value.clone())
        } else {
            match self.storage().get_character_content(&input.character.id) {
                Ok(stored) => stored.value,
                Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => {
                    CharacterContentV1::default()
                }
                Err(error) => return Err(error),
            }
        };
        let mut book_ids = preset
            .knowledge_book_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if let Some(book_id) = character_content
            .knowledge_book
            .as_ref()
            .and_then(|reference| reference.id.as_ref())
        {
            book_ids.insert(book_id.clone());
        }
        book_ids.extend(module_books.keys().cloned());
        let mut requirements = Vec::new();
        for book_id in book_ids {
            let (book, book_revision_id) = if let Some(revision) = module_books.get(&book_id) {
                (revision.value.clone(), revision.revision_id.clone())
            } else if let Some(revision) = prompt_books.get(&book_id) {
                (revision.value.clone(), revision.revision_id.clone())
            } else if let Some(revision) = input
                .prompt_selection_authority
                .and_then(|authority| authority.character_knowledge_book.as_ref())
                .filter(|revision| revision.value.id == book_id)
            {
                let revision_id = revision.revision_id.clone().ok_or_else(|| {
                    CoreError::new(
                        lorepia_domain::CoreErrorCode::StorageCorrupted,
                        "sealed character knowledge book is missing its exact revision",
                        false,
                    )
                })?;
                (revision.value.clone(), revision_id)
            } else {
                let stored = self.get_knowledge_book(&book_id)?;
                let revision_id = stored.revision_id.ok_or_else(|| {
                    CoreError::internal(
                        "semantic knowledge book is missing its immutable revision identity",
                    )
                })?;
                (stored.value, revision_id)
            };
            let entry_ids = book
                .entries
                .iter()
                .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>();
            if !entry_ids.is_empty() {
                requirements.push(KnowledgeSemanticProviderRequirement {
                    book_revision_id,
                    entry_ids,
                });
            }
        }
        Ok(requirements)
    }

    fn prepare_generation_plan_with_memory(
        &self,
        input: GenerationPlanInput<'_>,
        resolved_memory_semantics: Option<&ResolvedMemorySemanticQuery>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<PreparedGenerationPlan> {
        let activation_seed = input.session_seed.ok_or_else(|| {
            CoreError::internal("generation prompt resolution is missing its attempt-owned seed")
        })?;
        let latest = input
            .history
            .last()
            .filter(|message| message.role == MessageRole::User)
            .ok_or_else(|| CoreError::invalid("prompt history must end with a user message"))?;
        if input
            .history
            .iter()
            .any(|message| message.conversation_id != *input.conversation_id)
        {
            return Err(CoreError::invalid(
                "prompt history contains a message from another conversation",
            ));
        }
        let sources = self.prepare_prompt_plan_sources(
            &input,
            resolved_memory_semantics,
            latest,
            activation_seed,
            knowledge_work_budget,
        )?;
        let mut assembly = self.assemble_prompt_plan(&input, latest, sources)?;
        let resolved =
            Self::resolve_prompt_plan_assembly(&input, resolved_memory_semantics, &mut assembly)?;
        Self::materialize_prompt_plan_assembly(
            &input,
            resolved_memory_semantics,
            assembly,
            resolved,
        )
    }

    fn prepare_prompt_plan_sources(
        &self,
        input: &GenerationPlanInput<'_>,
        resolved_memory_semantics: Option<&ResolvedMemorySemanticQuery>,
        latest: &Message,
        activation_seed: u64,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<PromptPlanSources> {
        let mut preset = self.prepare_prompt_preset_sources(input)?;
        let binding = preset.binding.as_ref().map(|stored| &stored.value);
        let quick_settings = self.resolve_prompt_quick_settings(binding, input)?;
        preset
            .warnings
            .extend(quick_settings.warnings.iter().cloned());
        let variables = self.resolve_prompt_variable_state(
            &preset.preset,
            binding,
            &preset.module_overlay,
            input,
        )?;
        let transforms = self.prepare_prompt_transforms(
            &preset.prompt_transform_sets,
            &preset.module_overlay,
            input,
            latest,
            &variables.variables,
        )?;
        let conversation =
            self.prepare_prompt_conversation(input, latest, &transforms.transformed_latest)?;
        let selection = self.prepare_prompt_selections(
            PromptSelectionInput {
                preset: &preset.preset,
                character_content: &conversation.character_content,
                prompt_knowledge_books: &preset.prompt_knowledge_books,
                module_knowledge_books: &preset.module_overlay.knowledge_books,
                exact_character_knowledge_book: input
                    .prompt_selection_authority
                    .and_then(|authority| authority.character_knowledge_book.as_ref()),
                memory_profile: preset.prompt_memory_profile.as_ref(),
                conversation_id: input.conversation_id,
                branch_id: input.branch_id,
                memory_lineage_branch_id: input.memory_lineage_branch_id,
                memory_context_head_message_id: input.context_head_message_id,
                generation_attempt_id: input.generation_attempt_id,
                prompt_messages: &conversation.prompt_messages,
                scan_texts: &conversation.scan_texts,
                manually_active_knowledge: &variables.manually_active_knowledge,
                variables: &variables.variables,
                supported_capabilities: &transforms.supported_capabilities,
                resolved_memory_semantics,
                activation_seed,
                resolution_time: input.resolution_time,
                knowledge_enabled: quick_settings.knowledge_enabled,
                memory_enabled: quick_settings.memory_enabled,
            },
            knowledge_work_budget,
        )?;
        preset.warnings.extend(selection.warnings.iter().cloned());
        Ok(PromptPlanSources {
            preset,
            quick_settings,
            variables,
            transforms,
            conversation,
            selection,
        })
    }

    fn assemble_prompt_plan(
        &self,
        input: &GenerationPlanInput<'_>,
        latest: &Message,
        sources: PromptPlanSources,
    ) -> CoreResult<PromptPlanAssembly> {
        let PromptPlanSources {
            preset,
            quick_settings,
            variables,
            transforms,
            conversation,
            selection,
        } = sources;
        let prompt_context = self.materialize_prompt_context_sources(
            &preset.preset,
            preset.binding.as_ref(),
            preset.prompt_persona.as_ref(),
            input.conversation_id,
            input.branch_id,
            input.context_source_branch_id,
            input.context_head_message_id,
            latest.parent_id.as_ref(),
            &conversation.prompt_messages,
            input.generation_attempt_id,
            input
                .prompt_selection_authority
                .map(|authority| authority.local_user_id_sha256.as_str()),
        )?;
        let (provider_resolution, preparation_warnings) =
            self.resolve_prompt_provider_and_warnings(input, &quick_settings, preset.warnings)?;
        let PromptSelectionPreparation {
            selected_knowledge,
            knowledge_logs,
            knowledge_semantic_evidence,
            selected_memory,
            memory_evidence,
            ..
        } = selection;
        let PromptConversationPreparation {
            prompt_character,
            prompt_messages,
            ..
        } = conversation;
        let persona = preset
            .prompt_persona
            .as_ref()
            .map(|materialized| materialized.content.clone());
        let request = PromptResolveRequest {
            preset: preset.preset,
            context: PromptResolutionContext {
                conversation_id: input.conversation_id.clone(),
                branch_id: input.branch_id.clone(),
                character: prompt_character,
                persona,
                user_name: prompt_context.user_name,
                messages: prompt_messages,
                latest_user_message_id: latest.id.clone(),
                selected_knowledge,
                selected_memory,
                summary_boundaries: prompt_context.summaries.boundaries,
                conversation_summary: prompt_context.summaries.conversation_summary,
                author_note: prompt_context.author_note,
                group_context: prompt_context.group_context,
                variables: variables.variables.clone(),
                slots: prompt_context.slots,
                current_date: input.resolution_time.format("%Y-%m-%d").to_string(),
                current_time: input.resolution_time.format("%H:%M:%S%:z").to_string(),
                supported_capabilities: transforms.supported_capabilities.clone(),
                session_seed: input.session_seed,
                context_snapshot: Some(prompt_context.snapshot),
            },
            provider: provider_resolution.contract.clone(),
            generation_preset_id: input
                .generation_target
                .map(|target| target.generation_preset_id.clone()),
            max_context_tokens: provider_resolution.max_context_tokens,
            reserved_output_tokens: provider_resolution.reserved_output_tokens,
        };
        Ok(PromptPlanAssembly {
            request,
            provider_resolution,
            prompt_preset_revision: preset.revision,
            prompt_preset_revision_id: preset.revision_id,
            block_source_revisions: preset.block_source_revisions,
            quick_settings,
            transform_sets: transforms.transform_sets,
            transform_set_revisions: transforms.transform_set_revisions,
            approved_import_source_ids: transforms.approved_import_source_ids,
            variables: variables.variables,
            supported_capabilities: transforms.supported_capabilities,
            knowledge_logs,
            knowledge_semantic_evidence,
            memory_evidence,
            module_overlay: preset.module_overlay,
            preparation_warnings,
        })
    }

    fn resolve_prompt_provider_and_warnings(
        &self,
        input: &GenerationPlanInput<'_>,
        quick_settings: &PromptQuickSettings,
        mut warnings: Vec<String>,
    ) -> CoreResult<(PromptProviderResolution, Vec<String>)> {
        let provider = self.prompt_provider_contract(
            input.generation_target,
            input.provider_family,
            quick_settings.max_output_tokens,
            quick_settings.reasoning_effort,
            input.prompt_wire_contract,
        )?;
        if quick_settings.reasoning_effort.is_some()
            && provider.reasoning_effort_applied != quick_settings.reasoning_effort
        {
            warnings.push(
                "reasoning effort quick setting was omitted because the selected route does not expose that exact effort"
                    .to_owned(),
            );
        }
        Ok((provider, warnings))
    }

    fn resolve_prompt_plan_assembly(
        input: &GenerationPlanInput<'_>,
        resolved_memory_semantics: Option<&ResolvedMemorySemanticQuery>,
        assembly: &mut PromptPlanAssembly,
    ) -> CoreResult<ResolvedPromptAssembly> {
        let plan = resolve_prompt_plan_engine(&assembly.request)
            .map_err(orchestration_validation_error)?;
        let plan = reseal_prompt_resolution_evidence(
            &plan,
            &assembly.block_source_revisions,
            &assembly.memory_evidence,
        )
        .map_err(orchestration_validation_error)?;
        let (plan, transform_warnings) = apply_resolved_prompt_transforms(
            &plan,
            &assembly.transform_sets,
            &assembly.variables,
            &assembly.supported_capabilities,
            &assembly.approved_import_source_ids,
        )?;
        assembly.preparation_warnings.extend(transform_warnings);
        verify_resolved_prompt_plan(&plan).map_err(orchestration_validation_error)?;
        let provider_preview = assembly
            .provider_resolution
            .adapter
            .compile_resolved_plan(
                &plan,
                assembly.provider_resolution.developer_capability,
                assembly.provider_resolution.cache_dialect,
            )
            .map_err(|error| {
                CoreError::invalid(format!(
                    "resolved prompt cannot be represented by the selected provider route: {error}"
                ))
            })?
            .preview();
        let cacheable_prefix_tokens = provider_cacheable_prefix_tokens(&provider_preview);
        if cacheable_prefix_has_volatile_before_fixed_after(&plan, &provider_preview) {
            assembly.preparation_warnings.push(
                "cache boundary has volatile prompt content before fixed content; moving fixed blocks earlier may improve cache reuse"
                    .to_owned(),
            );
        }
        let memory_semantic_evidence = resolved_memory_semantics.map(|resolved| &resolved.evidence);
        let execution_hash = prompt_execution_hash(
            &plan,
            &assembly.prompt_preset_revision_id,
            input.generation_target,
            &assembly.provider_resolution,
            &provider_preview,
            assembly.quick_settings.temperature,
            assembly.quick_settings.response_length,
            assembly.quick_settings.creativity,
            assembly.quick_settings.reasoning_effort,
            assembly.quick_settings.memory_enabled,
            assembly.quick_settings.knowledge_enabled,
            &assembly.variables,
            &assembly.transform_sets,
            assembly.module_overlay.plan_sha256.as_deref(),
            &assembly.approved_import_source_ids,
            memory_semantic_evidence,
            &assembly.knowledge_semantic_evidence,
        )?;
        if input
            .expected_plan_hash
            .is_some_and(|expected| expected != execution_hash)
        {
            return Err(CoreError::invalid(
                "prompt plan changed after preview; resolve a new preview before sending",
            ));
        }
        Ok(ResolvedPromptAssembly {
            plan,
            provider_preview,
            execution_hash,
            cacheable_prefix_tokens,
        })
    }

    fn materialize_prompt_plan_assembly(
        input: &GenerationPlanInput<'_>,
        resolved_memory_semantics: Option<&ResolvedMemorySemanticQuery>,
        assembly: PromptPlanAssembly,
        resolved: ResolvedPromptAssembly,
    ) -> CoreResult<PreparedGenerationPlan> {
        let mut display_context = assembly.request.context.clone();
        display_context.selected_knowledge.clear();
        display_context.selected_memory.clear();
        display_context.context_snapshot = None;
        let preview = redacted_prompt_preview(
            &resolved.plan,
            &resolved.execution_hash,
            assembly.prompt_preset_revision,
            &assembly.prompt_preset_revision_id,
            input.generation_target.cloned(),
            &resolved.provider_preview,
            &assembly.preparation_warnings,
        )?;
        let tokenizer_id = resolved.plan.trace.estimator_id.clone();
        let provider_execution_hash = resolved.provider_preview.execution_hash.clone();
        let mut materialized = PromptPlanner::materialize_resolved_plan(
            input.conversation_id.clone(),
            resolved.plan,
            input.model,
            assembly.quick_settings.temperature,
            assembly.quick_settings.max_output_tokens,
        )?;
        materialized.request.provider_execution_plan_hash = Some(provider_execution_hash);
        Ok(PreparedGenerationPlan {
            materialized,
            preview,
            prompt_preset_revision_id: assembly.prompt_preset_revision_id,
            execution_hash: resolved.execution_hash,
            transform_sets: assembly.transform_sets,
            transform_set_revisions: assembly.transform_set_revisions,
            variables: assembly.variables,
            supported_capabilities: assembly.supported_capabilities,
            knowledge_logs: assembly.knowledge_logs,
            approved_import_source_ids: assembly.approved_import_source_ids,
            display_context,
            module_plan_sha256: assembly.module_overlay.plan_sha256,
            cacheable_prefix_tokens: resolved.cacheable_prefix_tokens,
            tokenizer_id,
            tokenizer_version: "fallback-inexact-v1".to_owned(),
            memory_semantic_evidence: resolved_memory_semantics
                .map(|resolved| resolved.evidence.clone()),
            knowledge_semantic_evidence: assembly.knowledge_semantic_evidence,
        })
    }

    fn resolve_prompt_quick_settings(
        &self,
        binding: Option<&PromptPresetBinding>,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<PromptQuickSettings> {
        if let (Some(binding), Some(target)) = (binding, input.generation_target)
            && binding
                .generation_preset_override_id
                .as_ref()
                .is_some_and(|id| id != &target.generation_preset_id)
        {
            return Err(CoreError::invalid(
                "prompt binding generation override does not match the selected target",
            ));
        }
        if let Some(authority) = input.prompt_selection_authority {
            let quick = &authority.quick_settings;
            let mut warnings = Vec::new();
            if binding.is_some()
                && input.temperature.is_none()
                && quick.resolved_temperature.is_none()
                && !quick.supports_temperature
            {
                warnings.push(
                    "creativity quick setting was ignored because the selected route does not expose temperature"
                        .to_owned(),
                );
            }
            return Ok(PromptQuickSettings {
                temperature: quick.resolved_temperature,
                max_output_tokens: quick.resolved_max_output_tokens,
                response_length: quick.response_length,
                creativity: quick.creativity,
                reasoning_effort: quick.reasoning_effort,
                memory_enabled: quick.memory_enabled,
                knowledge_enabled: quick.knowledge_enabled,
                warnings,
            });
        }
        let mut settings = PromptQuickSettings {
            temperature: input.temperature,
            max_output_tokens: input.max_output_tokens,
            response_length: PromptResponseLength::Balanced,
            creativity: 50,
            reasoning_effort: None,
            memory_enabled: true,
            knowledge_enabled: true,
            warnings: Vec::new(),
        };
        let Some(binding) = binding else {
            return Ok(settings);
        };
        settings.response_length = binding.response_length;
        settings.creativity = binding.creativity;
        settings.reasoning_effort = binding.reasoning_effort;
        settings.memory_enabled = binding.memory_enabled;
        settings.knowledge_enabled = binding.knowledge_enabled;
        if settings.temperature.is_none() {
            let supports_temperature = if let Some(contract) = input.prompt_wire_contract {
                contract.supports_temperature
            } else {
                input.generation_target.map_or(Ok(false), |target| {
                    crate::app::prompt_route_supports_temperature(self, target)
                })?
            };
            if supports_temperature {
                settings.temperature = Some(prompt_creativity_temperature(binding.creativity));
            } else {
                settings.warnings.push(
                    "creativity quick setting was ignored because the selected route does not expose temperature"
                        .to_owned(),
                );
            }
        }
        if settings.max_output_tokens.is_none() {
            settings.max_output_tokens = Some(match binding.response_length {
                PromptResponseLength::Short => 512,
                PromptResponseLength::Balanced => 2_048,
                PromptResponseLength::Long => 4_096,
            });
        }
        Ok(settings)
    }

    fn prepare_prompt_preset_sources(
        &self,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<PromptPresetPreparation> {
        let (mut preset, revision, revision_id, binding, persona_selection) =
            self.resolve_generation_prompt_selection(input)?;
        enforce_application_policy(&mut preset);
        self.validate_prompt_preset(&preset)?;
        let prompt_persona = persona_selection
            .as_ref()
            .map(|selection| self.materialize_prompt_persona(selection))
            .transpose()?;
        let prompt_knowledge_books = self
            .storage()
            .get_prompt_preset_knowledge_book_revisions(&revision_id)?;
        let prompt_transform_sets = self
            .storage()
            .get_prompt_preset_transform_set_revisions(&revision_id)?;
        let prompt_memory_profile = self
            .storage()
            .get_prompt_preset_memory_profile_revision(&revision_id)?;
        let mut block_source_revisions = preset
            .blocks
            .iter()
            .filter(|block| block.provenance.source_kind != SourceKind::ApplicationBuiltIn)
            .map(|block| (block.id.clone(), revision_id.clone()))
            .collect::<BTreeMap<_, _>>();
        let module_overlay = self.resolve_prompt_module_overlay(
            &preset,
            &revision_id,
            PromptModuleOverlayInput {
                character: input
                    .prompt_selection_authority
                    .map_or(input.character, |authority| &authority.character),
                conversation_id: input.conversation_id,
                branch_id: input.branch_id,
                persona_id: persona_selection
                    .as_ref()
                    .map(|selection| &selection.value.persona_id),
                applied_plan_override: input.applied_module_plan_override,
                sealed_local_user_id_sha256: input
                    .prompt_selection_authority
                    .map(|authority| authority.local_user_id_sha256.as_str()),
                generation_attempt_id: input.generation_attempt_id,
            },
        )?;
        block_source_revisions.extend(module_overlay.prompt_block_source_revisions.clone());
        preset.blocks.extend(module_overlay.prompt_blocks.clone());
        preset.controls.extend(module_overlay.controls.clone());
        enforce_application_policy(&mut preset);
        preset.blocks.sort_by_key(|block| block.placement_zone);
        self.validate_prompt_preset(&preset)?;
        let warnings = module_overlay.warnings.clone();
        Ok(PromptPresetPreparation {
            preset,
            revision,
            revision_id,
            binding,
            prompt_persona,
            prompt_knowledge_books,
            prompt_transform_sets,
            prompt_memory_profile,
            block_source_revisions,
            module_overlay,
            warnings,
        })
    }

    fn materialize_prompt_persona(
        &self,
        selection: &StoredRevision<lorepia_domain::ConversationPersonaSelection>,
    ) -> CoreResult<PromptPersonaMaterialization> {
        let revision_id = selection.revision_id.as_deref().ok_or_else(|| {
            CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "persona selection is missing its exact revision identity",
                false,
            )
        })?;
        let persona = self
            .storage()
            .get_persona_revision(&selection.value.persona_id, revision_id)?;
        Ok(PromptPersonaMaterialization {
            evidence: PromptContextPersonaEvidence {
                selection_revision: selection.revision,
                persona_id: selection.value.persona_id.clone(),
                persona_revision_id: persona.revision_id.clone(),
                persona_sha256: persona.sha256,
            },
            content: PersonaPromptContent {
                persona_id: persona.value.id,
                name: persona.value.name,
                description: persona.value.description,
            },
        })
    }

    fn prepare_prompt_selections(
        &self,
        input: PromptSelectionInput<'_>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<PromptSelectionPreparation> {
        let mut warnings = Vec::new();
        let (selected_knowledge, knowledge_logs, knowledge_semantic_evidence) =
            if input.knowledge_enabled {
                self.select_prompt_knowledge(
                    input.preset,
                    input.character_content,
                    input.prompt_knowledge_books,
                    input.module_knowledge_books,
                    input.exact_character_knowledge_book,
                    input.conversation_id,
                    input.branch_id,
                    input.scan_texts,
                    input.manually_active_knowledge,
                    input.variables,
                    input.supported_capabilities,
                    input.resolved_memory_semantics,
                    input.activation_seed,
                    input.resolution_time,
                    knowledge_work_budget,
                )?
            } else {
                warnings.push("knowledge retrieval was disabled by quick settings".to_owned());
                (Vec::new(), Vec::new(), Vec::new())
            };
        let (selected_memory, memory_evidence) = if input.memory_enabled {
            self.select_prompt_memory(&input)?
        } else {
            warnings.push("memory retrieval was disabled by quick settings".to_owned());
            (Vec::new(), Vec::new())
        };
        Ok(PromptSelectionPreparation {
            selected_knowledge,
            knowledge_logs,
            knowledge_semantic_evidence,
            selected_memory,
            memory_evidence,
            warnings,
        })
    }

    fn resolve_prompt_variable_state(
        &self,
        preset: &PromptPreset,
        binding: Option<&PromptPresetBinding>,
        module_overlay: &PromptModuleOverlay,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<PromptVariableState> {
        let mut variables = self.character_runtime_initial_variables(input)?;
        merge_variable_map(&mut variables, &preset.default_values);
        if let Some(binding) = binding {
            merge_variable_map(&mut variables, &binding.variable_overrides);
        }
        merge_variable_map(&mut variables, &module_overlay.variables);
        let state_branch = input.interaction_state_branch_id.unwrap_or(input.branch_id);
        let current_module_knowledge =
            prompt_module_knowledge_revisions(&module_overlay.knowledge_books)?;
        let manually_active_knowledge = if let Some(snapshot) = input.interaction_state_override {
            if snapshot.key.conversation_id != *input.conversation_id
                || snapshot.key.branch_id != *state_branch
            {
                return Err(CoreError::invalid(
                    "historical interaction state does not match the prompt lineage",
                ));
            }
            merge_variable_map(&mut variables, &snapshot.state.variables);
            exact_prompt_manual_knowledge(
                &snapshot.state.manually_active_knowledge,
                &snapshot.knowledge,
                &current_module_knowledge,
            )?
        } else {
            match self
                .storage()
                .get_interaction_state_snapshot(input.conversation_id, state_branch)
            {
                Ok(snapshot) => {
                    merge_variable_map(&mut variables, &snapshot.state.variables);
                    exact_prompt_manual_knowledge(
                        &snapshot.state.manually_active_knowledge,
                        &snapshot.knowledge,
                        &current_module_knowledge,
                    )?
                }
                Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => {
                    BTreeSet::new()
                }
                Err(error) => return Err(error),
            }
        };
        merge_variable_map(&mut variables, input.variable_overrides);
        Ok(PromptVariableState {
            variables,
            manually_active_knowledge,
        })
    }

    fn character_runtime_initial_variables(
        &self,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<VariableMap> {
        let values = if let Some(authority) = input.prompt_selection_authority {
            authority
                .character_content
                .as_ref()
                .map(|content| content.value.runtime.initial_variables.clone())
                .unwrap_or_default()
        } else {
            match self.storage().get_character_content(&input.character.id) {
                Ok(content) => content.value.runtime.initial_variables,
                Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => {
                    BTreeMap::new()
                }
                Err(error) => return Err(error),
            }
        };
        let mut variables = VariableMap::default();
        for (name, value) in values {
            let value = match value.trim().to_ascii_lowercase().as_str() {
                "true" => VariableValue::Bool(true),
                "false" => VariableValue::Bool(false),
                _ => value
                    .parse::<i64>()
                    .map_or_else(|_| VariableValue::Text(value), VariableValue::Integer),
            };
            variables.insert(
                VariableRef {
                    scope: VariableScope::Character,
                    namespace: None,
                    id: VariableId::from(name),
                },
                value,
            );
        }
        Ok(variables)
    }

    fn prepare_prompt_transforms(
        &self,
        prompt_transform_sets: &[ObjectRevision<TransformSet>],
        module_overlay: &PromptModuleOverlay,
        input: &GenerationPlanInput<'_>,
        latest: &Message,
        variables: &VariableMap,
    ) -> CoreResult<PromptTransformPreparation> {
        let mut transform_set_revisions = BTreeMap::new();
        for revision in prompt_transform_sets
            .iter()
            .chain(&module_overlay.transform_sets)
        {
            if transform_set_revisions
                .insert(revision.value.id.clone(), revision.revision_id.clone())
                .is_some()
            {
                return Err(CoreError::invalid(
                    "prompt preset and approved module select the same transform set ambiguously",
                ));
            }
        }
        let mut transform_sets = prompt_transform_sets
            .iter()
            .map(|revision| revision.value.clone())
            .collect::<Vec<_>>();
        append_exact_module_transform_sets(&mut transform_sets, &module_overlay.transform_sets)?;
        // Imported character-card transforms are session-granted display behavior.
        // Core has no revision-bound portable-runtime grant, so it must not add the
        // stored native projection to canonical generation transforms implicitly.
        let approved_import_source_ids = module_overlay.approved_import_source_ids.clone();
        let supported_capabilities = if let Some(authority) = input.prompt_selection_authority {
            authority.supported_capabilities.clone()
        } else {
            input.generation_target.map_or_else(
                || Ok(Vec::new()),
                |target| self.prompt_supported_capabilities(&target.model_route_id),
            )?
        };
        let transformed_latest = apply_transform_sets_with_import_approvals(
            &transform_sets,
            lorepia_domain::TransformPhase::UserInputForRequest,
            &latest.content,
            variables,
            &supported_capabilities,
            &approved_import_source_ids,
        )?;
        Ok(PromptTransformPreparation {
            transform_sets,
            transform_set_revisions,
            approved_import_source_ids,
            supported_capabilities,
            transformed_latest,
        })
    }

    fn prepare_prompt_conversation(
        &self,
        input: &GenerationPlanInput<'_>,
        latest: &Message,
        transformed_latest: &TransformResult,
    ) -> CoreResult<PromptConversationPreparation> {
        let (character, character_content) =
            if let Some(authority) = input.prompt_selection_authority {
                (
                    &authority.character,
                    authority
                        .character_content
                        .as_ref()
                        .map_or_else(CharacterContentV1::default, |stored| stored.value.clone()),
                )
            } else {
                let content = match self.storage().get_character_content(&input.character.id) {
                    Ok(stored) => stored.value,
                    Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => {
                        CharacterContentV1::default()
                    }
                    Err(error) => return Err(error),
                };
                (input.character, content)
            };
        let prompt_character = character_prompt_content(character, &character_content);
        let prompt_messages = input
            .history
            .iter()
            .filter(|message| message.role != MessageRole::System)
            .enumerate()
            .map(|(index, message)| PromptConversationMessage {
                id: message.id.clone(),
                branch_id: input.branch_id.clone(),
                role: match message.role {
                    MessageRole::System => PromptMessageRole::System,
                    MessageRole::User => PromptMessageRole::User,
                    MessageRole::Assistant => PromptMessageRole::Assistant,
                },
                content: if message.id == latest.id {
                    transformed_latest.output.clone()
                } else {
                    message.content.clone()
                },
                turn_index: u32::try_from(index).unwrap_or(u32::MAX),
            })
            .collect::<Vec<_>>();
        let scan_texts = prompt_messages
            .iter()
            .map(|message| message.content.clone())
            .collect();
        Ok(PromptConversationPreparation {
            character_content,
            prompt_character,
            prompt_messages,
            scan_texts,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn materialize_prompt_context_sources(
        &self,
        preset: &PromptPreset,
        binding: Option<&StoredRevision<PromptPresetBinding>>,
        persona: Option<&PromptPersonaMaterialization>,
        conversation_id: &ConversationId,
        prompt_branch_id: &ConversationBranchId,
        context_source_branch_id: &ConversationBranchId,
        context_head_message_id: Option<&MessageId>,
        hypothetical_parent_id: Option<&MessageId>,
        messages: &[PromptConversationMessage],
        generation_attempt_id: Option<&GenerationId>,
        sealed_local_user_id_sha256: Option<&str>,
    ) -> CoreResult<PromptContextMaterialization> {
        if context_head_message_id != hypothetical_parent_id {
            return Err(CoreError::invalid(
                "attempt prompt context head differs from the hypothetical user turn",
            ));
        }
        if messages
            .iter()
            .any(|message| message.branch_id != *prompt_branch_id)
        {
            return Err(CoreError::internal(
                "materialized prompt messages use an inconsistent target branch",
            ));
        }
        let binding_value = binding.map(|stored| &stored.value);
        validate_prompt_binding_sources(preset, binding_value)?;
        let summaries = self.materialize_prompt_summaries(
            preset,
            conversation_id,
            context_source_branch_id,
            context_head_message_id,
            messages,
            generation_attempt_id,
        )?;
        let local_user_id_sha256 = sealed_local_user_id_sha256.map_or_else(
            || {
                self.storage()
                    .load_settings()
                    .map(|settings| prompt_local_user_id_sha256(&settings.local_user_id))
            },
            |sha256| Ok(sha256.to_owned()),
        )?;
        let binding_evidence = binding
            .map(|stored| {
                Ok(PromptContextBindingEvidence {
                    binding_id: stored.value.id.clone(),
                    binding_revision: stored.revision,
                    document_sha256: stored.value.canonical_document_sha256()?,
                })
            })
            .transpose()?;
        let mut snapshot = PromptContextSnapshotV1 {
            schema_version: 1,
            conversation_id: conversation_id.clone(),
            source_branch_id: context_source_branch_id.clone(),
            context_head_message_id: context_head_message_id.cloned(),
            local_user_id_sha256,
            binding: binding_evidence,
            persona: persona.map(|materialized| materialized.evidence.clone()),
            conversation_summary_id: summaries.conversation_summary_id.clone(),
            summaries: summaries.evidence.clone(),
            snapshot_sha256: String::new(),
        };
        snapshot.snapshot_sha256 =
            prompt_context_snapshot_sha256(&snapshot).map_err(orchestration_validation_error)?;
        let user_name = persona
            .map(|materialized| materialized.content.name.clone())
            .or_else(|| binding_value.and_then(|binding| binding.user_name_override.clone()))
            .unwrap_or_else(|| "Local user".to_owned());
        Ok(PromptContextMaterialization {
            user_name,
            author_note: binding_value.and_then(|binding| binding.author_note.clone()),
            group_context: binding_value.and_then(|binding| binding.group_context.clone()),
            slots: binding_value
                .map(|binding| binding.template_slots.clone())
                .unwrap_or_default(),
            summaries,
            snapshot,
        })
    }

    fn materialize_prompt_summaries(
        &self,
        preset: &PromptPreset,
        conversation_id: &ConversationId,
        context_source_branch_id: &ConversationBranchId,
        context_head_message_id: Option<&MessageId>,
        messages: &[PromptConversationMessage],
        generation_attempt_id: Option<&GenerationId>,
    ) -> CoreResult<PromptSummaryMaterialization> {
        let (needs_conversation_summary, required_summary_ids) =
            prompt_summary_requirements(preset);
        if !needs_conversation_summary && required_summary_ids.is_empty() {
            return Ok(empty_prompt_summary_materialization());
        }
        let Some(context_head_message_id) = context_head_message_id else {
            return Err(CoreError::invalid(
                "prompt summary source is unavailable before the first durable message",
            ));
        };
        let selected = generation_attempt_id.map_or_else(
            || {
                self.storage().list_memory_records_at_head(
                    conversation_id,
                    context_source_branch_id,
                    Some(context_head_message_id),
                    false,
                )
            },
            |generation_id| {
                self.load_generation_attempt_memory_selection(
                    generation_id,
                    conversation_id,
                    context_source_branch_id,
                    Some(context_head_message_id),
                )
            },
        )?;
        let visible = self.visible_prompt_summaries(
            selected,
            conversation_id,
            context_source_branch_id,
            context_head_message_id,
        )?;
        select_prompt_summary_materialization(
            &visible,
            needs_conversation_summary,
            &required_summary_ids,
            messages,
        )
    }

    fn visible_prompt_summaries(
        &self,
        selected: lorepia_storage::MemoryRecordsAtHeadSelection,
        conversation_id: &ConversationId,
        context_source_branch_id: &ConversationBranchId,
        context_head_message_id: &MessageId,
    ) -> CoreResult<Vec<VisiblePromptSummary>> {
        if selected.snapshot.conversation_id != *conversation_id
            || selected.snapshot.source_branch_id != *context_source_branch_id
            || selected.snapshot.context_head_message_id.as_ref() != Some(context_head_message_id)
            || selected.snapshot.include_invalidated
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "memory source snapshot differs from the exact prompt boundary",
                false,
            ));
        }
        if selected.records.len() != selected.snapshot.records.len() {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "memory source records differ from their exact-head evidence",
                false,
            ));
        }
        let mut candidates = Vec::new();
        for (stored, evidence) in selected.records.into_iter().zip(selected.snapshot.records) {
            validate_prompt_summary_record(&stored, &evidence)?;
            if stored.value.kind == MemoryKind::ConversationSummary
                && stored.value.invalidated_at.is_none()
                && !stored.value.excluded_from_conversation
                && !stored.value.excluded_from_character
            {
                candidates.push((stored.value, prompt_summary_evidence(evidence)));
            }
        }
        let mut endpoint_ids = candidates
            .iter()
            .map(|(record, _)| record.source_end_message_id.clone())
            .collect::<Vec<_>>();
        endpoint_ids.sort_by(|left, right| left.0.cmp(&right.0));
        endpoint_ids.dedup_by(|left, right| left.0 == right.0);
        let depths = self.storage().message_lineage_depths_at_head(
            conversation_id,
            context_source_branch_id,
            context_head_message_id,
            &endpoint_ids,
        )?;
        candidates
            .into_iter()
            .map(|(record, evidence)| {
                let end_depth = depths
                    .get(&record.source_end_message_id)
                    .copied()
                    .ok_or_else(|| {
                        CoreError::new(
                            lorepia_domain::CoreErrorCode::StorageCorrupted,
                            "summary source end has no exact lineage position",
                            false,
                        )
                    })?;
                Ok(VisiblePromptSummary {
                    record,
                    evidence,
                    end_depth,
                })
            })
            .collect()
    }

    fn load_generation_attempt_memory_selection(
        &self,
        generation_id: &GenerationId,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        context_head_message_id: Option<&MessageId>,
    ) -> CoreResult<lorepia_storage::MemoryRecordsAtHeadSelection> {
        let before = self
            .storage()
            .get_generation_attempt_before_review(generation_id)?
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "generation attempt is missing its sealed memory snapshot",
                    false,
                )
            })?;
        let snapshot = before.memory_head_snapshot;
        if snapshot.conversation_id != *conversation_id
            || snapshot.source_branch_id != *source_branch_id
            || snapshot.context_head_message_id.as_ref() != context_head_message_id
            || snapshot.include_invalidated
            || lorepia_storage::memory_records_at_head_snapshot_sha256(&snapshot)?
                != snapshot.snapshot_sha256
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "generation memory snapshot differs from its prompt boundary",
                false,
            ));
        }
        let mut records = Vec::with_capacity(snapshot.records.len());
        for evidence in &snapshot.records {
            let exact = self
                .storage()
                .get_memory_record_revision_by_id(&evidence.active_revision_id)?;
            if exact.object_kind != "memory_record"
                || exact.object_id != evidence.record_id.as_str()
                || exact.revision_id != evidence.active_revision_id
                || exact.revision != evidence.state_revision
                || exact.sha256 != evidence.active_revision_sha256
                || exact.value.id != evidence.record_id
                || exact.value.branch_id != evidence.record_branch_id
                || exact.value.source_start_message_id != evidence.source_start_message_id
                || exact.value.source_end_message_id != evidence.source_end_message_id
            {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "sealed memory revision differs from its attempt evidence",
                    false,
                ));
            }
            records.push(StoredRevision {
                value: exact.value,
                revision: exact.revision,
                revision_id: Some(exact.revision_id),
                created_at: exact.created_at,
                updated_at: exact.created_at,
                deleted_at: None,
            });
        }
        Ok(lorepia_storage::MemoryRecordsAtHeadSelection { snapshot, records })
    }

    /// Resolves the immutable module-composition authority that must be bound
    /// into a generation attempt before `BeforeGeneration` is delivered.
    ///
    /// Interaction rules may change variables, knowledge, or approval state,
    /// but they cannot replace the exact content-module composition admitted
    /// for this room. The final prompt plan independently carries the same
    /// hash and storage rechecks it at the dispatch-ready append boundary.
    pub(crate) fn resolve_generation_module_plan_sha256(
        &self,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        prompt_preset_id: Option<&PromptPresetId>,
    ) -> CoreResult<lorepia_domain::Sha256Digest> {
        let (preset, _revision, prompt_preset_revision_id, _binding, persona_selection) = self
            .resolve_prompt_preset_selection(
                character,
                conversation_id,
                branch_id,
                mode,
                prompt_preset_id,
            )?;
        let module_overlay = self.resolve_prompt_module_overlay(
            &preset,
            &prompt_preset_revision_id,
            PromptModuleOverlayInput {
                character,
                conversation_id,
                branch_id,
                persona_id: persona_selection
                    .as_ref()
                    .map(|selection| &selection.value.persona_id),
                applied_plan_override: None,
                sealed_local_user_id_sha256: None,
                generation_attempt_id: None,
            },
        )?;
        module_overlay.plan_sha256.map_or_else(
            || Ok(lorepia_orchestration::no_applied_module_runtime_plan_sha256()),
            |sha256| lorepia_domain::Sha256Digest::parse(sha256).map_err(CoreError::invalid),
        )
    }

    /// Validates a prompt preset without changing durable state.
    pub fn validate_prompt_preset(&self, preset: &PromptPreset) -> CoreResult<()> {
        validate_prompt_preset_document(preset).map_err(orchestration_validation_error)
    }

    /// Inserts a new prompt preset or updates the exact expected revision.
    pub fn upsert_prompt_preset(
        &self,
        preset: &PromptPreset,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<PromptPreset>> {
        if is_builtin_prompt_preset_id(&preset.id) {
            return Err(CoreError::invalid(
                "built-in prompt presets cannot be edited",
            ));
        }
        if preset.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn
            || preset
                .blocks
                .iter()
                .any(|block| block.provenance.source_kind == SourceKind::ApplicationBuiltIn)
        {
            return Err(CoreError::invalid(
                "creator prompt presets cannot claim application-built-in provenance",
            ));
        }
        let mut preset = preset.clone();
        preset
            .blocks
            .retain(|block| block.authority != lorepia_domain::InstructionAuthority::Application);
        enforce_application_policy(&mut preset);
        self.validate_prompt_preset(&preset)?;
        self.storage()
            .save_prompt_preset(&preset, expected_revision)
            .map(project_revision)
    }

    pub fn get_prompt_preset(&self, id: &PromptPresetId) -> CoreResult<Revisioned<PromptPreset>> {
        self.storage().get_prompt_preset(id).map(project_revision)
    }

    /// Returns a creator revision without application policy.
    /// Saving validates creator content and restores Core policy.
    pub fn get_editable_prompt_preset(
        &self,
        id: &PromptPresetId,
    ) -> CoreResult<Revisioned<PromptPreset>> {
        let mut stored = self.get_prompt_preset(id)?;
        if is_builtin_prompt_preset_id(id)
            || stored.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::PermissionDenied,
                "built-in prompt presets are read-only",
                false,
            ));
        }
        self.validate_prompt_preset(&stored.value)?;
        stored.value.blocks.retain(|block| {
            let application_authority =
                block.authority == lorepia_domain::InstructionAuthority::Application;
            let application_zone =
                block.placement_zone == lorepia_domain::PlacementZone::ApplicationPolicy;
            !application_authority && !application_zone
        });
        Ok(stored)
    }

    pub fn list_prompt_presets(&self) -> CoreResult<Vec<Revisioned<PromptPreset>>> {
        self.storage().list_prompt_presets().map(project_revisions)
    }

    /// Lists immutable preset history in ascending revision order.
    pub fn list_prompt_preset_revisions(
        &self,
        id: &PromptPresetId,
    ) -> CoreResult<Vec<ObjectRevision<PromptPreset>>> {
        self.storage().list_prompt_preset_revisions(id)
    }

    /// Returns a deterministic, content-addressed JSON diff between two
    /// immutable preset revisions.
    pub fn diff_prompt_preset_revisions(
        &self,
        id: &PromptPresetId,
        from_revision: u64,
        to_revision: u64,
    ) -> CoreResult<PromptPresetRevisionDiff> {
        self.storage()
            .diff_prompt_preset_revisions(id, from_revision, to_revision)
    }

    /// Reviews a rollback against the exact current preset, immutable target,
    /// dependency rows, and every binding whose effective revision can change.
    pub fn review_prompt_preset_rollback(
        &self,
        id: &PromptPresetId,
        expected_current_state_revision: u64,
        target_revision: u64,
    ) -> CoreResult<PromptPresetRollbackReview> {
        self.ensure_prompt_preset_is_creator_owned(id)?;
        self.load_creator_owned_prompt_preset_revision(id, target_revision)?;
        self.storage().review_prompt_preset_rollback(
            id,
            expected_current_state_revision,
            target_revision,
            Utc::now(),
        )
    }

    /// Applies a reviewed rollback as a new immutable revision.
    ///
    /// The target document is always loaded from Storage. Core removes every
    /// historical application-policy slot, injects the current canonical
    /// policy exactly once, validates the complete document, and delegates the
    /// final state/binding/dependency CAS to one Storage transaction.
    pub fn apply_prompt_preset_rollback(
        &self,
        request: &PromptPresetRollbackApplyRequest,
    ) -> CoreResult<PromptPresetRollbackReceipt> {
        self.ensure_prompt_preset_is_creator_owned(&request.review.preset_id)?;
        if request.expected_review_sha256 != request.review.review_sha256 {
            return Err(CoreError::invalid(
                "prompt preset rollback approval does not match the reviewed hash",
            ));
        }
        let target = self.load_creator_owned_prompt_preset_revision(
            &request.review.preset_id,
            request.review.target_revision,
        )?;
        if target.revision_id != request.review.target_revision_id
            || target.sha256 != request.review.target_sha256
        {
            return Err(CoreError::invalid(
                "prompt preset rollback target changed after review",
            ));
        }
        let mut canonical_target = target.value;
        canonical_target.blocks.retain(|block| {
            block.authority != lorepia_domain::InstructionAuthority::Application
                && block.placement_zone != lorepia_domain::PlacementZone::ApplicationPolicy
        });
        enforce_application_policy(&mut canonical_target);
        self.validate_prompt_preset(&canonical_target)?;

        let approval_sha256 = prompt_preset_rollback_approval_sha256(
            &request.approval_id,
            &request.expected_review_sha256,
        )?;
        let approval = PromptPresetRollbackApproval {
            approval_id: request.approval_id.clone(),
            expected_review_sha256: request.expected_review_sha256.clone(),
            approval_sha256,
            approved_at: Utc::now(),
        };
        let preset = self
            .storage()
            .apply_prompt_preset_rollback(&PromptPresetRollbackCommit {
                review: request.review.clone(),
                approval: approval.clone(),
                canonical_target,
            })?;
        let durable_approval = self
            .storage()
            .get_prompt_preset_rollback_approval(&request.approval_id)?;
        if durable_approval.approval_id != approval.approval_id
            || durable_approval.expected_review_sha256 != approval.expected_review_sha256
            || durable_approval.approval_sha256 != approval.approval_sha256
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "durable prompt preset rollback approval differs from the applied approval",
                false,
            ));
        }
        Ok(PromptPresetRollbackReceipt {
            preset: project_revision(preset),
            approval: durable_approval,
        })
    }

    pub fn delete_prompt_preset(
        &self,
        id: &PromptPresetId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<PromptPreset>> {
        let preset = self.get_prompt_preset(id)?;
        if is_builtin_prompt_preset_id(id)
            || preset.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn
        {
            return Err(CoreError::invalid(
                "built-in prompt presets cannot be deleted",
            ));
        }
        self.storage()
            .soft_delete_prompt_preset(id, expected_revision)
            .map(project_revision)
    }

    fn ensure_prompt_preset_is_creator_owned(&self, id: &PromptPresetId) -> CoreResult<()> {
        let preset = self.get_prompt_preset(id)?;
        if is_builtin_prompt_preset_id(id)
            || preset.value.metadata.provenance.source_kind == SourceKind::ApplicationBuiltIn
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::PermissionDenied,
                "built-in prompt presets are read-only",
                false,
            ));
        }
        Ok(())
    }

    fn load_creator_owned_prompt_preset_revision(
        &self,
        id: &PromptPresetId,
        revision: u64,
    ) -> CoreResult<ObjectRevision<PromptPreset>> {
        let target = self
            .storage()
            .list_prompt_preset_revisions(id)?
            .into_iter()
            .find(|candidate| candidate.revision == revision)
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::NotFound,
                    "prompt preset rollback target revision was not found",
                    false,
                )
            })?;
        let claims_application_provenance = target.value.metadata.provenance.source_kind
            == SourceKind::ApplicationBuiltIn
            || target.value.blocks.iter().any(|block| {
                block.provenance.source_kind == SourceKind::ApplicationBuiltIn
                    && (block.authority != lorepia_domain::InstructionAuthority::Application
                        || block.placement_zone != lorepia_domain::PlacementZone::ApplicationPolicy)
            });
        if claims_application_provenance {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::PermissionDenied,
                "creator prompt preset rollback targets cannot claim application-built-in provenance",
                false,
            ));
        }
        Ok(target)
    }

    /// Reorders all blocks in a creator preset with optimistic concurrency.
    /// The canonical application-policy block must remain first.
    pub fn reorder_prompt_blocks(
        &self,
        id: &PromptPresetId,
        ordered_block_ids: &[lorepia_domain::PromptBlockId],
        expected_revision: u64,
    ) -> CoreResult<Revisioned<PromptPreset>> {
        if is_builtin_prompt_preset_id(id) {
            return Err(CoreError::invalid(
                "built-in prompt presets cannot be reordered",
            ));
        }
        let stored = self.get_prompt_preset(id)?;
        if stored.revision != expected_revision {
            return Err(CoreError::invalid(
                "prompt preset changed before blocks were reordered",
            ));
        }
        let mut preset = stored.value;
        self.validate_prompt_preset(&preset)?;
        let application_policy = preset
            .blocks
            .iter()
            .find(|block| {
                block.placement_zone == lorepia_domain::PlacementZone::ApplicationPolicy
                    && block.authority == lorepia_domain::InstructionAuthority::Application
            })
            .cloned()
            .ok_or_else(|| CoreError::internal("prompt preset is missing application policy"))?;
        let mut remaining = std::mem::take(&mut preset.blocks)
            .into_iter()
            .filter(|block| {
                block.placement_zone != lorepia_domain::PlacementZone::ApplicationPolicy
                    && block.authority != lorepia_domain::InstructionAuthority::Application
            })
            .map(|block| (block.id.clone(), block))
            .collect::<std::collections::BTreeMap<_, _>>();
        if ordered_block_ids.len() != remaining.len() {
            return Err(CoreError::invalid(
                "block reorder must contain every creator-owned block exactly once",
            ));
        }
        let mut blocks = Vec::with_capacity(ordered_block_ids.len().saturating_add(1));
        blocks.push(application_policy);
        for block_id in ordered_block_ids {
            let block = remaining.remove(block_id).ok_or_else(|| {
                CoreError::invalid("block reorder contains an unknown or duplicate block")
            })?;
            blocks.push(block);
        }
        if !remaining.is_empty() {
            return Err(CoreError::invalid(
                "block reorder omitted a creator-owned block",
            ));
        }
        preset.blocks = blocks;
        self.upsert_prompt_preset(&preset, Some(expected_revision))
    }

    /// Saves the prompt selection and quick-setting overrides for one scope.
    pub fn bind_prompt_preset(
        &self,
        binding: &PromptPresetBinding,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<PromptPresetBinding>> {
        let preset = self.get_prompt_preset(&binding.prompt_preset_id)?.value;
        validate_prompt_binding_sources(&preset, Some(binding))?;
        self.storage()
            .save_prompt_preset_binding(binding, expected_revision)
            .map(project_revision)
    }

    pub fn get_room_orchestration_config(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<RoomOrchestrationConfig> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let branch = self.storage().get_conversation_branch(branch_id)?;
        if branch.conversation_id != *conversation_id {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        let character = self.storage().get_character(&conversation.character_id)?;
        let state = self.storage().get_conversation_state(conversation_id)?;
        let (preset, _, _, effective_binding, _) = self.resolve_prompt_preset_selection(
            &character,
            conversation_id,
            branch_id,
            state.selected_mode,
            None,
        )?;
        let branch_bindings = self
            .list_prompt_preset_bindings(ModuleScope::Branch, Some(branch_id.0.as_str()))?
            .into_iter()
            .filter(|stored| stored.deleted_at.is_none() && stored.value.enabled)
            .collect::<Vec<_>>();
        if branch_bindings.len() > 1 {
            return Err(CoreError::invalid(
                "multiple enabled prompt bindings apply to this room",
            ));
        }
        let binding_revision = branch_bindings.first().map(|stored| stored.revision);
        let binding = effective_binding.as_ref().map(|stored| &stored.value);
        let creator_values = creator_values_from_binding(&preset, binding)?;
        let generation_preset_id = binding
            .and_then(|binding| binding.generation_preset_override_id.clone())
            .or_else(|| preset.default_generation_preset_id.clone());
        let generation_target = if let Some(generation_preset_id) = &generation_preset_id {
            let generation_preset = self.storage().get_generation_preset(generation_preset_id)?;
            Some(GenerationTarget {
                model_route_id: generation_preset.model_route_id,
                generation_preset_id: generation_preset.id,
            })
        } else {
            let settings = self.get_settings()?;
            match (
                settings.selected_model_route_id,
                settings.selected_generation_preset_id,
            ) {
                (Some(model_route_id), Some(generation_preset_id)) => Some(GenerationTarget {
                    model_route_id,
                    generation_preset_id,
                }),
                (None, None) => None,
                _ => {
                    return Err(CoreError::new(
                        lorepia_domain::CoreErrorCode::StorageCorrupted,
                        "stored generation target is incomplete",
                        false,
                    ));
                }
            }
        };
        if let Some(target) = &generation_target {
            self.validate_generation_preset(&target.model_route_id, &target.generation_preset_id)?;
        }
        Ok(RoomOrchestrationConfig {
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            prompt_preset_id: preset.id.clone(),
            generation_preset_id,
            generation_target,
            creator_values,
            variable_overrides: binding
                .map(|binding| binding.variable_overrides.clone())
                .unwrap_or_default(),
            response_length: binding.map_or(PromptResponseLength::Balanced, |binding| {
                binding.response_length
            }),
            creativity: binding.map_or(50, |binding| binding.creativity),
            reasoning_effort: binding.and_then(|binding| binding.reasoning_effort),
            memory_enabled: binding.is_none_or(|binding| binding.memory_enabled),
            knowledge_enabled: binding.is_none_or(|binding| binding.knowledge_enabled),
            user_name_override: binding.and_then(|binding| binding.user_name_override.clone()),
            author_note: binding.and_then(|binding| binding.author_note.clone()),
            group_context: binding.and_then(|binding| binding.group_context.clone()),
            template_slots: binding
                .map(|binding| binding.template_slots.clone())
                .unwrap_or_default(),
            binding_revision,
        })
    }

    pub fn save_room_orchestration_config(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_revision: Option<u64>,
        patch: &RoomOrchestrationConfigPatch,
    ) -> CoreResult<RoomOrchestrationConfig> {
        if patch.creativity > 100 {
            return Err(CoreError::invalid(
                "room creativity must be between 0 and 100",
            ));
        }
        let current = self.get_room_orchestration_config(conversation_id, branch_id)?;
        if current.binding_revision != expected_revision {
            return Err(CoreError::invalid(
                "room orchestration settings changed before save",
            ));
        }
        let preset_id = patch
            .prompt_preset_id
            .clone()
            .unwrap_or(current.prompt_preset_id);
        let stored_preset = self.get_prompt_preset(&preset_id)?;
        let preset = stored_preset.value;
        let variable_overrides =
            canonical_creator_variable_overrides(&preset, &patch.creator_values)?;
        if let Some(generation_preset_id) = &patch.generation_preset_id {
            let generation_preset = self.storage().get_generation_preset(generation_preset_id)?;
            if generation_preset.id != *generation_preset_id {
                return Err(CoreError::internal(
                    "generation preset identity changed during room save",
                ));
            }
        }
        let now = Utc::now();
        let binding = PromptPresetBinding {
            id: deterministic_room_prompt_binding_id(conversation_id, branch_id),
            prompt_preset_id: preset_id,
            scope: ModuleScope::Branch,
            target_id: Some(branch_id.0.clone()),
            conversation_id: Some(conversation_id.clone()),
            pinned_revision_id: None,
            priority: 0,
            enabled: true,
            response_length: patch.response_length,
            creativity: patch.creativity,
            reasoning_effort: patch.reasoning_effort,
            memory_enabled: patch.memory_enabled,
            knowledge_enabled: patch.knowledge_enabled,
            variable_overrides,
            generation_preset_override_id: patch.generation_preset_id.clone(),
            user_name_override: patch.user_name_override.clone(),
            author_note: patch.author_note.clone(),
            group_context: patch.group_context.clone(),
            template_slots: patch.template_slots.clone(),
            created_at: now,
            updated_at: now,
        };
        self.bind_prompt_preset(&binding, expected_revision)?;
        self.get_room_orchestration_config(conversation_id, branch_id)
    }

    pub fn list_prompt_preset_bindings(
        &self,
        scope: ModuleScope,
        target_id: Option<&str>,
    ) -> CoreResult<Vec<Revisioned<PromptPresetBinding>>> {
        self.storage()
            .list_prompt_preset_bindings(scope, target_id)
            .map(project_revisions)
    }

    pub fn unbind_prompt_preset(
        &self,
        binding_id: &str,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<PromptPresetBinding>> {
        self.storage()
            .soft_delete_prompt_preset_binding(binding_id, expected_revision)
            .map(project_revision)
    }

    pub fn upsert_task_profile(
        &self,
        profile: &TaskProfile,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<TaskProfile>> {
        self.validate_task_profile(profile)?;
        self.storage()
            .save_task_profile(profile, expected_revision)
            .map(project_revision)
    }

    pub fn get_task_profile(&self, id: &TaskProfileId) -> CoreResult<Revisioned<TaskProfile>> {
        self.storage().get_task_profile(id).map(project_revision)
    }

    pub fn list_task_profiles(&self) -> CoreResult<Vec<Revisioned<TaskProfile>>> {
        self.storage().list_task_profiles().map(project_revisions)
    }

    pub fn delete_task_profile(
        &self,
        id: &TaskProfileId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<TaskProfile>> {
        self.storage()
            .soft_delete_task_profile(id, expected_revision)
            .map(project_revision)
    }

    /// Resolves a task profile to an ordered, provider-valid target list.
    ///
    /// The explicitly configured route/preset is always first. Each fallback
    /// route contributes its first stored preset in the storage-defined stable
    /// ordering. Missing fallback configuration is rejected before a job is
    /// launched, so background work never silently switches parameters.
    pub fn resolve_task_generation_targets(
        &self,
        id: &TaskProfileId,
    ) -> CoreResult<TaskGenerationTargetPlan> {
        let profile = self.get_task_profile(id)?.value;
        self.validate_task_profile(&profile)?;
        let mut targets = vec![GenerationTarget {
            model_route_id: profile.route_id.clone(),
            generation_preset_id: profile.generation_preset_id,
        }];
        for route_id in profile.fallback_route_ids {
            if targets
                .iter()
                .any(|target| target.model_route_id == route_id)
            {
                continue;
            }
            let preset = self
                .storage()
                .list_generation_presets(&route_id)?
                .into_iter()
                .next()
                .ok_or_else(|| {
                    CoreError::invalid(format!(
                        "task fallback route {} has no generation preset",
                        route_id.as_str()
                    ))
                })?;
            targets.push(GenerationTarget {
                model_route_id: route_id,
                generation_preset_id: preset.id,
            });
        }
        Ok(TaskGenerationTargetPlan {
            task_profile_id: id.clone(),
            targets,
        })
    }

    fn validate_task_profile(&self, profile: &TaskProfile) -> CoreResult<()> {
        if profile.timeout_ms == 0
            || profile.concurrency_limit == 0
            || profile.rate_limit.requests == 0
            || profile.rate_limit.per_seconds == 0
        {
            return Err(CoreError::invalid(
                "task profile timeout, concurrency, and rate limits must be greater than zero",
            ));
        }
        self.storage().get_model_route(&profile.route_id)?;
        let primary_preset = self
            .storage()
            .get_generation_preset(&profile.generation_preset_id)?;
        if primary_preset.model_route_id != profile.route_id {
            return Err(CoreError::invalid(
                "task profile generation preset does not belong to its primary route",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        seen.insert(profile.route_id.clone());
        for route_id in &profile.fallback_route_ids {
            if !seen.insert(route_id.clone()) {
                return Err(CoreError::invalid(
                    "task profile fallback routes must be unique",
                ));
            }
            self.storage().get_model_route(route_id)?;
            if self.storage().list_generation_presets(route_id)?.is_empty() {
                return Err(CoreError::invalid(format!(
                    "task fallback route {} has no generation preset",
                    route_id.as_str()
                )));
            }
        }
        Ok(())
    }

    pub fn upsert_memory_profile(
        &self,
        profile: &MemoryProfile,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<MemoryProfile>> {
        profile.validate().map_err(orchestration_validation_error)?;
        self.storage()
            .save_memory_profile(profile, expected_revision)
            .map(project_revision)
    }

    pub fn get_memory_profile(
        &self,
        id: &MemoryProfileId,
    ) -> CoreResult<Revisioned<MemoryProfile>> {
        self.storage().get_memory_profile(id).map(project_revision)
    }

    pub fn list_memory_profiles(&self) -> CoreResult<Vec<Revisioned<MemoryProfile>>> {
        self.storage().list_memory_profiles().map(project_revisions)
    }

    pub fn delete_memory_profile(
        &self,
        id: &MemoryProfileId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<MemoryProfile>> {
        self.storage()
            .soft_delete_memory_profile(id, expected_revision)
            .map(project_revision)
    }

    pub fn get_memory_record(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
    ) -> CoreResult<Revisioned<MemoryRecord>> {
        self.storage()
            .get_memory_record(conversation_id, branch_id, id)
            .map(project_revision)
    }

    pub fn delete_memory_record(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<MemoryRecord>> {
        self.storage()
            .delete_memory_record_tombstone(
                conversation_id,
                branch_id,
                id,
                expected_revision,
                Utc::now(),
            )
            .map(project_revision)
    }

    pub fn list_memory_records(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        include_invalidated: bool,
    ) -> CoreResult<Vec<Revisioned<MemoryRecord>>> {
        self.storage()
            .list_memory_records(conversation_id, branch_id, include_invalidated)
            .map(project_revisions)
    }

    pub fn retrieve_memory(&self, request: &MemoryRetrievalRequest) -> CoreResult<MemorySelection> {
        let profile = self.get_memory_profile(&request.profile_id)?.value;
        if profile.embedding_task.is_some() {
            return Err(CoreError::invalid(
                "configured memory embeddings require the provider-native retrieval path",
            ));
        }
        let records = self
            .list_memory_records(&request.conversation_id, &request.branch_id, false)?
            .into_iter()
            .map(|stored| stored.value)
            .collect::<Vec<_>>();
        if request.query_texts.len() > 32
            || request
                .query_texts
                .iter()
                .try_fold(0_usize, |total, text| total.checked_add(text.len()))
                .is_none_or(|total| total > 65_536)
        {
            return Err(CoreError::invalid(
                "memory retrieval query exceeds the local lexical safety limit",
            ));
        }
        let token_estimates = records
            .iter()
            .map(|record| {
                (
                    record.id.clone(),
                    estimate_prompt_memory_tokens(&record.title, &record.summary),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let semantic_scores = lexical_memory_semantic_scores(
            &records,
            request.query_texts.iter().map(String::as_str),
        );
        MemoryEngine::select(
            &records,
            &profile,
            &MemorySelectionContext {
                conversation_id: &request.conversation_id,
                branch_id: &request.branch_id,
                visible_message_ids: &request.visible_message_ids,
                semantic_scores: &semantic_scores,
                token_estimates: &token_estimates,
            },
        )
        .map_err(orchestration_validation_error)
    }

    pub fn invalidate_memory_range(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        start_message_id: &MessageId,
        end_message_id: &MessageId,
        invalidated_at: DateTime<Utc>,
    ) -> CoreResult<MemoryInvalidationResult> {
        self.storage().invalidate_memory_range(
            conversation_id,
            branch_id,
            start_message_id,
            end_message_id,
            invalidated_at,
        )
    }

    pub fn get_memory_job(&self, id: &MemoryJobId) -> CoreResult<Revisioned<MemoryJob>> {
        self.storage().get_memory_job(id).map(project_revision)
    }

    pub fn upsert_knowledge_book(
        &self,
        book: &KnowledgeBook,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<KnowledgeBook>> {
        book.validate().map_err(orchestration_validation_error)?;
        self.storage()
            .save_knowledge_book(book, expected_revision)
            .map(project_revision)
    }

    pub fn get_knowledge_book(
        &self,
        id: &KnowledgeBookId,
    ) -> CoreResult<Revisioned<KnowledgeBook>> {
        self.storage().get_knowledge_book(id).map(project_revision)
    }

    pub fn list_knowledge_books(&self) -> CoreResult<Vec<Revisioned<KnowledgeBook>>> {
        self.storage().list_knowledge_books().map(project_revisions)
    }

    pub fn delete_knowledge_book(
        &self,
        id: &KnowledgeBookId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<KnowledgeBook>> {
        self.storage()
            .soft_delete_knowledge_book(id, expected_revision)
            .map(project_revision)
    }

    pub fn simulate_knowledge_activation(
        &self,
        request: &KnowledgeSimulationRequest,
    ) -> CoreResult<KnowledgeSelection> {
        let book = self.get_knowledge_book(&request.book_id)?.value;
        let manual_entry_ids = request
            .manual_entry_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let mut token_estimates = std::collections::BTreeMap::new();
        for estimate in &request.token_estimates {
            if token_estimates
                .insert(estimate.entry_id.clone(), estimate.tokens)
                .is_some()
            {
                return Err(CoreError::invalid(
                    "knowledge token estimates contain a duplicate entry",
                ));
            }
        }
        KnowledgeEngine::select(
            &book,
            &KnowledgeSelectionContext {
                scan_texts: &request.sample_texts,
                manual_entry_ids: &manual_entry_ids,
                semantic_scores: &request.semantic_scores,
                variables: &request.variables,
                supported_capabilities: &request.supported_capabilities,
                token_estimates: &token_estimates,
                activation_seed: request.activation_seed,
            },
        )
        .map_err(orchestration_validation_error)
    }

    pub fn preview_transform(
        &self,
        request: &TransformPreviewRequest,
    ) -> CoreResult<TransformResult> {
        let transform_set = self.get_transform_set(&request.transform_set_id)?.value;
        let rule = transform_set
            .rules
            .iter()
            .find(|rule| rule.id == request.rule_id)
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::NotFound,
                    "transform rule was not found in the selected set",
                    false,
                )
            })?;
        let approved_import_source_ids =
            request.approved_import_source_ids.iter().cloned().collect();
        preview_transform_rule(
            rule,
            &request.input,
            TransformContext {
                variables: &request.variables,
                model_capabilities: &request.supported_capabilities,
            },
            TransformLimits::default(),
            &TransformCompileOptions {
                approved_import_source_ids,
            },
            TransformApplyOptions {
                allow_resolved_prompt: request.allow_resolved_prompt,
            },
        )
        .map_err(orchestration_validation_error)
    }

    pub fn upsert_interaction_rule_set(
        &self,
        rule_set: &InteractionRuleSet,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<InteractionRuleSet>> {
        self.storage()
            .save_interaction_rule_set(rule_set, expected_revision)
            .map(project_revision)
    }

    pub fn get_interaction_rule_set(
        &self,
        id: &InteractionRuleSetId,
    ) -> CoreResult<Revisioned<InteractionRuleSet>> {
        self.storage()
            .get_interaction_rule_set(id)
            .map(project_revision)
    }

    pub fn list_interaction_rule_sets(&self) -> CoreResult<Vec<Revisioned<InteractionRuleSet>>> {
        self.storage()
            .list_interaction_rule_sets()
            .map(project_revisions)
    }

    pub fn delete_interaction_rule_set(
        &self,
        id: &InteractionRuleSetId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<InteractionRuleSet>> {
        self.storage()
            .soft_delete_interaction_rule_set(id, expected_revision)
            .map(project_revision)
    }

    pub fn upsert_content_module(
        &self,
        module: &ContentModule,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<ContentModule>> {
        self.storage()
            .save_content_module(module, expected_revision)
            .map(project_revision)
    }

    pub fn get_content_module(
        &self,
        id: &ContentModuleId,
    ) -> CoreResult<Revisioned<ContentModule>> {
        self.storage().get_content_module(id).map(project_revision)
    }

    pub fn list_content_modules(&self) -> CoreResult<Vec<Revisioned<ContentModule>>> {
        self.storage().list_content_modules().map(project_revisions)
    }

    pub fn delete_content_module(
        &self,
        id: &ContentModuleId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<ContentModule>> {
        self.storage()
            .soft_delete_content_module(id, expected_revision)
            .map(project_revision)
    }

    pub fn list_content_module_bindings(
        &self,
        module_id: &ContentModuleId,
    ) -> CoreResult<Vec<Revisioned<ModuleBinding>>> {
        self.storage()
            .list_module_bindings(module_id)
            .map(project_revisions)
    }

    pub fn unbind_content_module(
        &self,
        binding_id: &ModuleBindingId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<ModuleBinding>> {
        self.storage()
            .soft_delete_module_binding(binding_id, expected_revision)
            .map(project_revision)
    }

    pub fn list_content_module_revisions(
        &self,
        id: &ContentModuleId,
    ) -> CoreResult<Vec<ObjectRevision<ContentModule>>> {
        self.storage().list_content_module_revisions(id)
    }

    pub fn diff_content_module_revisions(
        &self,
        id: &ContentModuleId,
        from_revision: u64,
        to_revision: u64,
    ) -> CoreResult<ContentModuleRevisionDiff> {
        self.storage()
            .diff_content_module_revisions(id, from_revision, to_revision)
    }

    /// Evaluates the non-networked share gate for a module.
    ///
    /// The decision does not upload or publish anything. Unknown licenses,
    /// missing imported-source hashes, high-risk assets, and explicit
    /// redistribution denial fail closed while local use remains available.
    pub fn evaluate_content_module_share_gate(
        &self,
        id: &ContentModuleId,
    ) -> CoreResult<ContentShareGate> {
        let module = self.get_content_module(id)?.value;
        let mut reasons = Vec::new();
        let license = module.metadata.license.trim();
        if license.is_empty()
            || license.eq_ignore_ascii_case("unknown")
            || license.eq_ignore_ascii_case("LicenseRef-Unknown")
        {
            reasons.push("content license is unknown".to_owned());
        }
        if !module.metadata.redistribution_allowed {
            reasons.push("content metadata does not allow redistribution".to_owned());
        }
        if module
            .required_capabilities
            .contains(&ContentCapability::HighRiskAssets)
        {
            reasons.push("module contains high-risk assets".to_owned());
        }
        if module.metadata.provenance.source_kind == SourceKind::ImportedPackage
            && module.metadata.provenance.source_hash.is_none()
        {
            reasons.push("imported module has no immutable source hash".to_owned());
        }
        Ok(ContentShareGate {
            module_id: module.id,
            local_use_allowed: true,
            sharing_allowed: reasons.is_empty(),
            reasons,
        })
    }

    fn resolve_prompt_preset_selection(
        &self,
        character: &Character,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        explicit_id: Option<&PromptPresetId>,
    ) -> CoreResult<PromptPresetSelection> {
        let persona_selection = self
            .storage()
            .get_conversation_persona_selection(conversation_id)?;
        let persona_target = persona_selection
            .as_ref()
            .map(|selection| selection.value.persona_id.0.as_str());
        let mut scopes = vec![
            (ModuleScope::Branch, Some(branch_id.0.as_str())),
            (ModuleScope::Conversation, Some(conversation_id.0.as_str())),
            (ModuleScope::Character, Some(character.id.as_str())),
        ];
        if let Some(persona_id) = persona_target {
            scopes.push((ModuleScope::Persona, Some(persona_id)));
        }
        scopes.extend([(ModuleScope::User, None), (ModuleScope::App, None)]);
        let mut selected_binding = None;
        for (scope, target_id) in scopes {
            let enabled = self
                .storage()
                .list_prompt_preset_bindings(scope, target_id)?
                .into_iter()
                .filter(|stored| stored.deleted_at.is_none() && stored.value.enabled)
                .collect::<Vec<_>>();
            if enabled.len() > 1 {
                return Err(CoreError::invalid(
                    "multiple enabled prompt bindings apply at the same scope",
                ));
            }
            if let Some(stored) = enabled.into_iter().next() {
                selected_binding = Some(stored);
                break;
            }
        }
        let preset_id = if let Some(explicit_id) = explicit_id {
            explicit_id.clone()
        } else if let Some(binding) = &selected_binding {
            binding.value.prompt_preset_id.clone()
        } else {
            let built_ins = built_in_prompt_presets();
            match mode {
                ConversationMode::Chat => built_ins[0].id.clone(),
                ConversationMode::Story => built_ins[1].id.clone(),
            }
        };
        if selected_binding
            .as_ref()
            .is_some_and(|binding| binding.value.prompt_preset_id != preset_id)
        {
            selected_binding = None;
        }
        let stored = self.get_prompt_preset(&preset_id)?;
        let revision_id = stored.revision_id.clone().ok_or_else(|| {
            CoreError::internal("prompt preset is missing its immutable revision identity")
        })?;
        Ok((
            stored.value,
            stored.revision,
            revision_id,
            selected_binding,
            persona_selection,
        ))
    }

    pub(crate) fn capture_generation_prompt_selection_authority(
        &self,
        input: GenerationPromptAuthorityCapture<'_>,
    ) -> CoreResult<GenerationPromptSelectionAuthority> {
        let GenerationPromptAuthorityCapture {
            character,
            conversation_id,
            branch_id,
            mode,
            explicit_preset_id,
            generation_target,
            temperature,
            max_output_tokens,
            prompt_wire_contract,
            provider_target_authority,
        } = input;
        let (preset, preset_revision, preset_revision_id, binding, persona_selection) = self
            .resolve_prompt_preset_selection(
                character,
                conversation_id,
                branch_id,
                mode,
                explicit_preset_id,
            )?;
        let character_content = match self.storage().get_character_content(&character.id) {
            Ok(stored) => Some(stored),
            Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => None,
            Err(error) => return Err(error),
        };
        let character_knowledge_book = character_content
            .as_ref()
            .and_then(|content| content.value.knowledge_book.as_ref())
            .and_then(|reference| reference.id.as_ref())
            .map(|book_id| self.storage().get_knowledge_book(book_id))
            .transpose()?;
        let supported_capabilities = generation_target.map_or_else(
            || Ok(Vec::new()),
            |target| self.prompt_supported_capabilities(&target.model_route_id),
        )?;
        let supported_capabilities = canonical_prompt_capabilities(supported_capabilities)?;
        let binding_value = binding.as_ref().map(|stored| &stored.value);
        let response_length = binding_value.map_or(PromptResponseLength::Balanced, |value| {
            value.response_length
        });
        let creativity = binding_value.map_or(50, |value| value.creativity);
        let supports_temperature = prompt_wire_contract.map_or_else(
            || {
                generation_target.map_or(Ok(temperature.is_some()), |target| {
                    crate::app::prompt_route_supports_temperature(self, target)
                })
            },
            |contract| Ok(contract.supports_temperature),
        )?;
        let resolved_temperature = temperature.or_else(|| {
            (binding_value.is_some() && supports_temperature)
                .then_some(prompt_creativity_temperature(creativity))
        });
        let resolved_max_output_tokens = max_output_tokens.or_else(|| {
            binding_value.map(|_| match response_length {
                PromptResponseLength::Short => 512,
                PromptResponseLength::Balanced => 2_048,
                PromptResponseLength::Long => 4_096,
            })
        });
        let authority = GenerationPromptSelectionAuthority {
            schema_version: 1,
            mode,
            local_user_id_sha256: prompt_local_user_id_sha256(
                &self.storage().load_settings()?.local_user_id,
            ),
            character: character.clone(),
            character_content,
            character_knowledge_book,
            supported_capabilities,
            quick_settings: GenerationPromptQuickSettingsAuthority {
                response_length,
                creativity,
                reasoning_effort: binding_value.and_then(|value| value.reasoning_effort),
                memory_enabled: binding_value.is_none_or(|value| value.memory_enabled),
                knowledge_enabled: binding_value.is_none_or(|value| value.knowledge_enabled),
                supports_temperature,
                resolved_temperature,
                resolved_max_output_tokens,
            },
            provider_target_authority: Some(provider_target_authority),
            explicit_preset_id: explicit_preset_id.cloned(),
            preset,
            preset_revision,
            preset_revision_id,
            binding,
            persona_selection,
        };
        generation_prompt_selection_authority_sha256(&authority)?;
        Ok(authority)
    }

    fn resolve_generation_prompt_selection(
        &self,
        input: &GenerationPlanInput<'_>,
    ) -> CoreResult<PromptPresetSelection> {
        let Some(authority) = input.prompt_selection_authority else {
            return self.resolve_prompt_preset_selection(
                input.character,
                input.conversation_id,
                input.branch_id,
                input.mode,
                input.prompt_preset_id,
            );
        };
        generation_prompt_selection_authority_sha256(authority)?;
        if authority.explicit_preset_id.as_ref() != input.prompt_preset_id {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "attempt prompt selection differs from the requested prompt context",
                false,
            ));
        }
        Ok((
            authority.preset.clone(),
            authority.preset_revision,
            authority.preset_revision_id.clone(),
            authority.binding.clone(),
            authority.persona_selection.clone(),
        ))
    }

    pub(crate) fn prompt_reasoning_effort_for_context(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mode: ConversationMode,
        explicit_id: Option<&PromptPresetId>,
    ) -> CoreResult<Option<GenerationReasoningEffort>> {
        let conversation = self.storage().get_conversation(conversation_id)?;
        let character = self.storage().get_character(&conversation.character_id)?;
        self.resolve_prompt_preset_selection(
            &character,
            conversation_id,
            branch_id,
            mode,
            explicit_id,
        )
        .map(|(_, _, _, binding, _)| binding.and_then(|binding| binding.value.reasoning_effort))
    }

    fn resolve_prompt_module_overlay(
        &self,
        preset: &PromptPreset,
        prompt_preset_revision_id: &str,
        input: PromptModuleOverlayInput<'_>,
    ) -> CoreResult<PromptModuleOverlay> {
        let preset_dependencies = if preset.module_ids.is_empty() {
            Vec::new()
        } else {
            self.storage()
                .get_prompt_preset_module_dependencies(prompt_preset_revision_id)?
        };
        if input.applied_plan_override.is_none()
            && let Some(generation_id) = input.generation_attempt_id
        {
            let attempt = self.storage().get_generation_attempt(generation_id)?;
            if input.sealed_local_user_id_sha256.is_none()
                || attempt.input.module_plan_sha256
                    != lorepia_orchestration::no_applied_module_runtime_plan_sha256()
            {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "attempt no-module authority is incomplete",
                    false,
                ));
            }
            return missing_preset_module_overlay(preset, &preset_dependencies);
        }
        let local_user_id = if let Some(applied) = input.applied_plan_override {
            applied.verify().map_err(module_plan_error)?;
            if input.sealed_local_user_id_sha256.is_some_and(|expected| {
                prompt_local_user_id_sha256(&applied.review.context.local_user_id) != expected
            }) {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "attempt module plan local user differs from sealed prompt authority",
                    false,
                ));
            }
            applied.review.context.local_user_id.clone()
        } else {
            let local_user_id = self.storage().load_settings()?.local_user_id;
            if input
                .sealed_local_user_id_sha256
                .is_some_and(|expected| prompt_local_user_id_sha256(&local_user_id) != expected)
            {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "current local user differs from sealed prompt authority",
                    false,
                ));
            }
            local_user_id
        };
        let context = ModuleResolutionContext {
            local_user_id,
            persona_id: input.persona_id.cloned(),
            character_id: Some(input.character.id.clone()),
            conversation_id: Some(input.conversation_id.0.clone()),
            branch_id: Some(input.branch_id.0.clone()),
            supported_capabilities: crate::module_orchestration::SUPPORTED_CONTENT_CAPABILITIES
                .to_vec(),
        };
        if let Some(applied) = input.applied_plan_override {
            if applied.review.context != context {
                return Err(CoreError::invalid(
                    "applied module plan override does not match the prompt context",
                ));
            }
            return self.materialize_prompt_module_overlay(preset, &preset_dependencies, applied);
        }
        let bindings = self.storage().list_all_module_bindings()?;
        let has_applicable_binding = bindings.iter().any(|stored| {
            stored.deleted_at.is_none()
                && stored.value.enabled
                && stored.value.approved
                && module_binding_applies_to_prompt(
                    &stored.value,
                    input.conversation_id,
                    input.branch_id,
                    &input.character.id,
                    input.persona_id.map(PersonaId::as_str),
                )
        });
        let approved = match self.resolve_applied_content_module_runtime_plan(&context) {
            Ok(approved) => approved,
            Err(error)
                if error.code == lorepia_domain::CoreErrorCode::NotFound
                    && !has_applicable_binding =>
            {
                return missing_preset_module_overlay(preset, &preset_dependencies);
            }
            Err(error) => return Err(error),
        };
        approved.verify().map_err(module_plan_error)?;
        self.materialize_prompt_module_overlay(preset, &preset_dependencies, &approved)
    }

    fn materialize_prompt_module_overlay(
        &self,
        preset: &PromptPreset,
        preset_dependencies: &[lorepia_storage::PromptPresetModuleDependency],
        approved: &AppliedModuleRuntimePlan,
    ) -> CoreResult<PromptModuleOverlay> {
        let mut overlay = initialize_prompt_module_overlay(preset, preset_dependencies, approved)?;

        for component in &approved.plan.components {
            let snapshot = self.storage().get_module_revision_component(
                &component.selected_source,
                &component.component,
                &component.sha256,
            )?;
            match (&component.component, snapshot) {
                (
                    ModuleComponentRef::PromptBlock { .. },
                    ModuleRevisionComponentSnapshot::PromptBlock(mut block),
                ) => {
                    if block.authority == lorepia_domain::InstructionAuthority::Application
                        || block.placement_zone == lorepia_domain::PlacementZone::ApplicationPolicy
                        || block.provenance.source_kind == SourceKind::ApplicationBuiltIn
                    {
                        return Err(CoreError::new(
                            lorepia_domain::CoreErrorCode::PermissionDenied,
                            "approved module attempted to replace application prompt policy",
                            false,
                        ));
                    }
                    // The immutable component digest is stronger runtime
                    // provenance than an optional package-level source hash.
                    block.provenance.source_hash = Some(component.sha256.as_str().to_owned());
                    overlay.prompt_block_source_revisions.insert(
                        block.id.clone(),
                        component.selected_source.revision_id.as_str().to_owned(),
                    );
                    overlay.prompt_blocks.push(block);
                }
                (
                    ModuleComponentRef::Control { .. },
                    ModuleRevisionComponentSnapshot::Control(control),
                ) => overlay.controls.push(control),
                (
                    ModuleComponentRef::KnowledgeBook { .. },
                    ModuleRevisionComponentSnapshot::KnowledgeBook(book),
                ) => overlay.knowledge_books.push(book),
                (
                    ModuleComponentRef::TransformSet { .. },
                    ModuleRevisionComponentSnapshot::TransformSet(mut transform_set),
                ) => {
                    apply_exact_transform_runtime_overlay(
                        &mut transform_set.value,
                        component.runtime_enabled,
                    );
                    if component.runtime_enabled {
                        collect_exact_component_import_approvals(
                            &transform_set.value.provenance,
                            transform_set
                                .value
                                .rules
                                .iter()
                                .map(|rule| &rule.provenance),
                            &mut overlay.approved_import_source_ids,
                        )?;
                    }
                    overlay.transform_sets.push(transform_set);
                }
                (
                    ModuleComponentRef::InteractionRuleSet { .. },
                    ModuleRevisionComponentSnapshot::InteractionRuleSet(_),
                )
                | (ModuleComponentRef::Asset { .. }, ModuleRevisionComponentSnapshot::Asset(_)) => {
                }
                _ => {
                    return Err(CoreError::new(
                        lorepia_domain::CoreErrorCode::StorageCorrupted,
                        "approved module component resolved to the wrong immutable type",
                        false,
                    ));
                }
            }
        }
        Ok(overlay)
    }

    fn prompt_supported_capabilities(
        &self,
        model_route_id: &lorepia_domain::ModelRouteId,
    ) -> CoreResult<Vec<CapabilityKey>> {
        const KEYS: [CapabilityKey; 16] = [
            CapabilityKey::Streaming,
            CapabilityKey::Reasoning,
            CapabilityKey::PromptCaching,
            CapabilityKey::ToolCalling,
            CapabilityKey::ParallelToolCalling,
            CapabilityKey::StructuredOutput,
            CapabilityKey::JsonMode,
            CapabilityKey::ImageInput,
            CapabilityKey::AudioInput,
            CapabilityKey::AudioOutput,
            CapabilityKey::Logprobs,
            CapabilityKey::Seed,
            CapabilityKey::Batch,
            CapabilityKey::Background,
            CapabilityKey::ContextWindow,
            CapabilityKey::MaxOutputTokens,
        ];
        let mut supported = Vec::new();
        for key in KEYS {
            let Some(capability) = self.effective_capability(model_route_id, key)? else {
                continue;
            };
            if capability.has_conflict || capability.selected_is_stale {
                continue;
            }
            if matches!(
                capability.selected.status,
                lorepia_domain::SupportStatus::Unsupported | lorepia_domain::SupportStatus::Unknown
            ) || matches!(capability.selected.value, CapabilityValue::Boolean(false))
            {
                continue;
            }
            supported.push(key);
        }
        Ok(supported)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn select_prompt_knowledge(
        &self,
        preset: &PromptPreset,
        character_content: &CharacterContentV1,
        exact_prompt_books: &[ObjectRevision<KnowledgeBook>],
        exact_module_books: &[ObjectRevision<KnowledgeBook>],
        exact_character_book: Option<&StoredRevision<KnowledgeBook>>,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        scan_texts: &[String],
        manual_entry_ids: &BTreeSet<KnowledgeEntryId>,
        variables: &VariableMap,
        supported_capabilities: &[CapabilityKey],
        resolved_semantics: Option<&ResolvedMemorySemanticQuery>,
        activation_seed: u64,
        selected_at: DateTime<Utc>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<(
        Vec<SelectedKnowledge>,
        Vec<KnowledgeActivationLog>,
        Vec<KnowledgeSemanticBookEvidence>,
    )> {
        let mut book_ids = preset
            .knowledge_book_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if let Some(id) = character_content
            .knowledge_book
            .as_ref()
            .and_then(|reference| reference.id.as_ref())
        {
            book_ids.insert(id.clone());
        }
        let prompt_books = exact_prompt_books
            .iter()
            .map(|revision| (revision.value.id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        let module_books = exact_module_books
            .iter()
            .map(|revision| (revision.value.id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        if prompt_books
            .keys()
            .any(|book_id| module_books.contains_key(book_id))
        {
            return Err(CoreError::invalid(
                "prompt preset and approved module select the same knowledge book",
            ));
        }
        book_ids.extend(module_books.keys().cloned());
        let token_estimates = std::collections::BTreeMap::new();
        let mut selected_all = Vec::new();
        let mut logs = Vec::new();
        let mut semantic_evidence = Vec::new();
        for book_id in book_ids {
            let (book, book_revision_id) = if let Some(revision) = module_books.get(&book_id) {
                (revision.value.clone(), revision.revision_id.clone())
            } else if let Some(revision) = prompt_books.get(&book_id) {
                (revision.value.clone(), revision.revision_id.clone())
            } else if let Some(revision) =
                exact_character_book.filter(|revision| revision.value.id == book_id)
            {
                let revision_id = revision.revision_id.clone().ok_or_else(|| {
                    CoreError::new(
                        lorepia_domain::CoreErrorCode::StorageCorrupted,
                        "sealed character knowledge book is missing its exact revision",
                        false,
                    )
                })?;
                (revision.value.clone(), revision_id)
            } else {
                let stored_book = self.get_knowledge_book(&book_id)?;
                let revision_id = stored_book.revision_id.ok_or_else(|| {
                    CoreError::internal("knowledge book is missing its immutable revision identity")
                })?;
                (stored_book.value, revision_id)
            };
            let semantic_entry_count = book
                .entries
                .iter()
                .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
                .count();
            let (semantic_scores, semantic_source) = if semantic_entry_count > 0 {
                self.resolve_prompt_knowledge_semantic_scores(
                    &book,
                    &book_revision_id,
                    scan_texts,
                    resolved_semantics,
                    knowledge_work_budget,
                )?
            } else {
                (Vec::new(), None)
            };
            if let Some(source) = semantic_source {
                semantic_evidence.push(KnowledgeSemanticBookEvidence {
                    book_id: book.id.clone(),
                    book_revision_id: book_revision_id.clone(),
                    source,
                    semantic_entry_count: u32::try_from(semantic_entry_count).map_err(|_| {
                        CoreError::internal("knowledge semantic entry count overflowed")
                    })?,
                    scores_sha256: knowledge_semantic_scores_sha256(
                        &book_revision_id,
                        &semantic_scores,
                        book.id.as_str(),
                        knowledge_work_budget,
                    )?,
                });
            }
            let selection = KnowledgeEngine::select_with_budget(
                &book,
                &KnowledgeSelectionContext {
                    scan_texts,
                    manual_entry_ids,
                    semantic_scores: &semantic_scores,
                    variables,
                    supported_capabilities,
                    token_estimates: &token_estimates,
                    activation_seed,
                },
                knowledge_work_budget,
            )
            .map_err(orchestration_validation_error)?;
            for selected in selection.selected {
                let entry = book
                    .entries
                    .iter()
                    .find(|entry| entry.id == selected.entry_id)
                    .ok_or_else(|| CoreError::internal("selected knowledge entry disappeared"))?;
                selected_all.push(SelectedKnowledge {
                    entry_id: entry.id.clone(),
                    content: selected.content,
                    placement: selected.placement,
                    priority: entry.priority,
                    evidence: selected.reasons,
                    provenance: entry.provenance.clone(),
                });
            }
            for evidence in selection.evidence {
                let identity = format!(
                    "lorepia:knowledge-log:v1\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
                    conversation_id.0,
                    branch_id.0,
                    book.id.as_str(),
                    evidence.entry_id.as_str()
                );
                logs.push(KnowledgeActivationLog {
                    id: Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string(),
                    book_id: book.id.clone(),
                    book_revision_id: book_revision_id.clone(),
                    entry_id: evidence.entry_id,
                    conversation_id: conversation_id.clone(),
                    branch_id: branch_id.clone(),
                    selected: evidence.selected,
                    reasons: evidence.reasons,
                    estimated_tokens: evidence.estimated_tokens,
                    exclusion_reason: evidence.exclusion_reason,
                    created_at: selected_at,
                });
            }
        }
        selected_all.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.entry_id.cmp(&right.entry_id))
        });
        Ok((selected_all, logs, semantic_evidence))
    }

    #[allow(clippy::too_many_lines)]
    fn resolve_prompt_knowledge_semantic_scores(
        &self,
        book: &KnowledgeBook,
        book_revision_id: &str,
        scan_texts: &[String],
        resolved_semantics: Option<&ResolvedMemorySemanticQuery>,
        work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<(
        Vec<SemanticKnowledgeScore>,
        Option<KnowledgeSemanticScoreSourceEvidence>,
    )> {
        if let Some(resolved) = resolved_semantics
            && let Some(values) = resolved.provider_query_values.as_ref()
        {
            let MemorySemanticQueryEvidence::ProviderEmbeddingV1 {
                memory_profile_revision_id,
                task_profile_revision_id,
                model_route_id,
                dimensions,
                vector_space_sha256,
                query_sha256,
                query_embedding_id,
                query_embedding_revision,
                query_vector_sha256,
                ..
            } = &resolved.evidence
            else {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "provider knowledge query vector has no provider evidence",
                    false,
                ));
            };
            let (
                Some(query_embedding_id),
                Some(query_embedding_revision),
                Some(query_vector_sha256),
            ) = (
                query_embedding_id.as_ref(),
                *query_embedding_revision,
                query_vector_sha256.as_ref(),
            )
            else {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "provider knowledge query vector is missing its durable identity",
                    false,
                ));
            };
            let required_clone_work = book
                .entries
                .iter()
                .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
                .fold(0_usize, |total, entry| {
                    total.saturating_add(entry.id.as_str().len())
                });
            charge_provider_knowledge_work(book.id.as_str(), work_budget, required_clone_work)?;
            let required_entry_ids = book
                .entries
                .iter()
                .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>();
            let query_clone_work = values
                .len()
                .checked_mul(std::mem::size_of::<f32>())
                .and_then(|value| value.checked_add(book_revision_id.len()))
                .and_then(|value| value.checked_add(task_profile_revision_id.len()))
                .and_then(|value| value.checked_add(model_route_id.as_str().len()))
                .and_then(|value| value.checked_add(vector_space_sha256.len()))
                .ok_or_else(|| CoreError::invalid("knowledge embedding query work overflowed"))?;
            charge_provider_knowledge_work(book.id.as_str(), work_budget, query_clone_work)?;
            let query_result = self
                .storage()
                .query_required_knowledge_embeddings_cosine_bounded(
                    &KnowledgeEmbeddingQuery {
                        book_revision_id: book_revision_id.to_owned(),
                        task_profile_revision_id: task_profile_revision_id.clone(),
                        model_route_id: model_route_id.clone(),
                        dimensions: *dimensions,
                        vector_space_sha256: vector_space_sha256.clone(),
                        values: values.clone(),
                    },
                    &required_entry_ids,
                    work_budget.remaining_work_bytes(),
                )?;
            charge_provider_knowledge_work(book.id.as_str(), work_budget, query_result.work_bytes)?;
            let matches = query_result.matches;
            if matches.len() == required_entry_ids.len() {
                let score_projection_work = matches.iter().fold(0_usize, |total, candidate| {
                    total
                        .saturating_add(candidate.entry_id.as_str().len())
                        .saturating_add(std::mem::size_of::<SemanticKnowledgeScore>())
                });
                charge_provider_knowledge_work(
                    book.id.as_str(),
                    work_budget,
                    score_projection_work,
                )?;
                let mut scores = matches
                    .iter()
                    .map(|candidate| {
                        Ok(SemanticKnowledgeScore {
                            entry_id: candidate.entry_id.clone(),
                            score: semantic_score_from_millionths(candidate.similarity_millionths)?,
                        })
                    })
                    .collect::<CoreResult<Vec<_>>>()?;
                scores.sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
                return Ok((
                    scores,
                    Some(KnowledgeSemanticScoreSourceEvidence::ProviderEmbeddingV1 {
                        memory_profile_revision_id: memory_profile_revision_id.clone(),
                        task_profile_revision_id: task_profile_revision_id.clone(),
                        model_route_id: model_route_id.clone(),
                        dimensions: *dimensions,
                        vector_space_sha256: vector_space_sha256.clone(),
                        query_sha256: query_sha256.clone(),
                        query_embedding_id: query_embedding_id.clone(),
                        query_embedding_revision,
                        query_vector_sha256: query_vector_sha256.clone(),
                        matches_sha256: knowledge_embedding_matches_sha256(
                            book_revision_id,
                            &matches,
                            book.id.as_str(),
                            work_budget,
                        )?,
                    }),
                ));
            }
        }

        let scores = lexical_knowledge_semantic_scores_with_budget(book, scan_texts, work_budget)?;
        let query_sha256 = knowledge_semantic_query_sha256(book, scan_texts, work_budget)?;
        Ok((
            scores,
            Some(KnowledgeSemanticScoreSourceEvidence::LexicalV1 { query_sha256 }),
        ))
    }

    fn select_prompt_memory(
        &self,
        input: &PromptSelectionInput<'_>,
    ) -> CoreResult<(Vec<SelectedMemory>, Vec<PromptMemorySelectionEvidence>)> {
        let Some(source) = self.load_prompt_memory_source(input)? else {
            return Ok((Vec::new(), Vec::new()));
        };
        let semantic_scores = prompt_memory_selection_semantic_scores(
            &source.profile,
            input.memory_profile,
            &source.records,
            input.prompt_messages,
            input.resolved_memory_semantics,
        )?;
        let visible_message_ids = input
            .prompt_messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        let token_estimates = source
            .records
            .iter()
            .map(|record| {
                (
                    record.id.clone(),
                    estimate_prompt_memory_tokens(&record.title, &record.summary),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let selection = MemoryEngine::select(
            &source.records,
            &source.profile,
            &MemorySelectionContext {
                conversation_id: input.conversation_id,
                branch_id: input.branch_id,
                visible_message_ids: &visible_message_ids,
                semantic_scores: &semantic_scores,
                token_estimates: &token_estimates,
            },
        )
        .map_err(orchestration_validation_error)?;
        materialize_prompt_memory_selection(selection, &source.records)
    }

    fn load_prompt_memory_source(
        &self,
        input: &PromptSelectionInput<'_>,
    ) -> CoreResult<Option<PromptMemorySource>> {
        let Some(profile_id) = &input.preset.memory_profile_id else {
            return Ok(None);
        };
        let profile = input
            .memory_profile
            .filter(|revision| revision.value.id == *profile_id)
            .map(|revision| revision.value.clone())
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "prompt preset memory profile dependency is missing its exact revision",
                    false,
                )
            })?;
        let lineage_branch_id = input.memory_lineage_branch_id.unwrap_or(input.branch_id);
        let selection = input.generation_attempt_id.map_or_else(
            || {
                self.storage().list_memory_records_at_head(
                    input.conversation_id,
                    lineage_branch_id,
                    input.memory_context_head_message_id,
                    false,
                )
            },
            |generation_id| {
                self.load_generation_attempt_memory_selection(
                    generation_id,
                    input.conversation_id,
                    lineage_branch_id,
                    input.memory_context_head_message_id,
                )
            },
        )?;
        let records = selection
            .records
            .into_iter()
            .map(|stored| stored.value)
            .collect();
        Ok(Some(PromptMemorySource { profile, records }))
    }

    fn prompt_provider_contract(
        &self,
        target: Option<&GenerationTarget>,
        family: Option<ApiFamily>,
        max_output_tokens: Option<u32>,
        requested_reasoning_effort: Option<GenerationReasoningEffort>,
        supplied_wire_contract: Option<&crate::app::PromptRouteWireContract>,
    ) -> CoreResult<PromptProviderResolution> {
        let owned_wire_contract = self.resolve_owned_prompt_wire_contract(
            target,
            supplied_wire_contract,
            requested_reasoning_effort,
        )?;
        let wire_contract = supplied_wire_contract.or(owned_wire_contract.as_ref());
        if wire_contract.is_some_and(|contract| {
            contract.reasoning_effort_applied != requested_reasoning_effort
                && contract.reasoning_effort_applied.is_some()
        }) {
            return Err(CoreError::internal(
                "provider snapshot reasoning overlay does not match prompt quick settings",
            ));
        }
        let family = resolve_prompt_provider_family(family, wire_contract)?;
        let (max_context_tokens, reserved_output_tokens) =
            prompt_provider_token_limits(wire_contract, max_output_tokens)?;
        let metadata = prompt_provider_wire_metadata(family, wire_contract);
        let adapter = ProviderPromptAdapterContract::for_family(family)
            .with_context_limit_tokens(Some(max_context_tokens))
            .map_err(orchestration_validation_error)?;
        let mut contract = adapter.resolution_contract(metadata.developer_capability);
        contract.supports_explicit_cache = matches!(
            metadata.cache_dialect,
            PromptCacheWireDialect::Anthropic {
                supports_explicit_breakpoints: true,
                ..
            }
        );
        contract.max_cache_boundaries = if contract.supports_explicit_cache {
            4
        } else {
            0
        };
        Ok(PromptProviderResolution {
            contract,
            adapter,
            developer_capability: metadata.developer_capability,
            cache_dialect: metadata.cache_dialect,
            max_context_tokens,
            reserved_output_tokens,
            reasoning_effort_applied: wire_contract
                .and_then(|contract| contract.reasoning_effort_applied),
            request_plan_sha256: metadata.request_plan_sha256,
            generation_preset_sha256: metadata.generation_preset_sha256,
        })
    }

    fn resolve_owned_prompt_wire_contract(
        &self,
        target: Option<&GenerationTarget>,
        supplied: Option<&crate::app::PromptRouteWireContract>,
        reasoning_effort: Option<GenerationReasoningEffort>,
    ) -> CoreResult<Option<crate::app::PromptRouteWireContract>> {
        match (target, supplied) {
            (Some(_), Some(_)) | (None, None) => Ok(None),
            (Some(target), None) => crate::app::prompt_route_wire_contract_with_reasoning_effort(
                self,
                target,
                reasoning_effort,
            )
            .map(Some),
            (None, Some(_)) => Err(CoreError::internal(
                "legacy provider cannot carry a catalog route contract",
            )),
        }
    }
}

fn resolve_prompt_provider_family(
    family: Option<ApiFamily>,
    wire_contract: Option<&crate::app::PromptRouteWireContract>,
) -> CoreResult<ApiFamily> {
    match (family, wire_contract) {
        (Some(family), Some(contract)) if family != contract.api_family => Err(
            CoreError::internal("provider snapshot API family does not match prompt preparation"),
        ),
        (Some(family), _) => Ok(family),
        (None, Some(contract)) => Ok(contract.api_family),
        (None, None) => Ok(ApiFamily::OpenAiChatCompletions),
    }
}

fn prompt_provider_token_limits(
    wire_contract: Option<&crate::app::PromptRouteWireContract>,
    max_output_tokens: Option<u32>,
) -> CoreResult<(u32, u32)> {
    let max_context_tokens = wire_contract
        .and_then(|contract| contract.context_limit_tokens)
        .unwrap_or(8_192);
    let requested_output_tokens = max_output_tokens
        .or_else(|| wire_contract.and_then(|contract| contract.configured_max_output_tokens))
        .unwrap_or(4_096);
    let reserved_output_tokens = wire_contract
        .and_then(|contract| contract.observed_max_output_tokens)
        .map_or(requested_output_tokens, |limit| {
            requested_output_tokens.min(limit)
        });
    if reserved_output_tokens >= max_context_tokens {
        return Err(CoreError::invalid(
            "reserved output tokens must be smaller than the model context limit",
        ));
    }
    Ok((max_context_tokens, reserved_output_tokens))
}

fn prompt_provider_wire_metadata(
    family: ApiFamily,
    wire_contract: Option<&crate::app::PromptRouteWireContract>,
) -> PromptProviderWireMetadata {
    wire_contract.map_or_else(
        || PromptProviderWireMetadata {
            developer_capability: match family {
                ApiFamily::OpenAiResponses => DeveloperRoleCapability::Supported,
                ApiFamily::OpenAiChatCompletions => DeveloperRoleCapability::Unknown,
                ApiFamily::AnthropicMessages
                | ApiFamily::GeminiGenerateContent
                | ApiFamily::OllamaNative => DeveloperRoleCapability::Unsupported,
            },
            cache_dialect: PromptCacheWireDialect::Unsupported,
            request_plan_sha256: "legacy-provider-request-plan".to_owned(),
            generation_preset_sha256: "legacy-generation-preset".to_owned(),
        },
        |contract| PromptProviderWireMetadata {
            developer_capability: contract.developer_capability,
            cache_dialect: contract.cache_dialect,
            request_plan_sha256: contract.request_plan_sha256.clone(),
            generation_preset_sha256: contract.generation_preset_sha256.clone(),
        },
    )
}

fn prompt_memory_selection_semantic_scores(
    profile: &MemoryProfile,
    exact_profile: Option<&ObjectRevision<MemoryProfile>>,
    records: &[MemoryRecord],
    messages: &[PromptConversationMessage],
    resolved_semantics: Option<&ResolvedMemorySemanticQuery>,
) -> CoreResult<Vec<MemorySemanticScore>> {
    match (profile.embedding_task.is_some(), resolved_semantics) {
        (false, None) => Ok(prompt_memory_semantic_scores(records, messages)),
        (true, Some(resolved)) => {
            if !memory_semantic_evidence_matches_profile(
                &resolved.evidence,
                &profile.id,
                exact_profile
                    .map(|revision| revision.revision_id.as_str())
                    .unwrap_or_default(),
            ) {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "provider-native memory evidence differs from the exact prompt profile",
                    false,
                ));
            }
            Ok(resolved.scores.clone())
        }
        (true, None) => Err(CoreError::invalid(
            "configured memory embeddings require the durable provider-native retrieval path",
        )),
        (false, Some(_)) => Err(CoreError::invalid(
            "lexical memory profiles cannot accept provider-native semantic scores",
        )),
    }
}

fn materialize_prompt_memory_selection(
    selection: MemorySelection,
    records: &[MemoryRecord],
) -> CoreResult<(Vec<SelectedMemory>, Vec<PromptMemorySelectionEvidence>)> {
    let selected = selection
        .selected
        .into_iter()
        .map(|selected| {
            let record = records
                .iter()
                .find(|record| record.id == selected.record_id)
                .ok_or_else(|| CoreError::internal("selected memory record disappeared"))?;
            Ok(SelectedMemory {
                record_id: record.id.clone(),
                branch_id: record.branch_id.clone(),
                content: selected.summary,
                score_millionths: u32::try_from(selected.rank_millionths).unwrap_or(u32::MAX),
                reason: serde_json::to_string(&selected.reasons).map_err(|error| {
                    CoreError::internal(format!("memory evidence could not be encoded: {error}"))
                })?,
                provenance: record.provenance.clone(),
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let evidence = selection
        .evidence
        .into_iter()
        .map(|evidence| PromptMemorySelectionEvidence {
            record_id: evidence.record_id,
            selected: evidence.selected,
            lane: evidence.lane.map(prompt_memory_lane),
            rank_millionths: evidence.rank_millionths,
            estimated_tokens: evidence.estimated_tokens,
            reasons: evidence
                .reasons
                .into_iter()
                .map(prompt_memory_reason)
                .collect(),
            exclusion_reason: evidence.exclusion_reason,
        })
        .collect();
    Ok((selected, evidence))
}

fn orchestration_validation_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!("prompt preset is invalid: {error}"))
}

fn validate_prompt_binding_sources(
    preset: &PromptPreset,
    binding: Option<&PromptPresetBinding>,
) -> CoreResult<()> {
    let needs_author_note = preset.blocks.iter().any(|block| {
        block.enabled && matches!(block.source, lorepia_domain::BlockSource::AuthorNote)
    });
    let needs_group_context = preset.blocks.iter().any(|block| {
        block.enabled && matches!(block.source, lorepia_domain::BlockSource::GroupContext)
    });
    if needs_author_note
        && binding
            .and_then(|binding| binding.author_note.as_ref())
            .is_none()
    {
        return Err(CoreError::invalid(
            "enabled author-note block requires a room author note",
        ));
    }
    if needs_group_context
        && binding
            .and_then(|binding| binding.group_context.as_ref())
            .is_none()
    {
        return Err(CoreError::invalid(
            "enabled group-context block requires room group context",
        ));
    }
    let required_slots = preset
        .blocks
        .iter()
        .filter(|block| block.enabled)
        .filter_map(|block| block.template.as_ref())
        .flat_map(|template| &template.parts)
        .filter_map(|part| match part {
            lorepia_domain::TemplatePart::Slot { name } if name != "block_content" => {
                Some(name.as_str())
            }
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let available_slots = binding
        .map(|binding| {
            binding
                .template_slots
                .iter()
                .map(|slot| slot.name.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if let Some(missing) = required_slots
        .iter()
        .find(|name| !available_slots.contains(**name))
    {
        return Err(CoreError::invalid(format!(
            "enabled prompt template requires unavailable room slot `{missing}`"
        )));
    }
    Ok(())
}

fn prompt_summary_requirements(preset: &PromptPreset) -> (bool, BTreeSet<MemoryRecordId>) {
    let needs_conversation_summary = preset.blocks.iter().any(|block| {
        block.enabled
            && matches!(
                block.source,
                lorepia_domain::BlockSource::ConversationSummary
            )
    });
    let required_summary_ids = preset
        .blocks
        .iter()
        .filter(|block| block.enabled)
        .filter_map(|block| match block.history_selector.as_ref() {
            Some(lorepia_domain::HistorySelector::SinceSummary { summary_id }) => {
                Some(summary_id.clone())
            }
            _ => None,
        })
        .collect();
    (needs_conversation_summary, required_summary_ids)
}

fn empty_prompt_summary_materialization() -> PromptSummaryMaterialization {
    PromptSummaryMaterialization {
        boundaries: Vec::new(),
        conversation_summary: None,
        conversation_summary_id: None,
        evidence: Vec::new(),
    }
}

fn validate_prompt_summary_record(
    stored: &StoredRevision<MemoryRecord>,
    evidence: &lorepia_storage::MemoryRecordAtHeadEvidence,
) -> CoreResult<()> {
    if stored.value.id != evidence.record_id
        || stored.value.branch_id != evidence.record_branch_id
        || stored.value.source_start_message_id != evidence.source_start_message_id
        || stored.value.source_end_message_id != evidence.source_end_message_id
        || stored.revision != evidence.state_revision
        || stored.revision_id.as_deref() != Some(evidence.active_revision_id.as_str())
        || stored.deleted_at.is_some()
    {
        return Err(CoreError::new(
            lorepia_domain::CoreErrorCode::StorageCorrupted,
            "summary memory record differs from its exact-head evidence",
            false,
        ));
    }
    Ok(())
}

fn prompt_summary_evidence(
    evidence: lorepia_storage::MemoryRecordAtHeadEvidence,
) -> PromptSummarySourceEvidence {
    PromptSummarySourceEvidence {
        summary_id: evidence.record_id,
        record_branch_id: evidence.record_branch_id,
        source_start_message_id: evidence.source_start_message_id,
        source_end_message_id: evidence.source_end_message_id,
        state_revision: evidence.state_revision,
        active_revision_id: evidence.active_revision_id,
        active_revision_sha256: evidence.active_revision_sha256,
    }
}

fn select_prompt_summary_materialization(
    visible: &[VisiblePromptSummary],
    needs_conversation_summary: bool,
    required_summary_ids: &BTreeSet<MemoryRecordId>,
    messages: &[PromptConversationMessage],
) -> CoreResult<PromptSummaryMaterialization> {
    let mut ordered = visible.to_vec();
    ordered.sort_by(|left, right| {
        right
            .end_depth
            .cmp(&left.end_depth)
            .then_with(|| left.record.id.cmp(&right.record.id))
    });
    for required in required_summary_ids {
        let Some(summary) = ordered
            .iter()
            .find(|summary| summary.record.id == *required)
        else {
            return Err(CoreError::invalid(format!(
                "prompt history requires unavailable summary `{}`",
                required.as_str()
            )));
        };
        if !messages
            .iter()
            .any(|message| message.id == summary.record.source_end_message_id)
        {
            return Err(CoreError::invalid(format!(
                "summary `{}` ends outside the bounded prompt history",
                required.as_str()
            )));
        }
    }
    let conversation_summary = if needs_conversation_summary {
        Some(ordered.last().ok_or_else(|| {
            CoreError::invalid("enabled conversation-summary block has no visible summary memory")
        })?)
    } else {
        None
    };
    let conversation_summary_id = conversation_summary.map(|summary| summary.record.id.clone());
    let conversation_summary_text =
        conversation_summary.map(|summary| summary.record.summary.clone());
    let mut selected_ids = required_summary_ids.clone();
    if let Some(summary_id) = &conversation_summary_id {
        selected_ids.insert(summary_id.clone());
    }
    let selected = ordered
        .into_iter()
        .filter(|summary| selected_ids.contains(&summary.record.id))
        .collect::<Vec<_>>();
    let boundaries = selected
        .iter()
        .filter(|summary| required_summary_ids.contains(&summary.record.id))
        .map(|summary| SummaryBoundary {
            summary_id: summary.record.id.clone(),
            end_message_id: summary.record.source_end_message_id.clone(),
        })
        .collect();
    Ok(PromptSummaryMaterialization {
        boundaries,
        conversation_summary: conversation_summary_text,
        conversation_summary_id,
        evidence: selected
            .into_iter()
            .map(|summary| summary.evidence)
            .collect(),
    })
}

fn validate_prompt_user_text(text: &str) -> CoreResult<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(CoreError::invalid("message text must not be empty"));
    }
    if trimmed.len() > 64 * 1024 || trimmed.chars().count() > 16 * 1024 {
        return Err(CoreError::invalid(
            "message text exceeds the 65536-byte or 16384-character limit",
        ));
    }
    Ok(trimmed)
}

pub(crate) fn deterministic_prompt_user_message_id(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    text: &str,
) -> MessageId {
    let identity = format!(
        "lorepia:prompt-user-message:v1\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
        conversation_id.0,
        branch_id.0,
        expected_head.map_or("", |id| id.0.as_str()),
        text
    );
    MessageId(Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string())
}

fn deterministic_room_prompt_binding_id(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> String {
    let identity = format!(
        "lorepia:room-prompt-binding:v1\u{0}{}\u{0}{}",
        conversation_id.0, branch_id.0
    );
    Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string()
}

fn canonical_creator_variable_overrides(
    preset: &PromptPreset,
    creator_values: &BTreeMap<String, CreatorControlValue>,
) -> CoreResult<VariableMap> {
    if creator_values.len() > preset.controls.len() {
        return Err(CoreError::invalid(
            "creator values contain more controls than the selected preset",
        ));
    }
    let mut variables = VariableMap::default();
    for (control_id, supplied) in creator_values {
        let control = preset
            .controls
            .iter()
            .find(|control| control.id.as_str() == control_id)
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "creator value references unknown control `{control_id}`"
                ))
            })?;
        if control.sensitive {
            return Err(CoreError::invalid(
                "sensitive creator controls cannot cross the frontend boundary",
            ));
        }
        let variable = control.variable.as_ref().ok_or_else(|| {
            CoreError::invalid("presentation-only controls cannot receive creator values")
        })?;
        if variables.get(variable).is_some() {
            return Err(CoreError::invalid(
                "multiple creator controls cannot override the same variable",
            ));
        }
        let value = canonical_creator_control_value(control, supplied)?;
        variables.insert(variable.clone(), value);
    }
    Ok(variables)
}

fn canonical_creator_control_value(
    control: &ControlSpec,
    supplied: &CreatorControlValue,
) -> CoreResult<VariableValue> {
    let value_type = control
        .value_type
        .ok_or_else(|| CoreError::invalid("creator control has no declared value type"))?;
    let value = match (value_type, supplied) {
        (VariableType::Bool, CreatorControlValue::Bool(value)) => VariableValue::Bool(*value),
        (VariableType::Integer, CreatorControlValue::Integer(value)) => {
            VariableValue::Integer(*value)
        }
        (VariableType::Integer, CreatorControlValue::Decimal(value)) => {
            VariableValue::Integer(exact_i64_from_f64(*value).ok_or_else(|| {
                CoreError::invalid("creator value type does not match the selected preset control")
            })?)
        }
        (VariableType::Decimal, CreatorControlValue::Integer(value)) => {
            VariableValue::Decimal(i64_as_f64(*value)?)
        }
        (VariableType::Decimal, CreatorControlValue::Decimal(value)) if value.is_finite() => {
            VariableValue::Decimal(*value)
        }
        (VariableType::Text, CreatorControlValue::Text(value)) => {
            validate_creator_text(value)?;
            VariableValue::Text(value.clone())
        }
        (VariableType::Enum, CreatorControlValue::Text(value)) => {
            validate_creator_text(value)?;
            VariableValue::Enum(value.clone())
        }
        (VariableType::StringList, CreatorControlValue::StringList(values)) => {
            if values.len() > 1_024 {
                return Err(CoreError::invalid(
                    "creator multi-select contains too many values",
                ));
            }
            let mut unique = std::collections::BTreeSet::new();
            for value in values {
                validate_creator_text(value)?;
                if !unique.insert(value.as_str()) {
                    return Err(CoreError::invalid(
                        "creator multi-select contains duplicate values",
                    ));
                }
            }
            let allowed = control
                .options
                .iter()
                .filter_map(|option| match &option.value {
                    VariableValue::Text(value) | VariableValue::Enum(value) => Some(value),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if values.iter().any(|value| !allowed.contains(&value)) {
                return Err(CoreError::invalid(
                    "creator multi-select value is not a declared option",
                ));
            }
            let canonical = allowed
                .into_iter()
                .filter(|value| unique.contains(value.as_str()))
                .cloned()
                .collect();
            VariableValue::StringList(canonical)
        }
        _ => {
            return Err(CoreError::invalid(
                "creator value type does not match the selected preset control",
            ));
        }
    };
    if control.kind == ControlKind::Select
        && !control.options.iter().any(|option| option.value == value)
    {
        return Err(CoreError::invalid(
            "creator select value is not a declared option",
        ));
    }
    validate_creator_numeric_control(control, &value)?;
    Ok(value)
}

fn validate_creator_text(value: &str) -> CoreResult<()> {
    if value.len() > 262_144 || value.chars().count() > 65_536 || value.contains('\0') {
        return Err(CoreError::invalid(
            "creator text exceeds its safe size or contains a null character",
        ));
    }
    Ok(())
}

fn validate_creator_numeric_control(
    control: &ControlSpec,
    value: &VariableValue,
) -> CoreResult<()> {
    let numeric = match value {
        VariableValue::Integer(value) => i64_as_f64(*value)?,
        VariableValue::Decimal(value) => *value,
        _ => return Ok(()),
    };
    if !numeric.is_finite()
        || control.minimum.is_some_and(|minimum| numeric < minimum)
        || control.maximum.is_some_and(|maximum| numeric > maximum)
    {
        return Err(CoreError::invalid(
            "creator numeric value is outside the declared bounds",
        ));
    }
    if let Some(step) = control.step {
        let origin = control.minimum.unwrap_or(0.0);
        let steps = (numeric - origin) / step;
        let tolerance = f64::EPSILON * steps.abs().max(1.0) * 16.0;
        if (steps - steps.round()).abs() > tolerance {
            return Err(CoreError::invalid(
                "creator numeric value does not match the declared step",
            ));
        }
    }
    Ok(())
}

fn exact_i64_from_f64(value: f64) -> Option<i64> {
    (value.is_finite() && value.fract() == 0.0)
        .then(|| value.to_string().parse::<i64>().ok())
        .flatten()
}

fn i64_as_f64(value: i64) -> CoreResult<f64> {
    value
        .to_string()
        .parse::<f64>()
        .map_err(|_| CoreError::internal("integer creator value could not be converted"))
}

fn creator_values_from_binding(
    preset: &PromptPreset,
    binding: Option<&PromptPresetBinding>,
) -> CoreResult<BTreeMap<String, CreatorControlValue>> {
    let Some(binding) = binding else {
        return Ok(BTreeMap::new());
    };
    let mut values = BTreeMap::new();
    for control in &preset.controls {
        if control.sensitive {
            continue;
        }
        let Some(variable) = &control.variable else {
            continue;
        };
        let Some(value) = binding.variable_overrides.get(variable) else {
            continue;
        };
        let value = match value {
            VariableValue::Bool(value) => CreatorControlValue::Bool(*value),
            VariableValue::Integer(value) => CreatorControlValue::Integer(*value),
            VariableValue::Decimal(value) if value.is_finite() => {
                CreatorControlValue::Decimal(*value)
            }
            VariableValue::Text(value) | VariableValue::Enum(value) => {
                CreatorControlValue::Text(value.clone())
            }
            VariableValue::StringList(values) => CreatorControlValue::StringList(values.clone()),
            VariableValue::Decimal(_) => {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "stored creator value is not finite",
                    false,
                ));
            }
        };
        values.insert(control.id.as_str().to_owned(), value);
    }
    Ok(values)
}

fn merge_variable_map(target: &mut VariableMap, source: &VariableMap) {
    for binding in &source.values {
        target.insert(binding.variable.clone(), binding.value.clone());
    }
}

fn prompt_module_knowledge_revisions(
    books: &[ObjectRevision<KnowledgeBook>],
) -> CoreResult<BTreeMap<KnowledgeEntryId, String>> {
    let mut revisions = BTreeMap::new();
    for book in books {
        for entry in &book.value.entries {
            if revisions
                .insert(entry.id.clone(), book.revision_id.clone())
                .is_some()
            {
                return Err(CoreError::invalid(
                    "approved module knowledge entry IDs are ambiguous",
                ));
            }
        }
    }
    Ok(revisions)
}

fn exact_prompt_manual_knowledge(
    manually_active: &[KnowledgeEntryId],
    bindings: &[InteractionKnowledgeBinding],
    current_revisions: &BTreeMap<KnowledgeEntryId, String>,
) -> CoreResult<BTreeSet<KnowledgeEntryId>> {
    let mut bindings_by_entry = BTreeMap::new();
    for binding in bindings {
        if bindings_by_entry
            .insert(binding.entry_id.clone(), binding)
            .is_some()
        {
            return Err(CoreError::invalid(
                "manual knowledge activation has duplicate revision bindings",
            ));
        }
    }
    let mut exact = BTreeSet::new();
    for entry_id in manually_active {
        let binding = bindings_by_entry.get(entry_id).ok_or_else(|| {
            CoreError::invalid(format!(
                "manual knowledge entry {} has no revision binding",
                entry_id.as_str()
            ))
        })?;
        if current_revisions
            .get(entry_id)
            .is_some_and(|revision| revision.as_str() == binding.book_revision_id.as_str())
        {
            exact.insert(entry_id.clone());
        }
    }
    Ok(exact)
}

fn missing_preset_module_overlay(
    preset: &PromptPreset,
    dependencies: &[lorepia_storage::PromptPresetModuleDependency],
) -> CoreResult<PromptModuleOverlay> {
    if dependencies.is_empty() {
        return Ok(PromptModuleOverlay::default());
    }
    if matches!(
        preset.metadata.provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    ) {
        return Err(CoreError::new(
            lorepia_domain::CoreErrorCode::PermissionDenied,
            "imported prompt preset requires an exact approved module plan",
            false,
        ));
    }
    Ok(PromptModuleOverlay {
        warnings: vec![format!(
            "{} local preset module dependencies were omitted because no exact approved module plan exists",
            dependencies.len()
        )],
        ..PromptModuleOverlay::default()
    })
}

fn initialize_prompt_module_overlay(
    preset: &PromptPreset,
    preset_dependencies: &[lorepia_storage::PromptPresetModuleDependency],
    approved: &AppliedModuleRuntimePlan,
) -> CoreResult<PromptModuleOverlay> {
    let selected_sources = approved
        .plan
        .components
        .iter()
        .flat_map(|component| {
            std::iter::once(&component.selected_source).chain(component.coalesced_sources.iter())
        })
        .collect::<BTreeSet<_>>();
    let missing_dependencies = preset_dependencies
        .iter()
        .filter(|dependency| {
            !selected_sources.iter().any(|source| {
                source.module_id == dependency.module_id
                    && source.revision_id == dependency.module_revision_id
                    && source.revision_source_sha256 == dependency.source_sha256
            })
        })
        .count();
    let mut overlay = PromptModuleOverlay {
        plan_sha256: Some(approved.applied_plan_sha256.as_str().to_owned()),
        variables: approved.plan.effective_variable_overrides.clone(),
        ..PromptModuleOverlay::default()
    };
    if missing_dependencies == 0 {
        return Ok(overlay);
    }
    if matches!(
        preset.metadata.provenance.source_kind,
        SourceKind::ImportedPackage | SourceKind::ImportedStandard
    ) {
        return Err(CoreError::new(
            lorepia_domain::CoreErrorCode::PermissionDenied,
            "imported prompt preset has an unapproved or stale module dependency",
            false,
        ));
    }
    overlay.warnings.push(format!(
        "{missing_dependencies} local preset module dependencies were omitted because the exact approved revision is unavailable"
    ));
    Ok(overlay)
}

fn module_binding_applies_to_prompt(
    binding: &ModuleBinding,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    character_id: &str,
    persona_id: Option<&str>,
) -> bool {
    match binding.scope {
        ModuleScope::App | ModuleScope::User => binding.target_id.is_none(),
        ModuleScope::Persona => binding.target_id.as_deref() == persona_id,
        ModuleScope::Character => binding.target_id.as_deref() == Some(character_id),
        ModuleScope::Conversation => {
            binding.target_id.as_deref() == Some(conversation_id.0.as_str())
        }
        ModuleScope::Branch => {
            binding.target_id.as_deref() == Some(branch_id.0.as_str())
                && binding.conversation_id.as_ref() == Some(conversation_id)
        }
    }
}

fn append_exact_module_transform_sets(
    target: &mut Vec<TransformSet>,
    module_sets: &[ObjectRevision<TransformSet>],
) -> CoreResult<()> {
    for revision in module_sets {
        if target.iter().any(|set| set.id == revision.value.id) {
            return Err(CoreError::invalid(
                "prompt preset and approved module select the same transform set ambiguously",
            ));
        }
        target.push(revision.value.clone());
    }
    Ok(())
}

fn module_plan_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!("approved module plan is invalid: {error}"))
}

pub(crate) fn enforce_application_policy(preset: &mut PromptPreset) {
    let mut built_ins = built_in_prompt_presets();
    let story_policy_index = usize::from(preset.id == built_ins[1].id);
    let application_policy = built_ins[story_policy_index].blocks.remove(0);
    // Replace only the reserved policy slot. Other trusted built-in blocks
    // (including the story-mode instruction and compatibility prompt blocks)
    // deliberately carry application provenance and must survive runtime
    // normalization. Creator writes are separately stripped of application
    // authority by `upsert_prompt_preset`, while module overlays reject that
    // authority before reaching this merge.
    preset
        .blocks
        .retain(|block| block.placement_zone != lorepia_domain::PlacementZone::ApplicationPolicy);
    preset.blocks.insert(0, application_policy);
}

fn is_builtin_prompt_preset_id(id: &PromptPresetId) -> bool {
    built_in_prompt_presets()
        .iter()
        .any(|preset| preset.id == *id)
}

pub(crate) fn apply_transform_sets_with_import_approvals(
    sets: &[TransformSet],
    phase: lorepia_domain::TransformPhase,
    input: &str,
    variables: &VariableMap,
    supported_capabilities: &[CapabilityKey],
    approved_import_source_ids: &BTreeSet<String>,
) -> CoreResult<TransformResult> {
    let pipeline = TransformPipeline::compile_with_options(
        sets,
        TransformLimits::default(),
        &TransformCompileOptions {
            approved_import_source_ids: approved_import_source_ids.clone(),
        },
    )
    .map_err(|error| CoreError::invalid(format!("transform pipeline is invalid: {error}")))?;
    // Runtime transform failures deliberately return the original input. The
    // structured report stays available for diagnostics while generation never
    // consumes a partial or ambiguous transform output.
    Ok(pipeline.apply(
        phase,
        input,
        TransformContext {
            variables,
            model_capabilities: supported_capabilities,
        },
        TransformApplyOptions::default(),
    ))
}

fn apply_resolved_prompt_transforms(
    plan: &ResolvedPromptPlan,
    sets: &[TransformSet],
    variables: &VariableMap,
    supported_capabilities: &[CapabilityKey],
    approved_import_source_ids: &BTreeSet<String>,
) -> CoreResult<(ResolvedPromptPlan, Vec<String>)> {
    if !sets.iter().any(|set| {
        set.enabled
            && set.rules.iter().any(|rule| {
                rule.enabled && rule.phase == lorepia_domain::TransformPhase::ResolvedPrompt
            })
    }) {
        return Ok((plan.clone(), Vec::new()));
    }
    let pipeline = TransformPipeline::compile_with_options(
        sets,
        TransformLimits::default(),
        &TransformCompileOptions {
            approved_import_source_ids: approved_import_source_ids.clone(),
        },
    )
    .map_err(|error| CoreError::invalid(format!("transform pipeline is invalid: {error}")))?;
    let mut transformed_contents = Vec::with_capacity(plan.effective_messages.len());
    let mut warnings = Vec::new();
    let mut changed = false;
    for message in &plan.effective_messages {
        let result = pipeline.apply(
            lorepia_domain::TransformPhase::ResolvedPrompt,
            &message.content,
            TransformContext {
                variables,
                model_capabilities: supported_capabilities,
            },
            TransformApplyOptions {
                allow_resolved_prompt: true,
            },
        );
        if let Some(error) = &result.error {
            warnings.push(format!(
                "resolved-prompt transform failed for block {} and preserved the original text: {:?}",
                message.block_id.as_str(),
                error.code
            ));
        }
        changed |= result.changed;
        transformed_contents.push(result.output);
    }
    if !changed {
        return Ok((plan.clone(), warnings));
    }
    match reseal_resolved_prompt_plan(plan, &transformed_contents) {
        Ok(plan) => Ok((plan, warnings)),
        Err(error) => {
            warnings.push(format!(
                "resolved-prompt transform exceeded the reviewed plan boundary and was ignored: {error}"
            ));
            Ok((plan.clone(), warnings))
        }
    }
}

fn character_prompt_content(
    character: &Character,
    content: &CharacterContentV1,
) -> CharacterPromptContent {
    CharacterPromptContent {
        character_id: character.id.clone(),
        name: character.name.clone(),
        aliases: Vec::new(),
        description: character.description.clone(),
        personality: content.personality.clone(),
        scenario: content.scenario.clone(),
        first_message: content.first_message.clone(),
        dialogue_examples: content.example_dialogs.clone(),
        system_instruction: content.system_instruction.clone(),
        post_history_instruction: content.post_history_instruction.clone(),
        alternate_greetings: content.alternate_greetings.clone(),
        knowledge_book_ids: content
            .knowledge_book
            .as_ref()
            .and_then(|reference| reference.id.clone())
            .into_iter()
            .collect(),
        asset_ids: content
            .assets
            .iter()
            .map(|asset| asset.id.clone())
            .collect(),
    }
}

fn redacted_prompt_preview(
    plan: &ResolvedPromptPlan,
    execution_hash: &str,
    prompt_preset_revision: u64,
    prompt_preset_revision_id: &str,
    generation_target: Option<GenerationTarget>,
    provider: &ProviderCompiledPromptPreview,
    preparation_warnings: &[String],
) -> CoreResult<PromptPlanPreview> {
    verify_resolved_prompt_plan(plan).map_err(orchestration_validation_error)?;
    let mut warnings = plan.trace.warnings.clone();
    warnings.extend_from_slice(preparation_warnings);
    for boundary in &provider.cache_boundaries {
        if let ProviderCacheBoundaryDisposition::Ignored { warning } = boundary.disposition {
            warnings.push(format!(
                "provider ignored cache boundary {}: {warning:?}",
                boundary.boundary_id.as_str()
            ));
        }
    }
    Ok(PromptPlanPreview {
        plan_id: execution_hash.to_owned(),
        plan_hash: execution_hash.to_owned(),
        neutral_plan_hash: plan.plan_hash.clone(),
        prompt_preset_id: plan.preset_id.clone(),
        prompt_preset_revision,
        prompt_preset_revision_id: prompt_preset_revision_id.to_owned(),
        generation_target,
        estimated_input_tokens: plan.trace.estimated_input_tokens,
        available_input_tokens: plan.trace.available_input_tokens,
        token_estimator_id: plan.trace.estimator_id.clone(),
        token_estimate_exact: false,
        messages: plan
            .effective_messages
            .iter()
            .map(|message| PromptPlanMessagePreview {
                sequence: message.sequence,
                block_id: message.block_id.clone(),
                block_kind: message.block_kind,
                requested_role: message.requested_role,
                effective_role: message.effective_role,
                estimated_tokens: message.estimated_tokens,
                source_message_ids: message.source_message_ids.clone(),
            })
            .collect(),
        provider_family: provider.family,
        provider_messages: provider
            .messages
            .iter()
            .map(|message| PromptProviderMessagePreview {
                sequence: message.sequence,
                block_id: message.block_id.clone(),
                effective_role: message.effective_role,
                wire_role: message.wire_role,
                placement: message.placement,
                estimated_tokens: message.estimated_tokens,
            })
            .collect(),
        provider_cache_boundaries: provider.cache_boundaries.clone(),
        cache_directives: plan.cache_directives.clone(),
        blocks: plan.trace.blocks.clone(),
        role_mappings: plan.trace.role_mappings.clone(),
        overflow: plan.trace.overflow.clone(),
        warnings,
    })
}

fn provider_cacheable_prefix_tokens(provider: &ProviderCompiledPromptPreview) -> u32 {
    let last_applied_sequence = provider
        .cache_boundaries
        .iter()
        .filter(|boundary| {
            matches!(
                boundary.disposition,
                ProviderCacheBoundaryDisposition::Mapped { .. }
            )
        })
        .filter_map(|boundary| boundary.after_message_sequence)
        .max();
    last_applied_sequence.map_or(0, |last| {
        provider
            .messages
            .iter()
            .filter(|message| message.sequence <= last)
            .map(|message| message.estimated_tokens)
            .fold(0_u32, u32::saturating_add)
    })
}

fn cacheable_prefix_has_volatile_before_fixed_after(
    plan: &ResolvedPromptPlan,
    provider: &ProviderCompiledPromptPreview,
) -> bool {
    let Some(last_applied_sequence) = provider
        .cache_boundaries
        .iter()
        .filter(|boundary| {
            matches!(
                boundary.disposition,
                ProviderCacheBoundaryDisposition::Mapped { .. }
            )
        })
        .filter_map(|boundary| boundary.after_message_sequence)
        .max()
    else {
        return false;
    };
    let volatile_before = plan.effective_messages.iter().any(|message| {
        message.sequence <= last_applied_sequence && prompt_block_is_volatile(message.block_kind)
    });
    let fixed_after = plan.effective_messages.iter().any(|message| {
        message.sequence > last_applied_sequence && !prompt_block_is_volatile(message.block_kind)
    });
    volatile_before && fixed_after
}

const fn prompt_block_is_volatile(kind: lorepia_domain::PromptBlockKind) -> bool {
    matches!(
        kind,
        lorepia_domain::PromptBlockKind::WorldKnowledge
            | lorepia_domain::PromptBlockKind::RetrievedMemory
            | lorepia_domain::PromptBlockKind::ConversationSummary
            | lorepia_domain::PromptBlockKind::HistorySlice
            | lorepia_domain::PromptBlockKind::LatestUserTurn
            | lorepia_domain::PromptBlockKind::AuthorNote
            | lorepia_domain::PromptBlockKind::AssistantPrefill
            | lorepia_domain::PromptBlockKind::GroupContext
    )
}

fn estimate_prompt_memory_tokens(_title: &str, summary: &str) -> u32 {
    if summary.is_empty() {
        0
    } else {
        u32::try_from(summary.len().div_ceil(4)).unwrap_or(u32::MAX)
    }
}

fn activation_rule_uses_semantic(rule: &ActivationRule) -> bool {
    match rule {
        ActivationRule::Semantic { .. } => true,
        ActivationRule::Any { rules } | ActivationRule::All { rules } => {
            rules.iter().any(activation_rule_uses_semantic)
        }
        ActivationRule::Always
        | ActivationRule::Manual
        | ActivationRule::Keyword { .. }
        | ActivationRule::Regex { .. }
        | ActivationRule::Condition { .. } => false,
    }
}

fn lexical_knowledge_semantic_scores_with_budget(
    book: &KnowledgeBook,
    scan_texts: &[String],
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<Vec<SemanticKnowledgeScore>> {
    const MAX_SCAN_CHARS: usize = 512 * 1_024;
    let depth = usize::try_from(book.scan_depth).unwrap_or(usize::MAX);
    let start = scan_texts.len().saturating_sub(depth);
    let query_chars = normalized_semantic_characters(
        scan_texts[start..]
            .iter()
            .flat_map(|text| text.chars())
            .take(MAX_SCAN_CHARS),
        book.id.as_str(),
        work_budget,
    )?;
    let mut scores = book
        .entries
        .iter()
        .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
        .map(|entry| -> CoreResult<_> {
            let candidate_chars = normalized_semantic_characters(
                entry
                    .name
                    .chars()
                    .chain(entry.content.chars())
                    .take(MAX_SCAN_CHARS),
                entry.id.as_str(),
                work_budget,
            )?;
            let comparison_work = query_chars
                .len()
                .saturating_add(candidate_chars.len())
                .saturating_mul(2);
            work_budget
                .charge_work_bytes(entry.id.as_str(), comparison_work)
                .map_err(orchestration_validation_error)?;
            let union = query_chars.union(&candidate_chars).count();
            let intersection = query_chars.intersection(&candidate_chars).count();
            Ok(SemanticKnowledgeScore {
                entry_id: entry.id.clone(),
                score: if union == 0 {
                    0.0
                } else {
                    jaccard_score(intersection, union)?
                },
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    scores.sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
    Ok(scores)
}

pub(crate) fn charge_provider_knowledge_work(
    scope_id: &str,
    work_budget: &mut KnowledgeWorkBudget,
    work_bytes: usize,
) -> CoreResult<()> {
    work_budget
        .charge_work_bytes(scope_id, work_bytes)
        .map_err(orchestration_validation_error)
}

fn normalized_semantic_characters(
    characters: impl Iterator<Item = char>,
    scope_id: &str,
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<BTreeSet<char>> {
    let mut normalized = BTreeSet::new();
    for character in characters {
        work_budget
            .charge_work_bytes(scope_id, character.len_utf8())
            .map_err(orchestration_validation_error)?;
        normalized.extend(
            character
                .to_lowercase()
                .filter(|character| character.is_alphanumeric()),
        );
    }
    Ok(normalized)
}

fn jaccard_score(intersection: usize, union: usize) -> CoreResult<f32> {
    if intersection > union || union == 0 {
        return Err(CoreError::internal(
            "knowledge semantic Jaccard cardinality is invalid",
        ));
    }
    let intersection = u64::try_from(intersection)
        .map_err(|_| CoreError::internal("knowledge semantic intersection overflowed"))?;
    let union = u64::try_from(union)
        .map_err(|_| CoreError::internal("knowledge semantic union overflowed"))?;
    let rounded_millionths = intersection
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(union / 2))
        .ok_or_else(|| CoreError::internal("knowledge semantic score overflowed"))?
        / union;
    semantic_score_from_millionths(
        u32::try_from(rounded_millionths)
            .map_err(|_| CoreError::internal("knowledge semantic score overflowed"))?,
    )
}

pub(crate) fn semantic_score_from_millionths(millionths: u32) -> CoreResult<f32> {
    if millionths > 1_000_000 {
        return Err(CoreError::internal(
            "knowledge semantic score exceeds one million millionths",
        ));
    }
    let thousands = u16::try_from(millionths / 1_000)
        .map_err(|_| CoreError::internal("knowledge semantic score overflowed"))?;
    let remainder = u16::try_from(millionths % 1_000)
        .map_err(|_| CoreError::internal("knowledge semantic score overflowed"))?;
    Ok((f32::from(thousands) * 1_000.0 + f32::from(remainder)) / 1_000_000.0)
}

fn knowledge_semantic_query_sha256(
    book: &KnowledgeBook,
    scan_texts: &[String],
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<String> {
    let depth = usize::try_from(book.scan_depth).unwrap_or(usize::MAX);
    let start = scan_texts.len().saturating_sub(depth);
    let hash_work = scan_texts[start..]
        .iter()
        .fold(0_usize, |total, text| total.saturating_add(text.len()))
        .saturating_mul(6);
    work_budget
        .charge_work_bytes(book.id.as_str(), hash_work)
        .map_err(orchestration_validation_error)?;
    let encoded = serde_json::to_vec(&("lorepia.knowledge-lexical-query.v1", &scan_texts[start..]))
        .map_err(|error| {
            CoreError::internal(format!(
                "cannot encode knowledge semantic query evidence: {error}"
            ))
        })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn knowledge_semantic_scores_sha256(
    book_revision_id: &str,
    scores: &[SemanticKnowledgeScore],
    scope_id: &str,
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<String> {
    let fixed = scores
        .iter()
        .map(|score| {
            work_budget
                .charge_work_bytes(
                    scope_id,
                    score
                        .entry_id
                        .as_str()
                        .len()
                        .saturating_add(std::mem::size_of::<u32>()),
                )
                .map_err(orchestration_validation_error)?;
            if !score.score.is_finite() || !(0.0..=1.0).contains(&score.score) {
                return Err(CoreError::internal(
                    "knowledge semantic score is outside the canonical domain",
                ));
            }
            Ok((
                score.entry_id.as_str(),
                semantic_score_millionths(score.score)?,
            ))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let encoded = serde_json::to_vec(&(
        "lorepia.knowledge-semantic-scores.v1",
        book_revision_id,
        fixed,
    ))
    .map_err(|error| {
        CoreError::internal(format!(
            "cannot encode knowledge semantic score evidence: {error}"
        ))
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn semantic_score_millionths(score: f32) -> CoreResult<u32> {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return Err(CoreError::internal(
            "knowledge semantic score is outside the canonical domain",
        ));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fixed = (score * 1_000_000.0).round() as u32;
    Ok(fixed)
}

fn knowledge_embedding_matches_sha256(
    book_revision_id: &str,
    matches: &[KnowledgeEmbeddingMatch],
    scope_id: &str,
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<String> {
    let mut writer = BudgetedKnowledgeMatchHasher {
        hasher: Sha256::new(),
        scope_id,
        work_budget,
        exhausted: false,
    };
    if let Err(error) = serde_json::to_writer(
        &mut writer,
        &(
            "lorepia.knowledge-embedding-matches.v1",
            book_revision_id,
            matches,
        ),
    ) {
        if writer.exhausted {
            return Err(CoreError::invalid(
                "knowledge embedding match evidence exceeds the generation work budget",
            ));
        }
        return Err(CoreError::internal(format!(
            "cannot encode knowledge embedding match evidence: {error}"
        )));
    }
    Ok(format!("{:x}", writer.hasher.finalize()))
}

struct BudgetedKnowledgeMatchHasher<'a> {
    hasher: Sha256,
    scope_id: &'a str,
    work_budget: &'a mut KnowledgeWorkBudget,
    exhausted: bool,
}

impl std::io::Write for BudgetedKnowledgeMatchHasher<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if charge_provider_knowledge_work(self.scope_id, self.work_budget, bytes.len()).is_err() {
            self.exhausted = true;
            return Err(std::io::Error::other(
                "knowledge embedding match evidence budget exhausted",
            ));
        }
        self.hasher.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn prompt_memory_semantic_scores(
    records: &[MemoryRecord],
    messages: &[PromptConversationMessage],
) -> Vec<MemorySemanticScore> {
    lexical_memory_semantic_scores(
        records,
        messages
            .iter()
            .rev()
            .map(|message| message.content.as_str()),
    )
}

fn memory_semantic_evidence_matches_profile(
    evidence: &MemorySemanticQueryEvidence,
    _profile_id: &MemoryProfileId,
    revision_id: &str,
) -> bool {
    match evidence {
        MemorySemanticQueryEvidence::LexicalV1 {
            memory_profile_revision_id,
            ..
        }
        | MemorySemanticQueryEvidence::ProviderEmbeddingV1 {
            memory_profile_revision_id,
            ..
        } => memory_profile_revision_id == revision_id,
    }
}

fn lexical_memory_semantic_scores<'a>(
    records: &[MemoryRecord],
    query_texts: impl IntoIterator<Item = &'a str>,
) -> Vec<MemorySemanticScore> {
    const MAX_QUERY_MESSAGES: usize = 32;
    const MAX_QUERY_CHARS: usize = 65_536;
    let query_chars = query_texts
        .into_iter()
        .take(MAX_QUERY_MESSAGES)
        .flat_map(str::chars)
        .take(MAX_QUERY_CHARS)
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect::<BTreeSet<_>>();
    records
        .iter()
        .map(|record| {
            let candidate_chars = record
                .title
                .chars()
                .chain(record.summary.chars())
                .flat_map(char::to_lowercase)
                .filter(|character| character.is_alphanumeric())
                .collect::<BTreeSet<_>>();
            let union = query_chars.union(&candidate_chars).count();
            let intersection = query_chars.intersection(&candidate_chars).count();
            let score = if union == 0 {
                0.0
            } else {
                usize_as_f32(intersection) / usize_as_f32(union)
            };
            MemorySemanticScore {
                record_id: record.id.clone(),
                score,
            }
        })
        .collect()
}

fn usize_as_f32(mut value: usize) -> f32 {
    let mut result = 0.0_f32;
    let mut place = 1.0_f32;
    while value != 0 {
        let chunk = u16::try_from(value & 0xffff).unwrap_or(u16::MAX);
        result += f32::from(chunk) * place;
        value >>= 16;
        place *= 65_536.0;
    }
    result
}

fn prompt_creativity_temperature(creativity: u8) -> f64 {
    // Preserve the product's 0.015 step through a JSON round trip. Multiplying
    // directly by a binary floating-point literal can serialize values such as
    // 90 as 1.3499999999999999 and then normalize to 1.35 when decoded.
    f64::from(u16::from(creativity) * 15) / 1_000.0
}

fn canonical_prompt_capabilities(
    capabilities: Vec<CapabilityKey>,
) -> CoreResult<Vec<CapabilityKey>> {
    let mut keyed = capabilities
        .into_iter()
        .map(|capability| {
            serde_json::to_string(&capability)
                .map(|key| (key, capability))
                .map_err(|error| {
                    CoreError::internal(format!("prompt capability cannot be encoded: {error}"))
                })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    keyed.dedup_by(|left, right| left.0 == right.0);
    Ok(keyed
        .into_iter()
        .map(|(_, capability)| capability)
        .collect())
}

const fn prompt_memory_lane(lane: MemorySelectionLane) -> PromptMemorySelectionLane {
    match lane {
        MemorySelectionLane::Pinned => PromptMemorySelectionLane::Pinned,
        MemorySelectionLane::Semantic => PromptMemorySelectionLane::Semantic,
        MemorySelectionLane::Episodic => PromptMemorySelectionLane::Episodic,
    }
}

fn prompt_memory_reason(reason: MemorySelectionReason) -> PromptMemorySelectionReason {
    match reason {
        MemorySelectionReason::Pinned => PromptMemorySelectionReason::Pinned,
        MemorySelectionReason::CurrentBranch => PromptMemorySelectionReason::CurrentBranch,
        MemorySelectionReason::SharedAncestor { source_branch_id } => {
            PromptMemorySelectionReason::SharedAncestor { source_branch_id }
        }
        MemorySelectionReason::Recency { score_millionths } => {
            PromptMemorySelectionReason::Recency { score_millionths }
        }
        MemorySelectionReason::Similarity { score_millionths } => {
            PromptMemorySelectionReason::Similarity { score_millionths }
        }
        MemorySelectionReason::Importance { score_millionths } => {
            PromptMemorySelectionReason::Importance { score_millionths }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prompt_execution_hash(
    plan: &ResolvedPromptPlan,
    prompt_preset_revision_id: &str,
    generation_target: Option<&GenerationTarget>,
    provider: &PromptProviderResolution,
    provider_preview: &ProviderCompiledPromptPreview,
    temperature: Option<f64>,
    response_length: PromptResponseLength,
    creativity: u8,
    requested_reasoning_effort: Option<GenerationReasoningEffort>,
    memory_enabled: bool,
    knowledge_enabled: bool,
    variables: &VariableMap,
    transform_sets: &[TransformSet],
    module_plan_sha256: Option<&str>,
    approved_import_source_ids: &BTreeSet<String>,
    memory_semantic_evidence: Option<&MemorySemanticQueryEvidence>,
    knowledge_semantic_evidence: &[KnowledgeSemanticBookEvidence],
) -> CoreResult<String> {
    #[derive(serde::Serialize)]
    struct ExecutionIdentity<'a> {
        schema_version: u32,
        neutral_plan_hash: &'a str,
        prompt_preset_revision_id: &'a str,
        generation_target: Option<&'a GenerationTarget>,
        provider_family: ApiFamily,
        developer_capability: DeveloperRoleCapability,
        cache_dialect: PromptCacheWireDialect,
        request_plan_sha256: &'a str,
        generation_preset_sha256: &'a str,
        context_limit_tokens: u32,
        reserved_output_tokens: u32,
        temperature: Option<f64>,
        response_length: PromptResponseLength,
        creativity: u8,
        requested_reasoning_effort: Option<GenerationReasoningEffort>,
        reasoning_effort_applied: Option<GenerationReasoningEffort>,
        memory_enabled: bool,
        knowledge_enabled: bool,
        variables: &'a VariableMap,
        transform_sets: &'a [TransformSet],
        module_plan_sha256: Option<&'a str>,
        approved_import_source_ids: &'a BTreeSet<String>,
        provider_preview: &'a ProviderCompiledPromptPreview,
        memory_semantic_evidence: Option<&'a MemorySemanticQueryEvidence>,
        knowledge_semantic_evidence: &'a [KnowledgeSemanticBookEvidence],
    }

    let encoded = serde_json::to_vec(&ExecutionIdentity {
        schema_version: 1,
        neutral_plan_hash: &plan.plan_hash,
        prompt_preset_revision_id,
        generation_target,
        provider_family: provider.adapter.family(),
        developer_capability: provider.developer_capability,
        cache_dialect: provider.cache_dialect,
        request_plan_sha256: &provider.request_plan_sha256,
        generation_preset_sha256: &provider.generation_preset_sha256,
        context_limit_tokens: provider.max_context_tokens,
        reserved_output_tokens: provider.reserved_output_tokens,
        temperature,
        response_length,
        creativity,
        requested_reasoning_effort,
        reasoning_effort_applied: provider.reasoning_effort_applied,
        memory_enabled,
        knowledge_enabled,
        variables,
        transform_sets,
        module_plan_sha256,
        approved_import_source_ids,
        provider_preview,
        memory_semantic_evidence,
        knowledge_semantic_evidence,
    })
    .map_err(|error| {
        CoreError::internal(format!("cannot encode prompt execution identity: {error}"))
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

#[cfg(test)]
mod prompt_manual_knowledge_revision_tests {
    use std::collections::BTreeMap;

    use lorepia_domain::KnowledgeEntryId;
    use lorepia_storage::InteractionKnowledgeBinding;

    use super::exact_prompt_manual_knowledge;

    #[test]
    fn prompt_manual_activation_requires_the_exact_current_book_revision() {
        let entry_id = KnowledgeEntryId::from("shared-entry");
        let active = [entry_id.clone()];
        let old_binding = [InteractionKnowledgeBinding {
            book_revision_id: "book-old".to_owned(),
            entry_id: entry_id.clone(),
        }];
        let current = BTreeMap::from([(entry_id.clone(), "book-new".to_owned())]);

        let stale = exact_prompt_manual_knowledge(&active, &old_binding, &current)
            .expect("stale state remains readable but inert");
        assert!(stale.is_empty());

        let exact_binding = [InteractionKnowledgeBinding {
            book_revision_id: "book-new".to_owned(),
            entry_id: entry_id.clone(),
        }];
        let exact = exact_prompt_manual_knowledge(&active, &exact_binding, &current)
            .expect("exact binding");
        assert!(exact.contains(&entry_id));
    }
}

#[cfg(test)]
mod knowledge_work_budget_tests {
    use lorepia_domain::{
        ActivationRule, KnowledgeBook, KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId,
        KnowledgePlacement, Provenance, SourceKind, TokenBudget, TokenPolicy,
    };
    use lorepia_orchestration::KnowledgeWorkBudget;
    use lorepia_storage::KnowledgeEmbeddingMatch;

    use super::{
        charge_provider_knowledge_work, knowledge_embedding_matches_sha256,
        lexical_knowledge_semantic_scores_with_budget,
    };

    fn semantic_only_book() -> KnowledgeBook {
        let book_id = KnowledgeBookId::from("semantic-budget-book");
        let provenance = Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: None,
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        };
        KnowledgeBook {
            id: book_id.clone(),
            name: "Semantic budget book".to_owned(),
            schema_version: 1,
            entries: vec![KnowledgeEntry {
                id: KnowledgeEntryId::from("semantic-entry"),
                book_id,
                name: "Semantic entry".to_owned(),
                content: "semantic fallback candidate text".repeat(8),
                enabled: true,
                activation: ActivationRule::Semantic {
                    threshold: 0.0,
                    top_k: 1,
                },
                priority: 0,
                importance: 0,
                placement: KnowledgePlacement::RetrievedContext,
                token_policy: TokenPolicy {
                    priority: 0,
                    min_tokens: None,
                    max_tokens: None,
                    reserve_tokens: None,
                },
                parent_id: None,
                activation_probability_basis_points: 10_000,
                provenance: provenance.clone(),
            }],
            scan_depth: 8,
            token_budget: TokenBudget { max_tokens: 1_024 },
            recursive: false,
            max_recursion_depth: 0,
            provenance,
        }
    }

    #[test]
    fn semantic_only_fallback_exhausts_the_generation_budget() {
        let book = semantic_only_book();
        let scan = vec!["semantic fallback query".repeat(8)];
        let mut measurement = KnowledgeWorkBudget::default();
        let scores = lexical_knowledge_semantic_scores_with_budget(&book, &scan, &mut measurement)
            .expect("semantic fallback fits the default budget");
        assert_eq!(scores.len(), 1);
        let one_fallback_work = measurement.used_work_bytes();
        assert!(one_fallback_work > 0, "semantic fallback must be charged");

        let mut exhausted =
            KnowledgeWorkBudget::with_max_work_bytes(one_fallback_work.saturating_sub(1));
        assert!(
            lexical_knowledge_semantic_scores_with_budget(&book, &scan, &mut exhausted).is_err()
        );
    }

    #[test]
    fn provider_and_lexical_work_share_one_generation_budget() {
        let book = semantic_only_book();
        let scan = vec!["combined provider and lexical query".repeat(8)];
        let mut measurement = KnowledgeWorkBudget::default();
        lexical_knowledge_semantic_scores_with_budget(&book, &scan, &mut measurement)
            .expect("measure lexical fallback work");
        let lexical_work = measurement.used_work_bytes();
        let provider_work = 256_usize;
        let combined_limit = provider_work
            .checked_add(lexical_work)
            .expect("combined work fits usize")
            .saturating_sub(1);
        let mut combined = KnowledgeWorkBudget::with_max_work_bytes(combined_limit);

        charge_provider_knowledge_work(book.id.as_str(), &mut combined, provider_work)
            .expect("provider work fits before lexical fallback");
        assert!(
            lexical_knowledge_semantic_scores_with_budget(&book, &scan, &mut combined).is_err(),
            "provider work must reduce the budget available to lexical fallback"
        );
    }

    #[test]
    fn provider_match_evidence_hash_uses_the_generation_budget() {
        let matches = [KnowledgeEmbeddingMatch {
            embedding_id: "embedding:budgeted-match".to_owned(),
            entry_id: KnowledgeEntryId::from("entry:budgeted-match"),
            vector_sha256: "a".repeat(64),
            similarity_millionths: 750_000,
        }];
        let mut measurement = KnowledgeWorkBudget::default();
        knowledge_embedding_matches_sha256(
            "book-revision:budgeted-match",
            &matches,
            "book:budgeted-match",
            &mut measurement,
        )
        .expect("measure provider match evidence hash work");
        let hash_work = measurement.used_work_bytes();
        assert!(hash_work > 0, "provider match hash must be charged");

        let mut exhausted = KnowledgeWorkBudget::with_max_work_bytes(hash_work.saturating_sub(1));
        assert!(
            knowledge_embedding_matches_sha256(
                "book-revision:budgeted-match",
                &matches,
                "book:budgeted-match",
                &mut exhausted,
            )
            .is_err()
        );
    }
}
