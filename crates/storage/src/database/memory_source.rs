//! Summary selection reads metadata for the lineage and bodies only for its range.
use super::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, Message, MessageId, MessageRole,
    MessageStatus, OptionalExtension, Storage, TransactionBehavior, map_message, not_found, params,
    storage_db_error,
};

/// A completed, non-system, nonempty message's identity for summary cadence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemorySourceMessageIdentity {
    pub id: MessageId,
    pub role: MessageRole,
}

const WHITE_SPACE: &str = "\u{9}\u{a}\u{b}\u{c}\u{d}\u{20}\u{85}\u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
const MAX_MESSAGES: usize = 512;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_CHARS: usize = 1_048_576;

const SOURCE_SQL: &str = "SELECT m.id,m.conversation_id,m.parent_id,m.role,
                        CASE WHEN typeof(m.content) != 'text' THEN m.content
                             WHEN CAST(j.key AS INTEGER) BETWEEN ?2 AND ?3 THEN
                               CASE WHEN m.role != 'system' AND m.status='complete'
                                      AND trim(m.content,?4) != ''
                                      AND length(CAST(m.content AS BLOB)) <= ?5
                                    THEN m.content ELSE '' END
                             ELSE '' END,
                        m.status,m.generation_id,m.created_at,
                        CASE WHEN ?2 < 0 OR CAST(j.key AS INTEGER) BETWEEN ?2 AND ?3
                             THEN trim(m.content,?4) != '' ELSE 0 END,
                        CASE WHEN CAST(j.key AS INTEGER) BETWEEN ?2 AND ?3
                             THEN m.role != 'system' AND m.status='complete'
                                  AND trim(m.content,?4) != '' ELSE 0 END,
                        CASE WHEN CAST(j.key AS INTEGER) BETWEEN ?2 AND ?3
                             THEN length(CAST(m.content AS BLOB)) ELSE 0 END
                 FROM json_each(?1) j JOIN messages m ON m.id=j.value
                 ORDER BY CAST(j.key AS INTEGER) DESC";

type SourceRead = (Vec<MemorySourceMessageIdentity>, Vec<Message>);

impl Storage {
    pub fn list_branch_memory_source_identities(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Vec<MemorySourceMessageIdentity>> {
        self.read_memory_source(conversation_id, branch_id, None)
            .map(|read| read.0)
    }

    pub fn list_branch_memory_source(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        start: &MessageId,
        end: &MessageId,
    ) -> CoreResult<Vec<Message>> {
        self.read_memory_source(conversation_id, branch_id, Some((start, end)))
            .map(|read| read.1)
    }

    fn read_memory_source(
        &self,
        conversation: &ConversationId,
        branch: &ConversationBranchId,
        range: Option<(&MessageId, &MessageId)>,
    ) -> CoreResult<SourceRead> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Deferred)
            .map_err(storage_db_error)?;
        let (owner, head) = transaction
            .query_row(
                "SELECT conversation_id, head_message_id FROM conversation_branches WHERE id=?1",
                [&branch.0],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("conversation branch"))?;
        if owner != conversation.0 {
            return Err(CoreError::invalid(
                "memory job branch does not belong to its conversation",
            ));
        }
        let lineage = self
            .lineage_cache
            .lock()
            .map_err(|_| CoreError::internal("lineage cache lock was poisoned"))?
            .load(
                &transaction,
                &owner,
                head.as_deref(),
                self.change_tracking.lineage_epoch(),
            )?;
        let (from, to) = range_indices(&lineage, range)?;
        let ids = serde_json::to_string(lineage.as_slice()).map_err(|error| {
            CoreError::internal(format!("cannot encode memory identities: {error}"))
        })?;
        if range.is_some() {
            preflight_range(&transaction, &ids, from, to)?;
        }
        let mut identities = Vec::new();
        let mut messages = Vec::new();
        let mut bytes = 0usize;
        let mut chars = 0usize;
        {
            let mut statement = transaction
                .prepare_cached(SOURCE_SQL)
                .map_err(storage_db_error)?;
            let mut rows = statement
                .query(params![ids, from, to, WHITE_SPACE, MAX_BYTES])
                .map_err(storage_db_error)?;
            while let Some(row) = rows.next().map_err(storage_db_error)? {
                // SQL omits an oversized selected body before SQLite sends it
                // across the row boundary; aggregate Rust allocation is bounded too.
                let selected = row.get::<_, bool>(9).map_err(storage_db_error)?;
                if selected {
                    let length = row.get::<_, usize>(10).map_err(storage_db_error)?;
                    if bytes.saturating_add(length) > MAX_BYTES {
                        return Err(CoreError::invalid(
                            "memory summary source exceeds the text safety limit",
                        ));
                    }
                }
                let message = map_message(row).map_err(storage_db_error)?;
                if message.role == MessageRole::System
                    || message.status != MessageStatus::Complete
                    || !row.get::<_, bool>(8).map_err(storage_db_error)?
                {
                    continue;
                }
                if range.is_none() {
                    identities.push(MemorySourceMessageIdentity {
                        id: message.id.clone(),
                        role: message.role,
                    });
                }
                if row.get::<_, bool>(9).map_err(storage_db_error)? {
                    bytes = bytes.saturating_add(message.content.len());
                    chars = chars.saturating_add(message.content.chars().count());
                    messages.push(message);
                    if messages.len() > MAX_MESSAGES {
                        return Err(CoreError::invalid(
                            "memory source exceeds the message-count safety limit",
                        ));
                    }
                    if bytes > MAX_BYTES || chars > MAX_CHARS {
                        return Err(CoreError::invalid(
                            "memory summary source exceeds the text safety limit",
                        ));
                    }
                }
            }
        }
        validate_endpoints(&messages, range)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok((identities, messages))
    }
}

