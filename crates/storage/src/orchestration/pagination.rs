//! Bounded catalog reads. Live keyset cursors bind their visibility scope, not offsets.
use super::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, Deserialize, DeserializeOwned,
    Digest, DocumentTable, MemoryRecord, OptionalExtension, Serialize, Sha256, Storage,
    StoredRevision, decode_memory_record, decode_stored_document, not_found, params,
    raw_memory_record, storage_db_error, validate_identifier,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadPageCursor {
    pub scope: String,
    pub after_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after_updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatorDocumentKind {
    MemoryProfile,
    KnowledgeBook,
    TransformSet,
    InteractionRuleSet,
    ContentModule,
}
impl CreatorDocumentKind {
    fn table(self) -> DocumentTable {
        match self {
            Self::MemoryProfile => DocumentTable::MemoryProfiles,
            Self::KnowledgeBook => DocumentTable::KnowledgeBooks,
            Self::TransformSet => DocumentTable::TransformSets,
            Self::InteractionRuleSet => DocumentTable::InteractionRuleSets,
            Self::ContentModule => DocumentTable::ContentModules,
        }
    }
}

pub struct StoredReadPage<T> {
    pub items: Vec<StoredRevision<T>>,
    pub scope: String,
    pub has_more: bool,
}

const DOCUMENT_FROM: &str = "FROM content_objects AS object
 JOIN content_object_state AS state ON state.object_id = object.id
 JOIN content_revisions AS revision ON revision.id = state.active_revision_id
 AND revision.object_id = object.id
 WHERE object.object_kind = ?1 AND object.deleted_at IS NULL
 AND revision.source_kind = 'user_created'";
const MEMORY_WITH: &str = "WITH RECURSIVE lineage(id, parent_id) AS (
 SELECT message.id, message.parent_id FROM conversation_branches AS branch
 JOIN messages AS message ON message.conversation_id = branch.conversation_id
 AND message.id = branch.head_message_id WHERE branch.conversation_id = ?1 AND branch.id = ?2
 UNION SELECT parent.id, parent.parent_id FROM messages AS parent
 JOIN lineage AS child ON child.parent_id = parent.id WHERE parent.conversation_id = ?1)";
const MEMORY_FROM: &str = "FROM memory_records AS record
 JOIN lineage AS source_start ON source_start.id = record.source_start_message_id
 JOIN lineage AS source_end ON source_end.id = record.source_end_message_id
 JOIN memory_record_state AS state ON state.record_id = record.id
 JOIN memory_record_revisions AS revision ON revision.id = state.active_revision_id
 WHERE record.conversation_id = ?1 AND state.deleted_at IS NULL
 AND (?3 OR state.invalidated_at IS NULL)";
const PAGE_RAW_BYTES: usize = 512 * 1024;

