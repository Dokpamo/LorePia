//! Search newest-first identities without constructing an unbounded JSON parameter.
use super::{
    Connection, CoreError, CoreResult, Message, MessageRole, OptionalExtension, map_message,
    params, storage_corrupted, storage_db_error,
};

const IDENTITY_BATCH: usize = 128;

pub(super) fn load_last_assistant(
    connection: &Connection,
    conversation_id: &str,
    lineage: &[String],
    page_messages: &[Message],
    start: usize,
) -> CoreResult<Option<Message>> {
    let candidates = if page_messages
        .iter()
        .any(|message| message.role == MessageRole::Assistant)
    {
        lineage
            .get(..start)
            .ok_or_else(|| storage_corrupted("assistant page start exceeds lineage"))?
    } else {
        lineage
    };
    for batch in candidates.chunks(IDENTITY_BATCH) {
        let ids = serde_json::to_string(batch).map_err(|error| {
            CoreError::internal(format!(
                "cannot encode assistant lookup identities: {error}"
            ))
        })?;
        let selected: Option<String> = connection
            .prepare_cached(
                "SELECT message.id FROM json_each(?1) AS lineage
             JOIN messages AS message ON message.id=lineage.value
             WHERE message.conversation_id=?2 AND message.role='assistant'
             ORDER BY CAST(lineage.key AS INTEGER) LIMIT 1",
            )
            .map_err(storage_db_error)?
            .query_row(params![ids, conversation_id], |row| row.get(0))
            .optional()
            .map_err(storage_db_error)?;
        let Some(selected) = selected else {
            continue;
        };
        if page_messages.iter().any(|message| message.id.0 == selected) {
            return Ok(None);
        }
        return connection
            .prepare_cached(
                "SELECT id, conversation_id, parent_id, role, content, status,
                    generation_id, created_at FROM messages_with_checkpoints
             WHERE id=?1 AND conversation_id=?2 AND role='assistant'",
            )
            .map_err(storage_db_error)?
            .query_row(params![selected, conversation_id], map_message)
            .optional()
            .map_err(storage_db_error)?
            .map(Some)
            .ok_or_else(|| storage_corrupted("assistant lookup lost its selected message"));
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
