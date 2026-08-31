use super::{
    GenerationPlanInput, KnowledgeSemanticProviderRequirement, PromptModuleOverlayInput,
    orchestration_validation_error,
};
use crate::{
    Core,
    orchestration_runtime::{ResolvedMemorySemanticQuery, TaskCredentialBroker},
};
use lorepia_domain::{
    ActivationRule, CharacterContentV1, CoreError, CoreResult, KnowledgeBook, MessageRole,
    PersonaId, PromptPreset, SemanticKnowledgeScore,
};
use lorepia_orchestration::KnowledgeWorkBudget;
use lorepia_storage::KnowledgeEmbeddingMatch;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

impl Core {
    #[allow(clippy::too_many_lines)]
    pub(super) async fn prepare_generation_memory_semantic_query(
        &self,
        input: &GenerationPlanInput<'_>,
        credential_broker: &dyn TaskCredentialBroker,
        cancelled: tokio::sync::watch::Receiver<bool>,
        knowledge_work_budget: &mut KnowledgeWorkBudget,
    ) -> CoreResult<Option<ResolvedMemorySemanticQuery>> {
        let latest = input
            .history
            .last()
            .filter(|message| message.role == MessageRole::User)
            .ok_or_else(|| CoreError::invalid("prompt history must end with a user message"))?;
        let (preset, _, prompt_preset_revision_id, binding, persona_selection) =
            self.resolve_generation_prompt_selection(input)?;
        let memory_enabled = input.prompt_selection_authority.map_or_else(
            || {
                binding
                    .as_ref()
                    .is_none_or(|binding| binding.value.memory_enabled)
            },
            |authority| authority.quick_settings.memory_enabled,
        );
        let knowledge_enabled = input.prompt_selection_authority.map_or_else(
            || {
                binding
                    .as_ref()
                    .is_none_or(|binding| binding.value.knowledge_enabled)
            },
            |authority| authority.quick_settings.knowledge_enabled,
        );
        let semantic_requirements = if knowledge_enabled {
            self.prompt_semantic_knowledge_requirements(
                input,
                &preset,
                &prompt_preset_revision_id,
                persona_selection
                    .as_ref()
                    .map(|selection| &selection.value.persona_id),
            )?
        } else {
            Vec::new()
        };
        if (!memory_enabled && semantic_requirements.is_empty())
            || preset.memory_profile_id.is_none()
        {
            return Ok(None);
        }
        let exact_profile = self
            .storage()
            .get_prompt_preset_memory_profile_revision(&prompt_preset_revision_id)?
            .ok_or_else(|| {
                CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "prompt preset memory profile dependency is missing its exact revision",
                    false,
                )
            })?;
        if preset.memory_profile_id.as_ref() != Some(&exact_profile.value.id) {
            return Err(CoreError::new(
                lorepia_domain::CoreErrorCode::StorageCorrupted,
                "prompt preset memory profile identity differs from its exact revision",
                false,
            ));
        }
        if exact_profile.value.embedding_task.is_none() {
            return Ok(None);
        }
        let lineage_branch_id = input.memory_lineage_branch_id.unwrap_or(input.branch_id);
        let records = if memory_enabled {
            let selection = match input.generation_attempt_id {
                Some(generation_id) => self.load_generation_attempt_memory_selection(
                    generation_id,
                    input.conversation_id,
                    lineage_branch_id,
                    input.context_head_message_id,
                )?,
                None => self.storage().list_memory_records_at_head(
                    input.conversation_id,
                    lineage_branch_id,
                    input.context_head_message_id,
                    false,
                )?,
            };
            selection
                .records
                .into_iter()
                .map(|stored| stored.value)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let query_texts = input
            .history
            .iter()
            .filter(|message| message.role != MessageRole::System)
            .rev()
            .take(32)
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        // Preview's deterministic user message is not yet durable. Both
        // preview and send therefore bind the query intent to the same
        // pre-action lineage anchor (the user message's parent), while the
        // canonical query hash still binds the complete hypothetical input.
        let Some(lineage_anchor) = latest.parent_id.as_ref() else {
            // A read-only first-turn preview has no durable message authority
            // to own an exactly-once provider intent. Return exact provider
            // profile evidence with an empty memory result so memory remains
            // valid, while knowledge receives the bounded deterministic
            // lexical scorer because no raw query vector is present.
            if !records.is_empty() {
                return Err(CoreError::new(
                    lorepia_domain::CoreErrorCode::StorageCorrupted,
                    "root prompt history unexpectedly has visible memory records",
                    false,
                ));
            }
            return self
                .resolve_memory_semantic_scores(
                    &exact_profile,
                    input.conversation_id,
                    lineage_branch_id,
                    &latest.id,
                    &latest.id,
                    &records,
                    &query_texts,
                    &[],
                    credential_broker,
                    cancelled,
                    knowledge_work_budget,
                )
                .await
                .map(Some);
        };
        self.resolve_memory_semantic_scores(
            &exact_profile,
            input.conversation_id,
            lineage_branch_id,
            lineage_anchor,
            lineage_anchor,
            &records,
            &query_texts,
            &semantic_requirements,
            credential_broker,
            cancelled,
            knowledge_work_budget,
        )
        .await
        .map(Some)
    }

