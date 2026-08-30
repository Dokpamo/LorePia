use super::{
    Connection, ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult,
    GenerationId, Message, MessageId, MessageRole, MessageStatus, OptionalExtension, Storage,
    TransactionBehavior, Utc, invalid_enum, params, parse_datetime_sql, stale_branch_error,
    storage_db_error,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageGenerationAction {
    EditUser,
    RegenerateAssistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageGenerationActionContext {
    pub fork_message_id: Option<MessageId>,
    pub user_text: String,
}

impl Storage {
    pub fn save_message(&self, message: &Message) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let changed = transaction
            .execute(
                "INSERT INTO messages
                 (id, conversation_id, parent_id, role, content, status, generation_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                   content = excluded.content,
                   status = excluded.status
                 WHERE messages.conversation_id = excluded.conversation_id
                   AND messages.parent_id IS excluded.parent_id
                   AND messages.role = excluded.role
                   AND messages.generation_id IS excluded.generation_id
                   AND messages.created_at = excluded.created_at",
                params![
                    message.id.0,
                    message.conversation_id.0,
                    message.parent_id.as_ref().map(|value| value.0.as_str()),
                    role_to_str(message.role),
                    message.content,
                    status_to_str(message.status),
                    message.generation_id.as_ref().map(|value| value.0.as_str()),
                    message.created_at.to_rfc3339()
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "message identity fields cannot be replaced",
                false,
            ));
        }
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![message.conversation_id.0, Utc::now().to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(())
    }

    /// Updates only the content of the matching in-flight assistant row.
    ///
    /// This conditional update prevents a delayed streaming checkpoint from
    /// replacing a terminal message or a row owned by another generation.
    pub fn checkpoint_pending_assistant(&self, message: &Message) -> CoreResult<()> {
        if message.role != MessageRole::Assistant || message.status != MessageStatus::Pending {
            return Err(CoreError::invalid(
                "only a pending assistant message can be checkpointed",
            ));
        }
        let generation_id = message.generation_id.as_ref().ok_or_else(|| {
            CoreError::invalid("a pending assistant checkpoint requires a generation id")
        })?;
        let changed = self
            .connection()?
            .execute(
                "UPDATE messages
                 SET content = ?3
                 WHERE id = ?1
                   AND generation_id = ?2
                   AND role = 'assistant'
                   AND status = 'pending'",
                params![message.id.0, generation_id.0, message.content],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(CoreError::new(
                CoreErrorCode::NotFound,
                "pending assistant checkpoint target was not found",
                false,
            ))
        }
    }

    pub fn delete_message(&self, id: &MessageId) -> CoreResult<()> {
        self.connection()?
            .execute("DELETE FROM messages WHERE id = ?1", [&id.0])
            .map_err(storage_db_error)?;
        Ok(())
    }

    pub fn list_messages(&self, conversation_id: &ConversationId) -> CoreResult<Vec<Message>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM messages WHERE conversation_id = ?1
                 ORDER BY created_at, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([&conversation_id.0], map_message)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn list_branch_messages(
        &self,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Vec<Message>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH RECURSIVE lineage(
                   id, conversation_id, parent_id, role, content, status,
                   generation_id, created_at, depth
                 ) AS (
                   SELECT messages.id, messages.conversation_id, messages.parent_id,
                          messages.role, messages.content, messages.status,
                          messages.generation_id, messages.created_at, 0
                   FROM conversation_branches
                   JOIN messages
                     ON messages.conversation_id = conversation_branches.conversation_id
                    AND messages.id = conversation_branches.head_message_id
                   WHERE conversation_branches.id = ?1
                   UNION ALL
                   SELECT parent.id, parent.conversation_id, parent.parent_id,
                          parent.role, parent.content, parent.status,
                          parent.generation_id, parent.created_at, lineage.depth + 1
                   FROM messages AS parent
                   JOIN lineage
                     ON parent.conversation_id = lineage.conversation_id
                    AND parent.id = lineage.parent_id
                 )
                 SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM lineage
                 ORDER BY depth DESC",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([&branch_id.0], map_message)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn prepare_message_generation_action(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        expected_head: Option<&MessageId>,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
    ) -> CoreResult<MessageGenerationActionContext> {
        let connection = self.connection()?;
        load_message_generation_action_context(
            &connection,
            conversation_id,
            branch_id,
            expected_head,
            target_message_id,
            action,
        )
    }

    /// Loads the immutable message context needed to derive a generation-action identity.
    ///
    /// This does not authorize a new action against the live branch snapshot. Callers must
    /// either resolve an exact durable operation replay or call
    /// [`Self::prepare_message_generation_action`] before creating a new attempt.
    pub fn load_message_generation_action_identity_context(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        target_message_id: &MessageId,
        action: MessageGenerationAction,
    ) -> CoreResult<MessageGenerationActionContext> {
        let connection = self.connection()?;
        load_message_generation_action_identity_context(
            &connection,
            conversation_id,
            branch_id,
            target_message_id,
            action,
        )
    }

    pub fn list_recent_message_lineage_for_prompt(
        &self,
        conversation_id: &ConversationId,
        head_message_id: Option<&MessageId>,
        max_messages: usize,
        max_message_bytes: usize,
        max_message_chars: usize,
    ) -> CoreResult<Vec<Message>> {
        if head_message_id.is_none()
            || max_messages == 0
            || max_message_bytes == 0
            || max_message_chars == 0
        {
            return Ok(Vec::new());
        }
        let max_messages = i64::try_from(max_messages)
            .map_err(|_| CoreError::invalid("message limit exceeds SQLite integer range"))?;
        let max_message_bytes = i64::try_from(max_message_bytes)
            .map_err(|_| CoreError::invalid("byte limit exceeds SQLite integer range"))?;
        let max_message_chars = i64::try_from(max_message_chars)
            .map_err(|_| CoreError::invalid("character limit exceeds SQLite integer range"))?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH RECURSIVE lineage(
                   id, conversation_id, parent_id, role, content, status,
                   generation_id, created_at, depth
                 ) AS (
                   SELECT id, conversation_id, parent_id, role, content, status,
                          generation_id, created_at, 0
                   FROM messages
                   WHERE conversation_id = ?1 AND id = ?2
                   UNION ALL
                   SELECT parent.id, parent.conversation_id, parent.parent_id,
                          parent.role, parent.content, parent.status,
                          parent.generation_id, parent.created_at, lineage.depth + 1
                   FROM messages AS parent
                   JOIN lineage
                     ON parent.conversation_id = lineage.conversation_id
                    AND parent.id = lineage.parent_id
                   WHERE lineage.depth < 511
                 ),
                 selected AS (
                   SELECT *
                   FROM lineage
                   WHERE role != 'system'
                     AND status != 'pending'
                     AND (status = 'complete' OR length(content) > 0)
                     AND length(CAST(content AS BLOB)) <= ?4
                     AND length(content) <= ?5
                   ORDER BY depth
                   LIMIT ?3
                 )
                 SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM selected
                 ORDER BY depth DESC",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map(
                params![
                    conversation_id.0,
                    head_message_id.map(|message_id| message_id.0.as_str()),
                    max_messages,
                    max_message_bytes,
                    max_message_chars
                ],
                map_message,
            )
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    /// Loads the newest eligible suffix from one selected message lineage.
    pub fn list_recent_branch_messages_for_prompt(
        &self,
        branch_id: &ConversationBranchId,
        max_messages: usize,
        max_message_bytes: usize,
        max_message_chars: usize,
    ) -> CoreResult<Vec<Message>> {
        if max_messages == 0 || max_message_bytes == 0 || max_message_chars == 0 {
            return Ok(Vec::new());
        }
        let max_messages = i64::try_from(max_messages)
            .map_err(|_| CoreError::invalid("message limit exceeds SQLite integer range"))?;
        let max_message_bytes = i64::try_from(max_message_bytes)
            .map_err(|_| CoreError::invalid("byte limit exceeds SQLite integer range"))?;
        let max_message_chars = i64::try_from(max_message_chars)
            .map_err(|_| CoreError::invalid("character limit exceeds SQLite integer range"))?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH RECURSIVE lineage(
                   id, conversation_id, parent_id, role, content, status,
                   generation_id, created_at, depth
                 ) AS (
                   SELECT messages.id, messages.conversation_id, messages.parent_id,
                          messages.role, messages.content, messages.status,
                          messages.generation_id, messages.created_at, 0
                   FROM conversation_branches
                   JOIN messages
                     ON messages.conversation_id = conversation_branches.conversation_id
                    AND messages.id = conversation_branches.head_message_id
                   WHERE conversation_branches.id = ?1
                   UNION ALL
                   SELECT parent.id, parent.conversation_id, parent.parent_id,
                          parent.role, parent.content, parent.status,
                          parent.generation_id, parent.created_at, lineage.depth + 1
                   FROM messages AS parent
                   JOIN lineage
                     ON parent.conversation_id = lineage.conversation_id
                    AND parent.id = lineage.parent_id
                   WHERE lineage.depth < 511
                 ),
                 selected AS (
                   SELECT *
                   FROM lineage
                   WHERE role != 'system'
                     AND status != 'pending'
                     AND (status = 'complete' OR length(content) > 0)
                     AND length(CAST(content AS BLOB)) <= ?3
                     AND length(content) <= ?4
                   ORDER BY depth
                   LIMIT ?2
                 )
                 SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM selected
                 ORDER BY depth DESC",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map(
                params![
                    branch_id.0,
                    max_messages,
                    max_message_bytes,
                    max_message_chars
                ],
                map_message,
            )
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }
    /// Loads a bounded recent suffix without materializing oversized legacy rows.
    pub fn list_recent_messages_for_prompt(
        &self,
        conversation_id: &ConversationId,
        max_messages: usize,
        max_message_bytes: usize,
        max_message_chars: usize,
    ) -> CoreResult<Vec<Message>> {
        if max_messages == 0 || max_message_bytes == 0 || max_message_chars == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, conversation_id, parent_id, role, content, status,
                        generation_id, created_at
                 FROM (
                   SELECT id, conversation_id, parent_id, role, content, status,
                          generation_id, created_at
                   FROM messages
                   WHERE conversation_id = ?1
                     AND role != 'system'
                     AND length(CAST(content AS BLOB)) <= ?3
                     AND length(content) <= ?4
                   ORDER BY created_at DESC, id DESC
                   LIMIT ?2
                 )
                 ORDER BY created_at, id",
            )
            .map_err(storage_db_error)?;
        let max_messages = i64::try_from(max_messages)
            .map_err(|_| CoreError::invalid("message limit exceeds SQLite integer range"))?;
        let max_message_bytes = i64::try_from(max_message_bytes)
            .map_err(|_| CoreError::invalid("byte limit exceeds SQLite integer range"))?;
        let max_message_chars = i64::try_from(max_message_chars)
            .map_err(|_| CoreError::invalid("character limit exceeds SQLite integer range"))?;
        let rows = statement
            .query_map(
                params![
                    conversation_id.0,
                    max_messages,
                    max_message_bytes,
                    max_message_chars
                ],
                map_message,
            )
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }
}

