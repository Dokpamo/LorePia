use std::{sync::Arc, time::Duration};

use chrono::Utc;
use lorepia_domain::{
    AuxiliaryTaskKind, ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult,
    MemoryJob, MemoryJobId, MemoryJobKind, MemoryJobStatus, MemoryProfile, MemoryRecord, MessageId,
    ModelAvailability, ModelRouteId, ProviderConnection, TaskProfile, ValidateOrchestration,
};
use lorepia_orchestration::{MemoryJobKeyInput, derive_memory_job_idempotency_key};
use lorepia_providers::{
    AdapterRegistry, EmbeddingProvider, EmbeddingPurpose, EmbeddingRequest, EmbeddingRunOutcome,
    MAX_EMBEDDING_INPUT_BYTES, MAX_EMBEDDING_INPUT_CHARS,
};
use lorepia_storage::{
    MemoryEmbeddingJobInput, MemoryEmbeddingJobSeed, MemoryEmbeddingRecord, MemoryJobFinish,
    MemoryQueryEmbeddingIntent, ObjectRevision, StoredMemoryJobQueueEntry,
};

use super::{
    auxiliary_tasks::{
        MemoryJobExecutionResult, TaskCredentialBroker, memory_execution_without_record,
        memory_job_error, memory_job_id_from_key, queue_entry_as_revisioned,
    },
    versioned_digest,
};
use crate::Core;

