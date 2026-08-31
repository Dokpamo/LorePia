use chrono::{DateTime, Utc};
use lorepia_core::{
    Conversation, ConversationBranch, ConversationMode, ConversationState, Message,
    MessagePresentation, MessageRole, MessageStatus, MessageTransformDiagnostic,
    MessageTransformDisposition, MessageTransformStage,
};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationModeDto {
    Chat,
    Story,
}

impl From<ConversationMode> for ConversationModeDto {
    fn from(value: ConversationMode) -> Self {
        match value {
            ConversationMode::Chat => Self::Chat,
            ConversationMode::Story => Self::Story,
        }
    }
}

impl From<ConversationModeDto> for ConversationMode {
    fn from(value: ConversationModeDto) -> Self {
        match value {
            ConversationModeDto::Chat => Self::Chat,
            ConversationModeDto::Story => Self::Story,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationDto {
    pub id: String,
    pub character_id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<Conversation> for ConversationDto {
    fn from(value: Conversation) -> Self {
        Self {
            id: value.id.0,
            character_id: value.character_id,
            title: value.title,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationBranchDto {
    pub id: String,
    pub conversation_id: String,
    pub title: Option<String>,
    pub fork_message_id: Option<String>,
    pub head_message_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<ConversationBranch> for ConversationBranchDto {
    fn from(value: ConversationBranch) -> Self {
        Self {
            id: value.id.0,
            conversation_id: value.conversation_id.0,
            title: value.title,
            fork_message_id: value.fork_message_id.map(|id| id.0),
            head_message_id: value.head_message_id.map(|id| id.0),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationStateDto {
    pub conversation_id: String,
    pub active_branch_id: String,
    pub selected_mode: ConversationModeDto,
    pub updated_at: DateTime<Utc>,
}

impl From<ConversationState> for ConversationStateDto {
    fn from(value: ConversationState) -> Self {
        Self {
            conversation_id: value.conversation_id.0,
            active_branch_id: value.active_branch_id.0,
            selected_mode: value.selected_mode.into(),
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRoleDto {
    System,
    User,
    Assistant,
}

impl From<MessageRole> for MessageRoleDto {
    fn from(value: MessageRole) -> Self {
        match value {
            MessageRole::System => Self::System,
            MessageRole::User => Self::User,
            MessageRole::Assistant => Self::Assistant,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatusDto {
    Pending,
    Complete,
    Cancelled,
    Failed,
}

impl From<MessageStatus> for MessageStatusDto {
    fn from(value: MessageStatus) -> Self {
        match value {
            MessageStatus::Pending => Self::Pending,
            MessageStatus::Complete => Self::Complete,
            MessageStatus::Cancelled => Self::Cancelled,
            MessageStatus::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDto {
    pub id: String,
    pub conversation_id: String,
    pub parent_id: Option<String>,
    pub role: MessageRoleDto,
    pub content: String,
    pub status: MessageStatusDto,
    pub generation_id: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Present only when Core loaded a hash-verified `DisplayOnly` sidecar.
    /// Canonical message text never crosses in this metadata object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_projection: Option<MessageDisplayProjectionDto>,
}

impl From<Message> for MessageDto {
    fn from(value: Message) -> Self {
        Self {
            id: value.id.0,
            conversation_id: value.conversation_id.0,
            parent_id: value.parent_id.map(|id| id.0),
            role: value.role.into(),
            content: value.content,
            status: value.status.into(),
            generation_id: value.generation_id.map(|id| id.0),
            created_at: value.created_at,
            display_projection: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageTransformStageDto {
    ProviderOutputCanonical,
    DisplayOnly,
}

impl From<MessageTransformStage> for MessageTransformStageDto {
    fn from(value: MessageTransformStage) -> Self {
        match value {
            MessageTransformStage::ProviderOutputCanonical => Self::ProviderOutputCanonical,
            MessageTransformStage::DisplayOnly => Self::DisplayOnly,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageTransformDispositionDto {
    Applied,
    NoMatch,
    Disabled,
    PendingImportApproval,
    ResolvedPromptDisabled,
    ConditionFalse,
    Failed,
    LimitRejected,
    PipelineRejected,
}

impl From<MessageTransformDisposition> for MessageTransformDispositionDto {
    fn from(value: MessageTransformDisposition) -> Self {
        match value {
            MessageTransformDisposition::Applied => Self::Applied,
            MessageTransformDisposition::NoMatch => Self::NoMatch,
            MessageTransformDisposition::Disabled => Self::Disabled,
            MessageTransformDisposition::PendingImportApproval => Self::PendingImportApproval,
            MessageTransformDisposition::ResolvedPromptDisabled => Self::ResolvedPromptDisabled,
            MessageTransformDisposition::ConditionFalse => Self::ConditionFalse,
            MessageTransformDisposition::Failed => Self::Failed,
            MessageTransformDisposition::LimitRejected => Self::LimitRejected,
            MessageTransformDisposition::PipelineRejected => Self::PipelineRejected,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageTransformDiagnosticDto {
    pub set_revision_id: Option<String>,
    pub rule_id: Option<String>,
    pub stage: MessageTransformStageDto,
    pub disposition: MessageTransformDispositionDto,
    pub code: Option<String>,
    pub before_sha256: String,
    pub after_sha256: Option<String>,
    pub recorded_at: DateTime<Utc>,
}

impl From<MessageTransformDiagnostic> for MessageTransformDiagnosticDto {
    fn from(value: MessageTransformDiagnostic) -> Self {
        Self {
            set_revision_id: value.set_revision_id,
            rule_id: value.rule_id,
            stage: value.stage.into(),
            disposition: value.disposition.into(),
            code: value.code,
            before_sha256: value.before_sha256.into_inner(),
            after_sha256: value
                .after_sha256
                .map(lorepia_core::Sha256Digest::into_inner),
            recorded_at: value.recorded_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MessageDisplayProjectionDto {
    pub canonical_content_sha256: String,
    pub display_content_sha256: String,
    pub diagnostics_sha256: String,
    pub diagnostics: Vec<MessageTransformDiagnosticDto>,
}

impl From<MessagePresentation> for MessageDto {
    fn from(value: MessagePresentation) -> Self {
        let MessagePresentation {
            message,
            display_content,
            canonical_content_sha256,
            display_content_sha256,
            projection_diagnostics_sha256,
            transform_diagnostics,
        } = value;
        let mut dto = Self::from(message);
        dto.content = display_content;
        dto.display_projection =
            projection_diagnostics_sha256.map(|diagnostics_sha256| MessageDisplayProjectionDto {
                canonical_content_sha256: canonical_content_sha256.into_inner(),
                display_content_sha256: display_content_sha256.into_inner(),
                diagnostics_sha256: diagnostics_sha256.into_inner(),
                diagnostics: transform_diagnostics.into_iter().map(Into::into).collect(),
            });
        dto
    }
}
