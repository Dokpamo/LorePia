//! Memory visibility, lineage, and range invalidation.

use super::{
    Connection, ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult,
    DateTime, HashMap, MAX_MEMORY_RECORDS, MAX_PROMPT_BLOCKS, MemoryInvalidationResult,
    MemoryRecord, MemoryRecordAtHeadEvidence, MemoryRecordId, MemoryRecordsAtHeadSelection,
    MemoryRecordsAtHeadSnapshot, MessageId, OptionalExtension, RawMemoryRecord, Storage,
    StoredRevision, Transaction, TransactionBehavior, Utc, append_memory_event,
    decode_memory_record, memory_records_at_head_snapshot_sha256, not_found, params,
    raw_memory_record, storage_db_error, u64_revision, validate_identifier,
};

impl Storage {
    pub fn list_memory_records(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        include_invalidated: bool,
    ) -> CoreResult<Vec<StoredRevision<MemoryRecord>>> {
        list_visible_memory_records(self, conversation_id, branch_id, include_invalidated)
    }

    /// Resolves bounded message positions on one exact historical branch
    /// lineage without loading the full conversation into Core.
    ///
    /// Depth zero is `context_head_message_id`; larger depths are older. The
    /// context head must remain visible from `source_branch_id`, but it need
    /// not equal that branch's newer mutable head.
    pub fn message_lineage_depths_at_head(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        context_head_message_id: &MessageId,
        message_ids: &[MessageId],
    ) -> CoreResult<HashMap<MessageId, u64>> {
        validate_identifier("message lineage conversation", &conversation_id.0)?;
        validate_identifier("message lineage source branch", &source_branch_id.0)?;
        validate_identifier("message lineage context head", &context_head_message_id.0)?;
        if message_ids.len() > MAX_PROMPT_BLOCKS {
            return Err(CoreError::invalid(
                "message lineage position request exceeds its bound",
            ));
        }
        let mut requested = message_ids
            .iter()
            .map(|id| id.0.as_str())
            .collect::<Vec<_>>();
        for id in &requested {
            validate_identifier("message lineage member", id)?;
        }
        requested.sort_unstable();
        if requested.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(CoreError::invalid(
                "message lineage position identifiers must be unique",
            ));
        }
        let requested_json = serde_json::to_string(&requested).map_err(|error| {
            CoreError::internal(format!("cannot encode message lineage request: {error}"))
        })?;
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "WITH RECURSIVE source_lineage(id, parent_id, depth) AS (
                     SELECT message.id, message.parent_id, 0
                     FROM conversation_branches AS branch
                     JOIN messages AS message
                       ON message.conversation_id = branch.conversation_id
                      AND message.id = branch.head_message_id
                     WHERE branch.conversation_id = ?1 AND branch.id = ?2
                     UNION ALL
                     SELECT parent.id, parent.parent_id, child.depth + 1
                     FROM messages AS parent
                     JOIN source_lineage AS child ON child.parent_id = parent.id
                     WHERE parent.conversation_id = ?1 AND child.depth < 100000
                 ),
                 context(id, parent_id) AS (
                     SELECT message.id, message.parent_id
                     FROM messages AS message
                     JOIN source_lineage ON source_lineage.id = message.id
                     WHERE message.conversation_id = ?1 AND message.id = ?3
                 ),
                 lineage(id, parent_id, depth) AS (
                     SELECT id, parent_id, 0 FROM context
                     UNION ALL
                     SELECT parent.id, parent.parent_id, child.depth + 1
                     FROM messages AS parent
                     JOIN lineage AS child ON child.parent_id = parent.id
                     WHERE parent.conversation_id = ?1 AND child.depth < 100000
                 )
                 SELECT lineage.id, lineage.depth
                 FROM json_each(?4) AS requested
                 JOIN lineage ON lineage.id = requested.value
                 ORDER BY lineage.depth DESC, lineage.id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map(
                params![
                    conversation_id.0,
                    source_branch_id.0,
                    context_head_message_id.0,
                    requested_json
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        if rows.len() != requested.len() {
            return Err(CoreError::invalid(
                "requested message is unavailable at the exact prompt context head",
            ));
        }
        rows.into_iter()
            .map(|(id, depth)| Ok((MessageId(id), u64_revision(depth)?)))
            .collect()
    }

    pub fn invalidate_memory_range(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        start_message_id: &MessageId,
        end_message_id: &MessageId,
        invalidated_at: DateTime<Utc>,
    ) -> CoreResult<MemoryInvalidationResult> {
        invalidate_memory_range(
            self,
            conversation_id,
            branch_id,
            start_message_id,
            end_message_id,
            invalidated_at,
        )
    }
}

