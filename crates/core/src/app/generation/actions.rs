use std::{sync::Arc, time::Duration};

use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreResult, GenerationTarget, MessageActionGeneration,
    MessageId,
};
use lorepia_providers::OpenAiCompatibleProvider;
use lorepia_storage::MessageGenerationAction;
use tokio::sync::watch;

use super::{
    ConnectionBoundCredential, GenerationActionTargetIdentity, GenerationCredentialAdmissionLease,
    GenerationOperationContext, MessageGenerationActionIdentityInput,
    build_resolved_generation_target, preflight_generation_target_connection_credential,
    provider_profile_temporal_context, validate_connection_credential_binding,
};
use crate::app::{CORE_MAX_OUTPUT_TOKENS, Core};

impl Core {
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
}
