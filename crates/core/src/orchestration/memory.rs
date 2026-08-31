use super::{PromptSelectionInput, orchestration_validation_error};
use crate::{
    Core, Revisioned,
    orchestration_runtime::{MemorySemanticQueryEvidence, ResolvedMemorySemanticQuery},
    revision::{project_revision, project_revisions},
};
use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId, MemoryJob,
    MemoryJobId, MemoryKind, MemoryProfile, MemoryProfileId, MemoryRecord, MemoryRecordId,
    MessageId, PromptContextSnapshotV1, PromptConversationMessage, PromptMemorySelectionEvidence,
    PromptMemorySelectionLane, PromptMemorySelectionReason, PromptPreset,
    PromptSummarySourceEvidence, SelectedMemory, SummaryBoundary, TemplateSlot,
    ValidateOrchestration,
};
use lorepia_orchestration::{
    MemoryEngine, MemorySelection, MemorySelectionContext, MemorySelectionLane,
    MemorySelectionReason, MemorySemanticScore,
};
use lorepia_storage::{MemoryInvalidationResult, ObjectRevision, StoredRevision};
use std::collections::{BTreeMap, BTreeSet};

/// Owned retrieval request; visible message ids define the complete active
/// branch lineage and are the authority for cross-branch ancestor sharing.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemoryRetrievalRequest {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub profile_id: MemoryProfileId,
    pub visible_message_ids: Vec<MessageId>,
    /// Bounded local query text. Core derives lexical fallback scores or an
    /// exact configured embedding query; callers never provide scores.
    pub query_texts: Vec<String>,
}

pub(super) struct PromptSummaryMaterialization {
    pub(super) boundaries: Vec<SummaryBoundary>,
    pub(super) conversation_summary: Option<String>,
    pub(super) conversation_summary_id: Option<MemoryRecordId>,
    pub(super) evidence: Vec<PromptSummarySourceEvidence>,
}

#[derive(Clone)]
struct VisiblePromptSummary {
    record: MemoryRecord,
    evidence: PromptSummarySourceEvidence,
    end_depth: u64,
}

pub(super) struct PromptContextMaterialization {
    pub(super) user_name: String,
    pub(super) author_note: Option<String>,
    pub(super) group_context: Option<String>,
    pub(super) slots: Vec<TemplateSlot>,
    pub(super) summaries: PromptSummaryMaterialization,
    pub(super) snapshot: PromptContextSnapshotV1,
}

struct PromptMemorySource {
    profile: MemoryProfile,
    records: Vec<MemoryRecord>,
}

impl Core {
    pub(super) fn materialize_prompt_summaries(
        &self,
        preset: &PromptPreset,
        conversation_id: &ConversationId,
        context_source_branch_id: &ConversationBranchId,
        context_head_message_id: Option<&MessageId>,
        messages: &[PromptConversationMessage],
        generation_attempt_id: Option<&GenerationId>,
    ) -> CoreResult<PromptSummaryMaterialization> {
        let (needs_conversation_summary, required_summary_ids) =
            prompt_summary_requirements(preset);
        if !needs_conversation_summary && required_summary_ids.is_empty() {
            return Ok(empty_prompt_summary_materialization());
        }
        let Some(context_head_message_id) = context_head_message_id else {
            return Err(CoreError::invalid(
                "prompt summary source is unavailable before the first durable message",
            ));
        };
        let selected = generation_attempt_id.map_or_else(
            || {
                self.storage().list_memory_records_at_head(
                    conversation_id,
                    context_source_branch_id,
                    Some(context_head_message_id),
                    false,
                )
            },
            |generation_id| {
                self.load_generation_attempt_memory_selection(
                    generation_id,
                    conversation_id,
                    context_source_branch_id,
                    Some(context_head_message_id),
                )
            },
        )?;
        let visible = self.visible_prompt_summaries(
            selected,
            conversation_id,
            context_source_branch_id,
            context_head_message_id,
        )?;
        select_prompt_summary_materialization(
            &visible,
            needs_conversation_summary,
            &required_summary_ids,
            messages,
        )
    }

