use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreResult, Message, MessageRole, MessageStatus,
    Sha256Digest,
};
use lorepia_storage::MessageTransformDiagnostic;
use sha2::{Digest, Sha256};

use crate::Core;

/// UI-safe representation of one canonical message.
///
/// `message.content` always remains the exact stored canonical value. Only
/// `display_content` may contain the separately persisted `DisplayOnly` result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePresentation {
    pub message: Message,
    pub display_content: String,
    pub canonical_content_sha256: Sha256Digest,
    pub display_content_sha256: Sha256Digest,
    pub projection_diagnostics_sha256: Option<Sha256Digest>,
    pub transform_diagnostics: Vec<MessageTransformDiagnostic>,
}

impl MessagePresentation {
    /// Produces the existing product message shape for render-only clients.
    /// This consumes a clone owned by the caller and never mutates storage.
    #[must_use]
    pub fn into_display_message(mut self) -> Message {
        self.message.content = self.display_content;
        self.message
    }
}

impl Core {
    /// Lists one branch with hash-verified, Core-owned display projections.
    /// Canonical message content remains available on every item.
    pub fn list_branch_message_presentations(
        &self,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Vec<MessagePresentation>> {
        let messages = self.list_branch_messages(branch_id)?;
        self.present_messages(messages)
    }

    /// Lists the active branch of a conversation with display projections.
    pub fn list_message_presentations(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<Vec<MessagePresentation>> {
        let state = self.storage().get_conversation_state(conversation_id)?;
        self.list_branch_message_presentations(&state.active_branch_id)
    }

    fn present_messages(&self, messages: Vec<Message>) -> CoreResult<Vec<MessagePresentation>> {
        messages
            .into_iter()
            .map(|message| {
                let eligible = message.role == MessageRole::Assistant
                    && message.status != MessageStatus::Pending
                    && message
                        .generation_id
                        .as_ref()
                        .is_some_and(|generation_id| !generation_id.is_character_greeting());
                let projection = if eligible {
                    self.storage().get_message_display_projection(&message)?
                } else {
                    None
                };
                if let Some(projection) = projection {
                    return Ok(MessagePresentation {
                        message,
                        display_content: projection.display_content,
                        canonical_content_sha256: projection.canonical_content_sha256,
                        display_content_sha256: projection.display_content_sha256,
                        projection_diagnostics_sha256: Some(projection.diagnostics_sha256),
                        transform_diagnostics: projection.diagnostics,
                    });
                }
                let display_content_sha256 = sha256_digest(message.content.as_bytes())?;
                Ok(MessagePresentation {
                    display_content: message.content.clone(),
                    message,
                    canonical_content_sha256: display_content_sha256.clone(),
                    display_content_sha256,
                    projection_diagnostics_sha256: None,
                    transform_diagnostics: Vec::new(),
                })
            })
            .collect()
    }
}

fn sha256_digest(bytes: &[u8]) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(format!("{:x}", Sha256::digest(bytes)))
        .map_err(|error| lorepia_domain::CoreError::internal(format!("invalid SHA-256: {error}")))
}