impl Storage {
    /// Creator-only documents by recency, then ID. Imported documents are filtered in SQL.
    pub fn list_creator_documents_page<T: DeserializeOwned>(
        &self,
        kind: CreatorDocumentKind,
        after: Option<&ReadPageCursor>,
        limit: u32,
    ) -> CoreResult<StoredReadPage<T>> {
        validate_page(after, limit)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let table = kind.table();
        let scope = page_scope(table.object_kind(), after)?;
        let after_time = match after {
            Some(cursor) => Some(match cursor.after_updated_at {
                Some(time) => time.to_rfc3339(),
                None => transaction
                    .query_row(
                        "SELECT state.updated_at FROM content_object_state AS state
                     JOIN content_objects AS object ON object.id = state.object_id
                     WHERE object.id = ?1 AND object.object_kind = ?2",
                        params![cursor.after_id, table.object_kind()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(storage_db_error)?
                    .ok_or_else(|| CoreError::invalid("creator cursor anchor no longer exists"))?,
            }),
            None => None,
        };
        let query = format!(
            "SELECT revision.document_json, state.state_version, revision.id,
            object.created_at, state.updated_at, object.deleted_at {DOCUMENT_FROM}
            AND (?2 IS NULL OR state.updated_at < ?2
                 OR (state.updated_at = ?2 AND object.id > ?3))
            ORDER BY state.updated_at DESC, object.id LIMIT ?4"
        );
        let mut statement = transaction.prepare(&query).map_err(storage_db_error)?;
        let mut rows = statement
            .query(params![
                table.object_kind(),
                after_time,
                after.map(|c| &c.after_id),
                limit + 1
            ])
            .map_err(storage_db_error)?;
        let mut items = Vec::new();
        let mut bytes = 0;
        let mut has_more = false;
        while let Some(row) = rows.next().map_err(storage_db_error)? {
            let json: String = row.get(0).map_err(storage_db_error)?;
            if items.len() == limit as usize
                || (!items.is_empty() && bytes + json.len() > PAGE_RAW_BYTES)
            {
                has_more = true;
                break;
            }
            bytes += json.len();
            let raw = (
                json,
                row.get(1).map_err(storage_db_error)?,
                row.get(2).map_err(storage_db_error)?,
                row.get(3).map_err(storage_db_error)?,
                row.get(4).map_err(storage_db_error)?,
                row.get(5).map_err(storage_db_error)?,
            );
            items.push(decode_stored_document(table.object_kind(), raw)?);
        }
        Ok(StoredReadPage {
            items,
            scope,
            has_more,
        })
    }

    /// Same visibility predicate as memory CRUD, with an ID cursor and bounded document decoding.
    pub fn list_memory_records_page(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        include_invalidated: bool,
        after: Option<&ReadPageCursor>,
        limit: u32,
    ) -> CoreResult<StoredReadPage<MemoryRecord>> {
        validate_page(after, limit)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        transaction
            .query_row(
                "SELECT 1 FROM conversation_branches WHERE conversation_id = ?1 AND id = ?2",
                params![conversation_id.0, branch_id.0],
                |_| Ok(()),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("memory record branch"))?;
        let scope_input = serde_json::to_string(&(conversation_id, branch_id, include_invalidated))
            .map_err(|_| CoreError::internal("cannot encode memory page scope"))?;
        let scope = page_scope(&scope_input, after)?;
        let query = format!(
            "{MEMORY_WITH} SELECT revision.document_json, state.state_version,
            state.active_revision_id, record.created_at, state.updated_at, state.deleted_at,
            state.pinned, state.invalidated_at, state.excluded_from_conversation_at,
            state.excluded_from_character_at {MEMORY_FROM}
            AND (?4 IS NULL OR record.id > ?4) ORDER BY record.id LIMIT ?5"
        );
        let mut statement = transaction.prepare(&query).map_err(storage_db_error)?;
        let mut rows = statement
            .query(params![
                conversation_id.0,
                branch_id.0,
                include_invalidated,
                after.map(|c| &c.after_id),
                limit + 1
            ])
            .map_err(storage_db_error)?;
        let mut items = Vec::new();
        let mut bytes = 0;
        let mut has_more = false;
        while let Some(row) = rows.next().map_err(storage_db_error)? {
            let json: String = row.get(0).map_err(storage_db_error)?;
            if items.len() == limit as usize
                || (!items.is_empty() && bytes + json.len() > PAGE_RAW_BYTES)
            {
                has_more = true;
                break;
            }
            bytes += json.len();
            items.push(decode_memory_record(
                raw_memory_record(row).map_err(storage_db_error)?,
            )?);
        }
        Ok(StoredReadPage {
            items,
            scope,
            has_more,
        })
    }
}

fn validate_page(after: Option<&ReadPageCursor>, limit: u32) -> CoreResult<()> {
    if !(1..=100).contains(&limit) {
        return Err(CoreError::invalid("page limit must be between 1 and 100"));
    }
    if let Some(cursor) = after {
        validate_identifier("page cursor", &cursor.after_id)?;
        if cursor.scope.len() != 64
            || !cursor
                .scope
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(CoreError::invalid(
                "page scope must be a lowercase SHA-256 digest",
            ));
        }
    }
    Ok(())
}

fn page_scope(scope: &str, after: Option<&ReadPageCursor>) -> CoreResult<String> {
    let mut digest = Sha256::new();
    digest.update(b"lorepia:read-page-scope:v1\0");
    digest.update(scope.as_bytes());
    let scope = hex::encode(digest.finalize());
    if after.is_some_and(|cursor| cursor.scope != scope) {
        return Err(CoreError::invalid(
            "page cursor belongs to a different collection",
        ));
    }
    Ok(scope)
}
