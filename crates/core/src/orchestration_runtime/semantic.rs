use std::collections::BTreeSet;

use chrono::Utc;
use lorepia_domain::{
    ApiFamily, ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult,
    MemoryProfile, MemoryRecord, MessageId, ModelRouteId, ValidateOrchestration,
};
use lorepia_orchestration::{KnowledgeWorkBudget, MemorySemanticScore};
use lorepia_providers::{EmbeddingPurpose, MAX_EMBEDDING_INPUT_BYTES, MAX_EMBEDDING_INPUT_CHARS};
use lorepia_storage::{
    KnowledgeEmbeddingCoverageQuery, MemoryEmbeddingQuery, MemoryQueryEmbeddingStatus,
    ObjectRevision,
};
use serde::{Deserialize, Serialize};

use super::{
    auxiliary_tasks::TaskCredentialBroker,
    embedding::{
        EmbeddingDispatchOutcome, embedding_failure_code, memory_embedding_candidate_limit,
        memory_query_embedding_intent,
    },
    versioned_digest,
};
use crate::{
    Core,
    orchestration::{
        KnowledgeSemanticProviderRequirement, charge_provider_knowledge_work,
        semantic_score_from_millionths,
    },
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum MemorySemanticQueryEvidence {
    LexicalV1 {
        memory_profile_revision_id: String,
        query_sha256: String,
        scores_sha256: String,
    },
    ProviderEmbeddingV1 {
        memory_profile_revision_id: String,
        task_profile_revision_id: String,
        model_route_id: ModelRouteId,
        api_family: ApiFamily,
        model_id: String,
        dimensions: u32,
        vector_space_sha256: String,
        contract_sha256: String,
        query_sha256: String,
        query_embedding_id: Option<String>,
        query_embedding_revision: Option<u64>,
        query_vector_sha256: Option<String>,
        matches_sha256: String,
    },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolvedMemorySemanticQuery {
    pub scores: Vec<MemorySemanticScore>,
    pub evidence: MemorySemanticQueryEvidence,
    /// Provider vector retained only inside Core so exact knowledge-entry
    /// vectors can reuse the same durable query intent. It is never serialized
    /// into prompt diagnostics or exposed over IPC.
    #[serde(skip)]
    pub provider_query_values: Option<Vec<f32>>,
}
impl Core {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(crate) async fn resolve_memory_semantic_scores(
        &self,
        exact_profile: &ObjectRevision<MemoryProfile>,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        source_start_message_id: &MessageId,
        source_end_message_id: &MessageId,
        records: &[MemoryRecord],
        query_texts: &[String],
        semantic_requirements: &[KnowledgeSemanticProviderRequirement],
        credential_broker: &dyn TaskCredentialBroker,
        cancelled: tokio::sync::watch::Receiver<bool>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<ResolvedMemorySemanticQuery> {
        exact_profile.value.validate().map_err(|error| {
            CoreError::invalid(format!("invalid exact memory profile: {error}"))
        })?;
        if records.iter().any(|record| {
            record.conversation_id != *conversation_id
                || record.invalidated_at.is_some()
                || record.excluded_from_conversation
                || record.excluded_from_character
        }) {
            return Err(CoreError::invalid(
                "memory semantic candidates are outside the active retrieval scope",
            ));
        }
        let query = render_memory_embedding_query(query_texts)?;
        let query_sha256 = versioned_digest(&("lorepia.memory-query.v1", &query))?;
        if exact_profile.value.embedding_task.is_none() {
            let scores = lexical_memory_semantic_scores_runtime(records, query_texts);
            let scores_sha256 = semantic_scores_sha256(&scores)?;
            return Ok(ResolvedMemorySemanticQuery {
                scores,
                evidence: MemorySemanticQueryEvidence::LexicalV1 {
                    memory_profile_revision_id: exact_profile.revision_id.clone(),
                    query_sha256,
                    scores_sha256,
                },
                provider_query_values: None,
            });
        }

        let resolved = self.resolve_exact_embedding_task(exact_profile, None)?;
        let contract = resolved.provider.contract();
        let task_profile_revision_id = resolved.task_profile.revision_id.clone();
        let model_route_id = contract.model_route_id().clone();
        let api_family = contract.api_family();
        let model_id = contract.model_id().to_owned();
        let dimensions = contract.dimensions();
        let vector_space_sha256 = contract.vector_space_sha256();
        let contract_sha256 = contract.execution_sha256(EmbeddingPurpose::RetrievalQuery);
        let provider_query_needed = if records.is_empty() {
            let mut complete_provider_book_exists = false;
            for requirement in semantic_requirements {
                let coverage_clone_work = requirement.entry_ids.iter().fold(
                    requirement
                        .book_revision_id
                        .len()
                        .saturating_add(task_profile_revision_id.len())
                        .saturating_add(model_route_id.as_str().len())
                        .saturating_add(vector_space_sha256.len()),
                    |total, entry_id| total.saturating_add(entry_id.as_str().len()),
                );
                charge_provider_knowledge_work(
                    &requirement.book_revision_id,
                    knowledge_work_budget,
                    coverage_clone_work,
                )?;
                let coverage = self
                    .storage()
                    .knowledge_embedding_space_covers_entries_bounded(
                        &KnowledgeEmbeddingCoverageQuery {
                            book_revision_id: requirement.book_revision_id.clone(),
                            task_profile_revision_id: task_profile_revision_id.clone(),
                            model_route_id: model_route_id.clone(),
                            dimensions,
                            vector_space_sha256: vector_space_sha256.clone(),
                            required_entry_ids: requirement.entry_ids.clone(),
                        },
                        knowledge_work_budget.remaining_work_bytes(),
                    )?;
                charge_provider_knowledge_work(
                    &requirement.book_revision_id,
                    knowledge_work_budget,
                    coverage.work_bytes,
                )?;
                if coverage.covered {
                    complete_provider_book_exists = true;
                    break;
                }
            }
            complete_provider_book_exists
        } else {
            true
        };
        if !provider_query_needed {
            return Ok(ResolvedMemorySemanticQuery {
                scores: Vec::new(),
                evidence: MemorySemanticQueryEvidence::ProviderEmbeddingV1 {
                    memory_profile_revision_id: exact_profile.revision_id.clone(),
                    task_profile_revision_id,
                    model_route_id,
                    api_family,
                    model_id,
                    dimensions,
                    vector_space_sha256,
                    contract_sha256,
                    query_sha256,
                    query_embedding_id: None,
                    query_embedding_revision: None,
                    query_vector_sha256: None,
                    matches_sha256: versioned_digest(&(
                        "lorepia.memory-embedding-matches.v1",
                        Vec::<String>::new(),
                    ))?,
                },
                provider_query_values: None,
            });
        }

        let intent = memory_query_embedding_intent(
            exact_profile,
            &resolved.task_profile,
            conversation_id,
            branch_id,
            source_start_message_id,
            source_end_message_id,
            &query_sha256,
            &vector_space_sha256,
            &model_route_id,
            dimensions,
            Utc::now(),
        )?;
        let enqueued = self.storage().enqueue_memory_query_embedding(&intent)?;
        let stored = match enqueued.entry.status {
            MemoryQueryEmbeddingStatus::Succeeded => enqueued.entry,
            MemoryQueryEmbeddingStatus::Interrupted => {
                return Err(CoreError::new(
                    CoreErrorCode::ProviderUnavailable,
                    "memory query embedding has an unknown prior provider outcome; explicit retry is required",
                    false,
                ));
            }
            MemoryQueryEmbeddingStatus::Running => {
                return Err(CoreError::new(
                    CoreErrorCode::ProviderUnavailable,
                    "memory query embedding is already running and was not dispatched again",
                    true,
                ));
            }
            MemoryQueryEmbeddingStatus::Failed => {
                return Err(CoreError::new(
                    CoreErrorCode::ProviderUnavailable,
                    "memory query embedding previously failed and was not retried",
                    false,
                ));
            }
            MemoryQueryEmbeddingStatus::Cancelled => {
                return Err(CoreError::new(
                    CoreErrorCode::Cancelled,
                    "memory query embedding was previously cancelled and was not retried",
                    false,
                ));
            }
            MemoryQueryEmbeddingStatus::Queued => {
                let running = self.storage().claim_memory_query_embedding(
                    &intent.id,
                    enqueued.entry.revision,
                    Utc::now(),
                )?;
                let running_revision = running.revision;
                let dispatch_resolved = match self.resolve_exact_embedding_task(
                    exact_profile,
                    Some(&resolved.task_profile.revision_id),
                ) {
                    Ok(current)
                        if current.provider.contract().vector_space_sha256()
                            == vector_space_sha256 =>
                    {
                        current
                    }
                    Ok(_) => {
                        self.storage().fail_memory_query_embedding(
                            &intent.id,
                            running_revision,
                            "embedding_vector_space_changed",
                            Utc::now(),
                        )?;
                        return Err(CoreError::new(
                            CoreErrorCode::ProviderUnavailable,
                            "memory query embedding provider vector space changed before dispatch",
                            false,
                        ));
                    }
                    Err(error) => {
                        self.storage().fail_memory_query_embedding(
                            &intent.id,
                            running_revision,
                            "embedding_provider_unavailable",
                            Utc::now(),
                        )?;
                        return Err(error);
                    }
                };
                match self
                    .dispatch_embedding(
                        &dispatch_resolved,
                        query,
                        EmbeddingPurpose::RetrievalQuery,
                        credential_broker,
                        cancelled,
                    )
                    .await
                {
                    EmbeddingDispatchOutcome::Completed(values) => {
                        self.storage().complete_memory_query_embedding(
                            &intent.id,
                            running_revision,
                            &values,
                            Utc::now(),
                        )?
                    }
                    EmbeddingDispatchOutcome::Failed(error) => {
                        self.storage().fail_memory_query_embedding(
                            &intent.id,
                            running_revision,
                            embedding_failure_code(&error),
                            Utc::now(),
                        )?;
                        return Err(error);
                    }
                    EmbeddingDispatchOutcome::CancelledBeforeDispatch => {
                        self.storage().cancel_memory_query_embedding(
                            &intent.id,
                            running_revision,
                            Utc::now(),
                        )?;
                        return Err(CoreError::new(
                            CoreErrorCode::Cancelled,
                            "memory embedding query was cancelled before provider dispatch",
                            true,
                        ));
                    }
                    EmbeddingDispatchOutcome::UnknownOutcome => {
                        self.storage().interrupt_memory_query_embedding(
                            &intent.id,
                            running_revision,
                            "provider_unknown_outcome",
                            Utc::now(),
                        )?;
                        return Err(CoreError::new(
                            CoreErrorCode::ProviderUnavailable,
                            "memory embedding query outcome is unknown; explicit retry is required",
                            false,
                        ));
                    }
                }
            }
        };
        let values = stored.values.ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "completed memory query embedding has no vector",
                false,
            )
        })?;
        let query_vector_sha256 = stored.vector_sha256.ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "completed memory query embedding has no vector digest",
                false,
            )
        })?;
        let query_embedding_id = stored.intent.id.clone();
        let query_embedding_revision = stored.revision;
        let matches = if records.is_empty() {
            Vec::new()
        } else {
            let candidate_limit =
                u32::try_from(memory_embedding_candidate_limit(records.len(), dimensions)?)
                    .map_err(|_| {
                        CoreError::internal("memory embedding candidate limit overflowed")
                    })?;
            self.storage()
                .query_memory_embeddings_cosine(&MemoryEmbeddingQuery {
                    conversation_id: conversation_id.clone(),
                    branch_id: branch_id.clone(),
                    context_head_message_id: source_end_message_id.clone(),
                    task_profile_revision_id: resolved.task_profile.revision_id.clone(),
                    model_route_id: resolved.task_profile.value.route_id.clone(),
                    dimensions,
                    vector_space_sha256: vector_space_sha256.clone(),
                    values: values.clone(),
                    candidate_limit,
                    result_limit: candidate_limit,
                })?
        };
        let allowed_records = records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<BTreeSet<_>>();
        let scores = matches
            .iter()
            .filter(|candidate| allowed_records.contains(candidate.memory_record_id.as_str()))
            .map(|candidate| {
                Ok(MemorySemanticScore {
                    record_id: candidate.memory_record_id.clone(),
                    score: semantic_score_from_millionths(candidate.similarity_millionths)?,
                })
            })
            .collect::<CoreResult<Vec<_>>>()?;
        let matches_sha256 = versioned_digest(&("lorepia.memory-embedding-matches.v1", &matches))?;
        Ok(ResolvedMemorySemanticQuery {
            scores,
            evidence: MemorySemanticQueryEvidence::ProviderEmbeddingV1 {
                memory_profile_revision_id: exact_profile.revision_id.clone(),
                task_profile_revision_id,
                model_route_id,
                api_family,
                model_id,
                dimensions,
                vector_space_sha256,
                contract_sha256,
                query_sha256,
                query_embedding_id: Some(query_embedding_id),
                query_embedding_revision: Some(query_embedding_revision),
                query_vector_sha256: Some(query_vector_sha256),
                matches_sha256,
            },
            provider_query_values: Some(values),
        })
    }
}

