//! High-level use cases for prompt orchestration and creator-owned content.
//!
//! Storage documents remain revisioned and all writes use explicit optimistic
//! concurrency. Prompt rendering stays in `lorepia-orchestration`; this module
//! coordinates that pure engine with conversation, branch, and provider state.

mod knowledge;
mod memory;
mod modules;
mod presets;
mod prompt_assembly;
mod prompt_plan;
mod semantic;
mod targets;
mod transforms;
mod variables;

#[allow(unused_imports)]
pub(crate) use knowledge::KnowledgeSemanticScoreSourceEvidence;
pub(crate) use knowledge::{KnowledgeSemanticBookEvidence, KnowledgeSemanticProviderRequirement};
pub use knowledge::{KnowledgeSimulationRequest, KnowledgeTokenEstimate};
pub use memory::MemoryRetrievalRequest;
pub use modules::ContentShareGate;
use modules::{
    PromptModuleOverlay, PromptModuleOverlayInput, exact_prompt_manual_knowledge,
    prompt_module_knowledge_revisions,
};
pub(crate) use presets::enforce_application_policy;
pub use presets::{PromptPresetRollbackApplyRequest, PromptPresetRollbackReceipt};
pub(crate) use prompt_assembly::PreparedGenerationPlan;
use prompt_assembly::PromptSelectionInput;
pub(crate) use prompt_plan::{
    AsyncPromptPlanPreparation, GenerationPlanInput, GenerationPromptAuthorityCapture,
    PromptPlanPreparation, deterministic_prompt_user_message_id,
};
pub use prompt_plan::{
    ExpertPromptPreview, PromptDiffEntry, PromptPlanMessagePreview, PromptPlanPreview,
    PromptPlanRequest,
};
use semantic::{
    activation_rule_uses_semantic, knowledge_embedding_matches_sha256,
    knowledge_semantic_query_sha256, knowledge_semantic_scores_sha256,
    lexical_knowledge_semantic_scores_with_budget,
};
pub(crate) use semantic::{charge_provider_knowledge_work, semantic_score_from_millionths};
pub use targets::{
    PromptAppliedParameterPreview, PromptEffectiveMessageContentPreview,
    PromptProviderMessagePreview, TaskGenerationTargetPlan,
};
pub use transforms::TransformPreviewRequest;
pub(crate) use transforms::apply_transform_sets_with_import_approvals;
pub use variables::{CreatorControlValue, RoomOrchestrationConfig, RoomOrchestrationConfigPatch};

use memory::PromptContextMaterialization;
use presets::{
    PromptPersonaMaterialization, PromptPresetPreparation, orchestration_validation_error,
    validate_prompt_binding_sources,
};
use targets::{
    PromptProviderResolution, cacheable_prefix_has_volatile_before_fixed_after,
    canonical_prompt_capabilities, prompt_execution_hash, provider_cacheable_prefix_tokens,
    redacted_prompt_preview,
};
use transforms::{PromptTransformPreparation, apply_resolved_prompt_transforms};
use variables::{PromptQuickSettings, PromptVariableState, prompt_creativity_temperature};

#[cfg(test)]
mod prompt_manual_knowledge_revision_tests {
    use std::collections::BTreeMap;

    use lorepia_domain::KnowledgeEntryId;
    use lorepia_storage::InteractionKnowledgeBinding;

    use super::exact_prompt_manual_knowledge;

    #[test]
    fn prompt_manual_activation_requires_the_exact_current_book_revision() {
        let entry_id = KnowledgeEntryId::from("shared-entry");
        let active = [entry_id.clone()];
        let old_binding = [InteractionKnowledgeBinding {
            book_revision_id: "book-old".to_owned(),
            entry_id: entry_id.clone(),
        }];
        let current = BTreeMap::from([(entry_id.clone(), "book-new".to_owned())]);

        let stale = exact_prompt_manual_knowledge(&active, &old_binding, &current)
            .expect("stale state remains readable but inert");
        assert!(stale.is_empty());

        let exact_binding = [InteractionKnowledgeBinding {
            book_revision_id: "book-new".to_owned(),
            entry_id: entry_id.clone(),
        }];
        let exact = exact_prompt_manual_knowledge(&active, &exact_binding, &current)
            .expect("exact binding");
        assert!(exact.contains(&entry_id));
    }
}

