use std::{cmp::Reverse, collections::BTreeMap};

use lorepia_domain::{
    BlockResolutionStatus, BlockResolutionTrace, BlockSource, CacheDirectiveStatus, CharacterField,
    HistorySelector, InstructionAuthority, KnowledgeSelectionEvidence, MergePolicy, OverflowPolicy,
    OverflowTrace, PROMPT_PLAN_SCHEMA_VERSION, PlacementZone, PromptBlock, PromptBlockId,
    PromptBlockKind, PromptBlockSourceTrace, PromptConversationMessage,
    PromptMemorySelectionEvidence, PromptMemorySelectionLane, PromptMemorySelectionReason,
    PromptMessageRole, PromptPreset, PromptPreview, PromptResolutionTrace, PromptResolveRequest,
    Provenance, ProviderMessageRole, ResolvedCacheDirective, ResolvedPromptMessage,
    ResolvedPromptPlan, RoleHint, RoleMappingTrace, SourceKind, TemplateSlot,
    UnsupportedRolePolicy, ValidateOrchestration,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::template::{
    TemplateEnvironment, TemplateError, evaluate_condition, render_safe_template,
};

const MESSAGE_OVERHEAD_TOKENS: u32 = 4;
const BLOCK_CONTENT_SLOT: &str = "block_content";

/// Provider-independent token accounting used during deterministic planning.
pub trait TokenEstimator {
    fn id(&self) -> &'static str;
    fn estimate_text(&self, text: &str) -> u32;
    fn keep_prefix(&self, text: &str, max_tokens: u32) -> String;
    fn keep_suffix(&self, text: &str, max_tokens: u32) -> String;
}

/// Stable conservative approximation used before a provider tokenizer is
/// available. It counts UTF-8 bytes in four-byte token quanta.
#[derive(Debug, Clone, Copy, Default)]
pub struct Utf8TokenEstimator;

impl TokenEstimator for Utf8TokenEstimator {
    fn id(&self) -> &'static str {
        "utf8_bytes_div_4_v1"
    }

    fn estimate_text(&self, text: &str) -> u32 {
        if text.is_empty() {
            return 0;
        }
        u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX)
    }

    fn keep_prefix(&self, text: &str, max_tokens: u32) -> String {
        keep_utf8_prefix(text, max_token_bytes(max_tokens))
    }

    fn keep_suffix(&self, text: &str, max_tokens: u32) -> String {
        keep_utf8_suffix(text, max_token_bytes(max_tokens))
    }
}

fn max_token_bytes(max_tokens: u32) -> usize {
    usize::try_from(max_tokens)
        .unwrap_or(usize::MAX)
        .saturating_mul(4)
}

fn keep_utf8_prefix(text: &str, maximum_bytes: usize) -> String {
    if text.len() <= maximum_bytes {
        return text.to_owned();
    }
    let mut boundary = 0;
    for (index, character) in text.char_indices() {
        let end = index + character.len_utf8();
        if end > maximum_bytes {
            break;
        }
        boundary = end;
    }
    text[..boundary].to_owned()
}

fn keep_utf8_suffix(text: &str, maximum_bytes: usize) -> String {
    if text.len() <= maximum_bytes {
        return text.to_owned();
    }
    let minimum_start = text.len().saturating_sub(maximum_bytes);
    let start = text
        .char_indices()
        .map(|(index, _)| index)
        .find(|index| *index >= minimum_start)
        .unwrap_or(text.len());
    text[start..].to_owned()
}

#[derive(Debug, Error)]
pub enum OrchestrationError {
    #[error("invalid prompt input: {0}")]
    Invalid(String),
    #[error(transparent)]
    Template(#[from] TemplateError),
    #[error("block `{block_id}` cannot materialize: {reason}")]
    Materialization { block_id: String, reason: String },
    #[error("role `{role:?}` requested by block `{block_id}` is unsupported")]
    UnsupportedRole { block_id: String, role: RoleHint },
    #[error(
        "latest user message needs {required_tokens} tokens but only {available_tokens} are available"
    )]
    LatestUserMessageExceedsBudget {
        required_tokens: u32,
        available_tokens: u32,
    },
    #[error(
        "prompt remains over budget: {estimated_tokens} estimated tokens for {available_tokens}"
    )]
    OverflowRejected {
        estimated_tokens: u32,
        available_tokens: u32,
    },
    #[error("resolved plan serialization failed: {0}")]
    PlanSerialization(serde_json::Error),
    #[error("resolved plan hash does not match its canonical content")]
    PlanHashMismatch,
}

/// Validates a persisted preset, including fixed latest-user and prefill
/// placement constraints.
///
/// # Errors
///
/// Returns the first persisted-contract violation.
pub fn validate_prompt_preset(preset: &PromptPreset) -> Result<(), OrchestrationError> {
    preset
        .validate()
        .map_err(|error| OrchestrationError::Invalid(error.to_string()))
}

/// Resolves with `LorePia`'s stable built-in token approximation.
///
/// # Errors
///
/// Returns a validation, materialization, role-mapping, or budget error.
pub fn resolve_prompt_plan(
    request: &PromptResolveRequest,
) -> Result<ResolvedPromptPlan, OrchestrationError> {
    PromptResolver::new(Utf8TokenEstimator).resolve(request)
}

/// Produces a preview through the exact same materialization path as a request.
///
/// # Errors
///
/// Returns the same errors as [`resolve_prompt_plan`].
pub fn render_prompt_preview(
    request: &PromptResolveRequest,
) -> Result<PromptPreview, OrchestrationError> {
    resolve_prompt_plan(request).map(|plan| plan.preview)
}

/// Verifies both the materialized-plan invariants and its canonical SHA-256
/// binding before provider compilation or durable replay.
///
/// # Errors
///
/// Returns an error if the plan shape is invalid, serialization fails, or any
/// hash-bound field has been changed.
pub fn verify_resolved_prompt_plan(plan: &ResolvedPromptPlan) -> Result<(), OrchestrationError> {
    plan.validate()
        .map_err(|error| OrchestrationError::Invalid(error.to_string()))?;
    if canonical_plan_hash(plan)? != plan.plan_hash {
        return Err(OrchestrationError::PlanHashMismatch);
    }
    Ok(())
}

/// Replaces only the effective message contents of a verified plan and
/// recomputes every derived token, preview, trace, and hash field.
///
/// This is the safe boundary for the opt-in `ResolvedPrompt` transform phase:
/// callers cannot reseal arbitrary role, provenance, cache, or identity
/// changes because those fields are cloned from the verified original plan.
///
/// # Errors
///
/// Returns an error if the original plan is invalid or tampered, the
/// transformed message count differs, any transformed message is empty, or the
/// transformed plan exceeds its already-reviewed input budget.
pub fn reseal_resolved_prompt_plan(
    original: &ResolvedPromptPlan,
    transformed_contents: &[String],
) -> Result<ResolvedPromptPlan, OrchestrationError> {
    reseal_resolved_prompt_plan_with_estimator(original, transformed_contents, &Utf8TokenEstimator)
}

