//! High-level use cases for prompt orchestration and creator-owned content.
//!
//! Storage documents remain revisioned and all writes use explicit optimistic
//! concurrency. Prompt rendering stays in `lorepia-orchestration`; this module
//! coordinates that pure engine with conversation, branch, and provider state.

mod knowledge;
mod memory;
mod modules;
mod presets;
mod semantic;
mod targets;
mod transforms;
mod variables;

#[allow(unused_imports)]
pub(crate) use knowledge::KnowledgeSemanticScoreSourceEvidence;
pub(crate) use knowledge::{KnowledgeSemanticBookEvidence, KnowledgeSemanticProviderRequirement};
pub use knowledge::{KnowledgeSimulationRequest, KnowledgeTokenEstimate};
pub use memory::MemoryRetrievalRequest;
pub use modules::ContentShareGate;
use modules::{
    PromptModuleOverlay, PromptModuleOverlayInput, exact_prompt_manual_knowledge,
    prompt_module_knowledge_revisions,
};
pub(crate) use presets::enforce_application_policy;
pub use presets::{PromptPresetRollbackApplyRequest, PromptPresetRollbackReceipt};
use semantic::{
    activation_rule_uses_semantic, knowledge_embedding_matches_sha256,
    knowledge_semantic_query_sha256, knowledge_semantic_scores_sha256,
    lexical_knowledge_semantic_scores_with_budget,
};
pub(crate) use semantic::{charge_provider_knowledge_work, semantic_score_from_millionths};
pub use targets::{
    PromptAppliedParameterPreview, PromptEffectiveMessageContentPreview,
    PromptProviderMessagePreview, TaskGenerationTargetPlan,
};
pub use transforms::TransformPreviewRequest;
pub(crate) use transforms::apply_transform_sets_with_import_approvals;
pub use variables::{CreatorControlValue, RoomOrchestrationConfig, RoomOrchestrationConfigPatch};

use memory::PromptContextMaterialization;
use presets::{
    PromptPersonaMaterialization, PromptPresetPreparation, orchestration_validation_error,
    validate_prompt_binding_sources,
};
use targets::{
    PromptProviderResolution, cacheable_prefix_has_volatile_before_fixed_after,
    canonical_prompt_capabilities, prompt_execution_hash, provider_cacheable_prefix_tokens,
    redacted_prompt_preview,
};
use transforms::{PromptTransformPreparation, apply_resolved_prompt_transforms};
use variables::{PromptQuickSettings, PromptVariableState, prompt_creativity_temperature};

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_chat::{
    MAX_HISTORY_MESSAGE_BYTES, MAX_HISTORY_MESSAGE_CHARS, MAX_PROMPT_MESSAGES,
    MaterializedPromptPlan, PromptPlanner,
};
use lorepia_domain::{
    ApiFamily, BlockResolutionTrace, CapabilityKey, Character, CharacterContentV1,
    CharacterPromptContent, ConversationMode, KnowledgeBook, KnowledgeEntryId, MemoryProfile,
    Message, MessageId, MessageRole, OverflowTrace, PromptContextBindingEvidence,
    PromptContextSnapshotV1, PromptConversationMessage, PromptMemorySelectionEvidence,
    PromptMessageRole, PromptPreset, PromptPresetId, PromptResolutionContext,
    PromptResolutionTrace, PromptResolveRequest, ProviderMessageRole, ResolvedCacheDirective,
    ResolvedPromptPlan, RoleHint, RoleMappingTrace, SelectedKnowledge, SelectedMemory,
    TransformSet, TransformSetId, VariableMap, VersionedJson, prompt_context_snapshot_sha256,
    prompt_local_user_id_sha256,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, KnowledgeWorkBudget, TransformResult,
    reseal_prompt_resolution_evidence, resolve_prompt_plan as resolve_prompt_plan_engine,
    verify_resolved_prompt_plan,
};
use lorepia_providers::{ProviderCacheBoundaryCompilation, ProviderCompiledPromptPreview};
use lorepia_storage::{
    GenerationPromptPlanRecord, GenerationPromptSelectionAuthority,
    GenerationProviderTargetAuthority, KnowledgeActivationLog, ObjectRevision, PromptPresetBinding,
    ProviderRequestSnapshotRecord, StoredInteractionState, StoredRevision,
    generation_prompt_selection_authority_sha256,
};

use crate::{
    Core,
    orchestration_runtime::{
        MemorySemanticQueryEvidence, ResolvedMemorySemanticQuery, TaskCredentialBroker,
    },
};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId, GenerationTarget,
};
use uuid::Uuid;

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
#[serde(deny_unknown_fields)]
pub struct PromptDiffEntry {
    pub sequence: u32,
    pub block_id: lorepia_domain::PromptBlockId,
    pub changes: Vec<String>,
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
