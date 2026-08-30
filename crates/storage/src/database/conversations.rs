use super::{
    Conversation, ConversationBranch, ConversationGreetingBinding, ConversationId,
    ConversationMode, ConversationStart, ConversationState, CoreError, CoreErrorCode, CoreResult,
    GenerationId, Message, MessageId, MessageRole, MessageStatus, OptionalExtension, Storage,
    TransactionBehavior, active_character_content_revision, insert_message, mode_to_str, params,
    parse_datetime_sql, resolve_character_greeting, stale_character_greeting_catalog_error,
    storage_db_error, validate_character_greeting_id,
};

impl Storage {
    pub fn save_conversation(&self, conversation: &Conversation) -> CoreResult<()> {
        self.save_conversation_with_mode(conversation, ConversationMode::Chat)
            .map(|_| ())
    }

    pub fn save_conversation_with_mode(
        &self,
        conversation: &Conversation,
        mode: ConversationMode,
    ) -> CoreResult<(ConversationBranch, ConversationState)> {
        let catalog = self.character_greeting_catalog(&conversation.character_id)?;
        self.save_conversation_with_greeting(
            conversation,
            mode,
            catalog.character_content_revision_id.as_deref(),
            None,
        )
        .map(|started| (started.branch, started.state))
    }

    /// Atomically binds a new conversation to an exact character-content
    /// revision and resolves its optional greeting inside the same write
    /// transaction.
    ///
    /// `expected_character_content_revision_id = None` means the caller
    /// observed an exact legacy absence, not "choose whatever is current".
    /// `greeting_id = None` deterministically selects the enabled default
    /// greeting for that exact revision, preserving `first_message`
    /// compatibility. An explicit ID never falls back to another greeting.
    pub fn save_conversation_with_greeting(
        &self,
        conversation: &Conversation,
        mode: ConversationMode,
        expected_character_content_revision_id: Option<&str>,
        greeting_id: Option<&str>,
    ) -> CoreResult<ConversationStart> {
        if let Some(greeting_id) = greeting_id {
            validate_character_greeting_id(greeting_id)?;
        }

        let mut branch = ConversationBranch::root(conversation.id.clone());
        let state = ConversationState {
            conversation_id: conversation.id.clone(),
            active_branch_id: branch.id.clone(),
            selected_mode: mode,
            updated_at: conversation.updated_at,
        };
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let active_revision_id =
            active_character_content_revision(&transaction, &conversation.character_id)?;
        if active_revision_id.as_deref() != expected_character_content_revision_id {
            return Err(stale_character_greeting_catalog_error());
        }
        let selected_greeting =
            resolve_character_greeting(&transaction, active_revision_id.as_deref(), greeting_id)?;
        let initial_message = selected_greeting.as_ref().map(|(_, content)| Message {
            id: MessageId::new(),
            conversation_id: conversation.id.clone(),
            parent_id: None,
            role: MessageRole::Assistant,
            content: content.clone(),
            status: MessageStatus::Complete,
            generation_id: Some(GenerationId::for_character_greeting(&conversation.id)),
            created_at: conversation.created_at,
        });
        branch.head_message_id = initial_message.as_ref().map(|message| message.id.clone());
        insert_conversation_start_rows(
            &transaction,
            conversation,
            &branch,
            &state,
            active_revision_id.as_deref(),
            selected_greeting.as_ref(),
            initial_message.as_ref(),
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(ConversationStart {
            conversation: conversation.clone(),
            branch,
            state,
            initial_message,
            character_content_revision_id: active_revision_id,
            greeting_id: selected_greeting.map(|(id, _)| id),
        })
    }

    pub fn list_conversations(&self) -> CoreResult<Vec<Conversation>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, character_id, title, created_at, updated_at
                 FROM conversations ORDER BY updated_at DESC, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok(Conversation {
                    id: ConversationId(row.get(0)?),
                    character_id: row.get(1)?,
                    title: row.get(2)?,
                    created_at: parse_datetime_sql(row.get::<_, String>(3)?, 3)?,
                    updated_at: parse_datetime_sql(row.get::<_, String>(4)?, 4)?,
                })
            })
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_conversation_greeting_binding(
        &self,
        conversation_id: &ConversationId,
    ) -> CoreResult<ConversationGreetingBinding> {
        self.connection()?
            .query_row(
                "SELECT conversation_id, character_content_revision_id,
                        greeting_id, created_at
                 FROM conversation_greeting_bindings
                 WHERE conversation_id = ?1",
                [&conversation_id.0],
                |row| {
                    Ok(ConversationGreetingBinding {
                        conversation_id: ConversationId(row.get(0)?),
                        character_content_revision_id: row.get(1)?,
                        greeting_id: row.get(2)?,
                        created_at: parse_datetime_sql(row.get::<_, String>(3)?, 3)?,
                    })
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "conversation greeting binding was not found",
                    false,
                )
            })
    }

    pub fn list_conversations_for_character(
        &self,
        character_id: &str,
    ) -> CoreResult<Vec<Conversation>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, character_id, title, created_at, updated_at
                 FROM conversations
                 WHERE character_id = ?1
                 ORDER BY updated_at DESC, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([character_id], map_conversation)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_conversation(&self, id: &ConversationId) -> CoreResult<Conversation> {
        self.connection()?
            .query_row(
                "SELECT id, character_id, title, created_at, updated_at
                 FROM conversations WHERE id = ?1",
                [&id.0],
                map_conversation,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(CoreErrorCode::NotFound, "conversation was not found", false)
            })
    }
}