fn render_memory_embedding_query(query_texts: &[String]) -> CoreResult<String> {
    const MAX_QUERY_TEXTS: usize = 32;
    const MAX_LEXICAL_QUERY_BYTES: usize = 65_536;

    if query_texts.is_empty() || query_texts.len() > MAX_QUERY_TEXTS {
        return Err(CoreError::invalid(
            "memory embedding query must contain between 1 and 32 texts",
        ));
    }
    let lexical_bytes = query_texts
        .iter()
        .try_fold(0_usize, |total, text| total.checked_add(text.len()));
    if lexical_bytes.is_none_or(|total| total > MAX_LEXICAL_QUERY_BYTES) {
        return Err(CoreError::invalid(
            "memory embedding query exceeds the retrieval safety limit",
        ));
    }
    let rendered = query_texts
        .iter()
        .filter(|text| !text.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");
    if rendered.is_empty()
        || rendered.len() > MAX_EMBEDDING_INPUT_BYTES
        || rendered.chars().count() > MAX_EMBEDDING_INPUT_CHARS
    {
        return Err(CoreError::invalid(
            "memory embedding query exceeds the exact provider input limit",
        ));
    }
    Ok(rendered)
}
fn lexical_memory_semantic_scores_runtime(
    records: &[MemoryRecord],
    query_texts: &[String],
) -> Vec<MemorySemanticScore> {
    const MAX_QUERY_MESSAGES: usize = 32;
    const MAX_QUERY_CHARS: usize = 65_536;

    let query_chars = query_texts
        .iter()
        .take(MAX_QUERY_MESSAGES)
        .flat_map(|text| text.chars())
        .take(MAX_QUERY_CHARS)
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect::<BTreeSet<_>>();
    records
        .iter()
        .map(|record| {
            let candidate_chars = record
                .title
                .chars()
                .chain(record.summary.chars())
                .flat_map(char::to_lowercase)
                .filter(|character| character.is_alphanumeric())
                .collect::<BTreeSet<_>>();
            let union = query_chars.union(&candidate_chars).count();
            let intersection = query_chars.intersection(&candidate_chars).count();
            MemorySemanticScore {
                record_id: record.id.clone(),
                score: if union == 0 {
                    0.0
                } else {
                    usize_as_f32(intersection) / usize_as_f32(union)
                },
            }
        })
        .collect()
}
fn semantic_scores_sha256(scores: &[MemorySemanticScore]) -> CoreResult<String> {
    let canonical = scores
        .iter()
        .map(|score| {
            if !score.score.is_finite() || !(0.0..=1.0).contains(&score.score) {
                return Err(CoreError::internal(
                    "memory semantic score is outside the canonical domain",
                ));
            }
            Ok((
                score.record_id.as_str(),
                semantic_score_millionths(score.score)?,
            ))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    versioned_digest(&("lorepia.memory-semantic-scores.v1", canonical))
}
fn usize_as_f32(mut value: usize) -> f32 {
    let mut result = 0.0_f32;
    let mut place = 1.0_f32;
    while value != 0 {
        let chunk = u16::try_from(value & 0xffff).unwrap_or(u16::MAX);
        result += f32::from(chunk) * place;
        value >>= 16;
        place *= 65_536.0;
    }
    result
}
fn semantic_score_millionths(score: f32) -> CoreResult<u32> {
    format!("{:.0}", (score * 1_000_000.0).round())
        .parse::<u32>()
        .map_err(|_| CoreError::internal("memory semantic score could not be quantized"))
}
