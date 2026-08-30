use chrono::{DateTime, Utc};
use lorepia_domain::{GenerationId, Message, MessageRole, MessageStatus};
use rusqlite::{OptionalExtension, Transaction, params};

use super::finalize::persist_terminal_assistant;
use super::{
    CoreErrorCode, CoreResult, Storage, StoredGenerationRoute, map_message, storage_corrupted,
    storage_db_error, str_to_api_family,
};

pub(crate) struct InterruptedGenerationClosure {
    generation_id: GenerationId,
    route: StoredGenerationRoute,
    assistant: Option<Message>,
    attempt_present: bool,
}

impl InterruptedGenerationClosure {
    fn has_durable_partial_checkpoint(&self) -> bool {
        self.attempt_present
            && self
                .assistant
                .as_ref()
                .is_some_and(|assistant| !assistant.content.is_empty())
    }
}

struct RawInterruptedGenerationClosure {
    generation_id: String,
    conversation: String,
    branch: String,
    user_message: String,
    assistant_message: Option<String>,
    provider_family: Option<String>,
    attempt_present: bool,
}

pub(crate) fn load_interrupted_generation_closures(
    transaction: &Transaction<'_>,
) -> CoreResult<Vec<InterruptedGenerationClosure>> {
    load_raw_interrupted_generation_closures(transaction)?
        .into_iter()
        .map(|raw| validate_interrupted_generation_closure(transaction, raw))
        .collect()
}