pub(super) fn load_message_generation_action_context(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    target_message_id: &MessageId,
    action: MessageGenerationAction,
) -> CoreResult<MessageGenerationActionContext> {
    let target = load_branch_action_target(
        connection,
        conversation_id,
        branch_id,
        expected_head,
        target_message_id,
    )?;
    message_generation_action_context_from_target(connection, conversation_id, target, action)
}

fn load_message_generation_action_identity_context(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    target_message_id: &MessageId,
    action: MessageGenerationAction,
) -> CoreResult<MessageGenerationActionContext> {
    let branch_conversation_id = connection
        .query_row(
            "SELECT conversation_id FROM conversation_branches WHERE id = ?1",
            [&branch_id.0],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found",
                false,
            )
        })?;
    if branch_conversation_id != conversation_id.0 {
        return Err(CoreError::new(
            CoreErrorCode::NotFound,
            "conversation branch was not found in the conversation",
            false,
        ));
    }
    let target = connection
        .query_row(
            "SELECT id, conversation_id, parent_id, role, content, status,
                    generation_id, created_at
             FROM messages
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0, target_message_id.0],
            map_message,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "message was not found in the conversation",
                false,
            )
        })?;
    message_generation_action_context_from_target(connection, conversation_id, target, action)
}

fn message_generation_action_context_from_target(
    connection: &Connection,
    conversation_id: &ConversationId,
    target: Message,
    action: MessageGenerationAction,
) -> CoreResult<MessageGenerationActionContext> {
    match action {
        MessageGenerationAction::EditUser => {
            if target.role != MessageRole::User || target.status != MessageStatus::Complete {
                return Err(CoreError::invalid(
                    "only a complete user message can be edited",
                ));
            }
            Ok(MessageGenerationActionContext {
                fork_message_id: target.parent_id,
                user_text: target.content,
            })
        }
        MessageGenerationAction::RegenerateAssistant => {
            if target.role != MessageRole::Assistant {
                return Err(CoreError::invalid(
                    "only an assistant message can be regenerated",
                ));
            }
            if target.status == MessageStatus::Pending {
                return Err(active_generation_action_error());
            }
            let user_message_id = target.parent_id.ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "assistant message is missing its user parent",
                    false,
                )
            })?;
            let user = connection
                .query_row(
                    "SELECT id, conversation_id, parent_id, role, content, status,
                            generation_id, created_at
                     FROM messages
                     WHERE conversation_id = ?1 AND id = ?2",
                    params![conversation_id.0, user_message_id.0],
                    map_message,
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "assistant message user parent was not found",
                        false,
                    )
                })?;
            if user.role != MessageRole::User || user.status != MessageStatus::Complete {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "assistant message parent is not a complete user message",
                    false,
                ));
            }
            Ok(MessageGenerationActionContext {
                fork_message_id: user.parent_id,
                user_text: user.content,
            })
        }
    }
}