const MAX_MEMORY_EMBEDDING_CANDIDATES: usize = 2_048;
const MAX_MEMORY_EMBEDDING_QUERY_BYTES: usize = 16 * 1_024 * 1_024;
pub(super) struct ResolvedEmbeddingTask {
    pub(super) task_profile: ObjectRevision<TaskProfile>,
    pub(super) connection: ProviderConnection,
    pub(super) provider: Arc<dyn EmbeddingProvider>,
}
struct PreparedClaimedMemoryEmbedding {
    input: MemoryEmbeddingJobInput,
    record: ObjectRevision<MemoryRecord>,
    resolved: ResolvedEmbeddingTask,
}
pub(super) enum EmbeddingDispatchOutcome {
    Completed(Vec<f32>),
    Failed(CoreError),
    CancelledBeforeDispatch,
    UnknownOutcome,
}
impl Core {
    pub(super) fn resolve_exact_embedding_task(
        &self,
        memory_profile: &ObjectRevision<MemoryProfile>,
        expected_task_profile_revision_id: Option<&str>,
    ) -> CoreResult<ResolvedEmbeddingTask> {
        let expected_task_id = memory_profile
            .value
            .embedding_task
            .as_ref()
            .ok_or_else(|| CoreError::invalid("memory profile has no embedding task"))?;
        let task_profile = self
            .storage()
            .get_memory_profile_embedding_task_revision(&memory_profile.revision_id)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "memory profile embedding task revision is missing",
                    false,
                )
            })?;
        task_profile.value.validate().map_err(|error| {
            CoreError::invalid(format!("invalid memory embedding task profile: {error}"))
        })?;
        if task_profile.value.id != *expected_task_id
            || task_profile.value.kind != AuxiliaryTaskKind::MemoryEmbedding
            || expected_task_profile_revision_id
                .is_some_and(|expected| expected != task_profile.revision_id)
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "memory profile does not bind the expected exact embedding task revision",
                false,
            ));
        }
        let dimensions = task_profile
            .value
            .embedding_dimensions
            .ok_or_else(|| CoreError::invalid("memory embedding task has no exact dimensions"))?;
        let route = self
            .storage()
            .get_model_route(&task_profile.value.route_id)?;
        if matches!(
            route.status,
            ModelAvailability::MissingTemporarily
                | ModelAvailability::AccessDenied
                | ModelAvailability::Deprecated
                | ModelAvailability::Retired
        ) {
            return Err(CoreError::new(
                CoreErrorCode::ProviderUnavailable,
                "memory embedding model route is not currently available",
                true,
            ));
        }
        let connection = self
            .storage()
            .get_provider_connection(&route.connection_id)?;
        let template = self
            .storage()
            .get_provider_template(&connection.template_id, connection.template_version)?;
        let provider = AdapterRegistry::new().build_embedding_provider_for_route(
            &template,
            &connection,
            &route,
            dimensions,
        )?;
        let contract = provider.contract();
        if contract.connection_id() != &connection.id
            || contract.model_route_id() != &task_profile.value.route_id
            || contract.model_id() != route.model_id
            || contract.dimensions() != dimensions
            || contract.api_family() != route.api_family
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "resolved embedding provider contract differs from the exact task profile",
                false,
            ));
        }
        Ok(ResolvedEmbeddingTask {
            task_profile,
            connection,
            provider,
        })
    }

    pub(super) async fn dispatch_embedding(
        &self,
        resolved: &ResolvedEmbeddingTask,
        input: String,
        purpose: EmbeddingPurpose,
        credential_broker: &dyn TaskCredentialBroker,
        mut cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> EmbeddingDispatchOutcome {
        if *cancelled.borrow() {
            return EmbeddingDispatchOutcome::CancelledBeforeDispatch;
        }
        let contract = resolved.provider.contract();
        let request =
            match EmbeddingRequest::new(contract.model_id(), input, contract.dimensions(), purpose)
            {
                Ok(request) => request,
                Err(error) => return EmbeddingDispatchOutcome::Failed(error),
            };
        let credential = match credential_broker
            .credential_for(contract.connection_id())
            .await
        {
            Ok(credential) => credential,
            Err(error) => return EmbeddingDispatchOutcome::Failed(error),
        };
        let credential_value = match credential.value_for_connection(&resolved.connection) {
            Ok(value) => value,
            Err(error) => return EmbeddingDispatchOutcome::Failed(error),
        };
        if *cancelled.borrow() {
            return EmbeddingDispatchOutcome::CancelledBeforeDispatch;
        }
        let (attempt_cancel_sender, attempt_cancel_receiver) = tokio::sync::watch::channel(false);
        let provider_attempt =
            resolved
                .provider
                .embed(request, credential_value, attempt_cancel_receiver);
        tokio::pin!(provider_attempt);
        let timeout = tokio::time::sleep(Duration::from_millis(
            resolved.task_profile.value.timeout_ms,
        ));
        tokio::pin!(timeout);
        let cancellation = async {
            loop {
                if *cancelled.borrow() {
                    break;
                }
                if cancelled.changed().await.is_err() {
                    std::future::pending::<()>().await;
                }
            }
        };
        tokio::pin!(cancellation);
        let outcome = tokio::select! {
            outcome = &mut provider_attempt => outcome,
            () = &mut cancellation => {
                let _ = attempt_cancel_sender.send(true);
                return EmbeddingDispatchOutcome::UnknownOutcome;
            }
            () = &mut timeout => {
                let _ = attempt_cancel_sender.send(true);
                return EmbeddingDispatchOutcome::UnknownOutcome;
            }
        };
        match outcome {
            EmbeddingRunOutcome::Completed(output) => {
                EmbeddingDispatchOutcome::Completed(output.into_values())
            }
            EmbeddingRunOutcome::Failed(failure) => {
                EmbeddingDispatchOutcome::Failed(failure.into_core_error())
            }
            EmbeddingRunOutcome::CancelledBeforeDispatch => {
                EmbeddingDispatchOutcome::CancelledBeforeDispatch
            }
            EmbeddingRunOutcome::UnknownOutcome(_) => EmbeddingDispatchOutcome::UnknownOutcome,
        }
    }

    fn prepare_claimed_memory_embedding(
        &self,
        entry: &StoredMemoryJobQueueEntry,
    ) -> CoreResult<PreparedClaimedMemoryEmbedding> {
        if entry.job.kind != MemoryJobKind::Embedding
            || entry.job.status != MemoryJobStatus::Running
        {
            return Err(CoreError::invalid(
                "memory embedding worker requires one running embedding job",
            ));
        }
        if entry.payload.schema_version != 1 {
            return Err(CoreError::invalid(
                "memory embedding queue input schema version must be 1",
            ));
        }
        let input: MemoryEmbeddingJobInput = serde_json::from_value(entry.payload.value.clone())
            .map_err(|error| {
                CoreError::invalid(format!("invalid memory embedding queue input: {error}"))
            })?;
        let memory_profile = entry
            .memory_profile_revision
            .as_ref()
            .ok_or_else(|| CoreError::invalid("embedding job lacks its exact memory profile"))?;
        let task_profile_revision_id = entry
            .task_profile_revision_id
            .as_deref()
            .ok_or_else(|| CoreError::invalid("embedding job lacks its exact task profile id"))?;
        let resolved =
            self.resolve_exact_embedding_task(memory_profile, Some(task_profile_revision_id))?;
        if input.model_route_id != resolved.task_profile.value.route_id
            || Some(input.dimensions) != resolved.task_profile.value.embedding_dimensions
            || input.vector_space_sha256 != resolved.provider.contract().vector_space_sha256()
        {
            return Err(CoreError::invalid(
                "memory embedding queue input differs from its exact provider vector space",
            ));
        }
        let record = self
            .storage()
            .get_memory_record_revision_by_id(&input.memory_record_revision_id)?;
        if record.value.conversation_id != entry.job.conversation_id
            || record.value.branch_id != entry.job.branch_id
            || record.value.source_start_message_id != entry.job.source_start_message_id
            || record.value.source_end_message_id != entry.job.source_end_message_id
        {
            return Err(CoreError::invalid(
                "memory embedding record revision differs from its queue lineage",
            ));
        }
        Ok(PreparedClaimedMemoryEmbedding {
            input,
            record,
            resolved,
        })
    }

    pub(super) async fn execute_claimed_memory_embedding(
        &self,
        entry: StoredMemoryJobQueueEntry,
        expected_running_revision: u64,
        credential_broker: &dyn TaskCredentialBroker,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> CoreResult<MemoryJobExecutionResult> {
        let Ok(prepared) = self.prepare_claimed_memory_embedding(&entry) else {
            let failed = self.storage().finish_memory_job(
                &entry.job.id,
                expected_running_revision,
                MemoryJobFinish::Failed {
                    error_code: "memory_embedding_input_invalid".to_owned(),
                },
                Utc::now(),
            )?;
            return Ok(memory_execution_without_record(&failed));
        };
        let input = render_memory_embedding_document(&prepared.record.value)?;
        let values = match self
            .dispatch_embedding(
                &prepared.resolved,
                input,
                EmbeddingPurpose::RetrievalDocument,
                credential_broker,
                cancelled,
            )
            .await
        {
            EmbeddingDispatchOutcome::Completed(values) => values,
            EmbeddingDispatchOutcome::CancelledBeforeDispatch => {
                let cancelled = self.storage().finish_memory_job(
                    &entry.job.id,
                    expected_running_revision,
                    MemoryJobFinish::Cancelled,
                    Utc::now(),
                )?;
                return Ok(memory_execution_without_record(&cancelled));
            }
            EmbeddingDispatchOutcome::UnknownOutcome => {
                let interrupted = self.storage().interrupt_memory_job(
                    &entry.job.id,
                    expected_running_revision,
                    Some("provider_unknown_outcome"),
                    Utc::now(),
                )?;
                return Ok(memory_execution_without_record(&interrupted));
            }
            EmbeddingDispatchOutcome::Failed(error) => {
                let error_code = embedding_failure_code(&error);
                let failed = self.storage().finish_memory_job(
                    &entry.job.id,
                    expected_running_revision,
                    MemoryJobFinish::Failed {
                        error_code: error_code.to_owned(),
                    },
                    Utc::now(),
                )?;
                return Ok(memory_execution_without_record(&failed));
            }
        };
        let finished_at = Utc::now();
        let embedding = MemoryEmbeddingRecord {
            id: memory_embedding_id(
                &entry.job.id,
                &prepared.input.memory_record_revision_id,
                &prepared.input.model_route_id,
                prepared.input.dimensions,
            )?,
            memory_record_id: prepared.record.value.id.clone(),
            model_route_id: Some(prepared.input.model_route_id),
            dimensions: prepared.input.dimensions,
            values,
            created_at: finished_at,
        };
        let completed = self.storage().complete_memory_embedding_job(
            &entry.job.id,
            expected_running_revision,
            &embedding,
            finished_at,
        )?;
        Ok(MemoryJobExecutionResult {
            job: queue_entry_as_revisioned(&completed.job),
            record: None,
        })
    }
}

fn render_memory_embedding_document(record: &MemoryRecord) -> CoreResult<String> {
    record.validate().map_err(|error| {
        CoreError::invalid(format!("memory embedding record is invalid: {error}"))
    })?;
    let keywords = record.keywords.join(", ");
    let rendered = format!(
        "Title:\n{}\n\nSummary:\n{}\n\nKeywords:\n{}",
        record.title, record.summary, keywords
    );
    let mut bounded = String::with_capacity(rendered.len().min(MAX_EMBEDDING_INPUT_BYTES));
    for character in rendered.chars().take(MAX_EMBEDDING_INPUT_CHARS) {
        if bounded.len() + character.len_utf8() > MAX_EMBEDDING_INPUT_BYTES {
            break;
        }
        bounded.push(character);
    }
    if bounded.is_empty() {
        return Err(CoreError::invalid(
            "memory embedding document has no provider-visible content",
        ));
    }
    Ok(bounded)
}
fn memory_embedding_id(
    job_id: &MemoryJobId,
    record_revision_id: &str,
    model_route_id: &ModelRouteId,
    dimensions: u32,
) -> CoreResult<String> {
    Ok(format!(
        "memory-embedding-{}",
        versioned_digest(&(
            "lorepia.memory-embedding-id.v1",
            job_id.as_str(),
            record_revision_id,
            model_route_id.as_str(),
            dimensions,
        ))?
    ))
}
#[allow(clippy::too_many_arguments)]
pub(super) fn memory_query_embedding_intent(
    memory_profile: &ObjectRevision<MemoryProfile>,
    task_profile: &ObjectRevision<TaskProfile>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    source_start_message_id: &MessageId,
    source_end_message_id: &MessageId,
    query_sha256: &str,
    vector_space_sha256: &str,
    model_route_id: &ModelRouteId,
    dimensions: u32,
    created_at: chrono::DateTime<Utc>,
) -> CoreResult<MemoryQueryEmbeddingIntent> {
    let digest = versioned_digest(&(
        "lorepia.memory-query-embedding-intent.v1",
        memory_profile.value.id.as_str(),
        memory_profile.revision_id.as_str(),
        task_profile.revision_id.as_str(),
        conversation_id.0.as_str(),
        branch_id.0.as_str(),
        source_start_message_id.0.as_str(),
        source_end_message_id.0.as_str(),
        query_sha256,
        vector_space_sha256,
        model_route_id.as_str(),
        dimensions,
    ))?;
    Ok(MemoryQueryEmbeddingIntent {
        id: format!("memory-query-embedding-{digest}"),
        idempotency_key: format!("memory-query-embedding:v1:{digest}"),
        memory_profile_id: memory_profile.value.id.clone(),
        memory_profile_revision_id: memory_profile.revision_id.clone(),
        task_profile_revision_id: task_profile.revision_id.clone(),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
        source_start_message_id: source_start_message_id.clone(),
        source_end_message_id: source_end_message_id.clone(),
        query_sha256: query_sha256.to_owned(),
        vector_space_sha256: vector_space_sha256.to_owned(),
        model_route_id: model_route_id.clone(),
        dimensions,
        created_at,
    })
}
pub(super) const fn embedding_failure_code(error: &CoreError) -> &'static str {
    match error.code {
        CoreErrorCode::ProviderAuthFailed => "embedding_provider_auth_failed",
        CoreErrorCode::ProviderRateLimited => "embedding_provider_rate_limited",
        CoreErrorCode::ProviderUnavailable | CoreErrorCode::NetworkUnavailable => {
            "embedding_provider_unavailable"
        }
        CoreErrorCode::InvalidInput | CoreErrorCode::UnsupportedContent => {
            "embedding_provider_rejected"
        }
        CoreErrorCode::Cancelled => "embedding_provider_cancelled",
        _ => "embedding_provider_failed",
    }
}
pub(super) fn memory_embedding_candidate_limit(
    record_count: usize,
    dimensions: u32,
) -> CoreResult<usize> {
    let dimensions = usize::try_from(dimensions)
        .map_err(|_| CoreError::invalid("memory embedding dimensions are invalid"))?;
    if dimensions == 0 {
        return Err(CoreError::invalid(
            "memory embedding dimensions must be positive",
        ));
    }
    let bytes_per_vector = dimensions
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| CoreError::invalid("memory embedding vector size overflowed"))?;
    let budget_limit = MAX_MEMORY_EMBEDDING_QUERY_BYTES / bytes_per_vector;
    if budget_limit == 0 {
        return Err(CoreError::invalid(
            "one memory embedding vector exceeds the query budget",
        ));
    }
    Ok(record_count
        .min(MAX_MEMORY_EMBEDDING_CANDIDATES)
        .min(budget_limit))
}
pub(super) fn memory_embedding_job_seed(
    summary: &StoredMemoryJobQueueEntry,
    memory_profile: &ObjectRevision<MemoryProfile>,
    task_profile: &ObjectRevision<TaskProfile>,
    vector_space_sha256: &str,
    created_at: chrono::DateTime<Utc>,
) -> CoreResult<MemoryEmbeddingJobSeed> {
    if summary.job.kind != MemoryJobKind::Summary
        || task_profile.value.kind != AuxiliaryTaskKind::MemoryEmbedding
        || memory_profile.value.embedding_task.as_ref() != Some(&task_profile.value.id)
    {
        return Err(CoreError::invalid(
            "memory embedding job seed does not match its exact summary policy",
        ));
    }
    let dimensions = task_profile
        .value
        .embedding_dimensions
        .ok_or_else(|| CoreError::invalid("memory embedding task has no exact dimensions"))?;
    if vector_space_sha256.len() != 64
        || !vector_space_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CoreError::internal(
            "memory embedding provider returned an invalid vector-space digest",
        ));
    }
    let source_revision = versioned_digest(&(
        "lorepia.memory-embedding-job.v1",
        summary.job.id.as_str(),
        memory_profile.revision_id.as_str(),
        task_profile.revision_id.as_str(),
        task_profile.value.route_id.as_str(),
        dimensions,
        vector_space_sha256,
    ))?;
    let idempotency_key = derive_memory_job_idempotency_key(&MemoryJobKeyInput {
        kind: MemoryJobKind::Embedding,
        conversation_id: &summary.job.conversation_id,
        branch_id: &summary.job.branch_id,
        source_start_message_id: &summary.job.source_start_message_id,
        source_end_message_id: &summary.job.source_end_message_id,
        profile_id: Some(&memory_profile.value.id),
        profile_schema_version: Some(memory_profile.value.schema_version),
        source_revision: &source_revision,
    })
    .map_err(memory_job_error)?;
    let job = MemoryJob {
        id: memory_job_id_from_key(&idempotency_key)?,
        idempotency_key,
        kind: MemoryJobKind::Embedding,
        conversation_id: summary.job.conversation_id.clone(),
        branch_id: summary.job.branch_id.clone(),
        source_start_message_id: summary.job.source_start_message_id.clone(),
        source_end_message_id: summary.job.source_end_message_id.clone(),
        status: MemoryJobStatus::Queued,
        attempt: 0,
        created_at,
        updated_at: created_at,
        error_code: None,
    };
    Ok(MemoryEmbeddingJobSeed {
        job,
        memory_profile_revision_id: memory_profile.revision_id.clone(),
        task_profile_revision_id: task_profile.revision_id.clone(),
        model_route_id: task_profile.value.route_id.clone(),
        dimensions,
        vector_space_sha256: vector_space_sha256.to_owned(),
        available_at: created_at,
    })
}
