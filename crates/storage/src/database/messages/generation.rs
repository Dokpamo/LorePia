use super::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    Message, MessageRole, MessageStatus, Storage, map_message, params, storage_db_error,
};

impl Storage {
    /// Returns at most the user and terminal assistant of an exact generation
    /// route. Incomplete/deleted generations have no incremental presentation.
    pub fn list_generation_messages(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        generation_id: &GenerationId,
    ) -> CoreResult<Vec<Message>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT message.id, message.conversation_id, message.parent_id,
                    message.role, message.content, message.status, message.generation_id,
                    message.created_at
             FROM generations AS generation JOIN messages AS message
               ON message.id IN (generation.user_message_id, generation.assistant_message_id)
             WHERE generation.id = ?1 AND generation.conversation_id = ?2
               AND generation.branch_id = ?3 AND generation.status <> 'running'
               AND generation.assistant_message_id IS NOT NULL
             ORDER BY CASE WHEN message.id = generation.user_message_id THEN 0 ELSE 1 END",
            )
            .map_err(storage_db_error)?;
        let messages = statement
            .query_map(
                params![generation_id.0, conversation_id.0, branch_id.0],
                map_message,
            )
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        let [user, assistant] = messages.as_slice() else {
            return Ok(Vec::new());
        };
        if user.role != MessageRole::User
            || user.status != MessageStatus::Complete
            || assistant.role != MessageRole::Assistant
            || assistant.status == MessageStatus::Pending
            || assistant.parent_id.as_ref() != Some(&user.id)
            || assistant.generation_id.as_ref() != Some(generation_id)
            || user.conversation_id != *conversation_id
            || assistant.conversation_id != *conversation_id
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation message ownership is inconsistent",
                false,
            ));
        }
        Ok(messages)
    }
}
