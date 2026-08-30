use super::{
    BoundedJson, ConversationBranchId, CoreError, CoreErrorCode, CoreResult, DateTime,
    GenerationId, GenerationUsage, Message, MessageId, MessageRole, MessageStatus,
    OpaqueReasoningState, OptionalExtension, Storage, StoredGenerationRoute, Utc,
    generation_status_to_str, message_status_to_generation_status, params,
    serialize_opaque_reasoning_state_for_family, status_to_str, storage_db_error,
    str_to_api_family_sql, u64_to_i64,
};

impl Storage {
    pub fn finalize_generation(
        &self,
        assistant: &Message,
        usage: Option<&GenerationUsage>,
        error_code: Option<&str>,
        keep_assistant: bool,
    ) -> CoreResult<()> {
        self.finalize_generation_with_protocol_state(
            assistant,
            usage,
            &[],
            error_code,
            keep_assistant,
        )
    }

    pub fn finalize_generation_with_protocol_state(
        &self,
        assistant: &Message,
        usage: Option<&GenerationUsage>,
        opaque_reasoning_state: &[OpaqueReasoningState],
        error_code: Option<&str>,
        keep_assistant: bool,
    ) -> CoreResult<()> {
        self.finalize_generation_with_protocol_state_and_display(
            assistant,
            usage,
            opaque_reasoning_state,
            error_code,
            keep_assistant,
            None,
        )
    }

