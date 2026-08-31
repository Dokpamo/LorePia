//! Durable orchestration runtime coordination.
//!
//! Pure prompt, memory, transform, and interaction engines live in
//! `lorepia-orchestration`. This module is the trusted boundary that derives
//! conversation lineage, active content policy, model capabilities, and
//! compare-and-swap inputs before asking storage to mutate durable state.

mod auxiliary_tasks;
mod embedding;
mod interaction;
mod module_runtime;
mod plan;
mod recovery;
mod semantic;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use chrono::Utc;
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    InteractionAction, InteractionEffect, InteractionEvent, InteractionProposalDecision,
    InteractionProposalRecord, InteractionProposalRecordId, InteractionProposalStatus,
    InteractionRule, InteractionRuleId, InteractionState, MessageId, Sha256Digest,
    ValidateOrchestration, VersionedJson,
};
use lorepia_orchestration::{
    AppliedModuleRuntimePlan, InteractionOutcome, InteractionRuleStatus, ModuleMergeReview,
    decide_pending, expire_pending_proposal,
};
use lorepia_storage::{
    GenerationApprovalEvidence, GenerationAttemptBeforeReviewCommit,
    GenerationAttemptDerivedClosure, GenerationAttemptDerivedGuardAudit,
    GenerationAttemptDerivedGuardKind, GenerationAttemptDerivedTransition,
    GenerationAttemptProposalDecision, GenerationAttemptProposalDecisionCommit,
    GenerationAttemptStatus, GenerationBeforeEventEvidence, InteractionActionResultStatus,
    InteractionActionResultWrite, InteractionChoiceSelectionCommit, InteractionDerivedEventCommit,
    InteractionDerivedEventWrite, InteractionDerivedOccurrenceCommit, InteractionEvaluationSeal,
    InteractionEventCommit, InteractionEventOccurrenceLookup, InteractionKnowledgeBinding,
    InteractionPolicySnapshot, InteractionProposalApprovalCommit,
    InteractionProposalRejectionCommit, InteractionProposalWrite, LifecycleOccurrenceKind,
    RetryableGenerationAttemptProjection, StoredGenerationAttempt, StoredGenerationAttemptProposal,
    StoredInteractionDerivedEvent, StoredInteractionEvent, StoredInteractionProposal,
    StoredInteractionState, StoredLifecycleOccurrence, generation_attempt_derived_chain_sha256,
    generation_attempt_derived_closure_sha256, generation_attempt_derived_event_sha256,
    generation_attempt_derived_transition_commit_sha256,
    generation_attempt_derived_transition_sha256, interaction_action_sha256,
    interaction_evaluation_seal_sha256, interaction_proposal_review_sha256,
};
use serde::{Deserialize, Serialize};
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
#[cfg(test)]
use self::embedding::memory_embedding_candidate_limit;
use self::embedding::memory_embedding_job_seed;
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
pub(crate) use self::recovery::core_lifecycle_retry_seconds;
pub use self::recovery::{
    CoreLifecycleDeliveryReceipt, CoreLifecycleDeliveryStatus, CoreLifecycleDrainReceipt,
    InterruptedMemoryJob, MemoryQueryEmbeddingRetryCandidate,
};
pub(crate) use self::semantic::{MemorySemanticQueryEvidence, ResolvedMemorySemanticQuery};
use crate::{
    Core, InteractionChoiceSelectionReceipt, app::generation_attempt_module_authority,
    interaction_projection::project_interaction_choice_selection_receipt,
};

const MAX_GENERATION_ATTEMPT_DERIVED_EVENTS: usize = 256;
const MAX_GENERATION_ATTEMPT_DERIVED_DEPTH: u32 = 16;
const MAX_GENERATION_ATTEMPT_DERIVED_GUARDS: usize = 1_024;
const MAX_GENERATION_PROPOSAL_ROOM_PAGE: u32 = 100;
const MAX_GENERATION_PROPOSALS_PER_ATTEMPT: u32 = 1_024;

/// A decision can identify only one exact durable proposal record.
///
/// No action name or arguments are accepted. Approval dispatches the proposal
/// ID persisted in that record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalDecisionRequest {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub proposal_record_id: InteractionProposalRecordId,
    pub expected_state_revision: u64,
    pub expected_proposal_revision: u64,
    pub decision: InteractionProposalDecision,
}

/// Result of one durable proposal decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalDecisionReceipt {
    pub proposal: InteractionProposalRecord,
    pub state_revision: u64,
    pub effects: Vec<InteractionEffect>,
}

struct InteractionProposalApprovalInput<'a> {
    request: &'a InteractionProposalDecisionRequest,
    stored: &'a StoredInteractionProposal,
    decision_state: InteractionState,
    existing_knowledge: &'a [InteractionKnowledgeBinding],
    decided_at: chrono::DateTime<Utc>,
}

/// One isolated generation-attempt proposal plus the only current aggregate
/// CAS tokens a native caller may echo back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalView {
    pub proposal: StoredGenerationAttemptProposal,
    pub aggregate_revision: u64,
    pub interaction_state_revision: u64,
    pub pending_proposal_count: u32,
}

/// Decides one exact attempt-owned proposal discovered from its source room.
///
/// Core derives the decision idempotency key, trusted timestamp, policy,
/// state transition, and any approved `UserAction` materialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalDecisionRequest {
    pub conversation_id: ConversationId,
    pub source_branch_id: ConversationBranchId,
    pub generation_id: GenerationId,
    pub proposal_record_id: InteractionProposalRecordId,
    pub expected_aggregate_revision: u64,
    pub expected_proposal_revision: u64,
    pub decision: InteractionProposalDecision,
}

/// Safe decision outcome for one isolated generation aggregate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalDecisionReceipt {
    pub proposal: StoredGenerationAttemptProposal,
    pub aggregate_revision: u64,
    pub interaction_state_revision: u64,
    pub pending_proposal_count: u32,
    pub approval_evidence_sha256: Option<Sha256Digest>,
    pub exact_replay: bool,
}

/// One bounded due-proposal maintenance pass for a source room.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalExpiryReceipt {
    pub decisions: Vec<GenerationAttemptProposalDecisionReceipt>,
    pub has_more_due: bool,
}

#[derive(Debug)]
enum ProcessedCoreLifecycleOccurrence {
    Acknowledged {
        before_generation_evidence: Option<GenerationBeforeEventEvidence>,
        approval_evidence: Option<GenerationApprovalEvidence>,
    },
    AwaitingApproval {
        before_generation_evidence: Option<GenerationBeforeEventEvidence>,
    },
}