pub(super) fn load_branch_action_target(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
    target_message_id: &MessageId,
) -> CoreResult<Message> {
    validate_branch_action_snapshot(connection, conversation_id, branch_id, expected_head)?;

    connection
        .query_row(
            "WITH RECURSIVE lineage(
               id, conversation_id, parent_id, role, content, status,
               generation_id, created_at
             ) AS (
               SELECT messages.id, messages.conversation_id, messages.parent_id,
                      messages.role, messages.content, messages.status,
                      messages.generation_id, messages.created_at
               FROM conversation_branches
               JOIN messages
                 ON messages.conversation_id = conversation_branches.conversation_id
                AND messages.id = conversation_branches.head_message_id
               WHERE conversation_branches.id = ?1
               UNION
               SELECT parent.id, parent.conversation_id, parent.parent_id,
                      parent.role, parent.content, parent.status,
                      parent.generation_id, parent.created_at
               FROM messages AS parent
               JOIN lineage
                 ON parent.conversation_id = lineage.conversation_id
                AND parent.id = lineage.parent_id
             )
             SELECT id, conversation_id, parent_id, role, content, status,
                    generation_id, created_at
             FROM lineage
             WHERE id = ?2
             LIMIT 1",
            params![branch_id.0, target_message_id.0],
            map_message,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "message was not found in the selected branch",
                false,
            )
        })
}

