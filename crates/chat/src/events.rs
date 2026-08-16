use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, GenerationId, GenerationUsage, MessageId, MessageStatus,
    ToolCallArgumentsDelta, ToolCallId, ToolName,
};
use serde::{Deserialize, Serialize};

/// Current wire version for structured chat events.
pub const CHAT_EVENT_VERSION: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatEvent {
    pub event_version: u32,
    pub generation_id: GenerationId,
    pub conversation_id: ConversationId,
    pub branch_id: Option<ConversationBranchId>,
    pub assistant_message_id: Option<MessageId>,
    pub sequence: u64,
    pub emitted_at: DateTime<Utc>,
    pub kind: ChatEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum ChatEventKind {
    GenerationStarted,
    ReasoningDelta(String),
    TextDelta(String),
    ToolCallStarted {
        id: ToolCallId,
        name: ToolName,
    },
    ToolCallArgumentsDelta {
        id: ToolCallId,
        delta: ToolCallArgumentsDelta,
    },
    ToolCallCompleted {
        id: ToolCallId,
    },
    UsageUpdated(GenerationUsage),
    MessageCommitted {
        message_id: MessageId,
        status: MessageStatus,
    },
    GenerationCancelled,
    GenerationFailed {
        code: String,
        message: String,
    },
    GenerationFinished,
}

impl ChatEvent {
    pub fn new(
        generation_id: GenerationId,
        conversation_id: ConversationId,
        sequence: u64,
        kind: ChatEventKind,
    ) -> Self {
        Self {
            event_version: CHAT_EVENT_VERSION,
            generation_id,
            conversation_id,
            branch_id: None,
            assistant_message_id: None,
            sequence,
            emitted_at: Utc::now(),
            kind,
        }
    }

    #[must_use]
    pub fn with_route(
        mut self,
        branch_id: ConversationBranchId,
        assistant_message_id: MessageId,
    ) -> Self {
        self.branch_id = Some(branch_id);
        self.assistant_message_id = Some(assistant_message_id);
        self
    }
}
