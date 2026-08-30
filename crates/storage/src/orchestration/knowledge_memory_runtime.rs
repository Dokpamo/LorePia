//! Knowledge activation evidence and memory jobs/embeddings.

use super::{
    CoreError, CoreErrorCode, CoreResult, DateTime, GenerationPromptPlanRecord,
    KnowledgeActivationLog, KnowledgeActivationReason, MAX_MEMORY_EMBEDDING_DIMENSIONS,
    MemoryEmbeddingRecord, MemoryJob, MemoryJobId, MemoryJobStatus, MemoryRecordId, ModelRouteId,
    OptionalExtension, Storage, StoredRevision, Transaction, TransactionBehavior, Utc, Value,
    active_content_revision_id, encode_document, enum_wire, i64_revision,
    load_applied_module_runtime_plan_transaction, nonnegative_u32, not_found, params,
    parse_datetime, revision_conflict, sha256_hex, storage_corrupted, storage_db_error,
    u64_revision, validate_identifier, validate_optional_sha256,
};

impl Storage {
    pub fn save_memory_job(
        &self,
        job: &MemoryJob,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<MemoryJob>> {
        save_memory_job(self, job, expected_revision)
    }

    pub fn get_memory_job(&self, id: &MemoryJobId) -> CoreResult<StoredRevision<MemoryJob>> {
        get_memory_job(self, id)
    }

    pub fn save_memory_embedding(&self, embedding: &MemoryEmbeddingRecord) -> CoreResult<()> {
        save_memory_embedding(self, embedding)
    }

    pub fn get_memory_embedding(&self, id: &str) -> CoreResult<MemoryEmbeddingRecord> {
        get_memory_embedding(self, id)
    }
}

pub(super) fn write_generation_knowledge_logs(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    logs: &[KnowledgeActivationLog],
) -> CoreResult<()> {
    for (ordinal, log) in logs.iter().enumerate() {
        require_generation_knowledge_log_authority(transaction, record, log)?;
        let (activation_source, score_millionths) = knowledge_activation_summary(&log.reasons);
        let reason_json = serde_json::to_string(&serde_json::json!({
            "log_id": log.id,
            "reasons": log.reasons,
            "exclusion_reason": log.exclusion_reason,
        }))
        .map_err(|error| {
            CoreError::invalid(format!(
                "cannot encode knowledge activation evidence: {error}"
            ))
        })?;
        let ordinal = i64::try_from(ordinal)
            .map_err(|_| CoreError::invalid("too many knowledge activation logs"))?;
        transaction
            .execute(
                "INSERT INTO knowledge_activation_logs
                 (plan_id, ordinal, book_revision_id, entry_id,
                  activation_source, selected, score_millionths,
                  estimated_tokens, reason_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    record.id,
                    ordinal,
                    log.book_revision_id,
                    log.entry_id.as_str(),
                    activation_source,
                    log.selected,
                    score_millionths,
                    log.estimated_tokens,
                    reason_json,
                    log.created_at.to_rfc3339(),
                ],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute(
                "INSERT INTO generation_prompt_plan_knowledge_selections
                 (plan_id, ordinal, book_revision_id, entry_id, selected,
                  activation_source, score_millionths, estimated_tokens,
                  reason_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    record.id,
                    ordinal,
                    log.book_revision_id,
                    log.entry_id.as_str(),
                    log.selected,
                    activation_source,
                    score_millionths,
                    log.estimated_tokens,
                    reason_json,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn require_generation_knowledge_log_authority(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    log: &KnowledgeActivationLog,
) -> CoreResult<()> {
    if log.conversation_id != record.conversation_id || log.branch_id != record.branch_id {
        return Err(CoreError::invalid(
            "knowledge activation log belongs to another conversation branch",
        ));
    }
    let exact_entry_exists = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM knowledge_book_revisions AS revision
                JOIN knowledge_entries AS entry
                  ON entry.book_revision_id = revision.revision_id
                WHERE revision.knowledge_book_id = ?1
                  AND revision.revision_id = ?2
                  AND entry.entry_id = ?3
             )",
            params![
                log.book_id.as_str(),
                log.book_revision_id,
                log.entry_id.as_str()
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if !exact_entry_exists {
        return Err(CoreError::invalid(
            "knowledge activation log does not name an exact immutable book entry",
        ));
    }
    let active_book_revision_id =
        active_content_revision_id(transaction, log.book_id.as_str(), "knowledge_book")?;
    let selected_by_exact_module_plan =
        module_plan_selects_knowledge_revision(transaction, record, log)?;
    let selected_by_exact_prompt_preset =
        prompt_preset_selects_knowledge_revision(transaction, record, log)?;
    let selected_by_generation_attempt =
        generation_attempt_selects_knowledge_revision(transaction, record, log)?;
    if active_book_revision_id != log.book_revision_id
        && !selected_by_exact_module_plan
        && !selected_by_exact_prompt_preset
        && !selected_by_generation_attempt
    {
        return Err(CoreError::invalid(
            "knowledge book changed after prompt resolution; resolve a new prompt plan",
        ));
    }
    Ok(())
}

fn generation_attempt_selects_knowledge_revision(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    log: &KnowledgeActivationLog,
) -> CoreResult<bool> {
    let attempt = crate::generation_attempt::read_attempt(transaction, &record.generation_id)?;
    if attempt.input.conversation_id != record.conversation_id
        || attempt.input.proposed_branch_id != record.branch_id
    {
        return Err(storage_corrupted(
            "generation prompt plan is detached from its attempt authority",
        ));
    }
    Ok(attempt
        .input
        .prompt_selection_authority
        .as_ref()
        .and_then(|authority| authority.character_knowledge_book.as_ref())
        .is_some_and(|book| {
            book.value.id == log.book_id
                && book.revision_id.as_deref() == Some(log.book_revision_id.as_str())
        }))
}

fn prompt_preset_selects_knowledge_revision(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    log: &KnowledgeActivationLog,
) -> CoreResult<bool> {
    transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM prompt_preset_knowledge_books AS dependency
                JOIN prompt_preset_revisions AS preset
                  ON preset.revision_id = dependency.prompt_preset_revision_id
                JOIN knowledge_book_revisions AS book
                  ON book.revision_id = dependency.knowledge_book_revision_id
                WHERE dependency.prompt_preset_revision_id = ?1
                  AND preset.prompt_preset_id = ?2
                  AND dependency.enabled = 1
                  AND book.knowledge_book_id = ?3
                  AND dependency.knowledge_book_revision_id = ?4
             )",
            params![
                record.prompt_preset_revision_id,
                record.prompt_preset_id.as_str(),
                log.book_id.as_str(),
                log.book_revision_id,
            ],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)
}

fn module_plan_selects_knowledge_revision(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
    log: &KnowledgeActivationLog,
) -> CoreResult<bool> {
    let Some(runtime) = load_generation_module_plan_evidence(transaction, record)? else {
        return Ok(false);
    };
    for component in &runtime.plan.components {
        let lorepia_domain::ModuleComponentRef::KnowledgeBook { id } = &component.component else {
            continue;
        };
        if id != &log.book_id {
            continue;
        }
        let exact = transaction
            .query_row(
                "SELECT component.knowledge_book_revision_id,
                        component.component_sha256, revision.source_hash,
                        book.knowledge_book_id
                 FROM content_module_revisions AS revision
                 JOIN content_module_components AS component
                   ON component.module_revision_id = revision.revision_id
                  AND component.component_kind = 'knowledge_book'
                 JOIN knowledge_book_revisions AS book
                   ON book.revision_id = component.knowledge_book_revision_id
                 WHERE revision.module_id = ?1
                   AND revision.revision_id = ?2
                   AND book.knowledge_book_id = ?3",
                params![
                    component.selected_source.module_id.as_str(),
                    component.selected_source.revision_id.as_str(),
                    log.book_id.as_str(),
                ],
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
        if let Some(exact) = exact {
            if exact.1 != component.sha256.as_str()
                || exact.2 != component.selected_source.revision_source_sha256.as_str()
                || exact.3 != log.book_id.as_str()
            {
                return Err(storage_corrupted(
                    "applied module knowledge component diverges from its exact revision",
                ));
            }
            return Ok(exact.0 == log.book_revision_id);
        }
        return Err(storage_corrupted(
            "applied module plan knowledge component cannot be resolved exactly",
        ));
    }
    Ok(false)
}

pub(super) fn load_generation_module_plan_evidence(
    transaction: &Transaction<'_>,
    record: &GenerationPromptPlanRecord,
) -> CoreResult<Option<lorepia_orchestration::AppliedModuleRuntimePlan>> {
    let module_plan_sha256 = match record
        .provider_request
        .mapping_diagnostics
        .value
        .get("module_plan_sha256")
    {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(value)) => value,
        Some(_) => {
            return Err(CoreError::invalid(
                "generation module plan evidence must be a SHA-256 string or null",
            ));
        }
    };
    validate_optional_sha256("generation module plan hash", Some(module_plan_sha256))?;
    let applied_plan_sha256 = lorepia_domain::Sha256Digest::parse(module_plan_sha256.to_owned())
        .map_err(|_| CoreError::invalid("generation module plan hash is not canonical"))?;
    let runtime =
        match load_applied_module_runtime_plan_transaction(transaction, &applied_plan_sha256) {
            Ok(runtime) => runtime,
            Err(error) if error.code == CoreErrorCode::NotFound => {
                return Err(CoreError::invalid(
                    "generation references an unknown applied module runtime plan",
                ));
            }
            Err(error) => return Err(error),
        };
    if runtime.review.context.conversation_id.as_deref() != Some(record.conversation_id.0.as_str())
        || runtime.review.context.branch_id.as_deref() != Some(record.branch_id.0.as_str())
    {
        return Err(CoreError::invalid(
            "generation module plan belongs to another resolution context",
        ));
    }
    Ok(Some(runtime))
}

fn knowledge_activation_summary(
    reasons: &[KnowledgeActivationReason],
) -> (&'static str, Option<u32>) {
    match reasons.first() {
        Some(KnowledgeActivationReason::Always) | None => ("always", None),
        Some(KnowledgeActivationReason::Manual) => ("manual", None),
        Some(KnowledgeActivationReason::Keyword { .. }) => ("keyword", None),
        Some(KnowledgeActivationReason::Regex { .. }) => ("regex", None),
        Some(KnowledgeActivationReason::Semantic { score_millionths }) => {
            ("semantic", Some(*score_millionths))
        }
        Some(KnowledgeActivationReason::Condition) => ("condition", None),
        Some(KnowledgeActivationReason::Recursive { .. }) => ("recursive", None),
    }
}

fn memory_job_fingerprint(job: &MemoryJob) -> CoreResult<String> {
    let value = serde_json::json!({
        "schema_version": 1,
        "idempotency_key": job.idempotency_key,
        "kind": job.kind,
        "conversation_id": job.conversation_id,
        "branch_id": job.branch_id,
        "source_start_message_id": job.source_start_message_id,
        "source_end_message_id": job.source_end_message_id,
    });
    serde_json::to_vec(&value)
        .map(|bytes| sha256_hex(&bytes))
        .map_err(|error| CoreError::invalid(format!("cannot fingerprint memory job: {error}")))
}

fn memory_job_state(status: MemoryJobStatus) -> &'static str {
    match status {
        MemoryJobStatus::Queued => "queued",
        MemoryJobStatus::Running => "running",
        MemoryJobStatus::Interrupted => "interrupted",
        MemoryJobStatus::Succeeded => "succeeded",
        MemoryJobStatus::Failed => "failed",
        MemoryJobStatus::Cancelled => "cancelled",
    }
}

struct MemoryJobSaveContext {
    revision: u64,
    created_at: DateTime<Utc>,
    input_fingerprint: String,
    payload_json: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    failure_json: Option<String>,
}

fn save_memory_job(
    storage: &Storage,
    job: &MemoryJob,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<MemoryJob>> {
    validate_memory_job_for_save(job)?;
    let payload_json = encode_document("memory job", job)?.0;
    let input_fingerprint = memory_job_fingerprint(job)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let context = prepare_memory_job_save(
        &transaction,
        job,
        expected_revision,
        payload_json,
        input_fingerprint,
    )?;
    write_memory_job_row(&transaction, job, expected_revision, &context)?;
    transaction.commit().map_err(storage_db_error)?;
    let mut value = job.clone();
    value.created_at = context.created_at;
    Ok(StoredRevision {
        value,
        revision: context.revision,
        revision_id: None,
        created_at: context.created_at,
        updated_at: job.updated_at,
        deleted_at: None,
    })
}

fn validate_memory_job_for_save(job: &MemoryJob) -> CoreResult<()> {
    validate_identifier("memory job", job.id.as_str())?;
    validate_identifier("memory job idempotency key", &job.idempotency_key)?;
    if job.updated_at < job.created_at {
        return Err(CoreError::invalid(
            "memory job update time predates creation",
        ));
    }
    if job.status == MemoryJobStatus::Failed && job.error_code.is_none() {
        return Err(CoreError::invalid(
            "failed memory job requires an error code",
        ));
    }
    if job.status != MemoryJobStatus::Failed && job.error_code.is_some() {
        return Err(CoreError::invalid(
            "only a failed memory job may carry an error code",
        ));
    }
    Ok(())
}

fn prepare_memory_job_save(
    transaction: &Transaction<'_>,
    job: &MemoryJob,
    expected_revision: Option<u64>,
    payload_json: String,
    input_fingerprint: String,
) -> CoreResult<MemoryJobSaveContext> {
    let current = transaction
        .query_row(
            "SELECT revision, created_at, state FROM memory_jobs WHERE id = ?1",
            [job.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    let (revision, created_at) = match (expected_revision, current) {
        (None, None) if job.status == MemoryJobStatus::Queued => (1_u64, job.created_at),
        (None, None) => {
            return Err(CoreError::invalid("memory job must begin queued"));
        }
        (None, Some((actual, _, _))) => {
            return Err(revision_conflict(
                "memory job",
                job.id.as_str(),
                None,
                Some(u64_revision(actual)?),
            ));
        }
        (Some(expected), Some((actual, created_at, _))) if u64_revision(actual)? == expected => (
            expected
                .checked_add(1)
                .ok_or_else(|| CoreError::internal("memory job revision overflow"))?,
            parse_datetime("memory job created_at", &created_at)?,
        ),
        (Some(expected), Some((actual, _, _))) => {
            return Err(revision_conflict(
                "memory job",
                job.id.as_str(),
                Some(expected),
                Some(u64_revision(actual)?),
            ));
        }
        (Some(expected), None) => {
            return Err(revision_conflict(
                "memory job",
                job.id.as_str(),
                Some(expected),
                None,
            ));
        }
    };
    let started_at = match job.status {
        MemoryJobStatus::Running | MemoryJobStatus::Interrupted => {
            Some(job.updated_at.to_rfc3339())
        }
        MemoryJobStatus::Succeeded | MemoryJobStatus::Failed | MemoryJobStatus::Cancelled => {
            Some(job.created_at.to_rfc3339())
        }
        MemoryJobStatus::Queued => None,
    };
    let finished_at = matches!(
        job.status,
        MemoryJobStatus::Succeeded | MemoryJobStatus::Failed | MemoryJobStatus::Cancelled
    )
    .then(|| job.updated_at.to_rfc3339());
    let failure_json = job
        .error_code
        .as_ref()
        .map(|code| serde_json::to_string(&serde_json::json!({"error_code": code})))
        .transpose()
        .map_err(|error| {
            CoreError::invalid(format!("cannot encode memory job failure: {error}"))
        })?;
    Ok(MemoryJobSaveContext {
        revision,
        created_at,
        input_fingerprint,
        payload_json,
        started_at,
        finished_at,
        failure_json,
    })
}

fn write_memory_job_row(
    transaction: &Transaction<'_>,
    job: &MemoryJob,
    expected_revision: Option<u64>,
    context: &MemoryJobSaveContext,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "INSERT INTO memory_jobs
             (id, idempotency_key, job_kind, memory_profile_revision_id,
              task_profile_revision_id, conversation_id, branch_id,
              source_start_message_id, source_end_message_id,
              input_fingerprint_sha256, state, revision, attempts, available_at,
              started_at, finished_at, result_record_id, failure_json,
              payload_json, created_at, updated_at)
             VALUES (
                 ?1, ?2, ?3, NULL, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                 ?11, ?12, ?13, ?14, NULL, ?15, ?16, ?17, ?18
             )
             ON CONFLICT(id) DO UPDATE SET
                 state = excluded.state,
                 revision = excluded.revision,
                 attempts = excluded.attempts,
                 started_at = excluded.started_at,
                 finished_at = excluded.finished_at,
                 failure_json = excluded.failure_json,
                 payload_json = excluded.payload_json,
                 updated_at = excluded.updated_at
             WHERE memory_jobs.revision = ?19",
            params![
                job.id.as_str(),
                job.idempotency_key,
                enum_wire(&job.kind)?,
                job.conversation_id.0,
                job.branch_id.0,
                job.source_start_message_id.0,
                job.source_end_message_id.0,
                context.input_fingerprint.as_str(),
                memory_job_state(job.status),
                i64_revision(context.revision)?,
                job.attempt,
                context.created_at.to_rfc3339(),
                context.started_at.as_deref(),
                context.finished_at.as_deref(),
                context.failure_json.as_deref(),
                context.payload_json.as_str(),
                context.created_at.to_rfc3339(),
                job.updated_at.to_rfc3339(),
                expected_revision
                    .map(i64_revision)
                    .transpose()?
                    .unwrap_or(0),
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "memory job",
            job.id.as_str(),
            expected_revision,
            None,
        ));
    }
    Ok(())
}

fn get_memory_job(storage: &Storage, id: &MemoryJobId) -> CoreResult<StoredRevision<MemoryJob>> {
    let entry = storage.get_memory_job_queue_entry(id)?;
    let value = entry.job;
    Ok(StoredRevision {
        created_at: value.created_at,
        updated_at: value.updated_at,
        value,
        revision: entry.revision,
        revision_id: None,
        deleted_at: None,
    })
}

fn save_memory_embedding(storage: &Storage, embedding: &MemoryEmbeddingRecord) -> CoreResult<()> {
    validate_identifier("memory embedding", &embedding.id)?;
    if embedding.values.is_empty()
        || embedding.values.len() > MAX_MEMORY_EMBEDDING_DIMENSIONS
        || embedding.values.len()
            != usize::try_from(embedding.dimensions)
                .map_err(|_| CoreError::invalid("memory embedding dimensions are invalid"))?
        || embedding.values.iter().any(|value| !value.is_finite())
    {
        return Err(CoreError::invalid(
            "memory embedding dimensions or values are invalid",
        ));
    }
    let model_route_id = embedding
        .model_route_id
        .as_ref()
        .ok_or_else(|| CoreError::invalid("memory embedding requires a model route"))?;
    let connection = storage.connection()?;
    let record_revision_id = connection
        .query_row(
            "SELECT active_revision_id FROM memory_record_state
             WHERE record_id = ?1 AND deleted_at IS NULL",
            [embedding.memory_record_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory embedding record"))?;
    let mut bytes = Vec::with_capacity(embedding.values.len().saturating_mul(4));
    for value in &embedding.values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let vector_sha256 = sha256_hex(&bytes);
    connection
        .execute(
            "INSERT INTO memory_embeddings
             (id, record_revision_id, task_profile_revision_id, model_route_id,
              dimensions, encoding, vector_blob, vector_sha256, created_at)
             VALUES (?1, ?2, NULL, ?3, ?4, 'f32le', ?5, ?6, ?7)",
            params![
                embedding.id,
                record_revision_id,
                model_route_id.as_str(),
                embedding.dimensions,
                bytes,
                vector_sha256,
                embedding.created_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn get_memory_embedding(storage: &Storage, id: &str) -> CoreResult<MemoryEmbeddingRecord> {
    let row = storage
        .connection()?
        .query_row(
            "SELECT embedding.id, revision.record_id, embedding.model_route_id,
                    embedding.dimensions, embedding.vector_blob,
                    embedding.created_at
             FROM memory_embeddings AS embedding
             JOIN memory_record_revisions AS revision
               ON revision.id = embedding.record_revision_id
             WHERE embedding.id = ?1 AND embedding.encoding = 'f32le'",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, Vec<u8>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("memory embedding"))?;
    let dimensions = nonnegative_u32("memory embedding dimensions", row.3)?;
    let expected_bytes = usize::try_from(dimensions)
        .map_err(|_| storage_corrupted("stored memory embedding dimensions are invalid"))?
        .checked_mul(4)
        .ok_or_else(|| storage_corrupted("stored memory embedding size overflow"))?;
    if row.4.len() != expected_bytes {
        return Err(storage_corrupted(
            "stored memory embedding byte length is invalid",
        ));
    }
    let values = row
        .4
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    if values.iter().any(|value| !value.is_finite()) {
        return Err(storage_corrupted(
            "stored memory embedding contains a non-finite value",
        ));
    }
    Ok(MemoryEmbeddingRecord {
        id: row.0,
        memory_record_id: MemoryRecordId::from(row.1),
        model_route_id: Some(ModelRouteId::from(row.2)),
        dimensions,
        values,
        created_at: parse_datetime("memory embedding created_at", &row.5)?,
    })
}