    /// Atomically finalizes a generation together with its bounded `DisplayOnly`
    /// sidecar and content-free transform application diagnostics.
    pub fn finalize_generation_with_protocol_state_and_display(
        &self,
        assistant: &Message,
        usage: Option<&GenerationUsage>,
        opaque_reasoning_state: &[OpaqueReasoningState],
        error_code: Option<&str>,
        keep_assistant: bool,
        display_projection: Option<
            &crate::message_display_projection::MessageDisplayProjectionWrite,
        >,
    ) -> CoreResult<()> {
        if assistant.role != MessageRole::Assistant || assistant.status == MessageStatus::Pending {
            return Err(CoreError::invalid(
                "only a terminal assistant message can finalize a generation",
            ));
        }
        if assistant.status != MessageStatus::Complete && !opaque_reasoning_state.is_empty() {
            return Err(CoreError::invalid(
                "opaque reasoning state can be stored only for a completed generation",
            ));
        }
        if !keep_assistant && display_projection.is_some() {
            return Err(CoreError::invalid(
                "a discarded assistant message cannot retain a display projection",
            ));
        }
        let generation_id = assistant.generation_id.as_ref().ok_or_else(|| {
            CoreError::invalid("a terminal assistant message requires a generation id")
        })?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let generation = load_running_generation(&transaction, generation_id)?;
        validate_generation_assistant_ownership(&generation, assistant)?;
        let opaque_reasoning_state = serialize_opaque_reasoning_state_for_family(
            generation.provider_family,
            opaque_reasoning_state,
        )?;
        let occurred_at = Utc::now();
        let now = occurred_at.to_rfc3339();
        persist_terminal_assistant(
            &transaction,
            assistant,
            generation_id,
            &generation,
            &now,
            keep_assistant,
        )?;
        if keep_assistant {
            crate::message_display_projection::persist_terminal_message_display_projection(
                &transaction,
                assistant,
                display_projection,
                occurred_at,
            )?;
        }
        Self::update_generation_terminal_row(
            &transaction,
            generation_id,
            assistant.status,
            usage,
            opaque_reasoning_state.as_deref(),
            error_code,
            &now,
        )?;
        crate::generation_attempt::mark_attempt_completed_if_present_in_transaction(
            &transaction,
            generation_id,
            occurred_at,
        )?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![assistant.conversation_id.0, now],
            )
            .map_err(storage_db_error)?;
        Self::insert_generation_terminal_occurrences(
            &transaction,
            assistant,
            generation_id,
            &generation,
            keep_assistant,
            occurred_at,
        )?;
        transaction.commit().map_err(storage_db_error)
    }

    fn update_generation_terminal_row(
        transaction: &rusqlite::Transaction<'_>,
        generation_id: &GenerationId,
        assistant_status: MessageStatus,
        usage: Option<&GenerationUsage>,
        opaque_reasoning_state: Option<&str>,
        error_code: Option<&str>,
        finished_at: &str,
    ) -> CoreResult<()> {
        let token_count = |value: Option<u64>| value.map(u64_to_i64).transpose();
        let input_tokens = token_count(usage.and_then(|usage| usage.input_tokens))?;
        let cached_read_tokens = token_count(usage.and_then(|usage| usage.cached_read_tokens))?;
        let cached_write_tokens = token_count(usage.and_then(|usage| usage.cached_write_tokens))?;
        let output_tokens = token_count(usage.and_then(|usage| usage.output_tokens))?;
        let reasoning_tokens = token_count(usage.and_then(|usage| usage.reasoning_tokens))?;
        let tool_tokens = token_count(usage.and_then(|usage| usage.tool_tokens))?;
        let provider_raw_summary = usage
            .and_then(|usage| usage.provider_raw_summary.as_ref())
            .map(BoundedJson::as_str);
        transaction
            .execute(
                "UPDATE generations
                 SET status = ?2,
                     input_tokens = ?3,
                     cached_read_tokens = ?4,
                     cached_write_tokens = ?5,
                     output_tokens = ?6,
                     reasoning_tokens = ?7,
                     tool_tokens = ?8,
                     provider_raw_summary_json = ?9,
                     opaque_reasoning_state_json = ?10,
                     error_code = ?11,
                     finished_at = ?12
                 WHERE id = ?1 AND status = 'running'",
                params![
                    generation_id.0,
                    generation_status_to_str(message_status_to_generation_status(assistant_status)),
                    input_tokens,
                    cached_read_tokens,
                    cached_write_tokens,
                    output_tokens,
                    reasoning_tokens,
                    tool_tokens,
                    provider_raw_summary,
                    opaque_reasoning_state,
                    error_code,
                    finished_at
                ],
            )
            .map_err(storage_db_error)?;
        Ok(())
    }

    pub(super) fn insert_generation_terminal_occurrences(
        transaction: &rusqlite::Transaction<'_>,
        assistant: &Message,
        generation_id: &GenerationId,
        generation: &StoredGenerationRoute,
        keep_assistant: bool,
        occurred_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let exact_head_message_id = transaction
            .query_row(
                "SELECT head_message_id
                 FROM conversation_branches
                 WHERE conversation_id = ?1 AND id = ?2",
                params![assistant.conversation_id.0, generation.branch],
                |row| row.get::<_, Option<String>>(0),
            )
            .map_err(storage_db_error)?
            .map(MessageId);
        crate::lifecycle_outbox::insert_occurrence(
            transaction,
            &crate::lifecycle_outbox::LifecycleOccurrenceWrite {
                occurrence_id: format!("after-generation:{}", generation_id.0),
                event_kind: crate::lifecycle_outbox::LifecycleOccurrenceKind::AfterGeneration,
                conversation_id: assistant.conversation_id.clone(),
                branch_id: ConversationBranchId(generation.branch.clone()),
                exact_head_message_id: exact_head_message_id.clone(),
                owner_message_id: keep_assistant.then(|| assistant.id.clone()),
                generation_id: Some(generation_id.clone()),
                occurred_at,
            },
            false,
        )?;
        if keep_assistant {
            crate::lifecycle_outbox::insert_occurrence(
                transaction,
                &crate::lifecycle_outbox::LifecycleOccurrenceWrite {
                    occurrence_id: format!("message-committed:{}", assistant.id.0),
                    event_kind: crate::lifecycle_outbox::LifecycleOccurrenceKind::MessageCommitted,
                    conversation_id: assistant.conversation_id.clone(),
                    branch_id: ConversationBranchId(generation.branch.clone()),
                    exact_head_message_id,
                    owner_message_id: Some(assistant.id.clone()),
                    generation_id: Some(generation_id.clone()),
                    occurred_at,
                },
                false,
            )?;
        }
        Ok(())
    }

    /// Marks a generation failed after its normal terminal transaction could not complete.
    ///
    /// This intentionally stores only a stable error code. Provider credentials and raw
    /// persistence errors must never enter the conversation database.
    pub fn fail_generation_after_finalize_error(
        &self,
        assistant: &Message,
        keep_assistant: bool,
    ) -> CoreResult<()> {
        if assistant.role != MessageRole::Assistant || assistant.status != MessageStatus::Failed {
            return Err(CoreError::invalid(
                "only a failed assistant message can compensate a generation finalization",
            ));
        }
        let generation_id = assistant.generation_id.as_ref().ok_or_else(|| {
            CoreError::invalid("a failed assistant message requires a generation id")
        })?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let generation = load_running_generation(&transaction, generation_id)?;
        if generation.conversation != assistant.conversation_id.0
            || generation.assistant_message.as_deref() != Some(assistant.id.0.as_str())
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation assistant ownership is inconsistent",
                false,
            ));
        }
        let occurred_at = Utc::now();
        let now = occurred_at.to_rfc3339();
        compensate_terminal_assistant(
            &transaction,
            assistant,
            generation_id,
            &generation,
            &now,
            keep_assistant,
        )?;
        let changed = transaction
            .execute(
                "UPDATE generations
                SET status = 'failed',
                     input_tokens = NULL,
                     cached_read_tokens = NULL,
                     cached_write_tokens = NULL,
                     output_tokens = NULL,
                     reasoning_tokens = NULL,
                     tool_tokens = NULL,
                     provider_raw_summary_json = NULL,
                     opaque_reasoning_state_json = NULL,
                     error_code = 'storage_unavailable',
                     finished_at = ?2
                 WHERE id = ?1 AND status = 'running'",
                params![generation_id.0, now],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation compensation target was not found",
                false,
            ));
        }
        crate::generation_attempt::mark_attempt_completed_if_present_in_transaction(
            &transaction,
            generation_id,
            occurred_at,
        )?;
        transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![assistant.conversation_id.0, now],
            )
            .map_err(storage_db_error)?;
        Self::insert_generation_terminal_occurrences(
            &transaction,
            assistant,
            generation_id,
            &generation,
            keep_assistant,
            occurred_at,
        )?;
        transaction.commit().map_err(storage_db_error)
    }
}

