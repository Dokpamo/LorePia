use lorepia_core::{
    ConversationBranchId, ConversationId, CoreError, GenerationId, GenerationTarget, MessageId,
    RuntimeGenerationAuditContext, RuntimePromptMessage,
};

use crate::{
    ChatEventStream, EditUserMessageInput, GenerateRuntimeTextInput, GenerationCredential,
    GenerationSelectionInput, RegenerateAssistantMessageInput, RuntimeTextGenerationDto,
    SendMessageInput, ShellError, ShellResult, StartedGeneration, StartedMessageAction,
    sensitive::GenerationCredentialKind, stream::GenerationReattachment,
};

use super::{
    ShellApi, connection_bound_credential, validate_chat_route,
    validate_generation_operation_context, validate_identifier, validate_selection,
};

impl ShellApi {
    /// Executes a bounded provider-neutral prompt for an imported runtime.
    /// Native bindings still choose and read the credential; the webview can
    /// submit neither secret material nor a raw provider request.
    pub async fn generate_runtime_text(
        &self,
        input: GenerateRuntimeTextInput,
        credential: GenerationCredential,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> ShellResult<RuntimeTextGenerationDto> {
        validate_identifier("request_id", &input.request_id)?;
        let request_id = input.request_id.clone();
        let audit = RuntimeGenerationAuditContext {
            request_id: request_id.clone(),
            character_id: input.audit.character_id,
            character_content_revision_id: input.audit.character_content_revision_id,
            capability: input.audit.capability.into(),
            grant_sha256: input.audit.grant_sha256,
        };
        validate_selection(&input.selection)?;
        let messages = input
            .messages
            .into_iter()
            .map(|message| RuntimePromptMessage {
                role: message.role.into(),
                content: message.content,
            })
            .collect::<Vec<_>>();
        let result = match (input.selection, credential.into_kind()) {
            (
                GenerationSelectionInput::LegacyProfile {
                    provider_profile_id,
                },
                GenerationCredentialKind::Legacy {
                    credential,
                    admission_lease: _admission_lease,
                },
            ) => {
                validate_identifier("provider_profile_id", &provider_profile_id)?;
                self.core
                    .generate_runtime_text_with_provider_profile(
                        &provider_profile_id,
                        &messages,
                        credential.map(crate::SecretCredential::into_core_value),
                        cancelled.clone(),
                        audit,
                    )
                    .await
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
                    .generate_runtime_text_with_connection_credential(
                        &GenerationTarget::from(target),
                        &messages,
                        connection_bound_credential(
                            connection_id,
                            credential,
                            access_authority,
                            dispatch_lease,
                        ),
                        cancelled,
                        audit,
                    )
                    .await
            }
            _ => Err(CoreError::invalid(
                "credential context does not match the generation selection",
            )),
        }
        .map_err(ShellError::from)?;
        let (result, usage) = result;
        Ok(RuntimeTextGenerationDto {
            request_id,
            result,
            usage: usage.into(),
        })
    }

    pub fn send_message_to_branch(
        &self,
        input: SendMessageInput,
        credential: GenerationCredential,
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
        let generation_id = match (input.selection, credential.into_kind()) {
            (
                GenerationSelectionInput::LegacyProfile {
                    provider_profile_id,
                },
                GenerationCredentialKind::Legacy {
                    credential,
                    admission_lease: _admission_lease,
                },
            ) => self.core.send_message_to_branch_with_variables(
                &ConversationId(input.conversation_id.clone()),
                &ConversationBranchId(input.branch_id.clone()),
                expected_head.as_ref(),
                input.mode.into(),
                &input.text,
                operation_context.as_core(),
                &input.variable_overrides,
                &provider_profile_id,
                credential.map(crate::SecretCredential::into_core_value),
            ),
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
                let target = GenerationTarget::from(target);
                self.core
                    .send_message_to_branch_with_connection_credential_and_variables(
                        &ConversationId(input.conversation_id.clone()),
                        &ConversationBranchId(input.branch_id.clone()),
                        expected_head.as_ref(),
                        input.mode.into(),
                        &input.text,
                        operation_context.as_core(),
                        &input.variable_overrides,
                        &target,
                        connection_bound_credential(
                            connection_id,
                            credential,
                            access_authority,
                            dispatch_lease,
                        ),
                    )
            }
            _ => {
                return Err(ShellError::from(CoreError::invalid(
                    "credential context does not match the generation selection",
                )));
            }
        }
        .map_err(ShellError::from)?;
        Ok(StartedGeneration::new(
            generation_id,
            receiver,
            input.conversation_id,
            input.branch_id,
        ))
    }