#[cfg(test)]
mod knowledge_work_budget_tests {
    use lorepia_domain::{
        ActivationRule, KnowledgeBook, KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId,
        KnowledgePlacement, Provenance, SourceKind, TokenBudget, TokenPolicy,
    };
    use lorepia_orchestration::KnowledgeWorkBudget;
    use lorepia_storage::KnowledgeEmbeddingMatch;

    use super::{
        charge_provider_knowledge_work, knowledge_embedding_matches_sha256,
        lexical_knowledge_semantic_scores_with_budget,
    };

    fn semantic_only_book() -> KnowledgeBook {
        let book_id = KnowledgeBookId::from("semantic-budget-book");
        let provenance = Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: None,
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        };
        KnowledgeBook {
            id: book_id.clone(),
            name: "Semantic budget book".to_owned(),
            schema_version: 1,
            entries: vec![KnowledgeEntry {
                id: KnowledgeEntryId::from("semantic-entry"),
                book_id,
                name: "Semantic entry".to_owned(),
                content: "semantic fallback candidate text".repeat(8),
                enabled: true,
                activation: ActivationRule::Semantic {
                    threshold: 0.0,
                    top_k: 1,
                },
                priority: 0,
                importance: 0,
                placement: KnowledgePlacement::RetrievedContext,
                token_policy: TokenPolicy {
                    priority: 0,
                    min_tokens: None,
                    max_tokens: None,
                    reserve_tokens: None,
                },
                parent_id: None,
                activation_probability_basis_points: 10_000,
                provenance: provenance.clone(),
            }],
            scan_depth: 8,
            token_budget: TokenBudget { max_tokens: 1_024 },
            recursive: false,
            max_recursion_depth: 0,
            provenance,
        }
    }

    #[test]
    fn semantic_only_fallback_exhausts_the_generation_budget() {
        let book = semantic_only_book();
        let scan = vec!["semantic fallback query".repeat(8)];
        let mut measurement = KnowledgeWorkBudget::default();
        let scores = lexical_knowledge_semantic_scores_with_budget(&book, &scan, &mut measurement)
            .expect("semantic fallback fits the default budget");
        assert_eq!(scores.len(), 1);
        let one_fallback_work = measurement.used_work_bytes();
        assert!(one_fallback_work > 0, "semantic fallback must be charged");

        let mut exhausted =
            KnowledgeWorkBudget::with_max_work_bytes(one_fallback_work.saturating_sub(1));
        assert!(
            lexical_knowledge_semantic_scores_with_budget(&book, &scan, &mut exhausted).is_err()
        );
    }

    #[test]
    fn provider_and_lexical_work_share_one_generation_budget() {
        let book = semantic_only_book();
        let scan = vec!["combined provider and lexical query".repeat(8)];
        let mut measurement = KnowledgeWorkBudget::default();
        lexical_knowledge_semantic_scores_with_budget(&book, &scan, &mut measurement)
            .expect("measure lexical fallback work");
        let lexical_work = measurement.used_work_bytes();
        let provider_work = 256_usize;
        let combined_limit = provider_work
            .checked_add(lexical_work)
            .expect("combined work fits usize")
            .saturating_sub(1);
        let mut combined = KnowledgeWorkBudget::with_max_work_bytes(combined_limit);

        charge_provider_knowledge_work(book.id.as_str(), &mut combined, provider_work)
            .expect("provider work fits before lexical fallback");
        assert!(
            lexical_knowledge_semantic_scores_with_budget(&book, &scan, &mut combined).is_err(),
            "provider work must reduce the budget available to lexical fallback"
        );
    }

    #[test]
    fn provider_match_evidence_hash_uses_the_generation_budget() {
        let matches = [KnowledgeEmbeddingMatch {
            embedding_id: "embedding:budgeted-match".to_owned(),
            entry_id: KnowledgeEntryId::from("entry:budgeted-match"),
            vector_sha256: "a".repeat(64),
            similarity_millionths: 750_000,
        }];
        let mut measurement = KnowledgeWorkBudget::default();
        knowledge_embedding_matches_sha256(
            "book-revision:budgeted-match",
            &matches,
            "book:budgeted-match",
            &mut measurement,
        )
        .expect("measure provider match evidence hash work");
        let hash_work = measurement.used_work_bytes();
        assert!(hash_work > 0, "provider match hash must be charged");

        let mut exhausted = KnowledgeWorkBudget::with_max_work_bytes(hash_work.saturating_sub(1));
        assert!(
            knowledge_embedding_matches_sha256(
                "book-revision:budgeted-match",
                &matches,
                "book:budgeted-match",
                &mut exhausted,
            )
            .is_err()
        );
    }
}
