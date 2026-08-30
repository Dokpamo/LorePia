use std::sync::Arc;

use lorepia_chat::{MAX_HISTORY_MESSAGE_BYTES, MAX_HISTORY_MESSAGE_CHARS, MAX_PROMPT_MESSAGES};
use lorepia_domain::{
    ApiFamily, ConversationBranchId, ConversationId, ConversationMode, CoreError, CoreErrorCode,
    CoreResult, GenerationId, GenerationRecord, GenerationStatus, GenerationTarget, Message,
    MessageId, VariableMap,
};
use lorepia_providers::Provider;
use lorepia_storage::ProviderCredentialAccessAuthority;
use tokio::sync::watch;

use super::{
    GenerationActionTargetIdentity, GenerationCredential, GenerationCredentialAdmissionLease,
    GenerationOperationContext, GenerationProviderTemporalContext,
    PreparedSameBranchGenerationAttempt, PromptRouteWireContract, SameBranchGenerationAttempt,
    configure_generation_protocol_request, generation_attempt_prompt_authority,
    reviewed_prompt_session_seed, snapshot_provider_request,
};
use crate::{
    app::{Core, GenerationTransformContext, validate_user_message_text},
    orchestration::{GenerationPlanInput, deterministic_prompt_user_message_id},
};

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

impl Core {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn send_message_to_branch_with_provider_options_and_contract(
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
    pub(in crate::app) async fn send_message_to_branch_with_provider_options_and_contract_async(
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
}
