use chrono::Utc;
use lorepia_domain::{
    CapabilityKey, ConversationBranchId, ConversationId, CoreResult, InteractionEvent,
    InteractionRuleSetId, InteractionState, MessageId,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, InteractionCompileOptions, InteractionContext, InteractionEngine,
    InteractionOutcome,
};
use lorepia_storage::{InteractionEvaluationSeal, InteractionKnowledgeBinding};
use serde::{Deserialize, Serialize};

use super::{
    policy::{ResolvedInteractionPolicy, interaction_engine_template_values},
    state::{
        interaction_evaluation_seal, interaction_limits_from_evaluation,
        normalize_interaction_event_revision, reconcile_interaction_knowledge_state,
        validate_interaction_evaluation_seal,
    },
};
use crate::{
    Core,
    orchestration_runtime::{hex_prefix_bytes, interaction_error, versioned_digest},
};

/// Read-only interaction review request.
///
/// A generic event may be previewed by creator tooling. Mutation uses the
/// crate-private commit path below, so native callers cannot forge lifecycle
/// events such as `BeforeGeneration` or `MessageCommitted`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionReviewRequest {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub expected_head: Option<MessageId>,
    pub event: InteractionEvent,
}

/// Immutable rule-set identity included in an interaction review hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionRuleSetRevision {
    pub rule_set_id: InteractionRuleSetId,
    pub revision: u64,
    pub revision_id: String,
    pub sha256: String,
}

/// A deterministic review of one event against current durable state.
///
/// `review_sha256` commits to the request, state revision, exact rule-set
/// revisions, derived capabilities, effects, and next state. It contains no
/// credential and is recomputed immediately before a commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEventReview {
    pub request: InteractionReviewRequest,
    pub expected_state_revision: u64,
    pub event_epoch_seconds: i64,
    /// Exact full-context module activation plan used by this review.
    #[serde(default)]
    pub module_plan_sha256: Option<String>,
    pub rule_sets: Vec<InteractionRuleSetRevision>,
    pub supported_capabilities: Vec<CapabilityKey>,
    pub outcome: InteractionOutcome,
    pub review_sha256: String,
}

#[derive(Debug, Clone)]
pub(in crate::orchestration_runtime) struct PreparedInteractionReview {
    pub(in crate::orchestration_runtime) public: InteractionEventReview,
    pub(in crate::orchestration_runtime) policy: ResolvedInteractionPolicy,
    pub(in crate::orchestration_runtime) evaluation_seal: InteractionEvaluationSeal,
    pub(in crate::orchestration_runtime) deterministic_seed: u64,
}

impl Core {
    pub(in crate::orchestration_runtime) fn prepare_interaction_review_from_state(
        &self,
        request: &InteractionReviewRequest,
        state: InteractionState,
        existing_knowledge: &[InteractionKnowledgeBinding],
        explicit_event_at: Option<chrono::DateTime<Utc>>,
        enforce_current_head: bool,
    ) -> CoreResult<PreparedInteractionReview> {
        let branch = if enforce_current_head {
            self.validate_runtime_branch_head(
                &request.conversation_id,
                &request.branch_id,
                request.expected_head.as_ref(),
            )?
        } else {
            self.validate_runtime_branch_identity(&request.conversation_id, &request.branch_id)?
        };
        let policy =
            self.resolve_interaction_policy(&request.conversation_id, &request.branch_id)?;
        let event_at = explicit_event_at.unwrap_or(branch.updated_at);
        Self::prepare_interaction_review_with_policy(
            request,
            state,
            existing_knowledge,
            event_at,
            policy,
        )
    }

    pub(in crate::orchestration_runtime) fn prepare_proposed_branch_interaction_review_from_state(
        &self,
        request: &InteractionReviewRequest,
        state: InteractionState,
        existing_knowledge: &[InteractionKnowledgeBinding],
        event_at: chrono::DateTime<Utc>,
        applied_plan: Option<&AppliedModuleRuntimePlan>,
    ) -> CoreResult<PreparedInteractionReview> {
        let policy = self.resolve_interaction_policy_for_proposed_branch(
            &request.conversation_id,
            &request.branch_id,
            applied_plan,
        )?;
        Self::prepare_interaction_review_with_policy(
            request,
            state,
            existing_knowledge,
            event_at,
            policy,
        )
    }

    fn prepare_interaction_review_with_policy(
        request: &InteractionReviewRequest,
        state: InteractionState,
        existing_knowledge: &[InteractionKnowledgeBinding],
        event_at: chrono::DateTime<Utc>,
        policy: ResolvedInteractionPolicy,
    ) -> CoreResult<PreparedInteractionReview> {
        let evaluation_seal = interaction_evaluation_seal(&policy, event_at)?;
        Self::prepare_interaction_review_with_evaluation_seal(
            request,
            state,
            existing_knowledge,
            event_at,
            policy,
            evaluation_seal,
        )
    }

