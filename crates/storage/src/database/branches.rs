use super::{
    ConversationBranch, ConversationBranchId, ConversationId, ConversationMode, ConversationState,
    CoreError, CoreErrorCode, CoreResult, MessageId, OptionalExtension, Storage,
    TransactionBehavior, Utc, clone_interaction_checkpoint_for_branch_transaction,
    interaction_state_key_for_branch, invalid_enum, params, parse_datetime_sql, storage_db_error,
};

impl Storage {
    pub fn get_conversation_state(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<ConversationState> {
        self.connection()?
            .query_row(
                "SELECT conversation_id, active_branch_id, selected_mode, updated_at
                 FROM conversation_state
                 WHERE conversation_id = ?1",
                [&conversation_id.0],
                map_conversation_state,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "conversation state was not found",
                    false,
                )
            })
    }

    pub fn list_conversation_branches(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<Vec<ConversationBranch>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, conversation_id, title, fork_message_id, head_message_id,
                        created_at, updated_at
                 FROM conversation_branches
                 WHERE conversation_id = ?1
                 ORDER BY updated_at DESC, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([&conversation_id.0], map_conversation_branch)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_conversation_branch(
        &self,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ConversationBranch> {
        self.connection()?
            .query_row(
                "SELECT id, conversation_id, title, fork_message_id, head_message_id,
                        created_at, updated_at
                 FROM conversation_branches
                 WHERE id = ?1",
                [&branch_id.0],
                map_conversation_branch,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "conversation branch was not found",
                    false,
                )
            })
    }

    pub fn create_conversation_branch(
        &self,
        conversation_id: &ConversationId,
        from_message_id: Option<&MessageId>,
        title: Option<String>,
    ) -> CoreResult<ConversationBranch> {
        let branch = ConversationBranch {
            id: ConversationBranchId::new(),
            conversation_id: conversation_id.clone(),
            title,
            fork_message_id: from_message_id.cloned(),
            head_message_id: from_message_id.cloned(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let source_branch_id = transaction
            .query_row(
                "SELECT active_branch_id
                 FROM conversation_state
                 WHERE conversation_id = ?1",
                [&conversation_id.0],
                |row| row.get::<_, String>(0).map(ConversationBranchId),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "conversation was not found", false)
            })?;
        if let Some(message_id) = from_message_id {
            let exists = transaction
                .query_row(
                    "SELECT EXISTS(
                       SELECT 1 FROM messages
                       WHERE id = ?1 AND conversation_id = ?2 AND status <> 'pending'
                     )",
                    params![message_id.0, conversation_id.0],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if !exists {
                return Err(CoreError::new(
                    CoreErrorCode::NotFound,
                    "branch source message was not found in the conversation",
                    false,
                ));
            }
        }
        transaction
            .execute(
                "INSERT INTO conversation_branches
                 (id, conversation_id, title, fork_message_id, head_message_id,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    branch.id.0,
                    branch.conversation_id.0,
                    branch.title,
                    branch
                        .fork_message_id
                        .as_ref()
                        .map(|message_id| message_id.0.as_str()),
                    branch
                        .head_message_id
                        .as_ref()
                        .map(|message_id| message_id.0.as_str()),
                    branch.created_at.to_rfc3339(),
                    branch.updated_at.to_rfc3339()
                ],
            )
            .map_err(storage_db_error)?;
        let target_key = interaction_state_key_for_branch(conversation_id, &branch.id)?;
        clone_interaction_checkpoint_for_branch_transaction(
            &transaction,
            conversation_id,
            &source_branch_id,
            from_message_id,
            &target_key,
            branch.created_at,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(branch)
    }

    pub fn select_conversation_branch(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<ConversationState> {
        let now = Utc::now();
        let changed = self
            .connection()?
            .execute(
                "UPDATE conversation_state
                 SET active_branch_id = ?2, updated_at = ?3
                 WHERE conversation_id = ?1
                   AND EXISTS(
                     SELECT 1 FROM conversation_branches
                     WHERE conversation_id = ?1 AND id = ?2
                   )",
                params![conversation_id.0, branch_id.0, now.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "conversation branch was not found in the conversation",
                false,
            ));
        }
        self.get_conversation_state(conversation_id)
    }

    pub fn set_conversation_mode(
        &self,
        conversation_id: &ConversationId,
        mode: ConversationMode,
    ) -> CoreResult<ConversationState> {
        let now = Utc::now();
        let changed = self
            .connection()?
            .execute(
                "UPDATE conversation_state
                 SET selected_mode = ?2, updated_at = ?3
                 WHERE conversation_id = ?1",
                params![conversation_id.0, mode_to_str(mode), now.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "conversation state was not found",
                false,
            ));
        }
        self.get_conversation_state(conversation_id)
    }
}

pub(super) fn map_conversation_branch(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ConversationBranch> {
    Ok(ConversationBranch {
        id: ConversationBranchId(row.get(0)?),
        conversation_id: ConversationId(row.get(1)?),
        title: row.get(2)?,
        fork_message_id: row.get::<_, Option<String>>(3)?.map(MessageId),
        head_message_id: row.get::<_, Option<String>>(4)?.map(MessageId),
        created_at: parse_datetime_sql(row.get::<_, String>(5)?, 5)?,
        updated_at: parse_datetime_sql(row.get::<_, String>(6)?, 6)?,
    })
}

fn map_conversation_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationState> {
    let mode = row.get::<_, String>(2)?;
    Ok(ConversationState {
        conversation_id: ConversationId(row.get(0)?),
        active_branch_id: ConversationBranchId(row.get(1)?),
        selected_mode: str_to_mode(&mode, 2)?,
        updated_at: parse_datetime_sql(row.get::<_, String>(3)?, 3)?,
    })
}
pub(super) const fn mode_to_str(mode: ConversationMode) -> &'static str {
    match mode {
        ConversationMode::Chat => "chat",
        ConversationMode::Story => "story",
    }
}

pub(super) fn str_to_mode(value: &str, column: usize) -> rusqlite::Result<ConversationMode> {
    match value {
        "chat" => Ok(ConversationMode::Chat),
        "story" => Ok(ConversationMode::Story),
        other => Err(invalid_enum(column, "conversation mode", other)),
    }
}
