use std::collections::{HashMap, HashSet};

use super::{
    Connection, ConversationBranchId, CoreError, CoreResult, Message, MessageId, MessageRole,
    OptionalExtension, Storage, TransactionBehavior, map_message, not_found, params,
    storage_corrupted, storage_db_error,
};

/// A bounded chronological window from one branch's current head snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchMessagePage {
    pub messages: Vec<Message>,
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
        let lineage = load_lineage_identities(&transaction, &conversation_id, head.as_deref())?;
        let anchor = before
            .or(after)
            .map(|anchor| {
                lineage
                    .iter()
                    .position(|id| *id == anchor.0)
                    .ok_or_else(|| {
                        CoreError::invalid(
                            "message page anchor is not in the current branch lineage",
                        )
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
            load_last_assistant(
                &transaction,
                &conversation_id,
                &lineage,
                &messages,
                start == 0,
            )?
        } else {
            None
        };
        let page = BranchMessagePage {
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
        };
        transaction.commit().map_err(storage_db_error)?;
        Ok(page)
    }
}

// No content enters the recursive CTE. UNION terminates even corrupt cycles;
// reconstructing by parent identity makes ordering independent of SQLite order.
fn load_lineage_identities(
    connection: &Connection,
    conversation_id: &str,
    head: Option<&str>,
) -> CoreResult<Vec<String>> {
    let mut statement = connection
        .prepare_cached(
            "WITH RECURSIVE lineage(id, parent_id) AS (
           SELECT id, parent_id FROM messages WHERE conversation_id = ?1 AND id = ?2
           UNION
           SELECT parent.id, parent.parent_id FROM messages AS parent
           JOIN lineage ON parent.id = lineage.parent_id
           WHERE parent.conversation_id = ?1
         ) SELECT id, parent_id FROM lineage",
        )
        .map_err(storage_db_error)?;
    let mut parents = statement
        .query_map(params![conversation_id, head], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .map_err(storage_db_error)?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(storage_db_error)?;
    let mut next = head.map(str::to_owned);
    let mut lineage = Vec::with_capacity(parents.len());
    while let Some(id) = next {
        next = parents.remove(&id).ok_or_else(|| {
            storage_corrupted("message page lineage contains a cycle or missing parent")
        })?;
        lineage.push(id);
    }
    Ok(lineage)
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
         JOIN messages AS message ON message.id = selected.value
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

fn load_last_assistant(
    connection: &Connection,
    conversation_id: &str,
    lineage: &[String],
    page_messages: &[Message],
    includes_head: bool,
) -> CoreResult<Option<Message>> {
    if lineage.is_empty() {
        return Ok(None);
    }
    if includes_head
        && page_messages
            .iter()
            .any(|message| message.role == MessageRole::Assistant)
    {
        return Ok(None);
    }
    let ids = serde_json::to_string(lineage).map_err(|error| {
        CoreError::internal(format!(
            "cannot encode assistant lookup identities: {error}"
        ))
    })?;
    let page_ids = serde_json::to_string(
        &page_messages
            .iter()
            .map(|message| &message.id.0)
            .collect::<Vec<_>>(),
    )
    .map_err(|error| {
        CoreError::internal(format!("cannot encode assistant page identities: {error}"))
    })?;
    connection
        .prepare_cached(
            "WITH selected AS MATERIALIZED (
           SELECT message.id FROM json_each(?1) AS lineage
           JOIN messages AS message ON message.id = lineage.value
           WHERE message.conversation_id = ?2 AND message.role = 'assistant'
           ORDER BY CAST(lineage.key AS INTEGER) LIMIT 1
         )
         SELECT message.id, message.conversation_id, message.parent_id, message.role,
                message.content, message.status, message.generation_id, message.created_at
         FROM selected JOIN messages AS message ON message.id = selected.id
         WHERE selected.id NOT IN (SELECT value FROM json_each(?3))",
        )
        .map_err(storage_db_error)?
        .query_row(params![ids, conversation_id, page_ids], map_message)
        .optional()
        .map_err(storage_db_error)
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

fn retained_candidates(lineage: &[String], ids: &[MessageId]) -> Vec<MessageId> {
    if ids.is_empty() {
        return Vec::new();
    }
    let requested = ids.iter().map(|id| id.0.as_str()).collect::<HashSet<_>>();
    let retained = lineage
        .iter()
        .map(String::as_str)
        .filter(|id| requested.contains(id))
        .collect::<HashSet<_>>();
    ids.iter()
        .filter(|id| retained.contains(id.0.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
