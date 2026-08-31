use super::{
    activation_rule_uses_semantic, charge_provider_knowledge_work,
    knowledge_embedding_matches_sha256, knowledge_semantic_query_sha256,
    knowledge_semantic_scores_sha256, lexical_knowledge_semantic_scores_with_budget,
    orchestration_validation_error, semantic_score_from_millionths,
};
use crate::{
    Core, Revisioned,
    orchestration_runtime::{MemorySemanticQueryEvidence, ResolvedMemorySemanticQuery},
    revision::{project_revision, project_revisions},
};
use chrono::{DateTime, Utc};
use lorepia_domain::{
    CapabilityKey, CharacterContentV1, ConversationBranchId, ConversationId, CoreError, CoreResult,
    KnowledgeBook, KnowledgeBookId, KnowledgeEntryId, PromptPreset, SelectedKnowledge,
    SemanticKnowledgeScore, ValidateOrchestration, VariableMap,
};
use lorepia_orchestration::{
    KnowledgeEngine, KnowledgeSelection, KnowledgeSelectionContext, KnowledgeWorkBudget,
};
use lorepia_storage::{
    KnowledgeActivationLog, KnowledgeEmbeddingQuery, ObjectRevision, StoredRevision,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeTokenEstimate {
    pub entry_id: KnowledgeEntryId,
    pub tokens: u32,
}

/// Owned creator-tool input for deterministic knowledge activation simulation.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KnowledgeSimulationRequest {
    pub book_id: KnowledgeBookId,
    pub sample_texts: Vec<String>,
    pub manual_entry_ids: Vec<KnowledgeEntryId>,
    pub semantic_scores: Vec<SemanticKnowledgeScore>,
    pub variables: VariableMap,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub token_estimates: Vec<KnowledgeTokenEstimate>,
    pub activation_seed: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct KnowledgeSemanticBookEvidence {
    pub book_id: KnowledgeBookId,
    pub book_revision_id: String,
    pub source: KnowledgeSemanticScoreSourceEvidence,
    pub semantic_entry_count: u32,
    pub scores_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum KnowledgeSemanticScoreSourceEvidence {
    LexicalV1 {
        query_sha256: String,
    },
    ProviderEmbeddingV1 {
        memory_profile_revision_id: String,
        task_profile_revision_id: String,
        model_route_id: lorepia_domain::ModelRouteId,
        dimensions: u32,
        vector_space_sha256: String,
        query_sha256: String,
        query_embedding_id: String,
        query_embedding_revision: u64,
        query_vector_sha256: String,
        matches_sha256: String,
    },
}

pub(crate) struct KnowledgeSemanticProviderRequirement {
    pub book_revision_id: String,
    pub entry_ids: Vec<KnowledgeEntryId>,
}

impl Core {
    pub fn upsert_knowledge_book(
        &self,
        book: &KnowledgeBook,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<KnowledgeBook>> {
        book.validate().map_err(orchestration_validation_error)?;
        self.storage()
            .save_knowledge_book(book, expected_revision)
            .map(project_revision)
    }

    pub fn get_knowledge_book(
        &self,
        id: &KnowledgeBookId,
    ) -> CoreResult<Revisioned<KnowledgeBook>> {
        self.storage().get_knowledge_book(id).map(project_revision)
    }

    pub fn list_knowledge_books(&self) -> CoreResult<Vec<Revisioned<KnowledgeBook>>> {
        self.storage().list_knowledge_books().map(project_revisions)
    }

    pub fn delete_knowledge_book(
        &self,
        id: &KnowledgeBookId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<KnowledgeBook>> {
        self.storage()
            .soft_delete_knowledge_book(id, expected_revision)
            .map(project_revision)
    }

    pub fn simulate_knowledge_activation(
        &self,
        request: &KnowledgeSimulationRequest,
    ) -> CoreResult<KnowledgeSelection> {
        let book = self.get_knowledge_book(&request.book_id)?.value;
        let manual_entry_ids = request
            .manual_entry_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        let mut token_estimates = std::collections::BTreeMap::new();
        for estimate in &request.token_estimates {
            if token_estimates
                .insert(estimate.entry_id.clone(), estimate.tokens)
                .is_some()
            {
                return Err(CoreError::invalid(
                    "knowledge token estimates contain a duplicate entry",
                ));
            }
        }
        KnowledgeEngine::select(
            &book,
            &KnowledgeSelectionContext {
                scan_texts: &request.sample_texts,
                manual_entry_ids: &manual_entry_ids,
                semantic_scores: &request.semantic_scores,
                variables: &request.variables,
                supported_capabilities: &request.supported_capabilities,
                token_estimates: &token_estimates,
                activation_seed: request.activation_seed,
            },
        )
        .map_err(orchestration_validation_error)
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub(super) fn select_prompt_knowledge(
        &self,
        preset: &PromptPreset,
        character_content: &CharacterContentV1,
        exact_prompt_books: &[ObjectRevision<KnowledgeBook>],
        exact_module_books: &[ObjectRevision<KnowledgeBook>],
        exact_character_book: Option<&StoredRevision<KnowledgeBook>>,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        scan_texts: &[String],
        manual_entry_ids: &BTreeSet<KnowledgeEntryId>,
        variables: &VariableMap,
        supported_capabilities: &[CapabilityKey],
        resolved_semantics: Option<&ResolvedMemorySemanticQuery>,
        activation_seed: u64,
        selected_at: DateTime<Utc>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<(
        Vec<SelectedKnowledge>,
        Vec<KnowledgeActivationLog>,
        Vec<KnowledgeSemanticBookEvidence>,
    )> {
        let mut book_ids = preset
            .knowledge_book_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if let Some(id) = character_content
            .knowledge_book
            .as_ref()
            .and_then(|reference| reference.id.as_ref())
        {
            book_ids.insert(id.clone());
        }
        let prompt_books = exact_prompt_books
            .iter()
            .map(|revision| (revision.value.id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        let module_books = exact_module_books
            .iter()
            .map(|revision| (revision.value.id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        if prompt_books
            .keys()
            .any(|book_id| module_books.contains_key(book_id))
        {
            return Err(CoreError::invalid(
                "prompt preset and approved module select the same knowledge book",
            ));
        }
        book_ids.extend(module_books.keys().cloned());
        let token_estimates = std::collections::BTreeMap::new();
        let mut selected_all = Vec::new();
        let mut logs = Vec::new();
        let mut semantic_evidence = Vec::new();
        for book_id in book_ids {
            let (book, book_revision_id) = if let Some(revision) = module_books.get(&book_id) {
                (revision.value.clone(), revision.revision_id.clone())
            } else if let Some(revision) = prompt_books.get(&book_id) {
                (revision.value.clone(), revision.revision_id.clone())
            } else if let Some(revision) =
                exact_character_book.filter(|revision| revision.value.id == book_id)
            {
                let revision_id = revision.revision_id.clone().ok_or_else(|| {
                    CoreError::new(
                        lorepia_domain::CoreErrorCode::StorageCorrupted,
                        "sealed character knowledge book is missing its exact revision",
                        false,
                    )
                })?;
                (revision.value.clone(), revision_id)
            } else {
                let stored_book = self.get_knowledge_book(&book_id)?;
                let revision_id = stored_book.revision_id.ok_or_else(|| {
                    CoreError::internal("knowledge book is missing its immutable revision identity")
                })?;
                (stored_book.value, revision_id)
            };
            let semantic_entry_count = book
                .entries
                .iter()
                .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
                .count();
            let (semantic_scores, semantic_source) = if semantic_entry_count > 0 {
                self.resolve_prompt_knowledge_semantic_scores(
                    &book,
                    &book_revision_id,
                    scan_texts,
                    resolved_semantics,
                    knowledge_work_budget,
                )?
            } else {
                (Vec::new(), None)
            };
            if let Some(source) = semantic_source {
                semantic_evidence.push(KnowledgeSemanticBookEvidence {
                    book_id: book.id.clone(),
                    book_revision_id: book_revision_id.clone(),
                    source,
                    semantic_entry_count: u32::try_from(semantic_entry_count).map_err(|_| {
                        CoreError::internal("knowledge semantic entry count overflowed")
                    })?,
                    scores_sha256: knowledge_semantic_scores_sha256(
                        &book_revision_id,
                        &semantic_scores,
                        book.id.as_str(),
                        knowledge_work_budget,
                    )?,
                });
            }
            let selection = KnowledgeEngine::select_with_budget(
                &book,
                &KnowledgeSelectionContext {
                    scan_texts,
                    manual_entry_ids,
                    semantic_scores: &semantic_scores,
                    variables,
                    supported_capabilities,
                    token_estimates: &token_estimates,
                    activation_seed,
                },
                knowledge_work_budget,
            )
            .map_err(orchestration_validation_error)?;
            for selected in selection.selected {
                let entry = book
                    .entries
                    .iter()
                    .find(|entry| entry.id == selected.entry_id)
                    .ok_or_else(|| CoreError::internal("selected knowledge entry disappeared"))?;
                selected_all.push(SelectedKnowledge {
                    entry_id: entry.id.clone(),
                    content: selected.content,
                    placement: selected.placement,
                    priority: entry.priority,
                    evidence: selected.reasons,
                    provenance: entry.provenance.clone(),
                });
            }
            for evidence in selection.evidence {
                let identity = format!(
                    "lorepia:knowledge-log:v1\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
                    conversation_id.0,
                    branch_id.0,
                    book.id.as_str(),
                    evidence.entry_id.as_str()
                );
                logs.push(KnowledgeActivationLog {
                    id: Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string(),
                    book_id: book.id.clone(),
                    book_revision_id: book_revision_id.clone(),
                    entry_id: evidence.entry_id,
                    conversation_id: conversation_id.clone(),
                    branch_id: branch_id.clone(),
                    selected: evidence.selected,
                    reasons: evidence.reasons,
                    estimated_tokens: evidence.estimated_tokens,
                    exclusion_reason: evidence.exclusion_reason,
                    created_at: selected_at,
                });
            }
        }
        selected_all.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.entry_id.cmp(&right.entry_id))
        });
        Ok((selected_all, logs, semantic_evidence))
    }

    #[allow(clippy::too_many_lines)]
    fn resolve_prompt_knowledge_semantic_scores(
        &self,
        book: &KnowledgeBook,
        book_revision_id: &str,
        scan_texts: &[String],
        resolved_semantics: Option<&ResolvedMemorySemanticQuery>,
        work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<(
        Vec<SemanticKnowledgeScore>,
        Option<KnowledgeSemanticScoreSourceEvidence>,
    )> {
        if let Some(resolved) = resolved_semantics
            && let Some(values) = resolved.provider_query_values.as_ref()
        {
            let MemorySemanticQueryEvidence::ProviderEmbeddingV1 {
                memory_profile_revision_id,
                task_profile_revision_id,
                model_route_id,
                dimensions,
                vector_space_sha256,
                query_sha256,
                query_embedding_id,
                query_embedding_revision,
                query_vector_sha256,
                ..
            } = &resolved.evidence
            else {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "provider knowledge query vector has no provider evidence",
                    false,
                ));
            };
            let (
                Some(query_embedding_id),
                Some(query_embedding_revision),
                Some(query_vector_sha256),
            ) = (
                query_embedding_id.as_ref(),
                *query_embedding_revision,
                query_vector_sha256.as_ref(),
            )
            else {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "provider knowledge query vector is missing its durable identity",
                    false,
                ));
            };
            let required_clone_work = book
                .entries
                .iter()
                .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
                .fold(0_usize, |total, entry| {
                    total.saturating_add(entry.id.as_str().len())
                });
            charge_provider_knowledge_work(book.id.as_str(), work_budget, required_clone_work)?;
            let required_entry_ids = book
                .entries
                .iter()
                .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>();
            let query_clone_work = values
                .len()
                .checked_mul(std::mem::size_of::<f32>())
                .and_then(|value| value.checked_add(book_revision_id.len()))
                .and_then(|value| value.checked_add(task_profile_revision_id.len()))
                .and_then(|value| value.checked_add(model_route_id.as_str().len()))
                .and_then(|value| value.checked_add(vector_space_sha256.len()))
                .ok_or_else(|| CoreError::invalid("knowledge embedding query work overflowed"))?;
            charge_provider_knowledge_work(book.id.as_str(), work_budget, query_clone_work)?;
            let query_result = self
                .storage()
                .query_required_knowledge_embeddings_cosine_bounded(
                    &KnowledgeEmbeddingQuery {
                        book_revision_id: book_revision_id.to_owned(),
                        task_profile_revision_id: task_profile_revision_id.clone(),
                        model_route_id: model_route_id.clone(),
                        dimensions: *dimensions,
                        vector_space_sha256: vector_space_sha256.clone(),
                        values: values.clone(),
                    },
                    &required_entry_ids,
                    work_budget.remaining_work_bytes(),
                )?;
            charge_provider_knowledge_work(book.id.as_str(), work_budget, query_result.work_bytes)?;
            let matches = query_result.matches;
            if matches.len() == required_entry_ids.len() {
                let score_projection_work = matches.iter().fold(0_usize, |total, candidate| {
                    total
                        .saturating_add(candidate.entry_id.as_str().len())
                        .saturating_add(std::mem::size_of::<SemanticKnowledgeScore>())
                });
                charge_provider_knowledge_work(
                    book.id.as_str(),
                    work_budget,
                    score_projection_work,
                )?;
                let mut scores = matches
                    .iter()
                    .map(|candidate| {
                        Ok(SemanticKnowledgeScore {
                            entry_id: candidate.entry_id.clone(),
                            score: semantic_score_from_millionths(candidate.similarity_millionths)?,
                        })
                    })
                    .collect::<CoreResult<Vec<_>>>()?;
                scores.sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
                return Ok((
                    scores,
                    Some(KnowledgeSemanticScoreSourceEvidence::ProviderEmbeddingV1 {
                        memory_profile_revision_id: memory_profile_revision_id.clone(),
                        task_profile_revision_id: task_profile_revision_id.clone(),
                        model_route_id: model_route_id.clone(),
                        dimensions: *dimensions,
                        vector_space_sha256: vector_space_sha256.clone(),
                        query_sha256: query_sha256.clone(),
                        query_embedding_id: query_embedding_id.clone(),
                        query_embedding_revision,
                        query_vector_sha256: query_vector_sha256.clone(),
                        matches_sha256: knowledge_embedding_matches_sha256(
                            book_revision_id,
                            &matches,
                            book.id.as_str(),
                            work_budget,
                        )?,
                    }),
                ));
            }
        }

        let scores = lexical_knowledge_semantic_scores_with_budget(book, scan_texts, work_budget)?;
        let query_sha256 = knowledge_semantic_query_sha256(book, scan_texts, work_budget)?;
        Ok((
            scores,
            Some(KnowledgeSemanticScoreSourceEvidence::LexicalV1 { query_sha256 }),
        ))
    }
}