#[cfg(test)]
mod tests;

fn validate_endpoints(
    messages: &[Message],
    range: Option<(&MessageId, &MessageId)>,
) -> CoreResult<()> {
    if let Some((start, end)) = range {
        if messages.first().map(|m| &m.id) != Some(start) {
            return Err(CoreError::invalid(
                "memory source start is no longer in the branch",
            ));
        }
        if messages.last().map(|m| &m.id) != Some(end) {
            return Err(CoreError::invalid(
                "memory source end is no longer in the branch",
            ));
        }
    }
    Ok(())
}

// Before sorting/returning any selected body, bound the complete selected
// payload inside the same snapshot. The extra sentinel detects count overflow.
fn preflight_range(
    connection: &super::Connection,
    ids: &str,
    from: i64,
    to: i64,
) -> CoreResult<()> {
    let (count, bytes): (usize, usize) = connection
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(bytes),0) FROM (
            SELECT min(length(CAST(m.content AS BLOB)),?5+1) AS bytes
            FROM json_each(?1) j JOIN messages m ON m.id=j.value
            WHERE CAST(j.key AS INTEGER) BETWEEN ?2 AND ?3
              AND m.role != 'system' AND m.status='complete'
              AND trim(m.content,?4) != ''
            LIMIT 513
        )",
            params![ids, from, to, WHITE_SPACE, MAX_BYTES],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(storage_db_error)?;
    if count > MAX_MESSAGES {
        return Err(CoreError::invalid(
            "memory source exceeds the message-count safety limit",
        ));
    }
    if bytes > MAX_BYTES {
        return Err(CoreError::invalid(
            "memory summary source exceeds the text safety limit",
        ));
    }
    Ok(())
}

fn range_indices(
    lineage: &[String],
    range: Option<(&MessageId, &MessageId)>,
) -> CoreResult<(i64, i64)> {
    Ok(if let Some((start, end)) = range {
        let to = lineage
            .iter()
            .position(|id| id == &start.0)
            .ok_or_else(|| CoreError::invalid("memory source start is no longer in the branch"))?;
        let from = lineage
            .iter()
            .position(|id| id == &end.0)
            .ok_or_else(|| CoreError::invalid("memory source end is no longer in the branch"))?;
        if from > to {
            return Err(CoreError::invalid("memory source range is reversed"));
        }
        (
            i64::try_from(from).unwrap_or(i64::MAX),
            i64::try_from(to).unwrap_or(i64::MAX),
        )
    } else {
        (-1, -1)
    })
}
