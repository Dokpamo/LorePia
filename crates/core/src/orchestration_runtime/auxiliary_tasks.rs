mod summary;

use std::{collections::BTreeSet, future::Future, pin::Pin};

use chrono::Utc;
use lorepia_domain::{
    AuxiliaryTaskKind, ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult,
    MemoryJob, MemoryJobKind, MemoryJobStatus, MemoryKind, MemoryProfile, MemoryRecord,
    MemoryRecordId, Message, Provenance, ProviderConnectionId, SourceKind, TaskProfile,
    TaskProfileId, ValidateOrchestration, VersionedJson,
};
use lorepia_orchestration::TransformResult;
use lorepia_storage::{
    MemoryJobFinish, MemoryRecordExclusionScope, MemoryRecordUserPatch, ObjectRevision,
    StoredMemoryJobQueueEntry, StoredRevision,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
pub(in crate::orchestration_runtime) use self::summary::next_memory_summary_turn_window;
pub use self::summary::{
    EnqueueMemorySummaryRequest, MemoryJobEnqueueReceipt, MemoryRuntimeProvenance,
    RuntimeTaskTargetRevision, RuntimeTransformRevision,
};
pub(in crate::orchestration_runtime) use self::summary::{
    MemorySummaryHeadAuthority, ResolvedPromptRuntimePolicy, memory_job_id_from_key,
    memory_summary_system_instruction,
};
use self::summary::{memory_source_sha256, render_memory_source, versioned_sha256};
use super::{memory_embedding_job_seed, versioned_digest};
use crate::{
    ConnectionBoundCredential, Core, Revisioned,
    app::{
        BoundedTaskPrompt, PromptRouteWireContract, TaskDispatchClassification,
        TaskExecutionOutcome, prompt_route_wire_contract, resolve_generation_target,
    },
    revision::project_revision,
};

/// One claimed job and the exact task policy storage used to admit it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimedMemoryJob {
    pub job: Revisioned<MemoryJob>,
    pub memory_profile_revision_id: String,
    pub task_profile_revision_id: String,
}
/// Core-owned, credential-free memory task input.
///
/// This value may be handed to the provider task executor inside Core. It is
/// not a native DTO: raw message text and transform output must not cross the
/// Rust boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreparedMemoryTaskInput {
    pub job: StoredRevision<MemoryJob>,
    pub source_messages: Vec<Message>,
    pub transformed_source: String,
    pub transform_results: Vec<TransformResult>,
    pub source_sha256: String,
    pub task_profile_id: TaskProfileId,
    pub task_profile_revision_id: String,
}
/// Rust-only bridge to a platform secure store.
///
/// Implementations must return a credential cryptographically and exactly
/// bound to the requested provider connection. The broker is invoked once,
/// immediately before each provider attempt permitted by fallback policy. It
/// is never serialized and is not a Tauri command input.
pub trait TaskCredentialBroker: Send + Sync {
    fn credential_for<'a>(
        &'a self,
        connection_id: &'a ProviderConnectionId,
    ) -> Pin<Box<dyn Future<Output = CoreResult<ConnectionBoundCredential>> + Send + 'a>>;
}
/// One terminal or interrupted worker result.
///
/// This Rust-only type is intended for the background supervisor. Raw provider
/// output, credentials, endpoint details, and queue payloads are excluded.
#[derive(Debug, Clone, PartialEq)]
pub struct MemoryJobExecutionResult {
    pub job: Revisioned<MemoryJob>,
    pub record: Option<Revisioned<MemoryRecord>>,
}
#[derive(Debug, Clone)]
struct PreparedClaimedMemorySummary {
    input: PreparedMemoryTaskInput,
    memory_profile: ObjectRevision<MemoryProfile>,
    task_profile: StoredRevision<TaskProfile>,
    embedding_task_profile: Option<ObjectRevision<TaskProfile>>,
    embedding_vector_space_sha256: Option<String>,
    provenance: MemoryRuntimeProvenance,
}
struct MemorySummaryProfileContext {
    memory_profile: ObjectRevision<MemoryProfile>,
    task_profile: ObjectRevision<TaskProfile>,
    embedding_task_profile: Option<ObjectRevision<TaskProfile>>,
    embedding_vector_space_sha256: Option<String>,
}
impl Core {
    /// Applies only user-editable memory content and state fields under one
    /// expected state revision. Identity, source range, kind, structured
    /// provenance, embedding linkage, and invalidation state are immutable.
    pub fn patch_memory_record_user_fields(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
        expected_revision: u64,
        patch: &MemoryRecordUserPatch,
    ) -> CoreResult<Revisioned<MemoryRecord>> {
        if patch.excluded_from_conversation.is_some() || patch.excluded_from_character.is_some() {
            return Err(CoreError::invalid(
                "memory exclusions must use the scope-specific exclusion API",
            ));
        }
        self.storage()
            .patch_memory_record_user_fields(
                conversation_id,
                branch_id,
                id,
                expected_revision,
                patch,
                Utc::now(),
            )
            .map(project_revision)
    }

