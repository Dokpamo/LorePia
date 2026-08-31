mod events;

pub(super) use events::{InteractionCommitArtifacts, interaction_commit_artifacts};

use lorepia_domain::{
    CoreError, CoreErrorCode, CoreResult, InteractionEvent, InteractionProposalStatus, Sha256Digest,
};
use lorepia_orchestration::{AppliedModuleRuntimePlan, ModuleMergeReview};
use lorepia_storage::{
    GenerationApprovalEvidence, GenerationAttemptStatus, GenerationBeforeEventEvidence,
    LifecycleOccurrenceKind, StoredGenerationAttempt, StoredGenerationAttemptProposal,
    StoredInteractionState, StoredLifecycleOccurrence, interaction_evaluation_seal_sha256,
};

use super::{
    EnqueueMemorySummaryRequest, InteractionReviewRequest, MemorySummaryHeadAuthority,
    ResolvedInteractionPolicy, ResolvedModuleRuntime, interaction_policy_snapshot,
    module_plan_error, validate_interaction_evaluation_seal,
};
use crate::{Core, app::generation_attempt_module_authority};

#[derive(Debug)]
pub(super) enum ProcessedCoreLifecycleOccurrence {
    Acknowledged {
        before_generation_evidence: Option<GenerationBeforeEventEvidence>,
        approval_evidence: Option<GenerationApprovalEvidence>,
    },
    AwaitingApproval {
        before_generation_evidence: Option<GenerationBeforeEventEvidence>,
    },
}
impl Core {
    pub(super) fn process_core_lifecycle_occurrence(
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
    pub(super) fn validate_generation_attempt_before_review_authority(
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
    pub(super) fn resolve_generation_attempt_proposal_policy(
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
