//! Durable memory-work queue operations.
//!
//! The queue is deliberately separate from memory-record persistence.  A job
//! binds to immutable memory/task-profile revisions and to a caller-reviewed
//! input fingerprint before it can be claimed.  Claiming and all state
//! transitions use `SQLite` compare-and-swap updates inside `BEGIN IMMEDIATE`
//! transactions, so multiple local workers cannot exceed the persisted task
//! limits.

use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, MemoryJob,
    MemoryJobId, MemoryJobKind, MemoryJobStatus, MemoryKind, MemoryProfile, MemoryRecord,
    MemoryRecordId, MessageId, ModelRouteId, TaskProfile, ValidateOrchestration, VersionedJson,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    MemoryEmbeddingRecord, ObjectRevision, Storage, StoredRevision, database::storage_db_error,
};

const QUEUE_PAYLOAD_SCHEMA_VERSION: u32 = 1;
const MAX_QUEUE_PAYLOAD_BYTES: usize = 1024 * 1024;
const MAX_QUEUE_JSON_DEPTH: usize = 32;
const MAX_QUEUE_JSON_NODES: usize = 100_000;
const MAX_MEMORY_JOB_ATTEMPTS: u32 = 32;
const MAX_MEMORY_EMBEDDING_DIMENSIONS: usize = 32_768;
const MAX_MEMORY_EMBEDDING_CANDIDATES: u32 = 2_048;
const MAX_MEMORY_EMBEDDING_QUERY_BYTES: usize = 16 * 1024 * 1024;
const MAX_VISIBLE_MEMORY_SUMMARY_JOBS: usize = 100_000;
const MAX_INTERRUPTED_MEMORY_JOB_PAGE: u32 = 256;

/// Immutable inputs used to enqueue one durable memory job.
///
/// Summary and embedding jobs require both revision identifiers.
/// `invalidate_range` is deliberately rejected by this provider-work queue:
/// rewind/edit invalidation belongs inside the branch-mutation transaction.
/// The versioned payload is intended for bounded provenance (source hashes,
/// exact transform revisions and traces, capability snapshots, and variable
/// hashes), never raw credentials.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJobEnqueue {
    pub job: MemoryJob,
    #[serde(default)]
    pub memory_profile_revision_id: Option<String>,
    #[serde(default)]
    pub task_profile_revision_id: Option<String>,
    pub input_fingerprint_sha256: String,
    pub payload: VersionedJson,
    pub available_at: DateTime<Utc>,
}

/// Durable queue state returned to a worker or inspector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredMemoryJobQueueEntry {
    pub job: MemoryJob,
    pub revision: u64,
    pub memory_profile_revision_id: Option<String>,
    pub task_profile_revision_id: Option<String>,
    pub input_fingerprint_sha256: String,
    pub payload: VersionedJson,
    /// Immutable snapshots resolved by the exact revision identifiers above.
    #[serde(default)]
    pub memory_profile_revision: Option<ObjectRevision<MemoryProfile>>,
    #[serde(default)]
    pub task_profile_revision: Option<ObjectRevision<TaskProfile>>,
    pub available_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result_record_id: Option<MemoryRecordId>,
    /// Every provider-task start retained by the queue payload.  This keeps
    /// rate-window accounting durable across an explicit interrupted retry.
    #[serde(default)]
    pub attempt_started_at: Vec<DateTime<Utc>>,
    /// Bounded audit history for ambiguous provider outcomes and process
    /// recovery.  Interrupted jobs are never automatically requeued.
    #[serde(default)]
    pub interruptions: Vec<MemoryJobInterruption>,
}

/// One explicit reason a running job moved to `interrupted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJobInterruption {
    pub interrupted_at: DateTime<Utc>,
    #[serde(default)]
    pub error_code: Option<String>,
}

/// Result of an idempotent enqueue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryJobEnqueueResult {
    pub entry: StoredMemoryJobQueueEntry,
    /// `true` only when the exact idempotency key, immutable bindings,
    /// fingerprint, and versioned payload already existed.
    pub exact_replay: bool,
}

/// Legal terminal outcomes for a claimed memory job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemoryJobFinish {
    Succeeded {
        #[serde(default)]
        result_record_id: Option<MemoryRecordId>,
    },
    Failed {
        /// Stable, bounded machine code.  Free-form provider text is excluded.
        error_code: String,
    },
    Cancelled,
}

/// Atomic result of committing a generated summary record and its queue job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySummaryJobCompletion {
    pub job: StoredMemoryJobQueueEntry,
    pub record: StoredRevision<MemoryRecord>,
    /// Exact embedding job inserted in the same writer transaction, when the
    /// immutable memory-profile revision configures one.
    #[serde(default)]
    pub embedding_job: Option<StoredMemoryJobQueueEntry>,
    /// `true` when this exact expected running revision and output record had
    /// already committed successfully.
    pub exact_replay: bool,
}

/// Core-reviewed embedding work to attach to a summary completion.
///
/// The source memory-record revision does not exist until the surrounding
/// summary transaction inserts it. Storage fills that one field, computes the
/// canonical payload fingerprint, validates the exact memory/task-profile
/// relation and route, and inserts the queue row before committing either
/// result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEmbeddingJobSeed {
    pub job: MemoryJob,
    pub memory_profile_revision_id: String,
    pub task_profile_revision_id: String,
    pub model_route_id: ModelRouteId,
    pub dimensions: u32,
    /// Exact provider-native vector space resolved before the durable intent
    /// is inserted. This binds mutable route/template/manifest projections in
    /// addition to the immutable task-profile revision.
    pub vector_space_sha256: String,
    pub available_at: DateTime<Utc>,
}

/// Immutable embedding input sealed into an embedding job's versioned payload.
///
/// The exact memory-record revision prevents a later user edit from silently
/// changing what was embedded. The route and dimensions are likewise part of
/// the queue fingerprint and cannot be selected after the provider call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEmbeddingJobInput {
    pub memory_record_revision_id: String,
    pub model_route_id: ModelRouteId,
    pub dimensions: u32,
    pub vector_space_sha256: String,
}

/// One immutable embedding together with every normalized binding needed to
/// verify it independently of mutable memory-record state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredMemoryEmbedding {
    pub value: MemoryEmbeddingRecord,
    pub memory_record_revision_id: String,
    pub task_profile_revision_id: String,
    pub vector_space_sha256: String,
    pub vector_sha256: String,
}

/// Atomic result of committing an embedding and its queue job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEmbeddingJobCompletion {
    pub job: StoredMemoryJobQueueEntry,
    pub embedding: StoredMemoryEmbedding,
    /// `true` only when the same running revision, finish timestamp, embedding
    /// id, exact bindings, dimensions, and vector bytes had already committed.
    pub exact_replay: bool,
}

/// Bounded, exact-version semantic query.
///
/// Candidate loading is scoped to active visible memory revisions and requires
/// an exact task-profile revision, model route, and vector dimension. Cosine
/// scoring happens in Rust; raw `SQLite` never executes extension code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEmbeddingQuery {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    /// Exact historical head whose ancestor lineage bounds candidates.
    pub context_head_message_id: MessageId,
    pub task_profile_revision_id: String,
    pub model_route_id: ModelRouteId,
    pub dimensions: u32,
    pub vector_space_sha256: String,
    pub values: Vec<f32>,
    pub candidate_limit: u32,
    pub result_limit: u32,
}

/// One deterministic cosine result represented as fixed-point millionths in
/// the memory engine's `0..=1` score domain. Negative cosine values normalize
/// to zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryEmbeddingMatch {
    pub embedding_id: String,
    pub memory_record_id: MemoryRecordId,
    pub memory_record_revision_id: String,
    pub vector_sha256: String,
    pub similarity_millionths: u32,
}

/// Closed user-edit surface for a memory record.
///
/// Identity, kind, source lineage, structured data, embedding linkage,
/// provenance, creation time, and invalidation state are intentionally absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryRecordUserPatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub importance: Option<u8>,
    #[serde(default)]
    pub keywords: Option<Vec<String>>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub excluded_from_conversation: Option<bool>,
    #[serde(default)]
    pub excluded_from_character: Option<bool>,
}

/// Separate exclusion controls used by room- and character-level UI actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryRecordExclusionScope {
    Conversation,
    Character,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QueuePayload {
    queue_schema_version: u32,
    job: MemoryJob,
    input: VersionedJson,
    #[serde(default)]
    attempt_started_at: Vec<DateTime<Utc>>,
    #[serde(default)]
    interruptions: Vec<MemoryJobInterruption>,
}

#[derive(Debug)]
struct QueueRow {
    id: String,
    idempotency_key: String,
    job_kind: String,
    memory_profile_revision_id: Option<String>,
    task_profile_revision_id: Option<String>,
    conversation_id: String,
    branch_id: String,
    source_start_message_id: String,
    source_end_message_id: String,
    input_fingerprint_sha256: String,
    state: String,
    revision: i64,
    attempts: i64,
    available_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    result_record_id: Option<String>,
    failure_json: Option<String>,
    payload_json: String,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct MemoryEmbeddingCandidate {
    embedding_id: String,
    record_id: String,
    revision_id: String,
    vector_sha256: String,
    vector_blob: Vec<u8>,
}

#[derive(Debug)]
struct UserMemoryRecordState {
    stored: StoredRevision<MemoryRecord>,
    content_revision_no: u64,
    invalidation_reason: Option<String>,
    excluded_from_conversation_at: Option<DateTime<Utc>>,
    excluded_from_character_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy)]
struct OwnedMemoryRecordTarget<'a> {
    conversation_id: &'a ConversationId,
    branch_id: &'a ConversationBranchId,
    id: &'a MemoryRecordId,
}

#[derive(Debug)]
struct RawUserMemoryRecordState {
    conversation_id: String,
    branch_id: String,
    source_start_message_id: String,
    source_end_message_id: String,
    kind: String,
    record_created_at: String,
    state_revision: i64,
    active_revision_id: String,
    state_updated_at: String,
    deleted_at: Option<String>,
    pinned: bool,
    invalidated_at: Option<String>,
    invalidation_reason: Option<String>,
    excluded_from_conversation_at: Option<String>,
    excluded_from_character_at: Option<String>,
    document_json: String,
    content_sha256: String,
    content_revision_no: i64,
}