/// Estimator-specific variant of [`reseal_resolved_prompt_plan`].
///
/// The supplied estimator identity must exactly match the estimator recorded
/// by the original plan.
///
/// # Errors
///
/// Returns the same errors as [`reseal_resolved_prompt_plan`], plus an
/// estimator-identity mismatch.
pub fn reseal_resolved_prompt_plan_with_estimator<E: TokenEstimator>(
    original: &ResolvedPromptPlan,
    transformed_contents: &[String],
    estimator: &E,
) -> Result<ResolvedPromptPlan, OrchestrationError> {
    verify_resolved_prompt_plan(original)?;
    if original.trace.estimator_id != estimator.id() {
        return Err(OrchestrationError::Invalid(format!(
            "resolved plan estimator `{}` does not match reseal estimator `{}`",
            original.trace.estimator_id,
            estimator.id()
        )));
    }
    if transformed_contents.len() != original.effective_messages.len() {
        return Err(OrchestrationError::Invalid(
            "resolved prompt transform must preserve the exact message count".to_owned(),
        ));
    }
    if original
        .effective_messages
        .iter()
        .zip(transformed_contents)
        .any(|(message, content)| {
            content != &message.content
                && (message.authority == InstructionAuthority::Application
                    || message.block_kind == PromptBlockKind::LatestUserTurn)
        })
    {
        return Err(OrchestrationError::Invalid(
            "resolved prompt transform cannot change application or latest-user content".to_owned(),
        ));
    }

    let mut plan = original.clone();
    for (message, content) in plan.effective_messages.iter_mut().zip(transformed_contents) {
        if content.is_empty() {
            return Err(OrchestrationError::Invalid(
                "resolved prompt transform cannot produce an empty message".to_owned(),
            ));
        }
        message.content.clone_from(content);
        message.estimated_tokens = estimator
            .estimate_text(content)
            .saturating_add(MESSAGE_OVERHEAD_TOKENS);
    }

    let latest_user_tokens = plan
        .effective_messages
        .iter()
        .filter(|message| message.block_kind == PromptBlockKind::LatestUserTurn)
        .map(|message| message.estimated_tokens)
        .fold(0_u32, u32::saturating_add);
    if latest_user_tokens > plan.trace.available_input_tokens {
        return Err(OrchestrationError::LatestUserMessageExceedsBudget {
            required_tokens: latest_user_tokens,
            available_tokens: plan.trace.available_input_tokens,
        });
    }
    let estimated_input_tokens = plan
        .effective_messages
        .iter()
        .map(|message| message.estimated_tokens)
        .fold(0_u32, u32::saturating_add);
    if estimated_input_tokens > plan.trace.available_input_tokens {
        return Err(OrchestrationError::OverflowRejected {
            estimated_tokens: estimated_input_tokens,
            available_tokens: plan.trace.available_input_tokens,
        });
    }

    let block_tokens = plan.effective_messages.iter().fold(
        BTreeMap::<PromptBlockId, u32>::new(),
        |mut totals, message| {
            totals
                .entry(message.block_id.clone())
                .and_modify(|total| *total = total.saturating_add(message.estimated_tokens))
                .or_insert(message.estimated_tokens);
            totals
        },
    );
    for block in &mut plan.trace.blocks {
        block.final_estimated_tokens = block_tokens.get(&block.block_id).copied().unwrap_or(0);
    }
    plan.trace.estimated_input_tokens = estimated_input_tokens;
    plan.preview
        .effective_messages
        .clone_from(&plan.effective_messages);
    plan.preview
        .cache_directives
        .clone_from(&plan.cache_directives);
    plan.preview.estimated_input_tokens = estimated_input_tokens;
    plan.preview.available_input_tokens = plan.trace.available_input_tokens;
    plan.plan_hash = canonical_plan_hash(&plan)?;
    verify_resolved_prompt_plan(&plan)?;
    Ok(plan)
}

/// Adds Core-owned immutable source revisions and complete memory-selection
/// evidence to an already resolved plan, then recomputes the canonical hash.
///
/// The function cannot alter prompt content, roles, token accounting, or
/// selected memory identities. Evidence for records removed by the prompt
/// budget is retained as excluded, which keeps preview explanations complete
/// without leaking record content.
///
/// # Errors
///
/// Returns an error when the original plan is invalid, a revision references
/// an unknown block, selected memory evidence is missing, or the resealed plan
/// violates its bounded trace contract.
pub fn reseal_prompt_resolution_evidence(
    original: &ResolvedPromptPlan,
    source_revisions: &BTreeMap<PromptBlockId, String>,
    memory_evidence: &[PromptMemorySelectionEvidence],
) -> Result<ResolvedPromptPlan, OrchestrationError> {
    verify_resolved_prompt_plan(original)?;
    let mut plan = original.clone();
    for (block_id, revision) in source_revisions {
        let trace = plan
            .trace
            .blocks
            .iter_mut()
            .find(|trace| trace.block_id == *block_id)
            .ok_or_else(|| {
                OrchestrationError::Invalid(format!(
                    "source revision references unknown prompt block `{}`",
                    block_id.as_str()
                ))
            })?;
        trace.source.source_revision = Some(revision.clone());
    }

    for trace in &mut plan.trace.blocks {
        if trace.block_kind != PromptBlockKind::RetrievedMemory {
            continue;
        }
        let selected_ids = trace
            .memory_record_ids
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if selected_ids.iter().any(|record_id| {
            !memory_evidence
                .iter()
                .any(|item| &item.record_id == record_id)
        }) {
            return Err(OrchestrationError::Invalid(
                "memory trace selection has no matching engine evidence".to_owned(),
            ));
        }
        trace.memory_evidence = memory_evidence
            .iter()
            .cloned()
            .map(|mut evidence| {
                if !selected_ids.contains(&evidence.record_id) {
                    evidence.selected = false;
                    evidence.lane = None;
                    if evidence.exclusion_reason.is_none() {
                        evidence.exclusion_reason =
                            Some("removed by the prompt token budget".to_owned());
                    }
                }
                evidence
            })
            .collect();
    }
    plan.plan_hash = canonical_plan_hash(&plan)?;
    verify_resolved_prompt_plan(&plan)?;
    Ok(plan)
}

#[derive(Debug, Clone)]
pub struct PromptResolver<E> {
    estimator: E,
}

