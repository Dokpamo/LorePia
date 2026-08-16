//! Durable, exactly-once intents for provider-native memory query embeddings.

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, MemoryProfileId,
    MessageId, ModelRouteId,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Storage, database::storage_db_error};

const MAX_DIMENSIONS: usize = 32_768;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryQueryEmbeddingStatus {
    Queued,
    Running,
    Interrupted,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryQueryEmbeddingIntent {
    pub id: String,
    pub idempotency_key: String,
    pub memory_profile_id: MemoryProfileId,
    pub memory_profile_revision_id: String,
    pub task_profile_revision_id: String,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub source_start_message_id: MessageId,
    pub source_end_message_id: MessageId,
    pub query_sha256: String,
    pub vector_space_sha256: String,
    pub model_route_id: ModelRouteId,
    pub dimensions: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredMemoryQueryEmbedding {
    pub intent: MemoryQueryEmbeddingIntent,
    pub status: MemoryQueryEmbeddingStatus,
    pub revision: u64,
    pub attempts: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub error_code: Option<String>,
    pub values: Option<Vec<f32>>,
    pub vector_sha256: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryQueryEmbeddingEnqueueResult {
    pub entry: StoredMemoryQueryEmbedding,
    pub exact_replay: bool,
}

impl Storage {
    pub fn get_memory_query_embedding(&self, id: &str) -> CoreResult<StoredMemoryQueryEmbedding> {
        validate_text("query embedding id", id, 256)?;
        let connection = self.connection()?;
        load_by_id_or_key(&connection, Some(id), None)?
            .ok_or_else(|| not_found("memory query embedding"))
    }

    /// Crash recovery for intents left running by a prior process. Every row
    /// becomes interrupted (never queued), so startup cannot redispatch an
    /// ambiguous provider request.
    pub fn recover_running_memory_query_embeddings(
        &self,
        recovered_at: DateTime<Utc>,
    ) -> CoreResult<Vec<StoredMemoryQueryEmbedding>> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT id FROM memory_query_embeddings
                     WHERE state = 'running' AND started_at <= ?1
                     ORDER BY created_at, id",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([recovered_at.to_rfc3339()], |row| row.get::<_, String>(0))
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        if !ids.is_empty() {
            transaction
                .execute(
                    "UPDATE memory_query_embeddings
                     SET state = 'interrupted', revision = revision + 1,
                         error_code = 'provider_unknown_outcome',
                         updated_at = ?1
                     WHERE state = 'running' AND started_at <= ?1",
                    [recovered_at.to_rfc3339()],
                )
                .map_err(storage_db_error)?;
        }
        let recovered = ids
            .iter()
            .map(|id| {
                load_by_id_or_key(&transaction, Some(id), None)?
                    .ok_or_else(|| corrupted("recovered memory query embedding is missing"))
            })
            .collect::<CoreResult<Vec<_>>>()?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(recovered)
    }

    /// Inserts the durable provider intent before credential access or network
    /// dispatch. An exact replay returns the existing status/vector unchanged.
    pub fn enqueue_memory_query_embedding(
        &self,
        intent: &MemoryQueryEmbeddingIntent,
    ) -> CoreResult<MemoryQueryEmbeddingEnqueueResult> {
        validate_intent(intent)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        validate_intent_bindings(&transaction, intent)?;
        if let Some(existing) = load_by_id_or_key(
            &transaction,
            Some(&intent.id),
            Some(&intent.idempotency_key),
        )? {
            if !same_immutable_intent(&existing.intent, intent) {
                return Err(conflict(
                    "memory query embedding idempotency key belongs to different immutable input",
                ));
            }
            transaction.commit().map_err(storage_db_error)?;
            return Ok(MemoryQueryEmbeddingEnqueueResult {
                entry: existing,
                exact_replay: true,
            });
        }
        transaction
            .execute(
                "INSERT INTO memory_query_embeddings
                 (id, idempotency_key, memory_profile_revision_id,
                  task_profile_revision_id, conversation_id, branch_id,
                  source_start_message_id, source_end_message_id, query_sha256,
                  vector_space_sha256, model_route_id, dimensions, state,
                  revision, attempts, started_at, finished_at, error_code,
                  encoding, vector_blob, vector_sha256, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                         ?12, 'queued', 1, 0, NULL, NULL, NULL, NULL, NULL,
                         NULL, ?13, ?13)",
                params![
                    intent.id,
                    intent.idempotency_key,
                    intent.memory_profile_revision_id,
                    intent.task_profile_revision_id,
                    intent.conversation_id.0,
                    intent.branch_id.0,
                    intent.source_start_message_id.0,
                    intent.source_end_message_id.0,
                    intent.query_sha256,
                    intent.vector_space_sha256,
                    intent.model_route_id.as_str(),
                    intent.dimensions,
                    intent.created_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        let entry = load_by_id_or_key(&transaction, Some(&intent.id), None)?
            .ok_or_else(|| corrupted("inserted memory query embedding is missing"))?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(MemoryQueryEmbeddingEnqueueResult {
            entry,
            exact_replay: false,
        })
    }

    /// Claims one exact queued intent. Only the successful CAS caller may
    /// obtain a credential and dispatch the provider request.
    pub fn claim_memory_query_embedding(
        &self,
        id: &str,
        expected_revision: u64,
        started_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryQueryEmbedding> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE memory_query_embeddings
                 SET state = 'running', revision = revision + 1,
                     attempts = attempts + 1, started_at = ?3,
                     updated_at = ?3
                 WHERE id = ?1 AND state = 'queued' AND revision = ?2
                   AND ?3 >= created_at",
                params![id, to_i64(expected_revision)?, started_at.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        ensure_cas(changed, "memory query embedding claim")?;
        load_by_id_or_key(&connection, Some(id), None)?
            .ok_or_else(|| corrupted("claimed memory query embedding is missing"))
    }

    /// Commits the exact vector and terminal status in one CAS write.
    pub fn complete_memory_query_embedding(
        &self,
        id: &str,
        expected_revision: u64,
        values: &[f32],
        finished_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryQueryEmbedding> {
        let (vector_blob, vector_sha256) = encode_vector(values)?;
        let connection = self.connection()?;
        let entry = load_by_id_or_key(&connection, Some(id), None)?
            .ok_or_else(|| not_found("memory query embedding"))?;
        if entry.status == MemoryQueryEmbeddingStatus::Succeeded {
            if entry.revision == expected_revision.saturating_add(1)
                && entry.values.as_deref() == Some(values)
                && entry.finished_at == Some(finished_at)
            {
                return Ok(entry);
            }
            return Err(conflict(
                "memory query embedding completion conflicts with terminal output",
            ));
        }
        if entry.status != MemoryQueryEmbeddingStatus::Running
            || entry.revision != expected_revision
            || values.len() != usize::try_from(entry.intent.dimensions).unwrap_or(usize::MAX)
        {
            return Err(conflict(
                "memory query embedding completion lost its running CAS or dimension",
            ));
        }
        let changed = connection
            .execute(
                "UPDATE memory_query_embeddings
                 SET state = 'succeeded', revision = revision + 1,
                     finished_at = ?3, error_code = NULL, encoding = 'f32le',
                     vector_blob = ?4, vector_sha256 = ?5, updated_at = ?3
                 WHERE id = ?1 AND state = 'running' AND revision = ?2
                   AND ?3 >= started_at",
                params![
                    id,
                    to_i64(expected_revision)?,
                    finished_at.to_rfc3339(),
                    vector_blob,
                    vector_sha256,
                ],
            )
            .map_err(storage_db_error)?;
        ensure_cas(changed, "memory query embedding completion")?;
        load_by_id_or_key(&connection, Some(id), None)?
            .ok_or_else(|| corrupted("completed memory query embedding is missing"))
    }

    pub fn interrupt_memory_query_embedding(
        &self,
        id: &str,
        expected_revision: u64,
        error_code: &str,
        interrupted_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryQueryEmbedding> {
        transition_running_without_vector(
            self,
            id,
            expected_revision,
            "interrupted",
            Some(error_code),
            interrupted_at,
            false,
        )
    }

    pub fn fail_memory_query_embedding(
        &self,
        id: &str,
        expected_revision: u64,
        error_code: &str,
        finished_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryQueryEmbedding> {
        transition_running_without_vector(
            self,
            id,
            expected_revision,
            "failed",
            Some(error_code),
            finished_at,
            true,
        )
    }

    pub fn cancel_memory_query_embedding(
        &self,
        id: &str,
        expected_revision: u64,
        finished_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryQueryEmbedding> {
        transition_running_without_vector(
            self,
            id,
            expected_revision,
            "cancelled",
            None,
            finished_at,
            true,
        )
    }

    /// The sole retry seam. It requires an explicit caller action and exact
    /// retryable revision; ordinary enqueue/prepare never invokes it.
    pub fn retry_memory_query_embedding(
        &self,
        id: &str,
        expected_revision: u64,
        available_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryQueryEmbedding> {
        let connection = self.connection()?;
        let changed = connection
            .execute(
                "UPDATE memory_query_embeddings
                 SET state = 'queued', revision = revision + 1,
                     started_at = NULL, finished_at = NULL,
                     error_code = NULL, updated_at = ?3
                 WHERE id = ?1
                   AND state IN ('interrupted', 'failed', 'cancelled')
                   AND revision = ?2
                   AND ?3 >= updated_at",
                params![id, to_i64(expected_revision)?, available_at.to_rfc3339()],
            )
            .map_err(storage_db_error)?;
        ensure_cas(changed, "memory query embedding explicit retry")?;
        load_by_id_or_key(&connection, Some(id), None)?
            .ok_or_else(|| corrupted("retried memory query embedding is missing"))
    }

    /// Compatibility name retained for callers that specifically recovered an
    /// ambiguous outcome. Validation still occurs against the stored state.
    pub fn retry_interrupted_memory_query_embedding(
        &self,
        id: &str,
        expected_revision: u64,
        available_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryQueryEmbedding> {
        self.retry_memory_query_embedding(id, expected_revision, available_at)
    }

    /// Bounded, credential-free product surface for explicit user retry.
    pub fn list_retryable_memory_query_embeddings(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<StoredMemoryQueryEmbedding>> {
        if limit == 0 || limit > 256 {
            return Err(CoreError::invalid(
                "query embedding retry list limit must be between 1 and 256",
            ));
        }
        let connection = self.connection()?;
        let ids = {
            let mut statement = connection
                .prepare(
                    "SELECT id FROM memory_query_embeddings
                     WHERE conversation_id = ?1 AND branch_id = ?2
                       AND state IN ('interrupted', 'failed', 'cancelled')
                     ORDER BY updated_at DESC, id
                     LIMIT ?3",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map(
                    params![conversation_id.0, branch_id.0, i64::from(limit)],
                    |row| row.get::<_, String>(0),
                )
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        ids.iter()
            .map(|id| {
                load_by_id_or_key(&connection, Some(id), None)?
                    .ok_or_else(|| corrupted("listed memory query embedding is missing"))
            })
            .collect()
    }
}

fn transition_running_without_vector(
    storage: &Storage,
    id: &str,
    expected_revision: u64,
    state: &str,
    error_code: Option<&str>,
    changed_at: DateTime<Utc>,
    terminal: bool,
) -> CoreResult<StoredMemoryQueryEmbedding> {
    if let Some(code) = error_code {
        validate_error_code(code)?;
    }
    let connection = storage.connection()?;
    let changed = connection
        .execute(
            "UPDATE memory_query_embeddings
             SET state = ?3, revision = revision + 1,
                 finished_at = CASE WHEN ?4 THEN ?6 ELSE NULL END,
                 error_code = ?5, updated_at = ?6
             WHERE id = ?1 AND state = 'running' AND revision = ?2
               AND ?6 >= started_at",
            params![
                id,
                to_i64(expected_revision)?,
                state,
                terminal,
                error_code,
                changed_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    ensure_cas(changed, "memory query embedding state transition")?;
    load_by_id_or_key(&connection, Some(id), None)?
        .ok_or_else(|| corrupted("transitioned memory query embedding is missing"))
}

fn validate_intent(intent: &MemoryQueryEmbeddingIntent) -> CoreResult<()> {
    for (label, value) in [
        ("query embedding id", intent.id.as_str()),
        (
            "query embedding idempotency key",
            intent.idempotency_key.as_str(),
        ),
        (
            "memory profile revision",
            intent.memory_profile_revision_id.as_str(),
        ),
        (
            "task profile revision",
            intent.task_profile_revision_id.as_str(),
        ),
    ] {
        validate_text(label, value, 256)?;
    }
    validate_sha256("query digest", &intent.query_sha256)?;
    validate_sha256("vector-space digest", &intent.vector_space_sha256)?;
    let dimensions = usize::try_from(intent.dimensions)
        .map_err(|_| CoreError::invalid("query embedding dimensions are invalid"))?;
    if dimensions == 0 || dimensions > MAX_DIMENSIONS {
        return Err(CoreError::invalid(
            "query embedding dimensions must be between 1 and 32768",
        ));
    }
    Ok(())
}

fn same_immutable_intent(
    left: &MemoryQueryEmbeddingIntent,
    right: &MemoryQueryEmbeddingIntent,
) -> bool {
    left.id == right.id
        && left.idempotency_key == right.idempotency_key
        && left.memory_profile_id == right.memory_profile_id
        && left.memory_profile_revision_id == right.memory_profile_revision_id
        && left.task_profile_revision_id == right.task_profile_revision_id
        && left.conversation_id == right.conversation_id
        && left.branch_id == right.branch_id
        && left.source_start_message_id == right.source_start_message_id
        && left.source_end_message_id == right.source_end_message_id
        && left.query_sha256 == right.query_sha256
        && left.vector_space_sha256 == right.vector_space_sha256
        && left.model_route_id == right.model_route_id
        && left.dimensions == right.dimensions
}

fn validate_intent_bindings(
    connection: &rusqlite::Connection,
    intent: &MemoryQueryEmbeddingIntent,
) -> CoreResult<()> {
    let binding = connection
        .query_row(
            "SELECT profile.memory_profile_id, profile.embedding_task_profile_revision_id,
                    task.task_kind, task.model_route_id
             FROM memory_profile_revisions AS profile
             JOIN task_profile_revisions AS task ON task.revision_id = ?2
             WHERE profile.revision_id = ?1",
            params![
                intent.memory_profile_revision_id,
                intent.task_profile_revision_id,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory query embedding policy revisions"))?;
    if binding.0 != intent.memory_profile_id.as_str()
        || binding.1.as_deref() != Some(intent.task_profile_revision_id.as_str())
        || binding.2 != "memory_embedding"
        || binding.3 != intent.model_route_id.as_str()
    {
        return Err(CoreError::invalid(
            "query embedding intent differs from its exact memory/task policy",
        ));
    }
    let source_is_visible = connection
        .query_row(
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
             SELECT EXISTS(SELECT 1 FROM lineage WHERE id = ?3)
                AND EXISTS(SELECT 1 FROM lineage WHERE id = ?4)
                AND EXISTS(
                    WITH RECURSIVE source_lineage(id, parent_id) AS (
                        SELECT id, parent_id FROM messages
                        WHERE conversation_id = ?1 AND id = ?4
                        UNION
                        SELECT parent.id, parent.parent_id
                        FROM messages AS parent
                        JOIN source_lineage AS child ON child.parent_id = parent.id
                        WHERE parent.conversation_id = ?1
                    )
                    SELECT 1 FROM source_lineage WHERE id = ?3
                )",
            params![
                intent.conversation_id.0,
                intent.branch_id.0,
                intent.source_start_message_id.0,
                intent.source_end_message_id.0,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if !source_is_visible {
        return Err(CoreError::invalid(
            "query embedding source range is not ordered and visible on the branch",
        ));
    }
    Ok(())
}

fn load_by_id_or_key(
    connection: &rusqlite::Connection,
    id: Option<&str>,
    key: Option<&str>,
) -> CoreResult<Option<StoredMemoryQueryEmbedding>> {
    let row = connection
        .query_row(
            "SELECT query.id, query.idempotency_key,
                    profile.memory_profile_id,
                    query.memory_profile_revision_id,
                    query.task_profile_revision_id, query.conversation_id,
                    query.branch_id, query.source_start_message_id,
                    query.source_end_message_id, query.query_sha256,
                    query.vector_space_sha256, query.model_route_id,
                    query.dimensions, query.state, query.revision,
                    query.attempts, query.started_at, query.finished_at,
                    query.error_code, query.vector_blob, query.vector_sha256,
                    query.created_at, query.updated_at
             FROM memory_query_embeddings AS query
             JOIN memory_profile_revisions AS profile
               ON profile.revision_id = query.memory_profile_revision_id
             WHERE (?1 IS NOT NULL AND query.id = ?1)
                OR (?2 IS NOT NULL AND query.idempotency_key = ?2)
             ORDER BY query.id
             LIMIT 1",
            params![id, key],
            read_memory_query_embedding_row,
        )
        .optional()
        .map_err(storage_db_error)?;
    row.map(decode_memory_query_embedding_row).transpose()
}

struct RawMemoryQueryEmbeddingRow {
    id: String,
    idempotency_key: String,
    memory_profile_id: String,
    memory_profile_revision_id: String,
    task_profile_revision_id: String,
    conversation_id: String,
    branch_id: String,
    source_start_message_id: String,
    source_end_message_id: String,
    query_sha256: String,
    vector_space_sha256: String,
    model_route_id: String,
    dimensions: i64,
    status: String,
    revision: i64,
    attempts: i64,
    started_at: Option<String>,
    finished_at: Option<String>,
    error_code: Option<String>,
    vector_blob: Option<Vec<u8>>,
    vector_sha256: Option<String>,
    created_at: String,
    updated_at: String,
}

fn read_memory_query_embedding_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<RawMemoryQueryEmbeddingRow> {
    Ok(RawMemoryQueryEmbeddingRow {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        memory_profile_id: row.get(2)?,
        memory_profile_revision_id: row.get(3)?,
        task_profile_revision_id: row.get(4)?,
        conversation_id: row.get(5)?,
        branch_id: row.get(6)?,
        source_start_message_id: row.get(7)?,
        source_end_message_id: row.get(8)?,
        query_sha256: row.get(9)?,
        vector_space_sha256: row.get(10)?,
        model_route_id: row.get(11)?,
        dimensions: row.get(12)?,
        status: row.get(13)?,
        revision: row.get(14)?,
        attempts: row.get(15)?,
        started_at: row.get(16)?,
        finished_at: row.get(17)?,
        error_code: row.get(18)?,
        vector_blob: row.get(19)?,
        vector_sha256: row.get(20)?,
        created_at: row.get(21)?,
        updated_at: row.get(22)?,
    })
}

fn decode_memory_query_embedding_row(
    row: RawMemoryQueryEmbeddingRow,
) -> CoreResult<StoredMemoryQueryEmbedding> {
    let dimensions = u32::try_from(row.dimensions)
        .map_err(|_| corrupted("stored query embedding dimensions are invalid"))?;
    let values = row
        .vector_blob
        .as_deref()
        .map(|bytes| decode_vector(dimensions, bytes, row.vector_sha256.as_deref()))
        .transpose()?;
    Ok(StoredMemoryQueryEmbedding {
        intent: MemoryQueryEmbeddingIntent {
            id: row.id,
            idempotency_key: row.idempotency_key,
            memory_profile_id: MemoryProfileId::from(row.memory_profile_id),
            memory_profile_revision_id: row.memory_profile_revision_id,
            task_profile_revision_id: row.task_profile_revision_id,
            conversation_id: ConversationId(row.conversation_id),
            branch_id: ConversationBranchId(row.branch_id),
            source_start_message_id: MessageId(row.source_start_message_id),
            source_end_message_id: MessageId(row.source_end_message_id),
            query_sha256: row.query_sha256,
            vector_space_sha256: row.vector_space_sha256,
            model_route_id: ModelRouteId::from(row.model_route_id),
            dimensions,
            created_at: parse_time("query embedding created_at", &row.created_at)?,
        },
        status: parse_status(&row.status)?,
        revision: u64::try_from(row.revision)
            .map_err(|_| corrupted("stored query embedding revision is invalid"))?,
        attempts: u32::try_from(row.attempts)
            .map_err(|_| corrupted("stored query embedding attempts are invalid"))?,
        started_at: row
            .started_at
            .as_deref()
            .map(|value| parse_time("query embedding started_at", value))
            .transpose()?,
        finished_at: row
            .finished_at
            .as_deref()
            .map(|value| parse_time("query embedding finished_at", value))
            .transpose()?,
        error_code: row.error_code,
        values,
        vector_sha256: row.vector_sha256,
        updated_at: parse_time("query embedding updated_at", &row.updated_at)?,
    })
}

fn encode_vector(values: &[f32]) -> CoreResult<(Vec<u8>, String)> {
    if values.is_empty()
        || values.len() > MAX_DIMENSIONS
        || values.iter().any(|value| !value.is_finite())
    {
        return Err(CoreError::invalid(
            "query embedding vector dimensions or values are invalid",
        ));
    }
    let norm = values
        .iter()
        .map(|value| f64::from(*value).powi(2))
        .sum::<f64>();
    if !norm.is_finite() || norm <= f64::EPSILON {
        return Err(CoreError::invalid(
            "query embedding vector must have a non-zero finite norm",
        ));
    }
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let digest = format!("{:x}", Sha256::digest(&bytes));
    Ok((bytes, digest))
}

fn decode_vector(
    dimensions: u32,
    bytes: &[u8],
    expected_sha256: Option<&str>,
) -> CoreResult<Vec<f32>> {
    let expected_len = usize::try_from(dimensions)
        .map_err(|_| corrupted("stored query embedding dimensions are invalid"))?
        .checked_mul(4)
        .ok_or_else(|| corrupted("stored query embedding vector size overflow"))?;
    if bytes.len() != expected_len
        || expected_sha256 != Some(format!("{:x}", Sha256::digest(bytes)).as_str())
    {
        return Err(corrupted(
            "stored query embedding vector bytes or digest are invalid",
        ));
    }
    let values = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err(corrupted(
            "stored query embedding contains a non-finite value",
        ));
    }
    Ok(values)
}

fn parse_status(value: &str) -> CoreResult<MemoryQueryEmbeddingStatus> {
    match value {
        "queued" => Ok(MemoryQueryEmbeddingStatus::Queued),
        "running" => Ok(MemoryQueryEmbeddingStatus::Running),
        "interrupted" => Ok(MemoryQueryEmbeddingStatus::Interrupted),
        "succeeded" => Ok(MemoryQueryEmbeddingStatus::Succeeded),
        "failed" => Ok(MemoryQueryEmbeddingStatus::Failed),
        "cancelled" => Ok(MemoryQueryEmbeddingStatus::Cancelled),
        _ => Err(corrupted("stored query embedding status is invalid")),
    }
}

fn validate_text(label: &str, value: &str, max: usize) -> CoreResult<()> {
    if value.trim().is_empty() || value.len() > max || value.contains('\0') {
        Err(CoreError::invalid(format!("{label} is invalid")))
    } else {
        Ok(())
    }
}

fn validate_sha256(label: &str, value: &str) -> CoreResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Err(CoreError::invalid(format!(
            "{label} is not lowercase SHA-256"
        )))
    } else {
        Ok(())
    }
}

fn validate_error_code(value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        Err(CoreError::invalid("query embedding error code is invalid"))
    } else {
        Ok(())
    }
}

fn parse_time(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|_| corrupted(format!("{label} is invalid")))
}

fn to_i64(value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid("revision exceeds SQLite range"))
}

fn ensure_cas(changed: usize, operation: &str) -> CoreResult<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(conflict(format!("{operation} lost its expected revision")))
    }
}

fn conflict(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageUnavailable, message, true)
}

fn not_found(label: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{label} was not found"),
        false,
    )
}

fn corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}
