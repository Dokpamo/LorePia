//! Durable orchestration runtime coordination.
//!
//! Pure prompt, memory, transform, and interaction engines live in
//! `lorepia-orchestration`. This module is the trusted boundary that derives
//! conversation lineage, active content policy, model capabilities, and
//! compare-and-swap inputs before asking storage to mutate durable state.

mod auxiliary_tasks;
mod derived;
mod embedding;
mod generation_proposals;
mod interaction;
mod module_runtime;
mod persistence;
mod plan;
mod proposals;
mod recovery;
mod semantic;

#[cfg(test)]
use lorepia_domain::CoreErrorCode;
use lorepia_domain::{CoreError, CoreResult};
use serde::Serialize;
use sha2::{Digest, Sha256};

// Preserve the existing crate-visible orchestration_runtime path.
#[allow(unused_imports)]
pub(crate) use self::auxiliary_tasks::PreparedMemoryTaskInput;
pub use self::auxiliary_tasks::{
    ClaimedMemoryJob, EnqueueMemorySummaryRequest, MemoryJobEnqueueReceipt,
    MemoryJobExecutionResult, MemoryRuntimeProvenance, RuntimeTaskTargetRevision,
    RuntimeTransformRevision, TaskCredentialBroker,
};
use self::auxiliary_tasks::{MemorySummaryHeadAuthority, ResolvedPromptRuntimePolicy};
#[cfg(test)]
use self::auxiliary_tasks::{memory_summary_system_instruction, next_memory_summary_turn_window};
use self::derived::{
    prepare_generation_attempt_derived_closure,
    remap_generation_attempt_derived_closure_existing_proposals,
};
#[cfg(test)]
use self::embedding::memory_embedding_candidate_limit;
use self::embedding::memory_embedding_job_seed;
#[cfg(test)]
use self::generation_proposals::generation_proposal_decision_requires_reviewable_text;
use self::generation_proposals::remap_generation_attempt_proposal_ids;
pub use self::generation_proposals::{
    GenerationAttemptProposalDecisionReceipt, GenerationAttemptProposalDecisionRequest,
    GenerationAttemptProposalExpiryReceipt, GenerationAttemptProposalView,
};
#[cfg(test)]
use self::interaction::interaction_evaluation_limits;
pub(crate) use self::interaction::interaction_state_key;
pub use self::interaction::{
    InteractionEventReview, InteractionReviewRequest, InteractionRuleSetRevision,
};
use self::interaction::{
    PreparedInteractionReview, ResolvedInteractionPolicy, initial_interaction_state,
    interaction_knowledge_bindings, interaction_policy_snapshot, interaction_seed,
    reconcile_interaction_knowledge_state, validate_interaction_evaluation_seal,
};
use self::module_runtime::{ResolvedModuleRuntime, module_plan_error};
pub(crate) use self::module_runtime::{
    apply_exact_transform_runtime_overlay, collect_exact_component_import_approvals,
};
use self::persistence::{ProcessedCoreLifecycleOccurrence, interaction_commit_artifacts};
pub use self::proposals::{InteractionProposalDecisionReceipt, InteractionProposalDecisionRequest};
#[cfg(test)]
use self::proposals::{
    interaction_proposal_decision_requires_reviewable_text,
    require_reviewable_interaction_proposal_text,
};
pub(crate) use self::recovery::core_lifecycle_retry_seconds;
pub use self::recovery::{
    CoreLifecycleDeliveryReceipt, CoreLifecycleDeliveryStatus, CoreLifecycleDrainReceipt,
    InterruptedMemoryJob, MemoryQueryEmbeddingRetryCandidate,
};
pub(crate) use self::semantic::{MemorySemanticQueryEvidence, ResolvedMemorySemanticQuery};

include!("orchestration_runtime/interaction/tests/knowledge_binding_revision.rs");

include!("orchestration_runtime/tests/memory_summary_instruction.rs");

fn versioned_digest<T: Serialize>(value: &T) -> CoreResult<String> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| CoreError::internal(format!("cannot hash runtime value: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn hex_prefix_bytes(digest: &str) -> CoreResult<[u8; 8]> {
    if digest.len() < 16 {
        return Err(CoreError::internal("runtime digest is unexpectedly short"));
    }
    let mut bytes = [0_u8; 8];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let offset = index * 2;
        *byte = u8::from_str_radix(&digest[offset..offset + 2], 16)
            .map_err(|_| CoreError::internal("runtime digest is not hexadecimal"))?;
    }
    Ok(bytes)
}

fn interaction_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::invalid(format!(
        "interaction runtime rejected the operation: {error}"
    ))
}

include!("orchestration_runtime/tests/proposal_projection_authority.rs");

include!("orchestration_runtime/tests/generation_proposal_identity.rs");

include!("orchestration_runtime/tests/memory_cadence.rs");