    pub fn edit_user_message(
        &self,
        input: EditUserMessageInput,
        credential: GenerationCredential,
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
        let action = match (input.selection, credential.into_kind()) {
            (
                GenerationSelectionInput::LegacyProfile {
                    provider_profile_id,
                },
                GenerationCredentialKind::Legacy {
                    credential,
                    admission_lease: _admission_lease,
                },
            ) => self.core.edit_user_message(
                &ConversationId(input.conversation_id.clone()),
                &ConversationBranchId(input.branch_id),
                expected_head.as_ref(),
                &MessageId(input.message_id),
                &input.replacement_text,
                operation_context.as_core(),
                &provider_profile_id,
                credential.map(crate::SecretCredential::into_core_value),
            ),
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
                let target = GenerationTarget::from(target);
                self.core.edit_user_message_with_connection_credential(
                    &ConversationId(input.conversation_id.clone()),
                    &ConversationBranchId(input.branch_id),
                    expected_head.as_ref(),
                    &MessageId(input.message_id),
                    &input.replacement_text,
                    operation_context.as_core(),
                    &target,
                    connection_bound_credential(
                        connection_id,
                        credential,
                        access_authority,
                        dispatch_lease,
                    ),
                )
            }
            _ => {
                return Err(ShellError::from(CoreError::invalid(
                    "credential context does not match the generation selection",
                )));
            }
        }
        .map_err(ShellError::from)?;
        Ok(StartedMessageAction::new(
            action,
            receiver,
            input.conversation_id,
        ))
    }

    pub fn regenerate_assistant_message(
        &self,
        input: RegenerateAssistantMessageInput,
        credential: GenerationCredential,
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
        let action = match (input.selection, credential.into_kind()) {
            (
                GenerationSelectionInput::LegacyProfile {
                    provider_profile_id,
                },
                GenerationCredentialKind::Legacy {
                    credential,
                    admission_lease: _admission_lease,
                },
            ) => self.core.regenerate_assistant_message(
                &ConversationId(input.conversation_id.clone()),
                &ConversationBranchId(input.branch_id),
                expected_head.as_ref(),
                &MessageId(input.message_id),
                operation_context.as_core(),
                &provider_profile_id,
                credential.map(crate::SecretCredential::into_core_value),
            ),
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
                let target = GenerationTarget::from(target);
                self.core
                    .regenerate_assistant_message_with_connection_credential(
                        &ConversationId(input.conversation_id.clone()),
                        &ConversationBranchId(input.branch_id),
                        expected_head.as_ref(),
                        &MessageId(input.message_id),
                        operation_context.as_core(),
                        &target,
                        connection_bound_credential(
                            connection_id,
                            credential,
                            access_authority,
                            dispatch_lease,
                        ),
                    )
            }
            _ => {
                return Err(ShellError::from(CoreError::invalid(
                    "credential context does not match the generation selection",
                )));
            }
        }
        .map_err(ShellError::from)?;
        Ok(StartedMessageAction::new(
            action,
            receiver,
            input.conversation_id,
        ))
    }

    pub fn cancel_generation(&self, generation_id: &str) -> ShellResult<()> {
        validate_identifier("generation_id", generation_id)?;
        self.core
            .cancel_generation(&GenerationId(generation_id.to_owned()))
            .map_err(ShellError::from)
    }

    pub fn subscribe_generation(
        &self,
        generation_id: &str,
        conversation_id: &str,
        branch_id: &str,
        sequence_baseline: u64,
    ) -> ShellResult<ChatEventStream> {
        validate_identifier("generation_id", generation_id)?;
        validate_identifier("conversation_id", conversation_id)?;
        validate_identifier("branch_id", branch_id)?;
        let subscription = self
            .core
            .subscribe_generation_events(
                &GenerationId(generation_id.to_owned()),
                &ConversationId(conversation_id.to_owned()),
                &ConversationBranchId(branch_id.to_owned()),
            )
            .map_err(ShellError::from)?;
        let (
            receiver,
            assistant_message_id,
            authoritative_watermark,
            display_prefix,
            reasoning_prefix,
        ) = subscription.into_parts();
        if sequence_baseline > authoritative_watermark {
            return Err(ShellError::from(CoreError::invalid(
                "generation sequence baseline exceeds the live event watermark",
            )));
        }
        Ok(ChatEventStream::reattached(
            receiver,
            generation_id.to_owned(),
            conversation_id.to_owned(),
            branch_id.to_owned(),
            GenerationReattachment {
                assistant_message_id: assistant_message_id.0,
                sequence_baseline,
                authoritative_watermark,
                display_prefix,
                reasoning_prefix,
            },
        ))
    }
}
