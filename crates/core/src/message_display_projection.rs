use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreResult, GenerationId, Message, MessageId,
    MessageRole, MessageStatus, Sha256Digest,
};
use lorepia_storage::{MessageTransformDiagnostic, StoredMessageDisplayProjection};
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

/// A bounded display window and its position in one branch snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePresentationPage {
    pub messages: Vec<MessagePresentation>,
    pub snapshot_token: String,
    pub has_older: bool,
    pub has_newer: bool,
    pub head_message_id: Option<MessageId>,
    pub total_messages: u64,
    pub start_index: u64,
    pub retained_message_ids: Option<Vec<MessageId>>,
    pub last_assistant_message: Option<MessagePresentation>,
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
    /// Reads one bounded page and verifies display sidecars only for its messages.
    pub fn list_branch_message_presentations_page(
        &self,
        branch_id: &ConversationBranchId,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
        limit: u32,
        check_message_ids: Option<&[MessageId]>,
        include_last_assistant: bool,
    ) -> CoreResult<MessagePresentationPage> {
        let page = self.storage().list_branch_messages_page(
            branch_id,
            before,
            after,
            limit,
            check_message_ids,
            include_last_assistant,
        )?;
        let mut projections = page
            .display_projections
            .into_iter()
            .map(|projection| (projection.message_id.clone(), projection))
            .collect();
        let messages = present_with_projections(page.messages, &mut projections)?;
        let last_assistant_message = match page.last_assistant_message {
            Some(message) => {
                if let Some(existing) = messages.iter().find(|item| item.message.id == message.id) {
                    Some(existing.clone())
                } else {
                    present_with_projections(vec![message], &mut projections)?.pop()
                }
            }
            None => None,
        };
        Ok(MessagePresentationPage {
            messages,
            snapshot_token: page.snapshot_token,
            last_assistant_message,
            has_older: page.has_older,
            has_newer: page.has_newer,
            head_message_id: page.head_message_id,
            total_messages: page.total_messages,
            start_index: page.start_index,
            retained_message_ids: page.retained_message_ids,
        })
    }

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

    /// Loads only the durable terminal pair belonging to this generation route.
    pub fn list_generation_message_presentations(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        generation_id: &GenerationId,
    ) -> CoreResult<Vec<MessagePresentation>> {
        self.present_messages(self.storage().list_generation_messages(
            conversation_id,
            branch_id,
            generation_id,
        )?)
    }

    fn present_messages(&self, messages: Vec<Message>) -> CoreResult<Vec<MessagePresentation>> {
        let eligible = messages
            .iter()
            .filter(|message| {
                message.role == MessageRole::Assistant
                    && message.status != MessageStatus::Pending
                    && message
                        .generation_id
                        .as_ref()
                        .is_some_and(|id| !id.is_character_greeting())
            })
            .collect::<Vec<_>>();
        let mut projections = self
            .storage()
            .get_message_display_projections(&eligible)?
            .into_iter()
            .map(|projection| (projection.message_id.clone(), projection))
            .collect::<std::collections::HashMap<_, _>>();
        present_with_projections(messages, &mut projections)
    }
}

fn present_with_projections(
    messages: Vec<Message>,
    projections: &mut std::collections::HashMap<MessageId, StoredMessageDisplayProjection>,
) -> CoreResult<Vec<MessagePresentation>> {
    messages
        .into_iter()
        .map(|message| {
            let projection = projections.remove(&message.id);
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

fn sha256_digest(bytes: &[u8]) -> CoreResult<Sha256Digest> {
    Sha256Digest::parse(hex::encode(Sha256::digest(bytes)))
        .map_err(|error| lorepia_domain::CoreError::internal(format!("invalid SHA-256: {error}")))
}
