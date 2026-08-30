//! Revisioned memory-record persistence.

use super::{
    BTreeSet, Connection, ConversationBranchId, ConversationId, CoreError, CoreResult, DateTime,
    MemoryRecord, MemoryRecordId, MessageId, OptionalExtension, Storage, StoredRevision,
    Transaction, TransactionBehavior, Utc, Uuid, ValidateOrchestration, Value, decode_document,
    encode_document, enum_wire, i64_revision, not_found, params, parse_datetime, revision_conflict,
    storage_db_error, u64_revision,
};

impl Storage {
    pub fn save_memory_record(
        &self,
        record: &MemoryRecord,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<MemoryRecord>> {
        save_memory_record(self, record, expected_revision)
    }

    pub fn get_memory_record(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
    ) -> CoreResult<StoredRevision<MemoryRecord>> {
        get_memory_record(self, id, false, Some((conversation_id, branch_id)))
    }
}

#[derive(Debug)]
pub(super) struct RawMemoryRecord {
    document_json: String,
    pub(super) state_version: i64,
    pub(super) active_revision_id: String,
    created_at: String,
    updated_at: String,
    deleted_at: Option<String>,
    pinned: bool,
    invalidated_at: Option<String>,
    excluded_from_conversation_at: Option<String>,
    excluded_from_character_at: Option<String>,
}

pub(super) fn raw_memory_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMemoryRecord> {
    Ok(RawMemoryRecord {
        document_json: row.get(0)?,
        state_version: row.get(1)?,
        active_revision_id: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        deleted_at: row.get(5)?,
        pinned: row.get(6)?,
        invalidated_at: row.get(7)?,
        excluded_from_conversation_at: row.get(8)?,
        excluded_from_character_at: row.get(9)?,
    })
}

pub(super) fn decode_memory_record(
    raw: RawMemoryRecord,
) -> CoreResult<StoredRevision<MemoryRecord>> {
    let mut value = decode_document::<MemoryRecord>("memory record", &raw.document_json)?;
    value.created_at = parse_datetime("memory record created_at", &raw.created_at)?;
    value.updated_at = parse_datetime("memory record updated_at", &raw.updated_at)?;
    value.pinned = raw.pinned;
    value.invalidated_at = raw
        .invalidated_at
        .as_deref()
        .map(|value| parse_datetime("memory invalidated_at", value))
        .transpose()?;
    value.excluded_from_conversation = raw.excluded_from_conversation_at.is_some();
    value.excluded_from_character = raw.excluded_from_character_at.is_some();
    Ok(StoredRevision {
        value,
        revision: u64_revision(raw.state_version)?,
        revision_id: Some(raw.active_revision_id),
        created_at: parse_datetime("memory record created_at", &raw.created_at)?,
        updated_at: parse_datetime("memory record updated_at", &raw.updated_at)?,
        deleted_at: raw
            .deleted_at
            .as_deref()
            .map(|value| parse_datetime("memory record deleted_at", value))
            .transpose()?,
    })
}

fn get_memory_record(
    storage: &Storage,
    id: &MemoryRecordId,
    include_deleted: bool,
    owner: Option<(&ConversationId, &ConversationBranchId)>,
) -> CoreResult<StoredRevision<MemoryRecord>> {
    let deleted_clause = if include_deleted {
        ""
    } else {
        " AND state.deleted_at IS NULL"
    };
    let sql = format!(
        "SELECT revision.document_json, state.state_version,
                state.active_revision_id, record.created_at, state.updated_at,
                state.deleted_at, state.pinned, state.invalidated_at,
                state.excluded_from_conversation_at,
                state.excluded_from_character_at
         FROM memory_records AS record
         JOIN memory_record_state AS state ON state.record_id = record.id
         JOIN memory_record_revisions AS revision
           ON revision.record_id = record.id
          AND revision.id = state.active_revision_id
         WHERE record.id = ?1
           AND (?2 IS NULL OR (
               record.conversation_id = ?2 AND record.branch_id = ?3
           )){deleted_clause}"
    );
    let (conversation_id, branch_id) =
        owner.map_or((None, None), |(conversation_id, branch_id)| {
            (Some(conversation_id.0.as_str()), Some(branch_id.0.as_str()))
        });
    let raw = storage
        .connection()?
        .query_row(
            &sql,
            params![id.as_str(), conversation_id, branch_id],
            raw_memory_record,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory record"))?;
    decode_memory_record(raw)
}

fn normalize_memory_keywords(keywords: &[String]) -> CoreResult<Vec<(String, String)>> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::with_capacity(keywords.len());
    for keyword in keywords {
        let trimmed = keyword.trim();
        if trimmed.is_empty() || trimmed.chars().count() > 512 {
            return Err(CoreError::invalid(
                "memory keywords must be non-empty and bounded",
            ));
        }
        let folded = trimmed.to_lowercase();
        if !seen.insert(folded.clone()) {
            return Err(CoreError::invalid(
                "memory keywords must be unique after normalization",
            ));
        }
        normalized.push((trimmed.to_owned(), folded));
    }
    Ok(normalized)
}

fn validate_memory_source_range(connection: &Connection, record: &MemoryRecord) -> CoreResult<()> {
    let head = connection
        .query_row(
            "SELECT head_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![record.conversation_id.0, record.branch_id.0],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory record branch"))?
        .ok_or_else(|| CoreError::invalid("memory record branch has no messages"))?;
    if !message_is_ancestor(
        connection,
        &record.conversation_id,
        &record.source_end_message_id,
        &MessageId(head),
    )? || !message_is_ancestor(
        connection,
        &record.conversation_id,
        &record.source_start_message_id,
        &record.source_end_message_id,
    )? {
        return Err(CoreError::invalid(
            "memory source range is not an ordered range on its branch lineage",
        ));
    }
    Ok(())
}