    /// Changes exactly one room- or character-level exclusion flag.
    pub fn set_memory_record_exclusion(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
        expected_revision: u64,
        scope: MemoryRecordExclusionScope,
        excluded: bool,
    ) -> CoreResult<Revisioned<MemoryRecord>> {
        self.storage()
            .set_memory_record_exclusion(
                conversation_id,
                branch_id,
                id,
                expected_revision,
                (scope, excluded),
                Utc::now(),
            )
            .map(project_revision)
    }

    /// Claims and executes at most one memory job.
    ///
    /// This Rust-only entry point is for an application-state background
    /// supervisor. It must not be exposed as a Tauri command. The broker is
    /// consulted only immediately before a permitted provider attempt.
    pub async fn execute_next_memory_job(
        &self,
        credential_broker: &dyn TaskCredentialBroker,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> CoreResult<Option<MemoryJobExecutionResult>> {
        let Some(entry) = self.storage().claim_next_memory_job(Utc::now())? else {
            return Ok(None);
        };
        let expected_running_revision = entry.revision;
        if entry.job.kind == MemoryJobKind::Embedding {
            return self
                .execute_claimed_memory_embedding(
                    entry,
                    expected_running_revision,
                    credential_broker,
                    cancelled,
                )
                .await
                .map(Some);
        }
        self.execute_claimed_memory_summary_job(
            entry,
            expected_running_revision,
            credential_broker,
            cancelled,
        )
        .await
        .map(Some)
    }

    async fn execute_claimed_memory_summary_job(
        &self,
        entry: StoredMemoryJobQueueEntry,
        expected_running_revision: u64,
        credential_broker: &dyn TaskCredentialBroker,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> CoreResult<MemoryJobExecutionResult> {
        if entry.job.kind != MemoryJobKind::Summary {
            return self.finish_memory_job_execution(
                &entry,
                expected_running_revision,
                MemoryJobFinish::Failed {
                    error_code: "memory_job_kind_invalid".to_owned(),
                },
                Utc::now(),
            );
        }
        let Ok(prepared) = self.prepare_claimed_memory_summary(&entry) else {
            return self.finish_memory_job_execution(
                &entry,
                expected_running_revision,
                MemoryJobFinish::Failed {
                    error_code: "memory_input_invalid".to_owned(),
                },
                Utc::now(),
            );
        };
        let Ok(prompt) = BoundedTaskPrompt::new(
            memory_summary_system_instruction(&prepared.memory_profile.value.summary_schema),
            prepared.input.transformed_source.clone(),
        ) else {
            return self.finish_memory_job_execution(
                &entry,
                expected_running_revision,
                MemoryJobFinish::Failed {
                    error_code: "memory_prompt_invalid".to_owned(),
                },
                Utc::now(),
            );
        };
        self.dispatch_claimed_memory_summary(
            &entry,
            expected_running_revision,
            &prepared,
            prompt,
            credential_broker,
            cancelled,
        )
        .await
    }

    async fn dispatch_claimed_memory_summary(
        &self,
        entry: &StoredMemoryJobQueueEntry,
        expected_running_revision: u64,
        prepared: &PreparedClaimedMemorySummary,
        prompt: BoundedTaskPrompt,
        credential_broker: &dyn TaskCredentialBroker,
        cancelled: tokio::sync::watch::Receiver<bool>,
    ) -> CoreResult<MemoryJobExecutionResult> {
        let mut last_safe_failure = "task_before_dispatch";
        for target_revision in &prepared.provenance.task_targets {
            if *cancelled.borrow() {
                return self.finish_memory_job_execution(
                    entry,
                    expected_running_revision,
                    MemoryJobFinish::Cancelled,
                    Utc::now(),
                );
            }
            let Ok(resolved) = resolve_generation_target(self, &target_revision.target) else {
                last_safe_failure = "task_before_dispatch";
                continue;
            };
            if !self.memory_task_target_contract_is_current(target_revision) {
                last_safe_failure = "task_policy_changed";
                continue;
            }
            let Ok(credential) = credential_broker
                .credential_for(&resolved.connection_id)
                .await
            else {
                last_safe_failure = "task_credential_unavailable";
                continue;
            };
            let outcome = self
                .execute_task_profile_target(
                    &prepared.task_profile,
                    &target_revision.target,
                    resolved,
                    prompt.clone(),
                    credential,
                    cancelled.clone(),
                )
                .await;
            match outcome {
                TaskExecutionOutcome::Completed { canonical_text, .. } => {
                    return self.complete_claimed_memory_summary(
                        entry,
                        expected_running_revision,
                        prepared,
                        &canonical_text,
                    );
                }
                TaskExecutionOutcome::Failed {
                    classification: TaskDispatchClassification::BeforeDispatch,
                    ..
                } => {
                    last_safe_failure = "task_before_dispatch";
                }
                TaskExecutionOutcome::Failed {
                    classification: TaskDispatchClassification::KnownNoSideEffect,
                    error,
                } => {
                    if error.code == CoreErrorCode::Cancelled {
                        return self.finish_memory_job_execution(
                            entry,
                            expected_running_revision,
                            MemoryJobFinish::Cancelled,
                            Utc::now(),
                        );
                    }
                    last_safe_failure = "task_known_no_side_effect";
                }
                TaskExecutionOutcome::Failed {
                    classification: TaskDispatchClassification::UnknownOutcome,
                    ..
                } => {
                    let interrupted = self.storage().interrupt_memory_job(
                        &entry.job.id,
                        expected_running_revision,
                        Some("provider_unknown_outcome"),
                        Utc::now(),
                    )?;
                    return Ok(memory_execution_without_record(&interrupted));
                }
                TaskExecutionOutcome::Failed {
                    classification: TaskDispatchClassification::ProviderRejected,
                    ..
                } => {
                    return self.finish_memory_job_execution(
                        entry,
                        expected_running_revision,
                        MemoryJobFinish::Failed {
                            error_code: "provider_rejected_memory_task".to_owned(),
                        },
                        Utc::now(),
                    );
                }
            }
        }
        self.fail_memory_job_execution(entry, expected_running_revision, last_safe_failure)
    }

    fn memory_task_target_contract_is_current(
        &self,
        target_revision: &RuntimeTaskTargetRevision,
    ) -> bool {
        prompt_route_wire_contract(self, &target_revision.target)
            .ok()
            .and_then(|contract| task_target_contract_sha256(&contract).ok())
            .as_deref()
            == Some(target_revision.contract_sha256.as_str())
    }

    fn complete_claimed_memory_summary(
        &self,
        entry: &StoredMemoryJobQueueEntry,
        expected_running_revision: u64,
        prepared: &PreparedClaimedMemorySummary,
        canonical_text: &str,
    ) -> CoreResult<MemoryJobExecutionResult> {
        let finished_at = Utc::now();
        let Ok(record) = memory_record_from_provider_output(
            entry,
            &prepared.provenance,
            canonical_text,
            finished_at,
        ) else {
            return self.finish_memory_job_execution(
                entry,
                expected_running_revision,
                MemoryJobFinish::Failed {
                    error_code: "memory_output_invalid".to_owned(),
                },
                finished_at,
            );
        };
        let embedding_seed = prepared
            .embedding_task_profile
            .as_ref()
            .zip(prepared.embedding_vector_space_sha256.as_ref())
            .map(|(task_profile, vector_space_sha256)| {
                memory_embedding_job_seed(
                    entry,
                    &prepared.memory_profile,
                    task_profile,
                    vector_space_sha256,
                    finished_at,
                )
            })
            .transpose()?;
        let completed = self.storage().complete_memory_summary_job_with_embedding(
            &entry.job.id,
            expected_running_revision,
            &record,
            embedding_seed.as_ref(),
            finished_at,
        )?;
        Ok(MemoryJobExecutionResult {
            job: queue_entry_as_revisioned(&completed.job),
            record: Some(project_revision(completed.record)),
        })
    }

    fn finish_memory_job_execution(
        &self,
        entry: &StoredMemoryJobQueueEntry,
        expected_running_revision: u64,
        finish: MemoryJobFinish,
        finished_at: chrono::DateTime<Utc>,
    ) -> CoreResult<MemoryJobExecutionResult> {
        self.storage()
            .finish_memory_job(
                &entry.job.id,
                expected_running_revision,
                finish,
                finished_at,
            )
            .map(|finished| memory_execution_without_record(&finished))
    }

    fn fail_memory_job_execution(
        &self,
        entry: &StoredMemoryJobQueueEntry,
        expected_running_revision: u64,
        error_code: &str,
    ) -> CoreResult<MemoryJobExecutionResult> {
        self.finish_memory_job_execution(
            entry,
            expected_running_revision,
            MemoryJobFinish::Failed {
                error_code: error_code.to_owned(),
            },
            Utc::now(),
        )
    }
}

impl Core {
    fn memory_summary_profile_context(
        &self,
        entry: &StoredMemoryJobQueueEntry,
    ) -> CoreResult<MemorySummaryProfileContext> {
        if entry.job.kind != MemoryJobKind::Summary || entry.job.status != MemoryJobStatus::Running
        {
            return Err(CoreError::invalid(
                "memory summary worker requires one running summary job",
            ));
        }
        let memory_profile_revision = entry
            .memory_profile_revision
            .as_ref()
            .ok_or_else(|| CoreError::invalid("memory job lacks its exact memory profile"))?;
        let task_profile_revision = entry
            .task_profile_revision
            .as_ref()
            .ok_or_else(|| CoreError::invalid("memory job lacks its exact task profile"))?;
        let memory_profile_revision_id = entry
            .memory_profile_revision_id
            .as_deref()
            .ok_or_else(|| CoreError::invalid("memory job lacks a memory profile revision id"))?;
        let task_profile_revision_id = entry
            .task_profile_revision_id
            .as_deref()
            .ok_or_else(|| CoreError::invalid("memory job lacks a task profile revision id"))?;
        if memory_profile_revision.revision_id != memory_profile_revision_id
            || task_profile_revision.revision_id != task_profile_revision_id
            || task_profile_revision.value.kind != AuxiliaryTaskKind::MemorySummary
            || memory_profile_revision.value.summary_task != task_profile_revision.value.id
        {
            return Err(CoreError::invalid(
                "memory queue profile revisions are inconsistent",
            ));
        }
        memory_profile_revision
            .value
            .validate()
            .map_err(|error| CoreError::invalid(format!("invalid memory profile: {error}")))?;
        task_profile_revision
            .value
            .validate()
            .map_err(|error| CoreError::invalid(format!("invalid task profile: {error}")))?;
        let embedding_task_profile = self
            .storage()
            .get_memory_profile_embedding_task_revision(memory_profile_revision_id)?;
        match (
            memory_profile_revision.value.embedding_task.as_ref(),
            embedding_task_profile.as_ref(),
        ) {
            (None, None) => {}
            (Some(expected_id), Some(revision))
                if revision.value.id == *expected_id
                    && revision.value.kind == AuxiliaryTaskKind::MemoryEmbedding =>
            {
                revision.value.validate().map_err(|error| {
                    CoreError::invalid(format!("invalid memory embedding task profile: {error}"))
                })?;
            }
            _ => {
                return Err(CoreError::invalid(
                    "memory profile embedding task revision is inconsistent",
                ));
            }
        }
        let embedding_vector_space_sha256 = embedding_task_profile
            .as_ref()
            .map(|revision| {
                self.resolve_exact_embedding_task(
                    memory_profile_revision,
                    Some(&revision.revision_id),
                )
                .map(|resolved| resolved.provider.contract().vector_space_sha256())
            })
            .transpose()?;
        Ok(MemorySummaryProfileContext {
            memory_profile: memory_profile_revision.clone(),
            task_profile: task_profile_revision.clone(),
            embedding_task_profile,
            embedding_vector_space_sha256,
        })
    }

    fn prepare_claimed_memory_summary(
        &self,
        entry: &StoredMemoryJobQueueEntry,
    ) -> CoreResult<PreparedClaimedMemorySummary> {
        let profile_context = self.memory_summary_profile_context(entry)?;
        let memory_profile_revision = &profile_context.memory_profile;
        let task_profile_revision = &profile_context.task_profile;
        let memory_profile_revision_id = memory_profile_revision.revision_id.as_str();
        let task_profile_revision_id = task_profile_revision.revision_id.as_str();
        let provenance: MemoryRuntimeProvenance =
            serde_json::from_value(entry.payload.value.clone()).map_err(|error| {
                CoreError::invalid(format!("invalid memory runtime provenance: {error}"))
            })?;
        if entry.payload.schema_version != 1
            || provenance.memory_profile_id != memory_profile_revision.value.id
            || provenance.memory_profile_revision_id != memory_profile_revision.revision_id
            || provenance.task_profile_id != task_profile_revision.value.id
            || provenance.task_profile_revision_id != task_profile_revision.revision_id
        {
            return Err(CoreError::invalid(
                "memory runtime provenance does not match its immutable profiles",
            ));
        }

        let current_targets =
            self.resolve_task_generation_targets(&task_profile_revision.value.id)?;
        if current_targets.targets.len() != provenance.task_targets.len()
            || !current_targets
                .targets
                .iter()
                .zip(&provenance.task_targets)
                .all(|(current, stored)| current == &stored.target)
        {
            return Err(CoreError::invalid(
                "memory task target policy changed after enqueue",
            ));
        }
        for target in &provenance.task_targets {
            let contract = prompt_route_wire_contract(self, &target.target)?;
            if task_target_contract_sha256(&contract)? != target.contract_sha256 {
                return Err(CoreError::invalid(
                    "memory task provider contract changed after enqueue",
                ));
            }
        }

        let source_messages = self.load_memory_job_source(entry)?;
        let source_sha256 = memory_source_sha256(
            &source_messages,
            memory_profile_revision_id,
            task_profile_revision_id,
        )?;
        if source_sha256 != provenance.source_sha256 {
            return Err(CoreError::invalid(
                "memory source changed after the job was enqueued",
            ));
        }
        let policy =
            self.resolve_runtime_prompt_policy(&entry.job.conversation_id, &entry.job.branch_id)?;
        if policy.preset.id != provenance.prompt_preset_id
            || policy.preset_revision_id != provenance.prompt_preset_revision_id
            || policy.module_plan_sha256 != provenance.module_plan_sha256
            || policy.preset.memory_profile_id.as_ref() != Some(&provenance.memory_profile_id)
            || policy.transform_revisions != provenance.transform_sets
            || versioned_sha256(&policy.variables)? != provenance.variables_sha256
        {
            return Err(CoreError::invalid(
                "memory orchestration policy changed after enqueue",
            ));
        }
        let capabilities =
            self.supported_capabilities_for_route(&task_profile_revision.value.route_id)?;
        if capabilities != provenance.supported_capabilities {
            return Err(CoreError::invalid(
                "memory task capabilities changed after enqueue",
            ));
        }
        let source_text = render_memory_source(&source_messages)?;
        let transform_result =
            Self::apply_memory_input_transforms(&policy, &capabilities, &source_text)?;
        if versioned_sha256(&transform_result.reports)? != provenance.transform_trace_sha256 {
            return Err(CoreError::invalid(
                "memory transform result changed after enqueue",
            ));
        }
        let task_profile = object_revision_as_stored(task_profile_revision);
        Ok(PreparedClaimedMemorySummary {
            input: PreparedMemoryTaskInput {
                job: queue_entry_as_stored_revision(entry),
                source_messages,
                transformed_source: transform_result.output.clone(),
                transform_results: vec![transform_result],
                source_sha256,
                task_profile_id: task_profile_revision.value.id.clone(),
                task_profile_revision_id: task_profile_revision.revision_id.clone(),
            },
            memory_profile: memory_profile_revision.clone(),
            task_profile,
            embedding_task_profile: profile_context.embedding_task_profile,
            embedding_vector_space_sha256: profile_context.embedding_vector_space_sha256,
            provenance,
        })
    }
}

fn queue_entry_as_stored_revision(entry: &StoredMemoryJobQueueEntry) -> StoredRevision<MemoryJob> {
    StoredRevision {
        value: entry.job.clone(),
        revision: entry.revision,
        revision_id: None,
        created_at: entry.job.created_at,
        updated_at: entry.job.updated_at,
        deleted_at: None,
    }
}
pub(super) fn queue_entry_as_revisioned(
    entry: &StoredMemoryJobQueueEntry,
) -> Revisioned<MemoryJob> {
    project_revision(queue_entry_as_stored_revision(entry))
}
fn object_revision_as_stored<T: Clone>(revision: &ObjectRevision<T>) -> StoredRevision<T> {
    StoredRevision {
        value: revision.value.clone(),
        revision: revision.revision,
        revision_id: Some(revision.revision_id.clone()),
        created_at: revision.created_at,
        updated_at: revision.created_at,
        deleted_at: None,
    }
}
pub(super) fn memory_execution_without_record(
    entry: &StoredMemoryJobQueueEntry,
) -> MemoryJobExecutionResult {
    MemoryJobExecutionResult {
        job: queue_entry_as_revisioned(entry),
        record: None,
    }
}
pub(super) fn claimed_memory_job(
    entry: &StoredMemoryJobQueueEntry,
) -> CoreResult<ClaimedMemoryJob> {
    let memory_profile_revision_id = entry
        .memory_profile_revision_id
        .clone()
        .ok_or_else(|| CoreError::invalid("memory job has no memory profile revision"))?;
    let task_profile_revision_id = entry
        .task_profile_revision_id
        .clone()
        .ok_or_else(|| CoreError::invalid("memory job has no task profile revision"))?;
    Ok(ClaimedMemoryJob {
        job: queue_entry_as_revisioned(entry),
        memory_profile_revision_id,
        task_profile_revision_id,
    })
}
fn task_target_contract_sha256(contract: &PromptRouteWireContract) -> CoreResult<String> {
    versioned_digest(&(
        "lorepia.task-target-contract.v1",
        &contract.model_route_id,
        &contract.generation_preset_id,
        &contract.model,
        contract.api_family,
        contract.developer_capability,
        contract.cache_dialect,
        &contract.request_plan_sha256,
        &contract.generation_preset_sha256,
        contract.configured_max_output_tokens,
        contract.context_limit_tokens,
        contract.observed_max_output_tokens,
        contract.supports_temperature,
        contract.reasoning_effort_applied,
    ))
}
fn memory_record_from_provider_output(
    entry: &StoredMemoryJobQueueEntry,
    provenance: &MemoryRuntimeProvenance,
    canonical_text: &str,
    completed_at: chrono::DateTime<Utc>,
) -> CoreResult<MemoryRecord> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct SummaryOutput {
        title: String,
        summary: String,
        structured_data: serde_json::Value,
        importance: u8,
        keywords: Vec<String>,
    }

    let output: SummaryOutput = serde_json::from_str(canonical_text)
        .map_err(|_| CoreError::invalid("memory provider output is not the strict summary JSON"))?;
    if !output.structured_data.is_object() {
        return Err(CoreError::invalid(
            "memory provider structured_data must be a JSON object",
        ));
    }
    let mut normalized_keywords = BTreeSet::new();
    for keyword in &output.keywords {
        let normalized = keyword.trim().to_lowercase();
        if normalized.is_empty() || !normalized_keywords.insert(normalized) {
            return Err(CoreError::invalid(
                "memory provider keywords must be unique and non-empty",
            ));
        }
    }
    let record_digest = versioned_digest(&(
        "lorepia.memory-record.v1",
        &entry.job.id,
        &entry.input_fingerprint_sha256,
    ))?;
    let output_sha256 = format!("{:x}", Sha256::digest(canonical_text.as_bytes()));
    let record = MemoryRecord {
        id: MemoryRecordId::from(format!("memory-record-{record_digest}")),
        conversation_id: entry.job.conversation_id.clone(),
        branch_id: entry.job.branch_id.clone(),
        source_start_message_id: entry.job.source_start_message_id.clone(),
        source_end_message_id: entry.job.source_end_message_id.clone(),
        kind: MemoryKind::ConversationSummary,
        title: output.title,
        summary: output.summary,
        structured_data: VersionedJson {
            schema_version: 1,
            value: output.structured_data,
        },
        importance: output.importance,
        keywords: output.keywords,
        embedding_ref: None,
        pinned: false,
        excluded_from_conversation: false,
        excluded_from_character: false,
        created_at: completed_at,
        updated_at: completed_at,
        invalidated_at: None,
        provenance: Provenance {
            source_kind: SourceKind::Generated,
            source_id: Some(entry.job.id.as_str().to_owned()),
            source_hash: Some(output_sha256),
            author: Some("LorePia memory runtime".to_owned()),
            license: None,
            imported_at: None,
        },
    };
    if provenance.source_sha256.is_empty() {
        return Err(CoreError::invalid(
            "memory runtime provenance has no source digest",
        ));
    }
    record
        .validate()
        .map_err(|error| CoreError::invalid(format!("invalid generated memory record: {error}")))?;
    Ok(record)
}
pub(super) fn memory_job_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!("memory job input is invalid: {error}"))
}