    fn prompt_semantic_knowledge_requirements(
        &self,
        input: &GenerationPlanInput<'_>,
        preset: &PromptPreset,
        prompt_preset_revision_id: &str,
        persona_id: Option<&PersonaId>,
    ) -> CoreResult<Vec<KnowledgeSemanticProviderRequirement>> {
        let prompt_books = self
            .storage()
            .get_prompt_preset_knowledge_book_revisions(prompt_preset_revision_id)?
            .into_iter()
            .map(|revision| (revision.value.id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        let module_books = self
            .resolve_prompt_module_overlay(
                preset,
                prompt_preset_revision_id,
                PromptModuleOverlayInput {
                    character: input
                        .prompt_selection_authority
                        .map_or(input.character, |authority| &authority.character),
                    conversation_id: input.conversation_id,
                    branch_id: input.branch_id,
                    persona_id,
                    applied_plan_override: input.applied_module_plan_override,
                    sealed_local_user_id_sha256: input
                        .prompt_selection_authority
                        .map(|authority| authority.local_user_id_sha256.as_str()),
                    generation_attempt_id: input.generation_attempt_id,
                },
            )?
            .knowledge_books
            .into_iter()
            .map(|revision| (revision.value.id.clone(), revision))
            .collect::<BTreeMap<_, _>>();
        let character_content = if let Some(authority) = input.prompt_selection_authority {
            authority
                .character_content
                .as_ref()
                .map_or_else(CharacterContentV1::default, |stored| stored.value.clone())
        } else {
            match self.storage().get_character_content(&input.character.id) {
                Ok(stored) => stored.value,
                Err(error) if error.code == lorepia_domain::CoreErrorCode::NotFound => {
                    CharacterContentV1::default()
                }
                Err(error) => return Err(error),
            }
        };
        let mut book_ids = preset
            .knowledge_book_ids
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        if let Some(book_id) = character_content
            .knowledge_book
            .as_ref()
            .and_then(|reference| reference.id.as_ref())
        {
            book_ids.insert(book_id.clone());
        }
        book_ids.extend(module_books.keys().cloned());
        let mut requirements = Vec::new();
        for book_id in book_ids {
            let (book, book_revision_id) = if let Some(revision) = module_books.get(&book_id) {
                (revision.value.clone(), revision.revision_id.clone())
            } else if let Some(revision) = prompt_books.get(&book_id) {
                (revision.value.clone(), revision.revision_id.clone())
            } else if let Some(revision) = input
                .prompt_selection_authority
                .and_then(|authority| authority.character_knowledge_book.as_ref())
                .filter(|revision| revision.value.id == book_id)
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
                let stored = self.get_knowledge_book(&book_id)?;
                let revision_id = stored.revision_id.ok_or_else(|| {
                    CoreError::internal(
                        "semantic knowledge book is missing its immutable revision identity",
                    )
                })?;
                (stored.value, revision_id)
            };
            let entry_ids = book
                .entries
                .iter()
                .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>();
            if !entry_ids.is_empty() {
                requirements.push(KnowledgeSemanticProviderRequirement {
                    book_revision_id,
                    entry_ids,
                });
            }
        }
        Ok(requirements)
    }
}

pub(super) fn activation_rule_uses_semantic(rule: &ActivationRule) -> bool {
    match rule {
        ActivationRule::Semantic { .. } => true,
        ActivationRule::Any { rules } | ActivationRule::All { rules } => {
            rules.iter().any(activation_rule_uses_semantic)
        }
        ActivationRule::Always
        | ActivationRule::Manual
        | ActivationRule::Keyword { .. }
        | ActivationRule::Regex { .. }
        | ActivationRule::Condition { .. } => false,
    }
}

pub(super) fn lexical_knowledge_semantic_scores_with_budget(
    book: &KnowledgeBook,
    scan_texts: &[String],
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<Vec<SemanticKnowledgeScore>> {
    const MAX_SCAN_CHARS: usize = 512 * 1_024;
    let depth = usize::try_from(book.scan_depth).unwrap_or(usize::MAX);
    let start = scan_texts.len().saturating_sub(depth);
    let query_chars = normalized_semantic_characters(
        scan_texts[start..]
            .iter()
            .flat_map(|text| text.chars())
            .take(MAX_SCAN_CHARS),
        book.id.as_str(),
        work_budget,
    )?;
    let mut scores = book
        .entries
        .iter()
        .filter(|entry| entry.enabled && activation_rule_uses_semantic(&entry.activation))
        .map(|entry| -> CoreResult<_> {
            let candidate_chars = normalized_semantic_characters(
                entry
                    .name
                    .chars()
                    .chain(entry.content.chars())
                    .take(MAX_SCAN_CHARS),
                entry.id.as_str(),
                work_budget,
            )?;
            let comparison_work = query_chars
                .len()
                .saturating_add(candidate_chars.len())
                .saturating_mul(2);
            work_budget
                .charge_work_bytes(entry.id.as_str(), comparison_work)
                .map_err(orchestration_validation_error)?;
            let union = query_chars.union(&candidate_chars).count();
            let intersection = query_chars.intersection(&candidate_chars).count();
            Ok(SemanticKnowledgeScore {
                entry_id: entry.id.clone(),
                score: if union == 0 {
                    0.0
                } else {
                    jaccard_score(intersection, union)?
                },
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    scores.sort_by(|left, right| left.entry_id.cmp(&right.entry_id));
    Ok(scores)
}

pub(crate) fn charge_provider_knowledge_work(
    scope_id: &str,
    work_budget: &mut KnowledgeWorkBudget,
    work_bytes: usize,
) -> CoreResult<()> {
    work_budget
        .charge_work_bytes(scope_id, work_bytes)
        .map_err(orchestration_validation_error)
}

fn normalized_semantic_characters(
    characters: impl Iterator<Item = char>,
    scope_id: &str,
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<BTreeSet<char>> {
    let mut normalized = BTreeSet::new();
    for character in characters {
        work_budget
            .charge_work_bytes(scope_id, character.len_utf8())
            .map_err(orchestration_validation_error)?;
        normalized.extend(
            character
                .to_lowercase()
                .filter(|character| character.is_alphanumeric()),
        );
    }
    Ok(normalized)
}

fn jaccard_score(intersection: usize, union: usize) -> CoreResult<f32> {
    if intersection > union || union == 0 {
        return Err(CoreError::internal(
            "knowledge semantic Jaccard cardinality is invalid",
        ));
    }
    let intersection = u64::try_from(intersection)
        .map_err(|_| CoreError::internal("knowledge semantic intersection overflowed"))?;
    let union = u64::try_from(union)
        .map_err(|_| CoreError::internal("knowledge semantic union overflowed"))?;
    let rounded_millionths = intersection
        .checked_mul(1_000_000)
        .and_then(|value| value.checked_add(union / 2))
        .ok_or_else(|| CoreError::internal("knowledge semantic score overflowed"))?
        / union;
    semantic_score_from_millionths(
        u32::try_from(rounded_millionths)
            .map_err(|_| CoreError::internal("knowledge semantic score overflowed"))?,
    )
}

pub(crate) fn semantic_score_from_millionths(millionths: u32) -> CoreResult<f32> {
    if millionths > 1_000_000 {
        return Err(CoreError::internal(
            "knowledge semantic score exceeds one million millionths",
        ));
    }
    let thousands = u16::try_from(millionths / 1_000)
        .map_err(|_| CoreError::internal("knowledge semantic score overflowed"))?;
    let remainder = u16::try_from(millionths % 1_000)
        .map_err(|_| CoreError::internal("knowledge semantic score overflowed"))?;
    Ok((f32::from(thousands) * 1_000.0 + f32::from(remainder)) / 1_000_000.0)
}

pub(super) fn knowledge_semantic_query_sha256(
    book: &KnowledgeBook,
    scan_texts: &[String],
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<String> {
    let depth = usize::try_from(book.scan_depth).unwrap_or(usize::MAX);
    let start = scan_texts.len().saturating_sub(depth);
    let hash_work = scan_texts[start..]
        .iter()
        .fold(0_usize, |total, text| total.saturating_add(text.len()))
        .saturating_mul(6);
    work_budget
        .charge_work_bytes(book.id.as_str(), hash_work)
        .map_err(orchestration_validation_error)?;
    let encoded = serde_json::to_vec(&("lorepia.knowledge-lexical-query.v1", &scan_texts[start..]))
        .map_err(|error| {
            CoreError::internal(format!(
                "cannot encode knowledge semantic query evidence: {error}"
            ))
        })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

pub(super) fn knowledge_semantic_scores_sha256(
    book_revision_id: &str,
    scores: &[SemanticKnowledgeScore],
    scope_id: &str,
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<String> {
    let fixed = scores
        .iter()
        .map(|score| {
            work_budget
                .charge_work_bytes(
                    scope_id,
                    score
                        .entry_id
                        .as_str()
                        .len()
                        .saturating_add(std::mem::size_of::<u32>()),
                )
                .map_err(orchestration_validation_error)?;
            if !score.score.is_finite() || !(0.0..=1.0).contains(&score.score) {
                return Err(CoreError::internal(
                    "knowledge semantic score is outside the canonical domain",
                ));
            }
            Ok((
                score.entry_id.as_str(),
                semantic_score_millionths(score.score)?,
            ))
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let encoded = serde_json::to_vec(&(
        "lorepia.knowledge-semantic-scores.v1",
        book_revision_id,
        fixed,
    ))
    .map_err(|error| {
        CoreError::internal(format!(
            "cannot encode knowledge semantic score evidence: {error}"
        ))
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn semantic_score_millionths(score: f32) -> CoreResult<u32> {
    if !score.is_finite() || !(0.0..=1.0).contains(&score) {
        return Err(CoreError::internal(
            "knowledge semantic score is outside the canonical domain",
        ));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fixed = (score * 1_000_000.0).round() as u32;
    Ok(fixed)
}

pub(super) fn knowledge_embedding_matches_sha256(
    book_revision_id: &str,
    matches: &[KnowledgeEmbeddingMatch],
    scope_id: &str,
    work_budget: &mut KnowledgeWorkBudget,
) -> CoreResult<String> {
    let mut writer = BudgetedKnowledgeMatchHasher {
        hasher: Sha256::new(),
        scope_id,
        work_budget,
        exhausted: false,
    };
    if let Err(error) = serde_json::to_writer(
        &mut writer,
        &(
            "lorepia.knowledge-embedding-matches.v1",
            book_revision_id,
            matches,
        ),
    ) {
        if writer.exhausted {
            return Err(CoreError::invalid(
                "knowledge embedding match evidence exceeds the generation work budget",
            ));
        }
        return Err(CoreError::internal(format!(
            "cannot encode knowledge embedding match evidence: {error}"
        )));
    }
    Ok(format!("{:x}", writer.hasher.finalize()))
}

struct BudgetedKnowledgeMatchHasher<'a> {
    hasher: Sha256,
    scope_id: &'a str,
    work_budget: &'a mut KnowledgeWorkBudget,
    exhausted: bool,
}

impl std::io::Write for BudgetedKnowledgeMatchHasher<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if charge_provider_knowledge_work(self.scope_id, self.work_budget, bytes.len()).is_err() {
            self.exhausted = true;
            return Err(std::io::Error::other(
                "knowledge embedding match evidence budget exhausted",
            ));
        }
        self.hasher.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