impl Core {
    fn process_core_lifecycle_occurrence(
        &self,
        occurrence: &StoredLifecycleOccurrence,
    ) -> CoreResult<ProcessedCoreLifecycleOccurrence> {
        self.validate_core_lifecycle_occurrence_shape(occurrence)?;
        if occurrence.event_kind == LifecycleOccurrenceKind::BeforeGeneration {
            return self.process_before_generation_occurrence(occurrence);
        }

        let attempt = occurrence
            .generation_id
            .as_ref()
            .map(|generation_id| self.storage().get_generation_attempt(generation_id))
            .transpose()?;
        if let Some(attempt) = attempt.as_ref() {
            Self::validate_lifecycle_attempt_authority(occurrence, attempt)?;
        }

        if occurrence.event_kind == LifecycleOccurrenceKind::MessageCommitted {
            let generation_id = occurrence.generation_id.as_ref().ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "message-committed lifecycle occurrence is missing its generation",
                    false,
                )
            })?;
            let attempt = attempt.as_ref().ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "message-committed lifecycle generation attempt is missing",
                    false,
                )
            })?;
            self.process_interaction_event_with_authority(
                &InteractionReviewRequest {
                    conversation_id: occurrence.conversation_id.clone(),
                    branch_id: occurrence.branch_id.clone(),
                    expected_head: occurrence.exact_head_message_id.clone(),
                    event: InteractionEvent::AfterGeneration,
                },
                &format!("after-generation:{}", generation_id.0),
                Some(generation_id),
                None,
                occurrence.occurred_at,
                false,
                Some(&attempt.input.module_plan_sha256),
            )?;
        }

        let event = match occurrence.event_kind {
            LifecycleOccurrenceKind::ConversationOpened => InteractionEvent::ConversationOpened,
            LifecycleOccurrenceKind::ConversationStarted => InteractionEvent::ConversationStarted,
            LifecycleOccurrenceKind::AfterGeneration => InteractionEvent::AfterGeneration,
            LifecycleOccurrenceKind::MessageCommitted => InteractionEvent::MessageCommitted,
            LifecycleOccurrenceKind::BeforeGeneration => {
                return Err(CoreError::internal(
                    "before-generation lifecycle routing invariant failed",
                ));
            }
        };
        let interaction_generation_id = (occurrence.event_kind
            == LifecycleOccurrenceKind::AfterGeneration)
            .then_some(occurrence.generation_id.as_ref())
            .flatten();
        let interaction_owner_message_id = (occurrence.event_kind
            == LifecycleOccurrenceKind::MessageCommitted)
            .then_some(occurrence.owner_message_id.as_ref())
            .flatten();
        let expected_module_plan_sha256 = attempt
            .as_ref()
            .map(|attempt| &attempt.input.module_plan_sha256);
        self.process_interaction_event_with_authority(
            &InteractionReviewRequest {
                conversation_id: occurrence.conversation_id.clone(),
                branch_id: occurrence.branch_id.clone(),
                expected_head: occurrence.exact_head_message_id.clone(),
                event,
            },
            &occurrence.occurrence_id,
            interaction_generation_id,
            interaction_owner_message_id,
            occurrence.occurred_at,
            false,
            expected_module_plan_sha256,
        )?;

        if occurrence.event_kind == LifecycleOccurrenceKind::MessageCommitted {
            let _ = self.try_enqueue_memory_summary_with_authority(
                &EnqueueMemorySummaryRequest {
                    conversation_id: occurrence.conversation_id.clone(),
                    branch_id: occurrence.branch_id.clone(),
                    expected_head: occurrence.owner_message_id.clone(),
                },
                MemorySummaryHeadAuthority::HistoricalCommittedHead,
            )?;
        }

        Ok(ProcessedCoreLifecycleOccurrence::Acknowledged {
            before_generation_evidence: None,
            approval_evidence: None,
        })
    }

    fn validate_core_lifecycle_occurrence_shape(
        &self,
        occurrence: &StoredLifecycleOccurrence,
    ) -> CoreResult<()> {
        let valid = match occurrence.event_kind {
            LifecycleOccurrenceKind::ConversationOpened
            | LifecycleOccurrenceKind::ConversationStarted => {
                occurrence.generation_id.is_none() && occurrence.owner_message_id.is_none()
            }
            LifecycleOccurrenceKind::BeforeGeneration => {
                occurrence.generation_id.is_some() && occurrence.owner_message_id.is_none()
            }
            LifecycleOccurrenceKind::AfterGeneration => occurrence.generation_id.is_some(),
            LifecycleOccurrenceKind::MessageCommitted => {
                occurrence.generation_id.is_some()
                    && occurrence.owner_message_id.is_some()
                    && occurrence.owner_message_id == occurrence.exact_head_message_id
            }
        };
        if !valid {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "stored Core lifecycle occurrence has an invalid authority shape",
                false,
            ));
        }
        self.validate_runtime_branch_identity(&occurrence.conversation_id, &occurrence.branch_id)?;
        Ok(())
    }

    fn validate_lifecycle_attempt_authority(
        occurrence: &StoredLifecycleOccurrence,
        attempt: &StoredGenerationAttempt,
    ) -> CoreResult<()> {
        if attempt.input.conversation_id != occurrence.conversation_id
            || attempt.input.proposed_branch_id != occurrence.branch_id
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "lifecycle occurrence differs from its immutable generation attempt",
                false,
            ));
        }
        if occurrence.event_kind == LifecycleOccurrenceKind::BeforeGeneration {
            let expected_occurrence_head =
                if attempt.input.proposed_branch_id == attempt.input.source_branch_id {
                    attempt.input.expected_head_message_id.as_ref()
                } else {
                    attempt.input.context_head_message_id.as_ref()
                };
            if expected_occurrence_head != occurrence.exact_head_message_id.as_ref() {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "before-generation occurrence head differs from its immutable attempt",
                    false,
                ));
            }
        } else if matches!(
            occurrence.event_kind,
            LifecycleOccurrenceKind::AfterGeneration | LifecycleOccurrenceKind::MessageCommitted
        ) && attempt.status != GenerationAttemptStatus::Completed
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "terminal lifecycle occurrence is not backed by a completed generation attempt",
                false,
            ));
        }
        Ok(())
    }

    fn process_before_generation_occurrence(
        &self,
        occurrence: &StoredLifecycleOccurrence,
    ) -> CoreResult<ProcessedCoreLifecycleOccurrence> {
        let generation_id = occurrence.generation_id.as_ref().ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "before-generation occurrence is missing its generation attempt",
                false,
            )
        })?;
        let mut attempt = self.storage().get_generation_attempt(generation_id)?;
        Self::validate_lifecycle_attempt_authority(occurrence, &attempt)?;

        if attempt.status == GenerationAttemptStatus::FailedBeforeDispatch {
            return Err(CoreError::new(
                CoreErrorCode::PermissionDenied,
                "generation attempt requires an explicit pre-dispatch retry",
                true,
            ));
        }

        // A pending proposal created by an older lifecycle event is not part
        // of this attempt's immutable BeforeGeneration evidence. Wait for it
        // to resolve before evaluating a new BeforeGeneration occurrence.
        // Generation preparation and resume never expire or otherwise mutate
        // ordinary branch proposals. Current-room refresh owns that explicit,
        // idempotent maintenance path; attempt-owned proposal expiry remains
        // isolated in the generation-attempt aggregate.
        if attempt.status == GenerationAttemptStatus::Prepared
            && !self
                .storage()
                .list_interaction_proposals(
                    &occurrence.conversation_id,
                    &occurrence.branch_id,
                    InteractionProposalStatus::Pending,
                    1,
                )?
                .is_empty()
        {
            return Ok(ProcessedCoreLifecycleOccurrence::AwaitingApproval {
                before_generation_evidence: None,
            });
        }

        if attempt.status == GenerationAttemptStatus::Prepared {
            let boundary = self
                .storage()
                .get_generation_attempt_interaction_boundary(generation_id)?;
            let (module_runtime_review, applied_module_plan) =
                generation_attempt_module_authority(&attempt)?;
            let review = self.prepare_generation_attempt_before_review(
                &attempt,
                &boundary.state,
                &boundary.context_checkpoint_sha256,
                module_runtime_review,
                applied_module_plan,
                occurrence.occurred_at,
            )?;
            self.storage()
                .commit_generation_attempt_before_review(&review)?;
            attempt = self.storage().get_generation_attempt(generation_id)?;
        }

        let before_generation_evidence =
            attempt.before_generation_evidence.clone().ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation attempt status is missing BeforeGeneration evidence",
                    false,
                )
            })?;
        match attempt.status {
            GenerationAttemptStatus::AwaitingApproval => {
                Ok(ProcessedCoreLifecycleOccurrence::AwaitingApproval {
                    before_generation_evidence: Some(before_generation_evidence),
                })
            }
            GenerationAttemptStatus::BeforeGenerationApplied
            | GenerationAttemptStatus::DispatchReady
            | GenerationAttemptStatus::Running
            | GenerationAttemptStatus::Completed => {
                Ok(ProcessedCoreLifecycleOccurrence::Acknowledged {
                    before_generation_evidence: Some(before_generation_evidence),
                    approval_evidence: attempt.approval_evidence,
                })
            }
            GenerationAttemptStatus::Prepared => Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt remained prepared after BeforeGeneration commit",
                false,
            )),
            GenerationAttemptStatus::FailedBeforeDispatch => {
                unreachable!("failed-before-dispatch attempts return before lifecycle evaluation")
            }
        }
    }

    /// Prepares the immutable attempt-owned `BeforeGeneration` snapshot used
    /// by both same-branch sends and historical edit/regenerate forks.
    ///
    /// This is a pure review over an already verified boundary. The returned
    /// storage commit does not target a live interaction-state key and cannot
    /// create a branch, proposal, effect, message, or generation row.
    fn validate_generation_attempt_before_review_authority(
        attempt: &StoredGenerationAttempt,
        boundary: &StoredInteractionState,
        context_checkpoint_sha256: &str,
        module_runtime_review: &ModuleMergeReview,
        applied_runtime_plan: Option<&AppliedModuleRuntimePlan>,
    ) -> CoreResult<()> {
        if boundary.key.conversation_id != attempt.input.conversation_id
            || boundary.key.branch_id != attempt.input.source_branch_id
        {
            return Err(CoreError::invalid(
                "generation attempt interaction boundary differs from its source lineage",
            ));
        }
        Sha256Digest::parse(context_checkpoint_sha256.to_owned()).map_err(CoreError::invalid)?;
        let (sealed_review, sealed_plan) = generation_attempt_module_authority(attempt)?;
        if module_runtime_review != sealed_review || applied_runtime_plan != sealed_plan {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation attempt review differs from its sealed module authority",
                false,
            ));
        }
        let applied_module_plan_sha256 = if let Some(applied) = applied_runtime_plan {
            applied.verify().map_err(module_plan_error)?;
            if applied.review != *module_runtime_review {
                return Err(CoreError::invalid(
                    "generation attempt applied plan differs from its runtime review",
                ));
            }
            applied.applied_plan_sha256.clone()
        } else {
            if !module_runtime_review.ordered_bindings.is_empty() {
                return Err(CoreError::new(
                    CoreErrorCode::PermissionDenied,
                    "an applicable module binding has no exact applied runtime plan",
                    false,
                ));
            }
            lorepia_orchestration::no_applied_module_runtime_plan_sha256()
        };
        if applied_module_plan_sha256 != attempt.input.module_plan_sha256 {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation attempt module plan changed before interaction review",
                true,
            ));
        }
        Ok(())
    }

    pub(crate) fn prepare_generation_attempt_before_review(
        &self,
        attempt: &StoredGenerationAttempt,
        boundary: &StoredInteractionState,
        context_checkpoint_sha256: &str,
        module_runtime_review: &ModuleMergeReview,
        applied_runtime_plan: Option<&AppliedModuleRuntimePlan>,
        occurred_at: chrono::DateTime<Utc>,
    ) -> CoreResult<GenerationAttemptBeforeReviewCommit> {
        Self::validate_generation_attempt_before_review_authority(
            attempt,
            boundary,
            context_checkpoint_sha256,
            module_runtime_review,
            applied_runtime_plan,
        )?;

        let request = InteractionReviewRequest {
            conversation_id: attempt.input.conversation_id.clone(),
            branch_id: attempt.input.proposed_branch_id.clone(),
            expected_head: attempt.input.context_head_message_id.clone(),
            event: InteractionEvent::BeforeGeneration,
        };
        let prepared = self.prepare_proposed_branch_interaction_review_from_state(
            &request,
            boundary.state.clone(),
            &boundary.knowledge,
            occurred_at,
            applied_runtime_plan,
        )?;
        let policy = interaction_policy_snapshot(&prepared.policy);
        let artifacts = interaction_commit_artifacts(
            &boundary.state,
            &prepared.public.outcome,
            &prepared.policy,
            &request,
            &prepared.evaluation_seal,
            &boundary.knowledge,
        )?;
        let event_sha256 = versioned_digest(&(
            "lorepia.generation-attempt-before-event.v1",
            &attempt.generation_id,
            &request,
            occurred_at,
            &prepared.public.review_sha256,
        ))?;
        let event_id = format!("interaction-event-{event_sha256}");
        let derived_closure = prepare_generation_attempt_derived_closure(
            &attempt.generation_id,
            &event_id,
            &request,
            &boundary.state,
            &prepared,
            &artifacts,
            occurred_at,
        )?;
        let mut proposals = derived_closure
            .transitions
            .iter()
            .flat_map(|transition| transition.proposals.iter().cloned())
            .collect::<Vec<_>>();
        proposals.sort_by(|left, right| left.record.id.cmp(&right.record.id));
        let memory_head_snapshot = self
            .storage()
            .list_memory_records_at_head(
                &attempt.input.conversation_id,
                &attempt.input.source_branch_id,
                attempt.input.context_head_message_id.as_ref(),
                false,
            )?
            .snapshot;
        Ok(GenerationAttemptBeforeReviewCommit {
            generation_id: attempt.generation_id.clone(),
            expected_attempt_revision: attempt.revision,
            event_id,
            occurred_at,
            context_head_message_id: attempt.input.context_head_message_id.clone(),
            context_checkpoint_sha256: context_checkpoint_sha256.to_owned(),
            previous_state: boundary.state.clone(),
            previous_knowledge: boundary.knowledge.clone(),
            module_runtime_review: module_runtime_review.clone(),
            memory_head_snapshot,
            applied_runtime_plan: applied_runtime_plan.cloned(),
            policy,
            evaluation_seal: prepared.evaluation_seal.clone(),
            derived_closure,
            next_state: prepared.public.outcome.state.clone(),
            knowledge: artifacts.knowledge,
            action_results: artifacts.action_results,
            effects: prepared.public.outcome.effects.clone(),
            derived_events: artifacts.derived_events,
            proposals,
            review_sha256: prepared.public.review_sha256.clone(),
        })
    }

    /// Commits one trusted durable lifecycle occurrence.
    ///
    /// A persisted outbox occurrence may legitimately lag behind the branch
    /// head. Such delivery validates the immutable room identity and exact
    /// occurrence fields, but does not reinterpret `expected_head` as a fresh
    /// optimistic concurrency token. Generation-owned occurrences also bind
    /// the freshly resolved policy to the immutable attempt module-plan hash.
    #[allow(clippy::too_many_arguments)]
    fn process_interaction_event_with_authority(
        &self,
        request: &InteractionReviewRequest,
        occurrence_id: &str,
        generation_attempt_id: Option<&GenerationId>,
        owner_message_id: Option<&MessageId>,
        occurred_at: chrono::DateTime<Utc>,
        enforce_current_head: bool,
        expected_module_plan_sha256: Option<&Sha256Digest>,
    ) -> CoreResult<StoredInteractionEvent> {
        validate_runtime_occurrence_id(occurrence_id)?;
        validate_interaction_event_authority_binding(
            &request.event,
            generation_attempt_id,
            owner_message_id,
        )?;
        let (event_id, idempotency_key) = interaction_occurrence_identity(
            request,
            occurrence_id,
            generation_attempt_id,
            owner_message_id,
            occurred_at,
        )?;
        if let Some(replay) = self.storage().get_interaction_event_by_occurrence(
            &InteractionEventOccurrenceLookup {
                event_id: event_id.clone(),
                idempotency_key: idempotency_key.clone(),
                conversation_id: request.conversation_id.clone(),
                branch_id: request.branch_id.clone(),
                event: request.event.clone(),
                generation_attempt_id: generation_attempt_id.cloned(),
                owner_message_id: owner_message_id.cloned(),
                occurred_at,
            },
        )? {
            validate_expected_interaction_module_plan(&replay.policy, expected_module_plan_sha256)?;
            self.drain_interaction_derived_events()?;
            return Ok(replay);
        }
        if enforce_current_head {
            self.validate_runtime_branch_head(
                &request.conversation_id,
                &request.branch_id,
                request.expected_head.as_ref(),
            )?;
        } else {
            self.validate_runtime_branch_identity(&request.conversation_id, &request.branch_id)?;
        }
        let policy =
            self.resolve_interaction_policy(&request.conversation_id, &request.branch_id)?;
        let state_key = interaction_state_key(&request.conversation_id, &request.branch_id)?;
        let initial_state = initial_interaction_state(&policy);
        let initial_knowledge = interaction_knowledge_bindings(&initial_state, &policy, &[])?;
        self.storage().get_or_init_interaction_state(
            &state_key,
            &initial_state,
            &initial_knowledge,
            occurred_at,
        )?;
        let snapshot = self
            .storage()
            .get_interaction_state_snapshot(&request.conversation_id, &request.branch_id)?;
        let state = snapshot.state;
        // This review is intentionally created only after the durable state
        // read/init. The subsequent transaction independently CAS-checks the
        // same revision, so no caller-supplied review can be committed.
        let prepared = self.prepare_interaction_review_from_state(
            request,
            state.clone(),
            &snapshot.knowledge,
            Some(occurred_at),
            enforce_current_head,
        )?;
        let policy_snapshot = interaction_policy_snapshot(&prepared.policy);
        validate_expected_interaction_module_plan(&policy_snapshot, expected_module_plan_sha256)?;
        let artifacts = interaction_commit_artifacts(
            &state,
            &prepared.public.outcome,
            &prepared.policy,
            request,
            &prepared.evaluation_seal,
            &snapshot.knowledge,
        )?;
        let stored = self
            .storage()
            .commit_interaction_event(&InteractionEventCommit {
                event_id,
                idempotency_key,
                key: snapshot.key,
                expected_state_revision: state.revision,
                event: request.event.clone(),
                generation_attempt_id: generation_attempt_id.cloned(),
                owner_message_id: owner_message_id.cloned(),
                policy: policy_snapshot,
                evaluation_seal: Some(prepared.evaluation_seal.clone()),
                deterministic_seed: Some(prepared.deterministic_seed),
                next_state: prepared.public.outcome.state,
                knowledge: artifacts.knowledge,
                action_results: artifacts.action_results,
                effects: prepared.public.outcome.effects,
                derived_events: artifacts.derived_events,
                proposals: artifacts.proposals,
                created_at: occurred_at,
            })?;
        self.drain_interaction_derived_events()?;
        Ok(stored)
    }

    pub(crate) fn process_interaction_derived_occurrence(
        &self,
        occurrence: &StoredInteractionDerivedEvent,
    ) -> CoreResult<Option<StoredInteractionEvent>> {
        let branch = self
            .validate_runtime_branch_identity(&occurrence.conversation_id, &occurrence.branch_id)?;
        let policy = match self.resolve_sealed_interaction_policy(
            &occurrence.conversation_id,
            &occurrence.branch_id,
            &occurrence.policy,
            &occurrence.evaluation_seal,
        ) {
            Ok(policy) => policy,
            Err(error) if error.recoverable => return Err(error),
            Err(_) => {
                let active_policy = self
                    .resolve_interaction_policy(&occurrence.conversation_id, &occurrence.branch_id)
                    .ok()
                    .map(|policy| interaction_policy_snapshot(&policy));
                self.storage()
                    .quarantine_interaction_derived_event_authority_failure(
                        &occurrence.occurrence_id,
                        occurrence.delivery_attempts,
                        active_policy.as_ref(),
                        Utc::now(),
                    )?;
                return Ok(None);
            }
        };
        let snapshot = self
            .storage()
            .get_interaction_state_snapshot(&occurrence.conversation_id, &occurrence.branch_id)?;
        let request = InteractionReviewRequest {
            conversation_id: occurrence.conversation_id.clone(),
            branch_id: occurrence.branch_id.clone(),
            expected_head: branch.head_message_id,
            event: occurrence.event.clone(),
        };
        let prepared = Self::prepare_interaction_review_with_sealed_authority(
            &request,
            snapshot.state.clone(),
            &snapshot.knowledge,
            occurrence.occurred_at,
            policy,
            occurrence.evaluation_seal.clone(),
            occurrence.deterministic_seed,
        )?;
        let artifacts = interaction_commit_artifacts(
            &snapshot.state,
            &prepared.public.outcome,
            &prepared.policy,
            &request,
            &prepared.evaluation_seal,
            &snapshot.knowledge,
        )?;
        self.storage()
            .commit_interaction_derived_occurrence(&InteractionDerivedOccurrenceCommit {
                occurrence_id: occurrence.occurrence_id.clone(),
                expected_delivery_attempts: occurrence.delivery_attempts,
                key: snapshot.key,
                expected_state_revision: snapshot.state.revision,
                next_state: prepared.public.outcome.state,
                knowledge: artifacts.knowledge,
                action_results: artifacts.action_results,
                effects: prepared.public.outcome.effects,
                derived_events: artifacts.derived_events,
                proposals: artifacts.proposals,
                committed_at: Utc::now(),
            })
            .map(Some)
    }

    fn approve_interaction_proposal_decision(
        &self,
        input: InteractionProposalApprovalInput<'_>,
    ) -> CoreResult<InteractionProposalDecisionReceipt> {
        let InteractionProposalApprovalInput {
            request,
            stored,
            decision_state,
            existing_knowledge,
            decided_at,
        } = input;
        let branch = self.storage().get_conversation_branch(&request.branch_id)?;
        let review_request = InteractionReviewRequest {
            conversation_id: request.conversation_id.clone(),
            branch_id: request.branch_id.clone(),
            expected_head: branch.head_message_id.clone(),
            event: InteractionEvent::UserAction {
                action_id: stored.record.proposal_id.clone(),
            },
        };
        let prepared = self.prepare_interaction_review_from_state(
            &review_request,
            decision_state.clone(),
            existing_knowledge,
            Some(decided_at),
            true,
        )?;
        if !prepared
            .public
            .rule_sets
            .iter()
            .any(|revision| revision.revision_id == stored.rule_set_revision_id)
        {
            return Err(CoreError::invalid(
                "proposal source rule revision is no longer approved for this branch",
            ));
        }
        let artifacts = interaction_commit_artifacts(
            &decision_state,
            &prepared.public.outcome,
            &prepared.policy,
            &review_request,
            &prepared.evaluation_seal,
            existing_knowledge,
        )?;
        let event_sha256 = versioned_digest(&(
            "lorepia.interaction-proposal-action.v1",
            &request.proposal_record_id,
            request.expected_state_revision,
            request.expected_proposal_revision,
        ))?;
        let logical_state_changed = {
            let mut logical = prepared.public.outcome.state.clone();
            logical.revision = decision_state.revision;
            logical != decision_state
        };
        let current_policy = interaction_policy_snapshot(&prepared.policy);
        let derived = (logical_state_changed
            || !artifacts.action_results.is_empty()
            || !prepared.public.outcome.effects.is_empty()
            || !artifacts.proposals.is_empty())
        .then(|| InteractionDerivedEventCommit {
            event_id: format!("interaction-event-{event_sha256}"),
            idempotency_key: format!("interaction-proposal-action:v1:{event_sha256}"),
            policy: current_policy.clone(),
            evaluation_seal: Some(prepared.evaluation_seal.clone()),
            deterministic_seed: Some(prepared.deterministic_seed),
            next_state: prepared.public.outcome.state,
            knowledge: artifacts.knowledge,
            action_results: artifacts.action_results,
            effects: prepared.public.outcome.effects.clone(),
            derived_events: artifacts.derived_events,
            proposals: artifacts.proposals,
            created_at: decided_at,
        });
        let approval =
            self.storage()
                .approve_interaction_proposal(&InteractionProposalApprovalCommit {
                    proposal_record_id: request.proposal_record_id.clone(),
                    expected_state_revision: request.expected_state_revision,
                    expected_proposal_revision: request.expected_proposal_revision,
                    decided_at_epoch_seconds: decided_at.timestamp(),
                    current_policy,
                    decision_state,
                    derived,
                    updated_at: decided_at,
                })?;
        self.drain_interaction_derived_events()?;
        Ok(InteractionProposalDecisionReceipt {
            proposal: approval.proposal.record,
            state_revision: approval.resulting_state_revision,
            effects: prepared.public.outcome.effects,
        })
    }

    /// Decides one exact durable proposal record. Approval derives the only
    /// permitted `UserAction` from the stored proposal and saves its outcome in
    /// the same transaction as the proposal decision.
    pub fn decide_interaction_proposal(
        &self,
        request: &InteractionProposalDecisionRequest,
    ) -> CoreResult<InteractionProposalDecisionReceipt> {
        let stored = self
            .storage()
            .get_interaction_proposal(&request.proposal_record_id)?;
        if stored.conversation_id != request.conversation_id
            || stored.branch_id != request.branch_id
        {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "interaction proposal was not found in this branch",
                false,
            ));
        }
        if stored.record.status == InteractionProposalStatus::Pending
            && interaction_proposal_decision_requires_reviewable_text(request.decision)
        {
            require_reviewable_interaction_proposal_text(&stored.record)?;
        }
        let snapshot = self
            .storage()
            .get_interaction_state_snapshot(&request.conversation_id, &request.branch_id)?;
        let state = snapshot.state;
        let now = Utc::now();
        let decision = decide_pending(
            &state,
            &stored.record.proposal_id,
            request.decision,
            request.expected_state_revision,
            now.timestamp(),
        )
        .map_err(interaction_error)?;
        if decision.proposal.id != request.proposal_record_id {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "proposal decision resolved a different durable record",
                false,
            ));
        }
        match request.decision {
            InteractionProposalDecision::Reject => {
                let rejected = self.storage().reject_interaction_proposal(
                    &InteractionProposalRejectionCommit {
                        proposal_record_id: request.proposal_record_id.clone(),
                        expected_state_revision: request.expected_state_revision,
                        expected_proposal_revision: request.expected_proposal_revision,
                        decided_at_epoch_seconds: now.timestamp(),
                        decision_state: decision.state,
                        updated_at: now,
                    },
                )?;
                Ok(InteractionProposalDecisionReceipt {
                    proposal: rejected.record,
                    state_revision: request.expected_state_revision.checked_add(1).ok_or_else(
                        || CoreError::invalid("interaction state revision overflowed"),
                    )?,
                    effects: Vec::new(),
                })
            }
            InteractionProposalDecision::Approve => {
                self.approve_interaction_proposal_decision(InteractionProposalApprovalInput {
                    request,
                    stored: &stored,
                    decision_state: decision.state,
                    existing_knowledge: &snapshot.knowledge,
                    decided_at: now,
                })
            }
        }
    }

    /// Lists isolated generation-attempt proposals for one exact source room.
    ///
    /// The source-room query is restart-safe and bounded. Neither a transient
    /// frontend generation ID nor a materialized target branch is required.
    pub fn list_generation_attempt_proposals_for_source_room(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        status: InteractionProposalStatus,
        limit: u32,
    ) -> CoreResult<Vec<GenerationAttemptProposalView>> {
        self.validate_runtime_branch_identity(conversation_id, source_branch_id)?;
        if limit == 0 || limit > MAX_GENERATION_PROPOSAL_ROOM_PAGE {
            return Err(CoreError::invalid(
                "generation proposal room page must contain between 1 and 100 items",
            ));
        }
        let proposals = self
            .storage()
            .list_generation_attempt_proposals_for_source_room(
                conversation_id,
                source_branch_id,
                status,
                limit,
            )?;
        let mut aggregates = BTreeMap::new();
        let mut views = Vec::with_capacity(proposals.len());
        for proposal in proposals {
            if proposal.conversation_id != *conversation_id
                || proposal.source_branch_id != *source_branch_id
                || proposal.record.status != status
            {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation proposal room query returned mismatched authority",
                    false,
                ));
            }
            let generation_key = proposal.generation_id.0.clone();
            if !aggregates.contains_key(&generation_key) {
                aggregates.insert(
                    generation_key.clone(),
                    self.storage()
                        .get_generation_attempt_interaction_aggregate(&proposal.generation_id)?,
                );
            }
            let aggregate = aggregates.get(&generation_key).ok_or_else(|| {
                CoreError::internal("generation proposal aggregate cache is missing")
            })?;
            if aggregate.generation_id != proposal.generation_id {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation proposal aggregate belongs to a different attempt",
                    false,
                ));
            }
            views.push(GenerationAttemptProposalView {
                aggregate_revision: aggregate.aggregate_revision,
                interaction_state_revision: aggregate.state.revision,
                pending_proposal_count: aggregate.pending_proposal_count,
                proposal,
            });
        }
        Ok(views)
    }

    /// Lists non-sensitive generation attempts that can resume from one exact
    /// source room without exposing prompt, provider, operation, or nonce
    /// authority.
    pub fn list_retryable_generation_attempts_for_source_room(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<RetryableGenerationAttemptProjection>> {
        self.validate_runtime_branch_identity(conversation_id, source_branch_id)?;
        self.storage()
            .list_retryable_generation_attempts_for_source_room(
                conversation_id,
                source_branch_id,
                limit,
            )
    }

    /// Approves or rejects one exact isolated generation-attempt proposal.
    pub fn decide_generation_attempt_proposal(
        &self,
        request: &GenerationAttemptProposalDecisionRequest,
    ) -> CoreResult<GenerationAttemptProposalDecisionReceipt> {
        let decision = match request.decision {
            InteractionProposalDecision::Approve => GenerationAttemptProposalDecision::Approve,
            InteractionProposalDecision::Reject => GenerationAttemptProposalDecision::Reject,
        };
        self.decide_generation_attempt_proposal_with_disposition(
            &request.conversation_id,
            &request.source_branch_id,
            &request.generation_id,
            &request.proposal_record_id,
            request.expected_aggregate_revision,
            request.expected_proposal_revision,
            decision,
            Utc::now(),
        )
    }

    /// Expires a bounded set of due attempt-owned proposals for one source
    /// room. Each proposal advances its own attempt aggregate CAS exactly once
    /// and never derives a `UserAction`.
    pub fn expire_due_generation_attempt_proposals_for_source_room(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<GenerationAttemptProposalExpiryReceipt> {
        self.validate_runtime_branch_identity(conversation_id, source_branch_id)?;
        if limit == 0 || limit > MAX_GENERATION_PROPOSAL_ROOM_PAGE {
            return Err(CoreError::invalid(
                "generation proposal expiry page must contain between 1 and 100 items",
            ));
        }
        let now = Utc::now();
        let pending = self
            .storage()
            .list_generation_attempt_proposals_for_source_room(
                conversation_id,
                source_branch_id,
                InteractionProposalStatus::Pending,
                MAX_GENERATION_PROPOSALS_PER_ATTEMPT,
            )?;
        let due = pending
            .into_iter()
            .filter(|proposal| {
                proposal
                    .record
                    .expires_at_epoch_seconds
                    .is_some_and(|expires_at| now.timestamp() >= expires_at)
            })
            .collect::<Vec<_>>();
        let has_more_due = due.len() > limit as usize;
        let mut decisions = Vec::with_capacity(due.len().min(limit as usize));
        for proposal in due.into_iter().take(limit as usize) {
            let aggregate = self
                .storage()
                .get_generation_attempt_interaction_aggregate(&proposal.generation_id)?;
            decisions.push(self.decide_generation_attempt_proposal_with_disposition(
                conversation_id,
                source_branch_id,
                &proposal.generation_id,
                &proposal.record.id,
                aggregate.aggregate_revision,
                proposal.proposal_revision,
                GenerationAttemptProposalDecision::Expire,
                now,
            )?);
        }
        Ok(GenerationAttemptProposalExpiryReceipt {
            decisions,
            has_more_due,
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn decide_generation_attempt_proposal_with_disposition(
        &self,
        conversation_id: &ConversationId,
        source_branch_id: &ConversationBranchId,
        generation_id: &GenerationId,
        proposal_record_id: &InteractionProposalRecordId,
        expected_aggregate_revision: u64,
        expected_proposal_revision: u64,
        decision: GenerationAttemptProposalDecision,
        decided_at: chrono::DateTime<Utc>,
    ) -> CoreResult<GenerationAttemptProposalDecisionReceipt> {
        self.validate_runtime_branch_identity(conversation_id, source_branch_id)?;
        if expected_aggregate_revision == 0 || expected_proposal_revision == 0 {
            return Err(CoreError::invalid(
                "generation proposal decision CAS revisions must be positive",
            ));
        }
        let stored = self
            .storage()
            .get_generation_attempt_proposal(proposal_record_id)?;
        if stored.generation_id != *generation_id
            || stored.conversation_id != *conversation_id
            || stored.source_branch_id != *source_branch_id
        {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "generation proposal was not found in this source room",
                false,
            ));
        }
        let decision_sha256 = versioned_digest(&(
            "lorepia.generation-attempt-proposal-decision.v1",
            generation_id,
            proposal_record_id,
            expected_aggregate_revision,
            expected_proposal_revision,
            decision,
        ))?;
        let decision_idempotency_key = format!("generation-proposal-decision:v1:{decision_sha256}");
        let expected_status = match decision {
            GenerationAttemptProposalDecision::Approve => InteractionProposalStatus::Approved,
            GenerationAttemptProposalDecision::Reject => InteractionProposalStatus::Rejected,
            GenerationAttemptProposalDecision::Expire => InteractionProposalStatus::Expired,
        };
        if stored.record.status != InteractionProposalStatus::Pending {
            let expected_resulting_aggregate_revision = expected_aggregate_revision
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("generation aggregate revision overflowed"))?;
            let expected_resulting_proposal_revision = expected_proposal_revision
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("generation proposal revision overflowed"))?;
            if stored.record.status != expected_status
                || stored.decision_idempotency_key.as_deref()
                    != Some(decision_idempotency_key.as_str())
                || stored.resulting_aggregate_revision
                    != Some(expected_resulting_aggregate_revision)
                || stored.proposal_revision != expected_resulting_proposal_revision
            {
                return Err(CoreError::new(
                    CoreErrorCode::InvalidInput,
                    "generation proposal decision is stale or conflicts with its terminal record",
                    true,
                ));
            }
            let aggregate = self
                .storage()
                .get_generation_attempt_interaction_aggregate(generation_id)?;
            let before = self
                .storage()
                .get_generation_attempt_before_review(generation_id)?
                .ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "generation proposal is missing its immutable review",
                        false,
                    )
                })?;
            return Ok(GenerationAttemptProposalDecisionReceipt {
                proposal: stored,
                aggregate_revision: aggregate.aggregate_revision,
                interaction_state_revision: aggregate.state.revision,
                pending_proposal_count: aggregate.pending_proposal_count,
                approval_evidence_sha256: before.approval_evidence_sha256,
                exact_replay: true,
            });
        }
        if generation_proposal_decision_requires_reviewable_text(decision) {
            require_reviewable_interaction_proposal_text(&stored.record)?;
        }
        if stored.proposal_revision != expected_proposal_revision {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation proposal revision changed",
                true,
            ));
        }
        let aggregate = self
            .storage()
            .get_generation_attempt_interaction_aggregate(generation_id)?;
        if aggregate.aggregate_revision != expected_aggregate_revision {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "generation proposal aggregate revision changed",
                true,
            ));
        }
        let mut identity_proposals = Vec::new();
        for status in [
            InteractionProposalStatus::Pending,
            InteractionProposalStatus::Approved,
            InteractionProposalStatus::Rejected,
            InteractionProposalStatus::Expired,
        ] {
            identity_proposals.extend(self.storage().list_generation_attempt_proposals(
                generation_id,
                status,
                MAX_GENERATION_PROPOSALS_PER_ATTEMPT,
            )?);
        }
        if identity_proposals.len() > MAX_GENERATION_PROPOSALS_PER_ATTEMPT as usize {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation proposal identity set exceeds its durable bound",
                false,
            ));
        }
        let domain_aggregate_state = remap_generation_attempt_proposal_ids(
            generation_id,
            &aggregate.state,
            &identity_proposals,
            true,
        )?;
        let domain_decision_state = match decision {
            GenerationAttemptProposalDecision::Approve => {
                decide_pending(
                    &domain_aggregate_state,
                    &stored.record.proposal_id,
                    InteractionProposalDecision::Approve,
                    domain_aggregate_state.revision,
                    decided_at.timestamp(),
                )
                .map_err(interaction_error)?
                .state
            }
            GenerationAttemptProposalDecision::Reject => {
                decide_pending(
                    &domain_aggregate_state,
                    &stored.record.proposal_id,
                    InteractionProposalDecision::Reject,
                    domain_aggregate_state.revision,
                    decided_at.timestamp(),
                )
                .map_err(interaction_error)?
                .state
            }
            GenerationAttemptProposalDecision::Expire => {
                expire_pending_proposal(
                    &domain_aggregate_state,
                    &stored.record.proposal_id,
                    domain_aggregate_state.revision,
                    decided_at.timestamp(),
                )
                .map_err(interaction_error)?
                .state
            }
        };

        let (current_policy, evaluation_seal, derived_closure, derived) =
            if decision == GenerationAttemptProposalDecision::Approve {
                let attempt = self.storage().get_generation_attempt(generation_id)?;
                if attempt.status != GenerationAttemptStatus::AwaitingApproval {
                    return Err(CoreError::new(
                        CoreErrorCode::InvalidInput,
                        "generation attempt is no longer awaiting approval",
                        true,
                    ));
                }
                let sealed_module_plan_sha256 =
                    if let Some(sha256) = stored.origin_policy.module_plan_sha256.as_ref() {
                        Sha256Digest::parse(sha256.clone()).map_err(CoreError::invalid)?
                    } else {
                        lorepia_orchestration::no_applied_module_runtime_plan_sha256()
                    };
                if sealed_module_plan_sha256 != attempt.input.module_plan_sha256
                    || stored.origin_aggregate_revision > aggregate.aggregate_revision
                {
                    return Err(CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "generation proposal origin authority is inconsistent",
                        false,
                    ));
                }
                let policy = self.resolve_generation_attempt_proposal_policy(&stored)?;
                let sealed_event_at = chrono::DateTime::from_timestamp(
                    stored.origin_evaluation_seal.event_epoch_seconds,
                    0,
                )
                .ok_or_else(|| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        "generation proposal sealed timestamp is invalid",
                        false,
                    )
                })?;
                let user_action = InteractionEvent::UserAction {
                    action_id: stored.record.proposal_id.clone(),
                };
                let review_request = InteractionReviewRequest {
                    conversation_id: conversation_id.clone(),
                    branch_id: stored.proposed_branch_id.clone(),
                    expected_head: attempt.input.context_head_message_id.clone(),
                    event: user_action.clone(),
                };
                let prepared = Self::prepare_interaction_review_with_evaluation_seal(
                    &review_request,
                    domain_decision_state.clone(),
                    &aggregate.knowledge,
                    sealed_event_at,
                    policy,
                    stored.origin_evaluation_seal.clone(),
                )?;
                if !prepared
                    .public
                    .rule_sets
                    .iter()
                    .any(|revision| revision.revision_id == stored.rule_set_revision_id)
                {
                    return Err(CoreError::new(
                        CoreErrorCode::InvalidInput,
                        "generation proposal source rule revision is no longer active",
                        true,
                    ));
                }
                let policy = interaction_policy_snapshot(&prepared.policy);
                let artifacts = interaction_commit_artifacts(
                    &domain_decision_state,
                    &prepared.public.outcome,
                    &prepared.policy,
                    &review_request,
                    &prepared.evaluation_seal,
                    &aggregate.knowledge,
                )?;
                let event_id = format!("interaction-event-{decision_sha256}");
                let closure = prepare_generation_attempt_derived_closure(
                    generation_id,
                    &event_id,
                    &review_request,
                    &domain_decision_state,
                    &prepared,
                    &artifacts,
                    sealed_event_at,
                )?;
                let derived = InteractionDerivedEventCommit {
                    event_id,
                    idempotency_key: format!("generation-proposal-action:v1:{decision_sha256}"),
                    policy: policy.clone(),
                    evaluation_seal: Some(prepared.evaluation_seal.clone()),
                    deterministic_seed: Some(prepared.deterministic_seed),
                    next_state: prepared.public.outcome.state.clone(),
                    knowledge: artifacts.knowledge.clone(),
                    action_results: artifacts.action_results.clone(),
                    effects: prepared.public.outcome.effects.clone(),
                    derived_events: artifacts.derived_events.clone(),
                    proposals: artifacts.proposals.clone(),
                    created_at: sealed_event_at,
                };
                (
                    Some(policy),
                    Some(stored.origin_evaluation_seal.clone()),
                    Some(closure),
                    Some(derived),
                )
            } else {
                (None, None, None, None)
            };
        let decision_state = remap_generation_attempt_proposal_ids(
            generation_id,
            &domain_decision_state,
            &identity_proposals,
            false,
        )?;
        let derived_closure = derived_closure
            .map(|closure| {
                remap_generation_attempt_derived_closure_existing_proposals(
                    generation_id,
                    closure,
                    &identity_proposals,
                )
            })
            .transpose()?;
        let derived = derived
            .map(|mut derived| {
                derived.next_state = remap_generation_attempt_proposal_ids(
                    generation_id,
                    &derived.next_state,
                    &identity_proposals,
                    false,
                )?;
                Ok(derived)
            })
            .transpose()?;
        let receipt = self.storage().decide_generation_attempt_proposal(
            &GenerationAttemptProposalDecisionCommit {
                proposal_record_id: proposal_record_id.clone(),
                expected_proposal_revision,
                expected_aggregate_revision,
                decision,
                decision_idempotency_key,
                decided_at_epoch_seconds: decided_at.timestamp(),
                decision_state,
                current_policy,
                evaluation_seal,
                derived_closure,
                derived,
                updated_at: decided_at,
            },
        )?;
        Ok(GenerationAttemptProposalDecisionReceipt {
            aggregate_revision: receipt.aggregate.aggregate_revision,
            interaction_state_revision: receipt.aggregate.state.revision,
            pending_proposal_count: receipt.aggregate.pending_proposal_count,
            approval_evidence_sha256: receipt.approval_evidence_sha256,
            exact_replay: receipt.exact_replay,
            proposal: receipt.proposal,
        })
    }

    /// Selects one exact option from one exact durable `ChoicesPresented`
    /// effect and atomically commits the storage-derived `UserAction`.
    ///
    /// The frontend supplies neither an event kind nor action arguments. Core
    /// reloads the effect, validates room ownership and the exact stored
    /// option, recreates current policy/state review, and storage consumes the
    /// choice exactly once in the same transaction as the derived event.
    pub fn submit_interaction_choice(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        effect_id: &str,
        choice_id: &str,
        expected_state_revision: u64,
    ) -> CoreResult<InteractionChoiceSelectionReceipt> {
        let branch = self.validate_runtime_branch_identity(conversation_id, branch_id)?;
        let stored = self.storage().get_interaction_effect(effect_id)?;
        if stored.stored.conversation_id != *conversation_id
            || stored.stored.branch_id != *branch_id
        {
            return Err(CoreError::new(
                CoreErrorCode::NotFound,
                "interaction choice effect was not found in this branch",
                false,
            ));
        }
        let InteractionEffect::ChoicesPresented { choices } = &stored.stored.effect else {
            return Err(CoreError::invalid(
                "interaction effect does not present choices",
            ));
        };
        if !choices.iter().any(|choice| choice.id == choice_id) {
            return Err(CoreError::invalid(
                "interaction choice is not one of the exact stored options",
            ));
        }

        let snapshot = self
            .storage()
            .get_interaction_state_snapshot(conversation_id, branch_id)?;
        if snapshot.state.revision != expected_state_revision {
            return Err(CoreError::new(
                CoreErrorCode::InvalidInput,
                "interaction state changed before choice selection",
                true,
            ));
        }
        let now = Utc::now();
        let event = InteractionEvent::UserAction {
            action_id: choice_id.to_owned(),
        };
        let request = InteractionReviewRequest {
            conversation_id: conversation_id.clone(),
            branch_id: branch_id.clone(),
            expected_head: branch.head_message_id,
            event: event.clone(),
        };
        let prepared = self.prepare_interaction_review_from_state(
            &request,
            snapshot.state.clone(),
            &snapshot.knowledge,
            Some(now),
            true,
        )?;
        let artifacts = interaction_commit_artifacts(
            &snapshot.state,
            &prepared.public.outcome,
            &prepared.policy,
            &request,
            &prepared.evaluation_seal,
            &snapshot.knowledge,
        )?;
        let event_sha256 = versioned_digest(&(
            "lorepia.interaction-choice-action.v1",
            conversation_id,
            branch_id,
            effect_id,
            choice_id,
            expected_state_revision,
        ))?;
        let current_policy = interaction_policy_snapshot(&prepared.policy);
        let receipt =
            self.storage()
                .consume_interaction_choice(&InteractionChoiceSelectionCommit {
                    effect_id: effect_id.to_owned(),
                    choice_id: choice_id.to_owned(),
                    expected_state_revision,
                    selected_at_epoch_seconds: now.timestamp(),
                    current_policy: current_policy.clone(),
                    derived: InteractionDerivedEventCommit {
                        event_id: format!("interaction-event-{event_sha256}"),
                        idempotency_key: format!("interaction-choice-action:v1:{event_sha256}"),
                        policy: current_policy,
                        evaluation_seal: Some(prepared.evaluation_seal.clone()),
                        deterministic_seed: Some(prepared.deterministic_seed),
                        next_state: prepared.public.outcome.state,
                        knowledge: artifacts.knowledge,
                        action_results: artifacts.action_results,
                        effects: prepared.public.outcome.effects,
                        derived_events: artifacts.derived_events,
                        proposals: artifacts.proposals,
                        created_at: now,
                    },
                })?;
        self.drain_interaction_derived_events()?;
        Ok(project_interaction_choice_selection_receipt(receipt))
    }

    fn resolve_generation_attempt_proposal_policy(
        &self,
        proposal: &StoredGenerationAttemptProposal,
    ) -> CoreResult<ResolvedInteractionPolicy> {
        if interaction_evaluation_seal_sha256(&proposal.origin_evaluation_seal)?
            != proposal.origin_evaluation_seal_sha256
            || proposal.origin_evaluation_seal.policy_sha256 != proposal.origin_policy_sha256
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation proposal evaluation seal is inconsistent",
                false,
            ));
        }
        let before = self
            .storage()
            .get_generation_attempt_before_review(&proposal.generation_id)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation proposal is missing its immutable attempt review",
                    false,
                )
            })?;
        if before.evaluation_seal != proposal.origin_evaluation_seal {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation proposal evaluation seal differs from its attempt review",
                false,
            ));
        }
        let modules = if let Some(applied_plan_sha256) =
            proposal.origin_policy.module_plan_sha256.as_deref()
        {
            let applied_plan_sha256 =
                Sha256Digest::parse(applied_plan_sha256.to_owned()).map_err(CoreError::invalid)?;
            let applied = before.applied_runtime_plan.as_ref().ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation proposal attempt review is missing its applied module plan",
                    false,
                )
            })?;
            applied.verify().map_err(module_plan_error)?;
            if applied.applied_plan_sha256 != applied_plan_sha256 {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation proposal attempt plan hash is inconsistent",
                    false,
                ));
            }
            if applied.review.context.conversation_id.as_deref()
                != Some(proposal.conversation_id.0.as_str())
                || applied.review.context.branch_id.as_deref()
                    != Some(proposal.proposed_branch_id.0.as_str())
            {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation proposal module plan belongs to another target branch",
                    false,
                ));
            }
            self.materialize_resolved_module_runtime(applied)?
        } else {
            if before.applied_runtime_plan.is_some() {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "generation proposal no-module authority contains an applied plan",
                    false,
                ));
            }
            ResolvedModuleRuntime::default()
        };
        let policy = Self::resolve_interaction_policy_from_modules_with_evaluation_seal(
            &modules,
            &proposal.origin_evaluation_seal,
        )?;
        if interaction_policy_snapshot(&policy) != proposal.origin_policy {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation proposal sealed policy cannot be reconstructed exactly",
                false,
            ));
        }
        let event_at = chrono::DateTime::from_timestamp(
            proposal.origin_evaluation_seal.event_epoch_seconds,
            0,
        )
        .ok_or_else(|| {
            CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation proposal sealed event timestamp is invalid",
                false,
            )
        })?;
        validate_interaction_evaluation_seal(&policy, event_at, &proposal.origin_evaluation_seal)?;
        Ok(policy)
    }
}

