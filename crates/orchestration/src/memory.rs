use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{
    AuxiliaryTaskKind, ConversationBranchId, ConversationId, MemoryJob, MemoryJobId, MemoryJobKind,
    MemoryJobStatus, MemoryKind, MemoryProfile, MemoryProfileId, MemoryRecord, MemoryRecordId,
    MessageId, TaskProfile, ValidateOrchestration,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::knowledge::estimate_text_tokens;

pub const MAX_MEMORY_RECORDS_PER_SELECTION: usize = 100_000;
pub const MAX_VISIBLE_MEMORY_LINEAGE_MESSAGES: usize = 100_000;
pub const MAX_MEMORY_JOB_IDEMPOTENCY_KEY_BYTES: usize = 256;
pub const MAX_MEMORY_JOB_SOURCE_REVISION_BYTES: usize = 1_024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MemorySelectionError {
    #[error("memory profile is invalid: {message}")]
    InvalidProfile { message: String },
    #[error("memory selection contains more than {MAX_MEMORY_RECORDS_PER_SELECTION} records")]
    TooManyRecords,
    #[error(
        "memory branch lineage contains more than {MAX_VISIBLE_MEMORY_LINEAGE_MESSAGES} messages"
    )]
    LineageTooLarge,
    #[error("memory record id is duplicated: {record_id}")]
    DuplicateRecordId { record_id: String },
    #[error("branch lineage contains a duplicate message id: {message_id}")]
    DuplicateLineageMessage { message_id: String },
    #[error("semantic score is duplicated for memory record: {record_id}")]
    DuplicateSemanticScore { record_id: String },
    #[error("semantic score is not finite or is outside 0..=1 for memory record: {record_id}")]
    InvalidSemanticScore { record_id: String },
    #[error("memory ranking weights must be finite and non-negative")]
    InvalidRankingWeights,
    #[error("memory record importance is outside 0..=100: {record_id}")]
    InvalidImportance { record_id: String },
    #[error("memory ranking score overflow")]
    RankingScoreOverflow,
    #[error("memory selection invariant was violated")]
    InternalInvariant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySemanticScore {
    pub record_id: MemoryRecordId,
    pub score: f32,
}