    fn visible_prompt_summaries(
        &self,
        selected: lorepia_storage::MemoryRecordsAtHeadSelection,
        conversation_id: &ConversationId,
        context_source_branch_id: &ConversationBranchId,
        context_head_message_id: &MessageId,
    ) -> CoreResult<Vec<VisiblePromptSummary>> {
        if selected.snapshot.conversation_id != *conversation_id
            || selected.snapshot.source_branch_id != *context_source_branch_id
            || selected.snapshot.context_head_message_id.as_ref() != Some(context_head_message_id)
            || selected.snapshot.include_invalidated
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "memory source snapshot differs from the exact prompt boundary",
                false,
            ));
        }
        if selected.records.len() != selected.snapshot.records.len() {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "memory source records differ from their exact-head evidence",
                false,
            ));
        }
        let mut candidates = Vec::new();
        for (stored, evidence) in selected.records.into_iter().zip(selected.snapshot.records) {
            validate_prompt_summary_record(&stored, &evidence)?;
            if stored.value.kind == MemoryKind::ConversationSummary
                && stored.value.invalidated_at.is_none()
                && !stored.value.excluded_from_conversation
                && !stored.value.excluded_from_character
            {
                candidates.push((stored.value, prompt_summary_evidence(evidence)));
            }
        }
        let mut endpoint_ids = candidates
            .iter()
            .map(|(record, _)| record.source_end_message_id.clone())
            .collect::<Vec<_>>();
        endpoint_ids.sort_by(|left, right| left.0.cmp(&right.0));
        endpoint_ids.dedup_by(|left, right| left.0 == right.0);
        let depths = self.storage().message_lineage_depths_at_head(
            conversation_id,
            context_source_branch_id,
            context_head_message_id,
            &endpoint_ids,
        )?;
        candidates
            .into_iter()
            .map(|(record, evidence)| {
                let end_depth = depths
                    .get(&record.source_end_message_id)
                    .copied()
                    .ok_or_else(|| {
                        CoreError::new(
                            lorepia_domain::CoreErrorCode::StorageCorrupted,
                            "summary source end has no exact lineage position",
                            false,
                        )
                    })?;
                Ok(VisiblePromptSummary {
                    record,
                    evidence,
                    end_depth,
                })
            })
            .collect()
    }

    pub(super) fn load_generation_attempt_memory_selection(
        &self,
        generation_id: &GenerationId,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        context_head_message_id: Option<&MessageId>,
    ) -> CoreResult<lorepia_storage::MemoryRecordsAtHeadSelection> {
        let before = self
            .storage()
            .get_generation_attempt_before_review(generation_id)?
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "generation attempt is missing its sealed memory snapshot",
                    false,
                )
            })?;
        let snapshot = before.memory_head_snapshot;
        if snapshot.conversation_id != *conversation_id
            || snapshot.source_branch_id != *source_branch_id
            || snapshot.context_head_message_id.as_ref() != context_head_message_id
            || snapshot.include_invalidated
            || lorepia_storage::memory_records_at_head_snapshot_sha256(&snapshot)?
                != snapshot.snapshot_sha256
        {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "generation memory snapshot differs from its prompt boundary",
                false,
            ));
        }
        let mut records = Vec::with_capacity(snapshot.records.len());
        for evidence in &snapshot.records {
            let exact = self
                .storage()
                .get_memory_record_revision_by_id(&evidence.active_revision_id)?;
            if exact.object_kind != "memory_record"
                || exact.object_id != evidence.record_id.as_str()
                || exact.revision_id != evidence.active_revision_id
                || exact.revision != evidence.state_revision
                || exact.sha256 != evidence.active_revision_sha256
                || exact.value.id != evidence.record_id
                || exact.value.branch_id != evidence.record_branch_id
                || exact.value.source_start_message_id != evidence.source_start_message_id
                || exact.value.source_end_message_id != evidence.source_end_message_id
            {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "sealed memory revision differs from its attempt evidence",
                    false,
                ));
            }
            records.push(StoredRevision {
                value: exact.value,
                revision: exact.revision,
                revision_id: Some(exact.revision_id),
                created_at: exact.created_at,
                updated_at: exact.created_at,
                deleted_at: None,
            });
        }
        Ok(lorepia_storage::MemoryRecordsAtHeadSelection { snapshot, records })
    }

    pub fn upsert_memory_profile(
        &self,
        profile: &MemoryProfile,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<MemoryProfile>> {
        profile.validate().map_err(orchestration_validation_error)?;
        self.storage()
            .save_memory_profile(profile, expected_revision)
            .map(project_revision)
    }

    pub fn get_memory_profile(
        &self,
        id: &MemoryProfileId,
    ) -> CoreResult<Revisioned<MemoryProfile>> {
        self.storage().get_memory_profile(id).map(project_revision)
    }

    pub fn list_memory_profiles(&self) -> CoreResult<Vec<Revisioned<MemoryProfile>>> {
        self.storage().list_memory_profiles().map(project_revisions)
    }

    pub fn delete_memory_profile(
        &self,
        id: &MemoryProfileId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<MemoryProfile>> {
        self.storage()
            .soft_delete_memory_profile(id, expected_revision)
            .map(project_revision)
    }

    pub fn get_memory_record(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
    ) -> CoreResult<Revisioned<MemoryRecord>> {
        self.storage()
            .get_memory_record(conversation_id, branch_id, id)
            .map(project_revision)
    }

    pub fn delete_memory_record(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        id: &MemoryRecordId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<MemoryRecord>> {
        self.storage()
            .delete_memory_record_tombstone(
                conversation_id,
                branch_id,
                id,
                expected_revision,
                Utc::now(),
            )
            .map(project_revision)
    }

    pub fn list_memory_records(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        include_invalidated: bool,
    ) -> CoreResult<Vec<Revisioned<MemoryRecord>>> {
        self.storage()
            .list_memory_records(conversation_id, branch_id, include_invalidated)
            .map(project_revisions)
    }

    pub fn retrieve_memory(&self, request: &MemoryRetrievalRequest) -> CoreResult<MemorySelection> {
        let profile = self.get_memory_profile(&request.profile_id)?.value;
        if profile.embedding_task.is_some() {
            return Err(CoreError::invalid(
                "configured memory embeddings require the provider-native retrieval path",
            ));
        }
        let records = self
            .list_memory_records(&request.conversation_id, &request.branch_id, false)?
            .into_iter()
            .map(|stored| stored.value)
            .collect::<Vec<_>>();
        if request.query_texts.len() > 32
            || request
                .query_texts
                .iter()
                .try_fold(0_usize, |total, text| total.checked_add(text.len()))
                .is_none_or(|total| total > 65_536)
        {
            return Err(CoreError::invalid(
                "memory retrieval query exceeds the local lexical safety limit",
            ));
        }
        let token_estimates = records
            .iter()
            .map(|record| {
                (
                    record.id.clone(),
                    estimate_prompt_memory_tokens(&record.title, &record.summary),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let semantic_scores = lexical_memory_semantic_scores(
            &records,
            request.query_texts.iter().map(String::as_str),
        );
        MemoryEngine::select(
            &records,
            &profile,
            &MemorySelectionContext {
                conversation_id: &request.conversation_id,
                branch_id: &request.branch_id,
                visible_message_ids: &request.visible_message_ids,
                semantic_scores: &semantic_scores,
                token_estimates: &token_estimates,
            },
        )
        .map_err(orchestration_validation_error)
    }

    pub fn invalidate_memory_range(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        start_message_id: &MessageId,
        end_message_id: &MessageId,
        invalidated_at: DateTime<Utc>,
    ) -> CoreResult<MemoryInvalidationResult> {
        self.storage().invalidate_memory_range(
            conversation_id,
            branch_id,
            start_message_id,
            end_message_id,
            invalidated_at,
        )
    }

    pub fn get_memory_job(&self, id: &MemoryJobId) -> CoreResult<Revisioned<MemoryJob>> {
        self.storage().get_memory_job(id).map(project_revision)
    }

    pub(super) fn select_prompt_memory(
        &self,
        input: &PromptSelectionInput<'_>,
    ) -> CoreResult<(Vec<SelectedMemory>, Vec<PromptMemorySelectionEvidence>)> {
        let Some(source) = self.load_prompt_memory_source(input)? else {
            return Ok((Vec::new(), Vec::new()));
        };
        let semantic_scores = prompt_memory_selection_semantic_scores(
            &source.profile,
            input.memory_profile,
            &source.records,
            input.prompt_messages,
            input.resolved_memory_semantics,
        )?;
        let visible_message_ids = input
            .prompt_messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<Vec<_>>();
        let token_estimates = source
            .records
            .iter()
            .map(|record| {
                (
                    record.id.clone(),
                    estimate_prompt_memory_tokens(&record.title, &record.summary),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let selection = MemoryEngine::select(
            &source.records,
            &source.profile,
            &MemorySelectionContext {
                conversation_id: input.conversation_id,
                branch_id: input.branch_id,
                visible_message_ids: &visible_message_ids,
                semantic_scores: &semantic_scores,
                token_estimates: &token_estimates,
            },
        )
        .map_err(orchestration_validation_error)?;
        materialize_prompt_memory_selection(selection, &source.records)
    }

    fn load_prompt_memory_source(
        &self,
        input: &PromptSelectionInput<'_>,
    ) -> CoreResult<Option<PromptMemorySource>> {
        let Some(profile_id) = &input.preset.memory_profile_id else {
            return Ok(None);
        };
        let profile = input
            .memory_profile
            .filter(|revision| revision.value.id == *profile_id)
            .map(|revision| revision.value.clone())
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "prompt preset memory profile dependency is missing its exact revision",
                    false,
                )
            })?;
        let lineage_branch_id = input.memory_lineage_branch_id.unwrap_or(input.branch_id);
        let selection = input.generation_attempt_id.map_or_else(
            || {
                self.storage().list_memory_records_at_head(
                    input.conversation_id,
                    lineage_branch_id,
                    input.memory_context_head_message_id,
                    false,
                )
            },
            |generation_id| {
                self.load_generation_attempt_memory_selection(
                    generation_id,
                    input.conversation_id,
                    lineage_branch_id,
                    input.memory_context_head_message_id,
                )
            },
        )?;
        let records = selection
            .records
            .into_iter()
            .map(|stored| stored.value)
            .collect();
        Ok(Some(PromptMemorySource { profile, records }))
    }
}

fn prompt_memory_selection_semantic_scores(
    profile: &MemoryProfile,
    exact_profile: Option<&ObjectRevision<MemoryProfile>>,
    records: &[MemoryRecord],
    messages: &[PromptConversationMessage],
    resolved_semantics: Option<&ResolvedMemorySemanticQuery>,
) -> CoreResult<Vec<MemorySemanticScore>> {
    match (profile.embedding_task.is_some(), resolved_semantics) {
        (false, None) => Ok(prompt_memory_semantic_scores(records, messages)),
        (true, Some(resolved)) => {
            if !memory_semantic_evidence_matches_profile(
                &resolved.evidence,
                &profile.id,
                exact_profile
                    .map(|revision| revision.revision_id.as_str())
                    .unwrap_or_default(),
            ) {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "provider-native memory evidence differs from the exact prompt profile",
                    false,
                ));
            }
            Ok(resolved.scores.clone())
        }
        (true, None) => Err(CoreError::invalid(
            "configured memory embeddings require the durable provider-native retrieval path",
        )),
        (false, Some(_)) => Err(CoreError::invalid(
            "lexical memory profiles cannot accept provider-native semantic scores",
        )),
    }
}

fn materialize_prompt_memory_selection(
    selection: MemorySelection,
    records: &[MemoryRecord],
) -> CoreResult<(Vec<SelectedMemory>, Vec<PromptMemorySelectionEvidence>)> {
    let selected = selection
        .selected
        .into_iter()
        .map(|selected| {
            let record = records
                .iter()
                .find(|record| record.id == selected.record_id)
                .ok_or_else(|| CoreError::internal("selected memory record disappeared"))?;
            Ok(SelectedMemory {
                record_id: record.id.clone(),
                branch_id: record.branch_id.clone(),
                content: selected.summary,
                score_millionths: u32::try_from(selected.rank_millionths).unwrap_or(u32::MAX),
                reason: serde_json::to_string(&selected.reasons).map_err(|error| {
                    CoreError::internal(format!("memory evidence could not be encoded: {error}"))
                })?,
                provenance: record.provenance.clone(),
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let evidence = selection
        .evidence
        .into_iter()
        .map(|evidence| PromptMemorySelectionEvidence {
            record_id: evidence.record_id,
            selected: evidence.selected,
            lane: evidence.lane.map(prompt_memory_lane),
            rank_millionths: evidence.rank_millionths,
            estimated_tokens: evidence.estimated_tokens,
            reasons: evidence
                .reasons
                .into_iter()
                .map(prompt_memory_reason)
                .collect(),
            exclusion_reason: evidence.exclusion_reason,
        })
        .collect();
    Ok((selected, evidence))
}

fn prompt_summary_requirements(preset: &PromptPreset) -> (bool, BTreeSet<MemoryRecordId>) {
    let needs_conversation_summary = preset.blocks.iter().any(|block| {
        block.enabled
            && matches!(
                block.source,
                lorepia_domain::BlockSource::ConversationSummary
            )
    });
    let required_summary_ids = preset
        .blocks
        .iter()
        .filter(|block| block.enabled)
        .filter_map(|block| match block.history_selector.as_ref() {
            Some(lorepia_domain::HistorySelector::SinceSummary { summary_id }) => {
                Some(summary_id.clone())
            }
            _ => None,
        })
        .collect();
    (needs_conversation_summary, required_summary_ids)
}

fn empty_prompt_summary_materialization() -> PromptSummaryMaterialization {
    PromptSummaryMaterialization {
        boundaries: Vec::new(),
        conversation_summary: None,
        conversation_summary_id: None,
        evidence: Vec::new(),
    }
}

fn validate_prompt_summary_record(
    stored: &StoredRevision<MemoryRecord>,
    evidence: &lorepia_storage::MemoryRecordAtHeadEvidence,
) -> CoreResult<()> {
    if stored.value.id != evidence.record_id
        || stored.value.branch_id != evidence.record_branch_id
        || stored.value.source_start_message_id != evidence.source_start_message_id
        || stored.value.source_end_message_id != evidence.source_end_message_id
        || stored.revision != evidence.state_revision
        || stored.revision_id.as_deref() != Some(evidence.active_revision_id.as_str())
        || stored.deleted_at.is_some()
    {
        return Err(CoreError::new(
            lorepia_domain::CoreErrorCode::StorageCorrupted,
            "summary memory record differs from its exact-head evidence",
            false,
        ));
    }
    Ok(())
}

fn prompt_summary_evidence(
    evidence: lorepia_storage::MemoryRecordAtHeadEvidence,
) -> PromptSummarySourceEvidence {
    PromptSummarySourceEvidence {
        summary_id: evidence.record_id,
        record_branch_id: evidence.record_branch_id,
        source_start_message_id: evidence.source_start_message_id,
        source_end_message_id: evidence.source_end_message_id,
        state_revision: evidence.state_revision,
        active_revision_id: evidence.active_revision_id,
        active_revision_sha256: evidence.active_revision_sha256,
    }
}

fn select_prompt_summary_materialization(
    visible: &[VisiblePromptSummary],
    needs_conversation_summary: bool,
    required_summary_ids: &BTreeSet<MemoryRecordId>,
    messages: &[PromptConversationMessage],
) -> CoreResult<PromptSummaryMaterialization> {
    let mut ordered = visible.to_vec();
    ordered.sort_by(|left, right| {
        right
            .end_depth
            .cmp(&left.end_depth)
            .then_with(|| left.record.id.cmp(&right.record.id))
    });
    for required in required_summary_ids {
        let Some(summary) = ordered
            .iter()
            .find(|summary| summary.record.id == *required)
        else {
            return Err(CoreError::invalid(format!(
                "prompt history requires unavailable summary `{}`",
                required.as_str()
            )));
        };
        if !messages
            .iter()
            .any(|message| message.id == summary.record.source_end_message_id)
        {
            return Err(CoreError::invalid(format!(
                "summary `{}` ends outside the bounded prompt history",
                required.as_str()
            )));
        }
    }
    let conversation_summary = if needs_conversation_summary {
        Some(ordered.last().ok_or_else(|| {
            CoreError::invalid("enabled conversation-summary block has no visible summary memory")
        })?)
    } else {
        None
    };
    let conversation_summary_id = conversation_summary.map(|summary| summary.record.id.clone());
    let conversation_summary_text =
        conversation_summary.map(|summary| summary.record.summary.clone());
    let mut selected_ids = required_summary_ids.clone();
    if let Some(summary_id) = &conversation_summary_id {
        selected_ids.insert(summary_id.clone());
    }
    let selected = ordered
        .into_iter()
        .filter(|summary| selected_ids.contains(&summary.record.id))
        .collect::<Vec<_>>();
    let boundaries = selected
        .iter()
        .filter(|summary| required_summary_ids.contains(&summary.record.id))
        .map(|summary| SummaryBoundary {
            summary_id: summary.record.id.clone(),
            end_message_id: summary.record.source_end_message_id.clone(),
        })
        .collect();
    Ok(PromptSummaryMaterialization {
        boundaries,
        conversation_summary: conversation_summary_text,
        conversation_summary_id,
        evidence: selected
            .into_iter()
            .map(|summary| summary.evidence)
            .collect(),
    })
}

fn estimate_prompt_memory_tokens(_title: &str, summary: &str) -> u32 {
    if summary.is_empty() {
        0
    } else {
        u32::try_from(summary.len().div_ceil(4)).unwrap_or(u32::MAX)
    }
}

fn prompt_memory_semantic_scores(
    records: &[MemoryRecord],
    messages: &[PromptConversationMessage],
) -> Vec<MemorySemanticScore> {
    lexical_memory_semantic_scores(
        records,
        messages
            .iter()
            .rev()
            .map(|message| message.content.as_str()),
    )
}

fn memory_semantic_evidence_matches_profile(
    evidence: &MemorySemanticQueryEvidence,
    _profile_id: &MemoryProfileId,
    revision_id: &str,
) -> bool {
    match evidence {
        MemorySemanticQueryEvidence::LexicalV1 {
            memory_profile_revision_id,
            ..
        }
        | MemorySemanticQueryEvidence::ProviderEmbeddingV1 {
            memory_profile_revision_id,
            ..
        } => memory_profile_revision_id == revision_id,
    }
}

fn lexical_memory_semantic_scores<'a>(
    records: &[MemoryRecord],
    query_texts: impl IntoIterator<Item = &'a str>,
) -> Vec<MemorySemanticScore> {
    const MAX_QUERY_MESSAGES: usize = 32;
    const MAX_QUERY_CHARS: usize = 65_536;
    let query_chars = query_texts
        .into_iter()
        .take(MAX_QUERY_MESSAGES)
        .flat_map(str::chars)
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
            let score = if union == 0 {
                0.0
            } else {
                usize_as_f32(intersection) / usize_as_f32(union)
            };
            MemorySemanticScore {
                record_id: record.id.clone(),
                score,
            }
        })
        .collect()
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

const fn prompt_memory_lane(lane: MemorySelectionLane) -> PromptMemorySelectionLane {
    match lane {
        MemorySelectionLane::Pinned => PromptMemorySelectionLane::Pinned,
        MemorySelectionLane::Semantic => PromptMemorySelectionLane::Semantic,
        MemorySelectionLane::Episodic => PromptMemorySelectionLane::Episodic,
    }
}

fn prompt_memory_reason(reason: MemorySelectionReason) -> PromptMemorySelectionReason {
    match reason {
        MemorySelectionReason::Pinned => PromptMemorySelectionReason::Pinned,
        MemorySelectionReason::CurrentBranch => PromptMemorySelectionReason::CurrentBranch,
        MemorySelectionReason::SharedAncestor { source_branch_id } => {
            PromptMemorySelectionReason::SharedAncestor { source_branch_id }
        }
        MemorySelectionReason::Recency { score_millionths } => {
            PromptMemorySelectionReason::Recency { score_millionths }
        }
        MemorySelectionReason::Similarity { score_millionths } => {
            PromptMemorySelectionReason::Similarity { score_millionths }
        }
        MemorySelectionReason::Importance { score_millionths } => {
            PromptMemorySelectionReason::Importance { score_millionths }
        }
    }
}