fn validate_expected_interaction_module_plan(
    policy: &InteractionPolicySnapshot,
    expected: Option<&Sha256Digest>,
) -> CoreResult<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let matches = policy.module_plan_sha256.as_deref().map_or_else(
        || *expected == lorepia_orchestration::no_applied_module_runtime_plan_sha256(),
        |actual| actual == expected.as_str(),
    );
    if matches {
        Ok(())
    } else {
        Err(CoreError::new(
            CoreErrorCode::PermissionDenied,
            "interaction lifecycle policy differs from the immutable generation attempt",
            true,
        ))
    }
}

fn interaction_occurrence_identity(
    request: &InteractionReviewRequest,
    occurrence_id: &str,
    generation_attempt_id: Option<&GenerationId>,
    owner_message_id: Option<&MessageId>,
    occurred_at: chrono::DateTime<Utc>,
) -> CoreResult<(String, String)> {
    let occurrence_sha256 = versioned_digest(&(
        "lorepia.interaction-occurrence.v1",
        &request.conversation_id,
        &request.branch_id,
        occurrence_id,
        generation_attempt_id,
        owner_message_id,
        occurred_at,
        &request.event,
    ))?;
    Ok((
        format!("interaction-event-{occurrence_sha256}"),
        format!("interaction-event:v1:{occurrence_sha256}"),
    ))
}

