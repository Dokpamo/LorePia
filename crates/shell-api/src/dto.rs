use chrono::{DateTime, Utc};
use lorepia_core::{
    CHAT_EVENT_VERSION, Character, CharacterGreetingCatalog, CharacterGreetingKind,
    CharacterGreetingOption, ChatEvent, ChatEventKind, ContentKind, Conversation,
    ConversationBranch, ConversationMode, ConversationState, GenerationTarget, GenerationUsage,
    HealthReport, ImportImagePreview, ImportInspection, ImportWarning, Message,
    MessageActionGeneration, MessagePresentation, MessageRole, MessageStatus,
    MessageTransformDiagnostic, MessageTransformDisposition, MessageTransformStage,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BootstrapDto {
    pub shell_api_version: u32,
    pub core_api_version: u32,
    pub chat_event_version: u32,
    pub health: HealthDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct HealthDto {
    pub core_version: String,
    pub database_open: bool,
    pub schema_version: u32,
    pub data_root_writable: bool,
    pub staging_writable: bool,
    pub recovery_pending: bool,
    pub active_jobs: u32,
}

impl From<HealthReport> for HealthDto {
    fn from(value: HealthReport) -> Self {
        Self {
            core_version: value.core_version,
            database_open: value.database_open,
            schema_version: value.schema_version,
            data_root_writable: value.data_root_writable,
            staging_writable: value.staging_writable,
            recovery_pending: value.recovery_pending,
            active_jobs: value.active_jobs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source_hash: String,
    /// Content-addressed logical identifier, never a host filesystem path.
    pub avatar_asset_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<Character> for CharacterDto {
    fn from(value: Character) -> Self {
        Self {
            id: value.id,
            name: value.name,
            description: value.description,
            source_hash: value.source_hash,
            avatar_asset_id: value.avatar_asset_hash,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterGreetingKindDto {
    Default,
    Alternate,
}

impl From<CharacterGreetingKind> for CharacterGreetingKindDto {
    fn from(value: CharacterGreetingKind) -> Self {
        match value {
            CharacterGreetingKind::Default => Self::Default,
            CharacterGreetingKind::Alternate => Self::Alternate,
        }
    }
}

/// Safe greeting selector metadata. Source greeting text is intentionally
/// absent and can only be resolved by Core during an exact conversation start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterGreetingOptionDto {
    pub id: String,
    pub kind: CharacterGreetingKindDto,
    pub enabled: bool,
}

impl From<CharacterGreetingOption> for CharacterGreetingOptionDto {
    fn from(value: CharacterGreetingOption) -> Self {
        Self {
            id: value.id,
            kind: value.kind.into(),
            enabled: value.enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterGreetingCatalogDto {
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub greetings: Vec<CharacterGreetingOptionDto>,
}

impl From<CharacterGreetingCatalog> for CharacterGreetingCatalogDto {
    fn from(value: CharacterGreetingCatalog) -> Self {
        Self {
            character_id: value.character_id,
            character_content_revision_id: value.character_content_revision_id,
            greetings: value.greetings.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKindDto {
    CharacterCardV3,
    CharacterCardPng,
    CharxPackage,
}

impl From<ContentKind> for ContentKindDto {
    fn from(value: ContentKind) -> Self {
        match value {
            ContentKind::CharacterCardV3 => Self::CharacterCardV3,
            ContentKind::CharacterCardPng => Self::CharacterCardPng,
            ContentKind::CharxPackage => Self::CharxPackage,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportWarningDto {
    pub code: String,
    pub message: String,
}

impl From<ImportWarning> for ImportWarningDto {
    fn from(value: ImportWarning) -> Self {
        Self {
            code: value.code,
            message: value.message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportImagePreviewDto {
    pub logical_asset_id: String,
    pub media_type: String,
    pub size_bytes: u64,
}

impl From<ImportImagePreview> for ImportImagePreviewDto {
    fn from(value: ImportImagePreview) -> Self {
        Self {
            logical_asset_id: value.logical_asset_id,
            media_type: value.media_type,
            size_bytes: value.size_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportInspectionDto {
    pub inspection_id: String,
    pub kind: ContentKindDto,
    pub display_name: String,
    pub description: String,
    pub representative_image: Option<ImportImagePreviewDto>,
    pub source_sha256: String,
    pub source_size: u64,
    pub estimated_stored_size: u64,
    pub asset_count: u32,
    pub warnings: Vec<ImportWarningDto>,
    pub blocked_reasons: Vec<String>,
    pub unsupported_optional_fields: Vec<String>,
    pub allowed: bool,
}

impl From<ImportInspection> for ImportInspectionDto {
    fn from(value: ImportInspection) -> Self {
        let allowed = value.is_allowed();
        Self {
            inspection_id: value.id.0,
            kind: value.kind.into(),
            display_name: value.display_name,
            description: value.description,
            representative_image: value.representative_image.map(Into::into),
            source_sha256: value.source_sha256,
            source_size: value.source_size,
            estimated_stored_size: value.estimated_stored_size,
            asset_count: value.asset_count,
            warnings: value.warnings.into_iter().map(Into::into).collect(),
            blocked_reasons: value.blocked_reasons,
            unsupported_optional_fields: value.unsupported_optional_fields,
            allowed,
        }
    }
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

#[cfg(test)]
mod tests {
    use lorepia_core::{
        BoundedJson, ChatEvent, ChatEventKind, ConversationId, GenerationId, GenerationUsage,
        Message, MessageId, MessagePresentation, MessageStatus, MessageTransformDiagnostic,
        MessageTransformDisposition, MessageTransformStage, Sha256Digest,
    };

    use super::{ChatEventDto, ChatEventKindDto, MessageDto};

    #[test]
    fn event_projection_preserves_v4_wire_variant_and_omits_raw_usage_summary() {
        let canary = "provider-raw-summary-canary";
        let event = ChatEvent::new(
            GenerationId("generation-1".to_owned()),
            ConversationId("conversation-1".to_owned()),
            7,
            ChatEventKind::UsageUpdated(GenerationUsage {
                input_tokens: Some(11),
                cached_read_tokens: Some(3),
                cached_write_tokens: None,
                output_tokens: Some(5),
                reasoning_tokens: Some(2),
                tool_tokens: None,
                provider_raw_summary: Some(
                    BoundedJson::parse(format!(r#"{{"private":"{canary}"}}"#))
                        .expect("bounded metadata"),
                ),
            }),
        );

        let dto = ChatEventDto::from(event);
        let json = serde_json::to_string(&dto).expect("serialize event DTO");

        assert!(matches!(dto.kind, ChatEventKindDto::UsageUpdated(_)));
        assert!(json.contains(r#""type":"usage_updated""#));
        assert!(!json.contains("provider_raw_summary"));
        assert!(!json.contains(canary));
    }

    #[test]
    fn message_projection_uses_display_text_and_exposes_only_content_free_diagnostics() {
        let generation_id = GenerationId("generation-display-1".to_owned());
        let mut message = Message::pending_assistant(
            ConversationId("conversation-display-1".to_owned()),
            MessageId("user-display-1".to_owned()),
            generation_id,
        );
        message.content = "CANONICAL_CONTENT_CANARY".to_owned();
        message.status = MessageStatus::Complete;
        let recorded_at = message.created_at;
        let presentation = MessagePresentation {
            message,
            display_content: "Rendered display text".to_owned(),
            canonical_content_sha256: Sha256Digest::parse("a".repeat(64))
                .expect("canonical digest"),
            display_content_sha256: Sha256Digest::parse("b".repeat(64)).expect("display digest"),
            projection_diagnostics_sha256: Some(
                Sha256Digest::parse("c".repeat(64)).expect("diagnostic digest"),
            ),
            transform_diagnostics: vec![MessageTransformDiagnostic {
                set_revision_id: Some("transform-revision-1".to_owned()),
                rule_id: Some("display-rule-1".to_owned()),
                stage: MessageTransformStage::DisplayOnly,
                disposition: MessageTransformDisposition::Applied,
                code: None,
                before_sha256: Sha256Digest::parse("a".repeat(64)).expect("before digest"),
                after_sha256: Some(Sha256Digest::parse("b".repeat(64)).expect("after digest")),
                recorded_at,
            }],
        };

        let dto = MessageDto::from(presentation);
        let json = serde_json::to_string(&dto).expect("serialize projected message DTO");
        let projection = dto.display_projection.expect("display projection metadata");

        assert_eq!(dto.content, "Rendered display text");
        assert_eq!(projection.canonical_content_sha256, "a".repeat(64));
        assert_eq!(projection.display_content_sha256, "b".repeat(64));
        assert_eq!(projection.diagnostics_sha256, "c".repeat(64));
        assert_eq!(projection.diagnostics.len(), 1);
        assert!(!json.contains("CANONICAL_CONTENT_CANARY"));
        assert_eq!(json.matches("Rendered display text").count(), 1);
        assert!(!json.contains("\"pattern\""));
        assert!(!json.contains("\"replacement\""));
    }
}