#[derive(Debug)]
pub struct MemorySelectionContext<'a> {
    pub conversation_id: &'a ConversationId,
    pub branch_id: &'a ConversationBranchId,
    /// Immutable message ids on the active branch, oldest to newest. A record
    /// from another branch is shareable only when its complete source range is
    /// present in this lineage.
    pub visible_message_ids: &'a [MessageId],
    /// Scores supplied by the embedding/search subsystem.
    pub semantic_scores: &'a [MemorySemanticScore],
    pub token_estimates: &'a BTreeMap<MemoryRecordId, u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySelectionLane {
    Pinned,
    Semantic,
    Episodic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MemorySelectionReason {
    Pinned,
    CurrentBranch,
    SharedAncestor {
        source_branch_id: ConversationBranchId,
    },
    Recency {
        score_millionths: u32,
    },
    Similarity {
        score_millionths: u32,
    },
    Importance {
        score_millionths: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedMemoryRecord {
    pub record_id: MemoryRecordId,
    pub kind: MemoryKind,
    pub title: String,
    pub summary: String,
    pub lane: MemorySelectionLane,
    pub rank_millionths: u64,
    pub estimated_tokens: u32,
    pub reasons: Vec<MemorySelectionReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySelectionEvidence {
    pub record_id: MemoryRecordId,
    pub selected: bool,
    pub lane: Option<MemorySelectionLane>,
    pub rank_millionths: Option<u64>,
    pub estimated_tokens: u32,
    pub reasons: Vec<MemorySelectionReason>,
    pub exclusion_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemorySelection {
    pub selected: Vec<SelectedMemoryRecord>,
    pub evidence: Vec<MemorySelectionEvidence>,
    pub used_episodic_tokens: u32,
    pub used_semantic_tokens: u32,
}

#[derive(Debug, Clone)]
struct RankedMemory<'a> {
    record: &'a MemoryRecord,
    rank_millionths: u64,
    recency_millionths: u32,
    similarity_millionths: Option<u32>,
    importance_millionths: u32,
    estimated_tokens: u32,
    shared_ancestor: bool,
    pinned: bool,
}

pub struct MemoryEngine;

impl MemoryEngine {
    /// Selects branch-valid memory records with fixed-point ranking and stable
    /// id tie-breaking.
    #[allow(clippy::too_many_lines)]
    pub fn select(
        records: &[MemoryRecord],
        profile: &MemoryProfile,
        context: &MemorySelectionContext<'_>,
    ) -> Result<MemorySelection, MemorySelectionError> {
        validate_profile(profile)?;
        if records.len() > MAX_MEMORY_RECORDS_PER_SELECTION {
            return Err(MemorySelectionError::TooManyRecords);
        }
        let records = normalized_records(records)?;
        let lineage = normalized_lineage(context.visible_message_ids)?;
        let semantic_scores = normalize_semantic_scores(&records, context.semantic_scores)?;
        let mut evidence = records
            .iter()
            .map(|record| {
                (
                    record.id.clone(),
                    MemorySelectionEvidence {
                        record_id: record.id.clone(),
                        selected: false,
                        lane: None,
                        rank_millionths: None,
                        estimated_tokens: memory_tokens(record, context.token_estimates),
                        reasons: Vec::new(),
                        exclusion_reason: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut ranked = Vec::new();

        for record in records {
            let evidence_item = evidence
                .get_mut(&record.id)
                .ok_or(MemorySelectionError::InternalInvariant)?;
            let Some((_start_index, end_index)) =
                eligible_range(record, context, &lineage, evidence_item)
            else {
                continue;
            };
            let recency_millionths = normalized_recency(end_index, lineage.len());
            if record.importance > 100 {
                return Err(MemorySelectionError::InvalidImportance {
                    record_id: record.id.as_str().to_owned(),
                });
            }
            let importance_millionths = u32::from(record.importance) * 10_000;
            let similarity_millionths = semantic_scores.get(&record.id).copied();
            let rank_millionths = weighted_rank(
                recency_millionths,
                similarity_millionths.unwrap_or(0),
                importance_millionths,
                profile,
            )?;
            let shared_ancestor = record.branch_id != *context.branch_id;
            let pinned = record.pinned || record.kind == MemoryKind::CreatorPinned;
            ranked.push(RankedMemory {
                record,
                rank_millionths,
                recency_millionths,
                similarity_millionths,
                importance_millionths,
                estimated_tokens: memory_tokens(record, context.token_estimates),
                shared_ancestor,
                pinned,
            });
        }

        ranked.sort_by(|left, right| {
            (!left.pinned)
                .cmp(&(!right.pinned))
                .then_with(|| right.rank_millionths.cmp(&left.rank_millionths))
                .then_with(|| right.recency_millionths.cmp(&left.recency_millionths))
                .then_with(|| right.importance_millionths.cmp(&left.importance_millionths))
                .then_with(|| left.estimated_tokens.cmp(&right.estimated_tokens))
                .then_with(|| left.record.id.cmp(&right.record.id))
        });

        let mut selected = Vec::new();
        let mut used_episodic_tokens = 0_u32;
        let mut used_semantic_tokens = 0_u32;
        let mut ordinary_selected = 0_u32;
        for candidate in ranked {
            let evidence_item = evidence
                .get_mut(&candidate.record.id)
                .ok_or(MemorySelectionError::InternalInvariant)?;
            evidence_item.rank_millionths = Some(candidate.rank_millionths);
            evidence_item.reasons = ranking_reasons(&candidate, context.branch_id);

            if !candidate.pinned && ordinary_selected >= profile.retrieval_count {
                evidence_item.exclusion_reason =
                    Some("memory retrieval count limit reached".to_owned());
                continue;
            }

            let lane = choose_lane(
                &candidate,
                profile,
                used_episodic_tokens,
                used_semantic_tokens,
            );
            let Some(lane) = lane else {
                evidence_item.exclusion_reason =
                    Some("memory record does not fit the remaining token budgets".to_owned());
                continue;
            };
            match lane {
                MemorySelectionLane::Pinned | MemorySelectionLane::Episodic => {
                    used_episodic_tokens = used_episodic_tokens
                        .checked_add(candidate.estimated_tokens)
                        .ok_or(MemorySelectionError::InternalInvariant)?;
                }
                MemorySelectionLane::Semantic => {
                    used_semantic_tokens = used_semantic_tokens
                        .checked_add(candidate.estimated_tokens)
                        .ok_or(MemorySelectionError::InternalInvariant)?;
                }
            }
            if !candidate.pinned {
                ordinary_selected = ordinary_selected.saturating_add(1);
            }
            evidence_item.selected = true;
            evidence_item.lane = Some(lane);
            evidence_item.exclusion_reason = None;
            selected.push(SelectedMemoryRecord {
                record_id: candidate.record.id.clone(),
                kind: candidate.record.kind,
                title: candidate.record.title.clone(),
                summary: candidate.record.summary.clone(),
                lane,
                rank_millionths: candidate.rank_millionths,
                estimated_tokens: candidate.estimated_tokens,
                reasons: evidence_item.reasons.clone(),
            });
        }

        let mut evidence = evidence.into_values().collect::<Vec<_>>();
        evidence.sort_by(|left, right| left.record_id.cmp(&right.record_id));
        Ok(MemorySelection {
            selected,
            evidence,
            used_episodic_tokens,
            used_semantic_tokens,
        })
    }
}

fn validate_profile(profile: &MemoryProfile) -> Result<(), MemorySelectionError> {
    profile
        .validate()
        .map_err(|error| MemorySelectionError::InvalidProfile {
            message: error.to_string(),
        })?;
    let weights = [
        profile.recency_weight,
        profile.similarity_weight,
        profile.importance_weight,
    ];
    if weights
        .iter()
        .any(|weight| !weight.is_finite() || *weight < 0.0)
    {
        return Err(MemorySelectionError::InvalidRankingWeights);
    }
    Ok(())
}

fn normalized_records(
    records: &[MemoryRecord],
) -> Result<Vec<&MemoryRecord>, MemorySelectionError> {
    let mut records = records.iter().collect::<Vec<_>>();
    records.sort_by(|left, right| left.id.cmp(&right.id));
    for pair in records.windows(2) {
        if pair[0].id == pair[1].id {
            return Err(MemorySelectionError::DuplicateRecordId {
                record_id: pair[0].id.as_str().to_owned(),
            });
        }
    }
    Ok(records)
}

fn normalized_lineage(
    visible_message_ids: &[MessageId],
) -> Result<BTreeMap<String, usize>, MemorySelectionError> {
    if visible_message_ids.len() > MAX_VISIBLE_MEMORY_LINEAGE_MESSAGES {
        return Err(MemorySelectionError::LineageTooLarge);
    }
    let mut lineage = BTreeMap::new();
    for (index, message_id) in visible_message_ids.iter().enumerate() {
        if lineage.insert(message_id.0.clone(), index).is_some() {
            return Err(MemorySelectionError::DuplicateLineageMessage {
                message_id: message_id.0.clone(),
            });
        }
    }
    Ok(lineage)
}

fn normalize_semantic_scores(
    records: &[&MemoryRecord],
    supplied: &[MemorySemanticScore],
) -> Result<BTreeMap<MemoryRecordId, u32>, MemorySelectionError> {
    let known = records
        .iter()
        .map(|record| record.id.clone())
        .collect::<BTreeSet<_>>();
    let mut scores = BTreeMap::new();
    for score in supplied {
        if !known.contains(&score.record_id) {
            continue;
        }
        if !score.score.is_finite() || !(0.0..=1.0).contains(&score.score) {
            return Err(MemorySelectionError::InvalidSemanticScore {
                record_id: score.record_id.as_str().to_owned(),
            });
        }
        if scores
            .insert(score.record_id.clone(), score_to_millionths(score.score))
            .is_some()
        {
            return Err(MemorySelectionError::DuplicateSemanticScore {
                record_id: score.record_id.as_str().to_owned(),
            });
        }
    }
    Ok(scores)
}

fn eligible_range(
    record: &MemoryRecord,
    context: &MemorySelectionContext<'_>,
    lineage: &BTreeMap<String, usize>,
    evidence: &mut MemorySelectionEvidence,
) -> Option<(usize, usize)> {
    let reason = if record.conversation_id != *context.conversation_id {
        "memory belongs to another conversation"
    } else if record.invalidated_at.is_some() {
        "memory has been invalidated"
    } else if record.excluded_from_conversation {
        "memory is excluded from this conversation"
    } else if record.excluded_from_character {
        "memory is excluded from this character"
    } else {
        ""
    };
    if !reason.is_empty() {
        evidence.exclusion_reason = Some(reason.to_owned());
        return None;
    }

    let Some(start_index) = lineage
        .get(record.source_start_message_id.0.as_str())
        .copied()
    else {
        evidence.exclusion_reason =
            Some("memory source range is not on the active branch lineage".to_owned());
        return None;
    };
    let Some(end_index) = lineage
        .get(record.source_end_message_id.0.as_str())
        .copied()
    else {
        evidence.exclusion_reason =
            Some("memory source range is not on the active branch lineage".to_owned());
        return None;
    };
    if start_index > end_index {
        evidence.exclusion_reason = Some("memory source range is reversed".to_owned());
        return None;
    }
    Some((start_index, end_index))
}

fn normalized_recency(end_index: usize, lineage_len: usize) -> u32 {
    if lineage_len <= 1 {
        return 1_000_000;
    }
    let numerator = u64::try_from(end_index).unwrap_or(u64::MAX) * 1_000_000;
    let denominator = u64::try_from(lineage_len - 1).unwrap_or(u64::MAX);
    u32::try_from(numerator / denominator).unwrap_or(1_000_000)
}

fn weighted_rank(
    recency_millionths: u32,
    similarity_millionths: u32,
    importance_millionths: u32,
    profile: &MemoryProfile,
) -> Result<u64, MemorySelectionError> {
    [
        (recency_millionths, profile.recency_weight),
        (similarity_millionths, profile.similarity_weight),
        (importance_millionths, profile.importance_weight),
    ]
    .into_iter()
    .try_fold(0_u64, |total, (component, weight)| {
        let weight_millionths = weight_to_millionths(weight)?;
        let weighted = u64::from(component)
            .checked_mul(weight_millionths)
            .ok_or(MemorySelectionError::RankingScoreOverflow)?
            / 1_000_000;
        total
            .checked_add(weighted)
            .ok_or(MemorySelectionError::RankingScoreOverflow)
    })
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn weight_to_millionths(weight: f32) -> Result<u64, MemorySelectionError> {
    // The explicit bound keeps the fixed-point conversion and later
    // multiplication well inside u64.
    if !weight.is_finite() || !(0.0..=1_000_000.0).contains(&weight) {
        return Err(MemorySelectionError::RankingScoreOverflow);
    }
    Ok((f64::from(weight) * 1_000_000.0).round() as u64)
}

fn ranking_reasons(
    candidate: &RankedMemory<'_>,
    current_branch_id: &ConversationBranchId,
) -> Vec<MemorySelectionReason> {
    let mut reasons = Vec::with_capacity(5);
    if candidate.pinned {
        reasons.push(MemorySelectionReason::Pinned);
    }
    if candidate.shared_ancestor {
        reasons.push(MemorySelectionReason::SharedAncestor {
            source_branch_id: candidate.record.branch_id.clone(),
        });
    } else {
        debug_assert_eq!(&candidate.record.branch_id, current_branch_id);
        reasons.push(MemorySelectionReason::CurrentBranch);
    }
    reasons.push(MemorySelectionReason::Recency {
        score_millionths: candidate.recency_millionths,
    });
    if let Some(score_millionths) = candidate.similarity_millionths {
        reasons.push(MemorySelectionReason::Similarity { score_millionths });
    }
    reasons.push(MemorySelectionReason::Importance {
        score_millionths: candidate.importance_millionths,
    });
    reasons
}

fn choose_lane(
    candidate: &RankedMemory<'_>,
    profile: &MemoryProfile,
    used_episodic_tokens: u32,
    used_semantic_tokens: u32,
) -> Option<MemorySelectionLane> {
    if candidate.pinned {
        return fits_budget(
            used_episodic_tokens,
            candidate.estimated_tokens,
            profile.episodic_budget.max_tokens,
        )
        .then_some(MemorySelectionLane::Pinned);
    }
    if candidate.similarity_millionths.is_some()
        && fits_budget(
            used_semantic_tokens,
            candidate.estimated_tokens,
            profile.semantic_budget.max_tokens,
        )
    {
        return Some(MemorySelectionLane::Semantic);
    }
    fits_budget(
        used_episodic_tokens,
        candidate.estimated_tokens,
        profile.episodic_budget.max_tokens,
    )
    .then_some(MemorySelectionLane::Episodic)
}

fn fits_budget(used: u32, cost: u32, budget: u32) -> bool {
    used.checked_add(cost).is_some_and(|next| next <= budget)
}

fn memory_tokens(record: &MemoryRecord, token_estimates: &BTreeMap<MemoryRecordId, u32>) -> u32 {
    token_estimates
        .get(&record.id)
        .copied()
        .unwrap_or_else(|| estimate_text_tokens(&record.summary))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn score_to_millionths(score: f32) -> u32 {
    // Callers validate that scores are finite and in 0..=1.
    (f64::from(score) * 1_000_000.0).round() as u32
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MemoryInvalidationError {
    #[error(
        "memory branch lineage contains more than {MAX_VISIBLE_MEMORY_LINEAGE_MESSAGES} messages"
    )]
    LineageTooLarge,
    #[error("memory record id is duplicated: {record_id}")]
    DuplicateRecordId { record_id: String },
    #[error("branch lineage contains a duplicate message id: {message_id}")]
    DuplicateLineageMessage { message_id: String },
    #[error("invalidation start message is not on the supplied branch lineage")]
    StartMessageNotFound,
    #[error("memory source range is not on its owning branch: {record_id}")]
    RecordRangeNotFound { record_id: String },
    #[error("memory source range is reversed: {record_id}")]
    ReversedRecordRange { record_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryInvalidationDisposition {
    PreserveAsInvalidated,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemoryInvalidationAction {
    pub record_id: MemoryRecordId,
    pub disposition: MemoryInvalidationDisposition,
}

/// Plans invalidation for a destructive rewind of one branch. Records owned by
/// other branches remain intact; branch-aware selection naturally stops using
/// them when their source ids are no longer visible.
pub fn plan_memory_invalidation(
    records: &[MemoryRecord],
    profile: &MemoryProfile,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    branch_message_ids: &[MessageId],
    start_message_id: &MessageId,
) -> Result<Vec<MemoryInvalidationAction>, MemoryInvalidationError> {
    if branch_message_ids.len() > MAX_VISIBLE_MEMORY_LINEAGE_MESSAGES {
        return Err(MemoryInvalidationError::LineageTooLarge);
    }
    let mut lineage = BTreeMap::new();
    for (index, message_id) in branch_message_ids.iter().enumerate() {
        if lineage.insert(message_id.0.clone(), index).is_some() {
            return Err(MemoryInvalidationError::DuplicateLineageMessage {
                message_id: message_id.0.clone(),
            });
        }
    }
    let cut_index = lineage
        .get(start_message_id.0.as_str())
        .copied()
        .ok_or(MemoryInvalidationError::StartMessageNotFound)?;
    let disposition = if profile.preserve_invalidated_records {
        MemoryInvalidationDisposition::PreserveAsInvalidated
    } else {
        MemoryInvalidationDisposition::Delete
    };
    let mut actions = Vec::new();
    let mut seen_record_ids = BTreeSet::new();
    for record in records {
        if !seen_record_ids.insert(record.id.clone()) {
            return Err(MemoryInvalidationError::DuplicateRecordId {
                record_id: record.id.as_str().to_owned(),
            });
        }
        if record.conversation_id != *conversation_id
            || record.branch_id != *branch_id
            || record.invalidated_at.is_some()
        {
            continue;
        }
        let start = lineage
            .get(record.source_start_message_id.0.as_str())
            .copied()
            .ok_or_else(|| MemoryInvalidationError::RecordRangeNotFound {
                record_id: record.id.as_str().to_owned(),
            })?;
        let end = lineage
            .get(record.source_end_message_id.0.as_str())
            .copied()
            .ok_or_else(|| MemoryInvalidationError::RecordRangeNotFound {
                record_id: record.id.as_str().to_owned(),
            })?;
        if start > end {
            return Err(MemoryInvalidationError::ReversedRecordRange {
                record_id: record.id.as_str().to_owned(),
            });
        }
        if end >= cut_index {
            actions.push(MemoryInvalidationAction {
                record_id: record.id.clone(),
                disposition,
            });
        }
    }
    actions.sort_by(|left, right| left.record_id.cmp(&right.record_id));
    Ok(actions)
}

#[derive(Debug)]
pub struct MemoryJobKeyInput<'a> {
    pub kind: MemoryJobKind,
    pub conversation_id: &'a ConversationId,
    pub branch_id: &'a ConversationBranchId,
    pub source_start_message_id: &'a MessageId,
    pub source_end_message_id: &'a MessageId,
    pub profile_id: Option<&'a MemoryProfileId>,
    pub profile_schema_version: Option<u32>,
    /// Digest/version of canonical visible message input. It must not contain
    /// raw conversation text or credentials.
    pub source_revision: &'a str,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum MemoryJobError {
    #[error("memory job source revision exceeds {MAX_MEMORY_JOB_SOURCE_REVISION_BYTES} bytes")]
    SourceRevisionTooLong,
    #[error("memory job profile id and schema version must be supplied together")]
    IncompleteProfileIdentity,
    #[error("memory job idempotency key is empty or too long")]
    InvalidIdempotencyKey,
    #[error("multiple existing memory jobs share the same idempotency key")]
    DuplicateExistingKey,
    #[error("memory job idempotency key already belongs to a different request")]
    IdempotencyConflict,
    #[error("memory job status transition is not allowed")]
    InvalidStatusTransition,
    #[error("task profile kind does not match the memory job kind")]
    TaskKindMismatch,
    #[error("task profile rate and concurrency limits must be positive")]
    InvalidTaskProfile,
    #[error("memory job id is duplicated: {job_id}")]
    DuplicateJobId { job_id: String },
}

/// Produces a stable, redacted key. The key contains only a version marker and
/// SHA-256 digest, never raw conversation text.
pub fn derive_memory_job_idempotency_key(
    input: &MemoryJobKeyInput<'_>,
) -> Result<String, MemoryJobError> {
    if input.source_revision.len() > MAX_MEMORY_JOB_SOURCE_REVISION_BYTES {
        return Err(MemoryJobError::SourceRevisionTooLong);
    }
    if input.profile_id.is_some() != input.profile_schema_version.is_some() {
        return Err(MemoryJobError::IncompleteProfileIdentity);
    }
    let mut hasher = Sha256::new();
    update_job_hash(&mut hasher, b"lorepia-memory-job-v1");
    update_job_hash(&mut hasher, memory_job_kind_label(input.kind));
    update_job_hash(&mut hasher, input.conversation_id.0.as_bytes());
    update_job_hash(&mut hasher, input.branch_id.0.as_bytes());
    update_job_hash(&mut hasher, input.source_start_message_id.0.as_bytes());
    update_job_hash(&mut hasher, input.source_end_message_id.0.as_bytes());
    update_job_hash(
        &mut hasher,
        input.profile_id.map_or(b"", |id| id.as_str().as_bytes()),
    );
    update_job_hash(
        &mut hasher,
        &input
            .profile_schema_version
            .unwrap_or_default()
            .to_be_bytes(),
    );
    update_job_hash(&mut hasher, input.source_revision.as_bytes());
    Ok(format!("memory-job:v1:{}", hex::encode(hasher.finalize())))
}

fn memory_job_kind_label(kind: MemoryJobKind) -> &'static [u8] {
    match kind {
        MemoryJobKind::Summary => b"summary",
        MemoryJobKind::Embedding => b"embedding",
        MemoryJobKind::InvalidateRange => b"invalidate_range",
    }
}

fn update_job_hash(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryJobRequest {
    pub idempotency_key: String,
    pub kind: MemoryJobKind,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub source_start_message_id: MessageId,
    pub source_end_message_id: MessageId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryJobEnqueueDecision {
    Create,
    Reuse {
        job_id: MemoryJobId,
        status: MemoryJobStatus,
        attempt: u32,
    },
}

/// Returns `Reuse` for an exact replay and rejects key reuse with different
/// source scope. This function does not automatically rerun failed work.
pub fn decide_memory_job_enqueue(
    existing: &[MemoryJob],
    request: &MemoryJobRequest,
) -> Result<MemoryJobEnqueueDecision, MemoryJobError> {
    if request.idempotency_key.is_empty()
        || request.idempotency_key.len() > MAX_MEMORY_JOB_IDEMPOTENCY_KEY_BYTES
    {
        return Err(MemoryJobError::InvalidIdempotencyKey);
    }
    let mut matches = existing
        .iter()
        .filter(|job| job.idempotency_key == request.idempotency_key);
    let Some(job) = matches.next() else {
        return Ok(MemoryJobEnqueueDecision::Create);
    };
    if matches.next().is_some() {
        return Err(MemoryJobError::DuplicateExistingKey);
    }
    if job.kind != request.kind
        || job.conversation_id != request.conversation_id
        || job.branch_id != request.branch_id
        || job.source_start_message_id != request.source_start_message_id
        || job.source_end_message_id != request.source_end_message_id
    {
        return Err(MemoryJobError::IdempotencyConflict);
    }
    Ok(MemoryJobEnqueueDecision::Reuse {
        job_id: job.id.clone(),
        status: job.status,
        attempt: job.attempt,
    })
}

pub fn validate_memory_job_status_transition(
    from: MemoryJobStatus,
    to: MemoryJobStatus,
) -> Result<(), MemoryJobError> {
    let allowed = matches!(
        (from, to),
        (
            MemoryJobStatus::Queued,
            MemoryJobStatus::Running | MemoryJobStatus::Failed | MemoryJobStatus::Cancelled
        ) | (
            MemoryJobStatus::Running,
            MemoryJobStatus::Interrupted
                | MemoryJobStatus::Succeeded
                | MemoryJobStatus::Failed
                | MemoryJobStatus::Cancelled
        ) | (
            MemoryJobStatus::Interrupted,
            MemoryJobStatus::Queued | MemoryJobStatus::Cancelled
        )
    );
    if allowed {
        Ok(())
    } else {
        Err(MemoryJobError::InvalidStatusTransition)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryJobDispatchPlan {
    pub start_job_ids: Vec<MemoryJobId>,
    pub running_jobs: u32,
    pub remaining_rate_slots: u32,
    pub blocked_by_concurrency: bool,
    pub blocked_by_rate_limit: bool,
}

/// Selects queued work in `(created_at, id)` order. The caller supplies the
/// durable count of requests already started in the active rate-limit window;
/// this pure planner never relies on process-local clocks or counters.
pub fn plan_memory_job_dispatch(
    jobs: &[MemoryJob],
    kind: MemoryJobKind,
    task_profile: &TaskProfile,
    requests_started_in_window: u32,
) -> Result<MemoryJobDispatchPlan, MemoryJobError> {
    let expected_task_kind = match kind {
        MemoryJobKind::Summary => AuxiliaryTaskKind::MemorySummary,
        MemoryJobKind::Embedding => AuxiliaryTaskKind::MemoryEmbedding,
        MemoryJobKind::InvalidateRange => {
            return Err(MemoryJobError::TaskKindMismatch);
        }
    };
    if task_profile.kind != expected_task_kind {
        return Err(MemoryJobError::TaskKindMismatch);
    }
    if task_profile.concurrency_limit == 0
        || task_profile.rate_limit.requests == 0
        || task_profile.rate_limit.per_seconds == 0
    {
        return Err(MemoryJobError::InvalidTaskProfile);
    }

    let mut seen_job_ids = BTreeSet::new();
    let mut seen_keys = BTreeSet::new();
    for job in jobs {
        if !seen_job_ids.insert(job.id.clone()) {
            return Err(MemoryJobError::DuplicateJobId {
                job_id: job.id.as_str().to_owned(),
            });
        }
        if !seen_keys.insert(job.idempotency_key.as_str()) {
            return Err(MemoryJobError::DuplicateExistingKey);
        }
    }

    let running_jobs = u32::try_from(
        jobs.iter()
            .filter(|job| job.kind == kind && job.status == MemoryJobStatus::Running)
            .count(),
    )
    .unwrap_or(u32::MAX);
    let concurrency_slots = task_profile.concurrency_limit.saturating_sub(running_jobs);
    let remaining_rate_slots = task_profile
        .rate_limit
        .requests
        .saturating_sub(requests_started_in_window);
    let dispatch_count = concurrency_slots.min(remaining_rate_slots);

    let mut queued = jobs
        .iter()
        .filter(|job| job.kind == kind && job.status == MemoryJobStatus::Queued)
        .collect::<Vec<_>>();
    queued.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });
    let start_job_ids = queued
        .into_iter()
        .take(usize::try_from(dispatch_count).unwrap_or(usize::MAX))
        .map(|job| job.id.clone())
        .collect::<Vec<_>>();

    Ok(MemoryJobDispatchPlan {
        start_job_ids,
        running_jobs,
        remaining_rate_slots,
        blocked_by_concurrency: concurrency_slots == 0,
        blocked_by_rate_limit: remaining_rate_slots == 0,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use lorepia_domain::{
        AuxiliaryTaskKind, ConversationBranchId, ConversationId, GenerationPresetId, MemoryJob,
        MemoryJobId, MemoryJobKind, MemoryJobStatus, MemoryKind, MemoryProfile, MemoryProfileId,
        MemoryRecord, MemoryRecordId, MessageId, ModelRouteId, Provenance, RateLimit, SourceKind,
        SummarySchemaId, TaskProfile, TaskProfileId, TokenBudget, VersionedJson,
    };

    use super::{
        MemoryEngine, MemoryInvalidationDisposition, MemoryInvalidationError,
        MemoryJobEnqueueDecision, MemoryJobError, MemoryJobKeyInput, MemoryJobRequest,
        MemorySelectionContext, MemorySelectionLane, MemorySelectionReason, MemorySemanticScore,
        decide_memory_job_enqueue, derive_memory_job_idempotency_key, plan_memory_invalidation,
        plan_memory_job_dispatch, validate_memory_job_status_transition,
    };

    fn provenance() -> Provenance {
        Provenance {
            source_kind: SourceKind::Generated,
            source_id: None,
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        }
    }

    fn timestamp() -> serde_json::Value {
        serde_json::json!("2026-08-03T00:00:00Z")
    }

    fn record(id: &str, branch: &str, start: &str, end: &str, importance: u8) -> MemoryRecord {
        MemoryRecord {
            id: MemoryRecordId::from(id),
            conversation_id: ConversationId("conversation".to_owned()),
            branch_id: ConversationBranchId(branch.to_owned()),
            source_start_message_id: MessageId(start.to_owned()),
            source_end_message_id: MessageId(end.to_owned()),
            kind: MemoryKind::EpisodicEvent,
            title: id.to_owned(),
            summary: format!("summary {id}"),
            structured_data: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({}),
            },
            importance,
            keywords: Vec::new(),
            embedding_ref: None,
            pinned: false,
            excluded_from_conversation: false,
            excluded_from_character: false,
            created_at: serde_json::from_value(timestamp()).expect("timestamp"),
            updated_at: serde_json::from_value(timestamp()).expect("timestamp"),
            invalidated_at: None,
            provenance: provenance(),
        }
    }

    fn profile() -> MemoryProfile {
        MemoryProfile {
            id: MemoryProfileId::from("profile"),
            name: "Memory".to_owned(),
            schema_version: 1,
            summary_task: TaskProfileId::from("summary"),
            embedding_task: Some(TaskProfileId::from("embedding")),
            turns_per_summary: 8,
            recent_raw_budget: TokenBudget { max_tokens: 100 },
            episodic_budget: TokenBudget { max_tokens: 100 },
            semantic_budget: TokenBudget { max_tokens: 100 },
            retrieval_count: 10,
            recency_weight: 1.0,
            similarity_weight: 1.0,
            importance_weight: 1.0,
            preserve_invalidated_records: true,
            summary_schema: SummarySchemaId::from("schema"),
            provenance: provenance(),
        }
    }

    #[test]
    fn branch_selection_shares_only_complete_common_ancestor_ranges() {
        let records = vec![
            record("ancestor", "root", "m1", "m2", 1),
            record("current", "current", "m2", "m3", 1),
            record("sibling", "sibling", "m2", "sibling-m4", 100),
        ];
        let visible = vec![
            MessageId("m1".to_owned()),
            MessageId("m2".to_owned()),
            MessageId("m3".to_owned()),
        ];
        let conversation = ConversationId("conversation".to_owned());
        let branch = ConversationBranchId("current".to_owned());
        let estimates = BTreeMap::from([
            (MemoryRecordId::from("ancestor"), 1),
            (MemoryRecordId::from("current"), 1),
            (MemoryRecordId::from("sibling"), 1),
        ]);
        let selection = MemoryEngine::select(
            &records,
            &profile(),
            &MemorySelectionContext {
                conversation_id: &conversation,
                branch_id: &branch,
                visible_message_ids: &visible,
                semantic_scores: &[],
                token_estimates: &estimates,
            },
        )
        .expect("select");

        assert_eq!(
            selection
                .selected
                .iter()
                .map(|record| record.record_id.as_str())
                .collect::<Vec<_>>(),
            vec!["current", "ancestor"]
        );
        let ancestor = selection
            .selected
            .iter()
            .find(|record| record.record_id.as_str() == "ancestor")
            .expect("ancestor");
        assert!(
            ancestor
                .reasons
                .iter()
                .any(|reason| matches!(reason, MemorySelectionReason::SharedAncestor { .. }))
        );
        let sibling = selection
            .evidence
            .iter()
            .find(|item| item.record_id.as_str() == "sibling")
            .expect("sibling evidence");
        assert!(
            sibling
                .exclusion_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("active branch"))
        );
    }

    #[test]
    fn invalidated_and_user_excluded_records_never_rank() {
        let mut invalidated = record("invalidated", "current", "m1", "m1", 100);
        invalidated.invalidated_at = Some(serde_json::from_value(timestamp()).expect("timestamp"));
        let mut excluded = record("excluded", "current", "m1", "m1", 100);
        excluded.excluded_from_conversation = true;
        let visible = vec![MessageId("m1".to_owned())];
        let conversation = ConversationId("conversation".to_owned());
        let branch = ConversationBranchId("current".to_owned());
        let selection = MemoryEngine::select(
            &[invalidated, excluded],
            &profile(),
            &MemorySelectionContext {
                conversation_id: &conversation,
                branch_id: &branch,
                visible_message_ids: &visible,
                semantic_scores: &[],
                token_estimates: &BTreeMap::new(),
            },
        )
        .expect("select");
        assert!(selection.selected.is_empty());
    }

    #[test]
    fn ranking_uses_fixed_components_and_id_as_final_tie_break() {
        let records = vec![
            record("b", "current", "m1", "m2", 50),
            record("a", "current", "m1", "m2", 50),
            record("old", "current", "m1", "m1", 100),
        ];
        let visible = vec![MessageId("m1".to_owned()), MessageId("m2".to_owned())];
        let scores = vec![
            MemorySemanticScore {
                record_id: MemoryRecordId::from("a"),
                score: 0.5,
            },
            MemorySemanticScore {
                record_id: MemoryRecordId::from("b"),
                score: 0.5,
            },
        ];
        let estimates = BTreeMap::from([
            (MemoryRecordId::from("a"), 1),
            (MemoryRecordId::from("b"), 1),
            (MemoryRecordId::from("old"), 1),
        ]);
        let conversation = ConversationId("conversation".to_owned());
        let branch = ConversationBranchId("current".to_owned());
        let selection = MemoryEngine::select(
            &records,
            &profile(),
            &MemorySelectionContext {
                conversation_id: &conversation,
                branch_id: &branch,
                visible_message_ids: &visible,
                semantic_scores: &scores,
                token_estimates: &estimates,
            },
        )
        .expect("select");

        assert_eq!(
            selection
                .selected
                .iter()
                .map(|record| record.record_id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "old"]
        );
        assert_eq!(selection.selected[0].lane, MemorySelectionLane::Semantic);
    }

    #[test]
    fn pinned_memory_ignores_retrieval_count_but_not_the_token_budget() {
        let mut pinned = record("pinned", "current", "m1", "m1", 0);
        pinned.pinned = true;
        let ordinary = record("ordinary", "current", "m1", "m1", 100);
        let visible = vec![MessageId("m1".to_owned())];
        let estimates = BTreeMap::from([
            (MemoryRecordId::from("pinned"), 2),
            (MemoryRecordId::from("ordinary"), 1),
        ]);
        let mut profile = profile();
        profile.retrieval_count = 1;
        profile.episodic_budget.max_tokens = 3;
        let conversation = ConversationId("conversation".to_owned());
        let branch = ConversationBranchId("current".to_owned());
        let selection = MemoryEngine::select(
            &[ordinary, pinned],
            &profile,
            &MemorySelectionContext {
                conversation_id: &conversation,
                branch_id: &branch,
                visible_message_ids: &visible,
                semantic_scores: &[],
                token_estimates: &estimates,
            },
        )
        .expect("select");

        assert_eq!(selection.selected.len(), 2);
        assert_eq!(selection.selected[0].record_id.as_str(), "pinned");
        assert_eq!(selection.selected[0].lane, MemorySelectionLane::Pinned);
        assert_eq!(selection.selected[1].record_id.as_str(), "ordinary");
    }

    #[test]
    fn semantic_candidate_falls_back_to_episodic_budget() {
        let record = record("memory", "current", "m1", "m1", 1);
        let visible = vec![MessageId("m1".to_owned())];
        let scores = vec![MemorySemanticScore {
            record_id: MemoryRecordId::from("memory"),
            score: 0.9,
        }];
        let estimates = BTreeMap::from([(MemoryRecordId::from("memory"), 2)]);
        let mut profile = profile();
        profile.semantic_budget.max_tokens = 1;
        profile.episodic_budget.max_tokens = 2;
        let conversation = ConversationId("conversation".to_owned());
        let branch = ConversationBranchId("current".to_owned());
        let selection = MemoryEngine::select(
            &[record],
            &profile,
            &MemorySelectionContext {
                conversation_id: &conversation,
                branch_id: &branch,
                visible_message_ids: &visible,
                semantic_scores: &scores,
                token_estimates: &estimates,
            },
        )
        .expect("select");
        assert_eq!(selection.selected[0].lane, MemorySelectionLane::Episodic);
    }

    #[test]
    fn rewind_invalidation_is_branch_local_and_range_intersecting() {
        let records = vec![
            record("before", "current", "m1", "m1", 1),
            record("crossing", "current", "m1", "m3", 1),
            record("after", "current", "m3", "m4", 1),
            record("other-branch", "other", "m3", "m4", 1),
        ];
        let mut profile = profile();
        profile.preserve_invalidated_records = true;
        let actions = plan_memory_invalidation(
            &records,
            &profile,
            &ConversationId("conversation".to_owned()),
            &ConversationBranchId("current".to_owned()),
            &[
                MessageId("m1".to_owned()),
                MessageId("m2".to_owned()),
                MessageId("m3".to_owned()),
                MessageId("m4".to_owned()),
            ],
            &MessageId("m3".to_owned()),
        )
        .expect("plan");

        assert_eq!(
            actions
                .iter()
                .map(|action| action.record_id.as_str())
                .collect::<Vec<_>>(),
            vec!["after", "crossing"]
        );
        assert!(actions.iter().all(|action| {
            action.disposition == MemoryInvalidationDisposition::PreserveAsInvalidated
        }));
    }

    #[test]
    fn rewind_invalidation_requires_a_real_cut_point() {
        let error = plan_memory_invalidation(
            &[],
            &profile(),
            &ConversationId("conversation".to_owned()),
            &ConversationBranchId("current".to_owned()),
            &[MessageId("m1".to_owned())],
            &MessageId("missing".to_owned()),
        )
        .expect_err("missing cut");
        assert_eq!(error, MemoryInvalidationError::StartMessageNotFound);
    }

    fn job(key: &str) -> MemoryJob {
        MemoryJob {
            id: MemoryJobId::from("job"),
            idempotency_key: key.to_owned(),
            kind: MemoryJobKind::Summary,
            conversation_id: ConversationId("conversation".to_owned()),
            branch_id: ConversationBranchId("branch".to_owned()),
            source_start_message_id: MessageId("m1".to_owned()),
            source_end_message_id: MessageId("m2".to_owned()),
            status: MemoryJobStatus::Succeeded,
            attempt: 1,
            created_at: serde_json::from_value(timestamp()).expect("timestamp"),
            updated_at: serde_json::from_value(timestamp()).expect("timestamp"),
            error_code: None,
        }
    }

    fn task_profile(kind: AuxiliaryTaskKind) -> TaskProfile {
        TaskProfile {
            id: TaskProfileId::from("task"),
            kind,
            route_id: ModelRouteId::from("route"),
            generation_preset_id: GenerationPresetId::from("preset"),
            fallback_route_ids: Vec::new(),
            timeout_ms: 30_000,
            rate_limit: RateLimit {
                requests: 3,
                per_seconds: 60,
            },
            concurrency_limit: 2,
            embedding_dimensions: (kind == AuxiliaryTaskKind::MemoryEmbedding).then_some(3),
        }
    }

    fn request(key: &str) -> MemoryJobRequest {
        MemoryJobRequest {
            idempotency_key: key.to_owned(),
            kind: MemoryJobKind::Summary,
            conversation_id: ConversationId("conversation".to_owned()),
            branch_id: ConversationBranchId("branch".to_owned()),
            source_start_message_id: MessageId("m1".to_owned()),
            source_end_message_id: MessageId("m2".to_owned()),
        }
    }

    #[test]
    fn memory_job_key_is_stable_redacted_and_profile_sensitive() {
        let conversation = ConversationId("conversation".to_owned());
        let branch = ConversationBranchId("branch".to_owned());
        let start = MessageId("m1".to_owned());
        let end = MessageId("m2".to_owned());
        let profile = MemoryProfileId::from("profile");
        let input = |version| MemoryJobKeyInput {
            kind: MemoryJobKind::Summary,
            conversation_id: &conversation,
            branch_id: &branch,
            source_start_message_id: &start,
            source_end_message_id: &end,
            profile_id: Some(&profile),
            profile_schema_version: Some(version),
            source_revision: "canonical-digest",
        };
        let first = derive_memory_job_idempotency_key(&input(1)).expect("key");
        let again = derive_memory_job_idempotency_key(&input(1)).expect("key");
        let changed = derive_memory_job_idempotency_key(&input(2)).expect("key");
        assert_eq!(first, again);
        assert_ne!(first, changed);
        assert!(first.starts_with("memory-job:v1:"));
        assert!(!first.contains("conversation"));
        assert!(!first.contains("canonical-digest"));
    }

    #[test]
    fn job_enqueue_reuses_exact_work_and_rejects_conflicts() {
        let existing = job("key");
        assert!(matches!(
            decide_memory_job_enqueue(std::slice::from_ref(&existing), &request("key"))
                .expect("reuse"),
            MemoryJobEnqueueDecision::Reuse {
                status: MemoryJobStatus::Succeeded,
                ..
            }
        ));
        assert_eq!(
            decide_memory_job_enqueue(&[], &request("key")).expect("create"),
            MemoryJobEnqueueDecision::Create
        );
        let mut conflicting = request("key");
        conflicting.source_end_message_id = MessageId("different".to_owned());
        assert_eq!(
            decide_memory_job_enqueue(&[existing], &conflicting).expect_err("conflict"),
            MemoryJobError::IdempotencyConflict
        );
    }

    #[test]
    fn job_status_machine_allows_only_interrupted_explicit_retry() {
        assert!(
            validate_memory_job_status_transition(
                MemoryJobStatus::Queued,
                MemoryJobStatus::Running
            )
            .is_ok()
        );
        assert!(
            validate_memory_job_status_transition(
                MemoryJobStatus::Running,
                MemoryJobStatus::Interrupted
            )
            .is_ok()
        );
        assert!(
            validate_memory_job_status_transition(
                MemoryJobStatus::Interrupted,
                MemoryJobStatus::Queued
            )
            .is_ok()
        );
        assert_eq!(
            validate_memory_job_status_transition(MemoryJobStatus::Failed, MemoryJobStatus::Queued),
            Err(MemoryJobError::InvalidStatusTransition)
        );
    }

    #[test]
    fn dispatch_planner_honors_durable_rate_and_concurrency_limits() {
        let mut running = job("running");
        running.id = MemoryJobId::from("running");
        running.status = MemoryJobStatus::Running;
        let mut queued_b = job("queued-b");
        queued_b.id = MemoryJobId::from("b");
        queued_b.status = MemoryJobStatus::Queued;
        let mut queued_a = job("queued-a");
        queued_a.id = MemoryJobId::from("a");
        queued_a.status = MemoryJobStatus::Queued;

        let plan = plan_memory_job_dispatch(
            &[queued_b, running, queued_a],
            MemoryJobKind::Summary,
            &task_profile(AuxiliaryTaskKind::MemorySummary),
            1,
        )
        .expect("dispatch");

        assert_eq!(plan.running_jobs, 1);
        assert_eq!(plan.remaining_rate_slots, 2);
        assert_eq!(
            plan.start_job_ids
                .iter()
                .map(MemoryJobId::as_str)
                .collect::<Vec<_>>(),
            vec!["a"]
        );
        assert!(!plan.blocked_by_concurrency);
        assert!(!plan.blocked_by_rate_limit);
    }
}