fn insert_conversation_start_rows(
    transaction: &rusqlite::Transaction<'_>,
    conversation: &Conversation,
    branch: &ConversationBranch,
    state: &ConversationState,
    active_revision_id: Option<&str>,
    selected_greeting: Option<&(String, String)>,
    initial_message: Option<&Message>,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO conversations
             (id, character_id, title, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                conversation.id.0,
                conversation.character_id,
                conversation.title,
                conversation.created_at.to_rfc3339(),
                conversation.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO conversation_greeting_bindings
             (conversation_id, character_content_revision_id, greeting_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                conversation.id.0,
                active_revision_id,
                selected_greeting.map(|(greeting_id, _)| greeting_id.as_str()),
                conversation.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    if let Some(initial_message) = initial_message {
        insert_message(transaction, initial_message)?;
    }
    transaction
        .execute(
            "INSERT INTO conversation_branches
             (id, conversation_id, title, fork_message_id, head_message_id,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)",
            params![
                branch.id.0,
                branch.conversation_id.0,
                branch.title,
                branch
                    .head_message_id
                    .as_ref()
                    .map(|message_id| message_id.0.as_str()),
                branch.created_at.to_rfc3339(),
                branch.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "INSERT INTO conversation_state
             (conversation_id, active_branch_id, selected_mode, updated_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                state.conversation_id.0,
                state.active_branch_id.0,
                mode_to_str(state.selected_mode),
                state.updated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)?;
    crate::lifecycle_outbox::insert_occurrence(
        transaction,
        &crate::lifecycle_outbox::LifecycleOccurrenceWrite {
            occurrence_id: format!("conversation-started:{}", conversation.id.0),
            event_kind: crate::lifecycle_outbox::LifecycleOccurrenceKind::ConversationStarted,
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
            exact_head_message_id: branch.head_message_id.clone(),
            owner_message_id: None,
            generation_id: None,
            occurred_at: conversation.created_at,
        },
        false,
    )?;
    Ok(())
}
fn map_conversation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Conversation> {
    Ok(Conversation {
        id: ConversationId(row.get(0)?),
        character_id: row.get(1)?,
        title: row.get(2)?,
        created_at: parse_datetime_sql(row.get::<_, String>(3)?, 3)?,
        updated_at: parse_datetime_sql(row.get::<_, String>(4)?, 4)?,
    })
}
