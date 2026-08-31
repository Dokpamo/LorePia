use std::{fmt, path::Path};

use lorepia_core::{
    CORE_API_VERSION, ConnectionBoundCredential, Core, CoreConfig, CoreError,
    DiscoveryRecoveryOwner, GenerationId, GenerationOperationContext, ProviderConnectionId,
    ProviderCredentialAccessAuthority,
};

use crate::{
    BootstrapDto, ChatEventStream, GenerationStartedDto, HealthDto, MessageActionGenerationDto,
    ProviderCredentialAccessAuthorityContext, SecretCredential, ShellError, ShellResult,
    TaskCredentialLease,
};

mod conversation;
mod generation;
mod generation_async;
mod library;

pub use crate::dto::{
    ConversationGreetingSelectionInput, CreateConversationBranchInput, CreateConversationInput,
    EditUserMessageInput, GenerateRuntimeTextInput, GenerationSelectionInput,
    RegenerateAssistantMessageInput, RemoveMessageInput, RuntimePromptMessageInput,
    RuntimePromptRoleInput, RuntimeTextGenerationDto, SelectConversationBranchInput,
    SendMessageInput, SetConversationModeInput,
};

#[allow(unused_imports)]
pub use crate::dto::{RuntimeGenerationAuditInput, RuntimeGenerationCapabilityInput};

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
    include!("api/tests.rs");
}
