use std::collections::{BTreeMap, BTreeSet};

use chrono::Utc;
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, GenerationId,
    InteractionEvent, InteractionProposalDecision, InteractionProposalRecordId,
    InteractionProposalStatus, InteractionState, Sha256Digest, ValidateOrchestration,
};
use lorepia_orchestration::{decide_pending, expire_pending_proposal};
use lorepia_storage::{
    GenerationAttemptProposalDecision, GenerationAttemptProposalDecisionCommit,
    GenerationAttemptStatus, InteractionDerivedEventCommit, RetryableGenerationAttemptProjection,
    StoredGenerationAttemptProposal, interaction_proposal_review_sha256,
};
use serde::{Deserialize, Serialize};

use super::{
    InteractionReviewRequest, interaction_commit_artifacts, interaction_error,
    interaction_policy_snapshot, prepare_generation_attempt_derived_closure,
    proposals::require_reviewable_interaction_proposal_text,
    remap_generation_attempt_derived_closure_existing_proposals, versioned_digest,
};
use crate::Core;

const MAX_GENERATION_PROPOSAL_ROOM_PAGE: u32 = 100;
const MAX_GENERATION_PROPOSALS_PER_ATTEMPT: u32 = 1_024;
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
impl Core {
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
}

pub(super) fn remap_generation_attempt_proposal_ids(
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
pub(super) const fn generation_proposal_decision_requires_reviewable_text(
    decision: GenerationAttemptProposalDecision,
) -> bool {
    matches!(decision, GenerationAttemptProposalDecision::Approve)
}