#[derive(Debug)]
struct InteractionCommitArtifacts {
    knowledge: Vec<InteractionKnowledgeBinding>,
    action_results: Vec<InteractionActionResultWrite>,
    derived_events: Vec<InteractionDerivedEventWrite>,
    proposals: Vec<InteractionProposalWrite>,
}

#[derive(Clone)]
struct GenerationAttemptDerivedCandidate {
    parent_ordinal: u32,
    depth: u32,
    event: InteractionEvent,
    deterministic_seed: u64,
    visited_event_sha256s: BTreeSet<Sha256Digest>,
}

#[derive(Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationAttemptGuardFingerprint<'a> {
    schema_version: u32,
    kind: GenerationAttemptDerivedGuardKind,
    candidate_event_sha256: Option<&'a Sha256Digest>,
    parent_ordinal: u32,
    depth: u32,
    suppressed_count: u32,
}

struct GenerationAttemptTransitionInput<'a> {
    ordinal: u32,
    parent_ordinal: Option<u32>,
    depth: u32,
    event_id: &'a str,
    request: &'a InteractionReviewRequest,
    previous_state: &'a InteractionState,
    prepared: &'a PreparedInteractionReview,
    artifacts: &'a InteractionCommitArtifacts,
}

fn materialize_generation_attempt_transition(
    generation_id: &GenerationId,
    input: GenerationAttemptTransitionInput<'_>,
) -> CoreResult<GenerationAttemptDerivedTransition> {
    if input.prepared.public.expected_state_revision != input.previous_state.revision {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation derived transition review has the wrong state boundary",
            false,
        ));
    }
    let event_sha256 = generation_attempt_derived_event_sha256(&input.request.event)?;
    let deterministic_seed = input.prepared.deterministic_seed;
    let policy = interaction_policy_snapshot(&input.prepared.policy);
    let resulting_state_revision = input.prepared.public.outcome.state.revision;
    let mut transition = GenerationAttemptDerivedTransition {
        ordinal: input.ordinal,
        parent_ordinal: input.parent_ordinal,
        depth: input.depth,
        event_id: input.event_id.to_owned(),
        event: input.request.event.clone(),
        event_sha256,
        deterministic_seed,
        expected_state_revision: input.previous_state.revision,
        resulting_state_revision,
        policy,
        evaluation_seal: input.prepared.evaluation_seal.clone(),
        next_state: input.prepared.public.outcome.state.clone(),
        knowledge: input.artifacts.knowledge.clone(),
        action_results: input.artifacts.action_results.clone(),
        effects: input.prepared.public.outcome.effects.clone(),
        derived_events: input.artifacts.derived_events.clone(),
        proposals: input.artifacts.proposals.clone(),
        commit_sha256: Sha256Digest::parse("0".repeat(64)).map_err(CoreError::invalid)?,
    };
    transition.commit_sha256 =
        generation_attempt_derived_transition_commit_sha256(generation_id, &transition)?;
    generation_attempt_derived_transition_sha256(&transition)?;
    Ok(transition)
}

