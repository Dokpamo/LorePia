use std::{sync::Arc, time::Duration};

#[cfg(test)]
use lorepia_domain::{ApiFamily, GenerationTarget};
use lorepia_domain::{
    ConversationBranchId, ConversationId, ConversationMode, CoreResult, GenerationId,
    GenerationRecord, GenerationStatus, Message, MessageId, ProviderProfile, VariableMap,
};
use lorepia_providers::OpenAiCompatibleProvider;
#[cfg(test)]
use lorepia_providers::Provider;
#[cfg(test)]
use lorepia_storage::ProviderCredentialAccessAuthority;

use super::{
    GenerationOperationContext, ResolvedGenerationTarget, ReviewedPromptSendContext,
    SameBranchGenerationAttemptIdentity, configure_generation_protocol_request,
    provider_profile_temporal_context,
};
#[cfg(test)]
use super::{
    direct_model_temporal_context, generation_target_temporal_context,
    validate_generation_target_plan,
};
use crate::app::{CORE_MAX_OUTPUT_TOKENS, Core, GenerationTransformContext};

impl Core {
    pub(in crate::app) fn launch_reviewed_prompt_send(
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

    #[cfg(test)]
    pub(in crate::app) fn send_message_with_provider(
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
    pub(in crate::app) fn send_message_to_branch_with_provider_profile(
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
    pub(in crate::app) fn send_message_to_branch_with_provider(
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
    pub(in crate::app) fn send_message_to_branch_with_provider_options(
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