impl<E> PromptResolver<E>
where
    E: TokenEstimator,
{
    pub fn new(estimator: E) -> Self {
        Self { estimator }
    }

    /// Creates a deterministic provider-neutral plan.
    ///
    /// # Errors
    ///
    /// Returns an explicit error rather than silently dropping required
    /// content or the latest user turn.
    #[allow(clippy::too_many_lines)]
    pub fn resolve(
        &self,
        request: &PromptResolveRequest,
    ) -> Result<ResolvedPromptPlan, OrchestrationError> {
        request
            .validate()
            .map_err(|error| OrchestrationError::Invalid(error.to_string()))?;

        let available_tokens = request
            .max_context_tokens
            .saturating_sub(request.reserved_output_tokens);
        let mut blocks = request
            .preset
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| self.materialize_block(request, block, index))
            .collect::<Result<Vec<_>, _>>()?;
        blocks.sort_by(|left, right| {
            left.block
                .placement_zone
                .cmp(&right.block.placement_zone)
                .then_with(|| left.original_index.cmp(&right.original_index))
                .then_with(|| left.block.id.cmp(&right.block.id))
        });

        for block in &mut blocks {
            self.enforce_block_maximum(block, request)?;
        }

        let latest_tokens = blocks
            .iter()
            .filter(|block| block.block.kind == PromptBlockKind::LatestUserTurn)
            .map(|block| block.estimated_tokens_with(&self.estimator))
            .sum::<u32>();
        if latest_tokens > available_tokens {
            return Err(OrchestrationError::LatestUserMessageExceedsBudget {
                required_tokens: latest_tokens,
                available_tokens,
            });
        }

        self.enforce_global_budget(&mut blocks, request, available_tokens)?;

        let mut role_mappings = Vec::new();
        let mut effective_messages = Vec::new();
        for block in &blocks {
            for draft in &block.messages {
                let (effective_role, explanation) =
                    map_role(draft.requested_role, draft.authority, &request.provider).map_err(
                        |role| OrchestrationError::UnsupportedRole {
                            block_id: block.block.id.0.clone(),
                            role,
                        },
                    )?;
                role_mappings.push(RoleMappingTrace {
                    block_id: block.block.id.clone(),
                    requested_role: draft.requested_role,
                    effective_role,
                    explanation,
                });
                let sequence = u32::try_from(effective_messages.len())
                    .map_err(|_| OrchestrationError::Invalid("too many messages".into()))?;
                effective_messages.push(ResolvedPromptMessage {
                    sequence,
                    block_id: block.block.id.clone(),
                    block_kind: block.block.kind,
                    requested_role: draft.requested_role,
                    effective_role,
                    authority: draft.authority,
                    content: draft.content.clone(),
                    estimated_tokens: self.estimate_message(&draft.content),
                    source_message_ids: draft.source_message_ids.clone(),
                    provenance: draft.provenance.clone(),
                });
            }
        }

        if !effective_messages.iter().any(|message| {
            message.block_kind == PromptBlockKind::LatestUserTurn
                && message.effective_role == ProviderMessageRole::User
                && !message.content.is_empty()
        }) {
            return Err(OrchestrationError::Invalid(
                "resolved plan does not contain the latest user message".into(),
            ));
        }

        let (cache_directives, cache_warnings) =
            resolve_cache_directives(request, &effective_messages);
        let estimated_input_tokens = effective_messages
            .iter()
            .map(|message| message.estimated_tokens)
            .sum();
        let trace = PromptResolutionTrace {
            estimator_id: self.estimator.id().to_owned(),
            session_seed: request.context.session_seed,
            context_snapshot: request.context.context_snapshot.clone(),
            max_context_tokens: request.max_context_tokens,
            reserved_output_tokens: request.reserved_output_tokens,
            available_input_tokens: available_tokens,
            estimated_input_tokens,
            blocks: blocks
                .iter()
                .map(|block| block.trace(&self.estimator))
                .collect(),
            role_mappings,
            overflow: blocks
                .iter()
                .flat_map(|block| block.overflow_trace.iter().cloned())
                .collect(),
            warnings: cache_warnings,
        };
        let preview = PromptPreview {
            effective_messages: effective_messages.clone(),
            cache_directives: cache_directives.clone(),
            estimated_input_tokens,
            available_input_tokens: available_tokens,
        };
        let mut plan = ResolvedPromptPlan {
            schema_version: PROMPT_PLAN_SCHEMA_VERSION,
            preset_id: request.preset.id.clone(),
            generation_preset_id: request.generation_preset_id.clone(),
            effective_messages,
            cache_directives,
            trace,
            preview,
            plan_hash: String::new(),
        };
        plan.plan_hash = canonical_plan_hash(&plan)?;
        Ok(plan)
    }

    fn materialize_block(
        &self,
        request: &PromptResolveRequest,
        block: &PromptBlock,
        original_index: usize,
    ) -> Result<MaterializedBlock, OrchestrationError> {
        let mut materialized = MaterializedBlock::empty(block.clone(), original_index);
        if !block.enabled {
            materialized.status = BlockResolutionStatus::Disabled;
            materialized.explanation = "block is disabled".into();
            return Ok(materialized);
        }
        if let Some(condition) = &block.condition
            && !evaluate_condition(
                condition,
                &request.context.variables,
                &request.context.supported_capabilities,
            )?
        {
            materialized.status = BlockResolutionStatus::ConditionFalse;
            materialized.explanation = "block condition evaluated to false".into();
            return Ok(materialized);
        }

        materialized.messages = self.materialize_source(request, block, &mut materialized)?;
        if materialized.messages.is_empty() {
            materialized.status = BlockResolutionStatus::Empty;
            materialized.explanation = "block source produced no content".into();
            return Ok(materialized);
        }
        Self::apply_block_template(request, block, &mut materialized.messages)?;
        materialized
            .messages
            .retain(|message| !message.content.is_empty());
        if materialized.messages.is_empty() {
            materialized.status = BlockResolutionStatus::Empty;
            materialized.explanation = "rendered block was empty".into();
            return Ok(materialized);
        }
        if block.merge_policy == MergePolicy::MergeWithPreviousSameRole {
            merge_same_role_messages(&mut materialized.messages);
        }
        materialized.original_tokens = materialized.estimated_tokens_with(&self.estimator);
        materialized.explanation = "block included".into();
        Ok(materialized)
    }

    #[allow(clippy::too_many_lines)]
    fn materialize_source(
        &self,
        request: &PromptResolveRequest,
        block: &PromptBlock,
        materialized: &mut MaterializedBlock,
    ) -> Result<Vec<DraftMessage>, OrchestrationError> {
        let context = &request.context;
        let default = |content: String| {
            vec![DraftMessage {
                requested_role: role_for_block(block),
                authority: block.authority,
                content,
                source_message_ids: Vec::new(),
                source_memory_record_ids: Vec::new(),
                provenance: block.provenance.clone(),
            }]
        };
        match &block.source {
            BlockSource::Template => {
                let template =
                    block
                        .template
                        .as_ref()
                        .ok_or_else(|| OrchestrationError::Materialization {
                            block_id: block.id.0.clone(),
                            reason: "template source requires a template".into(),
                        })?;
                let environment = template_environment(context, &context.slots);
                Ok(default(render_safe_template(template, &environment)?))
            }
            BlockSource::CharacterField { field } => Ok(default(character_field(context, *field))),
            BlockSource::History => Self::history_messages(request, block),
            BlockSource::LatestUser => {
                let latest = context
                    .messages
                    .iter()
                    .find(|message| message.id == context.latest_user_message_id)
                    .ok_or_else(|| OrchestrationError::Materialization {
                        block_id: block.id.0.clone(),
                        reason: "latest user message is missing".into(),
                    })?;
                Ok(vec![DraftMessage {
                    requested_role: RoleHint::User,
                    authority: block.authority,
                    content: latest.content.clone(),
                    source_message_ids: vec![latest.id.clone()],
                    source_memory_record_ids: Vec::new(),
                    provenance: block.provenance.clone(),
                }])
            }
            BlockSource::SelectedKnowledge => {
                let mut entries = context
                    .selected_knowledge
                    .iter()
                    .filter(|entry| knowledge_matches_zone(entry.placement, block.placement_zone))
                    .collect::<Vec<_>>();
                entries.sort_by(|left, right| {
                    right
                        .priority
                        .cmp(&left.priority)
                        .then_with(|| left.entry_id.cmp(&right.entry_id))
                });
                for entry in &entries {
                    materialized
                        .knowledge_evidence
                        .push(KnowledgeSelectionEvidence {
                            entry_id: entry.entry_id.clone(),
                            selected: true,
                            reasons: entry.evidence.clone(),
                            estimated_tokens: self.estimator.estimate_text(&entry.content),
                            exclusion_reason: None,
                        });
                }
                Ok(entries
                    .into_iter()
                    .map(|entry| DraftMessage {
                        requested_role: role_for_block(block),
                        authority: authority_for_provenance(block.authority, &entry.provenance),
                        content: entry.content.clone(),
                        source_message_ids: Vec::new(),
                        source_memory_record_ids: Vec::new(),
                        provenance: entry.provenance.clone(),
                    })
                    .collect())
            }
            BlockSource::SelectedMemory => {
                let mut records = context.selected_memory.iter().collect::<Vec<_>>();
                records.sort_by(|left, right| {
                    right
                        .score_millionths
                        .cmp(&left.score_millionths)
                        .then_with(|| left.record_id.cmp(&right.record_id))
                });
                materialized.memory_evidence = records
                    .iter()
                    .map(|record| {
                        let reasons = serde_json::from_str::<Vec<PromptMemorySelectionReason>>(
                            &record.reason,
                        )
                        .unwrap_or_default();
                        let lane = if reasons
                            .iter()
                            .any(|reason| matches!(reason, PromptMemorySelectionReason::Pinned))
                        {
                            PromptMemorySelectionLane::Pinned
                        } else if reasons.iter().any(|reason| {
                            matches!(reason, PromptMemorySelectionReason::Similarity { .. })
                        }) {
                            PromptMemorySelectionLane::Semantic
                        } else {
                            PromptMemorySelectionLane::Episodic
                        };
                        PromptMemorySelectionEvidence {
                            record_id: record.record_id.clone(),
                            selected: true,
                            lane: Some(lane),
                            rank_millionths: Some(u64::from(record.score_millionths)),
                            estimated_tokens: self.estimator.estimate_text(&record.content),
                            reasons,
                            exclusion_reason: None,
                        }
                    })
                    .collect();
                Ok(records
                    .into_iter()
                    .map(|record| DraftMessage {
                        requested_role: role_for_block(block),
                        authority: authority_for_provenance(block.authority, &record.provenance),
                        content: record.content.clone(),
                        source_message_ids: Vec::new(),
                        source_memory_record_ids: vec![record.record_id.clone()],
                        provenance: record.provenance.clone(),
                    })
                    .collect())
            }
            BlockSource::ConversationSummary => context
                .conversation_summary
                .clone()
                .map(default)
                .ok_or_else(|| OrchestrationError::Materialization {
                    block_id: block.id.0.clone(),
                    reason: "conversation summary source is unavailable at this branch head"
                        .to_owned(),
                }),
            BlockSource::AuthorNote => context.author_note.clone().map(default).ok_or_else(|| {
                OrchestrationError::Materialization {
                    block_id: block.id.0.clone(),
                    reason: "author note source is unavailable for this room".to_owned(),
                }
            }),
            BlockSource::UserPersona => {
                Ok(context.persona.as_ref().map_or_else(Vec::new, |persona| {
                    default(format!("{}\n{}", persona.name, persona.description))
                }))
            }
            BlockSource::GroupContext => {
                context.group_context.clone().map(default).ok_or_else(|| {
                    OrchestrationError::Materialization {
                        block_id: block.id.0.clone(),
                        reason: "group context source is unavailable for this room".to_owned(),
                    }
                })
            }
        }
    }

    fn history_messages(
        request: &PromptResolveRequest,
        block: &PromptBlock,
    ) -> Result<Vec<DraftMessage>, OrchestrationError> {
        let selector =
            block
                .history_selector
                .as_ref()
                .ok_or_else(|| OrchestrationError::Materialization {
                    block_id: block.id.0.clone(),
                    reason: "history source requires a history selector".into(),
                })?;
        let mut messages = request
            .context
            .messages
            .iter()
            .filter(|message| {
                message.branch_id == request.context.branch_id
                    && message.id != request.context.latest_user_message_id
            })
            .collect::<Vec<_>>();
        messages.sort_by(|left, right| {
            left.turn_index
                .cmp(&right.turn_index)
                .then_with(|| left.id.0.cmp(&right.id.0))
        });
        let selected = select_history(messages, selector, &request.context.summary_boundaries)
            .map_err(|reason| OrchestrationError::Materialization {
                block_id: block.id.0.clone(),
                reason,
            })?;
        Ok(selected
            .into_iter()
            .map(|message| DraftMessage {
                requested_role: match message.role {
                    PromptMessageRole::System => RoleHint::System,
                    PromptMessageRole::User => RoleHint::User,
                    PromptMessageRole::Assistant => RoleHint::Assistant,
                },
                authority: block.authority,
                content: message.content.clone(),
                source_message_ids: vec![message.id.clone()],
                source_memory_record_ids: Vec::new(),
                provenance: block.provenance.clone(),
            })
            .collect())
    }

    fn apply_block_template(
        request: &PromptResolveRequest,
        block: &PromptBlock,
        messages: &mut [DraftMessage],
    ) -> Result<(), OrchestrationError> {
        if block.source == BlockSource::Template {
            return Ok(());
        }
        let Some(template) = &block.template else {
            return Ok(());
        };
        for message in messages {
            let mut slots = request
                .context
                .slots
                .iter()
                .filter(|slot| slot.name != BLOCK_CONTENT_SLOT)
                .cloned()
                .collect::<Vec<_>>();
            slots.push(TemplateSlot {
                name: BLOCK_CONTENT_SLOT.into(),
                value: message.content.clone(),
            });
            let environment = template_environment(&request.context, &slots);
            message.content = render_safe_template(template, &environment)?;
        }
        Ok(())
    }

    fn estimate_message(&self, content: &str) -> u32 {
        self.estimator
            .estimate_text(content)
            .saturating_add(MESSAGE_OVERHEAD_TOKENS)
    }

    fn enforce_block_maximum(
        &self,
        block: &mut MaterializedBlock,
        request: &PromptResolveRequest,
    ) -> Result<(), OrchestrationError> {
        let Some(maximum) = block.block.token_policy.max_tokens else {
            return Ok(());
        };
        let before = block.estimated_tokens_with(&self.estimator);
        if before <= maximum {
            return Ok(());
        }
        self.reduce_block(block, request, maximum)?;
        let after = block.estimated_tokens_with(&self.estimator);
        if after > maximum {
            return Err(OrchestrationError::OverflowRejected {
                estimated_tokens: after,
                available_tokens: maximum,
            });
        }
        block.overflow_trace.push(OverflowTrace {
            block_id: block.block.id.clone(),
            policy: block.block.overflow_policy,
            tokens_before: before,
            tokens_after: after,
            explanation: "applied the block maximum".into(),
        });
        Ok(())
    }

    fn enforce_global_budget(
        &self,
        blocks: &mut [MaterializedBlock],
        request: &PromptResolveRequest,
        available_tokens: u32,
    ) -> Result<(), OrchestrationError> {
        let mut total = total_tokens(blocks, &self.estimator);
        if total <= available_tokens {
            return Ok(());
        }

        let mut candidates = blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| block.block.kind != PromptBlockKind::LatestUserTurn)
            .map(|(index, block)| {
                (
                    reduction_phase(&block.block),
                    block.block.token_policy.priority,
                    Reverse(block.original_index),
                    index,
                )
            })
            .collect::<Vec<_>>();
        candidates.sort();

        for (_, _, _, index) in candidates {
            if total <= available_tokens {
                break;
            }
            let before = blocks[index].estimated_tokens_with(&self.estimator);
            if before == 0 || blocks[index].block.overflow_policy == OverflowPolicy::Reject {
                continue;
            }
            let needed = total.saturating_sub(available_tokens);
            let protected = blocks[index]
                .block
                .token_policy
                .min_tokens
                .unwrap_or(0)
                .max(blocks[index].block.token_policy.reserve_tokens.unwrap_or(0));
            let target = before.saturating_sub(needed).max(protected);
            self.reduce_block(&mut blocks[index], request, target)?;
            let after = blocks[index].estimated_tokens_with(&self.estimator);
            blocks[index].overflow_trace.push(OverflowTrace {
                block_id: blocks[index].block.id.clone(),
                policy: blocks[index].block.overflow_policy,
                tokens_before: before,
                tokens_after: after,
                explanation: "applied the global context budget".into(),
            });
            total = total.saturating_sub(before.saturating_sub(after));
        }

        if total > available_tokens {
            return Err(OrchestrationError::OverflowRejected {
                estimated_tokens: total,
                available_tokens,
            });
        }
        Ok(())
    }

    fn reduce_block(
        &self,
        block: &mut MaterializedBlock,
        request: &PromptResolveRequest,
        target_tokens: u32,
    ) -> Result<(), OrchestrationError> {
        let before = block.estimated_tokens_with(&self.estimator);
        if before <= target_tokens {
            return Ok(());
        }
        match block.block.overflow_policy {
            OverflowPolicy::Reject => {
                return Err(OrchestrationError::OverflowRejected {
                    estimated_tokens: before,
                    available_tokens: target_tokens,
                });
            }
            OverflowPolicy::DropBlock => {
                block.messages.clear();
                block.status = BlockResolutionStatus::DroppedForBudget;
                block.explanation = "optional block dropped for token budget".into();
            }
            OverflowPolicy::TrimHead => {
                trim_messages(
                    &mut block.messages,
                    target_tokens,
                    TrimDirection::Head,
                    &self.estimator,
                );
                block.status = BlockResolutionStatus::TrimmedHead;
                block.explanation = "oldest text trimmed for token budget".into();
            }
            OverflowPolicy::TrimTail => {
                trim_messages(
                    &mut block.messages,
                    target_tokens,
                    TrimDirection::Tail,
                    &self.estimator,
                );
                block.status = BlockResolutionStatus::TrimmedTail;
                block.explanation = "trailing text trimmed for token budget".into();
            }
            OverflowPolicy::KeepLatestItems => {
                keep_latest_items(&mut block.messages, target_tokens, &self.estimator);
                block.status = BlockResolutionStatus::ReducedItems;
                block.explanation = "oldest history items removed for token budget".into();
            }
            OverflowPolicy::Summarize => {
                let Some(summary) = request.context.conversation_summary.as_ref() else {
                    return Err(OrchestrationError::Materialization {
                        block_id: block.block.id.0.clone(),
                        reason: "summarize overflow requires a precomputed summary".into(),
                    });
                };
                let summary_tokens = self.estimate_message(summary);
                if summary_tokens >= before || summary_tokens > target_tokens {
                    return Err(OrchestrationError::OverflowRejected {
                        estimated_tokens: before,
                        available_tokens: target_tokens,
                    });
                }
                block.messages = vec![DraftMessage {
                    requested_role: role_for_block(&block.block),
                    authority: block.block.authority,
                    content: summary.clone(),
                    source_message_ids: block
                        .messages
                        .iter()
                        .flat_map(|message| message.source_message_ids.iter().cloned())
                        .collect(),
                    source_memory_record_ids: Vec::new(),
                    provenance: block.block.provenance.clone(),
                }];
                block.status = BlockResolutionStatus::Summarized;
                block.explanation = "raw history replaced by a precomputed summary".into();
            }
            OverflowPolicy::ReduceKnowledgeEntries => {
                while block.messages.len() > 1
                    && block.estimated_tokens_with(&self.estimator) > target_tokens
                {
                    block.messages.pop();
                    if let Some(evidence) = block
                        .knowledge_evidence
                        .iter_mut()
                        .rev()
                        .find(|evidence| evidence.selected)
                    {
                        evidence.selected = false;
                        evidence.exclusion_reason =
                            Some("removed by the prompt token budget".into());
                    }
                }
                if block.estimated_tokens_with(&self.estimator) > target_tokens {
                    return Err(OrchestrationError::OverflowRejected {
                        estimated_tokens: block.estimated_tokens_with(&self.estimator),
                        available_tokens: target_tokens,
                    });
                }
                block.status = BlockResolutionStatus::ReducedItems;
                block.explanation = "lower-ranked knowledge entries removed".into();
            }
        }
        Ok(())
    }
}

