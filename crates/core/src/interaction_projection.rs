use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreErrorCode, CoreResult, InteractionEffect,
    InteractionProposalRecord, InteractionProposalStatus,
};
use serde::{Deserialize, Serialize};

use crate::Core;

/// Storage-independent proposal projection exposed by the Core facade.
///
/// Persistence-only policy, dispatch, and hashing metadata remain inside the
/// storage boundary. The two revision fields are the complete caller-visible
/// authority needed to decide the proposal safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalView {
    pub record: InteractionProposalRecord,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub state_revision: u64,
    pub proposal_revision: u64,
}

/// Durable lifecycle of one projected `ChoicesPresented` effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionChoiceEffectStatus {
    Pending,
    Consumed,
    Expired,
}

/// Stable cursor for paging immutable effect history in branch order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectHistoryCursor {
    pub resulting_state_revision: u64,
    pub sequence: u64,
}

/// Core-owned delivery claim for one pending UI effect.
///
/// The event identity, sequence, and delivery-attempt count form the opaque
/// acknowledgement authority retained by the Rust dispatcher. Storage policy,
/// state-row identity, and lease timestamps are intentionally excluded.
#[derive(Debug, Clone, PartialEq)]
pub struct InteractionEffectClaim {
    pub effect_id: String,
    pub event_id: String,
    pub sequence: u64,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub resulting_state_revision: u64,
    pub event_created_at: DateTime<Utc>,
    pub effect: InteractionEffect,
    pub delivery_attempts: u64,
}

/// Storage-independent immutable effect entry used by history projections.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectHistoryEntry {
    pub effect_id: String,
    pub sequence: u64,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub resulting_state_revision: u64,
    pub event_created_at: DateTime<Utc>,
    pub effect: InteractionEffect,
    pub choice_status: Option<InteractionChoiceEffectStatus>,
    pub selected_choice_id: Option<String>,
    pub choice_decided_at_epoch_seconds: Option<i64>,
}

/// Core-owned effect history plus reopen behavior.
///
/// `stored` retains the previous field shape for Core callers while its value
/// is now a purpose-built projection rather than a persistence row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectHistoryView {
    pub stored: InteractionEffectHistoryEntry,
    pub replay_on_reopen: bool,
}

/// Safe result of consuming one durable choice exactly once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionChoiceSelectionReceipt {
    pub choice_effect: InteractionEffectHistoryView,
    pub resulting_state_revision: u64,
}

impl Core {
    /// Lists a bounded durable proposal view for one exact room branch.
    ///
    /// Storage-derived state and proposal revisions are returned as the only
    /// valid decision CAS tokens; callers cannot supply an action payload.
    pub fn list_interaction_proposals(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        status: InteractionProposalStatus,
        limit: u32,
    ) -> CoreResult<Vec<InteractionProposalView>> {
        self.validate_runtime_branch_identity(conversation_id, branch_id)?;
        self.storage()
            .list_interaction_proposals(conversation_id, branch_id, status, limit)
            .map(project_interaction_proposals)
    }