fn validate_branch_action_snapshot(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    expected_head: Option<&MessageId>,
) -> CoreResult<()> {
    let branch = connection
        .query_row(
            "SELECT branches.conversation_id, branches.head_message_id,
                    state.active_branch_id
             FROM conversation_branches AS branches
             JOIN conversation_state AS state
               ON state.conversation_id = branches.conversation_id
             WHERE branches.id = ?1",
            [&branch_id.0],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found",
                false,
            )
        })?;
    if branch.0 != conversation_id.0 {
        return Err(CoreError::new(
            CoreErrorCode::NotFound,
            "conversation branch was not found in the conversation",
            false,
        ));
    }
    if branch.1.as_deref() != expected_head.map(|message_id| message_id.0.as_str())
        || branch.2 != branch_id.0
    {
        return Err(stale_branch_error());
    }
    if let Some(head_message_id) = branch.1.as_deref() {
        let status = connection
            .query_row(
                "SELECT status
                 FROM messages
                 WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id.0, head_message_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "conversation branch head was not found",
                    false,
                )
            })?;
        if str_to_status(&status, 0).map_err(storage_db_error)? == MessageStatus::Pending {
            return Err(active_generation_action_error());
        }
    }
    Ok(())
}

pub(super) fn active_generation_action_error() -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        "message actions are unavailable while the branch is generating",
        true,
    )
}

pub(super) fn insert_message(
    transaction: &rusqlite::Transaction<'_>,
    message: &Message,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO messages
             (id, conversation_id, parent_id, role, content, status, generation_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                message.id.0,
                message.conversation_id.0,
                message.parent_id.as_ref().map(|value| value.0.as_str()),
                role_to_str(message.role),
                message.content,
                status_to_str(message.status),
                message.generation_id.as_ref().map(|value| value.0.as_str()),
                message.created_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}
pub(super) fn map_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let role: String = row.get(3)?;
    let status: String = row.get(5)?;
    Ok(Message {
        id: MessageId(row.get(0)?),
        conversation_id: ConversationId(row.get(1)?),
        parent_id: row.get::<_, Option<String>>(2)?.map(MessageId),
        role: str_to_role(&role, 3)?,
        content: row.get(4)?,
        status: str_to_status(&status, 5)?,
        generation_id: row.get::<_, Option<String>>(6)?.map(GenerationId),
        created_at: parse_datetime_sql(row.get::<_, String>(7)?, 7)?,
    })
}
fn role_to_str(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
    }
}

fn str_to_role(value: &str, column: usize) -> rusqlite::Result<MessageRole> {
    match value {
        "system" => Ok(MessageRole::System),
        "user" => Ok(MessageRole::User),
        "assistant" => Ok(MessageRole::Assistant),
        other => Err(invalid_enum(column, "message role", other)),
    }
}

pub(super) fn status_to_str(status: MessageStatus) -> &'static str {
    match status {
        MessageStatus::Pending => "pending",
        MessageStatus::Complete => "complete",
        MessageStatus::Cancelled => "cancelled",
        MessageStatus::Failed => "failed",
    }
}
pub(super) fn str_to_status(value: &str, column: usize) -> rusqlite::Result<MessageStatus> {
    match value {
        "pending" => Ok(MessageStatus::Pending),
        "complete" => Ok(MessageStatus::Complete),
        "cancelled" => Ok(MessageStatus::Cancelled),
        "failed" => Ok(MessageStatus::Failed),
        other => Err(invalid_enum(column, "message status", other)),
    }
}