fn refresh_generation_attempt_guard_hash(
    audit: &mut GenerationAttemptDerivedGuardAudit,
) -> CoreResult<()> {
    audit.evidence_sha256 =
        Sha256Digest::parse(versioned_digest(&GenerationAttemptGuardFingerprint {
            schema_version: 1,
            kind: audit.kind,
            candidate_event_sha256: audit.candidate_event_sha256.as_ref(),
            parent_ordinal: audit.parent_ordinal,
            depth: audit.depth,
            suppressed_count: audit.suppressed_count,
        })?)
        .map_err(CoreError::invalid)?;
    Ok(())
}

fn record_generation_attempt_guard(
    audits: &mut Vec<GenerationAttemptDerivedGuardAudit>,
    kind: GenerationAttemptDerivedGuardKind,
    candidate_event_sha256: Option<Sha256Digest>,
    parent_ordinal: u32,
    depth: u32,
) -> CoreResult<()> {
    if let Some(existing) = audits.iter_mut().find(|audit| {
        audit.kind == kind
            && audit.candidate_event_sha256 == candidate_event_sha256
            && audit.parent_ordinal == parent_ordinal
            && audit.depth == depth
    }) {
        existing.suppressed_count = existing
            .suppressed_count
            .checked_add(1)
            .ok_or_else(|| CoreError::invalid("generation derived guard count overflowed"))?;
        return refresh_generation_attempt_guard_hash(existing);
    }
    if audits.len() >= MAX_GENERATION_ATTEMPT_DERIVED_GUARDS {
        return Err(CoreError::invalid(
            "generation derived guard audit bound was exceeded",
        ));
    }
    let mut audit = GenerationAttemptDerivedGuardAudit {
        kind,
        candidate_event_sha256,
        parent_ordinal,
        depth,
        suppressed_count: 1,
        evidence_sha256: Sha256Digest::parse("0".repeat(64)).map_err(CoreError::invalid)?,
    };
    refresh_generation_attempt_guard_hash(&mut audit)?;
    audits.push(audit);
    Ok(())
}

