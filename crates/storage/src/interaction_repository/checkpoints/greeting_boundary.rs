use super::initial_boundary::read_initial_message_interaction_boundary;
use super::{
    Connection, ConversationBranchId, ConversationId, CoreResult, GenerationId,
    InteractionKnowledgeBinding, InteractionState, MessageId, OptionalExtension,
    interaction_state_snapshot_sha256, not_found, params, read_knowledge_bindings, read_state_row,
    storage_db_error, validate_normalized_state,
};

/// A greeting has durable conversation-start provenance, not a provider generation.
/// Current state is valid only before this branch has appended any generation.
/// Rewinding the head does not rewind state, so historical evidence wins even
/// when a later message removal makes the greeting the current head again.
pub(super) fn read_character_greeting_interaction_boundary(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    message_id: &MessageId,
) -> CoreResult<Option<(InteractionState, Vec<InteractionKnowledgeBinding>, String)>> {
    let marker = GenerationId::for_character_greeting(conversation_id);
    let greeting_exists = connection
        .query_row(
            "SELECT EXISTS(
            SELECT 1 FROM messages AS message
            JOIN conversation_greeting_bindings AS binding
              ON binding.conversation_id = message.conversation_id
            JOIN character_greetings AS greeting
              ON greeting.character_content_revision_id = binding.character_content_revision_id
             AND greeting.greeting_id = binding.greeting_id
            JOIN core_lifecycle_outbox AS started
              ON started.conversation_id = message.conversation_id
             AND started.event_kind = 'conversation_started'
             AND started.exact_head_message_id = message.id
             AND started.owner_message_id IS NULL AND started.generation_id IS NULL
             AND started.status = 'acknowledged'
            WHERE message.conversation_id = ?1 AND message.id = ?2
              AND message.generation_id = ?3 AND message.parent_id IS NULL
              AND message.role = 'assistant' AND message.status = 'complete'
              AND message.content = greeting.content
        )",
            params![conversation_id.0, message_id.0, marker.0],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if !greeting_exists {
        return Ok(None);
    }
    let has_generation_history = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM generations WHERE conversation_id = ?1 AND branch_id = ?2)",
        params![conversation_id.0, source_branch_id.0],
        |row| row.get::<_, bool>(0),
    ).map_err(storage_db_error)?;
    if has_generation_history {
        return read_initial_message_interaction_boundary(
            connection,
            conversation_id,
            source_branch_id,
            Some(message_id),
        )
        .map(Some);
    }
    let source_head = connection
        .query_row(
            "SELECT head_message_id FROM conversation_branches
         WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0, source_branch_id.0],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("source conversation branch"))?;
    if source_head.as_deref() == Some(message_id.0.as_str()) {
        let current = read_state_row(connection, conversation_id, source_branch_id)?
            .ok_or_else(|| not_found("source interaction state"))?;
        validate_normalized_state(connection, &current)?;
        let knowledge = read_knowledge_bindings(connection, &current.id)?;
        let digest = interaction_state_snapshot_sha256(&current.state, &knowledge)?;
        return Ok(Some((current.state, knowledge, digest)));
    }
    read_initial_message_interaction_boundary(
        connection,
        conversation_id,
        source_branch_id,
        Some(message_id),
    )
    .map(Some)
}
