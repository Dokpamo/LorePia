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
    local_epoch: u64,
    external_version: i64,
    identities: Arc<LineageSnapshot>,
}

pub(super) struct LineageSnapshot {
    identities: Vec<String>,
    by_id: Vec<usize>,
}

impl LineageSnapshot {
    fn new(identities: Vec<String>) -> Self {
        let mut by_id = (0..identities.len()).collect::<Vec<_>>();
        by_id.sort_unstable_by(|left, right| identities[*left].cmp(&identities[*right]));
        Self { identities, by_id }
    }

    pub(super) fn position(&self, id: &str) -> Option<usize> {
        self.by_id
            .binary_search_by(|index| self.identities[*index].as_str().cmp(id))
            .ok()
            .map(|index| self.by_id[index])
    }

    pub(super) fn as_slice(&self) -> &[String] {
        &self.identities
    }
}

impl std::ops::Deref for LineageSnapshot {
    type Target = [String];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}

impl LineageCache {
    // Caller holds the database guard and has already established its read
    // transaction snapshot by reading the branch. data_version describes that
    // snapshot; local writes use the monotonic relevant-table epoch.
    pub(super) fn load(
        &mut self,
        connection: &rusqlite::Transaction<'_>,
        conversation: &str,
        head: Option<&str>,
        local_epoch: u64,
    ) -> CoreResult<Arc<LineageSnapshot>> {
        // Never retain an uncommitted write snapshot: savepoint/transaction
        // rollback can restore ancestry without invoking row update hooks.
        let local_epoch = if connection
            .transaction_state(Some("main"))
            .map_err(storage_db_error)?
            == rusqlite::TransactionState::Read
        {
            local_epoch
        } else {
            u64::MAX
        };
        let external_version = connection
            .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
            .map_err(storage_db_error)?;
        if let Some(entry) = &self.entry
            && entry.conversation == conversation
            && entry.head.as_deref() == head
            && entry.local_epoch == local_epoch
            && local_epoch != u64::MAX
            && entry.external_version == external_version
        {
            return Ok(Arc::clone(&entry.identities));
        }
        self.entry = None;
        #[cfg(test)]
        {
            self.validations += 1;
        }
        let identities = Arc::new(LineageSnapshot::new(load_identities(
            connection,
            conversation,
            head,
        )?));
        let bytes = identities.iter().try_fold(
            identities
                .identities
                .capacity()
                .saturating_mul(std::mem::size_of::<String>())
                .saturating_add(conversation.len())
                .saturating_add(head.map_or(0, str::len))
                .saturating_add(std::mem::size_of::<Entry>())
                .saturating_add(
                    std::mem::size_of::<LineageSnapshot>() + 2 * std::mem::size_of::<usize>(),
                ),
            |sum, id| sum.checked_add(id.capacity()),
        );
        let bytes = bytes.and_then(|bytes| {
            bytes.checked_add(
                identities
                    .by_id
                    .capacity()
                    .saturating_mul(std::mem::size_of::<usize>()),
            )
        });
        if local_epoch != u64::MAX
            && bytes.is_some_and(|bytes| bytes <= MAX_CACHE_BYTES)
            && supports_row_hooks(connection)?
        {
            self.entry = Some(Entry {
                conversation: conversation.to_owned(),
                head: head.map(str::to_owned),
                local_epoch,
                external_version,
                identities: Arc::clone(&identities),
            });
        }
        Ok(identities)
    }
}

// Update hooks cannot observe WITHOUT ROWID mutations. A schema change already
// invalidates through the local DDL epoch or external data_version; only misses
// need this shape check. A temporary namesake could shadow unqualified reads.
fn supports_row_hooks(connection: &Connection) -> CoreResult<bool> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_list WHERE schema='main' AND name='messages' AND type='table' AND wr=0)
            AND NOT EXISTS(SELECT 1 FROM pragma_table_list WHERE schema='temp' AND name='messages')",
        [], |row| row.get(0),
    ).map_err(storage_db_error)
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

#[cfg(test)]
mod workload_tests;