fn load_running_generation(
    transaction: &rusqlite::Transaction<'_>,
    generation_id: &GenerationId,
) -> CoreResult<StoredGenerationRoute> {
    transaction
        .query_row(
            "SELECT conversation_id, branch_id, user_message_id, assistant_message_id,
                    provider_family
             FROM generations
             WHERE id = ?1 AND status = 'running'",
            [&generation_id.0],
            |row| {
                Ok(StoredGenerationRoute {
                    conversation: row.get(0)?,
                    branch: row.get(1)?,
                    user_message: row.get(2)?,
                    assistant_message: row.get(3)?,
                    provider_family: row
                        .get::<_, Option<String>>(4)?
                        .map(|value| str_to_api_family_sql(&value, 4))
                        .transpose()?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "running generation was not found",
                false,
            )
        })
}

fn validate_generation_assistant_ownership(
    generation: &StoredGenerationRoute,
    assistant: &Message,
) -> CoreResult<()> {
    if generation.conversation != assistant.conversation_id.0
        || generation.assistant_message.as_deref() != Some(assistant.id.0.as_str())
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation assistant ownership is inconsistent",
            false,
        ));
    }
    Ok(())
}

pub(super) fn persist_terminal_assistant(
    transaction: &rusqlite::Transaction<'_>,
    assistant: &Message,
    generation_id: &GenerationId,
    generation: &StoredGenerationRoute,
    finished_at: &str,
    keep_assistant: bool,
) -> CoreResult<()> {
    if keep_assistant {
        let changed = transaction
            .execute(
                "UPDATE messages
                 SET content = ?3, status = ?4
                 WHERE id = ?1
                   AND generation_id = ?2
                   AND role = 'assistant'
                   AND status = 'pending'",
                params![
                    assistant.id.0,
                    generation_id.0,
                    assistant.content,
                    status_to_str(assistant.status)
                ],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            return Ok(());
        }
        return Err(CoreError::new(
            CoreErrorCode::NotFound,
            "pending assistant finalization target was not found",
            false,
        ));
    }
    transaction
        .execute(
            "UPDATE conversation_branches
             SET head_message_id = ?3, updated_at = ?4
             WHERE id = ?1
               AND conversation_id = ?2
               AND head_message_id = ?5",
            params![
                generation.branch,
                generation.conversation,
                generation.user_message,
                finished_at,
                assistant.id.0
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute("DELETE FROM messages WHERE id = ?1", [&assistant.id.0])
        .map_err(storage_db_error)?;
    Ok(())
}

fn compensate_terminal_assistant(
    transaction: &rusqlite::Transaction<'_>,
    assistant: &Message,
    generation_id: &GenerationId,
    generation: &StoredGenerationRoute,
    finished_at: &str,
    keep_assistant: bool,
) -> CoreResult<()> {
    if keep_assistant {
        let changed = transaction
            .execute(
                "UPDATE messages
                 SET content = ?3, status = 'failed'
                 WHERE id = ?1
                   AND generation_id = ?2
                   AND role = 'assistant'
                   AND status = 'pending'",
                params![assistant.id.0, generation_id.0, assistant.content],
            )
            .map_err(storage_db_error)?;
        if changed == 1 {
            return Ok(());
        }
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation assistant compensation target was not found",
            false,
        ));
    }
    let changed = transaction
        .execute(
            "UPDATE conversation_branches
             SET head_message_id = ?3, updated_at = ?4
             WHERE id = ?1
               AND conversation_id = ?2
               AND head_message_id = ?5",
            params![
                generation.branch,
                generation.conversation,
                generation.user_message,
                finished_at,
                assistant.id.0
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation branch compensation target was not found",
            false,
        ));
    }
    let changed = transaction
        .execute(
            "DELETE FROM messages
             WHERE id = ?1
               AND generation_id = ?2
               AND role = 'assistant'
               AND status = 'pending'",
            params![assistant.id.0, generation_id.0],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation assistant compensation target was not found",
            false,
        ));
    }
    Ok(())
}
