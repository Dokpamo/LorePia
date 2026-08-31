use lorepia_core::{ConversationBranchId, ConversationId, CoreError, GenerationTarget, MessageId};

use crate::{
    EditUserMessageInput, GenerationCredential, GenerationSelectionInput,
    RegenerateAssistantMessageInput, SendMessageInput, ShellError, ShellResult, StartedGeneration,
    StartedMessageAction, TaskCredentialReader, orchestration::ShellTaskCredentialBroker,
    sensitive::GenerationCredentialKind,
};

use super::{
    ShellApi, connection_bound_credential, validate_chat_route,
    validate_generation_operation_context, validate_identifier, validate_selection,
};

impl ShellApi {
    /// Starts a same-branch generation after resolving every prompt-time task
    /// dependency, including durable semantic embedding work.
    #[allow(
        clippy::too_many_lines,
        reason = "the complete credential and runtime-variable routing matrix stays visible"
    )]
    pub async fn send_message_to_branch_async(
        &self,
        input: SendMessageInput,
        credential: GenerationCredential,
        credential_reader: &dyn TaskCredentialReader,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<StartedGeneration> {
        validate_chat_route(
            &input.conversation_id,
            &input.branch_id,
            input.expected_head.as_deref(),
        )?;
        validate_selection(&input.selection)?;
        let operation_context = validate_generation_operation_context(
            input.operation_nonce.as_deref(),
            input.generation_attempt_id.as_deref(),
        )?;
        let receiver = self.core.subscribe_events();
        let expected_head = input.expected_head.map(MessageId);
        let conversation_id = ConversationId(input.conversation_id.clone());
        let branch_id = ConversationBranchId(input.branch_id.clone());
        let broker = ShellTaskCredentialBroker { credential_reader };
        let generation_id = match (input.selection, credential.into_kind()) {
            (
                GenerationSelectionInput::LegacyProfile {
                    provider_profile_id,
                },
                GenerationCredentialKind::Legacy {
                    credential,
                    admission_lease,
                },
            ) => {
                let credential = credential.map(crate::SecretCredential::into_core_value);
                if let Some(admission_lease) = admission_lease {
                    self.core
                        .send_message_to_branch_async_with_credential_admission_lease_and_variables(
                            &conversation_id,
                            &branch_id,
                            expected_head.as_ref(),
                            input.mode.into(),
                            &input.text,
                            operation_context.as_core(),
                            &input.variable_overrides,
                            &provider_profile_id,
                            credential,
                            admission_lease,
                            &broker,
                            cancelled,
                        )
                        .await
                } else {
                    self.core
                        .send_message_to_branch_async_with_variables(
                            &conversation_id,
                            &branch_id,
                            expected_head.as_ref(),
                            input.mode.into(),
                            &input.text,
                            operation_context.as_core(),
                            &input.variable_overrides,
                            &provider_profile_id,
                            credential,
                            &broker,
                            cancelled,
                        )
                        .await
                }
            }
            (
                GenerationSelectionInput::Target { target },
                GenerationCredentialKind::Connection {
                    connection_id,
                    credential,
                    access_authority,
                    dispatch_lease,
                },
            ) => {
                validate_identifier("connection_id", &connection_id)?;
                self.core
                    .send_message_to_branch_with_connection_credential_and_variables_async(
                        &conversation_id,
                        &branch_id,
                        expected_head.as_ref(),
                        input.mode.into(),
                        &input.text,
                        operation_context.as_core(),
                        &input.variable_overrides,
                        &GenerationTarget::from(target),
                        connection_bound_credential(
                            connection_id,
                            credential,
                            access_authority,
                            dispatch_lease,
                        ),
                        &broker,
                        cancelled,
                    )
                    .await
            }
            _ => Err(CoreError::invalid(
                "credential context does not match the generation selection",
            )),
        }
        .map_err(ShellError::from)?;
        Ok(StartedGeneration::new(
            generation_id,
            receiver,
            input.conversation_id,
            input.branch_id,
        ))
    }

    pub async fn edit_user_message_async(
        &self,
        input: EditUserMessageInput,
        credential: GenerationCredential,
        credential_reader: &dyn TaskCredentialReader,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<StartedMessageAction> {
        validate_chat_route(
            &input.conversation_id,
            &input.branch_id,
            input.expected_head.as_deref(),
        )?;
        validate_identifier("message_id", &input.message_id)?;
        validate_selection(&input.selection)?;
        let operation_context = validate_generation_operation_context(
            input.operation_nonce.as_deref(),
            input.generation_attempt_id.as_deref(),
        )?;
        let receiver = self.core.subscribe_events();
        let expected_head = input.expected_head.map(MessageId);
        let broker = ShellTaskCredentialBroker { credential_reader };
        let action = match (input.selection, credential.into_kind()) {
            (
                GenerationSelectionInput::LegacyProfile {
                    provider_profile_id,
                },
                GenerationCredentialKind::Legacy {
                    credential,
                    admission_lease,
                },
            ) => {
                let credential = credential.map(crate::SecretCredential::into_core_value);
                if let Some(admission_lease) = admission_lease {
                    self.core
                        .edit_user_message_async_with_credential_admission_lease(
                            &ConversationId(input.conversation_id.clone()),
                            &ConversationBranchId(input.branch_id),
                            expected_head.as_ref(),
                            &MessageId(input.message_id),
                            &input.replacement_text,
                            operation_context.as_core(),
                            &provider_profile_id,
                            credential,
                            admission_lease,
                            &broker,
                            cancelled,
                        )
                        .await
                } else {
                    self.core
                        .edit_user_message_async(
                            &ConversationId(input.conversation_id.clone()),
                            &ConversationBranchId(input.branch_id),
                            expected_head.as_ref(),
                            &MessageId(input.message_id),
                            &input.replacement_text,
                            operation_context.as_core(),
                            &provider_profile_id,
                            credential,
                            &broker,
                            cancelled,
                        )
                        .await
                }
            }
            (
                GenerationSelectionInput::Target { target },
                GenerationCredentialKind::Connection {
                    connection_id,
                    credential,
                    access_authority,
                    dispatch_lease,
                },
            ) => {
                validate_identifier("connection_id", &connection_id)?;
                self.core
                    .edit_user_message_with_connection_credential_async(
                        &ConversationId(input.conversation_id.clone()),
                        &ConversationBranchId(input.branch_id),
                        expected_head.as_ref(),
                        &MessageId(input.message_id),
                        &input.replacement_text,
                        operation_context.as_core(),
                        &GenerationTarget::from(target),
                        connection_bound_credential(
                            connection_id,
                            credential,
                            access_authority,
                            dispatch_lease,
                        ),
                        &broker,
                        cancelled,
                    )
                    .await
            }
            _ => Err(CoreError::invalid(
                "credential context does not match the generation selection",
            )),
        }
        .map_err(ShellError::from)?;
        Ok(StartedMessageAction::new(
            action,
            receiver,
            input.conversation_id,
        ))
    }

    pub async fn regenerate_assistant_message_async(
        &self,
        input: RegenerateAssistantMessageInput,
        credential: GenerationCredential,
        credential_reader: &dyn TaskCredentialReader,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<StartedMessageAction> {
        validate_chat_route(
            &input.conversation_id,
            &input.branch_id,
            input.expected_head.as_deref(),
        )?;
        validate_identifier("message_id", &input.message_id)?;
        validate_selection(&input.selection)?;
        let operation_context = validate_generation_operation_context(
            input.operation_nonce.as_deref(),
            input.generation_attempt_id.as_deref(),
        )?;
        let receiver = self.core.subscribe_events();
        let expected_head = input.expected_head.map(MessageId);
        let broker = ShellTaskCredentialBroker { credential_reader };
        let action = match (input.selection, credential.into_kind()) {
            (
                GenerationSelectionInput::LegacyProfile {
                    provider_profile_id,
                },
                GenerationCredentialKind::Legacy {
                    credential,
                    admission_lease,
                },
            ) => {
                let credential = credential.map(crate::SecretCredential::into_core_value);
                if let Some(admission_lease) = admission_lease {
                    self.core
                        .regenerate_assistant_message_async_with_credential_admission_lease(
                            &ConversationId(input.conversation_id.clone()),
                            &ConversationBranchId(input.branch_id),
                            expected_head.as_ref(),
                            &MessageId(input.message_id),
                            operation_context.as_core(),
                            &provider_profile_id,
                            credential,
                            admission_lease,
                            &broker,
                            cancelled,
                        )
                        .await
                } else {
                    self.core
                        .regenerate_assistant_message_async(
                            &ConversationId(input.conversation_id.clone()),
                            &ConversationBranchId(input.branch_id),
                            expected_head.as_ref(),
                            &MessageId(input.message_id),
                            operation_context.as_core(),
                            &provider_profile_id,
                            credential,
                            &broker,
                            cancelled,
                        )
                        .await
                }
            }
            (
                GenerationSelectionInput::Target { target },
                GenerationCredentialKind::Connection {
                    connection_id,
                    credential,
                    access_authority,
                    dispatch_lease,
                },
            ) => {
                validate_identifier("connection_id", &connection_id)?;
                self.core
                    .regenerate_assistant_message_with_connection_credential_async(
                        &ConversationId(input.conversation_id.clone()),
                        &ConversationBranchId(input.branch_id),
                        expected_head.as_ref(),
                        &MessageId(input.message_id),
                        operation_context.as_core(),
                        &GenerationTarget::from(target),
                        connection_bound_credential(
                            connection_id,
                            credential,
                            access_authority,
                            dispatch_lease,
                        ),
                        &broker,
                        cancelled,
                    )
                    .await
            }
            _ => Err(CoreError::invalid(
                "credential context does not match the generation selection",
            )),
        }
        .map_err(ShellError::from)?;
        Ok(StartedMessageAction::new(
            action,
            receiver,
            input.conversation_id,
        ))
    }
}
