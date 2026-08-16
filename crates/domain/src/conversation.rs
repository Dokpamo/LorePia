use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{GenerationId, Message, MessageId};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationId(pub String);

impl ConversationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for ConversationId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConversationBranchId(pub String);

impl ConversationBranchId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for ConversationBranchId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationMode {
    Chat,
    Story,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conversation {
    pub id: ConversationId,
    pub character_id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationBranch {
    pub id: ConversationBranchId,
    pub conversation_id: ConversationId,
    pub title: Option<String>,
    pub fork_message_id: Option<MessageId>,
    pub head_message_id: Option<MessageId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConversationBranch {
    pub fn root(conversation_id: ConversationId) -> Self {
        let now = Utc::now();
        Self {
            id: ConversationBranchId::new(),
            conversation_id,
            title: None,
            fork_message_id: None,
            head_message_id: None,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationState {
    pub conversation_id: ConversationId,
    pub active_branch_id: ConversationBranchId,
    pub selected_mode: ConversationMode,
    pub updated_at: DateTime<Utc>,
}

/// The stable, non-secret classification of a character greeting.
///
/// Greeting text intentionally is not part of this projection. Callers select
/// by ID and the Rust storage boundary resolves the immutable content only
/// while atomically creating a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterGreetingKind {
    Default,
    Alternate,
}

/// One safe greeting selector option for the exact active character revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterGreetingOption {
    pub id: String,
    pub kind: CharacterGreetingKind,
    pub enabled: bool,
}

/// A read-only selector catalog bound to one exact character-content revision.
///
/// `None` is meaningful: it represents a legacy character with no active
/// character-content sidecar and can be supplied as an exact-absence binding
/// when the conversation is created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterGreetingCatalog {
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub greetings: Vec<CharacterGreetingOption>,
}

/// Immutable character-content and greeting provenance captured when a
/// conversation is created.
///
/// A missing content revision is an exact legacy-absence binding. A greeting
/// can only be present together with its immutable content revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationGreetingBinding {
    pub conversation_id: ConversationId,
    pub character_content_revision_id: Option<String>,
    pub greeting_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// The durable result of creating a conversation from a greeting selection.
///
/// The initial message is an ordinary committed chat message. Public selector
/// APIs should continue to project only [`CharacterGreetingOption`] rather than
/// returning greeting source text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationStart {
    pub conversation: Conversation,
    pub branch: ConversationBranch,
    pub state: ConversationState,
    pub initial_message: Option<Message>,
    pub character_content_revision_id: Option<String>,
    pub greeting_id: Option<String>,
}

/// A new immutable conversation branch together with the generation it started.
///
/// Editing or regenerating never rewrites an existing message. Instead, the
/// operation forks a branch and appends a fresh user/assistant generation pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageActionGeneration {
    pub branch: ConversationBranch,
    pub generation_id: GenerationId,
}

impl Conversation {
    pub fn new(character_id: impl Into<String>, title: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            id: ConversationId::new(),
            character_id: character_id.into(),
            title: title.into(),
            created_at: now,
            updated_at: now,
        }
    }
}