fn template_environment<'a>(
    context: &'a lorepia_domain::PromptResolutionContext,
    slots: &'a [TemplateSlot],
) -> TemplateEnvironment<'a> {
    TemplateEnvironment {
        variables: &context.variables,
        capabilities: &context.supported_capabilities,
        character_name: &context.character.name,
        user_name: &context.user_name,
        persona_name: context
            .persona
            .as_ref()
            .map(|persona| persona.name.as_str()),
        persona_description: context
            .persona
            .as_ref()
            .map(|persona| persona.description.as_str()),
        current_date: &context.current_date,
        current_time: &context.current_time,
        slots,
    }
}

fn role_for_block(block: &PromptBlock) -> RoleHint {
    if block.kind == PromptBlockKind::AssistantPrefill {
        RoleHint::Assistant
    } else if block.kind == PromptBlockKind::LatestUserTurn {
        RoleHint::User
    } else {
        block.role_hint
    }
}

fn character_field(
    context: &lorepia_domain::PromptResolutionContext,
    field: CharacterField,
) -> String {
    match field {
        CharacterField::Name => {
            if context.character.aliases.is_empty() {
                context.character.name.clone()
            } else {
                format!(
                    "{}\nAliases: {}",
                    context.character.name,
                    context.character.aliases.join(", ")
                )
            }
        }
        CharacterField::Description => context.character.description.clone(),
        CharacterField::Personality => context.character.personality.clone(),
        CharacterField::Scenario => context.character.scenario.clone(),
        CharacterField::FirstMessage => context.character.first_message.clone(),
        CharacterField::DialogueExamples => context.character.dialogue_examples.join("\n\n"),
        CharacterField::SystemInstruction => context.character.system_instruction.clone(),
        CharacterField::PostHistoryInstruction => {
            context.character.post_history_instruction.clone()
        }
    }
}

