use std::sync::Arc;

use chrono::{DateTime, Utc};
use lorepia_chat::{MAX_HISTORY_MESSAGE_BYTES, MAX_HISTORY_MESSAGE_CHARS, MAX_PROMPT_MESSAGES};
use lorepia_domain::{
    ApiFamily, ConversationBranch, CoreError, CoreErrorCode, CoreResult, GenerationId,
    GenerationRecord, GenerationStatus, GenerationTarget, Message, MessageActionGeneration,
    VariableMap,
};
#[cfg(test)]
use lorepia_domain::{ConversationBranchId, ConversationId, MessageId};
use lorepia_providers::Provider;
#[cfg(test)]
use lorepia_storage::MessageGenerationAction;
use lorepia_storage::{GenerationProviderTargetAuthority, ProviderCredentialAccessAuthority};
#[cfg(test)]
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use super::{
    GenerationActionTargetIdentity, GenerationCredential, GenerationCredentialAdmissionLease,
    PromptRouteWireContract, configure_generation_protocol_request,
    generation_target_provider_authority, provider_profile_target_authority,
    reviewed_prompt_session_seed, snapshot_provider_request,
    validate_generation_target_plan_with_reasoning_effort,
};
#[cfg(test)]
use super::{
    GenerationOperationContext, MessageGenerationActionIdentityInput,
    direct_model_provider_target_authority,
};
#[cfg(test)]
use crate::app::CORE_MAX_OUTPUT_TOKENS;
use crate::{
    app::{Core, GenerationTransformContext},
    orchestration::{GenerationPlanInput, deterministic_prompt_user_message_id},
};

use super::message_actions::{
    MessageActionAttempt, MessageGenerationAttemptConfiguration, PreparedMessageActionAttempt,
    PreparedMessageGenerationAction,
};

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

impl Core {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(in crate::app) fn edit_user_message_with_provider(
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
    pub(in crate::app) fn regenerate_assistant_message_with_provider(
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
    pub(in crate::app) async fn start_message_generation_action_with_provider_async(
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
    pub(super) fn start_message_generation_action_with_provider_options_and_contract(
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
    pub(super) async fn start_message_generation_action_with_provider_options_and_contract_async(
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