fn enqueue_generation_attempt_derived_candidates(
    queue: &mut VecDeque<GenerationAttemptDerivedCandidate>,
    audits: &mut Vec<GenerationAttemptDerivedGuardAudit>,
    accepted_event_count: usize,
    parent_ordinal: u32,
    parent_depth: u32,
    parent_visited_event_sha256s: &BTreeSet<Sha256Digest>,
    derived_events: &[InteractionDerivedEventWrite],
) -> CoreResult<()> {
    let depth = parent_depth
        .checked_add(1)
        .ok_or_else(|| CoreError::invalid("generation derived depth overflowed"))?;
    for derived in derived_events {
        let event_sha256 = generation_attempt_derived_event_sha256(&derived.event)?;
        if parent_visited_event_sha256s.contains(&event_sha256) {
            record_generation_attempt_guard(
                audits,
                GenerationAttemptDerivedGuardKind::Cycle,
                Some(event_sha256),
                parent_ordinal,
                depth,
            )?;
            continue;
        }
        if depth > MAX_GENERATION_ATTEMPT_DERIVED_DEPTH {
            record_generation_attempt_guard(
                audits,
                GenerationAttemptDerivedGuardKind::DepthLimit,
                Some(event_sha256),
                parent_ordinal,
                depth,
            )?;
            continue;
        }
        if accepted_event_count.saturating_add(queue.len()) >= MAX_GENERATION_ATTEMPT_DERIVED_EVENTS
        {
            record_generation_attempt_guard(
                audits,
                GenerationAttemptDerivedGuardKind::CountLimit,
                None,
                parent_ordinal,
                depth,
            )?;
            continue;
        }
        let mut visited_event_sha256s = parent_visited_event_sha256s.clone();
        visited_event_sha256s.insert(event_sha256);
        queue.push_back(GenerationAttemptDerivedCandidate {
            parent_ordinal,
            depth,
            event: derived.event.clone(),
            deterministic_seed: derived.deterministic_seed,
            visited_event_sha256s,
        });
    }
    Ok(())
}

