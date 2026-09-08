use super::{
    Connection, ConversationBranchId, ConversationId, CoreResult, InteractionKnowledgeBinding,
    InteractionState, MAX_STATE_JSON_BYTES, MessageId, OptionalExtension, decode_json, encode_json,
    interaction_state_snapshot_sha256, is_sha256, params, revision_conflict, storage_corrupted,
    storage_db_error, validate_knowledge_bindings, validate_state,
};

pub(super) fn read_pre_first_message_interaction_boundary(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
) -> CoreResult<(InteractionState, Vec<InteractionKnowledgeBinding>, String)> {
    read_initial_message_interaction_boundary(connection, conversation_id, source_branch_id, None)
}

pub(super) fn read_initial_message_interaction_boundary(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head_message_id: Option<&MessageId>,
) -> CoreResult<(InteractionState, Vec<InteractionKnowledgeBinding>, String)> {
    let historical = connection
        .query_row(
            "SELECT snapshot.previous_state_json,
                    snapshot.previous_knowledge_json,
                    snapshot.previous_state_snapshot_sha256,
                    snapshot.context_checkpoint_sha256
             FROM (
                 SELECT generation.id FROM generations AS generation
                 JOIN messages AS user_message
                   ON user_message.id = generation.user_message_id
                  AND user_message.conversation_id = generation.conversation_id
                 WHERE generation.conversation_id = ?1 AND generation.branch_id = ?2
                   AND user_message.parent_id IS ?3
                 ORDER BY generation.started_at, generation.id LIMIT 1
             ) AS initial_generation
             JOIN generation_attempt_before_event_snapshots AS snapshot
               ON snapshot.generation_id = initial_generation.id
              AND snapshot.context_head_message_id IS ?3",
            params![
                conversation_id.0.as_str(),
                source_branch_id.0.as_str(),
                context_head_message_id.map(|id| id.0.as_str())
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            revision_conflict(
                "pre-first-message interaction boundary has no generation attempt snapshot",
            )
        })?;
    let state: InteractionState = decode_json(
        "historical pre-first-message interaction state",
        &historical.0,
        MAX_STATE_JSON_BYTES,
    )?;
    let knowledge: Vec<InteractionKnowledgeBinding> = decode_json(
        "historical pre-first-message interaction knowledge",
        &historical.1,
        MAX_STATE_JSON_BYTES,
    )?;
    validate_state(&state)?;
    validate_knowledge_bindings(&state, &knowledge)?;
    if encode_json(
        "historical pre-first-message interaction state",
        &state,
        MAX_STATE_JSON_BYTES,
    )? != historical.0
        || encode_json(
            "historical pre-first-message interaction knowledge",
            &knowledge,
            MAX_STATE_JSON_BYTES,
        )? != historical.1
        || interaction_state_snapshot_sha256(&state, &knowledge)? != historical.2
        || !is_sha256(&historical.3)
    {
        return Err(storage_corrupted(
            "historical pre-first-message interaction snapshot is invalid",
        ));
    }
    Ok((state, knowledge, historical.3))
}