    /// Returns only the current durable interaction-state revision for choice
    /// and proposal CAS. No variables, proposals, or internal state identity
    /// are exposed.
    pub fn get_interaction_state_revision(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<u64> {
        self.validate_runtime_branch_identity(conversation_id, branch_id)?;
        match self
            .storage()
            .get_interaction_state_snapshot(conversation_id, branch_id)
        {
            Ok(snapshot) => Ok(snapshot.state.revision),
            Err(error) if error.code == CoreErrorCode::NotFound => Ok(0),
            Err(error) => Err(error),
        }
    }

    /// Expires every due pending proposal in one atomic room-scoped transition.
    ///
    /// This is an explicit maintenance operation for room refresh. The
    /// timestamp and state CAS are Core-owned, no frontend event is accepted,
    /// and the storage transition never derives or dispatches a `UserAction`.
    /// Generation-attempt proposals use their separate aggregate CAS and are
    /// intentionally outside this operation.
    pub fn expire_due_interaction_proposals(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Vec<InteractionProposalView>> {
        self.validate_runtime_branch_identity(conversation_id, branch_id)?;
        let snapshot = match self
            .storage()
            .get_interaction_state_snapshot(conversation_id, branch_id)
        {
            Ok(snapshot) => snapshot,
            Err(error) if error.code == CoreErrorCode::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let now = Utc::now();
        self.storage()
            .expire_due_interaction_proposals(&lorepia_storage::InteractionProposalExpiryCommit {
                conversation_id: conversation_id.clone(),
                branch_id: branch_id.clone(),
                expected_state_revision: snapshot.state.revision,
                now_epoch_seconds: now.timestamp(),
                updated_at: now,
            })
            .map(|receipt| project_interaction_proposals(receipt.expired_proposals))
    }

    /// Pages immutable durable effects, including already delivered rows, so a
    /// UI can reconstruct history without reevaluating interaction rules.
    pub fn list_interaction_effect_history(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        after: Option<InteractionEffectHistoryCursor>,
        limit: u32,
    ) -> CoreResult<Vec<InteractionEffectHistoryView>> {
        self.prepare_interaction_projection_read(conversation_id, branch_id)?;
        let after = after.map(storage_interaction_effect_history_cursor);
        self.storage()
            .list_interaction_effect_history(conversation_id, branch_id, after, limit)
            .map(project_interaction_effect_histories)
    }

    /// Pages the durable reopen projection. One-shot audio is excluded by
    /// storage, while pending/consumed/expired choices retain their lifecycle.
    pub fn list_reopen_interaction_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        after: Option<InteractionEffectHistoryCursor>,
        limit: u32,
    ) -> CoreResult<Vec<InteractionEffectHistoryView>> {
        self.prepare_interaction_projection_read(conversation_id, branch_id)?;
        let after = after.map(storage_interaction_effect_history_cursor);
        self.storage()
            .list_reopen_interaction_effects(conversation_id, branch_id, after, limit)
            .map(project_interaction_effect_histories)
    }

    /// Returns the latest bounded reopen projection in chronological order.
    /// This reconstructs current region assets in long rooms without an
    /// unbounded scan from the oldest event.
    pub fn list_recent_reopen_interaction_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<InteractionEffectHistoryView>> {
        self.prepare_interaction_projection_read(conversation_id, branch_id)?;
        self.storage()
            .list_recent_reopen_interaction_effects(conversation_id, branch_id, limit)
            .map(project_interaction_effect_histories)
    }

    /// Pages older reopen-safe effects before an exclusive durable cursor.
    pub fn list_older_reopen_interaction_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        before: InteractionEffectHistoryCursor,
        limit: u32,
    ) -> CoreResult<Vec<InteractionEffectHistoryView>> {
        self.prepare_interaction_projection_read(conversation_id, branch_id)?;
        self.storage()
            .list_older_reopen_interaction_effects(
                conversation_id,
                branch_id,
                storage_interaction_effect_history_cursor(before),
                limit,
            )
            .map(project_interaction_effect_histories)
    }

    /// Returns the newest durable `AssetShown` effect for each UI region.
    ///
    /// This projection is independent of the bounded recent tail, so reopening
    /// a long room cannot lose a still-current background or portrait.
    pub fn get_interaction_region_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<Vec<InteractionEffectHistoryView>> {
        self.prepare_interaction_projection_read(conversation_id, branch_id)?;
        self.storage()
            .get_interaction_region_effects(conversation_id, branch_id)
            .map(project_interaction_effect_histories)
    }

    /// Returns a bounded list of still-actionable durable choice effects.
    ///
    /// Pending choices are projected separately from the recent tail so they
    /// remain available after a long-running conversation is reopened.
    pub fn list_pending_interaction_choice_effects(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        limit: u32,
    ) -> CoreResult<Vec<InteractionEffectHistoryView>> {
        self.prepare_interaction_projection_read(conversation_id, branch_id)?;
        self.storage()
            .list_pending_interaction_choice_effects(conversation_id, branch_id, limit)
            .map(project_interaction_effect_histories)
    }

    /// Builds the complete bounded branch-reopen projection from one storage
    /// read snapshot.
    ///
    /// The recent tail alone is insufficient for long rooms: the latest
    /// asset in a region or a still-pending choice may be older than that
    /// window. Exact duplicate rows are coalesced by durable effect identity.
    pub fn get_interaction_reopen_projection(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
        recent_limit: u32,
        pending_choice_limit: u32,
    ) -> CoreResult<Vec<InteractionEffectHistoryView>> {
        self.prepare_interaction_projection_read(conversation_id, branch_id)?;
        self.storage()
            .get_interaction_reopen_projection(
                conversation_id,
                branch_id,
                recent_limit,
                pending_choice_limit,
            )
            .map(project_interaction_effect_histories)
    }

    fn prepare_interaction_projection_read(
        &self,
        conversation_id: &ConversationId,
        branch_id: &ConversationBranchId,
    ) -> CoreResult<()> {
        self.validate_runtime_branch_identity(conversation_id, branch_id)?;
        self.drain_available_core_lifecycle_occurrences()?;
        self.validate_runtime_branch_identity(conversation_id, branch_id)?;
        Ok(())
    }

    /// Claims stored UI effects for a Rust-only dispatcher. Actions are never
    /// reevaluated during delivery.
    pub fn claim_interaction_effects(
        &self,
        limit: u32,
        lease_seconds: u32,
    ) -> CoreResult<Vec<InteractionEffectClaim>> {
        if !(1..=300).contains(&lease_seconds) {
            return Err(CoreError::invalid(
                "interaction effect lease must be between 1 and 300 seconds",
            ));
        }
        let now = Utc::now();
        self.storage()
            .claim_pending_interaction_effects(
                now,
                now + chrono::Duration::seconds(i64::from(lease_seconds)),
                limit,
            )
            .map(project_interaction_effect_claims)
    }

    pub fn acknowledge_interaction_effect(
        &self,
        event_id: &str,
        sequence: u64,
        expected_delivery_attempts: u64,
    ) -> CoreResult<()> {
        self.storage().mark_interaction_effect_delivered(
            event_id,
            sequence,
            expected_delivery_attempts,
            Utc::now(),
        )
    }

    pub fn retry_interaction_effect(
        &self,
        event_id: &str,
        sequence: u64,
        expected_delivery_attempts: u64,
        delay_seconds: u32,
    ) -> CoreResult<()> {
        if delay_seconds > 86_400 {
            return Err(CoreError::invalid(
                "interaction effect retry delay exceeds one day",
            ));
        }
        self.storage().retry_interaction_effect_after(
            event_id,
            sequence,
            expected_delivery_attempts,
            Utc::now() + chrono::Duration::seconds(i64::from(delay_seconds)),
        )
    }
}

pub(super) fn project_interaction_proposal(
    value: lorepia_storage::StoredInteractionProposal,
) -> InteractionProposalView {
    InteractionProposalView {
        record: value.record,
        conversation_id: value.conversation_id,
        branch_id: value.branch_id,
        state_revision: value.state_revision,
        proposal_revision: value.proposal_revision,
    }
}

pub(super) fn project_interaction_proposals(
    values: Vec<lorepia_storage::StoredInteractionProposal>,
) -> Vec<InteractionProposalView> {
    values
        .into_iter()
        .map(project_interaction_proposal)
        .collect()
}

pub(super) fn project_interaction_effect_claim(
    value: lorepia_storage::StoredInteractionEffect,
) -> InteractionEffectClaim {
    InteractionEffectClaim {
        effect_id: value.effect_id,
        event_id: value.event_id,
        sequence: value.sequence,
        conversation_id: value.conversation_id,
        branch_id: value.branch_id,
        resulting_state_revision: value.resulting_state_revision,
        event_created_at: value.event_created_at,
        effect: value.effect,
        delivery_attempts: value.delivery_attempts,
    }
}

pub(super) fn project_interaction_effect_claims(
    values: Vec<lorepia_storage::StoredInteractionEffect>,
) -> Vec<InteractionEffectClaim> {
    values
        .into_iter()
        .map(project_interaction_effect_claim)
        .collect()
}

pub(super) fn project_interaction_effect_history(
    value: lorepia_storage::StoredInteractionEffectHistory,
) -> InteractionEffectHistoryView {
    let stored = value.stored;
    InteractionEffectHistoryView {
        stored: InteractionEffectHistoryEntry {
            effect_id: stored.effect_id,
            sequence: stored.sequence,
            conversation_id: stored.conversation_id,
            branch_id: stored.branch_id,
            resulting_state_revision: stored.resulting_state_revision,
            event_created_at: stored.event_created_at,
            effect: stored.effect,
            choice_status: stored.choice_status.map(project_interaction_choice_status),
            selected_choice_id: stored.selected_choice_id,
            choice_decided_at_epoch_seconds: stored.choice_decided_at_epoch_seconds,
        },
        replay_on_reopen: value.replay_on_reopen,
    }
}

pub(super) fn project_interaction_effect_histories(
    values: Vec<lorepia_storage::StoredInteractionEffectHistory>,
) -> Vec<InteractionEffectHistoryView> {
    values
        .into_iter()
        .map(project_interaction_effect_history)
        .collect()
}

pub(super) fn project_interaction_choice_selection_receipt(
    value: lorepia_storage::InteractionChoiceSelectionReceipt,
) -> InteractionChoiceSelectionReceipt {
    InteractionChoiceSelectionReceipt {
        choice_effect: project_interaction_effect_history(value.choice_effect),
        resulting_state_revision: value.resulting_state_revision,
    }
}

pub(super) const fn storage_interaction_effect_history_cursor(
    value: InteractionEffectHistoryCursor,
) -> lorepia_storage::InteractionEffectHistoryCursor {
    lorepia_storage::InteractionEffectHistoryCursor {
        resulting_state_revision: value.resulting_state_revision,
        sequence: value.sequence,
    }
}

const fn project_interaction_choice_status(
    value: lorepia_storage::InteractionChoiceEffectStatus,
) -> InteractionChoiceEffectStatus {
    match value {
        lorepia_storage::InteractionChoiceEffectStatus::Pending => {
            InteractionChoiceEffectStatus::Pending
        }
        lorepia_storage::InteractionChoiceEffectStatus::Consumed => {
            InteractionChoiceEffectStatus::Consumed
        }
        lorepia_storage::InteractionChoiceEffectStatus::Expired => {
            InteractionChoiceEffectStatus::Expired
        }
    }
}