fn message_is_ancestor(
    connection: &Connection,
    conversation_id: &ConversationId,
    ancestor_id: &MessageId,
    descendant_id: &MessageId,
) -> CoreResult<bool> {
    connection
        .query_row(
            "WITH RECURSIVE lineage(id, parent_id) AS (
                 SELECT id, parent_id
                 FROM messages
                 WHERE conversation_id = ?1 AND id = ?2
                 UNION
                 SELECT parent.id, parent.parent_id
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1
             )
             SELECT EXISTS(SELECT 1 FROM lineage WHERE id = ?3)",
            params![conversation_id.0, descendant_id.0, ancestor_id.0],
            |row| row.get(0),
        )
        .map_err(storage_db_error)
}

struct CurrentMemoryRecordRevision {
    conversation_id: String,
    branch_id: String,
    source_start_message_id: String,
    source_end_message_id: String,
    kind: String,
    created_at: String,
    state_version: i64,
    active_revision_id: String,
    deleted_at: Option<String>,
    document_json: String,
}

fn current_memory_record_revision(
    transaction: &Transaction<'_>,
    id: &MemoryRecordId,
) -> CoreResult<Option<CurrentMemoryRecordRevision>> {
    transaction
        .query_row(
            "SELECT record.conversation_id, record.branch_id,
                    record.source_start_message_id, record.source_end_message_id,
                    record.kind, record.created_at, state.state_version,
                    state.active_revision_id, state.deleted_at,
                    revision.document_json
             FROM memory_records AS record
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN memory_record_revisions AS revision
               ON revision.id = state.active_revision_id
             WHERE record.id = ?1",
            [id.as_str()],
            |row| {
                Ok(CurrentMemoryRecordRevision {
                    conversation_id: row.get(0)?,
                    branch_id: row.get(1)?,
                    source_start_message_id: row.get(2)?,
                    source_end_message_id: row.get(3)?,
                    kind: row.get(4)?,
                    created_at: row.get(5)?,
                    state_version: row.get(6)?,
                    active_revision_id: row.get(7)?,
                    deleted_at: row.get(8)?,
                    document_json: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)
}

struct MemoryRevisionContext {
    next_state_version: u64,
    revision_no: u64,
    parent_revision_id: Option<String>,
    created_at: DateTime<Utc>,
    previous: Option<MemoryRecord>,
}

fn resolve_memory_revision_context(
    transaction: &Transaction<'_>,
    record: &MemoryRecord,
    expected_revision: Option<u64>,
    current: Option<CurrentMemoryRecordRevision>,
) -> CoreResult<MemoryRevisionContext> {
    match (expected_revision, current) {
        (None, None) => Ok(MemoryRevisionContext {
            next_state_version: 1,
            revision_no: 1,
            parent_revision_id: None,
            created_at: record.created_at,
            previous: None,
        }),
        (None, Some(current)) => Err(revision_conflict(
            "memory record",
            record.id.as_str(),
            None,
            Some(u64_revision(current.state_version)?),
        )),
        (Some(expected), None) => Err(revision_conflict(
            "memory record",
            record.id.as_str(),
            Some(expected),
            None,
        )),
        (Some(expected), Some(current)) => {
            resolve_existing_memory_revision_context(transaction, record, expected, current)
        }
    }
}

fn resolve_existing_memory_revision_context(
    transaction: &Transaction<'_>,
    record: &MemoryRecord,
    expected: u64,
    current: CurrentMemoryRecordRevision,
) -> CoreResult<MemoryRevisionContext> {
    let actual = u64_revision(current.state_version)?;
    if current.deleted_at.is_some() || actual != expected {
        return Err(revision_conflict(
            "memory record",
            record.id.as_str(),
            Some(expected),
            Some(actual),
        ));
    }
    if current.conversation_id != record.conversation_id.0
        || current.branch_id != record.branch_id.0
        || current.source_start_message_id != record.source_start_message_id.0
        || current.source_end_message_id != record.source_end_message_id.0
        || current.kind != enum_wire(&record.kind)?
    {
        return Err(CoreError::invalid(
            "memory record identity and source range are immutable",
        ));
    }
    let latest_revision_no = transaction
        .query_row(
            "SELECT MAX(revision_no)
             FROM memory_record_revisions WHERE record_id = ?1",
            [record.id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)?;
    Ok(MemoryRevisionContext {
        next_state_version: expected
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("memory state revision overflow"))?,
        revision_no: u64_revision(latest_revision_no)?
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("memory revision overflow"))?,
        parent_revision_id: Some(current.active_revision_id),
        created_at: parse_datetime("memory record created_at", &current.created_at)?,
        previous: Some(decode_document::<MemoryRecord>(
            "memory record",
            &current.document_json,
        )?),
    })
}

fn insert_memory_record_identity(
    transaction: &Transaction<'_>,
    value: &MemoryRecord,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO memory_records
             (id, conversation_id, branch_id, source_start_message_id,
              source_end_message_id, kind, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                value.id.as_str(),
                value.conversation_id.0,
                value.branch_id.0,
                value.source_start_message_id.0,
                value.source_end_message_id.0,
                enum_wire(&value.kind)?,
                value.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_memory_record_revision(
    transaction: &Transaction<'_>,
    value: &MemoryRecord,
    context: &MemoryRevisionContext,
    revision_id: &str,
) -> CoreResult<()> {
    let (document_json, content_sha256) = encode_document("memory record", value)?;
    let (structured_data_json, _) =
        encode_document("memory structured data", &value.structured_data)?;
    let (provenance_json, _) = encode_document("memory provenance", &value.provenance)?;
    transaction
        .execute(
            "INSERT INTO memory_record_revisions
             (id, record_id, revision_no, parent_revision_id, title, summary,
              structured_data_json, importance, content_sha256,
              provenance_json, document_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                revision_id,
                value.id.as_str(),
                i64_revision(context.revision_no)?,
                context.parent_revision_id,
                value.title,
                value.summary,
                structured_data_json,
                value.importance,
                content_sha256,
                provenance_json,
                document_json,
                value.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_memory_record_keywords(
    transaction: &Transaction<'_>,
    revision_id: &str,
    keywords: &[(String, String)],
) -> CoreResult<()> {
    for (ordinal, (keyword, normalized_keyword)) in keywords.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO memory_record_keywords
                 (record_revision_id, ordinal, keyword, normalized_keyword)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    revision_id,
                    i64::try_from(ordinal)
                        .map_err(|_| CoreError::invalid("too many memory keywords"))?,
                    keyword,
                    normalized_keyword,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_memory_record_state(
    transaction: &Transaction<'_>,
    value: &MemoryRecord,
    context: &MemoryRevisionContext,
    revision_id: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    let invalidation_reason = value.invalidated_at.map(|_| "record_update");
    let excluded_conversation_at = value
        .excluded_from_conversation
        .then(|| value.updated_at.to_rfc3339());
    let excluded_character_at = value
        .excluded_from_character
        .then(|| value.updated_at.to_rfc3339());
    let Some(expected_revision) = expected_revision else {
        transaction
            .execute(
                "INSERT INTO memory_record_state
                 (record_id, active_revision_id, pinned, invalidated_at,
                  invalidation_reason, excluded_from_conversation_at,
                  excluded_from_character_at, deleted_at, state_version,
                  updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 1, ?8)",
                params![
                    value.id.as_str(),
                    revision_id,
                    value.pinned,
                    value.invalidated_at.map(|time| time.to_rfc3339()),
                    invalidation_reason,
                    excluded_conversation_at,
                    excluded_character_at,
                    value.updated_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        return Ok(());
    };
    let changed = transaction
        .execute(
            "UPDATE memory_record_state
             SET active_revision_id = ?2, pinned = ?3, invalidated_at = ?4,
                 invalidation_reason = ?5,
                 excluded_from_conversation_at = ?6,
                 excluded_from_character_at = ?7,
                 state_version = ?8, updated_at = ?9
             WHERE record_id = ?1 AND state_version = ?10
               AND deleted_at IS NULL",
            params![
                value.id.as_str(),
                revision_id,
                value.pinned,
                value.invalidated_at.map(|time| time.to_rfc3339()),
                invalidation_reason,
                excluded_conversation_at,
                excluded_character_at,
                i64_revision(context.next_state_version)?,
                value.updated_at.to_rfc3339(),
                i64_revision(expected_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "memory record",
            value.id.as_str(),
            Some(expected_revision),
            None,
        ));
    }
    Ok(())
}

fn memory_record_event_kind(previous: Option<&MemoryRecord>, value: &MemoryRecord) -> &'static str {
    match previous {
        None => "created",
        Some(previous) if previous.invalidated_at.is_some() && value.invalidated_at.is_none() => {
            "restored"
        }
        Some(previous) if previous.pinned != value.pinned => {
            if value.pinned {
                "pinned"
            } else {
                "unpinned"
            }
        }
        Some(previous)
            if !previous.excluded_from_conversation && value.excluded_from_conversation =>
        {
            "excluded_conversation"
        }
        Some(previous) if !previous.excluded_from_character && value.excluded_from_character => {
            "excluded_character"
        }
        _ => "edited",
    }
}

fn save_memory_record(
    storage: &Storage,
    record: &MemoryRecord,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<MemoryRecord>> {
    record
        .validate()
        .map_err(|error| CoreError::invalid(error.to_string()))?;
    let keywords = normalize_memory_keywords(&record.keywords)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    validate_memory_source_range(&transaction, record)?;
    let current = current_memory_record_revision(&transaction, &record.id)?;
    let context =
        resolve_memory_revision_context(&transaction, record, expected_revision, current)?;
    let mut value = record.clone();
    value.created_at = context.created_at;
    if value.updated_at < context.created_at {
        return Err(CoreError::invalid(
            "memory record update time predates creation",
        ));
    }
    let revision_id = Uuid::new_v4().to_string();
    if expected_revision.is_none() {
        insert_memory_record_identity(&transaction, &value)?;
    }
    insert_memory_record_revision(&transaction, &value, &context, &revision_id)?;
    insert_memory_record_keywords(&transaction, &revision_id, &keywords)?;
    write_memory_record_state(
        &transaction,
        &value,
        &context,
        &revision_id,
        expected_revision,
    )?;
    let event_kind = memory_record_event_kind(context.previous.as_ref(), &value);
    append_memory_event(
        &transaction,
        value.id.as_str(),
        event_kind,
        context.parent_revision_id.as_deref(),
        Some(&revision_id),
        serde_json::json!({"state_version": context.next_state_version}),
        value.updated_at,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value,
        revision: context.next_state_version,
        revision_id: Some(revision_id),
        created_at: context.created_at,
        updated_at: record.updated_at,
        deleted_at: None,
    })
}

pub(super) fn append_memory_event(
    transaction: &Transaction<'_>,
    record_id: &str,
    event_kind: &str,
    from_revision_id: Option<&str>,
    to_revision_id: Option<&str>,
    payload: Value,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let payload_json = serde_json::to_string(&payload)
        .map_err(|error| CoreError::invalid(format!("cannot encode memory event: {error}")))?;
    transaction
        .execute(
            "INSERT INTO memory_record_events
             (record_id, sequence, event_kind, from_revision_id, to_revision_id,
              payload_json, created_at)
             VALUES (
                 ?1,
                 (SELECT COALESCE(MAX(sequence), 0) + 1
                  FROM memory_record_events WHERE record_id = ?1),
                 ?2, ?3, ?4, ?5, ?6
             )",
            params![
                record_id,
                event_kind,
                from_revision_id,
                to_revision_id,
                payload_json,
                created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}