pub(super) fn prompt_context_changed(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::InvalidInput, message, true)
}

pub(super) fn memory_records_at_head_in_connection(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head_message_id: Option<&MessageId>,
    include_invalidated: bool,
) -> CoreResult<MemoryRecordsAtHeadSelection> {
    validate_identifier("memory conversation", &conversation_id.0)?;
    validate_identifier("memory source branch", &source_branch_id.0)?;
    require_memory_context_head_visible(
        connection,
        conversation_id,
        source_branch_id,
        context_head_message_id,
    )?;
    let (records, evidence) = context_head_message_id.map_or_else(
        || Ok((Vec::new(), Vec::new())),
        |context_head| {
            load_memory_records_at_head(
                connection,
                conversation_id,
                context_head,
                include_invalidated,
            )
        },
    )?;
    let mut snapshot = MemoryRecordsAtHeadSnapshot {
        schema_version: 1,
        conversation_id: conversation_id.clone(),
        source_branch_id: source_branch_id.clone(),
        context_head_message_id: context_head_message_id.cloned(),
        include_invalidated,
        records: evidence,
        snapshot_sha256: String::new(),
    };
    snapshot.snapshot_sha256 = memory_records_at_head_snapshot_sha256(&snapshot)?;
    Ok(MemoryRecordsAtHeadSelection { snapshot, records })
}

fn require_memory_context_head_visible(
    connection: &Connection,
    conversation_id: &ConversationId,
    source_branch_id: &ConversationBranchId,
    context_head_message_id: Option<&MessageId>,
) -> CoreResult<()> {
    let branch_head = connection
        .query_row(
            "SELECT head_message_id
             FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0, source_branch_id.0],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory record branch"))?;
    match (branch_head.as_deref(), context_head_message_id) {
        // `None` is the exact pre-first-message boundary. It remains a valid
        // historical fork point after the source branch has advanced and, by
        // definition, has no visible message-backed memory records.
        (_, None) => {}
        (Some(branch_head), Some(context_head)) => {
            let visible = connection
                .query_row(
                    "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                         SELECT id, parent_id, 0
                         FROM messages
                         WHERE conversation_id = ?1 AND id = ?2
                         UNION ALL
                         SELECT parent.id, parent.parent_id, child.depth + 1
                         FROM messages AS parent
                         JOIN lineage AS child ON child.parent_id = parent.id
                         WHERE parent.conversation_id = ?1
                           AND child.depth < 100000
                     )
                     SELECT EXISTS(SELECT 1 FROM lineage WHERE id = ?3)",
                    params![conversation_id.0, branch_head, context_head.0],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_db_error)?;
            if !visible {
                return Err(CoreError::invalid(
                    "memory context head is not on the selected source branch",
                ));
            }
        }
        (None, Some(_)) => {
            return Err(CoreError::invalid(
                "memory context head does not match the source branch boundary",
            ));
        }
    }
    Ok(())
}

struct RawMemoryRecordAtHead {
    record: RawMemoryRecord,
    record_id: String,
    record_branch_id: String,
    source_start_message_id: String,
    source_end_message_id: String,
    active_revision_sha256: String,
}

