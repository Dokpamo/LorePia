use chrono::{DateTime, Utc};
use lorepia_core::{
    CHAT_EVENT_VERSION, ChatEvent, ChatEventKind, GenerationTarget, GenerationUsage,
    MessageActionGeneration, MessageRole, RuntimeGenerationCapability, VariableMap,
};
use serde::{Deserialize, Serialize};

use super::{ConversationBranchDto, ConversationModeDto, MessageStatusDto};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationTargetDto {
    pub model_route_id: String,
    pub generation_preset_id: String,
}

impl From<GenerationTargetDto> for GenerationTarget {
    fn from(value: GenerationTargetDto) -> Self {
        Self {
            model_route_id: value.model_route_id.into(),
            generation_preset_id: value.generation_preset_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationStartedDto {
    pub generation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageActionGenerationDto {
    pub branch: ConversationBranchDto,
    pub generation_id: String,
}

impl From<MessageActionGeneration> for MessageActionGenerationDto {
    fn from(value: MessageActionGeneration) -> Self {
        Self {
            branch: value.branch.into(),
            generation_id: value.generation_id.0,
        }
    }
}

/// UI projection of provider-neutral usage counters.
///
/// `provider_raw_summary` is deliberately omitted. It is not required to
/// render chat and remains inside Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationUsageDto {
    pub input_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub cached_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub tool_tokens: Option<u64>,
}

impl From<GenerationUsage> for GenerationUsageDto {
    fn from(value: GenerationUsage) -> Self {
        Self {
            input_tokens: value.input_tokens,
            cached_read_tokens: value.cached_read_tokens,
            cached_write_tokens: value.cached_write_tokens,
            output_tokens: value.output_tokens,
            reasoning_tokens: value.reasoning_tokens,
            tool_tokens: value.tool_tokens,
        }
    }
}

/// UI-safe projection of the current Core `ChatEventKind`.
///
/// Variant names and payload topology intentionally match `ChatEvent` v4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ChatEventKindDto {
    GenerationStarted,
    ReasoningDelta(String),
    TextDelta(String),
    ToolCallStarted {
        id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        id: String,
        delta: String,
    },
    ToolCallCompleted {
        id: String,
    },
    UsageUpdated(GenerationUsageDto),
    MessageCommitted {
        message_id: String,
        status: MessageStatusDto,
    },
    GenerationCancelled,
    GenerationFailed {
        code: String,
        message: String,
    },
    GenerationFinished,
}

impl From<ChatEventKind> for ChatEventKindDto {
    fn from(value: ChatEventKind) -> Self {
        match value {
            ChatEventKind::GenerationStarted => Self::GenerationStarted,
            ChatEventKind::ReasoningDelta(delta) => Self::ReasoningDelta(delta),
            ChatEventKind::TextDelta(delta) => Self::TextDelta(delta),
            ChatEventKind::ToolCallStarted { id, name } => Self::ToolCallStarted {
                id: id.into_inner(),
                name: name.into_inner(),
            },
            ChatEventKind::ToolCallArgumentsDelta { id, delta } => Self::ToolCallArgumentsDelta {
                id: id.into_inner(),
                delta: delta.into_inner(),
            },
            ChatEventKind::ToolCallCompleted { id } => Self::ToolCallCompleted {
                id: id.into_inner(),
            },
            ChatEventKind::UsageUpdated(usage) => Self::UsageUpdated(usage.into()),
            ChatEventKind::MessageCommitted { message_id, status } => Self::MessageCommitted {
                message_id: message_id.0,
                status: status.into(),
            },
            ChatEventKind::GenerationCancelled => Self::GenerationCancelled,
            ChatEventKind::GenerationFailed { code, message } => {
                Self::GenerationFailed { code, message }
            }
            ChatEventKind::GenerationFinished => Self::GenerationFinished,
        }
    }
}

impl ChatEventKindDto {
    pub(crate) const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::GenerationCancelled | Self::GenerationFailed { .. } | Self::GenerationFinished
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatEventDto {
    pub event_version: u32,
    pub generation_id: String,
    pub conversation_id: String,
    pub branch_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub sequence: u64,
    pub emitted_at: DateTime<Utc>,
    pub kind: ChatEventKindDto,
}

impl From<ChatEvent> for ChatEventDto {
    fn from(value: ChatEvent) -> Self {
        Self {
            event_version: value.event_version,
            generation_id: value.generation_id.0,
            conversation_id: value.conversation_id.0,
            branch_id: value.branch_id.map(|id| id.0),
            assistant_message_id: value.assistant_message_id.map(|id| id.0),
            sequence: value.sequence,
            emitted_at: value.emitted_at,
            kind: value.kind.into(),
        }
    }
}

impl ChatEventDto {
    pub const fn is_supported_version(&self) -> bool {
        self.event_version == CHAT_EVENT_VERSION
    }
}