fn knowledge_matches_zone(
    placement: lorepia_domain::KnowledgePlacement,
    zone: PlacementZone,
) -> bool {
    use lorepia_domain::KnowledgePlacement;
    matches!(
        (placement, zone),
        (
            KnowledgePlacement::RetrievedContext,
            PlacementZone::RetrievedContext
        ) | (
            KnowledgePlacement::BeforeOlderHistory,
            PlacementZone::OlderHistory
        ) | (
            KnowledgePlacement::BeforeRecentHistory,
            PlacementZone::RecentEnhancement
        ) | (KnowledgePlacement::PostHistory, PlacementZone::PostHistory)
    )
}

fn select_history<'a>(
    messages: Vec<&'a PromptConversationMessage>,
    selector: &HistorySelector,
    summaries: &[lorepia_domain::SummaryBoundary],
) -> Result<Vec<&'a PromptConversationMessage>, String> {
    let turn_indices = || {
        let mut turns = messages
            .iter()
            .map(|message| message.turn_index)
            .collect::<Vec<_>>();
        turns.sort_unstable();
        turns.dedup();
        turns
    };
    match selector {
        HistorySelector::All => Ok(messages),
        HistorySelector::BeforeRecentTurns { recent_turns } => {
            let turns = turn_indices();
            let keep_before = turns
                .len()
                .saturating_sub(usize::try_from(*recent_turns).unwrap_or(usize::MAX));
            let recent_start = turns.get(keep_before).copied().unwrap_or(u32::MAX);
            Ok(messages
                .into_iter()
                .filter(|message| message.turn_index < recent_start)
                .collect())
        }
        HistorySelector::RecentTurns { count } => {
            let turns = turn_indices();
            let start = turns
                .len()
                .saturating_sub(usize::try_from(*count).unwrap_or(usize::MAX));
            let minimum = turns.get(start).copied().unwrap_or(u32::MAX);
            Ok(messages
                .into_iter()
                .filter(|message| message.turn_index >= minimum)
                .collect())
        }
        HistorySelector::ExcludingLatestUser { count } => {
            let turns = turn_indices();
            let start = turns
                .len()
                .saturating_sub(usize::try_from(*count).unwrap_or(usize::MAX));
            let minimum = turns.get(start).copied().unwrap_or(u32::MAX);
            Ok(messages
                .into_iter()
                .filter(|message| message.turn_index >= minimum)
                .collect())
        }
        HistorySelector::MessageRange { start, end } => {
            let start_index = messages
                .iter()
                .position(|message| message.id == *start)
                .ok_or_else(|| "history range start message is unavailable".to_owned())?;
            let end_index = messages
                .iter()
                .position(|message| message.id == *end)
                .ok_or_else(|| "history range end message is unavailable".to_owned())?;
            if start_index > end_index {
                return Err("history range start occurs after its end".into());
            }
            Ok(messages[start_index..=end_index].to_vec())
        }
        HistorySelector::SinceSummary { summary_id } => {
            let boundary = summaries
                .iter()
                .find(|boundary| boundary.summary_id == *summary_id)
                .ok_or_else(|| "summary boundary is unavailable".to_owned())?;
            let end_index = messages
                .iter()
                .position(|message| message.id == boundary.end_message_id)
                .ok_or_else(|| "summary end message is unavailable".to_owned())?;
            Ok(messages.into_iter().skip(end_index + 1).collect())
        }
    }
}

fn merge_same_role_messages(messages: &mut Vec<DraftMessage>) {
    let mut merged: Vec<DraftMessage> = Vec::with_capacity(messages.len());
    for message in messages.drain(..) {
        if let Some(previous) = merged.last_mut()
            && previous.requested_role == message.requested_role
            && previous.authority == message.authority
            && previous.provenance == message.provenance
        {
            previous.content.push_str("\n\n");
            previous.content.push_str(&message.content);
            previous
                .source_message_ids
                .extend(message.source_message_ids);
            previous
                .source_memory_record_ids
                .extend(message.source_memory_record_ids);
            continue;
        }
        merged.push(message);
    }
    *messages = merged;
}

fn map_role(
    requested: RoleHint,
    authority: InstructionAuthority,
    contract: &lorepia_domain::ProviderPromptContract,
) -> Result<(ProviderMessageRole, String), RoleHint> {
    if authority == InstructionAuthority::ImportedContent
        && matches!(requested, RoleHint::System | RoleHint::Developer)
    {
        return contract
            .supported_roles
            .contains(&ProviderMessageRole::User)
            .then(|| {
                (
                    ProviderMessageRole::User,
                    "imported content privileged role downgraded to user".into(),
                )
            })
            .ok_or(requested);
    }
    let desired = match requested {
        RoleHint::System => ProviderMessageRole::System,
        RoleHint::Developer => ProviderMessageRole::Developer,
        RoleHint::User => ProviderMessageRole::User,
        RoleHint::Assistant => ProviderMessageRole::Assistant,
        RoleHint::ProviderDefault => {
            return Ok((
                contract.provider_default_role,
                "provider-default role selected".into(),
            ));
        }
    };
    if contract.supported_roles.contains(&desired) {
        return Ok((desired, "requested role is supported".into()));
    }
    match (desired, contract.unsupported_role_policy) {
        (ProviderMessageRole::Developer, UnsupportedRolePolicy::MapDeveloperToSystem)
            if contract
                .supported_roles
                .contains(&ProviderMessageRole::System) =>
        {
            Ok((
                ProviderMessageRole::System,
                "developer role mapped to system because the provider lacks developer".into(),
            ))
        }
        (ProviderMessageRole::System, UnsupportedRolePolicy::MapSystemToDeveloper)
            if contract
                .supported_roles
                .contains(&ProviderMessageRole::Developer) =>
        {
            Ok((
                ProviderMessageRole::Developer,
                "system role mapped to developer because the provider lacks system".into(),
            ))
        }
        (_, UnsupportedRolePolicy::UseProviderDefault) => Ok((
            contract.provider_default_role,
            "unsupported role mapped to the provider default".into(),
        )),
        _ => Err(requested),
    }
}

