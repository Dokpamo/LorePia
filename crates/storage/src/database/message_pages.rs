use super::lineage_cache::LineageSnapshot;
mod last_assistant;
use crate::message_display_projection::{
    StoredMessageDisplayProjection, read_page_projections, verify_batch,
};
use last_assistant::load_last_assistant;

use super::{
    Connection, ConversationBranchId, CoreError, CoreResult, Message, MessageId, MessageRole,
    OptionalExtension, Storage, TransactionBehavior, map_message, not_found, params,
    storage_corrupted, storage_db_error,
};

/// A bounded chronological window from one branch's current head snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchMessagePage {
    pub messages: Vec<Message>,
    /// Equality-only evidence for bodies and projections from this read snapshot.
    pub snapshot_token: String,
    pub display_projections: Vec<StoredMessageDisplayProjection>,
    pub has_older: bool,
    pub has_newer: bool,
    pub head_message_id: Option<MessageId>,
    pub total_messages: u64,
    pub start_index: u64,
    pub retained_message_ids: Option<Vec<MessageId>>,
    /// Latest branch assistant only when it is outside this page.
    pub last_assistant_message: Option<Message>,
}

impl Storage {
    /// Loads the latest page, or the nearest page exclusively before/after an anchor.
    /// An anchor must still belong to the selected branch's current lineage.
    pub fn list_branch_messages_page(
        &self,
        branch_id: &ConversationBranchId,
        before: Option<&MessageId>,
        after: Option<&MessageId>,
        limit: u32,
        check_message_ids: Option<&[MessageId]>,
        include_last_assistant: bool,
    ) -> CoreResult<BranchMessagePage> {
        if !(1..=128).contains(&limit) || (before.is_some() && after.is_some()) {
            return Err(CoreError::invalid(
                "message page requires a limit between 1 and 128 and at most one anchor",
            ));
        }
        validate_membership_candidates(check_message_ids)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(storage_db_error)?;
        let (conversation_id, head) = transaction
            .query_row(
                "SELECT conversation_id, head_message_id FROM conversation_branches WHERE id = ?1",
                [&branch_id.0],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("conversation branch"))?;
        let lineage = self
            .lineage_cache
            .lock()
            .map_err(|_| CoreError::internal("lineage cache lock was poisoned"))?
            .load(
                &transaction,
                &conversation_id,
                head.as_deref(),
                self.change_tracking.lineage_epoch(),
            )?;
        let anchor = before
            .or(after)
            .map(|anchor| {
                lineage.position(&anchor.0).ok_or_else(|| {
                    CoreError::invalid("message page anchor is not in the current branch lineage")
                })
            })
            .transpose()?;
        let limit = usize::try_from(limit)
            .map_err(|_| CoreError::invalid("message page limit exceeds the platform range"))?;
        let (start, end) = if let (Some(end), Some(_)) = (anchor, after) {
            (end.saturating_sub(limit), end)
        } else {
            let start = anchor.map_or(0, |index| index + 1);
            (start, start.saturating_add(limit).min(lineage.len()))
        };
        let selected = lineage[start..end].iter().rev().collect::<Vec<_>>();
        let messages = load_selected_messages(&transaction, &conversation_id, &selected)?;
        let last_assistant_message = if include_last_assistant {
            load_last_assistant(&transaction, &conversation_id, &lineage, &messages, start)?
        } else {
            None
        };
        let projection_rows = read_page_projections(
            &transaction,
            messages.iter().chain(last_assistant_message.iter()),
        )?;
        let snapshot_token = self
            .change_tracking
            .history_snapshot_token(&transaction, &branch_id.0)?;
        transaction.commit().map_err(storage_db_error)?;
        drop(connection);
        let display_projections = verify_batch(projection_rows)?;
        Ok(BranchMessagePage {
            snapshot_token,
            display_projections,
            last_assistant_message,
            retained_message_ids: check_message_ids.map(|ids| retained_candidates(&lineage, ids)),
            messages,
            has_older: end < lineage.len(),
            has_newer: start > 0,
            head_message_id: head.map(MessageId),
            total_messages: u64::try_from(lineage.len())
                .map_err(|_| CoreError::internal("message count exceeds supported range"))?,
            start_index: u64::try_from(lineage.len() - end)
                .map_err(|_| CoreError::internal("message offset exceeds supported range"))?,
        })
    }
}

fn load_selected_messages(
    connection: &Connection,
    conversation_id: &str,
    ids: &[&String],
) -> CoreResult<Vec<Message>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let json = serde_json::to_string(ids).map_err(|error| {
        CoreError::internal(format!("cannot encode message page identities: {error}"))
    })?;
    let mut statement = connection
        .prepare_cached(
            "SELECT message.id, message.conversation_id, message.parent_id, message.role,
                message.content, message.status, message.generation_id, message.created_at
         FROM json_each(?1) AS selected
         JOIN messages_with_checkpoints AS message ON message.id = selected.value
         WHERE message.conversation_id = ?2
         ORDER BY CAST(selected.key AS INTEGER)",
        )
        .map_err(storage_db_error)?;
    let messages = statement
        .query_map(params![json, conversation_id], map_message)
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if messages.len() != ids.len() {
        return Err(storage_corrupted(
            "message page snapshot lost a selected message",
        ));
    }
    Ok(messages)
}

fn validate_membership_candidates(ids: Option<&[MessageId]>) -> CoreResult<()> {
    if let Some(ids) = ids {
        if ids.len() > 256 {
            return Err(CoreError::invalid(
                "message membership check exceeds 256 candidates",
            ));
        }
        for id in ids {
            if id.0.is_empty()
                || id.0.len() > 512
                || id.0.chars().count() > 256
                || id.0.chars().any(char::is_control)
            {
                return Err(CoreError::invalid(
                    "message membership candidate is not a bounded identifier",
                ));
            }
        }
    }
    Ok(())
}

fn retained_candidates(lineage: &LineageSnapshot, ids: &[MessageId]) -> Vec<MessageId> {
    ids.iter()
        .filter(|id| lineage.position(&id.0).is_some())
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