fn prepare_generation_attempt_derived_closure(
    generation_id: &GenerationId,
    root_event_id: &str,
    root_request: &InteractionReviewRequest,
    previous_state: &InteractionState,
    root_prepared: &PreparedInteractionReview,
    root_artifacts: &InteractionCommitArtifacts,
    occurred_at: chrono::DateTime<Utc>,
) -> CoreResult<GenerationAttemptDerivedClosure> {
    let root_transition = materialize_generation_attempt_transition(
        generation_id,
        GenerationAttemptTransitionInput {
            ordinal: 0,
            parent_ordinal: None,
            depth: 0,
            event_id: root_event_id,
            request: root_request,
            previous_state,
            prepared: root_prepared,
            artifacts: root_artifacts,
        },
    )?;
    let root_event_sha256 = root_transition.event_sha256.clone();
    let mut transitions = vec![root_transition];
    let mut guards = Vec::new();
    let mut queue = VecDeque::new();
    let mut root_visited = BTreeSet::new();
    root_visited.insert(root_event_sha256);
    enqueue_generation_attempt_derived_candidates(
        &mut queue,
        &mut guards,
        transitions.len(),
        0,
        0,
        &root_visited,
        &root_artifacts.derived_events,
    )?;
    let mut current_state = root_prepared.public.outcome.state.clone();
    let mut current_knowledge = root_artifacts.knowledge.clone();
    while let Some(candidate) = queue.pop_front() {
        let ordinal = u32::try_from(transitions.len())
            .map_err(|_| CoreError::invalid("generation derived ordinal overflowed"))?;
        let event_id =
            generation_attempt_derived_event_id(generation_id, root_event_id, ordinal, &candidate)?;
        let request = InteractionReviewRequest {
            conversation_id: root_request.conversation_id.clone(),
            branch_id: root_request.branch_id.clone(),
            expected_head: root_request.expected_head.clone(),
            event: candidate.event.clone(),
        };
        let prepared = Core::prepare_interaction_review_with_sealed_authority(
            &request,
            current_state.clone(),
            &current_knowledge,
            occurred_at,
            root_prepared.policy.clone(),
            root_prepared.evaluation_seal.clone(),
            candidate.deterministic_seed,
        )?;
        if prepared.evaluation_seal != root_prepared.evaluation_seal {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation derived transition changed its sealed evaluation context",
                false,
            ));
        }
        let artifacts = interaction_commit_artifacts(
            &current_state,
            &prepared.public.outcome,
            &prepared.policy,
            &request,
            &prepared.evaluation_seal,
            &current_knowledge,
        )?;
        let transition = materialize_generation_attempt_transition(
            generation_id,
            GenerationAttemptTransitionInput {
                ordinal,
                parent_ordinal: Some(candidate.parent_ordinal),
                depth: candidate.depth,
                event_id: &event_id,
                request: &request,
                previous_state: &current_state,
                prepared: &prepared,
                artifacts: &artifacts,
            },
        )?;
        current_state = transition.next_state.clone();
        current_knowledge.clone_from(&transition.knowledge);
        transitions.push(transition);
        enqueue_generation_attempt_derived_candidates(
            &mut queue,
            &mut guards,
            transitions.len(),
            ordinal,
            candidate.depth,
            &candidate.visited_event_sha256s,
            &artifacts.derived_events,
        )?;
    }
    finalize_generation_attempt_derived_closure(
        transitions,
        guards,
        current_state,
        current_knowledge,
    )
}

fn generation_attempt_derived_event_id(
    generation_id: &GenerationId,
    root_event_id: &str,
    ordinal: u32,
    candidate: &GenerationAttemptDerivedCandidate,
) -> CoreResult<String> {
    versioned_digest(&(
        "lorepia.generation-attempt-derived-occurrence.v1",
        generation_id,
        root_event_id,
        ordinal,
        candidate.parent_ordinal,
        &candidate.event,
    ))
    .map(|sha256| format!("interaction-event-{sha256}"))
}

fn finalize_generation_attempt_derived_closure(
    transitions: Vec<GenerationAttemptDerivedTransition>,
    guard_audits: Vec<GenerationAttemptDerivedGuardAudit>,
    final_state: InteractionState,
    final_knowledge: Vec<InteractionKnowledgeBinding>,
) -> CoreResult<GenerationAttemptDerivedClosure> {
    let event_count = u32::try_from(transitions.len())
        .map_err(|_| CoreError::invalid("generation derived event count overflowed"))?;
    let guard_count = u32::try_from(guard_audits.len())
        .map_err(|_| CoreError::invalid("generation derived guard count overflowed"))?;
    let mut closure = GenerationAttemptDerivedClosure {
        schema_version: 1,
        transitions,
        guard_audits,
        final_state,
        final_knowledge,
        event_count,
        guard_count,
        chain_sha256: Sha256Digest::parse("0".repeat(64)).map_err(CoreError::invalid)?,
    };
    closure.chain_sha256 = generation_attempt_derived_chain_sha256(&closure)?;
    generation_attempt_derived_closure_sha256(&closure)?;
    Ok(closure)
}

fn validate_runtime_occurrence_id(value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || b"._:-".contains(&byte)))
    {
        return Err(CoreError::invalid(
            "interaction occurrence ID is empty or non-canonical",
        ));
    }
    Ok(())
}

fn validate_interaction_event_authority_binding(
    event: &InteractionEvent,
    generation_attempt_id: Option<&GenerationId>,
    owner_message_id: Option<&MessageId>,
) -> CoreResult<()> {
    match (event, generation_attempt_id, owner_message_id) {
        (InteractionEvent::BeforeGeneration | InteractionEvent::AfterGeneration, Some(_), None)
        | (InteractionEvent::MessageCommitted, None, Some(_))
        | (
            InteractionEvent::ConversationStarted
            | InteractionEvent::ConversationOpened
            | InteractionEvent::UserAction { .. }
            | InteractionEvent::VariableChanged { .. }
            | InteractionEvent::KnowledgeActivated { .. },
            None,
            None,
        ) => Ok(()),
        (InteractionEvent::BeforeGeneration | InteractionEvent::AfterGeneration, None, _) => Err(
            CoreError::invalid("generation lifecycle event requires its exact generation attempt"),
        ),
        (InteractionEvent::MessageCommitted, _, None) => Err(CoreError::invalid(
            "message-committed interaction event requires its exact owner message",
        )),
        (_, Some(_), _) => Err(CoreError::invalid(
            "non-generation lifecycle event cannot bind a generation attempt",
        )),
        (_, _, Some(_)) => Err(CoreError::invalid(
            "only message-committed interaction events bind an owner message",
        )),
    }
}

include!("orchestration_runtime/interaction/tests/knowledge_binding_revision.rs");
fn interaction_commit_artifacts(
    previous: &InteractionState,
    outcome: &InteractionOutcome,
    policy: &ResolvedInteractionPolicy,
    request: &InteractionReviewRequest,
    evaluation_seal: &InteractionEvaluationSeal,
    existing_knowledge: &[InteractionKnowledgeBinding],
) -> CoreResult<InteractionCommitArtifacts> {
    let (_, reconciled_knowledge) =
        reconcile_interaction_knowledge_state(previous.clone(), policy, existing_knowledge)?;
    let rule_sources = interaction_rule_sources(policy)?;
    let action_results =
        interaction_action_results(outcome, policy, &request.event, &rule_sources)?;
    let derived_events =
        interaction_derived_event_writes(outcome, policy, request, evaluation_seal, &rule_sources)?;
    let proposals = interaction_proposal_writes(previous, outcome, &rule_sources)?;
    Ok(InteractionCommitArtifacts {
        knowledge: interaction_knowledge_bindings(&outcome.state, policy, &reconciled_knowledge)?,
        action_results,
        derived_events,
        proposals,
    })
}

type InteractionRuleSource<'a> = (&'a InteractionRuleSetRevision, &'a InteractionRule);

fn interaction_rule_sources(
    policy: &ResolvedInteractionPolicy,
) -> CoreResult<BTreeMap<InteractionRuleId, InteractionRuleSource<'_>>> {
    let mut rule_sources = BTreeMap::new();
    for set in &policy.rule_sets {
        let revision = policy
            .rule_set_revisions
            .iter()
            .find(|revision| revision.rule_set_id == set.id)
            .ok_or_else(|| CoreError::internal("interaction rule set revision is missing"))?;
        for rule in &set.rules {
            if rule_sources
                .insert(rule.id.clone(), (revision, rule))
                .is_some()
            {
                return Err(CoreError::invalid(
                    "interaction rule IDs are ambiguous across approved sets",
                ));
            }
        }
    }
    Ok(rule_sources)
}

fn interaction_action_results(
    outcome: &InteractionOutcome,
    policy: &ResolvedInteractionPolicy,
    event: &InteractionEvent,
    rule_sources: &BTreeMap<InteractionRuleId, InteractionRuleSource<'_>>,
) -> CoreResult<Vec<InteractionActionResultWrite>> {
    let mut action_results = Vec::new();
    for trace in &outcome.trace {
        let Some((set_revision, rule)) = rule_sources.get(&trace.rule_id).copied() else {
            return Err(CoreError::internal(
                "interaction trace references an unknown rule",
            ));
        };
        if &rule.event != event || trace.status == InteractionRuleStatus::EventDidNotMatch {
            continue;
        }
        for (ordinal, action) in rule.actions.iter().enumerate() {
            let action_ordinal = u32::try_from(ordinal)
                .map_err(|_| CoreError::invalid("interaction action ordinal overflowed"))?;
            let asset_diagnostic = policy
                .asset_action_diagnostics
                .get(&(rule.id.as_str().to_owned(), action_ordinal));
            let status = if asset_diagnostic.is_some() {
                InteractionActionResultStatus::Failed
            } else {
                match trace.status {
                    InteractionRuleStatus::Applied
                        if matches!(action, InteractionAction::RequestUserApproval { .. }) =>
                    {
                        InteractionActionResultStatus::Proposed
                    }
                    InteractionRuleStatus::Applied => InteractionActionResultStatus::Applied,
                    InteractionRuleStatus::Failed | InteractionRuleStatus::ActionBudgetExceeded => {
                        InteractionActionResultStatus::Failed
                    }
                    InteractionRuleStatus::ConditionFalse
                    | InteractionRuleStatus::Disabled
                    | InteractionRuleStatus::PendingImportApproval
                    | InteractionRuleStatus::EventDidNotMatch => {
                        InteractionActionResultStatus::Skipped
                    }
                }
            };
            action_results.push(InteractionActionResultWrite {
                set_revision_id: set_revision.revision_id.clone(),
                rule_id: rule.id.clone(),
                action_ordinal,
                status,
                result: asset_diagnostic.cloned().unwrap_or_else(|| VersionedJson {
                    schema_version: 1,
                    value: serde_json::json!({
                        "rule_status": &trace.status,
                        "state_changed": trace.state_changed,
                        "effect_count": trace.effect_count,
                    }),
                }),
            });
        }
    }
    Ok(action_results)
}