fn resolve_cache_directives(
    request: &PromptResolveRequest,
    messages: &[ResolvedPromptMessage],
) -> (Vec<ResolvedCacheDirective>, Vec<String>) {
    let mut applied_count = 0_u32;
    let mut warnings = Vec::new();
    let directives = request
        .preset
        .cache_boundaries
        .iter()
        .map(|boundary| {
            let after_message_sequence = messages
                .iter()
                .rev()
                .find(|message| message.block_id == boundary.after_block_id)
                .map(|message| message.sequence);
            let (status, explanation) = if after_message_sequence.is_none() {
                (
                    CacheDirectiveStatus::RemovedWithBlock,
                    "the referenced block produced no final message".to_owned(),
                )
            } else if boundary.mode == lorepia_domain::CacheMode::Automatic {
                (
                    CacheDirectiveStatus::Applied,
                    "provider automatic cache policy retained".to_owned(),
                )
            } else if !request.provider.supports_explicit_cache {
                let warning = format!(
                    "cache boundary `{}` ignored because the provider lacks explicit caching",
                    boundary.id.0
                );
                warnings.push(warning.clone());
                (CacheDirectiveStatus::IgnoredUnsupported, warning)
            } else if applied_count >= request.provider.max_cache_boundaries {
                let warning = format!(
                    "cache boundary `{}` ignored because the provider limit was reached",
                    boundary.id.0
                );
                warnings.push(warning.clone());
                (CacheDirectiveStatus::IgnoredLimit, warning)
            } else {
                applied_count = applied_count.saturating_add(1);
                (
                    CacheDirectiveStatus::Applied,
                    "explicit provider cache boundary applied".to_owned(),
                )
            };
            ResolvedCacheDirective {
                boundary_id: boundary.id.clone(),
                after_block_id: boundary.after_block_id.clone(),
                after_message_sequence,
                role_filter: boundary.role_filter,
                ttl: boundary.ttl,
                mode: boundary.mode,
                status,
                explanation,
            }
        })
        .collect();
    (directives, warnings)
}

fn reduction_phase(block: &PromptBlock) -> u8 {
    match (block.kind, block.overflow_policy) {
        (_, OverflowPolicy::DropBlock) => 0,
        (PromptBlockKind::WorldKnowledge, _) | (_, OverflowPolicy::ReduceKnowledgeEntries) => 1,
        (PromptBlockKind::HistorySlice, OverflowPolicy::Summarize) => 2,
        (PromptBlockKind::DialogueExamples, _) => 3,
        (PromptBlockKind::HistorySlice, _) => 4,
        _ => 5,
    }
}

fn total_tokens<E: TokenEstimator>(blocks: &[MaterializedBlock], estimator: &E) -> u32 {
    blocks
        .iter()
        .map(|block| block.estimated_tokens_with(estimator))
        .fold(0_u32, u32::saturating_add)
}

#[derive(Debug, Clone)]
struct DraftMessage {
    requested_role: RoleHint,
    authority: InstructionAuthority,
    content: String,
    source_message_ids: Vec<lorepia_domain::MessageId>,
    source_memory_record_ids: Vec<lorepia_domain::MemoryRecordId>,
    provenance: Provenance,
}

fn authority_for_provenance(
    block_authority: InstructionAuthority,
    provenance: &Provenance,
) -> InstructionAuthority {
    if matches!(
        provenance.source_kind,
        SourceKind::ImportedStandard | SourceKind::ImportedPackage
    ) {
        InstructionAuthority::ImportedContent
    } else {
        block_authority
    }
}

#[derive(Debug, Clone)]
struct MaterializedBlock {
    block: PromptBlock,
    original_index: usize,
    messages: Vec<DraftMessage>,
    status: BlockResolutionStatus,
    original_tokens: u32,
    explanation: String,
    knowledge_evidence: Vec<KnowledgeSelectionEvidence>,
    memory_evidence: Vec<PromptMemorySelectionEvidence>,
    overflow_trace: Vec<OverflowTrace>,
}

impl MaterializedBlock {
    fn empty(block: PromptBlock, original_index: usize) -> Self {
        Self {
            block,
            original_index,
            messages: Vec::new(),
            status: BlockResolutionStatus::Included,
            original_tokens: 0,
            explanation: String::new(),
            knowledge_evidence: Vec::new(),
            memory_evidence: Vec::new(),
            overflow_trace: Vec::new(),
        }
    }

    fn estimated_tokens_with<E: TokenEstimator>(&self, estimator: &E) -> u32 {
        self.messages
            .iter()
            .map(|message| {
                estimator
                    .estimate_text(&message.content)
                    .saturating_add(MESSAGE_OVERHEAD_TOKENS)
            })
            .fold(0_u32, u32::saturating_add)
    }

