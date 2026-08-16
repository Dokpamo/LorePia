use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiFamily, BoundedJson, ConversationBranchId, ConversationId, ConversationMode,
    GenerationPresetId, ModelRouteId, OpaqueReasoningState,
};

pub const CHARACTER_GREETING_GENERATION_ID_PREFIX: &str = "greeting:";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId(pub String);

impl MessageId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GenerationId(pub String);

impl GenerationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Creates the reserved message provenance marker used by a committed
    /// character greeting. It is intentionally not a provider generation row.
    pub fn for_character_greeting(conversation_id: &ConversationId) -> Self {
        Self(format!(
            "{CHARACTER_GREETING_GENERATION_ID_PREFIX}{}",
            conversation_id.0
        ))
    }

    pub fn is_character_greeting(&self) -> bool {
        self.0
            .strip_prefix(CHARACTER_GREETING_GENERATION_ID_PREFIX)
            .is_some_and(|conversation_id| !conversation_id.is_empty())
    }
}

impl Default for GenerationId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Pending,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationStatus {
    Running,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationRecord {
    pub id: GenerationId,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub user_message_id: MessageId,
    pub assistant_message_id: Option<MessageId>,
    pub mode: ConversationMode,
    pub model: String,
    /// Stable provider-catalog provenance. Legacy profile generations keep
    /// both fields unset.
    #[serde(default)]
    pub model_route_id: Option<ModelRouteId>,
    #[serde(default)]
    pub generation_preset_id: Option<GenerationPresetId>,
    #[serde(default)]
    pub provider_family: Option<ApiFamily>,
    pub status: GenerationStatus,
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub cached_read_tokens: Option<u64>,
    #[serde(default)]
    pub cached_write_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub tool_tokens: Option<u64>,
    #[serde(default)]
    pub provider_raw_summary: Option<BoundedJson>,
    /// Internal-only provider continuity state. Storage hydrates this field
    /// explicitly; generic DTO serialization must never expose it.
    #[serde(default, skip_serializing, skip_deserializing)]
    pub opaque_reasoning_state: Vec<OpaqueReasoningState>,
    pub error_code: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub conversation_id: ConversationId,
    pub parent_id: Option<MessageId>,
    pub role: MessageRole,
    pub content: String,
    pub status: MessageStatus,
    pub generation_id: Option<GenerationId>,
    pub created_at: DateTime<Utc>,
}

impl Message {
    pub fn user(conversation_id: ConversationId, content: impl Into<String>) -> Self {
        Self::user_after(conversation_id, None, content)
    }

    pub fn user_after(
        conversation_id: ConversationId,
        parent_id: Option<MessageId>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            id: MessageId::new(),
            conversation_id,
            parent_id,
            role: MessageRole::User,
            content: content.into(),
            status: MessageStatus::Complete,
            generation_id: None,
            created_at: Utc::now(),
        }
    }

    pub fn pending_assistant(
        conversation_id: ConversationId,
        parent_id: MessageId,
        generation_id: GenerationId,
    ) -> Self {
        Self {
            id: MessageId::new(),
            conversation_id,
            parent_id: Some(parent_id),
            role: MessageRole::Assistant,
            content: String::new(),
            status: MessageStatus::Pending,
            generation_id: Some(generation_id),
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        CHARACTER_GREETING_GENERATION_ID_PREFIX, ConversationBranchId, ConversationId,
        ConversationMode, GenerationId, GenerationRecord, GenerationStatus, MessageId,
    };
    use crate::{OpaqueReasoningState, OpenAiResponsesReasoningItem};

    #[test]
    fn generation_record_generic_serialization_never_exposes_opaque_state() {
        let item_id_canary = "generation-record-item-id-canary";
        let data_canary = "generation-record-data-canary";
        let record = GenerationRecord {
            id: GenerationId::new(),
            conversation_id: ConversationId::new(),
            branch_id: ConversationBranchId::new(),
            user_message_id: MessageId::new(),
            assistant_message_id: Some(MessageId::new()),
            mode: ConversationMode::Chat,
            model: "model".to_owned(),
            model_route_id: None,
            generation_preset_id: None,
            provider_family: None,
            status: GenerationStatus::Complete,
            input_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
            opaque_reasoning_state: vec![OpaqueReasoningState::OpenAiResponses {
                item: OpenAiResponsesReasoningItem::from_value(&serde_json::json!({
                    "id": item_id_canary,
                    "type": "reasoning",
                    "summary": [],
                    "encrypted_content": data_canary
                }))
                .expect("reasoning item"),
            }],
            error_code: None,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
        };

        let debug = format!("{record:?}");
        assert!(!debug.contains(item_id_canary));
        assert!(!debug.contains(data_canary));
        let encoded = serde_json::to_string(&record).expect("encode generation record");
        assert!(!encoded.contains("opaque_reasoning_state"));
        assert!(!encoded.contains(item_id_canary));
        assert!(!encoded.contains(data_canary));
    }

    #[test]
    fn character_greeting_generation_marker_is_reserved_and_detectable() {
        let conversation_id = ConversationId("conversation-greeting".to_owned());
        let marker = GenerationId::for_character_greeting(&conversation_id);
        assert_eq!(
            marker.0,
            format!(
                "{CHARACTER_GREETING_GENERATION_ID_PREFIX}{}",
                conversation_id.0
            )
        );
        assert!(marker.is_character_greeting());
        assert!(!GenerationId::new().is_character_greeting());
        assert!(
            !GenerationId(CHARACTER_GREETING_GENERATION_ID_PREFIX.to_owned())
                .is_character_greeting()
        );
    }
}