    pub(in crate::orchestration_runtime) fn prepare_interaction_review_with_evaluation_seal(
        request: &InteractionReviewRequest,
        state: InteractionState,
        existing_knowledge: &[InteractionKnowledgeBinding],
        event_at: chrono::DateTime<Utc>,
        policy: ResolvedInteractionPolicy,
        evaluation_seal: InteractionEvaluationSeal,
    ) -> CoreResult<PreparedInteractionReview> {
        validate_interaction_evaluation_seal(&policy, event_at, &evaluation_seal)?;
        let deterministic_seed = interaction_seed(
            request,
            state.revision,
            &policy.rule_set_revisions,
            event_at.timestamp(),
        )?;
        Self::prepare_interaction_review_with_sealed_authority(
            request,
            state,
            existing_knowledge,
            event_at,
            policy,
            evaluation_seal,
            deterministic_seed,
        )
    }

    pub(in crate::orchestration_runtime) fn prepare_interaction_review_with_sealed_authority(
        request: &InteractionReviewRequest,
        state: InteractionState,
        existing_knowledge: &[InteractionKnowledgeBinding],
        event_at: chrono::DateTime<Utc>,
        policy: ResolvedInteractionPolicy,
        evaluation_seal: InteractionEvaluationSeal,
        deterministic_seed: u64,
    ) -> CoreResult<PreparedInteractionReview> {
        validate_interaction_evaluation_seal(&policy, event_at, &evaluation_seal)?;
        let (mut state, _) =
            reconcile_interaction_knowledge_state(state, &policy, existing_knowledge)?;
        if state.revision == 0 && state.variables.values.is_empty() {
            state.variables = evaluation_seal.policy_variables.clone();
        }
        let engine = InteractionEngine::compile_with_options(
            &policy.rule_sets,
            interaction_limits_from_evaluation(&evaluation_seal.limits),
            &InteractionCompileOptions {
                approved_import_source_ids: policy.approved_import_source_ids.clone(),
            },
        )
        .map_err(interaction_error)?;
        let event_epoch_seconds = event_at.timestamp();
        let mut outcome = engine
            .handle_event(
                &state,
                &request.event,
                &InteractionContext {
                    deterministic_seed,
                    event_epoch_seconds,
                    model_capabilities: evaluation_seal.supported_capabilities.clone(),
                    template_values: interaction_engine_template_values(
                        &evaluation_seal.template_values,
                    ),
                },
            )
            .map_err(interaction_error)?;
        normalize_interaction_event_revision(&state, &mut outcome)?;
        let expected_state_revision = state.revision;
        let review_sha256 = interaction_review_sha256(
            request,
            expected_state_revision,
            event_epoch_seconds,
            policy.module_plan_sha256.as_deref(),
            &policy.rule_set_revisions,
            &policy.supported_capabilities,
            &outcome,
        )?;
        Ok(PreparedInteractionReview {
            public: InteractionEventReview {
                request: request.clone(),
                expected_state_revision,
                event_epoch_seconds,
                module_plan_sha256: policy.module_plan_sha256.clone(),
                rule_sets: policy.rule_set_revisions.clone(),
                supported_capabilities: policy.supported_capabilities.clone(),
                outcome,
                review_sha256,
            },
            policy,
            evaluation_seal,
            deterministic_seed,
        })
    }
}

pub(in crate::orchestration_runtime) fn interaction_seed(
    request: &InteractionReviewRequest,
    state_revision: u64,
    rule_sets: &[InteractionRuleSetRevision],
    event_epoch_seconds: i64,
) -> CoreResult<u64> {
    let digest = versioned_digest(&(
        "lorepia.interaction-seed.v1",
        request,
        state_revision,
        rule_sets,
        event_epoch_seconds,
    ))?;
    let bytes = hex_prefix_bytes(&digest)?;
    Ok(u64::from_be_bytes(bytes))
}

fn interaction_review_sha256(
    request: &InteractionReviewRequest,
    state_revision: u64,
    event_epoch_seconds: i64,
    module_plan_sha256: Option<&str>,
    rule_sets: &[InteractionRuleSetRevision],
    supported_capabilities: &[CapabilityKey],
    outcome: &InteractionOutcome,
) -> CoreResult<String> {
    versioned_digest(&(
        "lorepia.interaction-review.v1",
        request,
        state_revision,
        event_epoch_seconds,
        module_plan_sha256,
        rule_sets,
        supported_capabilities,
        outcome,
    ))
}