    fn trace<E: TokenEstimator>(&self, estimator: &E) -> BlockResolutionTrace {
        let selected_memory_ids = self
            .messages
            .iter()
            .flat_map(|message| message.source_memory_record_ids.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        let mut memory_evidence = self.memory_evidence.clone();
        for evidence in &mut memory_evidence {
            if !selected_memory_ids.contains(&evidence.record_id) {
                evidence.selected = false;
                evidence.lane = None;
                evidence
                    .exclusion_reason
                    .get_or_insert_with(|| "removed by the prompt token budget".to_owned());
            }
        }
        BlockResolutionTrace {
            block_id: self.block.id.clone(),
            block_kind: self.block.kind,
            source: PromptBlockSourceTrace {
                authority: self.block.authority,
                source_kind: self.block.provenance.source_kind.clone(),
                source_id: self.block.provenance.source_id.clone(),
                source_revision: None,
                source_hash: self.block.provenance.source_hash.clone(),
            },
            status: self.status,
            original_estimated_tokens: self.original_tokens,
            final_estimated_tokens: self.estimated_tokens_with(estimator),
            produced_message_count: u32::try_from(self.messages.len()).unwrap_or(u32::MAX),
            explanation: self.explanation.clone(),
            knowledge_evidence: self.knowledge_evidence.clone(),
            memory_record_ids: selected_memory_ids.into_iter().collect(),
            memory_evidence,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum TrimDirection {
    Head,
    Tail,
}

fn trim_messages<E: TokenEstimator>(
    messages: &mut Vec<DraftMessage>,
    target_tokens: u32,
    direction: TrimDirection,
    estimator: &E,
) {
    let mut ordered = match direction {
        TrimDirection::Head => messages.iter().rev().cloned().collect::<Vec<_>>(),
        TrimDirection::Tail => messages.clone(),
    };
    let mut retained = Vec::new();
    let mut remaining = target_tokens;
    for mut message in ordered.drain(..) {
        if remaining <= MESSAGE_OVERHEAD_TOKENS {
            break;
        }
        let message_tokens = estimator
            .estimate_text(&message.content)
            .saturating_add(MESSAGE_OVERHEAD_TOKENS);
        if message_tokens <= remaining {
            remaining = remaining.saturating_sub(message_tokens);
            retained.push(message);
            continue;
        }
        let text_budget = remaining.saturating_sub(MESSAGE_OVERHEAD_TOKENS);
        message.content = match direction {
            TrimDirection::Head => estimator.keep_suffix(&message.content, text_budget),
            TrimDirection::Tail => estimator.keep_prefix(&message.content, text_budget),
        };
        if !message.content.is_empty() {
            retained.push(message);
        }
        break;
    }
    if matches!(direction, TrimDirection::Head) {
        retained.reverse();
    }
    *messages = retained;
}

fn keep_latest_items<E: TokenEstimator>(
    messages: &mut Vec<DraftMessage>,
    target_tokens: u32,
    estimator: &E,
) {
    while !messages.is_empty()
        && messages
            .iter()
            .map(|message| {
                estimator
                    .estimate_text(&message.content)
                    .saturating_add(MESSAGE_OVERHEAD_TOKENS)
            })
            .sum::<u32>()
            > target_tokens
    {
        messages.remove(0);
    }
}

fn canonical_plan_hash(plan: &ResolvedPromptPlan) -> Result<String, OrchestrationError> {
    #[derive(Serialize)]
    struct HashMaterial<'a> {
        schema_version: u32,
        preset_id: &'a lorepia_domain::PromptPresetId,
        generation_preset_id: &'a Option<lorepia_domain::GenerationPresetId>,
        effective_messages: &'a [ResolvedPromptMessage],
        cache_directives: &'a [ResolvedCacheDirective],
        trace: &'a PromptResolutionTrace,
        preview: &'a PromptPreview,
    }
    let material = HashMaterial {
        schema_version: plan.schema_version,
        preset_id: &plan.preset_id,
        generation_preset_id: &plan.generation_preset_id,
        effective_messages: &plan.effective_messages,
        cache_directives: &plan.cache_directives,
        trace: &plan.trace,
        preview: &plan.preview,
    };
    let canonical = serde_json::to_vec(&material).map_err(OrchestrationError::PlanSerialization)?;
    Ok(hex::encode(Sha256::digest(canonical)))
}

/// Constructs the no-copy default pipeline used when a room has no custom
/// preset. Callers supply stable identifiers and provenance explicitly.
pub fn default_prompt_preset(
    id: lorepia_domain::PromptPresetId,
    name: impl Into<String>,
    metadata: lorepia_domain::PresetMetadata,
) -> PromptPreset {
    let provenance = Provenance {
        source_kind: SourceKind::ApplicationBuiltIn,
        source_id: Some("lorepia.default_prompt_v1".into()),
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    };
    let policy = |suffix: &str,
                  name: &str,
                  kind: PromptBlockKind,
                  source: BlockSource,
                  zone: PlacementZone,
                  role_hint: RoleHint,
                  overflow_policy: OverflowPolicy,
                  priority: u16| PromptBlock {
        id: PromptBlockId::from(format!("{}.{}", id.0, suffix)),
        name: name.into(),
        kind,
        enabled: true,
        role_hint,
        authority: InstructionAuthority::Creator,
        template: None,
        condition: None,
        source,
        placement_zone: zone,
        history_selector: None,
        token_policy: lorepia_domain::TokenPolicy {
            priority,
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        overflow_policy,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance.clone(),
    };
    let character = policy(
        "character",
        "Character",
        PromptBlockKind::CharacterDescription,
        BlockSource::CharacterField {
            field: CharacterField::Description,
        },
        PlacementZone::CharacterContext,
        RoleHint::System,
        OverflowPolicy::TrimTail,
        900,
    );
    let mut history = policy(
        "history",
        "Recent conversation",
        PromptBlockKind::HistorySlice,
        BlockSource::History,
        PlacementZone::RecentHistory,
        RoleHint::ProviderDefault,
        OverflowPolicy::KeepLatestItems,
        950,
    );
    history.history_selector = Some(HistorySelector::RecentTurns { count: 32 });
    let mut latest = policy(
        "latest_user",
        "Latest user turn",
        PromptBlockKind::LatestUserTurn,
        BlockSource::LatestUser,
        PlacementZone::LatestUser,
        RoleHint::User,
        OverflowPolicy::Reject,
        u16::MAX,
    );
    latest.authority = InstructionAuthority::User;
    PromptPreset {
        id,
        name: name.into(),
        schema_version: 1,
        blocks: vec![character, history, latest],
        controls: Vec::new(),
        default_values: lorepia_domain::VariableMap::default(),
        default_generation_preset_id: None,
        memory_profile_id: None,
        knowledge_book_ids: Vec::new(),
        transform_set_ids: Vec::new(),
        module_ids: Vec::new(),
        cache_boundaries: Vec::new(),
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};
    use lorepia_domain::{
        BlockSource, CharacterPromptContent, ConversationBranchId, ConversationId,
        GenerationPresetId, HistorySelector, MessageId, OverflowPolicy, PersonaPromptContent,
        PlacementZone, PresetMetadata, PromptBlockKind, PromptConversationMessage,
        PromptMessageRole, PromptPresetId, PromptResolutionContext, PromptResolveRequest,
        Provenance, ProviderMessageRole, ProviderPromptContract, SourceKind, UnsupportedRolePolicy,
        VariableMap,
    };

    use super::{
        OrchestrationError, default_prompt_preset, render_prompt_preview, resolve_prompt_plan,
    };

    fn provenance() -> Provenance {
        Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: None,
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        }
    }

    fn request(max_context_tokens: u32, latest: &str) -> PromptResolveRequest {
        let branch_id = ConversationBranchId("branch".into());
        let metadata = PresetMetadata {
            description: "default".into(),
            tags: Vec::new(),
            provenance: provenance(),
            created_at: Utc
                .with_ymd_and_hms(2026, 8, 3, 0, 0, 0)
                .single()
                .expect("time"),
            updated_at: Utc
                .with_ymd_and_hms(2026, 8, 3, 0, 0, 0)
                .single()
                .expect("time"),
            local_override_of: None,
        };
        let mut preset =
            default_prompt_preset(PromptPresetId::from("default"), "Default", metadata);
        preset.blocks[1].history_selector = Some(HistorySelector::RecentTurns { count: 20 });
        preset.blocks[1].token_policy.priority = 1;
        preset.blocks[1].overflow_policy = OverflowPolicy::KeepLatestItems;
        let latest_id = MessageId("m-latest".into());
        PromptResolveRequest {
            preset,
            context: PromptResolutionContext {
                conversation_id: ConversationId("conversation".into()),
                branch_id: branch_id.clone(),
                character: CharacterPromptContent {
                    character_id: "character".into(),
                    name: "Ari".into(),
                    aliases: Vec::new(),
                    description: "A synthetic character.".into(),
                    personality: String::new(),
                    scenario: String::new(),
                    first_message: String::new(),
                    dialogue_examples: Vec::new(),
                    system_instruction: String::new(),
                    post_history_instruction: String::new(),
                    alternate_greetings: Vec::new(),
                    knowledge_book_ids: Vec::new(),
                    asset_ids: Vec::new(),
                },
                persona: Some(PersonaPromptContent {
                    persona_id: "persona".into(),
                    name: "Sam".into(),
                    description: "Synthetic user".into(),
                }),
                user_name: "Sam".into(),
                messages: vec![
                    PromptConversationMessage {
                        id: MessageId("m-old-user".into()),
                        branch_id: branch_id.clone(),
                        role: PromptMessageRole::User,
                        content: "old question ".repeat(20),
                        turn_index: 1,
                    },
                    PromptConversationMessage {
                        id: MessageId("m-old-assistant".into()),
                        branch_id: branch_id.clone(),
                        role: PromptMessageRole::Assistant,
                        content: "old answer ".repeat(20),
                        turn_index: 1,
                    },
                    PromptConversationMessage {
                        id: latest_id.clone(),
                        branch_id,
                        role: PromptMessageRole::User,
                        content: latest.into(),
                        turn_index: 2,
                    },
                ],
                latest_user_message_id: latest_id,
                selected_knowledge: Vec::new(),
                selected_memory: Vec::new(),
                summary_boundaries: Vec::new(),
                conversation_summary: None,
                author_note: None,
                group_context: None,
                variables: VariableMap::default(),
                slots: Vec::new(),
                current_date: "2026-08-03".into(),
                current_time: "12:00".into(),
                supported_capabilities: Vec::new(),
                session_seed: Some(42),
                context_snapshot: None,
            },
            provider: ProviderPromptContract {
                supported_roles: vec![
                    ProviderMessageRole::System,
                    ProviderMessageRole::User,
                    ProviderMessageRole::Assistant,
                ],
                provider_default_role: ProviderMessageRole::User,
                unsupported_role_policy: UnsupportedRolePolicy::MapDeveloperToSystem,
                supports_explicit_cache: false,
                max_cache_boundaries: 0,
            },
            generation_preset_id: Some(GenerationPresetId::from("generation")),
            max_context_tokens,
            reserved_output_tokens: 16,
        }
    }

    #[test]
    fn same_input_has_same_hash_and_preview_matches_materialization() {
        let request = request(512, "Please continue.");
        let first = resolve_prompt_plan(&request).expect("first plan");
        let second = resolve_prompt_plan(&request).expect("second plan");

        assert_eq!(first.plan_hash, second.plan_hash);
        assert_eq!(first, second);
        assert_eq!(first.preview.effective_messages, first.effective_messages);
        assert_eq!(
            render_prompt_preview(&request).expect("preview"),
            first.preview
        );
    }

    #[test]
    fn enabled_dynamic_sources_fail_closed_when_context_is_unavailable() {
        for (kind, source, expected_reason) in [
            (
                PromptBlockKind::ConversationSummary,
                BlockSource::ConversationSummary,
                "conversation summary source is unavailable",
            ),
            (
                PromptBlockKind::AuthorNote,
                BlockSource::AuthorNote,
                "author note source is unavailable",
            ),
            (
                PromptBlockKind::GroupContext,
                BlockSource::GroupContext,
                "group context source is unavailable",
            ),
        ] {
            let mut request = request(512, "Please continue.");
            request.preset.blocks[0].kind = kind;
            request.preset.blocks[0].source = source;
            request.preset.blocks[0].template = None;
            let error = resolve_prompt_plan(&request).expect_err("missing source must fail");
            assert!(
                matches!(
                    &error,
                    OrchestrationError::Materialization { reason, .. }
                        if reason.contains(expected_reason)
                ),
                "unexpected error: {error}"
            );
        }
    }

    #[test]
    fn overflow_removes_old_history_but_never_latest_user() {
        let request = request(72, "latest survives");
        let plan = resolve_prompt_plan(&request).expect("bounded plan");
        assert!(plan.effective_messages.iter().any(|message| {
            message.content == "latest survives"
                && message.block_kind == lorepia_domain::PromptBlockKind::LatestUserTurn
        }));
        assert!(plan.trace.estimated_input_tokens <= plan.trace.available_input_tokens);
        assert!(!plan.trace.overflow.is_empty());
    }

    #[test]
    fn latest_user_that_cannot_fit_fails_explicitly() {
        let request = request(32, &"x".repeat(512));
        assert!(matches!(
            resolve_prompt_plan(&request),
            Err(OrchestrationError::LatestUserMessageExceedsBudget { .. })
        ));
    }

    #[test]
    fn role_fallback_is_visible_in_trace() {
        let mut request = request(512, "hello");
        request.preset.blocks[0].role_hint = lorepia_domain::RoleHint::Developer;
        let plan = resolve_prompt_plan(&request).expect("plan");
        assert!(plan.trace.role_mappings.iter().any(|mapping| {
            mapping.requested_role == lorepia_domain::RoleHint::Developer
                && mapping.effective_role == ProviderMessageRole::System
                && mapping.explanation.contains("mapped")
        }));
    }

    #[test]
    fn imported_content_cannot_resolve_to_a_privileged_provider_role() {
        for requested_role in [
            lorepia_domain::RoleHint::System,
            lorepia_domain::RoleHint::Developer,
        ] {
            let mut request = request(512, "hello");
            request.preset.blocks[0].authority =
                lorepia_domain::InstructionAuthority::ImportedContent;
            request.preset.blocks[0].role_hint = requested_role;
            let block_id = request.preset.blocks[0].id.clone();

            let plan = resolve_prompt_plan(&request).expect("imported content is downgraded");
            let message = plan
                .effective_messages
                .iter()
                .find(|message| message.block_id == block_id)
                .expect("imported block is materialized");

            assert_eq!(message.requested_role, requested_role);
            assert_eq!(message.effective_role, ProviderMessageRole::User);
            assert_eq!(
                message.authority,
                lorepia_domain::InstructionAuthority::ImportedContent
            );
        }
    }

    #[test]
    fn canonical_verifier_rejects_tampered_materialization() {
        let request = request(512, "hello");
        let mut plan = resolve_prompt_plan(&request).expect("plan");
        plan.effective_messages[0].content.push_str(" tampered");
        plan.preview.effective_messages[0]
            .content
            .push_str(" tampered");
        assert!(matches!(
            super::verify_resolved_prompt_plan(&plan),
            Err(OrchestrationError::PlanHashMismatch)
        ));
    }

    #[test]
    fn resolved_prompt_transform_reseals_only_contents_and_derived_fields() {
        let original = resolve_prompt_plan(&request(512, "hello")).expect("original plan");
        let mut contents = original
            .effective_messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        contents[0].push_str(" transformed");

        let resealed =
            super::reseal_resolved_prompt_plan(&original, &contents).expect("resealed plan");

        super::verify_resolved_prompt_plan(&original).expect("original remains valid");
        super::verify_resolved_prompt_plan(&resealed).expect("resealed plan is canonical");
        assert_ne!(resealed.plan_hash, original.plan_hash);
        assert_eq!(resealed.effective_messages[0].content, contents[0]);
        assert_eq!(
            resealed.preview.effective_messages,
            resealed.effective_messages
        );
        assert_eq!(
            resealed.trace.estimated_input_tokens,
            resealed.preview.estimated_input_tokens
        );
        for (before, after) in original
            .effective_messages
            .iter()
            .zip(&resealed.effective_messages)
        {
            assert_eq!(after.sequence, before.sequence);
            assert_eq!(after.block_id, before.block_id);
            assert_eq!(after.block_kind, before.block_kind);
            assert_eq!(after.requested_role, before.requested_role);
            assert_eq!(after.effective_role, before.effective_role);
            assert_eq!(after.authority, before.authority);
            assert_eq!(after.source_message_ids, before.source_message_ids);
            assert_eq!(after.provenance, before.provenance);
        }
    }

    #[test]
    fn resolved_prompt_reseal_rejects_tampered_original_or_changed_shape() {
        let original = resolve_prompt_plan(&request(512, "hello")).expect("original plan");
        let contents = original
            .effective_messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        let mut tampered = original.clone();
        tampered.effective_messages[0].effective_role = ProviderMessageRole::Assistant;
        tampered.preview.effective_messages[0].effective_role = ProviderMessageRole::Assistant;

        assert!(matches!(
            super::reseal_resolved_prompt_plan(&tampered, &contents),
            Err(OrchestrationError::PlanHashMismatch)
        ));
        assert!(matches!(
            super::reseal_resolved_prompt_plan(&original, &contents[..contents.len() - 1]),
            Err(OrchestrationError::Invalid(_))
        ));
    }

    #[test]
    fn resolved_prompt_reseal_rejects_empty_or_over_budget_output() {
        let original = resolve_prompt_plan(&request(512, "hello")).expect("original plan");
        let mut empty = original
            .effective_messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        empty[0].clear();
        assert!(matches!(
            super::reseal_resolved_prompt_plan(&original, &empty),
            Err(OrchestrationError::Invalid(_))
        ));

        let mut oversized = original
            .effective_messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        oversized[0] = "x".repeat(8_192);
        assert!(matches!(
            super::reseal_resolved_prompt_plan(&original, &oversized),
            Err(OrchestrationError::OverflowRejected { .. })
        ));
    }

    #[test]
    fn resolved_prompt_reseal_rejects_changes_to_protected_authority_content() {
        let latest_original = resolve_prompt_plan(&request(512, "hello")).expect("latest plan");
        let mut latest_contents = latest_original
            .effective_messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        let latest_index = latest_original
            .effective_messages
            .iter()
            .position(|message| message.block_kind == PromptBlockKind::LatestUserTurn)
            .expect("latest-user message");
        latest_contents[latest_index].push_str(" transformed");
        assert!(matches!(
            super::reseal_resolved_prompt_plan(&latest_original, &latest_contents),
            Err(OrchestrationError::Invalid(_))
        ));

        let mut application_request = request(512, "hello");
        application_request.preset.blocks[0].authority =
            lorepia_domain::InstructionAuthority::Application;
        application_request.preset.blocks[0].placement_zone = PlacementZone::ApplicationPolicy;
        let application_original =
            resolve_prompt_plan(&application_request).expect("application plan");
        let mut application_contents = application_original
            .effective_messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        let application_index = application_original
            .effective_messages
            .iter()
            .position(|message| {
                message.authority == lorepia_domain::InstructionAuthority::Application
            })
            .expect("application message");
        application_contents[application_index].push_str(" transformed");
        assert!(matches!(
            super::reseal_resolved_prompt_plan(&application_original, &application_contents),
            Err(OrchestrationError::Invalid(_))
        ));
    }

    #[test]
    fn placement_validation_rejects_latest_user_before_history() {
        let mut request = request(512, "hello");
        request.preset.blocks.swap(1, 2);
        assert!(matches!(
            resolve_prompt_plan(&request),
            Err(OrchestrationError::Invalid(_))
        ));
        request.preset.blocks[1].placement_zone = PlacementZone::LatestUser;
    }
}