fn load_memory_records_at_head(
    connection: &Connection,
    conversation_id: &ConversationId,
    context_head: &MessageId,
    include_invalidated: bool,
) -> CoreResult<(
    Vec<StoredRevision<MemoryRecord>>,
    Vec<MemoryRecordAtHeadEvidence>,
)> {
    let mut statement = connection
        .prepare(
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
             SELECT revision.document_json, state.state_version,
                    state.active_revision_id, record.created_at,
                    state.updated_at, state.deleted_at, state.pinned,
                    state.invalidated_at,
                    state.excluded_from_conversation_at,
                    state.excluded_from_character_at,
                    record.id, record.branch_id,
                    record.source_start_message_id,
                    record.source_end_message_id,
                    revision.content_sha256
             FROM memory_records AS record
             JOIN lineage AS source_start
               ON source_start.id = record.source_start_message_id
             JOIN lineage AS source_end
               ON source_end.id = record.source_end_message_id
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN memory_record_revisions AS revision
               ON revision.id = state.active_revision_id
             WHERE record.conversation_id = ?1
               AND state.deleted_at IS NULL
               AND (?3 OR state.invalidated_at IS NULL)
             ORDER BY record.created_at, record.id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![conversation_id.0, context_head.0, include_invalidated],
            raw_memory_record_at_head,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    if rows.len() > MAX_MEMORY_RECORDS {
        return Err(CoreError::invalid(format!(
            "memory head snapshot exceeds {MAX_MEMORY_RECORDS} records"
        )));
    }
    rows.into_iter().map(decode_memory_record_at_head).collect()
}

fn raw_memory_record_at_head(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawMemoryRecordAtHead> {
    Ok(RawMemoryRecordAtHead {
        record: raw_memory_record(row)?,
        record_id: row.get(10)?,
        record_branch_id: row.get(11)?,
        source_start_message_id: row.get(12)?,
        source_end_message_id: row.get(13)?,
        active_revision_sha256: row.get(14)?,
    })
}

fn decode_memory_record_at_head(
    raw: RawMemoryRecordAtHead,
) -> CoreResult<(StoredRevision<MemoryRecord>, MemoryRecordAtHeadEvidence)> {
    let state_revision = u64_revision(raw.record.state_version)?;
    let active_revision_id = raw.record.active_revision_id.clone();
    let evidence = MemoryRecordAtHeadEvidence {
        record_id: MemoryRecordId::from(raw.record_id),
        record_branch_id: ConversationBranchId(raw.record_branch_id),
        source_start_message_id: MessageId(raw.source_start_message_id),
        source_end_message_id: MessageId(raw.source_end_message_id),
        state_revision,
        active_revision_id,
        active_revision_sha256: raw.active_revision_sha256,
    };
    Ok((decode_memory_record(raw.record)?, evidence))
}

fn list_visible_memory_records(
    storage: &Storage,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    include_invalidated: bool,
) -> CoreResult<Vec<StoredRevision<MemoryRecord>>> {
    let connection = storage.connection()?;
    let branch_exists = connection
        .query_row(
            "SELECT 1 FROM conversation_branches
             WHERE conversation_id = ?1 AND id = ?2",
            params![conversation_id.0, branch_id.0],
            |_| Ok(()),
        )
        .optional()
        .map_err(storage_db_error)?
        .is_some();
    if !branch_exists {
        return Err(not_found("memory record branch"));
    }
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE lineage(id, parent_id) AS (
                 SELECT message.id, message.parent_id
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION
                 SELECT parent.id, parent.parent_id
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1
             )
             SELECT revision.document_json, state.state_version,
                    state.active_revision_id, record.created_at,
                    state.updated_at, state.deleted_at, state.pinned,
                    state.invalidated_at,
                    state.excluded_from_conversation_at,
                    state.excluded_from_character_at
             FROM memory_records AS record
             JOIN lineage AS source_start
               ON source_start.id = record.source_start_message_id
             JOIN lineage AS source_end
               ON source_end.id = record.source_end_message_id
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN memory_record_revisions AS revision
               ON revision.id = state.active_revision_id
             WHERE record.conversation_id = ?1
               AND state.deleted_at IS NULL
               AND (?3 OR state.invalidated_at IS NULL)
             ORDER BY record.created_at, record.id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(
            params![conversation_id.0, branch_id.0, include_invalidated],
            raw_memory_record,
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter().map(decode_memory_record).collect()
}

fn invalidate_memory_range(
    storage: &Storage,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_message_id: &MessageId,
    end_message_id: &MessageId,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<MemoryInvalidationResult> {
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let result = invalidate_memory_range_in_transaction(
        &transaction,
        conversation_id,
        branch_id,
        start_message_id,
        end_message_id,
        invalidated_at,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(result)
}

/// Invalidates memory derived from a lineage range inside an existing write
/// transaction. Callers must invoke this before moving the branch head because
/// the exact removed lineage is resolved from the current head.
pub(crate) fn invalidate_memory_range_in_transaction(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_message_id: &MessageId,
    end_message_id: &MessageId,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<MemoryInvalidationResult> {
    let (start_depth, end_depth) = memory_invalidation_depths(
        transaction,
        conversation_id,
        branch_id,
        start_message_id,
        end_message_id,
    )?;
    let records = memory_records_in_invalidation_range(
        transaction,
        conversation_id,
        branch_id,
        start_depth,
        end_depth,
    )?;
    invalidate_memory_records(
        transaction,
        &records,
        start_message_id,
        end_message_id,
        invalidated_at,
    )?;
    let invalidated_jobs = cancel_memory_jobs_in_range(
        transaction,
        conversation_id,
        branch_id,
        start_depth,
        end_depth,
        invalidated_at,
    )?;
    Ok(MemoryInvalidationResult {
        invalidated_records: u64::try_from(records.len())
            .map_err(|_| CoreError::internal("memory invalidation count overflow"))?,
        invalidated_jobs: u64::try_from(invalidated_jobs)
            .map_err(|_| CoreError::internal("memory job invalidation count overflow"))?,
    })
}

fn memory_invalidation_depths(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_message_id: &MessageId,
    end_message_id: &MessageId,
) -> CoreResult<(i64, i64)> {
    let bounds = transaction
        .query_row(
            "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                 SELECT message.id, message.parent_id, 0
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             )
             SELECT MAX(CASE WHEN id = ?3 THEN depth END),
                    MAX(CASE WHEN id = ?4 THEN depth END)
             FROM lineage",
            params![
                conversation_id.0,
                branch_id.0,
                start_message_id.0,
                end_message_id.0
            ],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(storage_db_error)?;
    let (Some(start_depth), Some(end_depth)) = bounds else {
        return Err(CoreError::invalid(
            "memory invalidation range is not on the selected branch",
        ));
    };
    if start_depth < end_depth {
        Err(CoreError::invalid(
            "memory invalidation start must not follow its end",
        ))
    } else {
        Ok((start_depth, end_depth))
    }
}

fn memory_records_in_invalidation_range(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_depth: i64,
    end_depth: i64,
) -> CoreResult<Vec<(String, i64, String)>> {
    let mut statement = transaction
        .prepare(
            "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                 SELECT message.id, message.parent_id, 0
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             )
             SELECT record.id, state.state_version, state.active_revision_id
             FROM memory_records AS record
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN lineage AS source_start ON source_start.id = record.source_start_message_id
             JOIN lineage AS source_end ON source_end.id = record.source_end_message_id
             WHERE record.conversation_id = ?1 AND record.branch_id = ?2
               AND state.deleted_at IS NULL AND state.invalidated_at IS NULL
               AND source_start.depth >= ?3 AND ?4 >= source_end.depth
             ORDER BY record.id",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map(
            params![conversation_id.0, branch_id.0, end_depth, start_depth],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)
}

fn invalidate_memory_records(
    transaction: &Transaction<'_>,
    records: &[(String, i64, String)],
    start_message_id: &MessageId,
    end_message_id: &MessageId,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<()> {
    for (record_id, state_version, active_revision_id) in records {
        let next = state_version
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("memory state revision overflow"))?;
        transaction
            .execute(
                "UPDATE memory_record_state
                 SET invalidated_at = ?2, invalidation_reason = 'source_range_changed',
                     state_version = ?3, updated_at = ?2
                 WHERE record_id = ?1 AND state_version = ?4
                   AND invalidated_at IS NULL AND deleted_at IS NULL",
                params![record_id, invalidated_at.to_rfc3339(), next, state_version],
            )
            .map_err(storage_db_error)?;
        append_memory_event(
            transaction,
            record_id,
            "invalidated",
            Some(active_revision_id),
            Some(active_revision_id),
            serde_json::json!({
                "start_message_id": start_message_id.0,
                "end_message_id": end_message_id.0,
            }),
            invalidated_at,
        )?;
    }
    Ok(())
}

fn cancel_memory_jobs_in_range(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    start_depth: i64,
    end_depth: i64,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<usize> {
    transaction
        .execute(
            "WITH RECURSIVE lineage(id, parent_id, depth) AS (
                 SELECT message.id, message.parent_id, 0
                 FROM conversation_branches AS branch
                 JOIN messages AS message
                   ON message.conversation_id = branch.conversation_id
                  AND message.id = branch.head_message_id
                 WHERE branch.conversation_id = ?1 AND branch.id = ?2
                 UNION ALL
                 SELECT parent.id, parent.parent_id, child.depth + 1
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1 AND child.depth < 100000
             ), affected AS (
                 SELECT job.id FROM memory_jobs AS job
                 JOIN lineage AS source_start ON source_start.id = job.source_start_message_id
                 JOIN lineage AS source_end ON source_end.id = job.source_end_message_id
                 WHERE job.conversation_id = ?1 AND job.branch_id = ?2
                   AND job.state IN ('queued', 'running', 'interrupted')
                   AND source_start.depth >= ?3 AND ?4 >= source_end.depth
             )
             UPDATE memory_jobs
             SET state = 'cancelled', revision = revision + 1,
                 finished_at = ?5, failure_json = NULL, updated_at = ?5
             WHERE id IN (SELECT id FROM affected)",
            params![
                conversation_id.0,
                branch_id.0,
                end_depth,
                start_depth,
                invalidated_at.to_rfc3339()
            ],
        )
        .map_err(storage_db_error)
}
