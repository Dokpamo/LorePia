use super::{
    PreparedGenerationPlan, PromptAppliedParameterPreview, PromptEffectiveMessageContentPreview,
    PromptProviderMessagePreview,
};
use crate::{Core, orchestration_runtime::TaskCredentialBroker};
use chrono::{DateTime, Utc};
use lorepia_chat::{MAX_HISTORY_MESSAGE_BYTES, MAX_HISTORY_MESSAGE_CHARS, MAX_PROMPT_MESSAGES};
use lorepia_domain::{
    ApiFamily, BlockResolutionTrace, Character, ConversationBranchId, ConversationId,
    ConversationMode, CoreError, CoreResult, GenerationId, GenerationTarget, Message, MessageId,
    OverflowTrace, PromptPresetId, PromptResolutionTrace, ProviderMessageRole,
    ResolvedCacheDirective, RoleHint, RoleMappingTrace, VariableMap,
};
use lorepia_orchestration::{AppliedModuleRuntimePlan, KnowledgeWorkBudget};
use lorepia_providers::ProviderCacheBoundaryCompilation;
use lorepia_storage::{
    GenerationPromptPlanRecord, GenerationPromptSelectionAuthority,
    GenerationProviderTargetAuthority, StoredInteractionState,
    generation_prompt_selection_authority_sha256,
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
