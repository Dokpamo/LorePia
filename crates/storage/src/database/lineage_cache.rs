//! A single bounded identity snapshot, never a cache of message bodies.
use super::{Connection, CoreResult, params, storage_corrupted, storage_db_error};
use std::{collections::HashMap, sync::Arc};

const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub(super) struct LineageCache {
    entry: Option<Entry>,
    #[cfg(test)]
    pub(super) validations: usize,
}

struct Entry {
    conversation: String,
    head: Option<String>,
    local_changes: u64,
    external_version: i64,
    identities: Arc<Vec<String>>,
}

impl LineageCache {
    // Caller holds the database guard and has already established its read
    // transaction snapshot by reading the branch. data_version therefore
    // describes that snapshot; writes on this connection use total_changes.
    pub(super) fn load(
        &mut self,
        connection: &rusqlite::Transaction<'_>,
        conversation: &str,
        head: Option<&str>,
    ) -> CoreResult<Arc<Vec<String>>> {
        let external_version = connection
            .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
            .map_err(storage_db_error)?;
        let local_changes = connection.total_changes();
        if let Some(entry) = &self.entry
            && entry.conversation == conversation
            && entry.head.as_deref() == head
            && entry.local_changes == local_changes
            && entry.external_version == external_version
        {
            return Ok(Arc::clone(&entry.identities));
        }
        self.entry = None;
        #[cfg(test)]
        {
            self.validations += 1;
        }
        let identities = Arc::new(load_identities(connection, conversation, head)?);
        let bytes = identities.iter().try_fold(
            identities
                .capacity()
                .saturating_mul(std::mem::size_of::<String>())
                .saturating_add(conversation.len())
                .saturating_add(head.map_or(0, str::len))
                .saturating_add(std::mem::size_of::<Entry>())
                .saturating_add(
                    std::mem::size_of::<Vec<String>>() + 2 * std::mem::size_of::<usize>(),
                ),
            |sum, id| sum.checked_add(id.capacity()),
        );
        if bytes.is_some_and(|bytes| bytes <= MAX_CACHE_BYTES) {
            self.entry = Some(Entry {
                conversation: conversation.to_owned(),
                head: head.map(str::to_owned),
                local_changes,
                external_version,
                identities: Arc::clone(&identities),
            });
        }
        Ok(identities)
    }
}

// No content enters the recursive CTE. UNION terminates even corrupt cycles;
// reconstructing by parent identity makes ordering independent of SQLite order.
fn load_identities(
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

#[cfg(test)]
mod tests;
