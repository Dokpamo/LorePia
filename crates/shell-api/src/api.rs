use std::{fmt, path::Path};

use lorepia_core::{
    CORE_API_VERSION, ConnectionBoundCredential, ConversationBranchId, ConversationId,
    ConversationMode, Core, CoreConfig, CoreError, DiscoveryRecoveryOwner, GenerationId,
    GenerationOperationContext, GenerationTarget, InspectionId, MessageId, MessageRole,
    ProviderConnectionId, ProviderCredentialAccessAuthority, RuntimeGenerationAuditContext,
    RuntimeGenerationCapability, RuntimePromptMessage, VariableMap,
};
use serde::{Deserialize, Serialize};

use crate::{
    BootstrapDto, CharacterDto, CharacterGreetingCatalogDto, CharacterRenderProfileDto,
    ChatEventStream, ConversationBranchDto, ConversationDto, ConversationModeDto,
    ConversationStateDto, GenerationCredential, GenerationStartedDto, GenerationTargetDto,
    GenerationUsageDto, HealthDto, ImportInspectionDto, MessageActionGenerationDto, MessageDto,
    ProviderCredentialAccessAuthorityContext, SecretCredential, ShellError, ShellResult,
    StagedImportFile, TaskCredentialLease, TaskCredentialReader,
    orchestration::ShellTaskCredentialBroker, sensitive::GenerationCredentialKind,
    stream::GenerationReattachment,
};

const MAX_IPC_IDENTIFIER_BYTES: usize = 512;
const MAX_IPC_IDENTIFIER_CHARS: usize = 256;
const MAX_OPERATION_NONCE_BYTES: usize = 128;
const MAX_OPERATION_NONCE_CHARS: usize = 64;

