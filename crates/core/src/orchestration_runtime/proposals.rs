use chrono::Utc;
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, InteractionEffect,
    InteractionEvent, InteractionProposalDecision, InteractionProposalRecord,
    InteractionProposalRecordId, InteractionProposalStatus, InteractionState,
};
use lorepia_orchestration::decide_pending;
use lorepia_storage::{
    InteractionDerivedEventCommit, InteractionKnowledgeBinding, InteractionProposalApprovalCommit,
    InteractionProposalRejectionCommit, StoredInteractionProposal,
};
use serde::{Deserialize, Serialize};

use super::{
    InteractionReviewRequest, interaction_commit_artifacts, interaction_error,
    interaction_policy_snapshot, versioned_digest,
};
use crate::Core;

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
impl Core {
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
}

pub(super) const fn interaction_proposal_decision_requires_reviewable_text(
    decision: InteractionProposalDecision,
) -> bool {
    matches!(decision, InteractionProposalDecision::Approve)
}
pub(super) fn require_reviewable_interaction_proposal_text(
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