impl Storage {
    /// Enqueues a reviewed memory input once.
    ///
    /// Reusing an idempotency key returns the existing row only when all
    /// immutable queue inputs are byte-for-byte equivalent after typed
    /// serialization.  Any mismatch fails closed.
    pub fn enqueue_memory_job_idempotent(
        &self,
        input: &MemoryJobEnqueue,
    ) -> CoreResult<MemoryJobEnqueueResult> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let result = enqueue_memory_job_idempotent_on_connection(&transaction, input)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(result)
    }

    /// Loads normalized queue state.  Relational state is authoritative, so
    /// this also reads rows written by the legacy memory-job API whose JSON
    /// document may not reflect a later normalized state transition.
    pub fn get_memory_job_queue_entry(
        &self,
        id: &MemoryJobId,
    ) -> CoreResult<StoredMemoryJobQueueEntry> {
        validate_identifier("memory job", id.as_str())?;
        let connection = self.connection()?;
        load_queue_entry(&connection, id.as_str())?.ok_or_else(|| not_found("memory job"))
    }

    /// Atomically claims the oldest currently eligible job.
    ///
    /// Eligibility is evaluated against the exact immutable task-profile
    /// revision stored on the row.  Running concurrency and every start inside
    /// that revision's durable rate window are counted while the `SQLite` writer
    /// lock is held.
    pub fn claim_next_memory_job(
        &self,
        now: DateTime<Utc>,
    ) -> CoreResult<Option<StoredMemoryJobQueueEntry>> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let candidate_id = find_claimable_memory_job_id(&transaction, now)?;
        let Some(candidate_id) = candidate_id else {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(None);
        };
        let mut entry = load_queue_entry(&transaction, &candidate_id)?
            .ok_or_else(|| corrupted("claimed memory job candidate disappeared"))?;
        if entry.job.status != MemoryJobStatus::Queued {
            return Err(corrupted(
                "memory job payload and normalized queued state are inconsistent",
            ));
        }
        if entry.job.attempt >= MAX_MEMORY_JOB_ATTEMPTS {
            return Err(corrupted(
                "memory job exceeded the bounded attempt count while queued",
            ));
        }
        let next_revision = checked_next_revision(entry.revision)?;
        entry.job.status = MemoryJobStatus::Running;
        entry.job.attempt += 1;
        entry.job.updated_at = now;
        entry.job.error_code = None;
        entry.attempt_started_at.push(now);
        let payload_json = encode_entry_payload(&entry)?;
        let changed = transaction
            .execute(
                "UPDATE memory_jobs
                 SET state = 'running', revision = ?2, attempts = ?3,
                     started_at = ?4, finished_at = NULL,
                     result_record_id = NULL, failure_json = NULL,
                     payload_json = ?5, updated_at = ?4
                 WHERE id = ?1 AND state = 'queued' AND revision = ?6",
                params![
                    candidate_id,
                    i64_revision(next_revision)?,
                    entry.job.attempt,
                    now.to_rfc3339(),
                    payload_json,
                    i64_revision(entry.revision)?,
                ],
            )
            .map_err(storage_db_error)?;
        ensure_cas(changed, "memory job claim")?;
        let claimed = load_queue_entry(&transaction, &candidate_id)?
            .ok_or_else(|| corrupted("claimed memory job is missing"))?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(Some(claimed))
    }

    /// Marks abandoned running jobs interrupted.  This method never requeues
    /// or executes work; a user/Core action must call
    /// [`Self::retry_interrupted_memory_job`] explicitly.
    pub fn recover_running_memory_jobs(
        &self,
        interrupted_at: DateTime<Utc>,
    ) -> CoreResult<Vec<StoredMemoryJobQueueEntry>> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let ids = {
            let mut statement = transaction
                .prepare(
                    "SELECT id FROM memory_jobs
                     WHERE state = 'running'
                     ORDER BY started_at, created_at, id",
                )
                .map_err(storage_db_error)?;
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage_db_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage_db_error)?
        };
        let mut recovered = Vec::with_capacity(ids.len());
        for id in ids {
            let mut entry = load_queue_entry(&transaction, &id)?
                .ok_or_else(|| corrupted("running memory job disappeared during recovery"))?;
            if interrupted_at < entry.job.created_at {
                return Err(CoreError::invalid(
                    "memory job recovery time predates job creation",
                ));
            }
            let previous_revision = entry.revision;
            entry.job.status = MemoryJobStatus::Interrupted;
            entry.job.updated_at = interrupted_at;
            entry.job.error_code = None;
            entry.interruptions.push(MemoryJobInterruption {
                interrupted_at,
                error_code: Some("process_restarted".to_owned()),
            });
            let payload_json = encode_entry_payload(&entry)?;
            let changed = transaction
                .execute(
                    "UPDATE memory_jobs
                     SET state = 'interrupted', revision = revision + 1,
                         finished_at = NULL, result_record_id = NULL,
                         failure_json = NULL, payload_json = ?3,
                         updated_at = ?4
                     WHERE id = ?1 AND state = 'running' AND revision = ?2",
                    params![
                        id,
                        i64_revision(previous_revision)?,
                        payload_json,
                        interrupted_at.to_rfc3339(),
                    ],
                )
                .map_err(storage_db_error)?;
            ensure_cas(changed, "memory job recovery")?;
            recovered.push(
                load_queue_entry(&transaction, &id)?
                    .ok_or_else(|| corrupted("recovered memory job is missing"))?,
            );
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(recovered)
    }

    /// Lists interrupted jobs for one branch so a user can review and retry
    /// them explicitly.  Interrupted jobs are never requeued automatically, so
    /// without this read they stay durably invisible to the shell.
    pub fn list_interrupted_memory_jobs(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<StoredMemoryJobQueueEntry>> {
        if limit == 0 || limit > MAX_INTERRUPTED_MEMORY_JOB_PAGE {
            return Err(CoreError::invalid(
                "interrupted memory job list limit must be between 1 and 256",
            ));
        }
        let connection = self.connection()?;
        let ids = {
            let mut statement = connection
                .prepare(
                    "SELECT id FROM memory_jobs
                     WHERE conversation_id = ?1 AND branch_id = ?2
                       AND state = 'interrupted'
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
                load_queue_entry(&connection, id)?
                    .ok_or_else(|| corrupted("listed interrupted memory job is missing"))
            })
            .collect()
    }

    /// Records an ambiguous running-job outcome without treating it as a
    /// provider failure or retrying it.  Timeouts and network disconnects
    /// after request dispatch should use this transition.
    pub fn interrupt_memory_job(
        &self,
        id: &MemoryJobId,
        expected_revision: u64,
        error_code: Option<&str>,
        interrupted_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryJobQueueEntry> {
        validate_identifier("memory job", id.as_str())?;
        if let Some(error_code) = error_code {
            validate_error_code(error_code)?;
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let mut entry =
            load_queue_entry(&transaction, id.as_str())?.ok_or_else(|| not_found("memory job"))?;
        ensure_expected_revision(&entry, expected_revision)?;
        if entry.job.status != MemoryJobStatus::Running {
            return Err(queue_conflict(
                "only a running memory job can be interrupted",
            ));
        }
        if interrupted_at < entry.job.created_at
            || entry
                .started_at
                .is_some_and(|started| interrupted_at < started)
        {
            return Err(CoreError::invalid(
                "memory job interruption time predates its durable start",
            ));
        }
        entry.job.status = MemoryJobStatus::Interrupted;
        entry.job.updated_at = interrupted_at;
        entry.job.error_code = None;
        entry.interruptions.push(MemoryJobInterruption {
            interrupted_at,
            error_code: error_code.map(ToOwned::to_owned),
        });
        let payload_json = encode_entry_payload(&entry)?;
        let changed = transaction
            .execute(
                "UPDATE memory_jobs
                 SET state = 'interrupted', revision = revision + 1,
                     finished_at = NULL, result_record_id = NULL,
                     failure_json = NULL, payload_json = ?3,
                     updated_at = ?4
                 WHERE id = ?1 AND state = 'running' AND revision = ?2",
                params![
                    id.as_str(),
                    i64_revision(expected_revision)?,
                    payload_json,
                    interrupted_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        ensure_cas(changed, "memory job interruption")?;
        let interrupted = load_queue_entry(&transaction, id.as_str())?
            .ok_or_else(|| corrupted("interrupted memory job is missing"))?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(interrupted)
    }

    /// Explicitly requeues one interrupted job under compare-and-swap.
    pub fn retry_interrupted_memory_job(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryJobId,
        expected_revision: u64,
        available_at: DateTime<Utc>,
        retried_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryJobQueueEntry> {
        validate_identifier("conversation", &conversation_id.0)?;
        validate_identifier("conversation branch", &branch_id.0)?;
        validate_identifier("memory job", id.as_str())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        ensure_memory_job_owner(&transaction, conversation_id, branch_id, id)?;
        let mut entry =
            load_queue_entry(&transaction, id.as_str())?.ok_or_else(|| not_found("memory job"))?;
        ensure_expected_revision(&entry, expected_revision)?;
        if entry.job.status != MemoryJobStatus::Interrupted {
            return Err(queue_conflict(
                "only an interrupted memory job can be explicitly retried",
            ));
        }
        if entry.job.attempt >= MAX_MEMORY_JOB_ATTEMPTS {
            return Err(CoreError::invalid(
                "memory job reached the maximum attempt count",
            ));
        }
        if retried_at < entry.job.created_at {
            return Err(CoreError::invalid(
                "memory job retry time predates job creation",
            ));
        }
        entry.job.status = MemoryJobStatus::Queued;
        entry.job.updated_at = retried_at;
        entry.job.error_code = None;
        entry.available_at = available_at;
        let payload_json = encode_entry_payload(&entry)?;
        let changed = transaction
            .execute(
                "UPDATE memory_jobs
                 SET state = 'queued', revision = revision + 1,
                     available_at = ?3, started_at = NULL, finished_at = NULL,
                     result_record_id = NULL, failure_json = NULL,
                     payload_json = ?4, updated_at = ?5
                 WHERE id = ?1 AND state = 'interrupted' AND revision = ?2
                   AND conversation_id = ?6 AND branch_id = ?7",
                params![
                    id.as_str(),
                    i64_revision(expected_revision)?,
                    available_at.to_rfc3339(),
                    payload_json,
                    retried_at.to_rfc3339(),
                    conversation_id.0,
                    branch_id.0,
                ],
            )
            .map_err(storage_db_error)?;
        ensure_cas(changed, "memory job retry")?;
        let retried = load_queue_entry(&transaction, id.as_str())?
            .ok_or_else(|| corrupted("retried memory job is missing"))?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(retried)
    }

    /// Finishes one running job under compare-and-swap.
    pub fn finish_memory_job(
        &self,
        id: &MemoryJobId,
        expected_revision: u64,
        outcome: MemoryJobFinish,
        finished_at: DateTime<Utc>,
    ) -> CoreResult<StoredMemoryJobQueueEntry> {
        validate_identifier("memory job", id.as_str())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let mut entry =
            load_queue_entry(&transaction, id.as_str())?.ok_or_else(|| not_found("memory job"))?;
        ensure_expected_revision(&entry, expected_revision)?;
        if entry.job.status != MemoryJobStatus::Running {
            return Err(queue_conflict("only a running memory job can finish"));
        }
        if matches!(&outcome, MemoryJobFinish::Succeeded { .. }) {
            let message = match entry.job.kind {
                MemoryJobKind::Summary | MemoryJobKind::Embedding => {
                    "summary and embedding success must use their atomic completion operations"
                }
                MemoryJobKind::InvalidateRange => {
                    "range invalidation must commit inside the branch-mutation transaction"
                }
            };
            return Err(CoreError::invalid(message));
        }
        if let MemoryJobFinish::Succeeded { result_record_id } = &outcome {
            validate_terminal_result_record(&transaction, &entry, result_record_id.as_ref())?;
        }
        if finished_at < entry.job.created_at
            || entry
                .started_at
                .is_some_and(|started| finished_at < started)
        {
            return Err(CoreError::invalid(
                "memory job finish time predates its durable start",
            ));
        }
        let (state, result_record_id, failure_json) = match outcome {
            MemoryJobFinish::Succeeded { result_record_id } => {
                entry.job.status = MemoryJobStatus::Succeeded;
                entry.job.error_code = None;
                ("succeeded", result_record_id.map(|value| value.0), None)
            }
            MemoryJobFinish::Failed { error_code } => {
                validate_error_code(&error_code)?;
                entry.job.status = MemoryJobStatus::Failed;
                entry.job.error_code = Some(error_code.clone());
                let failure = serde_json::to_string(&serde_json::json!({
                    "error_code": error_code
                }))
                .map_err(|error| {
                    CoreError::internal(format!("cannot encode memory job failure: {error}"))
                })?;
                ("failed", None, Some(failure))
            }
            MemoryJobFinish::Cancelled => {
                entry.job.status = MemoryJobStatus::Cancelled;
                entry.job.error_code = None;
                ("cancelled", None, None)
            }
        };
        entry.job.updated_at = finished_at;
        let payload_json = encode_entry_payload(&entry)?;
        let changed = transaction
            .execute(
                "UPDATE memory_jobs
                 SET state = ?3, revision = revision + 1, finished_at = ?4,
                     result_record_id = ?5, failure_json = ?6,
                     payload_json = ?7, updated_at = ?4
                 WHERE id = ?1 AND state = 'running' AND revision = ?2",
                params![
                    id.as_str(),
                    i64_revision(expected_revision)?,
                    state,
                    finished_at.to_rfc3339(),
                    result_record_id,
                    failure_json,
                    payload_json,
                ],
            )
            .map_err(storage_db_error)?;
        ensure_cas(changed, "memory job finish")?;
        let finished = load_queue_entry(&transaction, id.as_str())?
            .ok_or_else(|| corrupted("finished memory job is missing"))?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(finished)
    }

    /// Atomically inserts a generated summary record and completes its running
    /// queue job.  Neither the record nor the terminal job state can become
    /// visible alone.
    pub fn complete_memory_summary_job(
        &self,
        id: &MemoryJobId,
        expected_revision: u64,
        record: &MemoryRecord,
        finished_at: DateTime<Utc>,
    ) -> CoreResult<MemorySummaryJobCompletion> {
        self.complete_memory_summary_job_with_embedding(
            id,
            expected_revision,
            record,
            None,
            finished_at,
        )
    }

    /// Atomically inserts a generated summary record, completes its running
    /// queue job, and enqueues the exact configured embedding job when present.
    ///
    /// The embedding payload is derived only after the immutable record
    /// revision exists, while the same writer transaction is still held.
    pub fn complete_memory_summary_job_with_embedding(
        &self,
        id: &MemoryJobId,
        expected_revision: u64,
        record: &MemoryRecord,
        embedding_seed: Option<&MemoryEmbeddingJobSeed>,
        finished_at: DateTime<Utc>,
    ) -> CoreResult<MemorySummaryJobCompletion> {
        validate_identifier("memory job", id.as_str())?;
        validate_new_memory_summary_record(record)?;
        let keywords = normalize_memory_keywords(&record.keywords)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let mut entry =
            load_queue_entry(&transaction, id.as_str())?.ok_or_else(|| not_found("memory job"))?;
        validate_queue_entry_fingerprint(&entry)?;
        if let Some(completion) = replay_memory_summary_completion(
            &transaction,
            &entry,
            expected_revision,
            record,
            embedding_seed,
            finished_at,
        )? {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(completion);
        }
        validate_running_memory_summary_completion(
            &transaction,
            &entry,
            expected_revision,
            record,
            finished_at,
        )?;

        let record_revision_id = Uuid::new_v4().to_string();
        insert_memory_summary_record(&transaction, &entry, record, &record_revision_id, &keywords)?;

        let embedding_job = embedding_seed
            .map(|seed| {
                let enqueue = embedding_enqueue_from_seed(&entry, seed, &record_revision_id)?;
                enqueue_memory_job_idempotent_on_connection(&transaction, &enqueue)
                    .map(|result| result.entry)
            })
            .transpose()?;

        finish_memory_summary_queue_entry(
            &transaction,
            &mut entry,
            expected_revision,
            record,
            finished_at,
        )?;
        let job = load_queue_entry(&transaction, id.as_str())?
            .ok_or_else(|| corrupted("completed memory summary job is missing"))?;
        let stored_record = load_memory_record_for_completion(&transaction, record.id.as_str())?
            .ok_or_else(|| corrupted("completed memory summary record is missing"))?;
        transaction.commit().map_err(storage_db_error)?;

        Ok(MemorySummaryJobCompletion {
            job,
            record: stored_record,
            embedding_job,
            exact_replay: false,
        })
    }

    /// Atomically inserts one immutable embedding and completes its running
    /// queue job.
    ///
    /// The queue payload must be [`MemoryEmbeddingJobInput`] schema v1. The
    /// exact memory-record revision, task-profile revision, selected route,
    /// dimensions, and vector digest are validated while the `SQLite` writer
    /// lock is held. Neither half of the completion can become visible alone.
    pub fn complete_memory_embedding_job(
        &self,
        id: &MemoryJobId,
        expected_revision: u64,
        embedding: &MemoryEmbeddingRecord,
        finished_at: DateTime<Utc>,
    ) -> CoreResult<MemoryEmbeddingJobCompletion> {
        let (vector_blob, vector_sha256) = prepare_memory_embedding_output(id, embedding)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let mut entry =
            load_queue_entry(&transaction, id.as_str())?.ok_or_else(|| not_found("memory job"))?;
        validate_queue_entry_fingerprint(&entry)?;
        let input = decode_memory_embedding_job_input(&entry.payload)?;
        let task_profile_revision_id = entry
            .task_profile_revision_id
            .as_deref()
            .ok_or_else(|| corrupted("embedding job has no task profile revision"))?;
        validate_embedding_output_against_input(embedding, &input)?;
        let evidence = MemoryEmbeddingCompletionEvidence {
            input: &input,
            task_profile_revision_id,
            vector_sha256: &vector_sha256,
        };
        if let Some(completion) = replay_memory_embedding_completion(
            &transaction,
            &entry,
            expected_revision,
            embedding,
            finished_at,
            evidence,
        )? {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(completion);
        }
        validate_running_memory_embedding_completion(
            &transaction,
            &entry,
            expected_revision,
            embedding,
            finished_at,
            evidence,
        )?;
        ensure_memory_embedding_completion_is_unique(&transaction, embedding, evidence)?;
        insert_memory_embedding(&transaction, embedding, &vector_blob, evidence)?;
        finish_memory_embedding_queue_entry(
            &transaction,
            &mut entry,
            expected_revision,
            embedding,
            finished_at,
        )?;
        let job = load_queue_entry(&transaction, id.as_str())?
            .ok_or_else(|| corrupted("completed memory embedding job is missing"))?;
        let stored = load_memory_embedding_exact(&transaction, &embedding.id)?
            .ok_or_else(|| corrupted("completed memory embedding is missing"))?;
        transaction.commit().map_err(storage_db_error)?;

        Ok(MemoryEmbeddingJobCompletion {
            job,
            embedding: stored,
            exact_replay: false,
        })
    }

    /// Returns deterministic cosine matches without loading more than the
    /// caller's bounded candidate limit or executing a `SQLite` extension.
    pub fn query_memory_embeddings_cosine(
        &self,
        query: &MemoryEmbeddingQuery,
    ) -> CoreResult<Vec<MemoryEmbeddingMatch>> {
        validate_memory_embedding_query(query)?;
        let query_norm = vector_squared_norm(&query.values);
        if !query_norm.is_finite() || query_norm <= f64::EPSILON {
            return Err(CoreError::invalid(
                "memory embedding query vector must have a non-zero finite norm",
            ));
        }

        let connection = self.connection()?;
        validate_embedding_task_space(
            &connection,
            &query.task_profile_revision_id,
            &query.model_route_id,
            query.dimensions,
        )?;
        ensure_memory_embedding_context_is_visible(&connection, query)?;
        let candidates = load_memory_embedding_candidates(&connection, query)?;
        let mut matches = score_memory_embedding_candidates(query, query_norm, candidates)?;
        matches.sort_by(|left, right| {
            right
                .similarity_millionths
                .cmp(&left.similarity_millionths)
                .then_with(|| left.memory_record_id.cmp(&right.memory_record_id))
                .then_with(|| left.embedding_id.cmp(&right.embedding_id))
        });
        matches.truncate(
            usize::try_from(query.result_limit)
                .map_err(|_| CoreError::invalid("memory embedding result limit is invalid"))?,
        );
        Ok(matches)
    }

    /// Applies only the closed set of user-editable memory fields.
    ///
    /// The content revision, state CAS update, keyword projection, embedding
    /// job cancellation, and audit event commit in one immediate transaction.
    pub fn patch_memory_record_user_fields(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
        expected_revision: u64,
        patch: &MemoryRecordUserPatch,
        updated_at: DateTime<Utc>,
    ) -> CoreResult<StoredRevision<MemoryRecord>> {
        validate_identifier("conversation", &conversation_id.0)?;
        validate_identifier("conversation branch", &branch_id.0)?;
        validate_identifier("memory record", id.as_str())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let stored = patch_memory_record_user_fields_in_transaction(
            &transaction,
            conversation_id,
            branch_id,
            id,
            expected_revision,
            patch,
            updated_at,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }

    /// Sets exactly one room- or character-level exclusion flag.
    pub fn set_memory_record_exclusion(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
        expected_revision: u64,
        exclusion: (MemoryRecordExclusionScope, bool),
        updated_at: DateTime<Utc>,
    ) -> CoreResult<StoredRevision<MemoryRecord>> {
        validate_identifier("conversation", &conversation_id.0)?;
        validate_identifier("conversation branch", &branch_id.0)?;
        validate_identifier("memory record", id.as_str())?;
        let (scope, excluded) = exclusion;
        let patch = match scope {
            MemoryRecordExclusionScope::Conversation => MemoryRecordUserPatch {
                excluded_from_conversation: Some(excluded),
                ..MemoryRecordUserPatch::default()
            },
            MemoryRecordExclusionScope::Character => MemoryRecordUserPatch {
                excluded_from_character: Some(excluded),
                ..MemoryRecordUserPatch::default()
            },
        };
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let stored = patch_memory_record_user_fields_in_transaction(
            &transaction,
            conversation_id,
            branch_id,
            id,
            expected_revision,
            &patch,
            updated_at,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(stored)
    }

    /// Tombstones a memory record without deleting immutable content history.
    pub fn delete_memory_record_tombstone(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
        expected_revision: u64,
        deleted_at: DateTime<Utc>,
    ) -> CoreResult<StoredRevision<MemoryRecord>> {
        validate_identifier("conversation", &conversation_id.0)?;
        validate_identifier("conversation branch", &branch_id.0)?;
        validate_identifier("memory record", id.as_str())?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        ensure_memory_record_owner(&transaction, conversation_id, branch_id, id)?;
        let current = load_user_memory_record_state(&transaction, id)?;
        ensure_memory_record_expected_revision(&current, id, expected_revision)?;
        if current.stored.deleted_at.is_some() {
            return Err(not_found("memory record"));
        }
        if deleted_at < current.stored.updated_at {
            return Err(CoreError::invalid(
                "memory record deletion time predates its latest durable update",
            ));
        }
        let next_revision = checked_next_revision(expected_revision)?;
        let changed = transaction
            .execute(
                "UPDATE memory_record_state
                 SET state_version = ?2, updated_at = ?3, deleted_at = ?3
                 WHERE record_id = ?1 AND state_version = ?4
                   AND deleted_at IS NULL
                   AND EXISTS (
                       SELECT 1 FROM memory_records AS record
                       WHERE record.id = memory_record_state.record_id
                         AND record.conversation_id = ?5
                         AND record.branch_id = ?6
                   )",
                params![
                    id.as_str(),
                    i64_revision(next_revision)?,
                    deleted_at.to_rfc3339(),
                    i64_revision(expected_revision)?,
                    conversation_id.0,
                    branch_id.0,
                ],
            )
            .map_err(storage_db_error)?;
        ensure_memory_record_cas(changed, id, expected_revision)?;
        let cancelled_embedding_jobs = cancel_embedding_jobs_for_record_revision(
            &transaction,
            current
                .stored
                .revision_id
                .as_deref()
                .ok_or_else(|| corrupted("memory record has no active content revision"))?,
            deleted_at,
        )?;
        append_user_memory_event(
            &transaction,
            id.as_str(),
            "deleted",
            current.stored.revision_id.as_deref(),
            current.stored.revision_id.as_deref(),
            serde_json::json!({
                "state_version": next_revision,
                "reason": "user_deleted",
                "cancelled_embedding_jobs": cancelled_embedding_jobs,
            }),
            deleted_at,
        )?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(StoredRevision {
            value: current.stored.value,
            revision: next_revision,
            revision_id: current.stored.revision_id,
            created_at: current.stored.created_at,
            updated_at: deleted_at,
            deleted_at: Some(deleted_at),
        })
    }

    /// Loads one exact immutable memory-profile revision by revision id.
    pub fn get_memory_profile_revision_by_id(
        &self,
        revision_id: &str,
    ) -> CoreResult<ObjectRevision<MemoryProfile>> {
        validate_identifier("memory profile revision", revision_id)?;
        let connection = self.connection()?;
        load_object_revision_by_id(
            &connection,
            revision_id,
            "memory_profile",
            "memory_profile_revisions",
        )?
        .ok_or_else(|| not_found("memory profile revision"))
    }

    /// Loads one exact immutable memory-record content revision.
    pub fn get_memory_record_revision_by_id(
        &self,
        revision_id: &str,
    ) -> CoreResult<ObjectRevision<MemoryRecord>> {
        validate_identifier("memory record revision", revision_id)?;
        let connection = self.connection()?;
        load_memory_record_revision_by_id(&connection, revision_id)?
            .ok_or_else(|| not_found("memory record revision"))
    }

    /// Loads one exact immutable task-profile revision by revision id.
    pub fn get_task_profile_revision_by_id(
        &self,
        revision_id: &str,
    ) -> CoreResult<ObjectRevision<TaskProfile>> {
        validate_identifier("task profile revision", revision_id)?;
        let connection = self.connection()?;
        load_object_revision_by_id(
            &connection,
            revision_id,
            "task_profile",
            "task_profile_revisions",
        )?
        .ok_or_else(|| not_found("task profile revision"))
    }

    /// Loads the exact immutable embedding-task revision linked by one exact
    /// memory-profile revision. No mutable current-profile lookup is involved.
    pub fn get_memory_profile_embedding_task_revision(
        &self,
        memory_profile_revision_id: &str,
    ) -> CoreResult<Option<ObjectRevision<TaskProfile>>> {
        validate_identifier("memory profile revision", memory_profile_revision_id)?;
        let connection = self.connection()?;
        let revision_id = connection
            .query_row(
                "SELECT embedding_task_profile_revision_id
                 FROM memory_profile_revisions
                 WHERE revision_id = ?1",
                [memory_profile_revision_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("memory profile revision"))?;
        revision_id
            .map(|revision_id| {
                load_object_revision_by_id(
                    &connection,
                    &revision_id,
                    "task_profile",
                    "task_profile_revisions",
                )?
                .ok_or_else(|| corrupted("memory profile embedding task revision is missing"))
            })
            .transpose()
    }

    /// Lists summary work that reserves a source range on the requested
    /// branch lineage under the exact immutable memory/task policy.
    pub fn list_visible_memory_summary_jobs(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        memory_profile_revision_id: &str,
        task_profile_revision_id: &str,
    ) -> CoreResult<Vec<StoredMemoryJobQueueEntry>> {
        validate_identifier("memory profile revision", memory_profile_revision_id)?;
        validate_identifier("task profile revision", task_profile_revision_id)?;
        let connection = self.connection()?;
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
                 SELECT job.id
                 FROM memory_jobs AS job
                 JOIN lineage AS source_start
                   ON source_start.id = job.source_start_message_id
                 JOIN lineage AS source_end
                   ON source_end.id = job.source_end_message_id
                 WHERE job.conversation_id = ?1
                   AND job.job_kind = 'summary'
                   AND job.memory_profile_revision_id = ?3
                   AND job.task_profile_revision_id = ?4
                   AND job.state IN (
                       'queued', 'running', 'interrupted', 'succeeded',
                       'failed', 'cancelled'
                   )
                 ORDER BY julianday(job.created_at), job.id
                 LIMIT ?5",
            )
            .map_err(storage_db_error)?;
        let limit = i64::try_from(MAX_VISIBLE_MEMORY_SUMMARY_JOBS + 1)
            .map_err(|_| CoreError::internal("memory summary job query limit overflowed"))?;
        let ids = statement
            .query_map(
                params![
                    conversation_id.0,
                    branch_id.0,
                    memory_profile_revision_id,
                    task_profile_revision_id,
                    limit,
                ],
                |row| row.get::<_, String>(0),
            )
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        drop(statement);
        if ids.len() > MAX_VISIBLE_MEMORY_SUMMARY_JOBS {
            return Err(CoreError::invalid(
                "visible memory summary job count exceeds the safety limit",
            ));
        }
        ids.into_iter()
            .map(|id| {
                load_queue_entry(&connection, &id)?
                    .ok_or_else(|| corrupted("visible memory summary job disappeared"))
            })
            .collect()
    }
}

fn ensure_memory_embedding_context_is_visible(
    connection: &Connection,
    query: &MemoryEmbeddingQuery,
) -> CoreResult<()> {
    let context_is_visible = connection
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
             SELECT EXISTS(SELECT 1 FROM lineage WHERE id = ?3)",
            params![
                query.conversation_id.0,
                query.branch_id.0,
                query.context_head_message_id.0,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if context_is_visible {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "memory embedding query context head is not visible on the branch",
        ))
    }
}

fn load_memory_embedding_candidates(
    connection: &Connection,
    query: &MemoryEmbeddingQuery,
) -> CoreResult<Vec<MemoryEmbeddingCandidate>> {
    let mut statement = connection
        .prepare(
            "WITH RECURSIVE lineage(id, parent_id) AS (
                 SELECT message.id, message.parent_id
                 FROM messages AS message
                 WHERE message.conversation_id = ?1 AND message.id = ?3
                 UNION
                 SELECT parent.id, parent.parent_id
                 FROM messages AS parent
                 JOIN lineage AS child ON child.parent_id = parent.id
                 WHERE parent.conversation_id = ?1
             )
             SELECT embedding.id, revision.record_id,
                    embedding.record_revision_id, embedding.vector_sha256,
                    embedding.vector_blob
             FROM memory_embeddings AS embedding
             JOIN memory_record_revisions AS revision
               ON revision.id = embedding.record_revision_id
             JOIN memory_records AS record ON record.id = revision.record_id
             JOIN memory_record_state AS state
               ON state.record_id = record.id
              AND state.active_revision_id = embedding.record_revision_id
             WHERE record.conversation_id = ?1
               AND state.invalidated_at IS NULL
               AND state.excluded_from_conversation_at IS NULL
               AND state.excluded_from_character_at IS NULL
               AND state.deleted_at IS NULL
               AND EXISTS (
                   SELECT 1 FROM lineage
                   WHERE lineage.id = record.source_end_message_id
               )
               AND embedding.task_profile_revision_id = ?4
               AND embedding.model_route_id = ?5
               AND embedding.dimensions = ?6
               AND embedding.vector_space_sha256 = ?7
               AND embedding.encoding = 'f32le'
             ORDER BY embedding.id
             LIMIT ?8",
        )
        .map_err(storage_db_error)?;
    let candidates = statement
        .query_map(
            params![
                query.conversation_id.0,
                query.branch_id.0,
                query.context_head_message_id.0,
                query.task_profile_revision_id,
                query.model_route_id.as_str(),
                query.dimensions,
                query.vector_space_sha256,
                i64::from(query.candidate_limit),
            ],
            |row| {
                Ok(MemoryEmbeddingCandidate {
                    embedding_id: row.get(0)?,
                    record_id: row.get(1)?,
                    revision_id: row.get(2)?,
                    vector_sha256: row.get(3)?,
                    vector_blob: row.get(4)?,
                })
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    drop(statement);
    Ok(candidates)
}

fn score_memory_embedding_candidates(
    query: &MemoryEmbeddingQuery,
    query_norm: f64,
    candidates: Vec<MemoryEmbeddingCandidate>,
) -> CoreResult<Vec<MemoryEmbeddingMatch>> {
    let mut matches = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let values = decode_memory_embedding_vector(
            query.dimensions,
            &candidate.vector_blob,
            &candidate.vector_sha256,
        )?;
        let candidate_norm = vector_squared_norm(&values);
        if !candidate_norm.is_finite() || candidate_norm <= f64::EPSILON {
            continue;
        }
        let dot = query
            .values
            .iter()
            .zip(&values)
            .map(|(left, right)| f64::from(*left) * f64::from(*right))
            .sum::<f64>();
        let similarity = (dot / (query_norm * candidate_norm).sqrt()).clamp(-1.0, 1.0);
        matches.push(MemoryEmbeddingMatch {
            embedding_id: candidate.embedding_id,
            memory_record_id: MemoryRecordId::from(candidate.record_id),
            memory_record_revision_id: candidate.revision_id,
            vector_sha256: candidate.vector_sha256,
            similarity_millionths: similarity_millionths(similarity)?,
        });
    }
    Ok(matches)
}

fn validate_new_memory_summary_record(record: &MemoryRecord) -> CoreResult<()> {
    record
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid memory record: {error}")))?;
    if record.kind != MemoryKind::ConversationSummary {
        return Err(CoreError::invalid(
            "a summary job must create a conversation_summary memory record",
        ));
    }
    if record.invalidated_at.is_some()
        || record.excluded_from_conversation
        || record.excluded_from_character
        || record.embedding_ref.is_some()
    {
        return Err(CoreError::invalid(
            "a newly completed memory summary cannot begin invalidated, excluded, or pre-embedded",
        ));
    }
    Ok(())
}

fn replay_memory_summary_completion(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    expected_revision: u64,
    record: &MemoryRecord,
    embedding_seed: Option<&MemoryEmbeddingJobSeed>,
    finished_at: DateTime<Utc>,
) -> CoreResult<Option<MemorySummaryJobCompletion>> {
    if entry.job.status != MemoryJobStatus::Succeeded {
        return Ok(None);
    }
    let expected_terminal_revision = checked_next_revision(expected_revision)?;
    let same_result = entry.job.kind == MemoryJobKind::Summary
        && entry.revision == expected_terminal_revision
        && entry.result_record_id.as_ref() == Some(&record.id)
        && entry.finished_at == Some(finished_at);
    if !same_result {
        return Err(queue_conflict(
            "memory summary completion conflicts with an existing terminal outcome",
        ));
    }
    let stored_record = load_memory_record_for_completion(connection, record.id.as_str())?
        .ok_or_else(|| corrupted("completed memory summary record is missing"))?;
    if stored_record.value != *record {
        return Err(queue_conflict(
            "memory summary completion was replayed with different record content",
        ));
    }
    let embedding_job =
        replay_summary_embedding_job(connection, entry, &stored_record, embedding_seed)?;
    Ok(Some(MemorySummaryJobCompletion {
        job: entry.clone(),
        record: stored_record,
        embedding_job,
        exact_replay: true,
    }))
}

fn replay_summary_embedding_job(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    record: &StoredRevision<MemoryRecord>,
    embedding_seed: Option<&MemoryEmbeddingJobSeed>,
) -> CoreResult<Option<StoredMemoryJobQueueEntry>> {
    let Some(seed) = embedding_seed else {
        return Ok(None);
    };
    let record_revision_id = record
        .revision_id
        .as_deref()
        .ok_or_else(|| corrupted("completed memory summary has no immutable revision id"))?;
    let enqueue = embedding_enqueue_from_seed(entry, seed, record_revision_id)?;
    let existing = load_queue_entry_by_idempotency_key(connection, &enqueue.job.idempotency_key)?
        .ok_or_else(|| {
        corrupted("completed memory summary is missing its atomic embedding queue job")
    })?;
    ensure_exact_replay(&existing, &enqueue)?;
    if existing.job.id != enqueue.job.id {
        return Err(corrupted(
            "completed memory summary embedding job id is inconsistent",
        ));
    }
    Ok(Some(existing))
}

fn validate_running_memory_summary_completion(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    expected_revision: u64,
    record: &MemoryRecord,
    finished_at: DateTime<Utc>,
) -> CoreResult<()> {
    ensure_expected_revision(entry, expected_revision)?;
    if entry.job.status != MemoryJobStatus::Running || entry.job.kind != MemoryJobKind::Summary {
        return Err(queue_conflict(
            "only a running summary memory job can commit a summary record",
        ));
    }
    if record.conversation_id != entry.job.conversation_id
        || record.branch_id != entry.job.branch_id
        || record.source_start_message_id != entry.job.source_start_message_id
        || record.source_end_message_id != entry.job.source_end_message_id
    {
        return Err(CoreError::invalid(
            "memory summary record lineage differs from its immutable queue job",
        ));
    }
    if record.provenance.source_kind != lorepia_domain::SourceKind::Generated
        || record.provenance.source_id.as_deref() != Some(entry.job.id.as_str())
        || record.provenance.source_hash.is_none()
    {
        return Err(CoreError::invalid(
            "memory summary provenance must bind its generated output to the exact queue job",
        ));
    }
    if finished_at < entry.job.created_at
        || entry
            .started_at
            .is_some_and(|started| finished_at < started)
        || record.created_at > finished_at
        || record.updated_at > finished_at
    {
        return Err(CoreError::invalid(
            "memory summary completion timestamps are inconsistent",
        ));
    }
    validate_memory_record_source_range(connection, record)
}

fn insert_memory_summary_record(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    record: &MemoryRecord,
    revision_id: &str,
    keywords: &[(String, String)],
) -> CoreResult<()> {
    insert_memory_summary_revision(connection, record, revision_id)?;
    insert_memory_record_keywords(connection, revision_id, keywords)?;
    insert_memory_summary_state_and_event(connection, entry, record, revision_id)
}

fn insert_memory_summary_revision(
    connection: &Connection,
    record: &MemoryRecord,
    revision_id: &str,
) -> CoreResult<()> {
    let document_json = encode_bounded_json(record, 8 * 1024 * 1024, "memory record")?;
    let structured_data_json = encode_bounded_json(
        &record.structured_data,
        4 * 1024 * 1024,
        "memory structured data",
    )?;
    let provenance_json = encode_bounded_json(&record.provenance, 65_536, "memory provenance")?;
    let content_sha256 = hex::encode(Sha256::digest(document_json.as_bytes()));
    connection
        .execute(
            "INSERT INTO memory_records
             (id, conversation_id, branch_id, source_start_message_id,
              source_end_message_id, kind, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'conversation_summary', ?6)",
            params![
                record.id.as_str(),
                record.conversation_id.0,
                record.branch_id.0,
                record.source_start_message_id.0,
                record.source_end_message_id.0,
                record.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    connection
        .execute(
            "INSERT INTO memory_record_revisions
             (id, record_id, revision_no, parent_revision_id, title, summary,
              structured_data_json, importance, content_sha256,
              provenance_json, document_json, created_at)
             VALUES (?1, ?2, 1, NULL, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                revision_id,
                record.id.as_str(),
                record.title,
                record.summary,
                &structured_data_json,
                record.importance,
                &content_sha256,
                &provenance_json,
                &document_json,
                record.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_memory_record_keywords(
    connection: &Connection,
    revision_id: &str,
    keywords: &[(String, String)],
) -> CoreResult<()> {
    for (ordinal, (keyword, normalized_keyword)) in keywords.iter().enumerate() {
        connection
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

fn insert_memory_summary_state_and_event(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    record: &MemoryRecord,
    revision_id: &str,
) -> CoreResult<()> {
    connection
        .execute(
            "INSERT INTO memory_record_state
             (record_id, active_revision_id, pinned, invalidated_at,
              invalidation_reason, excluded_from_conversation_at,
              excluded_from_character_at, deleted_at, state_version, updated_at)
             VALUES (?1, ?2, ?3, NULL, NULL, NULL, NULL, NULL, 1, ?4)",
            params![
                record.id.as_str(),
                revision_id,
                record.pinned,
                record.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    let event_payload = serde_json::to_string(&serde_json::json!({
        "state_version": 1,
        "memory_job_id": entry.job.id.as_str(),
    }))
    .map_err(|error| CoreError::internal(format!("cannot encode memory record event: {error}")))?;
    connection
        .execute(
            "INSERT INTO memory_record_events
             (record_id, sequence, event_kind, from_revision_id,
              to_revision_id, payload_json, created_at)
             VALUES (?1, 1, 'created', NULL, ?2, ?3, ?4)",
            params![
                record.id.as_str(),
                revision_id,
                event_payload,
                record.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn finish_memory_summary_queue_entry(
    connection: &Connection,
    entry: &mut StoredMemoryJobQueueEntry,
    expected_revision: u64,
    record: &MemoryRecord,
    finished_at: DateTime<Utc>,
) -> CoreResult<()> {
    entry.job.status = MemoryJobStatus::Succeeded;
    entry.job.updated_at = finished_at;
    entry.job.error_code = None;
    let payload_json = encode_entry_payload(entry)?;
    let changed = connection
        .execute(
            "UPDATE memory_jobs
             SET state = 'succeeded', revision = revision + 1,
                 finished_at = ?3, result_record_id = ?4,
                 failure_json = NULL, payload_json = ?5, updated_at = ?3
             WHERE id = ?1 AND state = 'running' AND revision = ?2",
            params![
                entry.job.id.as_str(),
                i64_revision(expected_revision)?,
                finished_at.to_rfc3339(),
                record.id.as_str(),
                payload_json,
            ],
        )
        .map_err(storage_db_error)?;
    ensure_cas(changed, "memory summary completion")
}

#[derive(Clone, Copy)]
struct MemoryEmbeddingCompletionEvidence<'a> {
    input: &'a MemoryEmbeddingJobInput,
    task_profile_revision_id: &'a str,
    vector_sha256: &'a str,
}

fn prepare_memory_embedding_output(
    id: &MemoryJobId,
    embedding: &MemoryEmbeddingRecord,
) -> CoreResult<(Vec<u8>, String)> {
    validate_identifier("memory job", id.as_str())?;
    validate_identifier("memory embedding", &embedding.id)?;
    validate_identifier("memory record", embedding.memory_record_id.as_str())?;
    let model_route_id = embedding
        .model_route_id
        .as_ref()
        .ok_or_else(|| CoreError::invalid("memory embedding requires a model route"))?;
    validate_identifier("memory embedding model route", model_route_id.as_str())?;
    encode_memory_embedding_vector(embedding.dimensions, &embedding.values)
}

fn replay_memory_embedding_completion(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    expected_revision: u64,
    embedding: &MemoryEmbeddingRecord,
    finished_at: DateTime<Utc>,
    evidence: MemoryEmbeddingCompletionEvidence<'_>,
) -> CoreResult<Option<MemoryEmbeddingJobCompletion>> {
    if entry.job.status != MemoryJobStatus::Succeeded {
        return Ok(None);
    }
    let expected_terminal_revision = checked_next_revision(expected_revision)?;
    let same_result = entry.job.kind == MemoryJobKind::Embedding
        && entry.revision == expected_terminal_revision
        && entry.result_record_id.as_ref() == Some(&embedding.memory_record_id)
        && entry.finished_at == Some(finished_at);
    if !same_result {
        return Err(queue_conflict(
            "memory embedding completion conflicts with an existing terminal outcome",
        ));
    }
    let stored = load_memory_embedding_exact(connection, &embedding.id)?
        .ok_or_else(|| corrupted("completed memory embedding is missing"))?;
    if stored.value != *embedding
        || stored.memory_record_revision_id != evidence.input.memory_record_revision_id
        || stored.task_profile_revision_id != evidence.task_profile_revision_id
        || stored.vector_space_sha256 != evidence.input.vector_space_sha256
        || stored.vector_sha256 != evidence.vector_sha256
    {
        return Err(queue_conflict(
            "memory embedding completion was replayed with different output",
        ));
    }
    Ok(Some(MemoryEmbeddingJobCompletion {
        job: entry.clone(),
        embedding: stored,
        exact_replay: true,
    }))
}

fn validate_running_memory_embedding_completion(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    expected_revision: u64,
    embedding: &MemoryEmbeddingRecord,
    finished_at: DateTime<Utc>,
    evidence: MemoryEmbeddingCompletionEvidence<'_>,
) -> CoreResult<()> {
    ensure_expected_revision(entry, expected_revision)?;
    if entry.job.status != MemoryJobStatus::Running || entry.job.kind != MemoryJobKind::Embedding {
        return Err(queue_conflict(
            "only a running embedding memory job can commit an embedding",
        ));
    }
    if finished_at < entry.job.created_at
        || entry
            .started_at
            .is_some_and(|started| finished_at < started)
        || entry
            .started_at
            .is_some_and(|started| embedding.created_at < started)
        || embedding.created_at > finished_at
    {
        return Err(CoreError::invalid(
            "memory embedding completion timestamps are inconsistent",
        ));
    }
    validate_embedding_completion_bindings(
        connection,
        entry,
        evidence.input,
        embedding,
        evidence.task_profile_revision_id,
    )
}

fn ensure_memory_embedding_completion_is_unique(
    connection: &Connection,
    embedding: &MemoryEmbeddingRecord,
    evidence: MemoryEmbeddingCompletionEvidence<'_>,
) -> CoreResult<()> {
    if load_memory_embedding_exact(connection, &embedding.id)?.is_some() {
        return Err(queue_conflict(
            "memory embedding id already belongs to another completion",
        ));
    }
    let existing_binding = connection
        .query_row(
            "SELECT id FROM memory_embeddings
             WHERE record_revision_id = ?1
               AND task_profile_revision_id = ?2
               AND model_route_id = ?3
               AND dimensions = ?4
               AND vector_space_sha256 = ?5
             LIMIT 1",
            params![
                evidence.input.memory_record_revision_id,
                evidence.task_profile_revision_id,
                evidence.input.model_route_id.as_str(),
                embedding.dimensions,
                evidence.input.vector_space_sha256,
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if existing_binding.is_some() {
        return Err(queue_conflict(
            "the exact memory revision and embedding policy already have an output",
        ));
    }
    let duplicate_id = connection
        .query_row(
            "SELECT id FROM memory_embeddings
             WHERE record_revision_id = ?1
               AND model_route_id = ?2
               AND dimensions = ?3
               AND vector_space_sha256 = ?4
               AND vector_sha256 = ?5",
            params![
                evidence.input.memory_record_revision_id,
                evidence.input.model_route_id.as_str(),
                embedding.dimensions,
                evidence.input.vector_space_sha256,
                evidence.vector_sha256,
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if duplicate_id.is_some() {
        return Err(queue_conflict(
            "the exact memory embedding output already belongs to another id",
        ));
    }
    Ok(())
}

fn insert_memory_embedding(
    connection: &Connection,
    embedding: &MemoryEmbeddingRecord,
    vector_blob: &[u8],
    evidence: MemoryEmbeddingCompletionEvidence<'_>,
) -> CoreResult<()> {
    connection
        .execute(
            "INSERT INTO memory_embeddings
             (id, record_revision_id, task_profile_revision_id, model_route_id,
              dimensions, vector_space_sha256, encoding, vector_blob,
              vector_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'f32le', ?7, ?8, ?9)",
            params![
                embedding.id,
                evidence.input.memory_record_revision_id,
                evidence.task_profile_revision_id,
                evidence.input.model_route_id.as_str(),
                embedding.dimensions,
                evidence.input.vector_space_sha256,
                vector_blob,
                evidence.vector_sha256,
                embedding.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn finish_memory_embedding_queue_entry(
    connection: &Connection,
    entry: &mut StoredMemoryJobQueueEntry,
    expected_revision: u64,
    embedding: &MemoryEmbeddingRecord,
    finished_at: DateTime<Utc>,
) -> CoreResult<()> {
    entry.job.status = MemoryJobStatus::Succeeded;
    entry.job.updated_at = finished_at;
    entry.job.error_code = None;
    let payload_json = encode_entry_payload(entry)?;
    let changed = connection
        .execute(
            "UPDATE memory_jobs
             SET state = 'succeeded', revision = revision + 1,
                 finished_at = ?3, result_record_id = ?4,
                 failure_json = NULL, payload_json = ?5, updated_at = ?3
             WHERE id = ?1 AND state = 'running' AND revision = ?2",
            params![
                entry.job.id.as_str(),
                i64_revision(expected_revision)?,
                finished_at.to_rfc3339(),
                embedding.memory_record_id.as_str(),
                payload_json,
            ],
        )
        .map_err(storage_db_error)?;
    ensure_cas(changed, "memory embedding completion")
}

fn find_claimable_memory_job_id(
    connection: &Connection,
    now: DateTime<Utc>,
) -> CoreResult<Option<String>> {
    connection
        .query_row(
            "SELECT job.id
             FROM memory_jobs AS job
             LEFT JOIN task_profile_revisions AS task
               ON task.revision_id = job.task_profile_revision_id
             WHERE job.state = 'queued'
               AND job.attempts < 32
               AND julianday(job.available_at) <= julianday(?1)
               AND (
                   (
                       job.task_profile_revision_id IS NULL
                       AND (
                           SELECT COUNT(*)
                           FROM memory_jobs AS running
                           WHERE running.state = 'running'
                             AND running.task_profile_revision_id IS NULL
                       ) < 1
                   )
                   OR (
                       job.task_profile_revision_id IS NOT NULL
                       AND task.revision_id IS NOT NULL
                       AND (
                           SELECT COUNT(*)
                           FROM memory_jobs AS running
                           WHERE running.state = 'running'
                             AND running.task_profile_revision_id =
                                 job.task_profile_revision_id
                       ) < task.concurrency_limit
                       AND (
                           (
                               SELECT COUNT(*)
                               FROM memory_jobs AS recent
                               JOIN json_each(
                                   recent.payload_json,
                                   '$.attempt_started_at'
                               ) AS attempt
                               WHERE recent.task_profile_revision_id =
                                     job.task_profile_revision_id
                                 AND json_type(
                                     recent.payload_json,
                                     '$.attempt_started_at'
                                 ) = 'array'
                                 AND julianday(attempt.value) > julianday(?1)
                                     - (task.rate_limit_per_seconds / 86400.0)
                           )
                           + (
                               SELECT COUNT(*)
                               FROM memory_jobs AS recent
                               WHERE recent.task_profile_revision_id =
                                     job.task_profile_revision_id
                                 AND json_type(
                                     recent.payload_json,
                                     '$.attempt_started_at'
                                 ) IS NULL
                                 AND recent.started_at IS NOT NULL
                                 AND julianday(recent.started_at) > julianday(?1)
                                     - (task.rate_limit_per_seconds / 86400.0)
                           )
                       ) < task.rate_limit_requests
                   )
               )
             ORDER BY julianday(job.available_at), julianday(job.created_at), job.id
             LIMIT 1",
            [now.to_rfc3339()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)
}

/// Computes the queue's canonical fingerprint for immutable memory input.
///
/// Callers may use this before enqueueing; storage recomputes it and rejects a
/// mismatch.  Mutable job ids, idempotency keys, attempts, and timestamps are
/// intentionally excluded.
pub fn memory_job_input_fingerprint(
    job: &MemoryJob,
    memory_profile_revision_id: Option<&str>,
    task_profile_revision_id: Option<&str>,
    payload: &VersionedJson,
) -> CoreResult<String> {
    validate_optional_identifier("memory profile revision", memory_profile_revision_id)?;
    validate_optional_identifier("task profile revision", task_profile_revision_id)?;
    validate_versioned_payload(payload)?;
    let value = serde_json::json!({
        "schema_version": QUEUE_PAYLOAD_SCHEMA_VERSION,
        "job_kind": job_kind_to_str(job.kind),
        "conversation_id": job.conversation_id,
        "branch_id": job.branch_id,
        "source_start_message_id": job.source_start_message_id,
        "source_end_message_id": job.source_end_message_id,
        "memory_profile_revision_id": memory_profile_revision_id,
        "task_profile_revision_id": task_profile_revision_id,
        "input": payload,
    });
    let bytes = serde_json::to_vec(&value).map_err(|error| {
        CoreError::invalid(format!(
            "cannot encode memory job fingerprint input: {error}"
        ))
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn validate_queue_entry_fingerprint(entry: &StoredMemoryJobQueueEntry) -> CoreResult<()> {
    let actual = memory_job_input_fingerprint(
        &entry.job,
        entry.memory_profile_revision_id.as_deref(),
        entry.task_profile_revision_id.as_deref(),
        &entry.payload,
    )
    .map_err(|error| {
        corrupted(format!(
            "stored memory job fingerprint input is invalid: {}",
            error.message
        ))
    })?;
    if actual == entry.input_fingerprint_sha256 {
        Ok(())
    } else {
        Err(corrupted(
            "stored memory job input fingerprint does not match its immutable bindings",
        ))
    }
}

fn enqueue_memory_job_idempotent_on_connection(
    connection: &Connection,
    input: &MemoryJobEnqueue,
) -> CoreResult<MemoryJobEnqueueResult> {
    validate_enqueue(input)?;
    let expected_fingerprint = memory_job_input_fingerprint(
        &input.job,
        input.memory_profile_revision_id.as_deref(),
        input.task_profile_revision_id.as_deref(),
        &input.payload,
    )?;
    if input.input_fingerprint_sha256 != expected_fingerprint {
        return Err(CoreError::invalid(
            "memory job input fingerprint does not match the immutable queue input",
        ));
    }
    validate_profile_task_binding(
        connection,
        input.job.kind,
        input.memory_profile_revision_id.as_deref(),
        input.task_profile_revision_id.as_deref(),
    )?;
    if input.job.kind == MemoryJobKind::Embedding {
        validate_embedding_queue_input(connection, input)?;
    }

    if let Some(existing) =
        load_queue_entry_by_idempotency_key(connection, &input.job.idempotency_key)?
    {
        ensure_exact_replay(&existing, input)?;
        return Ok(MemoryJobEnqueueResult {
            entry: existing,
            exact_replay: true,
        });
    }
    if load_queue_entry(connection, input.job.id.as_str())?.is_some() {
        return Err(queue_conflict(
            "memory job id already belongs to a different idempotent enqueue",
        ));
    }
    if let Some(existing_key) = find_live_memory_job_for_input(connection, input)? {
        return Err(queue_conflict(format!(
            "the exact memory input is already live under idempotency key {existing_key}",
        )));
    }

    let queue_payload = QueuePayload {
        queue_schema_version: QUEUE_PAYLOAD_SCHEMA_VERSION,
        job: input.job.clone(),
        input: input.payload.clone(),
        attempt_started_at: Vec::new(),
        interruptions: Vec::new(),
    };
    let payload_json = encode_queue_payload(&queue_payload)?;
    insert_memory_job_queue_row(connection, input, &payload_json)?;
    let entry = load_queue_entry(connection, input.job.id.as_str())?
        .ok_or_else(|| corrupted("newly enqueued memory job is missing"))?;
    Ok(MemoryJobEnqueueResult {
        entry,
        exact_replay: false,
    })
}

fn find_live_memory_job_for_input(
    connection: &Connection,
    input: &MemoryJobEnqueue,
) -> CoreResult<Option<String>> {
    connection
        .query_row(
            "SELECT idempotency_key
             FROM memory_jobs
             WHERE job_kind = ?1
               AND conversation_id = ?2
               AND branch_id = ?3
               AND source_start_message_id = ?4
               AND source_end_message_id = ?5
               AND input_fingerprint_sha256 = ?6
               AND state IN ('queued', 'running')
             LIMIT 1",
            params![
                job_kind_to_str(input.job.kind),
                input.job.conversation_id.0,
                input.job.branch_id.0,
                input.job.source_start_message_id.0,
                input.job.source_end_message_id.0,
                input.input_fingerprint_sha256,
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)
}

fn insert_memory_job_queue_row(
    connection: &Connection,
    input: &MemoryJobEnqueue,
    payload_json: &str,
) -> CoreResult<()> {
    connection
        .execute(
            "INSERT INTO memory_jobs
             (id, idempotency_key, job_kind, memory_profile_revision_id,
              task_profile_revision_id, conversation_id, branch_id,
              source_start_message_id, source_end_message_id,
              input_fingerprint_sha256, state, revision, attempts,
              available_at, started_at, finished_at, result_record_id,
              failure_json, payload_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     'queued', 1, 0, ?11, NULL, NULL, NULL, NULL,
                     ?12, ?13, ?14)",
            params![
                input.job.id.as_str(),
                input.job.idempotency_key,
                job_kind_to_str(input.job.kind),
                input.memory_profile_revision_id,
                input.task_profile_revision_id,
                input.job.conversation_id.0,
                input.job.branch_id.0,
                input.job.source_start_message_id.0,
                input.job.source_end_message_id.0,
                input.input_fingerprint_sha256,
                input.available_at.to_rfc3339(),
                payload_json,
                input.job.created_at.to_rfc3339(),
                input.job.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn embedding_enqueue_from_seed(
    summary: &StoredMemoryJobQueueEntry,
    seed: &MemoryEmbeddingJobSeed,
    memory_record_revision_id: &str,
) -> CoreResult<MemoryJobEnqueue> {
    validate_identifier("memory record revision", memory_record_revision_id)?;
    if summary.job.kind != MemoryJobKind::Summary
        || seed.job.kind != MemoryJobKind::Embedding
        || seed.job.conversation_id != summary.job.conversation_id
        || seed.job.branch_id != summary.job.branch_id
        || seed.job.source_start_message_id != summary.job.source_start_message_id
        || seed.job.source_end_message_id != summary.job.source_end_message_id
    {
        return Err(CoreError::invalid(
            "atomic embedding job lineage differs from its summary job",
        ));
    }
    if summary.memory_profile_revision_id.as_deref()
        != Some(seed.memory_profile_revision_id.as_str())
    {
        return Err(CoreError::invalid(
            "atomic embedding job uses a different memory profile revision",
        ));
    }
    let payload = VersionedJson {
        schema_version: 1,
        value: serde_json::to_value(MemoryEmbeddingJobInput {
            memory_record_revision_id: memory_record_revision_id.to_owned(),
            model_route_id: seed.model_route_id.clone(),
            dimensions: seed.dimensions,
            vector_space_sha256: seed.vector_space_sha256.clone(),
        })
        .map_err(|error| {
            CoreError::internal(format!("cannot encode memory embedding job input: {error}"))
        })?,
    };
    let input_fingerprint_sha256 = memory_job_input_fingerprint(
        &seed.job,
        Some(&seed.memory_profile_revision_id),
        Some(&seed.task_profile_revision_id),
        &payload,
    )?;
    Ok(MemoryJobEnqueue {
        job: seed.job.clone(),
        memory_profile_revision_id: Some(seed.memory_profile_revision_id.clone()),
        task_profile_revision_id: Some(seed.task_profile_revision_id.clone()),
        input_fingerprint_sha256,
        payload,
        available_at: seed.available_at,
    })
}

fn validate_enqueue(input: &MemoryJobEnqueue) -> CoreResult<()> {
    input
        .job
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid memory job: {error}")))?;
    if input.job.status != MemoryJobStatus::Queued
        || input.job.attempt != 0
        || input.job.error_code.is_some()
    {
        return Err(CoreError::invalid(
            "a durable memory job must be enqueued queued at attempt zero without an error",
        ));
    }
    if input.job.kind == MemoryJobKind::InvalidateRange {
        return Err(CoreError::invalid(
            "invalidate_range is not a provider job; branch mutation must invalidate memory atomically",
        ));
    }
    if input.job.updated_at < input.job.created_at {
        return Err(CoreError::invalid(
            "memory job update time predates creation",
        ));
    }
    validate_sha256(
        "memory job input fingerprint",
        &input.input_fingerprint_sha256,
    )?;
    validate_optional_identifier(
        "memory profile revision",
        input.memory_profile_revision_id.as_deref(),
    )?;
    validate_optional_identifier(
        "task profile revision",
        input.task_profile_revision_id.as_deref(),
    )?;
    validate_versioned_payload(&input.payload)
}

fn validate_profile_task_binding(
    connection: &Connection,
    kind: MemoryJobKind,
    memory_profile_revision_id: Option<&str>,
    task_profile_revision_id: Option<&str>,
) -> CoreResult<()> {
    match (memory_profile_revision_id, task_profile_revision_id) {
        (None, None) => {
            return Err(CoreError::invalid(
                "summary and embedding jobs require exact memory/task profile revisions",
            ));
        }
        (Some(_), None) | (None, Some(_)) => {
            return Err(CoreError::invalid(
                "memory and task profile revisions must be supplied together",
            ));
        }
        (Some(_), Some(_)) => {}
    }
    let memory_revision_id = memory_profile_revision_id
        .ok_or_else(|| CoreError::internal("validated memory revision is missing"))?;
    let task_revision_id = task_profile_revision_id
        .ok_or_else(|| CoreError::internal("validated task revision is missing"))?;
    let relation = connection
        .query_row(
            "SELECT memory.summary_task_profile_revision_id,
                    memory.embedding_task_profile_revision_id,
                    task.task_kind
             FROM memory_profile_revisions AS memory
             JOIN task_profile_revisions AS task
               ON task.revision_id = ?2
             WHERE memory.revision_id = ?1",
            params![memory_revision_id, task_revision_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::NotFound,
                "memory or task profile revision was not found",
                false,
            )
        })?;
    let matches = match kind {
        MemoryJobKind::Summary => relation.0 == task_revision_id && relation.2 == "memory_summary",
        MemoryJobKind::Embedding => {
            relation.1.as_deref() == Some(task_revision_id) && relation.2 == "memory_embedding"
        }
        MemoryJobKind::InvalidateRange => false,
    };
    if !matches {
        return Err(CoreError::invalid(
            "memory job profile revisions do not belong together or have the required task kind",
        ));
    }
    Ok(())
}

fn validate_embedding_queue_input(
    connection: &Connection,
    enqueue: &MemoryJobEnqueue,
) -> CoreResult<()> {
    let input = decode_memory_embedding_job_input(&enqueue.payload)?;
    let task_profile_revision_id = enqueue
        .task_profile_revision_id
        .as_deref()
        .ok_or_else(|| CoreError::invalid("embedding job requires a task profile revision"))?;
    validate_embedding_task_space(
        connection,
        task_profile_revision_id,
        &input.model_route_id,
        input.dimensions,
    )?;
    validate_embedding_record_revision_binding(
        connection,
        &enqueue.job,
        &input.memory_record_revision_id,
    )?;
    Ok(())
}

fn decode_memory_embedding_job_input(
    payload: &VersionedJson,
) -> CoreResult<MemoryEmbeddingJobInput> {
    if payload.schema_version != 1 {
        return Err(CoreError::invalid(
            "memory embedding job input schema version must be 1",
        ));
    }
    let input = serde_json::from_value::<MemoryEmbeddingJobInput>(payload.value.clone()).map_err(
        |error| CoreError::invalid(format!("memory embedding job input is invalid: {error}")),
    )?;
    validate_identifier(
        "memory embedding record revision",
        &input.memory_record_revision_id,
    )?;
    validate_identifier(
        "memory embedding model route",
        input.model_route_id.as_str(),
    )?;
    validate_memory_embedding_dimensions(input.dimensions)?;
    validate_sha256("memory embedding vector space", &input.vector_space_sha256)?;
    Ok(input)
}

fn validate_embedding_task_space(
    connection: &Connection,
    task_profile_revision_id: &str,
    model_route_id: &ModelRouteId,
    dimensions: u32,
) -> CoreResult<()> {
    validate_identifier("task profile revision", task_profile_revision_id)?;
    validate_identifier("memory embedding model route", model_route_id.as_str())?;
    validate_memory_embedding_dimensions(dimensions)?;
    let task = load_object_revision_by_id::<TaskProfile>(
        connection,
        task_profile_revision_id,
        "task_profile",
        "task_profile_revisions",
    )?
    .ok_or_else(|| not_found("task profile revision"))?;
    task.value.validate().map_err(|error| {
        corrupted(format!(
            "stored memory embedding task profile is invalid: {error}"
        ))
    })?;
    if task.value.kind != lorepia_domain::AuxiliaryTaskKind::MemoryEmbedding
        || task.value.route_id != *model_route_id
        || task.value.embedding_dimensions != Some(dimensions)
    {
        Err(CoreError::invalid(
            "memory embedding route and dimensions do not match the exact task profile revision",
        ))
    } else {
        Ok(())
    }
}

fn validate_embedding_record_revision_binding(
    connection: &Connection,
    job: &MemoryJob,
    record_revision_id: &str,
) -> CoreResult<MemoryRecordId> {
    let row = connection
        .query_row(
            "SELECT revision.record_id, record.conversation_id,
                    record.branch_id, record.source_start_message_id,
                    record.source_end_message_id, state.active_revision_id,
                    state.invalidated_at, state.excluded_from_conversation_at,
                    state.excluded_from_character_at, state.deleted_at,
                    revision.document_json, revision.content_sha256
             FROM memory_record_revisions AS revision
             JOIN memory_records AS record ON record.id = revision.record_id
             JOIN memory_record_state AS state ON state.record_id = record.id
             WHERE revision.id = ?1",
            [record_revision_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory embedding record revision"))?;
    if row.1 != job.conversation_id.0
        || row.2 != job.branch_id.0
        || row.3 != job.source_start_message_id.0
        || row.4 != job.source_end_message_id.0
    {
        return Err(CoreError::invalid(
            "memory embedding record revision does not match the queue lineage",
        ));
    }
    if row.5 != record_revision_id
        || row.6.is_some()
        || row.7.is_some()
        || row.8.is_some()
        || row.9.is_some()
    {
        return Err(queue_conflict(
            "memory embedding record revision is no longer active and visible",
        ));
    }
    if row.10.len() > 8 * 1024 * 1024 || hex::encode(Sha256::digest(row.10.as_bytes())) != row.11 {
        return Err(corrupted(
            "memory embedding source revision content hash is invalid",
        ));
    }
    let value = serde_json::from_str::<MemoryRecord>(&row.10).map_err(|error| {
        corrupted(format!(
            "memory embedding source revision document is invalid: {error}"
        ))
    })?;
    if value.id.as_str() != row.0
        || value.conversation_id.0 != row.1
        || value.branch_id.0 != row.2
        || value.source_start_message_id.0 != row.3
        || value.source_end_message_id.0 != row.4
    {
        return Err(corrupted(
            "memory embedding source revision disagrees with normalized identity",
        ));
    }
    Ok(MemoryRecordId::from(row.0))
}

fn validate_embedding_output_against_input(
    embedding: &MemoryEmbeddingRecord,
    input: &MemoryEmbeddingJobInput,
) -> CoreResult<()> {
    if embedding.model_route_id.as_ref() != Some(&input.model_route_id)
        || embedding.dimensions != input.dimensions
    {
        return Err(CoreError::invalid(
            "memory embedding output route or dimensions differ from its immutable queue input",
        ));
    }
    Ok(())
}

fn validate_embedding_completion_bindings(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    input: &MemoryEmbeddingJobInput,
    embedding: &MemoryEmbeddingRecord,
    task_profile_revision_id: &str,
) -> CoreResult<()> {
    let record_id = validate_embedding_record_revision_binding(
        connection,
        &entry.job,
        &input.memory_record_revision_id,
    )?;
    if record_id != embedding.memory_record_id {
        return Err(CoreError::invalid(
            "memory embedding output names a different memory record revision",
        ));
    }
    validate_embedding_task_space(
        connection,
        task_profile_revision_id,
        &input.model_route_id,
        input.dimensions,
    )
}

fn validate_memory_embedding_dimensions(dimensions: u32) -> CoreResult<usize> {
    let dimensions = usize::try_from(dimensions)
        .map_err(|_| CoreError::invalid("memory embedding dimensions are invalid"))?;
    if dimensions == 0 || dimensions > MAX_MEMORY_EMBEDDING_DIMENSIONS {
        return Err(CoreError::invalid(format!(
            "memory embedding dimensions must be between 1 and {MAX_MEMORY_EMBEDDING_DIMENSIONS}",
        )));
    }
    Ok(dimensions)
}

fn encode_memory_embedding_vector(
    dimensions: u32,
    values: &[f32],
) -> CoreResult<(Vec<u8>, String)> {
    let dimensions = validate_memory_embedding_dimensions(dimensions)?;
    if values.len() != dimensions || values.iter().any(|value| !value.is_finite()) {
        return Err(CoreError::invalid(
            "memory embedding values do not match finite declared dimensions",
        ));
    }
    let norm = vector_squared_norm(values);
    if !norm.is_finite() || norm <= f64::EPSILON {
        return Err(CoreError::invalid(
            "memory embedding vector must have a non-zero finite norm",
        ));
    }
    let byte_capacity = dimensions
        .checked_mul(4)
        .ok_or_else(|| CoreError::invalid("memory embedding byte size overflow"))?;
    let mut bytes = Vec::with_capacity(byte_capacity);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let sha256 = hex::encode(Sha256::digest(&bytes));
    Ok((bytes, sha256))
}

fn decode_memory_embedding_vector(
    dimensions: u32,
    bytes: &[u8],
    expected_sha256: &str,
) -> CoreResult<Vec<f32>> {
    let dimensions = validate_memory_embedding_dimensions(dimensions).map_err(|error| {
        corrupted(format!(
            "stored memory embedding dimensions are invalid: {}",
            error.message
        ))
    })?;
    let expected_len = dimensions
        .checked_mul(4)
        .ok_or_else(|| corrupted("stored memory embedding byte size overflow"))?;
    if bytes.len() != expected_len {
        return Err(corrupted("stored memory embedding byte length is invalid"));
    }
    if hex::encode(Sha256::digest(bytes)) != expected_sha256 {
        return Err(corrupted("stored memory embedding digest is invalid"));
    }
    let values = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err(corrupted(
            "stored memory embedding contains a non-finite value",
        ));
    }
    Ok(values)
}

fn vector_squared_norm(values: &[f32]) -> f64 {
    values
        .iter()
        .map(|value| {
            let value = f64::from(*value);
            value * value
        })
        .sum()
}

fn similarity_millionths(similarity: f64) -> CoreResult<u32> {
    let rounded = (similarity.clamp(0.0, 1.0) * 1_000_000.0).round();
    if !rounded.is_finite() {
        return Err(corrupted(
            "memory embedding similarity produced a non-finite score",
        ));
    }
    format!("{rounded:.0}").parse::<u32>().map_err(|_| {
        corrupted("memory embedding similarity score is outside its fixed-point range")
    })
}

fn validate_memory_embedding_query(query: &MemoryEmbeddingQuery) -> CoreResult<()> {
    validate_identifier(
        "memory embedding query conversation",
        query.conversation_id.0.as_str(),
    )?;
    validate_identifier("memory embedding query branch", query.branch_id.0.as_str())?;
    validate_identifier(
        "memory embedding query context head",
        query.context_head_message_id.0.as_str(),
    )?;
    validate_identifier(
        "memory embedding query task profile revision",
        &query.task_profile_revision_id,
    )?;
    validate_identifier(
        "memory embedding query model route",
        query.model_route_id.as_str(),
    )?;
    validate_sha256(
        "memory embedding query vector space",
        &query.vector_space_sha256,
    )?;
    let dimensions = validate_memory_embedding_dimensions(query.dimensions)?;
    if query.values.len() != dimensions || query.values.iter().any(|value| !value.is_finite()) {
        return Err(CoreError::invalid(
            "memory embedding query values do not match finite declared dimensions",
        ));
    }
    if query.candidate_limit == 0
        || query.candidate_limit > MAX_MEMORY_EMBEDDING_CANDIDATES
        || query.result_limit == 0
        || query.result_limit > query.candidate_limit
    {
        return Err(CoreError::invalid(format!(
            "memory embedding query limits require 1 <= result <= candidate <= {MAX_MEMORY_EMBEDDING_CANDIDATES}",
        )));
    }
    let candidate_bytes = dimensions
        .checked_mul(4)
        .and_then(|bytes| bytes.checked_mul(usize::try_from(query.candidate_limit).ok()?))
        .ok_or_else(|| CoreError::invalid("memory embedding query size overflow"))?;
    if candidate_bytes > MAX_MEMORY_EMBEDDING_QUERY_BYTES {
        return Err(CoreError::invalid(format!(
            "memory embedding query candidates exceed the {MAX_MEMORY_EMBEDDING_QUERY_BYTES}-byte vector budget",
        )));
    }
    Ok(())
}

fn apply_memory_record_user_patch(
    current: &MemoryRecord,
    patch: &MemoryRecordUserPatch,
    updated_at: DateTime<Utc>,
) -> CoreResult<(MemoryRecord, Vec<&'static str>)> {
    let mut value = current.clone();
    let mut changed_fields = Vec::new();
    if let Some(title) = &patch.title
        && value.title != *title
    {
        value.title.clone_from(title);
        changed_fields.push("title");
    }
    if let Some(summary) = &patch.summary
        && value.summary != *summary
    {
        value.summary.clone_from(summary);
        changed_fields.push("summary");
    }
    if let Some(importance) = patch.importance
        && value.importance != importance
    {
        value.importance = importance;
        changed_fields.push("importance");
    }
    if let Some(keywords) = &patch.keywords
        && value.keywords != *keywords
    {
        value.keywords.clone_from(keywords);
        changed_fields.push("keywords");
    }
    if let Some(pinned) = patch.pinned
        && value.pinned != pinned
    {
        value.pinned = pinned;
        changed_fields.push("pinned");
    }
    if let Some(excluded) = patch.excluded_from_conversation
        && value.excluded_from_conversation != excluded
    {
        value.excluded_from_conversation = excluded;
        changed_fields.push("excluded_from_conversation");
    }
    if let Some(excluded) = patch.excluded_from_character
        && value.excluded_from_character != excluded
    {
        value.excluded_from_character = excluded;
        changed_fields.push("excluded_from_character");
    }
    if changed_fields.is_empty() {
        return Err(CoreError::invalid(
            "memory record user patch does not change an allowed field",
        ));
    }
    value.updated_at = updated_at;
    value
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid memory record patch: {error}")))?;
    Ok((value, changed_fields))
}

fn insert_user_memory_record_revision(
    connection: &Connection,
    id: &MemoryRecordId,
    current: &UserMemoryRecordState,
    value: &MemoryRecord,
    revision_id: &str,
    updated_at: DateTime<Utc>,
) -> CoreResult<()> {
    let keywords = normalize_memory_keywords(&value.keywords)?;
    let document_json = encode_bounded_json(value, 8 * 1024 * 1024, "memory record")?;
    let structured_data_json = encode_bounded_json(
        &value.structured_data,
        4 * 1024 * 1024,
        "memory structured data",
    )?;
    let provenance_json = encode_bounded_json(&value.provenance, 65_536, "memory provenance")?;
    let content_sha256 = hex::encode(Sha256::digest(document_json.as_bytes()));
    let next_content_revision = checked_next_revision(current.content_revision_no)?;
    let previous_revision_id = current
        .stored
        .revision_id
        .as_deref()
        .ok_or_else(|| corrupted("memory record has no active content revision"))?;
    connection
        .execute(
            "INSERT INTO memory_record_revisions
             (id, record_id, revision_no, parent_revision_id, title, summary,
              structured_data_json, importance, content_sha256,
              provenance_json, document_json, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                revision_id,
                id.as_str(),
                i64_revision(next_content_revision)?,
                previous_revision_id,
                value.title,
                value.summary,
                structured_data_json,
                value.importance,
                content_sha256,
                provenance_json,
                document_json,
                updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    insert_memory_record_keywords(connection, revision_id, &keywords)
}

fn update_user_memory_record_state(
    connection: &Connection,
    target: OwnedMemoryRecordTarget<'_>,
    current: &UserMemoryRecordState,
    value: &MemoryRecord,
    revision_id: &str,
    next_state_revision: u64,
    updated_at: DateTime<Utc>,
) -> CoreResult<u64> {
    let excluded_from_conversation_at = next_exclusion_timestamp(
        current.stored.value.excluded_from_conversation,
        value.excluded_from_conversation,
        current.excluded_from_conversation_at,
        updated_at,
    )?;
    let excluded_from_character_at = next_exclusion_timestamp(
        current.stored.value.excluded_from_character,
        value.excluded_from_character,
        current.excluded_from_character_at,
        updated_at,
    )?;
    let changed = connection
        .execute(
            "UPDATE memory_record_state
             SET active_revision_id = ?2, pinned = ?3,
                 excluded_from_conversation_at = ?4,
                 excluded_from_character_at = ?5,
                 state_version = ?6, updated_at = ?7
             WHERE record_id = ?1 AND state_version = ?8
               AND deleted_at IS NULL
               AND EXISTS (
                   SELECT 1 FROM memory_records AS record
                   WHERE record.id = memory_record_state.record_id
                     AND record.conversation_id = ?9
                     AND record.branch_id = ?10
               )",
            params![
                target.id.as_str(),
                revision_id,
                value.pinned,
                excluded_from_conversation_at.map(|time| time.to_rfc3339()),
                excluded_from_character_at.map(|time| time.to_rfc3339()),
                i64_revision(next_state_revision)?,
                updated_at.to_rfc3339(),
                i64_revision(current.stored.revision)?,
                target.conversation_id.0,
                target.branch_id.0,
            ],
        )
        .map_err(storage_db_error)?;
    ensure_memory_record_cas(changed, target.id, current.stored.revision)?;
    let previous_revision_id = current
        .stored
        .revision_id
        .as_deref()
        .ok_or_else(|| corrupted("memory record has no active content revision"))?;
    cancel_embedding_jobs_for_record_revision(connection, previous_revision_id, updated_at)
}

fn patch_memory_record_user_fields_in_transaction(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    id: &MemoryRecordId,
    expected_revision: u64,
    patch: &MemoryRecordUserPatch,
    updated_at: DateTime<Utc>,
) -> CoreResult<StoredRevision<MemoryRecord>> {
    ensure_memory_record_owner(connection, conversation_id, branch_id, id)?;
    let current = load_user_memory_record_state(connection, id)?;
    ensure_memory_record_expected_revision(&current, id, expected_revision)?;
    if current.stored.deleted_at.is_some() {
        return Err(not_found("memory record"));
    }
    if updated_at < current.stored.updated_at {
        return Err(CoreError::invalid(
            "memory record update time predates its latest durable update",
        ));
    }
    let (value, changed_fields) =
        apply_memory_record_user_patch(&current.stored.value, patch, updated_at)?;
    let next_state_revision = checked_next_revision(expected_revision)?;
    let previous_revision_id = current
        .stored
        .revision_id
        .as_deref()
        .ok_or_else(|| corrupted("memory record has no active content revision"))?;
    let revision_id = Uuid::new_v4().to_string();
    insert_user_memory_record_revision(connection, id, &current, &value, &revision_id, updated_at)?;
    let cancelled_embedding_jobs = update_user_memory_record_state(
        connection,
        OwnedMemoryRecordTarget {
            conversation_id,
            branch_id,
            id,
        },
        &current,
        &value,
        &revision_id,
        next_state_revision,
        updated_at,
    )?;
    let event_kind = user_memory_patch_event_kind(&current.stored.value, &value, &changed_fields);
    let reason = if changed_fields
        .iter()
        .all(|field| field.starts_with("excluded_from_"))
    {
        "user_exclusion_changed"
    } else {
        "user_edited"
    };
    append_user_memory_event(
        connection,
        id.as_str(),
        event_kind,
        Some(previous_revision_id),
        Some(&revision_id),
        serde_json::json!({
            "state_version": next_state_revision,
            "reason": reason,
            "changed_fields": changed_fields,
            "preserved_invalidation_reason": current.invalidation_reason,
            "cancelled_embedding_jobs": cancelled_embedding_jobs,
        }),
        updated_at,
    )?;

    Ok(StoredRevision {
        value,
        revision: next_state_revision,
        revision_id: Some(revision_id),
        created_at: current.stored.created_at,
        updated_at,
        deleted_at: None,
    })
}

fn query_user_memory_record_state(
    connection: &Connection,
    id: &MemoryRecordId,
) -> CoreResult<RawUserMemoryRecordState> {
    connection
        .query_row(
            "SELECT record.conversation_id, record.branch_id,
                    record.source_start_message_id, record.source_end_message_id,
                    record.kind, record.created_at, state.state_version,
                    state.active_revision_id, state.updated_at, state.deleted_at,
                    state.pinned, state.invalidated_at,
                    state.invalidation_reason,
                    state.excluded_from_conversation_at,
                    state.excluded_from_character_at,
                    revision.document_json, revision.content_sha256,
                    revision.revision_no
             FROM memory_records AS record
             JOIN memory_record_state AS state ON state.record_id = record.id
             JOIN memory_record_revisions AS revision
               ON revision.id = state.active_revision_id
             WHERE record.id = ?1",
            [id.as_str()],
            |row| {
                Ok(RawUserMemoryRecordState {
                    conversation_id: row.get(0)?,
                    branch_id: row.get(1)?,
                    source_start_message_id: row.get(2)?,
                    source_end_message_id: row.get(3)?,
                    kind: row.get(4)?,
                    record_created_at: row.get(5)?,
                    state_revision: row.get(6)?,
                    active_revision_id: row.get(7)?,
                    state_updated_at: row.get(8)?,
                    deleted_at: row.get(9)?,
                    pinned: row.get(10)?,
                    invalidated_at: row.get(11)?,
                    invalidation_reason: row.get(12)?,
                    excluded_from_conversation_at: row.get(13)?,
                    excluded_from_character_at: row.get(14)?,
                    document_json: row.get(15)?,
                    content_sha256: row.get(16)?,
                    content_revision_no: row.get(17)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory record"))
}

fn load_user_memory_record_state(
    connection: &Connection,
    id: &MemoryRecordId,
) -> CoreResult<UserMemoryRecordState> {
    let row = query_user_memory_record_state(connection, id)?;
    if row.document_json.len() > 8 * 1024 * 1024
        || hex::encode(Sha256::digest(row.document_json.as_bytes())) != row.content_sha256
    {
        return Err(corrupted("stored memory record revision hash is invalid"));
    }
    let mut value = serde_json::from_str::<MemoryRecord>(&row.document_json)
        .map_err(|error| corrupted(format!("stored memory record revision is invalid: {error}")))?;
    let created_at = parse_datetime("memory record created_at", &row.record_created_at)?;
    let updated_at = parse_datetime("memory record updated_at", &row.state_updated_at)?;
    let deleted_at = row
        .deleted_at
        .as_deref()
        .map(|time| parse_datetime("memory record deleted_at", time))
        .transpose()?;
    let invalidated_at = row
        .invalidated_at
        .as_deref()
        .map(|time| parse_datetime("memory record invalidated_at", time))
        .transpose()?;
    validate_memory_invalidation_reason(invalidated_at, row.invalidation_reason.as_deref())?;
    let excluded_from_conversation_at = row
        .excluded_from_conversation_at
        .as_deref()
        .map(|time| parse_datetime("memory conversation exclusion", time))
        .transpose()?;
    let excluded_from_character_at = row
        .excluded_from_character_at
        .as_deref()
        .map(|time| parse_datetime("memory character exclusion", time))
        .transpose()?;
    if [
        invalidated_at,
        excluded_from_conversation_at,
        excluded_from_character_at,
    ]
    .into_iter()
    .flatten()
    .any(|time| time < created_at || time > updated_at)
        || deleted_at.is_some_and(|time| time < created_at || time > updated_at)
    {
        return Err(corrupted(
            "stored memory record state timestamp is outside its durable lifetime",
        ));
    }
    if value.id != *id
        || value.conversation_id.0 != row.conversation_id
        || value.branch_id.0 != row.branch_id
        || value.source_start_message_id.0 != row.source_start_message_id
        || value.source_end_message_id.0 != row.source_end_message_id
        || memory_kind_to_str(value.kind) != row.kind
        || value.created_at != created_at
    {
        return Err(corrupted(
            "stored memory record revision disagrees with normalized identity",
        ));
    }
    value.created_at = created_at;
    value.updated_at = updated_at;
    value.pinned = row.pinned;
    value.invalidated_at = invalidated_at;
    value.excluded_from_conversation = excluded_from_conversation_at.is_some();
    value.excluded_from_character = excluded_from_character_at.is_some();
    value
        .validate()
        .map_err(|error| corrupted(format!("stored memory record is invalid: {error}")))?;
    Ok(UserMemoryRecordState {
        stored: StoredRevision {
            value,
            revision: u64::try_from(row.state_revision)
                .map_err(|_| corrupted("memory record state revision is invalid"))?,
            revision_id: Some(row.active_revision_id),
            created_at,
            updated_at,
            deleted_at,
        },
        content_revision_no: u64::try_from(row.content_revision_no)
            .map_err(|_| corrupted("memory record content revision is invalid"))?,
        invalidation_reason: row.invalidation_reason,
        excluded_from_conversation_at,
        excluded_from_character_at,
    })
}

fn ensure_memory_record_expected_revision(
    current: &UserMemoryRecordState,
    id: &MemoryRecordId,
    expected_revision: u64,
) -> CoreResult<()> {
    if current.stored.revision == expected_revision {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            format!(
                "memory record {} revision conflict: expected {}, actual {}",
                id.as_str(),
                expected_revision,
                current.stored.revision,
            ),
            true,
        ))
    }
}

fn ensure_memory_record_cas(
    changed: usize,
    id: &MemoryRecordId,
    expected_revision: u64,
) -> CoreResult<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            format!(
                "memory record {} revision conflict at expected revision {}",
                id.as_str(),
                expected_revision,
            ),
            true,
        ))
    }
}

fn next_exclusion_timestamp(
    was_excluded: bool,
    is_excluded: bool,
    previous: Option<DateTime<Utc>>,
    updated_at: DateTime<Utc>,
) -> CoreResult<Option<DateTime<Utc>>> {
    match (was_excluded, is_excluded, previous) {
        (false, false, None) | (true, false, Some(_)) => Ok(None),
        (false, true, None) => Ok(Some(updated_at)),
        (true, true, Some(previous)) => Ok(Some(previous)),
        _ => Err(corrupted(
            "memory exclusion flag and timestamp are inconsistent",
        )),
    }
}

fn user_memory_patch_event_kind(
    previous: &MemoryRecord,
    current: &MemoryRecord,
    changed_fields: &[&str],
) -> &'static str {
    if changed_fields == ["pinned"] {
        if current.pinned { "pinned" } else { "unpinned" }
    } else if changed_fields == ["excluded_from_conversation"]
        && !previous.excluded_from_conversation
        && current.excluded_from_conversation
    {
        "excluded_conversation"
    } else if changed_fields == ["excluded_from_character"]
        && !previous.excluded_from_character
        && current.excluded_from_character
    {
        "excluded_character"
    } else {
        "edited"
    }
}

fn cancel_embedding_jobs_for_record_revision(
    connection: &Connection,
    record_revision_id: &str,
    cancelled_at: DateTime<Utc>,
) -> CoreResult<u64> {
    let changed = connection
        .execute(
            "UPDATE memory_jobs
             SET state = 'cancelled', revision = revision + 1,
                 finished_at = ?2, result_record_id = NULL,
                 failure_json = NULL, updated_at = ?2
             WHERE job_kind = 'embedding'
               AND state IN ('queued', 'running', 'interrupted')
               AND json_extract(
                   payload_json,
                   '$.input.value.memory_record_revision_id'
               ) = ?1",
            params![record_revision_id, cancelled_at.to_rfc3339()],
        )
        .map_err(storage_db_error)?;
    u64::try_from(changed)
        .map_err(|_| CoreError::internal("cancelled embedding job count overflow"))
}

fn append_user_memory_event(
    connection: &Connection,
    record_id: &str,
    event_kind: &str,
    from_revision_id: Option<&str>,
    to_revision_id: Option<&str>,
    payload: Value,
    created_at: DateTime<Utc>,
) -> CoreResult<()> {
    let payload_json = encode_bounded_json(&payload, 262_144, "memory record event")?;
    connection
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

fn validate_memory_invalidation_reason(
    invalidated_at: Option<DateTime<Utc>>,
    reason: Option<&str>,
) -> CoreResult<()> {
    match (invalidated_at, reason) {
        (None, None) | (Some(_), Some("source_range_changed" | "record_update")) => Ok(()),
        (Some(_), Some(_)) => Err(corrupted(
            "stored memory invalidation reason is outside the closed set",
        )),
        _ => Err(corrupted(
            "stored memory invalidation timestamp and reason are inconsistent",
        )),
    }
}

const fn memory_kind_to_str(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::EpisodicEvent => "episodic_event",
        MemoryKind::CharacterFact => "character_fact",
        MemoryKind::RelationshipChange => "relationship_change",
        MemoryKind::UserPreference => "user_preference",
        MemoryKind::WorldState => "world_state",
        MemoryKind::UnresolvedThread => "unresolved_thread",
        MemoryKind::ConversationSummary => "conversation_summary",
        MemoryKind::CreatorPinned => "creator_pinned",
    }
}

fn ensure_exact_replay(
    existing: &StoredMemoryJobQueueEntry,
    input: &MemoryJobEnqueue,
) -> CoreResult<()> {
    let exact = existing.job.idempotency_key == input.job.idempotency_key
        && existing.job.kind == input.job.kind
        && existing.job.conversation_id == input.job.conversation_id
        && existing.job.branch_id == input.job.branch_id
        && existing.job.source_start_message_id == input.job.source_start_message_id
        && existing.job.source_end_message_id == input.job.source_end_message_id
        && existing.memory_profile_revision_id == input.memory_profile_revision_id
        && existing.task_profile_revision_id == input.task_profile_revision_id
        && existing.input_fingerprint_sha256 == input.input_fingerprint_sha256
        && existing.payload == input.payload;
    if exact {
        Ok(())
    } else {
        Err(queue_conflict(
            "memory job idempotency key was reused with different immutable input",
        ))
    }
}

fn load_queue_entry_by_idempotency_key(
    connection: &Connection,
    idempotency_key: &str,
) -> CoreResult<Option<StoredMemoryJobQueueEntry>> {
    let id = connection
        .query_row(
            "SELECT id FROM memory_jobs WHERE idempotency_key = ?1",
            [idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    id.map(|id| load_queue_entry(connection, &id))
        .transpose()
        .map(Option::flatten)
}

fn load_queue_entry(
    connection: &Connection,
    id: &str,
) -> CoreResult<Option<StoredMemoryJobQueueEntry>> {
    let row = connection
        .query_row(
            "SELECT id, idempotency_key, job_kind,
                    memory_profile_revision_id, task_profile_revision_id,
                    conversation_id, branch_id, source_start_message_id,
                    source_end_message_id, input_fingerprint_sha256, state,
                    revision, attempts, available_at, started_at, finished_at,
                    result_record_id, failure_json, payload_json,
                    created_at, updated_at
             FROM memory_jobs WHERE id = ?1",
            [id],
            |row| {
                Ok(QueueRow {
                    id: row.get(0)?,
                    idempotency_key: row.get(1)?,
                    job_kind: row.get(2)?,
                    memory_profile_revision_id: row.get(3)?,
                    task_profile_revision_id: row.get(4)?,
                    conversation_id: row.get(5)?,
                    branch_id: row.get(6)?,
                    source_start_message_id: row.get(7)?,
                    source_end_message_id: row.get(8)?,
                    input_fingerprint_sha256: row.get(9)?,
                    state: row.get(10)?,
                    revision: row.get(11)?,
                    attempts: row.get(12)?,
                    available_at: row.get(13)?,
                    started_at: row.get(14)?,
                    finished_at: row.get(15)?,
                    result_record_id: row.get(16)?,
                    failure_json: row.get(17)?,
                    payload_json: row.get(18)?,
                    created_at: row.get(19)?,
                    updated_at: row.get(20)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    row.map(|row| decode_queue_row(connection, row)).transpose()
}

fn ensure_memory_job_owner(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    id: &MemoryJobId,
) -> CoreResult<()> {
    let owned = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM memory_jobs
                 WHERE id = ?1 AND conversation_id = ?2 AND branch_id = ?3
             )",
            params![id.as_str(), conversation_id.0, branch_id.0],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if owned {
        Ok(())
    } else {
        Err(not_found("memory job"))
    }
}

fn ensure_memory_record_owner(
    connection: &Connection,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    id: &MemoryRecordId,
) -> CoreResult<()> {
    let owned = connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM memory_records
                 WHERE id = ?1 AND conversation_id = ?2 AND branch_id = ?3
             )",
            params![id.as_str(), conversation_id.0, branch_id.0],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if owned {
        Ok(())
    } else {
        Err(not_found("memory record"))
    }
}

fn decode_queue_row(
    connection: &Connection,
    row: QueueRow,
) -> CoreResult<StoredMemoryJobQueueEntry> {
    validate_sha256(
        "stored memory job input fingerprint",
        &row.input_fingerprint_sha256,
    )
    .map_err(|_| corrupted("stored memory job input fingerprint is invalid"))?;
    let decoded = decode_queue_payload(&row.payload_json)?;
    let mut job = decoded.job;
    let kind = str_to_job_kind(&row.job_kind)?;
    let status = str_to_job_status(&row.state)?;
    let attempt = u32::try_from(row.attempts)
        .map_err(|_| corrupted("stored memory job attempt count is invalid"))?;
    let revision = u64::try_from(row.revision)
        .map_err(|_| corrupted("stored memory job revision is invalid"))?;
    let created_at = parse_datetime("memory job created_at", &row.created_at)?;
    let updated_at = parse_datetime("memory job updated_at", &row.updated_at)?;
    let available_at = parse_datetime("memory job available_at", &row.available_at)?;
    let started_at = row
        .started_at
        .as_deref()
        .map(|value| parse_datetime("memory job started_at", value))
        .transpose()?;
    let finished_at = row
        .finished_at
        .as_deref()
        .map(|value| parse_datetime("memory job finished_at", value))
        .transpose()?;
    let error_code = decode_failure_code(row.failure_json.as_deref())?;

    if job.id.as_str() != row.id
        || job.idempotency_key != row.idempotency_key
        || job.kind != kind
        || job.conversation_id.0 != row.conversation_id
        || job.branch_id.0 != row.branch_id
        || job.source_start_message_id.0 != row.source_start_message_id
        || job.source_end_message_id.0 != row.source_end_message_id
    {
        return Err(corrupted(
            "stored memory job payload identity disagrees with normalized columns",
        ));
    }
    job.status = status;
    job.attempt = attempt;
    job.created_at = created_at;
    job.updated_at = updated_at;
    job.error_code = error_code;
    validate_state_shape(status, started_at, finished_at, job.error_code.as_deref())?;

    let memory_profile_revision = row
        .memory_profile_revision_id
        .as_deref()
        .map(|revision_id| {
            load_object_revision_by_id(
                connection,
                revision_id,
                "memory_profile",
                "memory_profile_revisions",
            )?
            .ok_or_else(|| corrupted("stored memory profile revision is missing"))
        })
        .transpose()?;
    let task_profile_revision = row
        .task_profile_revision_id
        .as_deref()
        .map(|revision_id| {
            load_object_revision_by_id(
                connection,
                revision_id,
                "task_profile",
                "task_profile_revisions",
            )?
            .ok_or_else(|| corrupted("stored task profile revision is missing"))
        })
        .transpose()?;

    Ok(StoredMemoryJobQueueEntry {
        job,
        revision,
        memory_profile_revision_id: row.memory_profile_revision_id,
        task_profile_revision_id: row.task_profile_revision_id,
        input_fingerprint_sha256: row.input_fingerprint_sha256,
        payload: decoded.input,
        memory_profile_revision,
        task_profile_revision,
        available_at,
        started_at,
        finished_at,
        result_record_id: row.result_record_id.map(MemoryRecordId::from),
        attempt_started_at: decoded.attempt_started_at,
        interruptions: decoded.interruptions,
    })
}

fn encode_entry_payload(entry: &StoredMemoryJobQueueEntry) -> CoreResult<String> {
    encode_queue_payload(&QueuePayload {
        queue_schema_version: QUEUE_PAYLOAD_SCHEMA_VERSION,
        job: entry.job.clone(),
        input: entry.payload.clone(),
        attempt_started_at: entry.attempt_started_at.clone(),
        interruptions: entry.interruptions.clone(),
    })
}

fn encode_queue_payload(payload: &QueuePayload) -> CoreResult<String> {
    validate_versioned_payload(&payload.input)?;
    if payload.attempt_started_at.len() > usize::try_from(MAX_MEMORY_JOB_ATTEMPTS).unwrap_or(32) {
        return Err(CoreError::invalid(
            "memory job attempt history exceeds the bounded maximum",
        ));
    }
    if payload.interruptions.len() > usize::try_from(MAX_MEMORY_JOB_ATTEMPTS).unwrap_or(32) {
        return Err(CoreError::invalid(
            "memory job interruption history exceeds the bounded maximum",
        ));
    }
    let json = serde_json::to_string(payload).map_err(|error| {
        CoreError::invalid(format!("cannot encode memory job payload: {error}"))
    })?;
    validate_json_bounds(&json, "memory job payload")?;
    Ok(json)
}

fn decode_queue_payload(json: &str) -> CoreResult<QueuePayload> {
    validate_stored_json_bounds(json, "memory job payload")?;
    let value = serde_json::from_str::<Value>(json)
        .map_err(|error| corrupted(format!("stored memory job payload is invalid: {error}")))?;
    if value.get("queue_schema_version").is_some() {
        let payload = serde_json::from_value::<QueuePayload>(value).map_err(|error| {
            corrupted(format!("stored memory queue payload is invalid: {error}"))
        })?;
        if payload.queue_schema_version != QUEUE_PAYLOAD_SCHEMA_VERSION {
            return Err(corrupted(
                "stored memory queue payload schema version is unsupported",
            ));
        }
        validate_versioned_payload(&payload.input)
            .map_err(|_| corrupted("stored memory queue input payload is invalid"))?;
        return Ok(payload);
    }

    // Legacy jobs stored the domain job directly.  Normalized columns remain
    // authoritative and the missing reviewed input is represented explicitly.
    let job = serde_json::from_value::<MemoryJob>(value)
        .map_err(|error| corrupted(format!("stored legacy memory job is invalid: {error}")))?;
    Ok(QueuePayload {
        queue_schema_version: QUEUE_PAYLOAD_SCHEMA_VERSION,
        job,
        input: VersionedJson {
            schema_version: 1,
            value: serde_json::json!({"legacy_payload": true}),
        },
        attempt_started_at: Vec::new(),
        interruptions: Vec::new(),
    })
}

fn validate_versioned_payload(payload: &VersionedJson) -> CoreResult<()> {
    if payload.schema_version == 0 {
        return Err(CoreError::invalid(
            "memory job payload schema version must be positive",
        ));
    }
    if !payload.value.is_object() {
        return Err(CoreError::invalid(
            "memory job payload value must be a JSON object",
        ));
    }
    let json = serde_json::to_string(payload).map_err(|error| {
        CoreError::invalid(format!(
            "cannot encode versioned memory job payload: {error}"
        ))
    })?;
    validate_json_bounds(&json, "versioned memory job input")
}

fn encode_bounded_json<T>(value: &T, maximum_bytes: usize, label: &str) -> CoreResult<String>
where
    T: Serialize,
{
    let json = serde_json::to_string(value)
        .map_err(|error| CoreError::invalid(format!("cannot encode {label}: {error}")))?;
    if json.len() > maximum_bytes {
        return Err(CoreError::invalid(format!(
            "{label} exceeds the {maximum_bytes}-byte limit",
        )));
    }
    let parsed = serde_json::from_str::<Value>(&json)
        .map_err(|error| CoreError::invalid(format!("{label} is invalid JSON: {error}")))?;
    if !parsed.is_object() {
        return Err(CoreError::invalid(format!(
            "{label} must encode as a JSON object",
        )));
    }
    let mut nodes = 0_usize;
    inspect_json_shape(&parsed, 0, &mut nodes).map_err(CoreError::invalid)?;
    Ok(json)
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

fn validate_memory_record_source_range(
    connection: &Connection,
    record: &MemoryRecord,
) -> CoreResult<()> {
    let ordered = connection
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
            params![
                record.conversation_id.0,
                record.source_end_message_id.0,
                record.source_start_message_id.0,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    let visible = connection
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
             SELECT EXISTS(SELECT 1 FROM lineage WHERE id = ?3)",
            params![
                record.conversation_id.0,
                record.branch_id.0,
                record.source_end_message_id.0,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if ordered && visible {
        Ok(())
    } else {
        Err(CoreError::invalid(
            "memory summary source is not an ordered visible branch range",
        ))
    }
}

fn validate_terminal_result_record(
    connection: &Connection,
    entry: &StoredMemoryJobQueueEntry,
    result_record_id: Option<&MemoryRecordId>,
) -> CoreResult<()> {
    if entry.job.kind == MemoryJobKind::InvalidateRange && result_record_id.is_some() {
        return Err(CoreError::invalid(
            "an invalidate_range job cannot return a memory record",
        ));
    }
    let Some(result_record_id) = result_record_id else {
        return Ok(());
    };
    let identity = connection
        .query_row(
            "SELECT conversation_id, branch_id, source_start_message_id,
                    source_end_message_id
             FROM memory_records WHERE id = ?1",
            [result_record_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory job result record"))?;
    if identity.0 != entry.job.conversation_id.0
        || identity.1 != entry.job.branch_id.0
        || identity.2 != entry.job.source_start_message_id.0
        || identity.3 != entry.job.source_end_message_id.0
    {
        return Err(CoreError::invalid(
            "memory job result record does not match its immutable source lineage",
        ));
    }
    Ok(())
}

fn load_memory_record_for_completion(
    connection: &Connection,
    record_id: &str,
) -> CoreResult<Option<StoredRevision<MemoryRecord>>> {
    let row = connection
        .query_row(
            "SELECT revision.document_json, revision.content_sha256,
                    revision.id, record.created_at
             FROM memory_records AS record
             JOIN memory_record_revisions AS revision
               ON revision.record_id = record.id
              AND revision.revision_no = 1
             WHERE record.id = ?1",
            [record_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    row.map(
        |(document_json, content_sha256, revision_id, record_created_at)| {
            if document_json.len() > 8 * 1024 * 1024 {
                return Err(corrupted("stored memory record document is oversized"));
            }
            if hex::encode(Sha256::digest(document_json.as_bytes())) != content_sha256 {
                return Err(corrupted("stored memory record document hash is invalid"));
            }
            let value = serde_json::from_str::<MemoryRecord>(&document_json).map_err(|error| {
                corrupted(format!("stored memory record document is invalid: {error}"))
            })?;
            let created_at = parse_datetime("memory record created_at", &record_created_at)?;
            if value.id.as_str() != record_id || value.created_at != created_at {
                return Err(corrupted(
                    "stored memory record identity disagrees with normalized columns",
                ));
            }
            Ok(StoredRevision {
                updated_at: value.updated_at,
                value,
                revision: 1,
                revision_id: Some(revision_id),
                created_at,
                deleted_at: None,
            })
        },
    )
    .transpose()
}

fn load_memory_record_revision_by_id(
    connection: &Connection,
    revision_id: &str,
) -> CoreResult<Option<ObjectRevision<MemoryRecord>>> {
    let row = connection
        .query_row(
            "SELECT revision.record_id, revision.revision_no,
                    revision.document_json, revision.content_sha256,
                    revision.created_at
             FROM memory_record_revisions AS revision
             WHERE revision.id = ?1",
            [revision_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    row.map(
        |(record_id, revision_no, document_json, content_sha256, created_at)| {
            if document_json.len() > 8 * 1024 * 1024 {
                return Err(corrupted("stored memory record document is oversized"));
            }
            let actual_sha256 = hex::encode(Sha256::digest(document_json.as_bytes()));
            if actual_sha256 != content_sha256 {
                return Err(corrupted("stored memory record document hash is invalid"));
            }
            let value = serde_json::from_str::<MemoryRecord>(&document_json).map_err(|error| {
                corrupted(format!("stored memory record document is invalid: {error}"))
            })?;
            if value.id.as_str() != record_id {
                return Err(corrupted(
                    "stored memory record revision identity is inconsistent",
                ));
            }
            value.validate().map_err(|error| {
                corrupted(format!("stored memory record revision is invalid: {error}"))
            })?;
            Ok(ObjectRevision {
                revision_id: revision_id.to_owned(),
                object_kind: "memory_record".to_owned(),
                object_id: record_id,
                revision: u64::try_from(revision_no)
                    .map_err(|_| corrupted("stored memory record revision number is invalid"))?,
                value,
                sha256: content_sha256,
                created_at: parse_datetime("memory record revision created_at", &created_at)?,
            })
        },
    )
    .transpose()
}

fn load_memory_embedding_exact(
    connection: &Connection,
    embedding_id: &str,
) -> CoreResult<Option<StoredMemoryEmbedding>> {
    let row = connection
        .query_row(
            "SELECT embedding.id, revision.record_id,
                    embedding.record_revision_id,
                    embedding.task_profile_revision_id,
                    embedding.model_route_id, embedding.dimensions,
                    embedding.vector_space_sha256, embedding.vector_blob,
                    embedding.vector_sha256, embedding.created_at
             FROM memory_embeddings AS embedding
             JOIN memory_record_revisions AS revision
               ON revision.id = embedding.record_revision_id
             WHERE embedding.id = ?1 AND embedding.encoding = 'f32le'",
            [embedding_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Vec<u8>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    row.map(
        |(
            id,
            record_id,
            record_revision_id,
            task_profile_revision_id,
            model_route_id,
            dimensions,
            vector_space_sha256,
            vector_blob,
            vector_sha256,
            created_at,
        )| {
            let dimensions = u32::try_from(dimensions)
                .map_err(|_| corrupted("stored memory embedding dimensions are invalid"))?;
            let values = decode_memory_embedding_vector(dimensions, &vector_blob, &vector_sha256)?;
            let task_profile_revision_id = task_profile_revision_id.ok_or_else(|| {
                corrupted("job-produced memory embedding has no task profile revision")
            })?;
            validate_sha256("stored memory embedding vector space", &vector_space_sha256)
                .map_err(|error| corrupted(error.message))?;
            Ok(StoredMemoryEmbedding {
                value: MemoryEmbeddingRecord {
                    id,
                    memory_record_id: MemoryRecordId::from(record_id),
                    model_route_id: Some(ModelRouteId::from(model_route_id)),
                    dimensions,
                    values,
                    created_at: parse_datetime("memory embedding created_at", &created_at)?,
                },
                memory_record_revision_id: record_revision_id,
                task_profile_revision_id,
                vector_space_sha256,
                vector_sha256,
            })
        },
    )
    .transpose()
}

fn validate_json_bounds(json: &str, label: &str) -> CoreResult<()> {
    if json.len() > MAX_QUEUE_PAYLOAD_BYTES {
        return Err(CoreError::invalid(format!(
            "{label} exceeds the {MAX_QUEUE_PAYLOAD_BYTES}-byte limit",
        )));
    }
    let value = serde_json::from_str::<Value>(json)
        .map_err(|error| CoreError::invalid(format!("{label} is invalid JSON: {error}")))?;
    let mut nodes = 0_usize;
    inspect_json_shape(&value, 0, &mut nodes).map_err(CoreError::invalid)
}

fn validate_stored_json_bounds(json: &str, label: &str) -> CoreResult<()> {
    validate_json_bounds(json, label).map_err(|error| {
        corrupted(format!(
            "{label} failed bounds validation: {}",
            error.message
        ))
    })
}

fn inspect_json_shape(value: &Value, depth: usize, nodes: &mut usize) -> Result<(), String> {
    if depth > MAX_QUEUE_JSON_DEPTH {
        return Err(format!(
            "JSON nesting exceeds the depth limit of {MAX_QUEUE_JSON_DEPTH}",
        ));
    }
    *nodes = nodes
        .checked_add(1)
        .ok_or_else(|| "JSON node count overflow".to_owned())?;
    if *nodes > MAX_QUEUE_JSON_NODES {
        return Err(format!(
            "JSON node count exceeds the limit of {MAX_QUEUE_JSON_NODES}",
        ));
    }
    match value {
        Value::Array(values) => {
            for value in values {
                inspect_json_shape(value, depth + 1, nodes)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                inspect_json_shape(value, depth + 1, nodes)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn load_object_revision_by_id<T>(
    connection: &Connection,
    revision_id: &str,
    object_kind: &str,
    projection_table: &str,
) -> CoreResult<Option<ObjectRevision<T>>>
where
    T: DeserializeOwned,
{
    // `projection_table` is selected only from the two compile-time literals
    // above; it is never caller-controlled.
    let sql = format!(
        "SELECT revision.object_id, revision.revision_no,
                revision.document_json, revision.document_sha256,
                revision.created_at
         FROM content_revisions AS revision
         JOIN {projection_table} AS projection
           ON projection.revision_id = revision.id
         WHERE revision.id = ?1 AND revision.object_kind = ?2",
    );
    let row = connection
        .query_row(&sql, params![revision_id, object_kind], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .optional()
        .map_err(storage_db_error)?;
    row.map(|(object_id, revision, document_json, sha256, created_at)| {
        validate_stored_json_bounds(&document_json, object_kind)?;
        let value = serde_json::from_str::<T>(&document_json).map_err(|error| {
            corrupted(format!(
                "stored {object_kind} revision document is invalid: {error}",
            ))
        })?;
        let actual_sha256 = hex::encode(Sha256::digest(document_json.as_bytes()));
        if actual_sha256 != sha256 {
            return Err(corrupted(format!(
                "stored {object_kind} revision hash is invalid",
            )));
        }
        Ok(ObjectRevision {
            revision_id: revision_id.to_owned(),
            object_kind: object_kind.to_owned(),
            object_id,
            revision: u64::try_from(revision)
                .map_err(|_| corrupted("stored content revision number is invalid"))?,
            value,
            sha256,
            created_at: parse_datetime("content revision created_at", &created_at)?,
        })
    })
    .transpose()
}

fn decode_failure_code(failure_json: Option<&str>) -> CoreResult<Option<String>> {
    let Some(failure_json) = failure_json else {
        return Ok(None);
    };
    if failure_json.len() > 65_536 {
        return Err(corrupted("stored memory job failure is oversized"));
    }
    let value = serde_json::from_str::<Value>(failure_json)
        .map_err(|error| corrupted(format!("stored memory job failure is invalid: {error}")))?;
    let code = value
        .get("error_code")
        .and_then(Value::as_str)
        .ok_or_else(|| corrupted("stored memory job failure code is missing"))?;
    validate_error_code(code)
        .map_err(|_| corrupted("stored memory job failure code is invalid"))?;
    Ok(Some(code.to_owned()))
}

fn validate_state_shape(
    status: MemoryJobStatus,
    started_at: Option<DateTime<Utc>>,
    finished_at: Option<DateTime<Utc>>,
    error_code: Option<&str>,
) -> CoreResult<()> {
    let valid_times = match status {
        MemoryJobStatus::Queued => started_at.is_none() && finished_at.is_none(),
        MemoryJobStatus::Running | MemoryJobStatus::Interrupted => {
            started_at.is_some() && finished_at.is_none()
        }
        MemoryJobStatus::Succeeded | MemoryJobStatus::Failed | MemoryJobStatus::Cancelled => {
            finished_at.is_some()
        }
    };
    let valid_failure = (status == MemoryJobStatus::Failed) == error_code.is_some();
    if !valid_times || !valid_failure {
        return Err(corrupted(
            "stored memory job state timestamps or failure are inconsistent",
        ));
    }
    Ok(())
}

fn parse_datetime(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| corrupted(format!("stored {label} is invalid: {error}")))
}

fn validate_identifier(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(format!(
            "{label} is empty, oversized, untrimmed, or contains control characters",
        )));
    }
    Ok(())
}

fn validate_optional_identifier(label: &str, value: Option<&str>) -> CoreResult<()> {
    if let Some(value) = value {
        validate_identifier(label, value)?;
    }
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> CoreResult<()> {
    if value.len() != 64
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(CoreError::invalid(format!(
            "{label} must be a lowercase SHA-256 digest",
        )));
    }
    Ok(())
}

fn validate_error_code(value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 128
        || value.bytes().any(|byte| {
            !(byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b'-' | b'.'))
        })
    {
        return Err(CoreError::invalid(
            "memory job error code must be a bounded lowercase machine identifier",
        ));
    }
    Ok(())
}

fn ensure_expected_revision(
    entry: &StoredMemoryJobQueueEntry,
    expected_revision: u64,
) -> CoreResult<()> {
    if entry.revision == expected_revision {
        Ok(())
    } else {
        Err(queue_conflict(format!(
            "memory job revision conflict: expected {expected_revision}, current {}",
            entry.revision,
        )))
    }
}

fn checked_next_revision(revision: u64) -> CoreResult<u64> {
    revision
        .checked_add(1)
        .ok_or_else(|| CoreError::internal("memory job revision overflow"))
}

fn i64_revision(revision: u64) -> CoreResult<i64> {
    i64::try_from(revision)
        .map_err(|_| CoreError::invalid("memory job revision exceeds SQLite integer range"))
}

fn ensure_cas(changed: usize, operation: &str) -> CoreResult<()> {
    if changed == 1 {
        Ok(())
    } else {
        Err(queue_conflict(format!(
            "{operation} lost its compare-and-swap race",
        )))
    }
}

const fn job_kind_to_str(kind: MemoryJobKind) -> &'static str {
    match kind {
        MemoryJobKind::Summary => "summary",
        MemoryJobKind::Embedding => "embedding",
        MemoryJobKind::InvalidateRange => "invalidate_range",
    }
}

fn str_to_job_kind(value: &str) -> CoreResult<MemoryJobKind> {
    match value {
        "summary" => Ok(MemoryJobKind::Summary),
        "embedding" => Ok(MemoryJobKind::Embedding),
        "invalidate_range" => Ok(MemoryJobKind::InvalidateRange),
        _ => Err(corrupted("stored memory job kind is invalid")),
    }
}

fn str_to_job_status(value: &str) -> CoreResult<MemoryJobStatus> {
    match value {
        "queued" => Ok(MemoryJobStatus::Queued),
        "running" => Ok(MemoryJobStatus::Running),
        "interrupted" => Ok(MemoryJobStatus::Interrupted),
        "succeeded" => Ok(MemoryJobStatus::Succeeded),
        "failed" => Ok(MemoryJobStatus::Failed),
        "cancelled" => Ok(MemoryJobStatus::Cancelled),
        _ => Err(corrupted("stored memory job state is invalid")),
    }
}

fn not_found(label: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{label} was not found"),
        false,
    )
}

fn queue_conflict(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::InvalidInput, message, true)
}

fn corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    use chrono::Duration;
    use lorepia_domain::{
        AuxiliaryTaskKind, Conversation, ConversationBranchId, ConversationId, GenerationPresetId,
        MemoryProfileId, Message, MessageId, ModelRouteId, Provenance, RateLimit, SourceKind,
        SummarySchemaId, TaskProfileId, TokenBudget,
    };
    use tempfile::TempDir;

    use super::*;

    struct QueueFixture {
        _root: TempDir,
        storage: Arc<Storage>,
        conversation_id: ConversationId,
        branch_id: ConversationBranchId,
        source_start_message_id: MessageId,
        source_end_message_id: MessageId,
        memory_profile_revision_id: String,
        task_profile_revision_id: String,
        embedding_task_profile_revision_id: String,
    }

    fn seed_provider_test_catalog(storage: &Storage, now: DateTime<Utc>) {
        let source_hash = "a".repeat(64);
        let manifest_json = "{}";
        let manifest_sha256 = hex::encode(Sha256::digest(manifest_json.as_bytes()));
        let connection = storage.connection().expect("database connection");
        connection
            .execute(
                "INSERT INTO content_sources
                 (sha256, relative_path, size_bytes, created_at)
                 VALUES (?1, 'sha256/aa/test', 1, ?2)",
                params![source_hash, now.to_rfc3339()],
            )
            .expect("content source");
        connection
            .execute(
                "INSERT INTO characters
                 (id, name, description, source_hash, avatar_asset_hash, created_at)
                 VALUES ('character:test', 'Test', '', ?1, NULL, ?2)",
                params![source_hash, now.to_rfc3339()],
            )
            .expect("character");
        connection
            .execute(
                "INSERT INTO provider_templates
                 (id, version, display_name, source_kind, manifest_json,
                  manifest_sha256, created_at)
                 VALUES ('template:test', 1, 'Test', 'built_in', ?1, ?2, ?3)",
                params![manifest_json, manifest_sha256, now.to_rfc3339()],
            )
            .expect("provider template");
        connection
            .execute(
                "INSERT INTO provider_connections
                 (id, template_id, template_version, display_name, api_origin,
                  config_json, credential_ref, credential_scope_json,
                  timeout_seconds, status, created_at, updated_at)
                 VALUES (
                     'connection:test', 'template:test', 1, 'Test',
                     'https://example.invalid', '{}', NULL, NULL, 30,
                     'connected', ?1, ?1
                 )",
                [now.to_rfc3339()],
            )
            .expect("provider connection");
        connection
            .execute(
                "INSERT INTO provider_models
                 (id, connection_id, api_family, model_id, display_name,
                  route_json, availability, raw_metadata_json,
                  first_seen_at, last_seen_at)
                 VALUES (
                     'route:test', 'connection:test',
                     'openai_chat_completions', 'test-model', 'Test',
                     '{}', 'available', NULL, ?1, ?1
                 )",
                [now.to_rfc3339()],
            )
            .expect("provider model");
        connection
            .execute(
                "INSERT INTO generation_presets
                 (id, model_route_id, display_name, values_json, created_at, updated_at)
                 VALUES ('preset:test', 'route:test', 'Test', '{}', ?1, ?1)",
                [now.to_rfc3339()],
            )
            .expect("generation preset");
    }

    fn create_memory_test_conversation(
        storage: &Storage,
        now: DateTime<Utc>,
    ) -> (ConversationId, ConversationBranchId, MessageId, MessageId) {
        let conversation = Conversation::new("character:test", "Memory queue");
        let (branch, _) = storage
            .save_conversation_with_mode(&conversation, lorepia_domain::ConversationMode::Chat)
            .expect("conversation");
        let mut first = Message::user(conversation.id.clone(), "first");
        first.created_at = now + Duration::seconds(1);
        storage.save_message(&first).expect("first message");
        let mut second =
            Message::user_after(conversation.id.clone(), Some(first.id.clone()), "second");
        second.created_at = now + Duration::seconds(2);
        storage.save_message(&second).expect("second message");
        storage
            .connection()
            .expect("database connection")
            .execute(
                "UPDATE conversation_branches
                 SET head_message_id = ?3, updated_at = ?4
                 WHERE conversation_id = ?1 AND id = ?2",
                params![
                    conversation.id.0,
                    branch.id.0,
                    second.id.0,
                    second.created_at.to_rfc3339(),
                ],
            )
            .expect("branch head");
        (conversation.id, branch.id, first.id, second.id)
    }

    fn save_memory_test_profiles(storage: &Storage) -> (String, String, String) {
        let task_profile = TaskProfile {
            id: TaskProfileId::from("task:memory-summary"),
            kind: AuxiliaryTaskKind::MemorySummary,
            route_id: ModelRouteId::from("route:test"),
            generation_preset_id: GenerationPresetId::from("preset:test"),
            fallback_route_ids: Vec::new(),
            embedding_dimensions: None,
            timeout_ms: 30_000,
            rate_limit: RateLimit {
                requests: 1,
                per_seconds: 60,
            },
            concurrency_limit: 1,
        };
        let task_profile_revision_id = storage
            .save_task_profile(&task_profile, None)
            .expect("task profile")
            .revision_id
            .expect("task profile revision id");
        let embedding_task_profile = TaskProfile {
            id: TaskProfileId::from("task:memory-embedding"),
            kind: AuxiliaryTaskKind::MemoryEmbedding,
            route_id: ModelRouteId::from("route:test"),
            generation_preset_id: GenerationPresetId::from("preset:test"),
            fallback_route_ids: Vec::new(),
            embedding_dimensions: Some(3),
            timeout_ms: 30_000,
            rate_limit: RateLimit {
                requests: 2,
                per_seconds: 60,
            },
            concurrency_limit: 2,
        };
        let embedding_task_profile_revision_id = storage
            .save_task_profile(&embedding_task_profile, None)
            .expect("embedding task profile")
            .revision_id
            .expect("embedding task profile revision id");
        let memory_profile = MemoryProfile {
            id: MemoryProfileId::from("memory-profile:test"),
            name: "Test memory".to_owned(),
            schema_version: 1,
            summary_task: task_profile.id,
            embedding_task: Some(embedding_task_profile.id),
            turns_per_summary: 1,
            recent_raw_budget: TokenBudget { max_tokens: 512 },
            episodic_budget: TokenBudget { max_tokens: 512 },
            semantic_budget: TokenBudget { max_tokens: 512 },
            retrieval_count: 4,
            recency_weight: 1.0,
            similarity_weight: 0.0,
            importance_weight: 0.0,
            preserve_invalidated_records: true,
            summary_schema: SummarySchemaId::from("summary-schema:test"),
            provenance: Provenance {
                source_kind: SourceKind::UserCreated,
                source_id: None,
                source_hash: None,
                author: None,
                license: None,
                imported_at: None,
            },
        };
        let memory_profile_revision_id = storage
            .save_memory_profile(&memory_profile, None)
            .expect("memory profile")
            .revision_id
            .expect("memory profile revision id");
        (
            memory_profile_revision_id,
            task_profile_revision_id,
            embedding_task_profile_revision_id,
        )
    }

    fn queue_fixture() -> QueueFixture {
        let root = tempfile::tempdir().expect("temporary storage root");
        let storage = Arc::new(Storage::open(root.path()).expect("open storage"));
        let now = Utc::now() - Duration::minutes(10);
        seed_provider_test_catalog(&storage, now);
        let (conversation_id, branch_id, source_start_message_id, source_end_message_id) =
            create_memory_test_conversation(&storage, now);
        let (
            memory_profile_revision_id,
            task_profile_revision_id,
            embedding_task_profile_revision_id,
        ) = save_memory_test_profiles(&storage);

        QueueFixture {
            _root: root,
            storage,
            conversation_id,
            branch_id,
            source_start_message_id,
            source_end_message_id,
            memory_profile_revision_id,
            task_profile_revision_id,
            embedding_task_profile_revision_id,
        }
    }

    fn enqueue_input(
        fixture: &QueueFixture,
        suffix: &str,
        available_at: DateTime<Utc>,
    ) -> MemoryJobEnqueue {
        let job = MemoryJob {
            id: MemoryJobId::from(format!("memory-job:{suffix}")),
            idempotency_key: format!("memory-summary-{suffix}-idempotency"),
            kind: MemoryJobKind::Summary,
            conversation_id: fixture.conversation_id.clone(),
            branch_id: fixture.branch_id.clone(),
            source_start_message_id: fixture.source_start_message_id.clone(),
            source_end_message_id: fixture.source_end_message_id.clone(),
            status: MemoryJobStatus::Queued,
            attempt: 0,
            created_at: available_at,
            updated_at: available_at,
            error_code: None,
        };
        let payload = VersionedJson {
            schema_version: 1,
            value: serde_json::json!({
                "source_sha256": hex::encode(Sha256::digest(suffix.as_bytes())),
                "transform_revisions": [],
                "capabilities": [],
            }),
        };
        let fingerprint = memory_job_input_fingerprint(
            &job,
            Some(&fixture.memory_profile_revision_id),
            Some(&fixture.task_profile_revision_id),
            &payload,
        )
        .expect("memory input fingerprint");
        MemoryJobEnqueue {
            job,
            memory_profile_revision_id: Some(fixture.memory_profile_revision_id.clone()),
            task_profile_revision_id: Some(fixture.task_profile_revision_id.clone()),
            input_fingerprint_sha256: fingerprint,
            payload,
            available_at,
        }
    }

    fn summary_record(
        fixture: &QueueFixture,
        input: &MemoryJobEnqueue,
        created_at: DateTime<Utc>,
    ) -> MemoryRecord {
        MemoryRecord {
            id: MemoryRecordId::from(format!("record:{}", input.job.id.as_str())),
            conversation_id: fixture.conversation_id.clone(),
            branch_id: fixture.branch_id.clone(),
            source_start_message_id: fixture.source_start_message_id.clone(),
            source_end_message_id: fixture.source_end_message_id.clone(),
            kind: MemoryKind::ConversationSummary,
            title: "Summary".to_owned(),
            summary: "A bounded generated summary.".to_owned(),
            structured_data: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({"facts": []}),
            },
            importance: 50,
            keywords: vec!["summary".to_owned()],
            embedding_ref: None,
            pinned: false,
            excluded_from_conversation: false,
            excluded_from_character: false,
            created_at,
            updated_at: created_at,
            invalidated_at: None,
            provenance: Provenance {
                source_kind: SourceKind::Generated,
                source_id: Some(input.job.id.as_str().to_owned()),
                source_hash: Some(input.input_fingerprint_sha256.clone()),
                author: None,
                license: None,
                imported_at: None,
            },
        }
    }

    fn embedding_enqueue_input(
        fixture: &QueueFixture,
        record: &StoredRevision<MemoryRecord>,
        suffix: &str,
        available_at: DateTime<Utc>,
    ) -> MemoryJobEnqueue {
        let job = MemoryJob {
            id: MemoryJobId::from(format!("memory-embedding-job:{suffix}")),
            idempotency_key: format!("memory-embedding-{suffix}-idempotency"),
            kind: MemoryJobKind::Embedding,
            conversation_id: fixture.conversation_id.clone(),
            branch_id: fixture.branch_id.clone(),
            source_start_message_id: record.value.source_start_message_id.clone(),
            source_end_message_id: record.value.source_end_message_id.clone(),
            status: MemoryJobStatus::Queued,
            attempt: 0,
            created_at: available_at,
            updated_at: available_at,
            error_code: None,
        };
        let payload = VersionedJson {
            schema_version: 1,
            value: serde_json::to_value(MemoryEmbeddingJobInput {
                memory_record_revision_id: record
                    .revision_id
                    .clone()
                    .expect("memory record immutable revision"),
                model_route_id: ModelRouteId::from("route:test"),
                dimensions: 3,
                vector_space_sha256: "a".repeat(64),
            })
            .expect("embedding input"),
        };
        let fingerprint = memory_job_input_fingerprint(
            &job,
            Some(&fixture.memory_profile_revision_id),
            Some(&fixture.embedding_task_profile_revision_id),
            &payload,
        )
        .expect("embedding fingerprint");
        MemoryJobEnqueue {
            job,
            memory_profile_revision_id: Some(fixture.memory_profile_revision_id.clone()),
            task_profile_revision_id: Some(fixture.embedding_task_profile_revision_id.clone()),
            input_fingerprint_sha256: fingerprint,
            payload,
            available_at,
        }
    }

    fn embedding_seed(
        fixture: &QueueFixture,
        summary: &MemoryJobEnqueue,
        suffix: &str,
        available_at: DateTime<Utc>,
    ) -> MemoryEmbeddingJobSeed {
        MemoryEmbeddingJobSeed {
            job: MemoryJob {
                id: MemoryJobId::from(format!("memory-embedding-job:{suffix}")),
                idempotency_key: format!("memory-embedding-{suffix}-idempotency"),
                kind: MemoryJobKind::Embedding,
                conversation_id: fixture.conversation_id.clone(),
                branch_id: fixture.branch_id.clone(),
                source_start_message_id: summary.job.source_start_message_id.clone(),
                source_end_message_id: summary.job.source_end_message_id.clone(),
                status: MemoryJobStatus::Queued,
                attempt: 0,
                created_at: available_at,
                updated_at: available_at,
                error_code: None,
            },
            memory_profile_revision_id: fixture.memory_profile_revision_id.clone(),
            task_profile_revision_id: fixture.embedding_task_profile_revision_id.clone(),
            model_route_id: ModelRouteId::from("route:test"),
            dimensions: 3,
            vector_space_sha256: "a".repeat(64),
            available_at,
        }
    }

    fn assert_memory_job_enqueue_idempotency(
        fixture: &QueueFixture,
        now: DateTime<Utc>,
    ) -> MemoryJobEnqueue {
        let input = enqueue_input(fixture, "first", now);
        let first = fixture
            .storage
            .enqueue_memory_job_idempotent(&input)
            .expect("first enqueue");
        assert!(!first.exact_replay);
        let projected = fixture
            .storage
            .get_memory_job(&input.job.id)
            .expect("modern queue row through public memory-job projection");
        assert_eq!(projected.value, first.entry.job);
        assert_eq!(projected.revision, first.entry.revision);
        let replay = fixture
            .storage
            .enqueue_memory_job_idempotent(&input)
            .expect("exact replay");
        assert!(replay.exact_replay);
        assert_eq!(replay.entry.job.id, first.entry.job.id);

        let mut mismatch = input.clone();
        mismatch.payload.value = serde_json::json!({"source_sha256": "b".repeat(64)});
        mismatch.input_fingerprint_sha256 = memory_job_input_fingerprint(
            &mismatch.job,
            mismatch.memory_profile_revision_id.as_deref(),
            mismatch.task_profile_revision_id.as_deref(),
            &mismatch.payload,
        )
        .expect("mismatched fingerprint");
        assert!(
            fixture
                .storage
                .enqueue_memory_job_idempotent(&mismatch)
                .is_err()
        );
        input
    }

    #[test]
    fn enqueue_is_exactly_idempotent_and_claim_enforces_durable_limits() {
        let fixture = queue_fixture();
        let now = Utc::now() + Duration::seconds(10);
        let first_input = assert_memory_job_enqueue_idempotency(&fixture, now);
        let second_input = enqueue_input(&fixture, "second", now);
        fixture
            .storage
            .enqueue_memory_job_idempotent(&second_input)
            .expect("second enqueue");
        let first_claim = fixture
            .storage
            .claim_next_memory_job(now)
            .expect("claim")
            .expect("first eligible job");
        assert_eq!(first_claim.job.id, first_input.job.id);
        assert_eq!(first_claim.revision, 2);
        assert_eq!(first_claim.job.attempt, 1);
        assert!(first_claim.memory_profile_revision.is_some());
        assert!(first_claim.task_profile_revision.is_some());
        assert!(
            fixture
                .storage
                .claim_next_memory_job(now + Duration::seconds(1))
                .expect("bounded claim")
                .is_none()
        );

        let interrupted = fixture
            .storage
            .interrupt_memory_job(
                &first_claim.job.id,
                first_claim.revision,
                Some("provider_outcome_unknown"),
                now + Duration::seconds(2),
            )
            .expect("interrupt");
        assert_eq!(interrupted.job.status, MemoryJobStatus::Interrupted);
        assert!(
            fixture
                .storage
                .claim_next_memory_job(now + Duration::seconds(3))
                .expect("rate-limited claim")
                .is_none()
        );
        let second_claim = fixture
            .storage
            .claim_next_memory_job(now + Duration::seconds(61))
            .expect("claim after window")
            .expect("second eligible job");
        let recovered = fixture
            .storage
            .recover_running_memory_jobs(now + Duration::seconds(62))
            .expect("recover");
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].job.id, second_claim.job.id);
        assert_eq!(recovered[0].job.status, MemoryJobStatus::Interrupted);
        assert!(
            fixture
                .storage
                .claim_next_memory_job(now + Duration::seconds(122))
                .expect("no automatic retry")
                .is_none()
        );
        let retried = fixture
            .storage
            .retry_interrupted_memory_job(
                &fixture.conversation_id,
                &fixture.branch_id,
                &second_claim.job.id,
                recovered[0].revision,
                now + Duration::seconds(122),
                now + Duration::seconds(122),
            )
            .expect("explicit retry");
        assert_eq!(retried.job.status, MemoryJobStatus::Queued);
        let retried_claim = fixture
            .storage
            .claim_next_memory_job(now + Duration::seconds(123))
            .expect("retry claim")
            .expect("retried eligible job");
        assert_eq!(retried_claim.job.attempt, 2);
    }

    #[test]
    fn explicit_memory_job_retry_denies_cross_room_owner() {
        let fixture = queue_fixture();
        let now = Utc::now() + Duration::seconds(10);
        let input = enqueue_input(&fixture, "cross-room-retry", now);
        fixture
            .storage
            .enqueue_memory_job_idempotent(&input)
            .expect("enqueue retry fixture");
        let claimed = fixture
            .storage
            .claim_next_memory_job(now)
            .expect("claim retry fixture")
            .expect("eligible retry fixture");
        let interrupted = fixture
            .storage
            .interrupt_memory_job(
                &claimed.job.id,
                claimed.revision,
                Some("provider_unknown_outcome"),
                now + Duration::seconds(1),
            )
            .expect("interrupt retry fixture");
        let foreign_conversation = ConversationId("conversation:foreign".to_owned());
        let foreign_branch = ConversationBranchId("branch:foreign".to_owned());
        for (conversation_id, branch_id, mismatch) in [
            (&foreign_conversation, &fixture.branch_id, "conversation"),
            (&fixture.conversation_id, &foreign_branch, "branch"),
        ] {
            let error = fixture
                .storage
                .retry_interrupted_memory_job(
                    conversation_id,
                    branch_id,
                    &claimed.job.id,
                    interrupted.revision,
                    now + Duration::seconds(2),
                    now + Duration::seconds(2),
                )
                .unwrap_err();
            assert_eq!(error.code, CoreErrorCode::NotFound, "{mismatch} mismatch");
        }
        let unchanged = fixture
            .storage
            .get_memory_job_queue_entry(&claimed.job.id)
            .expect("load denied retry");
        assert_eq!(unchanged.revision, interrupted.revision);
        assert_eq!(unchanged.job.status, MemoryJobStatus::Interrupted);

        let retried = fixture
            .storage
            .retry_interrupted_memory_job(
                &fixture.conversation_id,
                &fixture.branch_id,
                &claimed.job.id,
                interrupted.revision,
                now + Duration::seconds(2),
                now + Duration::seconds(2),
            )
            .expect("owner-bound retry");
        let replay = fixture
            .storage
            .retry_interrupted_memory_job(
                &fixture.conversation_id,
                &fixture.branch_id,
                &claimed.job.id,
                interrupted.revision,
                now + Duration::seconds(2),
                now + Duration::seconds(2),
            )
            .expect_err("stale retry replay must fail");
        assert_eq!(replay.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            fixture
                .storage
                .get_memory_job_queue_entry(&claimed.job.id)
                .expect("load single retry"),
            retried
        );
    }

    #[test]
    fn visible_summary_jobs_are_exactly_profile_scoped_and_include_interrupted_work() {
        let fixture = queue_fixture();
        let now = Utc::now() + Duration::seconds(10);
        let input = enqueue_input(&fixture, "visible-cadence", now);
        fixture
            .storage
            .enqueue_memory_job_idempotent(&input)
            .expect("summary enqueue");
        let queued = fixture
            .storage
            .list_visible_memory_summary_jobs(
                &fixture.conversation_id,
                &fixture.branch_id,
                &fixture.memory_profile_revision_id,
                &fixture.task_profile_revision_id,
            )
            .expect("visible queued summary");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].job.status, MemoryJobStatus::Queued);
        assert!(
            fixture
                .storage
                .list_visible_memory_summary_jobs(
                    &fixture.conversation_id,
                    &fixture.branch_id,
                    &fixture.memory_profile_revision_id,
                    &fixture.embedding_task_profile_revision_id,
                )
                .expect("wrong exact task revision")
                .is_empty()
        );

        let claimed = fixture
            .storage
            .claim_next_memory_job(now)
            .expect("summary claim")
            .expect("summary eligible");
        fixture
            .storage
            .interrupt_memory_job(
                &claimed.job.id,
                claimed.revision,
                Some("provider_unknown_outcome"),
                now + Duration::seconds(1),
            )
            .expect("summary interruption");
        let interrupted = fixture
            .storage
            .list_visible_memory_summary_jobs(
                &fixture.conversation_id,
                &fixture.branch_id,
                &fixture.memory_profile_revision_id,
                &fixture.task_profile_revision_id,
            )
            .expect("visible interrupted summary");
        assert_eq!(interrupted.len(), 1);
        assert_eq!(interrupted[0].job.status, MemoryJobStatus::Interrupted);

        let requeued = fixture
            .storage
            .retry_interrupted_memory_job(
                &fixture.conversation_id,
                &fixture.branch_id,
                &claimed.job.id,
                interrupted[0].revision,
                now + Duration::seconds(61),
                now + Duration::seconds(61),
            )
            .expect("explicit summary retry");
        let reclaimed = fixture
            .storage
            .claim_next_memory_job(now + Duration::seconds(62))
            .expect("retried summary claim")
            .expect("retried summary eligible");
        assert_eq!(reclaimed.job.id, requeued.job.id);
        fixture
            .storage
            .finish_memory_job(
                &reclaimed.job.id,
                reclaimed.revision,
                MemoryJobFinish::Cancelled,
                now + Duration::seconds(63),
            )
            .expect("cancel summary");
        let cancelled = fixture
            .storage
            .list_visible_memory_summary_jobs(
                &fixture.conversation_id,
                &fixture.branch_id,
                &fixture.memory_profile_revision_id,
                &fixture.task_profile_revision_id,
            )
            .expect("visible cancelled summary");
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].job.status, MemoryJobStatus::Cancelled);
    }

    #[test]
    fn summary_record_and_terminal_job_commit_once_with_exact_replay() {
        let fixture = queue_fixture();
        let now = Utc::now() + Duration::seconds(10);
        let input = enqueue_input(&fixture, "atomic", now);
        fixture
            .storage
            .enqueue_memory_job_idempotent(&input)
            .expect("enqueue");
        let claimed = fixture
            .storage
            .claim_next_memory_job(now)
            .expect("claim")
            .expect("eligible job");
        let finished_at = now + Duration::seconds(1);
        let record = summary_record(&fixture, &input, finished_at);
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let storage = Arc::clone(&fixture.storage);
            let id = claimed.job.id.clone();
            let record = record.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                storage.complete_memory_summary_job(&id, claimed.revision, &record, finished_at)
            }));
        }
        barrier.wait();
        let completions = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("completion thread")
                    .expect("completion")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            completions
                .iter()
                .filter(|completion| completion.exact_replay)
                .count(),
            1
        );
        assert_eq!(
            completions
                .iter()
                .filter(|completion| !completion.exact_replay)
                .count(),
            1
        );
        assert!(completions.iter().all(|completion| {
            completion.job.job.status == MemoryJobStatus::Succeeded
                && completion.job.result_record_id.as_ref() == Some(&record.id)
                && completion.record.value == record
        }));
        let connection = fixture.storage.connection().expect("database connection");
        let counts = connection
            .query_row(
                "SELECT
                     (SELECT COUNT(*) FROM memory_records WHERE id = ?1),
                     (SELECT COUNT(*) FROM memory_record_revisions
                      WHERE record_id = ?1),
                     (SELECT COUNT(*) FROM memory_record_events
                      WHERE record_id = ?1)",
                [record.id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .expect("completion counts");
        assert_eq!(counts, (1, 1, 1));
        drop(connection);

        let mut mismatched = record.clone();
        mismatched.summary = "different summary".to_owned();
        assert!(
            fixture
                .storage
                .complete_memory_summary_job(
                    &claimed.job.id,
                    claimed.revision,
                    &mismatched,
                    finished_at,
                )
                .is_err()
        );
    }

    fn assert_invalid_embedding_seed_rolls_back_summary(
        fixture: &QueueFixture,
        claimed: &StoredMemoryJobQueueEntry,
        record: &MemoryRecord,
        seed: &MemoryEmbeddingJobSeed,
        finished_at: DateTime<Utc>,
    ) {
        let mut invalid_seed = seed.clone();
        invalid_seed.dimensions = 4;
        assert!(
            fixture
                .storage
                .complete_memory_summary_job_with_embedding(
                    &claimed.job.id,
                    claimed.revision,
                    record,
                    Some(&invalid_seed),
                    finished_at,
                )
                .is_err()
        );
        assert_eq!(
            fixture
                .storage
                .get_memory_job_queue_entry(&claimed.job.id)
                .expect("summary after rollback")
                .job
                .status,
            MemoryJobStatus::Running
        );
        assert!(
            fixture
                .storage
                .get_memory_record(&fixture.conversation_id, &fixture.branch_id, &record.id,)
                .is_err()
        );
        assert!(
            fixture
                .storage
                .get_memory_job_queue_entry(&seed.job.id)
                .is_err()
        );
    }

    #[test]
    fn summary_completion_atomically_enqueues_exact_embedding_revision() {
        let fixture = queue_fixture();
        let now = Utc::now() + Duration::seconds(10);
        let input = enqueue_input(&fixture, "summary-with-embedding", now);
        fixture
            .storage
            .enqueue_memory_job_idempotent(&input)
            .expect("summary enqueue");
        let claimed = fixture
            .storage
            .claim_next_memory_job(now)
            .expect("summary claim")
            .expect("summary eligible");
        let finished_at = now + Duration::seconds(1);
        let record = summary_record(&fixture, &input, finished_at);
        let seed = embedding_seed(&fixture, &input, "summary-with-embedding", finished_at);
        assert_invalid_embedding_seed_rolls_back_summary(
            &fixture,
            &claimed,
            &record,
            &seed,
            finished_at,
        );

        let completed = fixture
            .storage
            .complete_memory_summary_job_with_embedding(
                &claimed.job.id,
                claimed.revision,
                &record,
                Some(&seed),
                finished_at,
            )
            .expect("atomic summary and embedding enqueue");
        let embedding_job = completed
            .embedding_job
            .as_ref()
            .expect("atomic embedding job");
        assert_eq!(embedding_job.job, seed.job);
        assert_eq!(
            embedding_job.memory_profile_revision_id.as_deref(),
            Some(fixture.memory_profile_revision_id.as_str())
        );
        assert_eq!(
            embedding_job.task_profile_revision_id.as_deref(),
            Some(fixture.embedding_task_profile_revision_id.as_str())
        );
        let embedding_input =
            decode_memory_embedding_job_input(&embedding_job.payload).expect("embedding payload");
        assert_eq!(
            embedding_input.memory_record_revision_id,
            completed
                .record
                .revision_id
                .clone()
                .expect("summary record revision")
        );
        assert_eq!(embedding_input.model_route_id, seed.model_route_id);
        assert_eq!(embedding_input.dimensions, seed.dimensions);
        assert_eq!(
            embedding_input.vector_space_sha256,
            seed.vector_space_sha256
        );

        let replay = fixture
            .storage
            .complete_memory_summary_job_with_embedding(
                &claimed.job.id,
                claimed.revision,
                &record,
                Some(&seed),
                finished_at,
            )
            .expect("exact completion replay");
        assert!(replay.exact_replay);
        assert_eq!(
            replay.embedding_job.expect("replayed embedding").job.id,
            seed.job.id
        );

        fixture
            .storage
            .delete_memory_record_tombstone(
                &fixture.conversation_id,
                &fixture.branch_id,
                &record.id,
                completed.record.revision,
                finished_at,
            )
            .expect("user summary tombstone");
        let covered_after_tombstone = fixture
            .storage
            .list_visible_memory_summary_jobs(
                &fixture.conversation_id,
                &fixture.branch_id,
                &fixture.memory_profile_revision_id,
                &fixture.task_profile_revision_id,
            )
            .expect("successful summary remains cadence coverage");
        assert_eq!(covered_after_tombstone.len(), 1);
        assert_eq!(
            covered_after_tombstone[0].job.status,
            MemoryJobStatus::Succeeded
        );
    }

    struct EmbeddingCompletionCase {
        claimed: StoredMemoryJobQueueEntry,
        embedding: MemoryEmbeddingRecord,
        finished_at: DateTime<Utc>,
    }

    fn prepare_embedding_completion_case(
        fixture: &QueueFixture,
        now: DateTime<Utc>,
    ) -> EmbeddingCompletionCase {
        let summary_input = enqueue_input(fixture, "embedding-source", now);
        fixture
            .storage
            .enqueue_memory_job_idempotent(&summary_input)
            .expect("summary enqueue");
        let summary_claim = fixture
            .storage
            .claim_next_memory_job(now)
            .expect("summary claim")
            .expect("summary eligible");
        let summary = summary_record(fixture, &summary_input, now + Duration::seconds(1));
        let summary_completion = fixture
            .storage
            .complete_memory_summary_job(
                &summary_claim.job.id,
                summary_claim.revision,
                &summary,
                now + Duration::seconds(1),
            )
            .expect("summary completion");
        let embedding_input = embedding_enqueue_input(
            fixture,
            &summary_completion.record,
            "atomic",
            now + Duration::seconds(2),
        );
        fixture
            .storage
            .enqueue_memory_job_idempotent(&embedding_input)
            .expect("embedding enqueue");
        let claimed = fixture
            .storage
            .claim_next_memory_job(now + Duration::seconds(2))
            .expect("embedding claim")
            .expect("embedding eligible");
        assert_eq!(claimed.job.kind, MemoryJobKind::Embedding);
        assert!(
            fixture
                .storage
                .finish_memory_job(
                    &claimed.job.id,
                    claimed.revision,
                    MemoryJobFinish::Succeeded {
                        result_record_id: Some(summary.id.clone()),
                    },
                    now + Duration::seconds(3),
                )
                .is_err(),
            "embedding success must use the atomic completion"
        );
        EmbeddingCompletionCase {
            claimed,
            embedding: MemoryEmbeddingRecord {
                id: "embedding:atomic".to_owned(),
                memory_record_id: summary.id,
                model_route_id: Some(ModelRouteId::from("route:test")),
                dimensions: 3,
                values: vec![1.0, 0.0, 0.0],
                created_at: now + Duration::seconds(3),
            },
            finished_at: now + Duration::seconds(3),
        }
    }

    fn complete_embedding_concurrently(
        fixture: &QueueFixture,
        case: &EmbeddingCompletionCase,
    ) -> Vec<MemoryEmbeddingJobCompletion> {
        let barrier = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let storage = Arc::clone(&fixture.storage);
            let id = case.claimed.job.id.clone();
            let expected_revision = case.claimed.revision;
            let embedding = case.embedding.clone();
            let finished_at = case.finished_at;
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                storage.complete_memory_embedding_job(
                    &id,
                    expected_revision,
                    &embedding,
                    finished_at,
                )
            }));
        }
        barrier.wait();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("embedding completion thread")
                    .expect("embedding completion")
            })
            .collect()
    }

    fn assert_embedding_cosine_scoping(fixture: &QueueFixture, embedding_id: &str) {
        let query = MemoryEmbeddingQuery {
            conversation_id: fixture.conversation_id.clone(),
            branch_id: fixture.branch_id.clone(),
            context_head_message_id: fixture.source_end_message_id.clone(),
            task_profile_revision_id: fixture.embedding_task_profile_revision_id.clone(),
            model_route_id: ModelRouteId::from("route:test"),
            dimensions: 3,
            vector_space_sha256: "a".repeat(64),
            values: vec![1.0, 0.0, 0.0],
            candidate_limit: 16,
            result_limit: 4,
        };
        let matches = fixture
            .storage
            .query_memory_embeddings_cosine(&query)
            .expect("cosine query");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].embedding_id, embedding_id);
        assert_eq!(matches[0].similarity_millionths, 1_000_000);

        let mismatched_space = fixture
            .storage
            .query_memory_embeddings_cosine(&MemoryEmbeddingQuery {
                vector_space_sha256: "c".repeat(64),
                ..query
            })
            .expect("different exact vector space remains isolated");
        assert!(mismatched_space.is_empty());
    }

    #[test]
    fn embedding_and_terminal_job_commit_once_and_cosine_is_exactly_scoped() {
        let fixture = queue_fixture();
        let now = Utc::now() + Duration::seconds(10);
        let case = prepare_embedding_completion_case(&fixture, now);
        let completions = complete_embedding_concurrently(&fixture, &case);
        assert_eq!(
            completions
                .iter()
                .filter(|completion| completion.exact_replay)
                .count(),
            1
        );
        assert_eq!(
            completions
                .iter()
                .filter(|completion| !completion.exact_replay)
                .count(),
            1
        );
        assert!(completions.iter().all(|completion| {
            completion.job.job.status == MemoryJobStatus::Succeeded
                && completion.embedding.value == case.embedding
                && completion.embedding.task_profile_revision_id
                    == fixture.embedding_task_profile_revision_id
                && completion.embedding.vector_sha256.len() == 64
        }));
        assert_embedding_cosine_scoping(&fixture, &case.embedding.id);

        let mut mismatched = case.embedding.clone();
        mismatched.values = vec![0.0, 1.0, 0.0];
        assert!(
            fixture
                .storage
                .complete_memory_embedding_job(
                    &case.claimed.job.id,
                    case.claimed.revision,
                    &mismatched,
                    case.finished_at,
                )
                .is_err()
        );
        let counts = fixture
            .storage
            .connection()
            .expect("database connection")
            .query_row(
                "SELECT
                     (SELECT COUNT(*) FROM memory_embeddings WHERE id = ?1),
                     (SELECT COUNT(*) FROM memory_jobs
                      WHERE id = ?2 AND state = 'succeeded')",
                params![case.embedding.id, case.claimed.job.id.as_str()],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .expect("embedding completion counts");
        assert_eq!(counts, (1, 1));
    }

    fn memory_query_embedding_intent(
        fixture: &QueueFixture,
        now: DateTime<Utc>,
    ) -> crate::MemoryQueryEmbeddingIntent {
        crate::MemoryQueryEmbeddingIntent {
            id: "memory-query-embedding:test".to_owned(),
            idempotency_key: "memory-query-embedding:v1:test".to_owned(),
            memory_profile_id: MemoryProfileId::from("memory-profile:test"),
            memory_profile_revision_id: fixture.memory_profile_revision_id.clone(),
            task_profile_revision_id: fixture.embedding_task_profile_revision_id.clone(),
            conversation_id: fixture.conversation_id.clone(),
            branch_id: fixture.branch_id.clone(),
            source_start_message_id: fixture.source_start_message_id.clone(),
            source_end_message_id: fixture.source_end_message_id.clone(),
            query_sha256: "b".repeat(64),
            vector_space_sha256: "a".repeat(64),
            model_route_id: ModelRouteId::from("route:test"),
            dimensions: 3,
            created_at: now,
        }
    }

    #[test]
    fn query_embedding_intent_reuses_completion_and_never_retries_interrupted_implicitly() {
        use crate::MemoryQueryEmbeddingStatus;

        let fixture = queue_fixture();
        let now = Utc::now() + Duration::seconds(10);
        let intent = memory_query_embedding_intent(&fixture, now);
        let queued = fixture
            .storage
            .enqueue_memory_query_embedding(&intent)
            .expect("query intent enqueue");
        assert!(!queued.exact_replay);
        let running = fixture
            .storage
            .claim_memory_query_embedding(&intent.id, queued.entry.revision, now)
            .expect("query intent claim");
        let credential_canary = "credential-canary-must-not-persist";
        let rejected = fixture
            .storage
            .interrupt_memory_query_embedding(
                &intent.id,
                running.revision,
                credential_canary,
                now + Duration::seconds(1),
            )
            .expect_err("free-form provider or credential text is not a durable error code");
        assert!(!format!("{rejected:?}").contains(credential_canary));
        let interrupted = fixture
            .storage
            .interrupt_memory_query_embedding(
                &intent.id,
                running.revision,
                "provider_unknown_outcome",
                now + Duration::seconds(1),
            )
            .expect("query unknown outcome");
        assert_eq!(interrupted.status, MemoryQueryEmbeddingStatus::Interrupted);

        let replay = fixture
            .storage
            .enqueue_memory_query_embedding(&intent)
            .expect("interrupted exact replay");
        assert!(replay.exact_replay);
        assert_eq!(replay.entry, interrupted);
        assert!(
            fixture
                .storage
                .claim_memory_query_embedding(
                    &intent.id,
                    interrupted.revision,
                    now + Duration::seconds(2),
                )
                .is_err(),
            "ordinary replay cannot redispatch an interrupted intent"
        );

        let requeued = fixture
            .storage
            .retry_interrupted_memory_query_embedding(
                &fixture.conversation_id,
                &fixture.branch_id,
                &intent.id,
                interrupted.revision,
                now + Duration::seconds(2),
            )
            .expect("explicit retry");
        let running = fixture
            .storage
            .claim_memory_query_embedding(&intent.id, requeued.revision, now + Duration::seconds(2))
            .expect("explicitly retried claim");
        let completed = fixture
            .storage
            .complete_memory_query_embedding(
                &intent.id,
                running.revision,
                &[1.0, 0.0, 0.0],
                now + Duration::seconds(3),
            )
            .expect("query vector completion");
        assert_eq!(completed.status, MemoryQueryEmbeddingStatus::Succeeded);
        assert_eq!(completed.values, Some(vec![1.0, 0.0, 0.0]));
        assert_eq!(completed.vector_sha256.as_deref().map(str::len), Some(64));
        assert!(
            !serde_json::to_string(&completed)
                .expect("serialize durable query evidence")
                .contains(credential_canary)
        );

        let replay = fixture
            .storage
            .enqueue_memory_query_embedding(&intent)
            .expect("completed exact replay");
        assert!(replay.exact_replay);
        assert_eq!(replay.entry, completed);
    }

    #[test]
    fn query_embedding_retry_denies_cross_room_owner() {
        use crate::MemoryQueryEmbeddingStatus;

        let fixture = queue_fixture();
        let now = Utc::now() + Duration::seconds(10);
        let intent = memory_query_embedding_intent(&fixture, now);
        let queued = fixture
            .storage
            .enqueue_memory_query_embedding(&intent)
            .expect("enqueue query retry fixture");
        let running = fixture
            .storage
            .claim_memory_query_embedding(&intent.id, queued.entry.revision, now)
            .expect("claim query retry fixture");
        let interrupted = fixture
            .storage
            .interrupt_memory_query_embedding(
                &intent.id,
                running.revision,
                "provider_unknown_outcome",
                now + Duration::seconds(1),
            )
            .expect("interrupt query retry fixture");
        let foreign_conversation = ConversationId("conversation:foreign".to_owned());
        let foreign_branch = ConversationBranchId("branch:foreign".to_owned());
        for (conversation_id, branch_id, mismatch) in [
            (&foreign_conversation, &fixture.branch_id, "conversation"),
            (&fixture.conversation_id, &foreign_branch, "branch"),
        ] {
            let error = fixture
                .storage
                .retry_memory_query_embedding(
                    conversation_id,
                    branch_id,
                    &intent.id,
                    interrupted.revision,
                    now + Duration::seconds(2),
                )
                .unwrap_err();
            assert_eq!(error.code, CoreErrorCode::NotFound, "{mismatch} mismatch");
        }
        let unchanged = fixture
            .storage
            .get_memory_query_embedding(&intent.id)
            .expect("load denied query retry");
        assert_eq!(unchanged.revision, interrupted.revision);
        assert_eq!(unchanged.status, MemoryQueryEmbeddingStatus::Interrupted);

        let retried = fixture
            .storage
            .retry_memory_query_embedding(
                &fixture.conversation_id,
                &fixture.branch_id,
                &intent.id,
                interrupted.revision,
                now + Duration::seconds(2),
            )
            .expect("owner-bound query retry");
        let replay = fixture
            .storage
            .retry_memory_query_embedding(
                &fixture.conversation_id,
                &fixture.branch_id,
                &intent.id,
                interrupted.revision,
                now + Duration::seconds(2),
            )
            .expect_err("stale query retry replay must fail");
        assert_eq!(replay.code, CoreErrorCode::StorageUnavailable);
        assert_eq!(
            fixture
                .storage
                .get_memory_query_embedding(&intent.id)
                .expect("load single query retry"),
            retried
        );
    }

    struct RewindMemoryCase {
        record: MemoryRecord,
        completed_input: MemoryJobEnqueue,
        running_input: MemoryJobEnqueue,
        queued_input: MemoryJobEnqueue,
    }

    fn prepare_rewind_memory_case(fixture: &QueueFixture, now: DateTime<Utc>) -> RewindMemoryCase {
        let completed_input = enqueue_input(fixture, "rewind-completed", now);
        fixture
            .storage
            .enqueue_memory_job_idempotent(&completed_input)
            .expect("completed enqueue");
        let completed_claim = fixture
            .storage
            .claim_next_memory_job(now)
            .expect("completed claim")
            .expect("completed eligible");
        let record = summary_record(fixture, &completed_input, now + Duration::seconds(1));
        fixture
            .storage
            .complete_memory_summary_job(
                &completed_claim.job.id,
                completed_claim.revision,
                &record,
                now + Duration::seconds(1),
            )
            .expect("completed summary");
        let running_input = enqueue_input(fixture, "rewind-running", now + Duration::seconds(61));
        fixture
            .storage
            .enqueue_memory_job_idempotent(&running_input)
            .expect("running enqueue");
        let running = fixture
            .storage
            .claim_next_memory_job(now + Duration::seconds(61))
            .expect("running claim")
            .expect("running eligible");
        assert_eq!(running.job.id, running_input.job.id);
        let queued_input = enqueue_input(fixture, "rewind-queued", now + Duration::seconds(62));
        fixture
            .storage
            .enqueue_memory_job_idempotent(&queued_input)
            .expect("queued enqueue");
        RewindMemoryCase {
            record,
            completed_input,
            running_input,
            queued_input,
        }
    }

    #[test]
    fn branch_rewind_atomically_invalidates_overlapping_memory_and_live_jobs() {
        let fixture = queue_fixture();
        let now = Utc::now() - Duration::seconds(120);
        let case = prepare_rewind_memory_case(&fixture, now);

        assert!(
            fixture
                .storage
                .remove_message_from_branch(
                    &fixture.conversation_id,
                    &fixture.branch_id,
                    Some(&fixture.source_start_message_id),
                    &fixture.source_end_message_id,
                )
                .is_err(),
            "stale-head rewind must fail before committing invalidation"
        );
        assert!(
            fixture
                .storage
                .get_memory_record(
                    &fixture.conversation_id,
                    &fixture.branch_id,
                    &case.record.id,
                )
                .expect("record after failed rewind")
                .value
                .invalidated_at
                .is_none()
        );
        assert_eq!(
            fixture
                .storage
                .get_memory_job_queue_entry(&case.running_input.job.id)
                .expect("running job after failed rewind")
                .job
                .status,
            MemoryJobStatus::Running
        );
        assert_eq!(
            fixture
                .storage
                .get_memory_job_queue_entry(&case.queued_input.job.id)
                .expect("queued job after failed rewind")
                .job
                .status,
            MemoryJobStatus::Queued
        );

        let rewound = fixture
            .storage
            .remove_message_from_branch(
                &fixture.conversation_id,
                &fixture.branch_id,
                Some(&fixture.source_end_message_id),
                &fixture.source_end_message_id,
            )
            .expect("atomic rewind");
        assert_eq!(
            rewound.head_message_id,
            Some(fixture.source_start_message_id.clone())
        );
        let invalidated_record = fixture
            .storage
            .get_memory_record(
                &fixture.conversation_id,
                &fixture.branch_id,
                &case.record.id,
            )
            .expect("invalidated record");
        assert!(invalidated_record.value.invalidated_at.is_some());
        let cancelled_running = fixture
            .storage
            .get_memory_job_queue_entry(&case.running_input.job.id)
            .expect("cancelled running job");
        assert_eq!(cancelled_running.job.status, MemoryJobStatus::Cancelled);
        assert!(cancelled_running.finished_at.is_some());
        let cancelled_queued = fixture
            .storage
            .get_memory_job_queue_entry(&case.queued_input.job.id)
            .expect("cancelled queued job");
        assert_eq!(cancelled_queued.job.status, MemoryJobStatus::Cancelled);
        assert!(cancelled_queued.finished_at.is_some());
        let completed_job = fixture
            .storage
            .get_memory_job_queue_entry(&case.completed_input.job.id)
            .expect("preserved completed job");
        assert_eq!(completed_job.job.status, MemoryJobStatus::Succeeded);
    }

    fn prepare_user_memory_record(
        fixture: &QueueFixture,
        now: DateTime<Utc>,
    ) -> (MemoryRecord, StoredRevision<MemoryRecord>) {
        let input = enqueue_input(fixture, "user-controls", now);
        fixture
            .storage
            .enqueue_memory_job_idempotent(&input)
            .expect("enqueue");
        let claimed = fixture
            .storage
            .claim_next_memory_job(now)
            .expect("claim")
            .expect("eligible");
        let original = summary_record(fixture, &input, now + Duration::seconds(1));
        let completed = fixture
            .storage
            .complete_memory_summary_job(
                &claimed.job.id,
                claimed.revision,
                &original,
                now + Duration::seconds(1),
            )
            .expect("summary completion")
            .record;
        (original, completed)
    }

    fn patch_and_assert_user_memory_record(
        fixture: &QueueFixture,
        original: &MemoryRecord,
        completed_revision: u64,
        now: DateTime<Utc>,
    ) -> StoredRevision<MemoryRecord> {
        let patched = fixture
            .storage
            .patch_memory_record_user_fields(
                &fixture.conversation_id,
                &fixture.branch_id,
                &original.id,
                completed_revision,
                &MemoryRecordUserPatch {
                    title: Some("User title".to_owned()),
                    summary: Some("User-edited bounded summary.".to_owned()),
                    importance: Some(75),
                    keywords: Some(vec!["edited".to_owned(), "memory".to_owned()]),
                    pinned: Some(true),
                    ..MemoryRecordUserPatch::default()
                },
                now + Duration::seconds(2),
            )
            .expect("user patch");
        assert_eq!(patched.revision, 2);
        assert_eq!(patched.value.title, "User title");
        assert_eq!(patched.value.importance, 75);
        assert!(patched.value.pinned);
        assert_eq!(patched.value.id, original.id);
        assert_eq!(patched.value.conversation_id, original.conversation_id);
        assert_eq!(patched.value.branch_id, original.branch_id);
        assert_eq!(
            patched.value.source_start_message_id,
            original.source_start_message_id
        );
        assert_eq!(
            patched.value.source_end_message_id,
            original.source_end_message_id
        );
        assert_eq!(patched.value.kind, original.kind);
        assert_eq!(patched.value.structured_data, original.structured_data);
        assert_eq!(patched.value.provenance, original.provenance);
        patched
    }

    fn assert_invalid_user_memory_patches(
        fixture: &QueueFixture,
        original: &MemoryRecord,
        stale_revision: u64,
        current_revision: u64,
        now: DateTime<Utc>,
    ) {
        assert!(
            fixture
                .storage
                .patch_memory_record_user_fields(
                    &fixture.conversation_id,
                    &fixture.branch_id,
                    &original.id,
                    stale_revision,
                    &MemoryRecordUserPatch {
                        importance: Some(10),
                        ..MemoryRecordUserPatch::default()
                    },
                    now + Duration::seconds(3),
                )
                .is_err(),
            "stale expected revision must not append content"
        );
        assert!(
            fixture
                .storage
                .patch_memory_record_user_fields(
                    &fixture.conversation_id,
                    &fixture.branch_id,
                    &original.id,
                    current_revision,
                    &MemoryRecordUserPatch::default(),
                    now + Duration::seconds(3),
                )
                .is_err(),
            "empty patch must not create an audit-only fake revision"
        );
    }

    fn invalidate_and_exclude_user_memory_record(
        fixture: &QueueFixture,
        original: &MemoryRecord,
        now: DateTime<Utc>,
    ) -> StoredRevision<MemoryRecord> {
        let invalidated = fixture
            .storage
            .invalidate_memory_range(
                &fixture.conversation_id,
                &fixture.branch_id,
                &fixture.source_start_message_id,
                &fixture.source_end_message_id,
                now + Duration::seconds(3),
            )
            .expect("invalidate source range");
        assert_eq!(invalidated.invalidated_records, 1);
        let invalidated_revision = fixture
            .storage
            .get_memory_record(&fixture.conversation_id, &fixture.branch_id, &original.id)
            .expect("invalidated record");
        assert_eq!(invalidated_revision.revision, 3);
        let excluded_conversation = fixture
            .storage
            .set_memory_record_exclusion(
                &fixture.conversation_id,
                &fixture.branch_id,
                &original.id,
                invalidated_revision.revision,
                (MemoryRecordExclusionScope::Conversation, true),
                now + Duration::seconds(4),
            )
            .expect("conversation exclusion");
        assert!(excluded_conversation.value.excluded_from_conversation);
        assert!(excluded_conversation.value.invalidated_at.is_some());
        let excluded_character = fixture
            .storage
            .set_memory_record_exclusion(
                &fixture.conversation_id,
                &fixture.branch_id,
                &original.id,
                excluded_conversation.revision,
                (MemoryRecordExclusionScope::Character, true),
                now + Duration::seconds(5),
            )
            .expect("character exclusion");
        assert!(excluded_character.value.excluded_from_character);
        let restored = fixture
            .storage
            .set_memory_record_exclusion(
                &fixture.conversation_id,
                &fixture.branch_id,
                &original.id,
                excluded_character.revision,
                (MemoryRecordExclusionScope::Conversation, false),
                now + Duration::seconds(6),
            )
            .expect("conversation inclusion");
        assert!(!restored.value.excluded_from_conversation);
        assert!(restored.value.excluded_from_character);
        assert!(restored.value.invalidated_at.is_some());
        restored
    }

    fn tombstone_and_assert_user_memory_audit(
        fixture: &QueueFixture,
        original: &MemoryRecord,
        expected_revision: u64,
        now: DateTime<Utc>,
    ) {
        let deleted = fixture
            .storage
            .delete_memory_record_tombstone(
                &fixture.conversation_id,
                &fixture.branch_id,
                &original.id,
                expected_revision,
                now + Duration::seconds(7),
            )
            .expect("tombstone");
        assert_eq!(deleted.revision, 7);
        assert_eq!(deleted.deleted_at, Some(now + Duration::seconds(7)));
        assert!(
            fixture
                .storage
                .get_memory_record(&fixture.conversation_id, &fixture.branch_id, &original.id,)
                .is_err()
        );
        let audit = fixture
            .storage
            .connection()
            .expect("database connection")
            .query_row(
                "SELECT
                     (SELECT COUNT(*) FROM memory_record_revisions WHERE record_id = ?1),
                     (SELECT COUNT(*) FROM memory_record_events WHERE record_id = ?1),
                     (SELECT invalidation_reason FROM memory_record_state WHERE record_id = ?1),
                     (SELECT event_kind FROM memory_record_events
                      WHERE record_id = ?1 ORDER BY sequence DESC LIMIT 1)",
                [original.id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .expect("user control audit");
        assert_eq!(audit.0, 5);
        assert_eq!(audit.1, 7);
        assert_eq!(audit.2.as_deref(), Some("source_range_changed"));
        assert_eq!(audit.3, "deleted");
    }

    #[test]
    fn user_memory_controls_are_cas_scoped_audited_and_tombstoned() {
        let fixture = queue_fixture();
        let now = Utc::now() - Duration::seconds(30);
        let (original, completed) = prepare_user_memory_record(&fixture, now);
        let patched =
            patch_and_assert_user_memory_record(&fixture, &original, completed.revision, now);
        assert_invalid_user_memory_patches(
            &fixture,
            &original,
            completed.revision,
            patched.revision,
            now,
        );
        let restored = invalidate_and_exclude_user_memory_record(&fixture, &original, now);
        tombstone_and_assert_user_memory_audit(&fixture, &original, restored.revision, now);
    }

    fn assert_memory_record_owner_mismatch(
        fixture: &QueueFixture,
        original: &MemoryRecord,
        completed_revision: u64,
        now: DateTime<Utc>,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        mismatch: &str,
    ) {
        let get_error = fixture
            .storage
            .get_memory_record(conversation_id, branch_id, &original.id)
            .unwrap_err();
        assert_eq!(get_error.code, CoreErrorCode::NotFound, "{mismatch} get");

        let patch_error = fixture
            .storage
            .patch_memory_record_user_fields(
                conversation_id,
                branch_id,
                &original.id,
                completed_revision,
                &MemoryRecordUserPatch {
                    title: Some("foreign overwrite".to_owned()),
                    ..MemoryRecordUserPatch::default()
                },
                now + Duration::seconds(2),
            )
            .unwrap_err();
        assert_eq!(
            patch_error.code,
            CoreErrorCode::NotFound,
            "{mismatch} patch"
        );

        let exclusion_error = fixture
            .storage
            .set_memory_record_exclusion(
                conversation_id,
                branch_id,
                &original.id,
                completed_revision,
                (MemoryRecordExclusionScope::Conversation, true),
                now + Duration::seconds(2),
            )
            .unwrap_err();
        assert_eq!(
            exclusion_error.code,
            CoreErrorCode::NotFound,
            "{mismatch} exclusion"
        );

        let delete_error = fixture
            .storage
            .delete_memory_record_tombstone(
                conversation_id,
                branch_id,
                &original.id,
                completed_revision,
                now + Duration::seconds(2),
            )
            .unwrap_err();
        assert_eq!(
            delete_error.code,
            CoreErrorCode::NotFound,
            "{mismatch} delete"
        );
    }

    #[test]
    fn memory_record_controls_deny_cross_room_owner() {
        let fixture = queue_fixture();
        let now = Utc::now() - Duration::seconds(30);
        let (original, completed) = prepare_user_memory_record(&fixture, now);
        let foreign_conversation = ConversationId("conversation:foreign".to_owned());
        let foreign_branch = ConversationBranchId("branch:foreign".to_owned());
        for (conversation_id, branch_id, mismatch) in [
            (&foreign_conversation, &fixture.branch_id, "conversation"),
            (&fixture.conversation_id, &foreign_branch, "branch"),
        ] {
            assert_memory_record_owner_mismatch(
                &fixture,
                &original,
                completed.revision,
                now,
                conversation_id,
                branch_id,
                mismatch,
            );
        }

        let unchanged = fixture
            .storage
            .get_memory_record(&fixture.conversation_id, &fixture.branch_id, &original.id)
            .expect("load denied record operations");
        assert_eq!(unchanged, completed);

        let patched = fixture
            .storage
            .patch_memory_record_user_fields(
                &fixture.conversation_id,
                &fixture.branch_id,
                &original.id,
                completed.revision,
                &MemoryRecordUserPatch {
                    title: Some("single accepted overwrite".to_owned()),
                    ..MemoryRecordUserPatch::default()
                },
                now + Duration::seconds(2),
            )
            .expect("owner-bound patch");
        let replay = fixture
            .storage
            .patch_memory_record_user_fields(
                &fixture.conversation_id,
                &fixture.branch_id,
                &original.id,
                completed.revision,
                &MemoryRecordUserPatch {
                    title: Some("replayed overwrite".to_owned()),
                    ..MemoryRecordUserPatch::default()
                },
                now + Duration::seconds(2),
            )
            .expect_err("stale record patch replay must fail");
        assert_eq!(replay.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            fixture
                .storage
                .get_memory_record(&fixture.conversation_id, &fixture.branch_id, &original.id)
                .expect("load single patch"),
            patched
        );
    }

    #[test]
    fn descendant_listing_preserves_ancestor_record_owner_for_mutation() {
        let fixture = queue_fixture();
        let now = Utc::now() - Duration::seconds(30);
        let (original, completed) = prepare_user_memory_record(&fixture, now);
        let descendant_id = ConversationBranchId("branch:memory-descendant".to_owned());
        fixture
            .storage
            .connection()
            .expect("descendant branch fixture connection")
            .execute(
                "INSERT INTO conversation_branches
                 (id, conversation_id, title, fork_message_id, head_message_id,
                  created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?5)",
                params![
                    descendant_id.0,
                    fixture.conversation_id.0,
                    "descendant memory view",
                    fixture.source_end_message_id.0,
                    now.to_rfc3339(),
                ],
            )
            .expect("insert descendant branch topology");

        let visible = fixture
            .storage
            .list_memory_records(&fixture.conversation_id, &descendant_id, false)
            .expect("list descendant-visible memory");
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].value.id, original.id);
        assert_eq!(visible[0].value.branch_id, fixture.branch_id);

        let descendant_owner = fixture
            .storage
            .patch_memory_record_user_fields(
                &fixture.conversation_id,
                &descendant_id,
                &original.id,
                completed.revision,
                &MemoryRecordUserPatch {
                    pinned: Some(true),
                    ..MemoryRecordUserPatch::default()
                },
                now + Duration::seconds(2),
            )
            .expect_err("descendant visibility must not rewrite persisted ownership");
        assert_eq!(descendant_owner.code, CoreErrorCode::NotFound);

        let updated = fixture
            .storage
            .patch_memory_record_user_fields(
                &fixture.conversation_id,
                &fixture.branch_id,
                &original.id,
                completed.revision,
                &MemoryRecordUserPatch {
                    pinned: Some(true),
                    ..MemoryRecordUserPatch::default()
                },
                now + Duration::seconds(2),
            )
            .expect("persisted ancestor owner may mutate visible memory");
        assert!(updated.value.pinned);
    }
}