fn load_raw_interrupted_generation_closures(
    transaction: &Transaction<'_>,
) -> CoreResult<Vec<RawInterruptedGenerationClosure>> {
    let mut statement = transaction
        .prepare(
            "SELECT generation.id, generation.conversation_id,
                    generation.branch_id, generation.user_message_id,
                    generation.assistant_message_id, generation.provider_family,
                    EXISTS(
                      SELECT 1
                      FROM generation_attempt_intents AS attempt
                      WHERE attempt.generation_id = generation.id
                    )
             FROM generations AS generation
             WHERE generation.status = 'running'
             ORDER BY generation.id",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map([], |row| {
            Ok(RawInterruptedGenerationClosure {
                generation_id: row.get(0)?,
                conversation: row.get(1)?,
                branch: row.get(2)?,
                user_message: row.get(3)?,
                assistant_message: row.get(4)?,
                provider_family: row.get(5)?,
                attempt_present: row.get(6)?,
            })
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn validate_interrupted_generation_closure(
    transaction: &Transaction<'_>,
    raw: RawInterruptedGenerationClosure,
) -> CoreResult<InterruptedGenerationClosure> {
    let generation_id = GenerationId(raw.generation_id);
    let route = StoredGenerationRoute {
        conversation: raw.conversation,
        branch: raw.branch,
        user_message: raw.user_message,
        assistant_message: raw.assistant_message,
        provider_family: raw
            .provider_family
            .map(|value| str_to_api_family(&value))
            .transpose()?,
    };
    let assistant = load_interrupted_generation_assistant(transaction, &route)?;
    if raw.attempt_present {
        validate_interrupted_generation_attempt(
            transaction,
            &generation_id,
            &route,
            assistant.as_ref(),
        )?;
    }
    if let Some(assistant) = assistant.as_ref()
        && (assistant.conversation_id.0 != route.conversation
            || assistant.parent_id.as_ref().map(|id| id.0.as_str())
                != Some(route.user_message.as_str())
            || assistant.role != MessageRole::Assistant
            || assistant.status != MessageStatus::Pending
            || assistant.generation_id.as_ref() != Some(&generation_id))
    {
        return Err(storage_corrupted(
            "running generation assistant route is inconsistent",
        ));
    }
    Ok(InterruptedGenerationClosure {
        generation_id,
        route,
        assistant,
        attempt_present: raw.attempt_present,
    })
}

fn load_interrupted_generation_assistant(
    transaction: &Transaction<'_>,
    route: &StoredGenerationRoute,
) -> CoreResult<Option<Message>> {
    route
        .assistant_message
        .as_deref()
        .map(|assistant_message_id| {
            transaction
                .query_row(
                    "SELECT id, conversation_id, parent_id, role, content, status,
                            generation_id, created_at
                     FROM messages
                     WHERE id = ?1",
                    [assistant_message_id],
                    map_message,
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| storage_corrupted("running generation assistant message is missing"))
        })
        .transpose()
}

fn validate_interrupted_generation_attempt(
    transaction: &Transaction<'_>,
    generation_id: &GenerationId,
    route: &StoredGenerationRoute,
    assistant: Option<&Message>,
) -> CoreResult<()> {
    let assistant = assistant.ok_or_else(|| {
        storage_corrupted("running generation attempt is missing its assistant message route")
    })?;
    let attempt = crate::generation_attempt::read_attempt(transaction, generation_id)?;
    if attempt.status != crate::generation_attempt::GenerationAttemptStatus::Running
        || attempt.input.conversation_id.0 != route.conversation
        || attempt.input.proposed_branch_id.0 != route.branch
    {
        return Err(storage_corrupted(
            "running generation attempt route or status is inconsistent",
        ));
    }
    let exact_head = transaction
        .query_row(
            "SELECT head_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![route.conversation, route.branch],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("running generation branch route is missing"))?;
    if exact_head.as_deref() != Some(assistant.id.0.as_str()) {
        return Err(storage_corrupted(
            "running generation assistant is not the exact branch head",
        ));
    }
    Ok(())
}

pub(crate) fn close_interrupted_generations_in_transaction(
    transaction: &Transaction<'_>,
    interrupted_generations: &[InterruptedGenerationClosure],
    preserve_partial_generations: bool,
    recovered_at: DateTime<Utc>,
    recovered_at_text: &str,
) -> CoreResult<()> {
    close_interrupted_generation_rows(
        transaction,
        interrupted_generations.len(),
        recovered_at_text,
    )?;
    recover_attempt_backed_generation_messages(
        transaction,
        interrupted_generations,
        recovered_at_text,
    )?;
    recover_pending_generation_messages(
        transaction,
        preserve_partial_generations,
        recovered_at_text,
    )?;
    close_interrupted_generation_attempts(
        transaction,
        interrupted_generations,
        recovered_at,
        recovered_at_text,
    )?;
    Ok(())
}

fn recover_attempt_backed_generation_messages(
    transaction: &Transaction<'_>,
    interrupted_generations: &[InterruptedGenerationClosure],
    recovered_at: &str,
) -> CoreResult<()> {
    for interrupted in interrupted_generations
        .iter()
        .filter(|interrupted| interrupted.attempt_present)
    {
        let assistant = interrupted.assistant.as_ref().ok_or_else(|| {
            storage_corrupted("running generation attempt assistant route is missing")
        })?;
        let mut terminal = assistant.clone();
        terminal.status = MessageStatus::Cancelled;
        persist_terminal_assistant(
            transaction,
            &terminal,
            &interrupted.generation_id,
            &interrupted.route,
            recovered_at,
            interrupted.has_durable_partial_checkpoint(),
        )?;
    }
    Ok(())
}

fn close_interrupted_generation_rows(
    transaction: &Transaction<'_>,
    expected_generations: usize,
    recovered_at: &str,
) -> CoreResult<()> {
    let recovered_generations = transaction
        .execute(
            "UPDATE generations
             SET status = 'cancelled',
                 input_tokens = NULL,
                 cached_read_tokens = NULL,
                 cached_write_tokens = NULL,
                 output_tokens = NULL,
                 reasoning_tokens = NULL,
                 tool_tokens = NULL,
                 provider_raw_summary_json = NULL,
                 opaque_reasoning_state_json = NULL,
                 error_code = ?1,
                 finished_at = ?2
             WHERE status = 'running'",
            params![CoreErrorCode::Cancelled.as_str(), recovered_at],
        )
        .map_err(storage_db_error)?;
    if recovered_generations != expected_generations {
        return Err(storage_corrupted(
            "running generation recovery set changed inside its transaction",
        ));
    }
    Ok(())
}

fn recover_pending_generation_messages(
    transaction: &Transaction<'_>,
    preserve_partial_generations: bool,
    recovered_at: &str,
) -> CoreResult<()> {
    if preserve_partial_generations {
        transaction
            .execute(
                "UPDATE messages SET status = 'cancelled' WHERE status = 'pending'",
                [],
            )
            .map_err(storage_db_error)?;
    } else {
        transaction
            .execute(
                "UPDATE conversation_branches
                 SET head_message_id = CASE
                       WHEN head_message_id IN (
                         SELECT id
                         FROM messages
                         WHERE role = 'assistant' AND status = 'pending'
                       )
                       THEN (
                         SELECT parent_id
                         FROM messages
                         WHERE messages.id = conversation_branches.head_message_id
                       )
                       ELSE head_message_id
                     END,
                     fork_message_id = CASE
                       WHEN fork_message_id IN (
                         SELECT id
                         FROM messages
                         WHERE role = 'assistant' AND status = 'pending'
                       )
                       THEN (
                         SELECT parent_id
                         FROM messages
                         WHERE messages.id = conversation_branches.fork_message_id
                       )
                       ELSE fork_message_id
                     END,
                     updated_at = ?1
                 WHERE head_message_id IN (
                         SELECT id
                         FROM messages
                         WHERE role = 'assistant' AND status = 'pending'
                       )
                    OR fork_message_id IN (
                         SELECT id
                         FROM messages
                         WHERE role = 'assistant' AND status = 'pending'
                       )",
                [recovered_at],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "UPDATE messages AS child
                 SET parent_id = (
                   SELECT pending.parent_id
                   FROM messages AS pending
                   WHERE pending.id = child.parent_id
                     AND pending.conversation_id = child.conversation_id
                     AND pending.role = 'assistant'
                     AND pending.status = 'pending'
                 )
                 WHERE child.parent_id IN (
                   SELECT id
                   FROM messages
                   WHERE role = 'assistant' AND status = 'pending'
                 )",
                [],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "DELETE FROM messages WHERE role = 'assistant' AND status = 'pending'",
                [],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn close_interrupted_generation_attempts(
    transaction: &Transaction<'_>,
    interrupted_generations: &[InterruptedGenerationClosure],
    recovered_at: DateTime<Utc>,
    recovered_at_text: &str,
) -> CoreResult<()> {
    for interrupted in interrupted_generations {
        let attempt_present =
            crate::generation_attempt::mark_attempt_completed_if_present_in_transaction(
                transaction,
                &interrupted.generation_id,
                recovered_at,
            )?;
        if attempt_present != interrupted.attempt_present {
            return Err(storage_corrupted(
                "running generation attempt set changed inside its recovery transaction",
            ));
        }
        let updated_conversations = transaction
            .execute(
                "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
                params![interrupted.route.conversation, recovered_at_text],
            )
            .map_err(storage_db_error)?;
        if updated_conversations != 1 {
            return Err(storage_corrupted(
                "running generation conversation route is missing",
            ));
        }
        if attempt_present {
            let assistant = interrupted.assistant.as_ref().ok_or_else(|| {
                storage_corrupted("running generation attempt assistant route is missing")
            })?;
            Storage::insert_generation_terminal_occurrences(
                transaction,
                assistant,
                &interrupted.generation_id,
                &interrupted.route,
                interrupted.has_durable_partial_checkpoint(),
                recovered_at,
            )?;
        }
    }
    Ok(())
}