fn interaction_derived_event_writes(
    outcome: &InteractionOutcome,
    policy: &ResolvedInteractionPolicy,
    request: &InteractionReviewRequest,
    evaluation_seal: &InteractionEvaluationSeal,
    rule_sources: &BTreeMap<InteractionRuleId, InteractionRuleSource<'_>>,
) -> CoreResult<Vec<InteractionDerivedEventWrite>> {
    let mut derived_events = Vec::with_capacity(outcome.derived_events.len());
    for derived in &outcome.derived_events {
        let Some((set_revision, rule)) = rule_sources.get(&derived.source_rule_id).copied() else {
            return Err(CoreError::internal(
                "derived interaction event references an unknown source rule",
            ));
        };
        if set_revision.rule_set_id != derived.source_rule_set_id {
            return Err(CoreError::internal(
                "derived interaction event references a mismatched source rule set",
            ));
        }
        let action_index = usize::try_from(derived.source_action_ordinal)
            .map_err(|_| CoreError::invalid("derived interaction action ordinal overflowed"))?;
        let action = rule.actions.get(action_index).ok_or_else(|| {
            CoreError::internal("derived interaction event source action disappeared")
        })?;
        let child_request = InteractionReviewRequest {
            conversation_id: request.conversation_id.clone(),
            branch_id: request.branch_id.clone(),
            expected_head: request.expected_head.clone(),
            event: derived.event.clone(),
        };
        let deterministic_seed = interaction_seed(
            &child_request,
            outcome.state.revision,
            &policy.rule_set_revisions,
            evaluation_seal.event_epoch_seconds,
        )?;
        derived_events.push(InteractionDerivedEventWrite {
            event: derived.event.clone(),
            source_set_revision_id: set_revision.revision_id.clone(),
            source_rule_id: derived.source_rule_id.clone(),
            source_action_ordinal: derived.source_action_ordinal,
            source_effect_ordinal: derived.source_effect_ordinal,
            source_action_sha256: interaction_action_sha256(action)?,
            deterministic_seed,
        });
    }
    Ok(derived_events)
}

fn interaction_proposal_writes(
    previous: &InteractionState,
    outcome: &InteractionOutcome,
    rule_sources: &BTreeMap<InteractionRuleId, InteractionRuleSource<'_>>,
) -> CoreResult<Vec<InteractionProposalWrite>> {
    let existing_ids = previous
        .proposals
        .iter()
        .map(|proposal| proposal.id.clone())
        .collect::<BTreeSet<_>>();
    let mut proposals = Vec::new();
    for record in outcome
        .state
        .proposals
        .iter()
        .filter(|record| !existing_ids.contains(&record.id))
    {
        if record.status != InteractionProposalStatus::Pending {
            return Err(CoreError::invalid(
                "new interaction proposal is not pending",
            ));
        }
        let Some((set_revision, rule)) = rule_sources.get(&record.rule_id).copied() else {
            return Err(CoreError::invalid(
                "new interaction proposal references an unknown rule",
            ));
        };
        if set_revision.rule_set_id != record.rule_set_id {
            return Err(CoreError::invalid(
                "new interaction proposal rule set identity is inconsistent",
            ));
        }
        let matching_actions = rule
            .actions
            .iter()
            .enumerate()
            .filter(|(_, action)| {
                matches!(
                    action,
                    InteractionAction::RequestUserApproval { proposal }
                        if proposal.id == record.proposal_id
                )
            })
            .map(|(ordinal, _)| ordinal)
            .collect::<Vec<_>>();
        let [action_ordinal] = matching_actions.as_slice() else {
            return Err(CoreError::invalid(
                "interaction proposal does not have one exact source action",
            ));
        };
        proposals.push(InteractionProposalWrite {
            record: record.clone(),
            rule_set_revision_id: set_revision.revision_id.clone(),
            action_ordinal: u32::try_from(*action_ordinal)
                .map_err(|_| CoreError::invalid("interaction proposal action overflowed"))?,
            review_payload_sha256: interaction_proposal_review_sha256(record)?,
        });
    }
    proposals.sort_by(|left, right| left.record.id.cmp(&right.record.id));
    Ok(proposals)
}

include!("orchestration_runtime/tests/memory_summary_instruction.rs");

fn versioned_digest<T: Serialize>(value: &T) -> CoreResult<String> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| CoreError::internal(format!("cannot hash runtime value: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn remap_generation_attempt_proposal_ids(
    generation_id: &GenerationId,
    state: &InteractionState,
    proposals: &[StoredGenerationAttemptProposal],
    to_domain: bool,
) -> CoreResult<InteractionState> {
    let mut storage_to_domain = BTreeMap::new();
    let mut domain_ids = BTreeSet::new();
    for proposal in proposals {
        let (storage_id, domain_id) =
            validate_generation_attempt_proposal_mapping(generation_id, proposal)?;
        if storage_to_domain
            .insert(storage_id, domain_id.clone())
            .is_some()
            || !domain_ids.insert(domain_id)
        {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation proposal identity mapping is not one-to-one",
                false,
            ));
        }
    }

    let source_to_target = if to_domain {
        storage_to_domain
    } else {
        storage_to_domain
            .into_iter()
            .map(|(storage_id, domain_id)| (domain_id, storage_id))
            .collect::<BTreeMap<_, _>>()
    };
    let mut source_counts = source_to_target
        .keys()
        .map(|id| (id.as_str(), 0_usize))
        .collect::<BTreeMap<_, _>>();
    let mut remapped = state.clone();
    for record in &mut remapped.proposals {
        if let Some(target) = source_to_target.get(record.id.as_str()) {
            let count = source_counts
                .get_mut(record.id.as_str())
                .ok_or_else(|| CoreError::internal("proposal identity count vanished"))?;
            *count = count.saturating_add(1);
            record.id = InteractionProposalRecordId::from(target.clone());
        } else if record.id.as_str().starts_with("attempt-proposal-") {
            return Err(CoreError::new(
                CoreErrorCode::StorageCorrupted,
                "generation aggregate contains an unbound attempt-owned proposal",
                false,
            ));
        }
    }
    if source_counts.values().any(|count| *count != 1) {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation proposal identity mapping is not total over its aggregate state",
            false,
        ));
    }
    remapped.validate().map_err(|error| {
        CoreError::new(
            CoreErrorCode::StorageCorrupted,
            format!("generation proposal remapping produced invalid state: {error}"),
            false,
        )
    })?;
    Ok(remapped)
}

fn remap_generation_attempt_derived_closure_existing_proposals(
    generation_id: &GenerationId,
    mut closure: GenerationAttemptDerivedClosure,
    proposals: &[StoredGenerationAttemptProposal],
) -> CoreResult<GenerationAttemptDerivedClosure> {
    for transition in &mut closure.transitions {
        transition.next_state = remap_generation_attempt_proposal_ids(
            generation_id,
            &transition.next_state,
            proposals,
            false,
        )?;
        transition.commit_sha256 =
            generation_attempt_derived_transition_commit_sha256(generation_id, transition)?;
    }
    closure.final_state = remap_generation_attempt_proposal_ids(
        generation_id,
        &closure.final_state,
        proposals,
        false,
    )?;
    closure.chain_sha256 = generation_attempt_derived_chain_sha256(&closure)?;
    generation_attempt_derived_closure_sha256(&closure)?;
    Ok(closure)
}

fn validate_generation_attempt_proposal_mapping(
    generation_id: &GenerationId,
    proposal: &StoredGenerationAttemptProposal,
) -> CoreResult<(String, String)> {
    if proposal.generation_id != *generation_id {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation proposal identity belongs to another attempt",
            false,
        ));
    }
    let mut reviewed_storage_record = proposal.record.clone();
    reviewed_storage_record.status = InteractionProposalStatus::Pending;
    reviewed_storage_record.decided_at_epoch_seconds = None;
    if reviewed_storage_record.id != proposal.record.id
        || interaction_proposal_review_sha256(&reviewed_storage_record)?
            != proposal.proposal_review_sha256.as_str()
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation proposal storage review fingerprint is invalid",
            false,
        ));
    }
    let mut domain_record = reviewed_storage_record;
    domain_record.id = proposal.domain_proposal_record_id.clone();
    if interaction_proposal_review_sha256(&domain_record)?
        != proposal.domain_proposal_review_sha256.as_str()
    {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation proposal domain review fingerprint is invalid",
            false,
        ));
    }
    let expected_storage_id =
        expected_generation_attempt_storage_proposal_id(generation_id, proposal)?;
    if proposal.record.id != expected_storage_id {
        return Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation proposal identity mapping is not one-to-one",
            false,
        ));
    }
    Ok((
        proposal.record.id.as_str().to_owned(),
        proposal.domain_proposal_record_id.as_str().to_owned(),
    ))
}

fn expected_generation_attempt_storage_proposal_id(
    generation_id: &GenerationId,
    proposal: &StoredGenerationAttemptProposal,
) -> CoreResult<InteractionProposalRecordId> {
    match proposal.storage_identity_version {
        1 => {
            if proposal.proposal_review_sha256 != proposal.domain_proposal_review_sha256 {
                return Err(CoreError::new(
                    CoreErrorCode::StorageCorrupted,
                    "legacy generation proposal review identity is invalid",
                    false,
                ));
            }
            Ok(proposal.domain_proposal_record_id.clone())
        }
        2 => Ok(InteractionProposalRecordId::from(format!(
            "attempt-proposal-{}",
            versioned_digest(&(
                "lorepia.generation-attempt-proposal-record.v1",
                generation_id,
                &proposal.domain_proposal_record_id,
                proposal.domain_proposal_review_sha256.as_str(),
                proposal.before_event_snapshot_sha256.as_str(),
            ))?
        ))),
        _ => Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "generation proposal storage identity version is invalid",
            false,
        )),
    }
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

const fn interaction_proposal_decision_requires_reviewable_text(
    decision: InteractionProposalDecision,
) -> bool {
    matches!(decision, InteractionProposalDecision::Approve)
}

const fn generation_proposal_decision_requires_reviewable_text(
    decision: GenerationAttemptProposalDecision,
) -> bool {
    matches!(decision, GenerationAttemptProposalDecision::Approve)
}

fn require_reviewable_interaction_proposal_text(
    proposal: &InteractionProposalRecord,
) -> CoreResult<()> {
    if lorepia_domain::validate_interaction_native_text("proposal_title", &proposal.title).is_err()
        || lorepia_domain::validate_interaction_native_text("proposal_body", &proposal.body)
            .is_err()
    {
        return Err(CoreError::new(
            CoreErrorCode::InvalidInput,
            "interaction proposal text is unavailable for approval",
            false,
        ));
    }
    Ok(())
}

include!("orchestration_runtime/tests/proposal_projection_authority.rs");

include!("orchestration_runtime/tests/generation_proposal_identity.rs");

include!("orchestration_runtime/tests/memory_cadence.rs");