fn connection_bound_credential(
    connection_id: String,
    credential: Option<SecretCredential>,
    access_authority: Option<ProviderCredentialAccessAuthorityContext>,
    dispatch_lease: Option<TaskCredentialLease>,
) -> ConnectionBoundCredential {
    let connection_id = ProviderConnectionId::from(connection_id);
    let credential = credential.map(SecretCredential::into_core_value);
    let credential = match access_authority {
        Some(authority) => ConnectionBoundCredential::new_with_access_authority(
            connection_id,
            credential,
            ProviderCredentialAccessAuthority {
                authority_id: authority.authority_id,
                connection_binding_sha256: authority.connection_binding_sha256,
            },
        ),
        None => ConnectionBoundCredential::new(connection_id, credential),
    };
    match dispatch_lease {
        Some(lease) => credential.with_dispatch_lease(lease.into_inner()),
        None => credential,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GenerationSelectionInput {
    LegacyProfile { provider_profile_id: String },
    Target { target: GenerationTargetDto },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimePromptRoleInput {
    System,
    User,
    Assistant,
}

impl From<RuntimePromptRoleInput> for MessageRole {
    fn from(value: RuntimePromptRoleInput) -> Self {
        match value {
            RuntimePromptRoleInput::System => Self::System,
            RuntimePromptRoleInput::User => Self::User,
            RuntimePromptRoleInput::Assistant => Self::Assistant,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePromptMessageInput {
    pub role: RuntimePromptRoleInput,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeGenerationCapabilityInput {
    #[serde(rename = "model:primary")]
    Primary,
    #[serde(rename = "model:auxiliary")]
    Auxiliary,
}

impl From<RuntimeGenerationCapabilityInput> for RuntimeGenerationCapability {
    fn from(value: RuntimeGenerationCapabilityInput) -> Self {
        match value {
            RuntimeGenerationCapabilityInput::Primary => Self::Primary,
            RuntimeGenerationCapabilityInput::Auxiliary => Self::Auxiliary,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeGenerationAuditInput {
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub capability: RuntimeGenerationCapabilityInput,
    pub grant_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateRuntimeTextInput {
    pub request_id: String,
    pub audit: RuntimeGenerationAuditInput,
    pub selection: GenerationSelectionInput,
    pub messages: Vec<RuntimePromptMessageInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTextGenerationDto {
    pub request_id: String,
    pub result: String,
    pub usage: GenerationUsageDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendMessageInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub expected_head: Option<String>,
    pub mode: ConversationModeDto,
    pub text: String,
    pub selection: GenerationSelectionInput,
    /// Per-generation character/runtime values merged after stored prompt state.
    #[serde(default)]
    pub variable_overrides: VariableMap,
    /// Caller-owned idempotency identity. Missing fields still deserialize so
    /// older clients receive a bounded validation error instead of a schema
    /// decoding failure.
    #[serde(default)]
    pub operation_nonce: Option<String>,
    /// Exact durable attempt to resume. This and `operation_nonce` are XOR.
    #[serde(default)]
    pub generation_attempt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditUserMessageInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub expected_head: Option<String>,
    pub message_id: String,
    pub replacement_text: String,
    pub selection: GenerationSelectionInput,
    /// Caller-owned identity for a new edit operation.
    #[serde(default)]
    pub operation_nonce: Option<String>,
    /// Exact durable edit attempt to resume; mutually exclusive with nonce.
    #[serde(default)]
    pub generation_attempt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegenerateAssistantMessageInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub expected_head: Option<String>,
    pub message_id: String,
    pub selection: GenerationSelectionInput,
    /// Caller-owned identity for a new regenerate operation.
    #[serde(default)]
    pub operation_nonce: Option<String>,
    /// Exact durable regenerate attempt to resume; mutually exclusive with nonce.
    #[serde(default)]
    pub generation_attempt_id: Option<String>,
}

pub(crate) enum ValidatedGenerationOperationContext {
    New(String),
    Resume(GenerationId),
}

impl ValidatedGenerationOperationContext {
    pub(crate) fn as_core(&self) -> GenerationOperationContext<'_> {
        match self {
            Self::New(operation_nonce) => GenerationOperationContext::New { operation_nonce },
            Self::Resume(generation_attempt_id) => GenerationOperationContext::Resume {
                generation_attempt_id,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveMessageInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub expected_head: Option<String>,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConversationInput {
    pub character_id: String,
    pub title: String,
    pub mode: ConversationModeDto,
    /// Present only when the caller is bound to a greeting catalog snapshot.
    /// A nested object distinguishes an exact legacy `null` revision from an
    /// older caller that did not participate in greeting selection.
    #[serde(default)]
    pub greeting: Option<ConversationGreetingSelectionInput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationGreetingSelectionInput {
    pub character_content_revision_id: Option<String>,
    pub greeting_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateConversationBranchInput {
    pub conversation_id: String,
    pub from_message_id: Option<String>,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectConversationBranchInput {
    pub conversation_id: String,
    pub branch_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetConversationModeInput {
    pub conversation_id: String,
    pub mode: ConversationModeDto,
}

#[derive(Clone)]
pub struct ShellApi {
    pub(crate) core: Core,
}

impl ShellApi {
    pub fn open_data_root(data_root: impl AsRef<Path>) -> ShellResult<Self> {
        Self::open(CoreConfig::new(data_root.as_ref()))
    }

    pub fn open(config: CoreConfig) -> ShellResult<Self> {
        Core::open(config)
            .map(Self::from_core)
            .map_err(ShellError::from)
    }

    /// Opens a shell whose provider-discovery recovery is owned by the native
    /// platform host.
    ///
    /// The returned shell must remain private until the host reconciles its
    /// credential vault and invokes Core's normal discovery recovery.
    pub fn open_for_native_discovery_recovery(config: CoreConfig) -> ShellResult<Self> {
        Core::open_with_discovery_recovery_owner(config, DiscoveryRecoveryOwner::NativePlatform)
            .map(Self::from_core)
            .map_err(ShellError::from)
    }

    pub fn open_data_root_for_native_discovery_recovery(
        data_root: impl AsRef<Path>,
    ) -> ShellResult<Self> {
        Self::open_for_native_discovery_recovery(CoreConfig::new(data_root.as_ref()))
    }

    pub const fn from_core(core: Core) -> Self {
        Self { core }
    }

    pub fn bootstrap(&self) -> ShellResult<BootstrapDto> {
        let health = self.core.health_check().map_err(ShellError::from)?;
        Ok(BootstrapDto {
            shell_api_version: crate::SHELL_API_VERSION,
            core_api_version: CORE_API_VERSION,
            chat_event_version: lorepia_core::CHAT_EVENT_VERSION,
            health: HealthDto::from(health),
        })
    }

    /// Processes a bounded batch of durable Core lifecycle occurrences.
    ///
    /// This is intentionally a Rust-host surface rather than an IPC command:
    /// the native host owns continuous delivery, while the webview cannot
    /// claim or acknowledge lifecycle work directly.
    pub fn drain_core_lifecycle_occurrences(&self, max_occurrences: u32) -> ShellResult<bool> {
        self.core
            .drain_core_lifecycle_occurrences(max_occurrences)
            .map(|receipt| !receipt.deliveries.is_empty())
            .map_err(ShellError::from)
    }

    /// Recovers expired in-process lifecycle leases before continuous drain.
    pub fn recover_expired_core_lifecycle_occurrence_leases(&self) -> ShellResult<u64> {
        self.core
            .recover_expired_core_lifecycle_occurrence_leases()
            .map_err(ShellError::from)
    }

    pub fn list_characters(&self) -> ShellResult<Vec<CharacterDto>> {
        self.core
            .list_characters()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn get_character(&self, character_id: &str) -> ShellResult<CharacterDto> {
        validate_identifier("character_id", character_id)?;
        self.core
            .get_character(character_id)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn get_character_render_profile(
        &self,
        character_id: &str,
    ) -> ShellResult<CharacterRenderProfileDto> {
        validate_identifier("character_id", character_id)?;
        self.core
            .get_character_content(character_id)
            .map(|stored| {
                CharacterRenderProfileDto::from_content(
                    character_id.to_owned(),
                    stored.revision_id,
                    stored.value,
                )
            })
            .map_err(ShellError::from)
    }

    pub fn get_character_greeting_catalog(
        &self,
        character_id: &str,
    ) -> ShellResult<CharacterGreetingCatalogDto> {
        validate_identifier("character_id", character_id)?;
        self.core
            .get_character_greeting_catalog(character_id)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn inspect_import(
        &self,
        staged_file: &StagedImportFile,
    ) -> ShellResult<ImportInspectionDto> {
        self.core
            .inspect_import(staged_file.as_path())
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn commit_import(&self, inspection_id: &str) -> ShellResult<CharacterDto> {
        validate_identifier("inspection_id", inspection_id)?;
        self.core
            .commit_import(&InspectionId(inspection_id.to_owned()))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn discard_import(&self, inspection_id: &str) -> ShellResult<()> {
        validate_identifier("inspection_id", inspection_id)?;
        self.core
            .discard_import(&InspectionId(inspection_id.to_owned()))
            .map_err(ShellError::from)
    }

    pub fn open_conversation(&self, character_id: &str) -> ShellResult<ConversationDto> {
        validate_identifier("character_id", character_id)?;
        self.core
            .open_conversation(character_id)
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn create_conversation(
        &self,
        input: CreateConversationInput,
    ) -> ShellResult<ConversationDto> {
        validate_identifier("character_id", &input.character_id)?;
        if let Some(greeting) = input.greeting {
            validate_optional_identifier(
                "character_content_revision_id",
                greeting.character_content_revision_id.as_deref(),
            )?;
            validate_optional_identifier("greeting_id", greeting.greeting_id.as_deref())?;
            self.core
                .create_conversation_with_greeting(
                    &input.character_id,
                    input.title,
                    ConversationMode::from(input.mode),
                    greeting.character_content_revision_id.as_deref(),
                    greeting.greeting_id.as_deref(),
                )
                .map(|started| started.conversation.into())
                .map_err(ShellError::from)
        } else {
            self.core
                .create_conversation(
                    &input.character_id,
                    input.title,
                    ConversationMode::from(input.mode),
                )
                .map(Into::into)
                .map_err(ShellError::from)
        }
    }

    pub fn list_conversations(&self) -> ShellResult<Vec<ConversationDto>> {
        self.core
            .list_conversations()
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_conversations_for_character(
        &self,
        character_id: &str,
    ) -> ShellResult<Vec<ConversationDto>> {
        validate_identifier("character_id", character_id)?;
        self.core
            .list_conversations_for_character(character_id)
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn get_conversation(&self, conversation_id: &str) -> ShellResult<ConversationDto> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .get_conversation(&ConversationId(conversation_id.to_owned()))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn open_existing_conversation(
        &self,
        conversation_id: &str,
    ) -> ShellResult<ConversationDto> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .open_existing_conversation(&ConversationId(conversation_id.to_owned()))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn get_conversation_state(
        &self,
        conversation_id: &str,
    ) -> ShellResult<ConversationStateDto> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .get_conversation_state(&ConversationId(conversation_id.to_owned()))
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn list_conversation_branches(
        &self,
        conversation_id: &str,
    ) -> ShellResult<Vec<ConversationBranchDto>> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .list_conversation_branches(&ConversationId(conversation_id.to_owned()))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn create_conversation_branch(
        &self,
        input: CreateConversationBranchInput,
    ) -> ShellResult<ConversationBranchDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_optional_identifier("from_message_id", input.from_message_id.as_deref())?;
        let from_message_id = input.from_message_id.map(MessageId);
        self.core
            .create_conversation_branch(
                &ConversationId(input.conversation_id),
                from_message_id.as_ref(),
                input.title,
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn select_conversation_branch(
        &self,
        input: SelectConversationBranchInput,
    ) -> ShellResult<ConversationStateDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        self.core
            .select_conversation_branch(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.branch_id),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn set_conversation_mode(
        &self,
        input: SetConversationModeInput,
    ) -> ShellResult<ConversationStateDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        self.core
            .set_conversation_mode(
                &ConversationId(input.conversation_id),
                ConversationMode::from(input.mode),
            )
            .map(Into::into)
            .map_err(ShellError::from)
    }

    pub fn list_branch_messages(&self, branch_id: &str) -> ShellResult<Vec<MessageDto>> {
        validate_identifier("branch_id", branch_id)?;
        self.core
            .list_branch_message_presentations(&ConversationBranchId(branch_id.to_owned()))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

    pub fn list_messages(&self, conversation_id: &str) -> ShellResult<Vec<MessageDto>> {
        validate_identifier("conversation_id", conversation_id)?;
        self.core
            .list_message_presentations(&ConversationId(conversation_id.to_owned()))
            .map(|values| values.into_iter().map(Into::into).collect())
            .map_err(ShellError::from)
    }

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

    pub fn remove_message_from_branch(
        &self,
        input: RemoveMessageInput,
    ) -> ShellResult<ConversationBranchDto> {
        validate_chat_route(
            &input.conversation_id,
            &input.branch_id,
            input.expected_head.as_deref(),
        )?;
        validate_identifier("message_id", &input.message_id)?;
        let expected_head = input.expected_head.map(MessageId);
        self.core
            .remove_message_from_branch(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.branch_id),
                expected_head.as_ref(),
                &MessageId(input.message_id),
            )
            .map(Into::into)
            .map_err(ShellError::from)
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

impl fmt::Debug for ShellApi {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ShellApi { core: [OPAQUE] }")
    }
}

pub struct StartedGeneration {
    response: GenerationStartedDto,
    stream: ChatEventStream,
}

impl StartedGeneration {
    pub(crate) fn new(
        generation_id: GenerationId,
        receiver: tokio::sync::broadcast::Receiver<lorepia_core::ChatEvent>,
        conversation_id: String,
        branch_id: String,
    ) -> Self {
        let generation_id = generation_id.0;
        Self {
            response: GenerationStartedDto {
                generation_id: generation_id.clone(),
            },
            stream: ChatEventStream::new(receiver, generation_id, conversation_id, branch_id),
        }
    }

    pub fn response(&self) -> &GenerationStartedDto {
        &self.response
    }

    pub fn into_parts(self) -> (GenerationStartedDto, ChatEventStream) {
        (self.response, self.stream)
    }
}

impl fmt::Debug for StartedGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartedGeneration")
            .field("response", &self.response)
            .field("stream", &self.stream)
            .finish()
    }
}

pub struct StartedMessageAction {
    response: MessageActionGenerationDto,
    stream: ChatEventStream,
}

impl StartedMessageAction {
    fn new(
        action: lorepia_core::MessageActionGeneration,
        receiver: tokio::sync::broadcast::Receiver<lorepia_core::ChatEvent>,
        conversation_id: String,
    ) -> Self {
        let generation_id = action.generation_id.0.clone();
        let branch_id = action.branch.id.0.clone();
        Self {
            response: action.into(),
            stream: ChatEventStream::new(receiver, generation_id, conversation_id, branch_id),
        }
    }

    pub fn response(&self) -> &MessageActionGenerationDto {
        &self.response
    }

    pub fn into_parts(self) -> (MessageActionGenerationDto, ChatEventStream) {
        (self.response, self.stream)
    }
}

impl fmt::Debug for StartedMessageAction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartedMessageAction")
            .field("response", &self.response)
            .field("stream", &self.stream)
            .finish()
    }
}

fn validate_chat_route(
    conversation_id: &str,
    branch_id: &str,
    expected_head: Option<&str>,
) -> ShellResult<()> {
    validate_identifier("conversation_id", conversation_id)?;
    validate_identifier("branch_id", branch_id)?;
    validate_optional_identifier("expected_head", expected_head)
}

fn validate_selection(selection: &GenerationSelectionInput) -> ShellResult<()> {
    match selection {
        GenerationSelectionInput::LegacyProfile {
            provider_profile_id,
        } => validate_identifier("provider_profile_id", provider_profile_id),
        GenerationSelectionInput::Target { target } => {
            validate_identifier("model_route_id", &target.model_route_id)?;
            validate_identifier("generation_preset_id", &target.generation_preset_id)
        }
    }
}

fn validate_optional_identifier(field: &str, value: Option<&str>) -> ShellResult<()> {
    value.map_or(Ok(()), |value| validate_identifier(field, value))
}

pub(crate) fn validate_identifier(field: &str, value: &str) -> ShellResult<()> {
    if value.is_empty()
        || value.len() > MAX_IPC_IDENTIFIER_BYTES
        || value.chars().count() > MAX_IPC_IDENTIFIER_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(ShellError::from(CoreError::invalid(format!(
            "{field} is not a bounded identifier"
        ))));
    }
    Ok(())
}

pub(crate) fn validate_required_operation_nonce(value: Option<&str>) -> ShellResult<&str> {
    let Some(value) = value else {
        return Err(ShellError::from(CoreError::invalid(
            "operation_nonce is required",
        )));
    };
    if value.is_empty()
        || value.trim() != value
        || value.len() > MAX_OPERATION_NONCE_BYTES
        || value.chars().count() > MAX_OPERATION_NONCE_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(ShellError::from(CoreError::invalid(
            "operation_nonce must be printable, trimmed, non-empty, and at most 128 UTF-8 bytes or 64 characters",
        )));
    }
    Ok(value)
}

pub(crate) fn validate_generation_operation_context(
    operation_nonce: Option<&str>,
    generation_attempt_id: Option<&str>,
) -> ShellResult<ValidatedGenerationOperationContext> {
    match (operation_nonce, generation_attempt_id) {
        (Some(operation_nonce), None) => validate_required_operation_nonce(Some(operation_nonce))
            .map(|value| ValidatedGenerationOperationContext::New(value.to_owned())),
        (None, Some(generation_attempt_id)) => {
            validate_identifier("generation_attempt_id", generation_attempt_id)?;
            Ok(ValidatedGenerationOperationContext::Resume(GenerationId(
                generation_attempt_id.to_owned(),
            )))
        }
        (None, None) => Err(ShellError::from(CoreError::invalid(
            "exactly one of operation_nonce or generation_attempt_id is required",
        ))),
        (Some(_), Some(_)) => Err(ShellError::from(CoreError::invalid(
            "operation_nonce and generation_attempt_id are mutually exclusive",
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use lorepia_core::{
        Core, CoreConfig, ProviderProfile, VariableId, VariableMap, VariableRef, VariableScope,
        VariableValue,
    };
    use tempfile::{NamedTempFile, tempdir};

    use crate::{
        ChatStreamItem, ConversationGreetingSelectionInput, CreateConversationInput,
        EditUserMessageInput, GenerationCredential, GenerationSelectionInput, GenerationTargetDto,
        RegenerateAssistantMessageInput, RemoveMessageInput, SHELL_API_VERSION, SecretCredential,
        SendMessageInput, ShellApi, ShellErrorCode, StagedImportFile, dto::ConversationModeDto,
    };

    const LIVE_CREDENTIAL_CANARY: &str = "sk-shell-live-canary";

    struct MissingTaskCredentialReader;

    struct AdmissionDropCounter(Arc<AtomicUsize>);

    impl Drop for AdmissionDropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl crate::TaskCredentialReader for MissingTaskCredentialReader {
        fn credential_for<'a>(
            &'a self,
            _connection_id: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::TaskCredentialRead> + Send + 'a>,
        > {
            Box::pin(async { crate::TaskCredentialRead::Missing })
        }
    }

    #[test]
    fn operation_nonce_is_serde_compatible_but_strictly_bounded_before_use() {
        let legacy = serde_json::from_value::<SendMessageInput>(serde_json::json!({
            "conversation_id": "conversation-1",
            "branch_id": "branch-1",
            "expected_head": null,
            "mode": "chat",
            "text": "hello",
            "selection": {
                "kind": "legacy_profile",
                "provider_profile_id": "profile-1"
            }
        }))
        .expect("older send payloads must still decode");
        assert_eq!(legacy.operation_nonce, None);
        assert_eq!(legacy.generation_attempt_id, None);
        assert_eq!(legacy.variable_overrides, VariableMap::default());

        let valid_ascii = "n".repeat(64);
        let valid_unicode = "가".repeat(42);
        assert!(super::validate_required_operation_nonce(Some(&valid_ascii)).is_ok());
        assert!(super::validate_required_operation_nonce(Some(&valid_unicode)).is_ok());
        assert!(super::validate_required_operation_nonce(None).is_err());
        assert!(super::validate_required_operation_nonce(Some("   ")).is_err());
        assert!(super::validate_required_operation_nonce(Some(" padded")).is_err());
        assert!(super::validate_required_operation_nonce(Some("line\nbreak")).is_err());
        assert!(super::validate_required_operation_nonce(Some(&"n".repeat(65))).is_err());
        assert!(super::validate_required_operation_nonce(Some(&"가".repeat(43))).is_err());
        assert!(
            super::validate_generation_operation_context(
                Some("caller-owned-nonce"),
                Some("generation-attempt-1")
            )
            .is_err()
        );
        assert!(super::validate_generation_operation_context(None, None).is_err());
        assert!(
            super::validate_generation_operation_context(None, Some("generation-attempt-1"))
                .is_ok()
        );

        let resumed = serde_json::to_value(SendMessageInput {
            generation_attempt_id: Some("generation-attempt-1".to_owned()),
            ..legacy.clone()
        })
        .expect("resume-bearing send input must serialize");
        assert_eq!(resumed["generation_attempt_id"], "generation-attempt-1");

        let encoded = serde_json::to_value(SendMessageInput {
            operation_nonce: Some("caller-owned-nonce".to_owned()),
            ..legacy
        })
        .expect("nonce-bearing send input must serialize");
        assert_eq!(encoded["operation_nonce"], "caller-owned-nonce");
    }

    fn imported_shell() -> (tempfile::TempDir, ShellApi, String) {
        let root = tempdir().expect("temporary data root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open shell");
        let mut source = NamedTempFile::new().expect("temporary source");
        write!(
            source,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Shell","description":"Synthetic"}}}}"#
        )
        .expect("write source");
        let inspection = shell
            .inspect_import(&StagedImportFile::new(source.path()))
            .expect("inspect");
        let character = shell
            .commit_import(&inspection.inspection_id)
            .expect("commit");
        (root, shell, character.id)
    }

    #[test]
    fn bootstrap_and_library_projection_never_expose_data_root() {
        let (root, shell, character_id) = imported_shell();
        let bootstrap = shell.bootstrap().expect("bootstrap");
        assert_eq!(bootstrap.shell_api_version, SHELL_API_VERSION);
        assert_eq!(bootstrap.core_api_version, lorepia_core::CORE_API_VERSION);
        let characters = shell.list_characters().expect("characters");
        let json = serde_json::to_string(&(bootstrap, characters)).expect("serialize");

        assert!(!json.contains(root.path().to_string_lossy().as_ref()));
        assert!(json.contains(&character_id));
    }

    #[test]
    fn conversation_collections_remain_whole_vec_mappings() {
        let (_root, shell, character_id) = imported_shell();
        let first = shell
            .create_conversation(CreateConversationInput {
                character_id: character_id.clone(),
                title: "First".into(),
                mode: ConversationModeDto::Chat,
                greeting: None,
            })
            .expect("first conversation");
        shell
            .create_conversation(CreateConversationInput {
                character_id: character_id.clone(),
                title: "Second".into(),
                mode: ConversationModeDto::Story,
                greeting: None,
            })
            .expect("second conversation");

        let all = shell.list_conversations().expect("all conversations");
        let filtered = shell
            .list_conversations_for_character(&character_id)
            .expect("filtered conversations");
        assert_eq!(all.len(), 2);
        assert_eq!(filtered.len(), 2);
        assert!(shell.list_messages(&first.id).expect("messages").is_empty());
    }

    #[test]
    fn greeting_catalog_is_text_free_and_exact_alternate_selection_starts_the_room() {
        let root = tempdir().expect("temporary data root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open shell");
        let default_canary = "SHELL PRIVATE DEFAULT GREETING";
        let alternate_canary = "SHELL PRIVATE ALTERNATE GREETING";
        let mut source = NamedTempFile::new().expect("temporary source");
        write!(
            source,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Greeting Shell","description":"Synthetic","first_mes":"{default_canary}","alternate_greetings":["{alternate_canary}"]}}}}"#
        )
        .expect("write source");
        let inspection = shell
            .inspect_import(&StagedImportFile::new(source.path()))
            .expect("inspect");
        let character = shell
            .commit_import(&inspection.inspection_id)
            .expect("commit");

        let catalog = shell
            .get_character_greeting_catalog(&character.id)
            .expect("safe greeting catalog");
        let revision_id = catalog
            .character_content_revision_id
            .clone()
            .expect("content revision");
        let json = serde_json::to_string(&catalog).expect("serialize catalog");
        assert!(!json.contains(default_canary));
        assert!(!json.contains(alternate_canary));
        assert_eq!(
            catalog
                .greetings
                .iter()
                .map(|greeting| greeting.id.as_str())
                .collect::<Vec<_>>(),
            ["default", "alternate-0"]
        );

        let conversation = shell
            .create_conversation(CreateConversationInput {
                character_id: character.id,
                title: "Alternate greeting".into(),
                mode: ConversationModeDto::Chat,
                greeting: Some(ConversationGreetingSelectionInput {
                    character_content_revision_id: Some(revision_id),
                    greeting_id: Some("alternate-0".into()),
                }),
            })
            .expect("exact alternate conversation start");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("conversation state");
        let messages = shell
            .list_branch_messages(&state.active_branch_id)
            .expect("greeting lineage");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, alternate_canary);
    }

    #[test]
    fn remove_message_passes_expected_head_instead_of_inventing_a_revision() {
        let (_root, shell, character_id) = imported_shell();
        let conversation = shell
            .open_conversation(&character_id)
            .expect("conversation");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("state");

        let error = shell
            .remove_message_from_branch(RemoveMessageInput {
                conversation_id: conversation.id,
                branch_id: state.active_branch_id,
                expected_head: Some("stale-head".into()),
                message_id: "missing-message".into(),
            })
            .expect_err("stale head must fail");

        assert_eq!(error.code, ShellErrorCode::InvalidInput);
    }

    #[test]
    fn target_selection_rejects_unbound_legacy_credential_context() {
        let (_root, shell, character_id) = imported_shell();
        let conversation = shell
            .open_conversation(&character_id)
            .expect("conversation");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("conversation state");

        let error = shell
            .send_message_to_branch(
                SendMessageInput {
                    conversation_id: conversation.id,
                    branch_id: state.active_branch_id.clone(),
                    expected_head: None,
                    mode: ConversationModeDto::Chat,
                    text: "must not be stored".to_owned(),
                    selection: GenerationSelectionInput::Target {
                        target: GenerationTargetDto {
                            model_route_id: "synthetic-route".to_owned(),
                            generation_preset_id: "synthetic-preset".to_owned(),
                        },
                    },
                    variable_overrides: VariableMap::default(),
                    operation_nonce: Some("shell-target-unbound-credential".to_owned()),
                    generation_attempt_id: None,
                },
                GenerationCredential::legacy(Some(SecretCredential::new(
                    "synthetic-unbound-credential",
                ))),
            )
            .expect_err("target selection must reject an unbound credential");

        assert_eq!(error.code, ShellErrorCode::InvalidInput);
        assert!(
            shell
                .list_branch_messages(&state.active_branch_id)
                .expect("messages after rejected credential context")
                .is_empty()
        );
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one vertical-slice test keeps the complete immutable-branch action sequence visible"
    )]
    async fn synthetic_async_core_stream_exercises_send_edit_regenerate_and_remove() {
        let root = tempdir().expect("temporary data root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let (base_url, provider_thread) = spawn_completed_provider(3);
        core.upsert_provider_profile(ProviderProfile {
            id: "synthetic-profile".into(),
            display_name: "Synthetic".into(),
            base_url,
            model: "synthetic-model".into(),
            timeout_seconds: 5,
        })
        .expect("save provider");
        let shell = ShellApi::from_core(core);
        let mut source = NamedTempFile::new().expect("temporary source");
        write!(
            source,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Chat","description":"Synthetic"}}}}"#
        )
        .expect("write source");
        let inspection = shell
            .inspect_import(&StagedImportFile::new(source.path()))
            .expect("inspect");
        let character = shell
            .commit_import(&inspection.inspection_id)
            .expect("commit");
        let conversation = shell
            .open_conversation(&character.id)
            .expect("conversation");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("state");
        let (_task_cancel, task_cancelled) = tokio::sync::watch::channel(false);
        let admission_drops = Arc::new(AtomicUsize::new(0));
        let mut runtime_variables = VariableMap::default();
        runtime_variables.insert(
            VariableRef {
                scope: VariableScope::Character,
                namespace: None,
                id: VariableId::from("background_music"),
            },
            VariableValue::Text("1".to_owned()),
        );

        let started = shell
            .send_message_to_branch_async(
                SendMessageInput {
                    conversation_id: conversation.id.clone(),
                    branch_id: state.active_branch_id.clone(),
                    expected_head: None,
                    mode: ConversationModeDto::Chat,
                    text: "first".into(),
                    selection: synthetic_profile_selection(),
                    variable_overrides: runtime_variables,
                    operation_nonce: Some("shell-send-first".to_owned()),
                    generation_attempt_id: None,
                },
                GenerationCredential::legacy_with_admission_lease(
                    Some(SecretCredential::new(LIVE_CREDENTIAL_CANARY)),
                    AdmissionDropCounter(Arc::clone(&admission_drops)),
                ),
                &MissingTaskCredentialReader,
                task_cancelled.clone(),
            )
            .await
            .expect("send");
        assert_eq!(admission_drops.load(Ordering::SeqCst), 1);
        let (started_response, stream) = started.into_parts();
        assert_stream_finishes(stream, &started_response.generation_id).await;

        let messages = shell
            .list_branch_messages(&state.active_branch_id)
            .expect("first messages");
        assert_eq!(messages.len(), 2);
        let first_user = &messages[0];
        let first_assistant = &messages[1];

        let edit = shell
            .edit_user_message_async(
                EditUserMessageInput {
                    conversation_id: conversation.id.clone(),
                    branch_id: state.active_branch_id,
                    expected_head: Some(first_assistant.id.clone()),
                    message_id: first_user.id.clone(),
                    replacement_text: "edited".into(),
                    selection: synthetic_profile_selection(),
                    operation_nonce: Some("shell-edit-first".to_owned()),
                    generation_attempt_id: None,
                },
                GenerationCredential::legacy_with_admission_lease(
                    None,
                    AdmissionDropCounter(Arc::clone(&admission_drops)),
                ),
                &MissingTaskCredentialReader,
                task_cancelled.clone(),
            )
            .await
            .expect("edit");
        assert_eq!(admission_drops.load(Ordering::SeqCst), 2);
        let (edit_response, stream) = edit.into_parts();
        assert_stream_finishes(stream, &edit_response.generation_id).await;

        let edited_messages = shell
            .list_branch_messages(&edit_response.branch.id)
            .expect("edited messages");
        assert_eq!(edited_messages.len(), 2);
        assert_eq!(edited_messages[0].content, "edited");
        let edited_assistant = &edited_messages[1];

        let regenerate = shell
            .regenerate_assistant_message_async(
                RegenerateAssistantMessageInput {
                    conversation_id: conversation.id.clone(),
                    branch_id: edit_response.branch.id,
                    expected_head: Some(edited_assistant.id.clone()),
                    message_id: edited_assistant.id.clone(),
                    selection: synthetic_profile_selection(),
                    operation_nonce: Some("shell-regenerate-first".to_owned()),
                    generation_attempt_id: None,
                },
                GenerationCredential::legacy_with_admission_lease(
                    None,
                    AdmissionDropCounter(Arc::clone(&admission_drops)),
                ),
                &MissingTaskCredentialReader,
                task_cancelled,
            )
            .await
            .expect("regenerate");
        assert_eq!(admission_drops.load(Ordering::SeqCst), 3);
        let (regenerate_response, stream) = regenerate.into_parts();
        assert_stream_finishes(stream, &regenerate_response.generation_id).await;

        let regenerated_messages = shell
            .list_branch_messages(&regenerate_response.branch.id)
            .expect("regenerated messages");
        let regenerated_assistant = regenerated_messages.last().expect("assistant");
        let shortened = shell
            .remove_message_from_branch(RemoveMessageInput {
                conversation_id: conversation.id,
                branch_id: regenerate_response.branch.id,
                expected_head: Some(regenerated_assistant.id.clone()),
                message_id: regenerated_assistant.id.clone(),
            })
            .expect("remove");
        assert_eq!(
            shortened.head_message_id.as_deref(),
            regenerated_assistant.parent_id.as_deref()
        );

        provider_thread.join().expect("provider thread");
    }

    fn synthetic_profile_selection() -> GenerationSelectionInput {
        GenerationSelectionInput::LegacyProfile {
            provider_profile_id: "synthetic-profile".into(),
        }
    }

    async fn assert_stream_finishes(
        mut stream: crate::ChatEventStream,
        expected_generation_id: &str,
    ) {
        let mut last_sequence = 0;
        loop {
            let item = tokio::time::timeout(Duration::from_secs(5), stream.recv())
                .await
                .expect("stream timeout");
            match item {
                ChatStreamItem::Event(event) => {
                    assert_eq!(event.generation_id, expected_generation_id);
                    assert!(event.sequence > last_sequence);
                    assert!(
                        !serde_json::to_string(&event)
                            .expect("serialize event")
                            .contains(LIVE_CREDENTIAL_CANARY)
                    );
                    last_sequence = event.sequence;
                    if matches!(event.kind, crate::ChatEventKindDto::GenerationFinished) {
                        break;
                    }
                }
                ChatStreamItem::ReconciliationRequired(required) => {
                    panic!("unexpected reconciliation: {required:?}");
                }
                ChatStreamItem::Closed => panic!("stream closed before terminal event"),
            }
        }
    }

    fn spawn_completed_provider(request_count: usize) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind provider");
        let address = listener.local_addr().expect("provider address");
        let handle = thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().expect("accept provider request");
                read_request(&mut stream);
                let body = concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"reply\"}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n",
                    "data: [DONE]\n\n"
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write provider response");
            }
        });
        (format!("http://{address}/v1"), handle)
    }

    fn read_request(stream: &mut TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("request timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let read = stream.read(&mut buffer).expect("read request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
    }
}
