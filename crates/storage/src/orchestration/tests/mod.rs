use lorepia_domain::{
    ActivationRule, AuxiliaryTaskKind, KnowledgeEntry, KnowledgePlacement, RateLimit,
    SummarySchemaId, TokenBudget,
};

use super::*;

struct AppliedRuntimeGenerationFixture {
    root: tempfile::TempDir,
    storage: Storage,
    activation_review: lorepia_orchestration::ModuleMergeReview,
    runtime: lorepia_orchestration::AppliedModuleRuntimePlan,
    generation: GenerationPromptPlanRecord,
}

struct MemoryHeadFixture {
    _root: tempfile::TempDir,
    storage: Storage,
    conversation_id: ConversationId,
    branch_id: ConversationBranchId,
    head_id: MessageId,
    source_sha256: String,
    now: DateTime<Utc>,
}

struct PromptContextAppendFixture {
    _root: tempfile::TempDir,
    storage: Storage,
    now: DateTime<Utc>,
    conversation_id: ConversationId,
    branch_id: ConversationBranchId,
    preset: PromptPreset,
    local_user_id: LocalUserId,
}

fn test_digest(label: &str) -> lorepia_domain::Sha256Digest {
    lorepia_domain::Sha256Digest::parse(sha256_hex(label.as_bytes())).expect("synthetic digest")
}

include!("character_content_metadata.rs");
include!("legacy_validation.rs");
include!("persona_catalog.rs");
include!("prompt_context.rs");
include!("runtime_evidence.rs");
include!("built_in_presets.rs");
include!("memory_head.rs");
include!("built_in_recovery.rs");
