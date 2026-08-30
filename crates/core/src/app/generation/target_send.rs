use std::{sync::Arc, time::Duration};

use lorepia_domain::{
    ConversationBranchId, ConversationId, ConversationMode, CoreError, CoreErrorCode, CoreResult,
    GenerationId, GenerationTarget, MessageId, VariableMap,
};
use lorepia_providers::OpenAiCompatibleProvider;
use lorepia_storage::GenerationProviderTargetAuthority;
use tokio::sync::watch;

use super::{
    ConnectionBoundCredential, GenerationActionTargetIdentity, GenerationCredentialAdmissionLease,
    GenerationOperationContext, GenerationProviderTemporalContext,
    SameBranchGenerationAttemptIdentity, ValidatedGenerationTarget,
    build_resolved_generation_target, generation_target_provider_authority,
    provider_profile_temporal_context, require_generation_provider_target_authority,
    validate_connection_credential_binding, validate_generation_target_for_attempt,
    validate_generation_target_plan_with_reasoning_effort,
    validate_same_branch_attempt_semantic_identity,
};
use crate::app::{CORE_MAX_OUTPUT_TOKENS, Core, generation_attempt_prompt_authority};

pub(in crate::app) struct SameBranchGenerationTargetInput<'a> {
    pub(in crate::app) conversation_id: &'a ConversationId,
    pub(in crate::app) branch_id: &'a ConversationBranchId,
    pub(in crate::app) expected_head: Option<&'a MessageId>,
    pub(in crate::app) live_mode: ConversationMode,
    pub(in crate::app) text: &'a str,
    pub(in crate::app) operation_context: GenerationOperationContext<'a>,
    pub(in crate::app) target: &'a GenerationTarget,
    pub(in crate::app) prompt_preset_id: Option<&'a lorepia_domain::PromptPresetId>,
    pub(in crate::app) variable_overrides: &'a VariableMap,
}

pub(in crate::app) struct PreparedSameBranchGenerationTarget {
    pub(in crate::app) mode: ConversationMode,
    pub(in crate::app) validated: ValidatedGenerationTarget,
    pub(in crate::app) provider_target_authority: GenerationProviderTargetAuthority,
}

impl Core {
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

    pub(in crate::app) fn prepare_same_branch_generation_target(
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
}
