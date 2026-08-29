//! Safe projections for durable interaction proposals and UI-effect delivery.
//!
//! Native callers cannot inject lifecycle events, rule sets, variable writes,
//! or generic user actions through this module.

use chrono::{DateTime, Utc};
use lorepia_core::{
    AssetDeliveryDescriptor, ConversationBranchId, ConversationId,
    GenerationAttemptProposalDecisionReceipt, GenerationAttemptProposalDecisionRequest,
    GenerationAttemptProposalExpiryReceipt, GenerationAttemptProposalView, GenerationAttemptStatus,
    GenerationId, InteractionChoiceEffectStatus, InteractionChoiceSelectionReceipt,
    InteractionEffect, InteractionEffectClaim, InteractionEffectHistoryCursor,
    InteractionEffectHistoryView, InteractionProposalDecision, InteractionProposalDecisionRequest,
    InteractionProposalRecord, InteractionProposalRecordId, InteractionProposalStatus,
    InteractionProposalView, RetryableGenerationAttemptProjection, UiRegion,
};
use serde::{Deserialize, Serialize};

use crate::{AssetDeliveryDto, ShellApi, ShellError, ShellResult, api::validate_identifier};

const MAX_EFFECTS_PER_CLAIM: u32 = 32;
const EFFECT_LEASE_SECONDS: u32 = 30;
const MAX_EFFECT_CHOICES: usize = 64;
const MAX_EFFECT_ROLLS: usize = 100;
const MAX_HISTORY_PAGE_SIZE: u32 = 100;
const MAX_PROPOSAL_PAGE_SIZE: u32 = 100;
const MAX_GENERATION_ATTEMPT_PENDING_PROPOSALS: u32 = 1_024;
const MAX_SAFE_JAVASCRIPT_INTEGER: i64 = 9_007_199_254_740_991;
const REDACTED_PROPOSAL_TITLE: &str = "Stored proposal unavailable";
const REDACTED_PROPOSAL_BODY: &str = "The original proposal text cannot be displayed safely.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionProposalDecisionInput {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideInteractionProposalInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub proposal_record_id: String,
    pub expected_state_revision: u64,
    pub expected_proposal_revision: u64,
    pub decision: InteractionProposalDecisionInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionProposalStatusDto {
    Pending,
    Approved,
    Rejected,
    Expired,
}

/// Stable, content-free reason for withholding a legacy proposal's text.
/// Identity, status, and CAS evidence remain available so the record can be
/// rejected or expired without exposing incompatible native content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionProposalProjectionRejectionReasonDto {
    UnsafeNativeText,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalDto {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_rejection_reason: Option<InteractionProposalProjectionRejectionReasonDto>,
    pub status: InteractionProposalStatusDto,
    pub source_interaction_state_revision: u64,
    pub requested_at_epoch_seconds: i64,
    pub expires_at_epoch_seconds: Option<i64>,
    pub decided_at_epoch_seconds: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListInteractionProposalsInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub status: InteractionProposalStatusDto,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalListItemDto {
    pub conversation_id: String,
    pub branch_id: String,
    pub state_revision: u64,
    pub proposal_revision: u64,
    pub proposal: InteractionProposalDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpireInteractionProposalsInput {
    pub conversation_id: String,
    pub branch_id: String,
    /// Maximum number of terminal proposal projections returned to the UI.
    /// Core still expires every due proposal in the room atomically.
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalExpiryReceiptDto {
    pub conversation_id: String,
    pub branch_id: String,
    pub current_state_revision: u64,
    pub expired_proposals: Vec<InteractionProposalListItemDto>,
    pub has_more_expired: bool,
}

/// Source-room query for proposals isolated inside generation attempts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListGenerationAttemptProposalsInput {
    pub conversation_id: String,
    pub source_branch_id: String,
    pub status: InteractionProposalStatusDto,
    pub limit: u32,
}

/// Bounded source-room query for attempts whose durable authority is safe to
/// resume without exposing prompt, provider, operation, or nonce state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListRetryableGenerationAttemptsInput {
    pub conversation_id: String,
    pub source_branch_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryableGenerationAttemptStatusDto {
    BeforeGenerationApplied,
    DispatchReady,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryableGenerationAttemptDto {
    pub generation_id: String,
    pub status: RetryableGenerationAttemptStatusDto,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalDto {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_rejection_reason: Option<InteractionProposalProjectionRejectionReasonDto>,
    pub status: InteractionProposalStatusDto,
    pub source_interaction_state_revision: String,
    pub requested_at_epoch_seconds: i64,
    pub expires_at_epoch_seconds: Option<i64>,
    pub decided_at_epoch_seconds: Option<i64>,
}

/// Safe attempt-owned proposal projection.
///
/// Every `u64` CAS value crosses the JavaScript boundary as a canonical
/// decimal string so a webview cannot silently round it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalListItemDto {
    pub conversation_id: String,
    pub source_branch_id: String,
    pub proposed_branch_id: String,
    pub generation_id: String,
    pub aggregate_revision: String,
    pub interaction_state_revision: String,
    pub pending_proposal_count: u32,
    pub proposal_revision: String,
    pub proposal: GenerationAttemptProposalDto,
}

/// Exact decision request for one attempt-owned proposal.
///
/// The frontend cannot provide an action identifier, event, arguments, or a
/// timestamp. Core derives `UserAction(stored proposal id)` only after
/// validating both immutable authorities and both CAS revisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecideGenerationAttemptProposalInput {
    pub conversation_id: String,
    pub source_branch_id: String,
    pub generation_id: String,
    pub proposal_record_id: String,
    pub expected_aggregate_revision: String,
    pub expected_proposal_revision: String,
    pub decision: InteractionProposalDecisionInput,
}

/// Safe terminal result for one attempt-owned proposal decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalDecisionReceiptDto {
    pub conversation_id: String,
    pub source_branch_id: String,
    pub proposed_branch_id: String,
    pub generation_id: String,
    pub aggregate_revision: String,
    pub interaction_state_revision: String,
    pub pending_proposal_count: u32,
    pub proposal_revision: String,
    pub proposal: GenerationAttemptProposalDto,
    pub approval_evidence_sha256: Option<String>,
    pub exact_replay: bool,
}

/// Bounded trusted-clock expiry pass for one source room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpireGenerationAttemptProposalsInput {
    pub conversation_id: String,
    pub source_branch_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalExpiryReceiptDto {
    pub conversation_id: String,
    pub source_branch_id: String,
    pub decisions: Vec<GenerationAttemptProposalDecisionReceiptDto>,
    pub has_more_due: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionUiRegionDto {
    Message,
    Background,
    CharacterPortrait,
    StatusPanel,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionChoiceDto {
    pub id: String,
    pub label: String,
}

/// Stable, content-free reason for suppressing one legacy effect projection.
/// The original durable evidence remains inside Rust storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionEffectProjectionRejectionReasonDto {
    UnsafeNativeText,
    InvalidStoredEffect,
    AssetUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionEffectDto {
    StateChanged,
    KnowledgeActivated {
        entry_id: String,
    },
    ShowAsset {
        asset: AssetDeliveryDto,
        region: InteractionUiRegionDto,
    },
    PlayAudio {
        asset: AssetDeliveryDto,
    },
    PresentChoices {
        choices: Vec<InteractionChoiceDto>,
    },
    VisibleSystemEvent {
        text: String,
    },
    DiceRolled {
        count: u16,
        sides: u32,
        modifier: i64,
        rolls: Vec<u32>,
        total: i64,
    },
    ApprovalPending {
        title: String,
        body: String,
        expires_after_seconds: Option<u32>,
    },
    ProjectionRejected {
        reason: InteractionEffectProjectionRejectionReasonDto,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectDeliveryDto {
    pub effect_id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub resulting_state_revision: u64,
    pub event_created_at: String,
    pub effect: InteractionEffectDto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionChoiceStatusDto {
    Pending,
    Consumed,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectHistoryCursorDto {
    pub resulting_state_revision: u64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListInteractionEffectHistoryInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub after: Option<InteractionEffectHistoryCursorDto>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListRecentReopenInteractionEffectsInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectHistoryItemDto {
    pub effect_id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub resulting_state_revision: u64,
    pub sequence: u64,
    pub event_created_at: String,
    pub replay_on_reopen: bool,
    pub choice_status: Option<InteractionChoiceStatusDto>,
    pub selected_choice_id: Option<String>,
    pub choice_decided_at_epoch_seconds: Option<i64>,
    pub effect: InteractionEffectDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEffectHistoryPageDto {
    pub current_state_revision: u64,
    pub items: Vec<InteractionEffectHistoryItemDto>,
    pub next_cursor: Option<InteractionEffectHistoryCursorDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionReopenSnapshotDto {
    pub current_state_revision: u64,
    pub items: Vec<InteractionEffectHistoryItemDto>,
    /// Cursor immediately before the oldest item in this newest-first window.
    /// It is present only when the bounded window may have older history.
    pub older_cursor: Option<InteractionEffectHistoryCursorDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitInteractionChoiceInput {
    pub conversation_id: String,
    pub branch_id: String,
    pub effect_id: String,
    pub choice_id: String,
    pub expected_state_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionChoiceSelectionReceiptDto {
    pub choice_effect: InteractionEffectHistoryItemDto,
    pub resulting_state_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalDecisionReceiptDto {
    pub proposal: InteractionProposalDto,
    pub state_revision: u64,
    pub effects: Vec<InteractionEffectDto>,
}

/// Rust-only claim token and safe event projection.
///
/// The CAS fields are not serializable; AppState keeps them behind an opaque
/// per-delivery ticket and emits only `delivery`.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct ClaimedInteractionEffect {
    pub delivery: InteractionEffectDeliveryDto,
    pub event_id: String,
    pub sequence: u64,
    pub delivery_attempts: u64,
}

impl From<InteractionProposalDecisionInput> for InteractionProposalDecision {
    fn from(value: InteractionProposalDecisionInput) -> Self {
        match value {
            InteractionProposalDecisionInput::Approve => Self::Approve,
            InteractionProposalDecisionInput::Reject => Self::Reject,
        }
    }
}

impl From<InteractionProposalStatus> for InteractionProposalStatusDto {
    fn from(value: InteractionProposalStatus) -> Self {
        match value {
            InteractionProposalStatus::Pending => Self::Pending,
            InteractionProposalStatus::Approved => Self::Approved,
            InteractionProposalStatus::Rejected => Self::Rejected,
            InteractionProposalStatus::Expired => Self::Expired,
        }
    }
}

impl From<InteractionProposalStatusDto> for InteractionProposalStatus {
    fn from(value: InteractionProposalStatusDto) -> Self {
        match value {
            InteractionProposalStatusDto::Pending => Self::Pending,
            InteractionProposalStatusDto::Approved => Self::Approved,
            InteractionProposalStatusDto::Rejected => Self::Rejected,
            InteractionProposalStatusDto::Expired => Self::Expired,
        }
    }
}

impl From<InteractionChoiceEffectStatus> for InteractionChoiceStatusDto {
    fn from(value: InteractionChoiceEffectStatus) -> Self {
        match value {
            InteractionChoiceEffectStatus::Pending => Self::Pending,
            InteractionChoiceEffectStatus::Consumed => Self::Consumed,
            InteractionChoiceEffectStatus::Expired => Self::Expired,
        }
    }
}

impl From<InteractionEffectHistoryCursorDto> for InteractionEffectHistoryCursor {
    fn from(value: InteractionEffectHistoryCursorDto) -> Self {
        Self {
            resulting_state_revision: value.resulting_state_revision,
            sequence: value.sequence,
        }
    }
}

impl From<UiRegion> for InteractionUiRegionDto {
    fn from(value: UiRegion) -> Self {
        match value {
            UiRegion::Message => Self::Message,
            UiRegion::Background => Self::Background,
            UiRegion::CharacterPortrait => Self::CharacterPortrait,
            UiRegion::StatusPanel => Self::StatusPanel,
            UiRegion::Audio => Self::Audio,
        }
    }
}

impl ShellApi {
    pub fn expire_interaction_proposals(
        &self,
        input: ExpireInteractionProposalsInput,
    ) -> ShellResult<InteractionProposalExpiryReceiptDto> {
        validate_room_and_limit(
            &input.conversation_id,
            &input.branch_id,
            input.limit,
            MAX_PROPOSAL_PAGE_SIZE,
        )?;
        let conversation_id = ConversationId(input.conversation_id);
        let branch_id = ConversationBranchId(input.branch_id);
        let mut expired = self
            .core
            .expire_due_interaction_proposals(&conversation_id, &branch_id)
            .map_err(ShellError::from)?;
        let has_more_expired = expired.len() > input.limit as usize;
        expired.truncate(input.limit as usize);
        let current_state_revision = self
            .core
            .get_interaction_state_revision(&conversation_id, &branch_id)
            .map_err(ShellError::from)?;
        Ok(InteractionProposalExpiryReceiptDto {
            conversation_id: conversation_id.0,
            branch_id: branch_id.0,
            current_state_revision,
            expired_proposals: expired
                .into_iter()
                .map(project_interaction_proposal_view)
                .collect::<ShellResult<Vec<_>>>()?,
            has_more_expired,
        })
    }

    pub fn list_interaction_proposals(
        &self,
        input: ListInteractionProposalsInput,
    ) -> ShellResult<Vec<InteractionProposalListItemDto>> {
        validate_room_and_limit(
            &input.conversation_id,
            &input.branch_id,
            input.limit,
            MAX_PROPOSAL_PAGE_SIZE,
        )?;
        self.core
            .list_interaction_proposals(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.branch_id),
                input.status.into(),
                input.limit,
            )
            .map_err(ShellError::from)?
            .into_iter()
            .map(project_interaction_proposal_view)
            .collect()
    }

    pub fn list_generation_attempt_proposals(
        &self,
        input: ListGenerationAttemptProposalsInput,
    ) -> ShellResult<Vec<GenerationAttemptProposalListItemDto>> {
        validate_room_and_limit(
            &input.conversation_id,
            &input.source_branch_id,
            input.limit,
            MAX_PROPOSAL_PAGE_SIZE,
        )?;
        self.core
            .list_generation_attempt_proposals_for_source_room(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.source_branch_id),
                input.status.into(),
                input.limit,
            )
            .map_err(ShellError::from)?
            .into_iter()
            .map(project_generation_attempt_proposal)
            .collect()
    }

    pub fn list_retryable_generation_attempts(
        &self,
        input: ListRetryableGenerationAttemptsInput,
    ) -> ShellResult<Vec<RetryableGenerationAttemptDto>> {
        validate_room_and_limit(
            &input.conversation_id,
            &input.source_branch_id,
            input.limit,
            MAX_PROPOSAL_PAGE_SIZE,
        )?;
        self.core
            .list_retryable_generation_attempts_for_source_room(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.source_branch_id),
                input.limit,
            )
            .map_err(ShellError::from)?
            .into_iter()
            .map(project_retryable_generation_attempt)
            .collect()
    }

    pub fn expire_generation_attempt_proposals(
        &self,
        input: ExpireGenerationAttemptProposalsInput,
    ) -> ShellResult<GenerationAttemptProposalExpiryReceiptDto> {
        validate_room_and_limit(
            &input.conversation_id,
            &input.source_branch_id,
            input.limit,
            MAX_PROPOSAL_PAGE_SIZE,
        )?;
        let conversation_id = ConversationId(input.conversation_id);
        let source_branch_id = ConversationBranchId(input.source_branch_id);
        let receipt = self
            .core
            .expire_due_generation_attempt_proposals_for_source_room(
                &conversation_id,
                &source_branch_id,
                input.limit,
            )
            .map_err(ShellError::from)?;
        project_generation_attempt_expiry(receipt, conversation_id, source_branch_id)
    }

    pub fn list_interaction_effect_history(
        &self,
        input: ListInteractionEffectHistoryInput,
    ) -> ShellResult<InteractionEffectHistoryPageDto> {
        self.list_projected_interaction_effect_history(input)
    }

    pub fn list_reopen_interaction_effects(
        &self,
        input: ListRecentReopenInteractionEffectsInput,
    ) -> ShellResult<InteractionReopenSnapshotDto> {
        validate_room_and_limit(
            &input.conversation_id,
            &input.branch_id,
            input.limit,
            MAX_HISTORY_PAGE_SIZE,
        )?;
        let conversation_id = ConversationId(input.conversation_id);
        let branch_id = ConversationBranchId(input.branch_id);
        let current_state_revision = self
            .core
            .get_interaction_state_revision(&conversation_id, &branch_id)
            .map_err(ShellError::from)?;
        let recent = self
            .core
            .list_recent_reopen_interaction_effects(&conversation_id, &branch_id, input.limit)
            .map_err(ShellError::from)?;
        let older_cursor = (recent.len() == input.limit as usize)
            .then(|| recent.first())
            .flatten()
            .map(|item| InteractionEffectHistoryCursorDto {
                resulting_state_revision: item.stored.resulting_state_revision,
                sequence: item.stored.sequence,
            });
        let items = self
            .core
            .get_interaction_reopen_projection(
                &conversation_id,
                &branch_id,
                input.limit,
                input.limit,
            )
            .map_err(ShellError::from)?
            .into_iter()
            .map(|item| self.project_interaction_effect_history(item))
            .collect::<ShellResult<Vec<_>>>()?;
        Ok(InteractionReopenSnapshotDto {
            current_state_revision,
            items,
            older_cursor,
        })
    }

    pub fn submit_interaction_choice(
        &self,
        input: SubmitInteractionChoiceInput,
    ) -> ShellResult<InteractionChoiceSelectionReceiptDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_identifier("effect_id", &input.effect_id)?;
        validate_identifier("choice_id", &input.choice_id)?;
        let receipt = self
            .core
            .submit_interaction_choice(
                &ConversationId(input.conversation_id),
                &ConversationBranchId(input.branch_id),
                &input.effect_id,
                &input.choice_id,
                input.expected_state_revision,
            )
            .map_err(ShellError::from)?;
        self.project_choice_selection_receipt(receipt)
    }

    pub fn decide_interaction_proposal(
        &self,
        input: DecideInteractionProposalInput,
    ) -> ShellResult<InteractionProposalDecisionReceiptDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("branch_id", &input.branch_id)?;
        validate_identifier("proposal_record_id", &input.proposal_record_id)?;
        let receipt = self
            .core
            .decide_interaction_proposal(&InteractionProposalDecisionRequest {
                conversation_id: lorepia_core::ConversationId(input.conversation_id),
                branch_id: lorepia_core::ConversationBranchId(input.branch_id),
                proposal_record_id: InteractionProposalRecordId::from(input.proposal_record_id),
                expected_state_revision: input.expected_state_revision,
                expected_proposal_revision: input.expected_proposal_revision,
                decision: input.decision.into(),
            })
            .map_err(ShellError::from)?;
        if receipt.effects.len() > MAX_EFFECTS_PER_CLAIM as usize {
            return Err(ShellError::from(lorepia_core::CoreError::invalid(
                "interaction decision returned too many effects",
            )));
        }
        Ok(InteractionProposalDecisionReceiptDto {
            proposal: project_proposal(receipt.proposal)?,
            state_revision: receipt.state_revision,
            effects: receipt
                .effects
                .into_iter()
                .map(|effect| self.project_interaction_effect(effect))
                .collect(),
        })
    }

    pub fn decide_generation_attempt_proposal(
        &self,
        input: DecideGenerationAttemptProposalInput,
    ) -> ShellResult<GenerationAttemptProposalDecisionReceiptDto> {
        validate_identifier("conversation_id", &input.conversation_id)?;
        validate_identifier("source_branch_id", &input.source_branch_id)?;
        validate_identifier("generation_id", &input.generation_id)?;
        validate_identifier("proposal_record_id", &input.proposal_record_id)?;
        let expected_aggregate_revision = parse_positive_decimal_revision(
            "expected_aggregate_revision",
            &input.expected_aggregate_revision,
        )?;
        let expected_proposal_revision = parse_positive_decimal_revision(
            "expected_proposal_revision",
            &input.expected_proposal_revision,
        )?;
        let conversation_id = ConversationId(input.conversation_id);
        let source_branch_id = ConversationBranchId(input.source_branch_id);
        let receipt = self
            .core
            .decide_generation_attempt_proposal(&GenerationAttemptProposalDecisionRequest {
                conversation_id: conversation_id.clone(),
                source_branch_id: source_branch_id.clone(),
                generation_id: GenerationId(input.generation_id),
                proposal_record_id: InteractionProposalRecordId::from(input.proposal_record_id),
                expected_aggregate_revision,
                expected_proposal_revision,
                decision: input.decision.into(),
            })
            .map_err(ShellError::from)?;
        project_generation_attempt_decision(receipt, &conversation_id, &source_branch_id)
    }

    /// Claims a bounded lease for the Rust-only Tauri dispatcher.
    #[doc(hidden)]
    pub fn claim_interaction_effects(&self) -> ShellResult<Vec<ClaimedInteractionEffect>> {
        self.core
            .claim_interaction_effects(MAX_EFFECTS_PER_CLAIM, EFFECT_LEASE_SECONDS)
            .map_err(ShellError::from)?
            .into_iter()
            .map(|stored| self.project_claimed_interaction_effect(stored))
            .collect()
    }

    #[doc(hidden)]
    pub fn acknowledge_interaction_effect(
        &self,
        event_id: &str,
        sequence: u64,
        delivery_attempts: u64,
    ) -> ShellResult<()> {
        validate_identifier("event_id", event_id)?;
        self.core
            .acknowledge_interaction_effect(event_id, sequence, delivery_attempts)
            .map_err(ShellError::from)
    }

    #[doc(hidden)]
    pub fn retry_interaction_effect(
        &self,
        event_id: &str,
        sequence: u64,
        delivery_attempts: u64,
    ) -> ShellResult<()> {
        validate_identifier("event_id", event_id)?;
        self.core
            .retry_interaction_effect(event_id, sequence, delivery_attempts, 1)
            .map_err(ShellError::from)
    }

    fn project_claimed_interaction_effect(
        &self,
        stored: InteractionEffectClaim,
    ) -> ShellResult<ClaimedInteractionEffect> {
        validate_identifier("effect_id", &stored.effect_id)?;
        validate_identifier("event_id", &stored.event_id)?;
        Ok(ClaimedInteractionEffect {
            delivery: InteractionEffectDeliveryDto {
                effect_id: stored.effect_id,
                conversation_id: stored.conversation_id.0,
                branch_id: stored.branch_id.0,
                resulting_state_revision: stored.resulting_state_revision,
                event_created_at: stored.event_created_at.to_rfc3339(),
                effect: self.project_interaction_effect(stored.effect),
            },
            event_id: stored.event_id,
            sequence: stored.sequence,
            delivery_attempts: stored.delivery_attempts,
        })
    }

    fn list_projected_interaction_effect_history(
        &self,
        input: ListInteractionEffectHistoryInput,
    ) -> ShellResult<InteractionEffectHistoryPageDto> {
        validate_room_and_limit(
            &input.conversation_id,
            &input.branch_id,
            input.limit,
            MAX_HISTORY_PAGE_SIZE,
        )?;
        let conversation_id = ConversationId(input.conversation_id);
        let branch_id = ConversationBranchId(input.branch_id);
        let current_state_revision = self
            .core
            .get_interaction_state_revision(&conversation_id, &branch_id)
            .map_err(ShellError::from)?;
        let query_limit = input.limit.saturating_add(1);
        let after = input.after.map(Into::into);
        let mut stored = self
            .core
            .list_interaction_effect_history(&conversation_id, &branch_id, after, query_limit)
            .map_err(ShellError::from)?;
        let has_more = stored.len() > input.limit as usize;
        if has_more {
            stored.truncate(input.limit as usize);
        }
        let next_cursor = has_more.then(|| stored.last()).flatten().map(|item| {
            InteractionEffectHistoryCursorDto {
                resulting_state_revision: item.stored.resulting_state_revision,
                sequence: item.stored.sequence,
            }
        });
        let items = stored
            .into_iter()
            .map(|item| self.project_interaction_effect_history(item))
            .collect::<ShellResult<Vec<_>>>()?;
        Ok(InteractionEffectHistoryPageDto {
            current_state_revision,
            items,
            next_cursor,
        })
    }

    fn project_choice_selection_receipt(
        &self,
        receipt: InteractionChoiceSelectionReceipt,
    ) -> ShellResult<InteractionChoiceSelectionReceiptDto> {
        Ok(InteractionChoiceSelectionReceiptDto {
            choice_effect: self.project_interaction_effect_history(receipt.choice_effect)?,
            resulting_state_revision: receipt.resulting_state_revision,
        })
    }

    fn project_interaction_effect_history(
        &self,
        value: InteractionEffectHistoryView,
    ) -> ShellResult<InteractionEffectHistoryItemDto> {
        let stored = value.stored;
        validate_identifier("effect_id", &stored.effect_id)?;
        validate_identifier("conversation_id", stored.conversation_id.0.as_str())?;
        validate_identifier("branch_id", stored.branch_id.0.as_str())?;
        if let Some(choice_id) = stored.selected_choice_id.as_deref() {
            validate_identifier("selected_choice_id", choice_id)?;
        }
        Ok(InteractionEffectHistoryItemDto {
            effect_id: stored.effect_id,
            conversation_id: stored.conversation_id.0,
            branch_id: stored.branch_id.0,
            resulting_state_revision: stored.resulting_state_revision,
            sequence: stored.sequence,
            event_created_at: stored.event_created_at.to_rfc3339(),
            replay_on_reopen: value.replay_on_reopen,
            choice_status: stored.choice_status.map(Into::into),
            selected_choice_id: stored.selected_choice_id,
            choice_decided_at_epoch_seconds: stored.choice_decided_at_epoch_seconds,
            effect: self.project_interaction_effect(stored.effect),
        })
    }

    fn project_interaction_effect(&self, effect: InteractionEffect) -> InteractionEffectDto {
        self.try_project_interaction_effect(effect)
            .unwrap_or_else(|reason| InteractionEffectDto::ProjectionRejected { reason })
    }

    fn try_project_interaction_effect(
        &self,
        effect: InteractionEffect,
    ) -> Result<InteractionEffectDto, InteractionEffectProjectionRejectionReasonDto> {
        use InteractionEffectProjectionRejectionReasonDto::{
            AssetUnavailable, InvalidStoredEffect, UnsafeNativeText,
        };

        match effect {
            InteractionEffect::VariableSet { .. } => Ok(InteractionEffectDto::StateChanged),
            InteractionEffect::KnowledgeActivated { entry_id } => {
                validate_identifier("knowledge_entry_id", entry_id.as_str())
                    .map_err(|_| InvalidStoredEffect)?;
                Ok(InteractionEffectDto::KnowledgeActivated {
                    entry_id: entry_id.0,
                })
            }
            InteractionEffect::AssetShown { asset_id, region } => {
                validate_identifier("asset_id", asset_id.as_str())
                    .map_err(|_| InvalidStoredEffect)?;
                let asset = self
                    .core
                    .resolve_asset_delivery_by_id(&asset_id)
                    .map(AssetDeliveryDto::from)
                    .map_err(|_| AssetUnavailable)?;
                Ok(InteractionEffectDto::ShowAsset {
                    asset,
                    region: region.into(),
                })
            }
            InteractionEffect::AudioRequested { asset_id } => {
                validate_identifier("asset_id", asset_id.as_str())
                    .map_err(|_| InvalidStoredEffect)?;
                let descriptor: AssetDeliveryDescriptor = self
                    .core
                    .resolve_asset_delivery_by_id(&asset_id)
                    .map_err(|_| AssetUnavailable)?;
                Ok(InteractionEffectDto::PlayAudio {
                    asset: descriptor.into(),
                })
            }
            InteractionEffect::ChoicesPresented { choices } => {
                if choices.is_empty() || choices.len() > MAX_EFFECT_CHOICES {
                    return Err(InvalidStoredEffect);
                }
                let choices = choices
                    .into_iter()
                    .map(|choice| {
                        validate_identifier("choice_id", &choice.id)
                            .map_err(|_| InvalidStoredEffect)?;
                        validate_effect_text_for_projection(&choice.label)
                            .map_err(|_| UnsafeNativeText)?;
                        Ok(InteractionChoiceDto {
                            id: choice.id,
                            label: choice.label,
                        })
                    })
                    .collect::<Result<Vec<_>, InteractionEffectProjectionRejectionReasonDto>>()?;
                Ok(InteractionEffectDto::PresentChoices { choices })
            }
            InteractionEffect::VisibleSystemEvent { text } => {
                validate_effect_text_for_projection(&text).map_err(|_| UnsafeNativeText)?;
                Ok(InteractionEffectDto::VisibleSystemEvent { text })
            }
            InteractionEffect::DiceRolled {
                expression,
                rolls,
                total,
                ..
            } => {
                let expected_total = rolls.iter().try_fold(expression.modifier, |sum, roll| {
                    sum.checked_add(i64::from(*roll))
                });
                if expression.count == 0
                    || expression.sides < 2
                    || rolls.is_empty()
                    || rolls.len() > MAX_EFFECT_ROLLS
                    || rolls.len() != usize::from(expression.count)
                    || rolls
                        .iter()
                        .any(|roll| *roll == 0 || *roll > expression.sides)
                    || !(-MAX_SAFE_JAVASCRIPT_INTEGER..=MAX_SAFE_JAVASCRIPT_INTEGER)
                        .contains(&expression.modifier)
                    || !(-MAX_SAFE_JAVASCRIPT_INTEGER..=MAX_SAFE_JAVASCRIPT_INTEGER)
                        .contains(&total)
                    || expected_total != Some(total)
                {
                    return Err(InvalidStoredEffect);
                }
                Ok(InteractionEffectDto::DiceRolled {
                    count: expression.count,
                    sides: expression.sides,
                    modifier: expression.modifier,
                    rolls,
                    total,
                })
            }
            InteractionEffect::ApprovalRequested {
                title,
                body,
                expires_after_seconds,
                ..
            } => project_approval_effect(title, body, expires_after_seconds),
        }
    }
}

fn project_approval_effect(
    title: String,
    body: String,
    expires_after_seconds: Option<u32>,
) -> Result<InteractionEffectDto, InteractionEffectProjectionRejectionReasonDto> {
    use InteractionEffectProjectionRejectionReasonDto::{InvalidStoredEffect, UnsafeNativeText};

    if title.is_empty()
        || title.chars().count() > lorepia_core::MAX_INTERACTION_PROPOSAL_TITLE_CHARS
        || body.is_empty()
        || expires_after_seconds == Some(0)
    {
        return Err(InvalidStoredEffect);
    }
    validate_effect_text_for_projection(&title).map_err(|_| UnsafeNativeText)?;
    validate_effect_text_for_projection(&body).map_err(|_| UnsafeNativeText)?;
    Ok(InteractionEffectDto::ApprovalPending {
        title,
        body,
        expires_after_seconds,
    })
}

fn project_proposal(value: InteractionProposalRecord) -> ShellResult<InteractionProposalDto> {
    validate_identifier("proposal_record_id", value.id.as_str())?;
    let (title, body, projection_rejection_reason) = project_proposal_text(value.title, value.body);
    Ok(InteractionProposalDto {
        id: value.id.0,
        title,
        body,
        projection_rejection_reason,
        status: value.status.into(),
        source_interaction_state_revision: value.source_interaction_state_revision,
        requested_at_epoch_seconds: value.requested_at_epoch_seconds,
        expires_at_epoch_seconds: value.expires_at_epoch_seconds,
        decided_at_epoch_seconds: value.decided_at_epoch_seconds,
    })
}

fn project_interaction_proposal_view(
    value: InteractionProposalView,
) -> ShellResult<InteractionProposalListItemDto> {
    validate_identifier("conversation_id", value.conversation_id.0.as_str())?;
    validate_identifier("branch_id", value.branch_id.0.as_str())?;
    Ok(InteractionProposalListItemDto {
        conversation_id: value.conversation_id.0,
        branch_id: value.branch_id.0,
        state_revision: value.state_revision,
        proposal_revision: value.proposal_revision,
        proposal: project_proposal(value.record)?,
    })
}

fn project_generation_attempt_proposal(
    value: GenerationAttemptProposalView,
) -> ShellResult<GenerationAttemptProposalListItemDto> {
    validate_generation_attempt_projection(GenerationAttemptProjection {
        conversation_id: &value.proposal.conversation_id,
        source_branch_id: &value.proposal.source_branch_id,
        proposed_branch_id: &value.proposal.proposed_branch_id,
        generation_id: &value.proposal.generation_id,
        aggregate_revision: value.aggregate_revision,
        interaction_state_revision: value.interaction_state_revision,
        pending_proposal_count: value.pending_proposal_count,
        proposal_revision: value.proposal.proposal_revision,
    })?;
    Ok(GenerationAttemptProposalListItemDto {
        conversation_id: value.proposal.conversation_id.0,
        source_branch_id: value.proposal.source_branch_id.0,
        proposed_branch_id: value.proposal.proposed_branch_id.0,
        generation_id: value.proposal.generation_id.0,
        aggregate_revision: value.aggregate_revision.to_string(),
        interaction_state_revision: value.interaction_state_revision.to_string(),
        pending_proposal_count: value.pending_proposal_count,
        proposal_revision: value.proposal.proposal_revision.to_string(),
        proposal: project_generation_attempt_proposal_record(value.proposal.record)?,
    })
}

fn project_retryable_generation_attempt(
    value: RetryableGenerationAttemptProjection,
) -> ShellResult<RetryableGenerationAttemptDto> {
    validate_identifier("generation_attempt_id", value.generation_id.0.as_str())?;
    if value.updated_at < value.created_at {
        return Err(generation_projection_error(
            "retryable generation attempt has inverted timestamps",
        ));
    }
    let status = match value.status {
        GenerationAttemptStatus::BeforeGenerationApplied => {
            RetryableGenerationAttemptStatusDto::BeforeGenerationApplied
        }
        GenerationAttemptStatus::DispatchReady => {
            RetryableGenerationAttemptStatusDto::DispatchReady
        }
        _ => {
            return Err(generation_projection_error(
                "retryable generation attempt has a non-retryable status",
            ));
        }
    };
    Ok(RetryableGenerationAttemptDto {
        generation_id: value.generation_id.0,
        status,
        created_at: value.created_at,
        updated_at: value.updated_at,
    })
}

fn project_generation_attempt_decision(
    value: GenerationAttemptProposalDecisionReceipt,
    expected_conversation_id: &ConversationId,
    expected_source_branch_id: &ConversationBranchId,
) -> ShellResult<GenerationAttemptProposalDecisionReceiptDto> {
    if value.proposal.conversation_id != *expected_conversation_id
        || value.proposal.source_branch_id != *expected_source_branch_id
    {
        return Err(generation_projection_error(
            "generation proposal decision returned mismatched source-room authority",
        ));
    }
    validate_generation_attempt_projection(GenerationAttemptProjection {
        conversation_id: &value.proposal.conversation_id,
        source_branch_id: &value.proposal.source_branch_id,
        proposed_branch_id: &value.proposal.proposed_branch_id,
        generation_id: &value.proposal.generation_id,
        aggregate_revision: value.aggregate_revision,
        interaction_state_revision: value.interaction_state_revision,
        pending_proposal_count: value.pending_proposal_count,
        proposal_revision: value.proposal.proposal_revision,
    })?;
    let approval_evidence_sha256 = value
        .approval_evidence_sha256
        .map(lorepia_core::Sha256Digest::into_inner);
    validate_generation_attempt_decision_evidence(
        value.pending_proposal_count,
        approval_evidence_sha256.as_deref(),
    )?;
    Ok(GenerationAttemptProposalDecisionReceiptDto {
        conversation_id: value.proposal.conversation_id.0,
        source_branch_id: value.proposal.source_branch_id.0,
        proposed_branch_id: value.proposal.proposed_branch_id.0,
        generation_id: value.proposal.generation_id.0,
        aggregate_revision: value.aggregate_revision.to_string(),
        interaction_state_revision: value.interaction_state_revision.to_string(),
        pending_proposal_count: value.pending_proposal_count,
        proposal_revision: value.proposal.proposal_revision.to_string(),
        proposal: project_generation_attempt_proposal_record(value.proposal.record)?,
        approval_evidence_sha256,
        exact_replay: value.exact_replay,
    })
}

fn project_generation_attempt_expiry(
    value: GenerationAttemptProposalExpiryReceipt,
    conversation_id: ConversationId,
    source_branch_id: ConversationBranchId,
) -> ShellResult<GenerationAttemptProposalExpiryReceiptDto> {
    if value.decisions.len() > MAX_PROPOSAL_PAGE_SIZE as usize {
        return Err(generation_projection_error(
            "generation proposal expiry returned too many decisions",
        ));
    }
    let decisions = value
        .decisions
        .into_iter()
        .map(|decision| {
            project_generation_attempt_decision(decision, &conversation_id, &source_branch_id)
        })
        .collect::<ShellResult<Vec<_>>>()?;
    Ok(GenerationAttemptProposalExpiryReceiptDto {
        conversation_id: conversation_id.0,
        source_branch_id: source_branch_id.0,
        decisions,
        has_more_due: value.has_more_due,
    })
}

fn project_generation_attempt_proposal_record(
    value: InteractionProposalRecord,
) -> ShellResult<GenerationAttemptProposalDto> {
    validate_identifier("proposal_record_id", value.id.as_str())?;
    validate_generation_attempt_proposal_timestamps(&value)?;
    let (title, body, projection_rejection_reason) = project_proposal_text(value.title, value.body);
    Ok(GenerationAttemptProposalDto {
        id: value.id.0,
        title,
        body,
        projection_rejection_reason,
        status: value.status.into(),
        source_interaction_state_revision: value.source_interaction_state_revision.to_string(),
        requested_at_epoch_seconds: value.requested_at_epoch_seconds,
        expires_at_epoch_seconds: value.expires_at_epoch_seconds,
        decided_at_epoch_seconds: value.decided_at_epoch_seconds,
    })
}

fn validate_generation_attempt_proposal_timestamps(
    value: &InteractionProposalRecord,
) -> ShellResult<()> {
    let timestamps = [
        Some(value.requested_at_epoch_seconds),
        value.expires_at_epoch_seconds,
        value.decided_at_epoch_seconds,
    ];
    if timestamps
        .into_iter()
        .flatten()
        .any(|timestamp| !(0..=MAX_SAFE_JAVASCRIPT_INTEGER).contains(&timestamp))
        || value
            .expires_at_epoch_seconds
            .is_some_and(|expires_at| expires_at < value.requested_at_epoch_seconds)
        || value
            .decided_at_epoch_seconds
            .is_some_and(|decided_at| decided_at < value.requested_at_epoch_seconds)
        || (value.status == InteractionProposalStatus::Pending
            && value.decided_at_epoch_seconds.is_some())
        || (value.status != InteractionProposalStatus::Pending
            && value.decided_at_epoch_seconds.is_none())
        || (value.status == InteractionProposalStatus::Expired
            && (value.expires_at_epoch_seconds.is_none()
                || value.decided_at_epoch_seconds < value.expires_at_epoch_seconds))
    {
        return Err(generation_projection_error(
            "generation proposal timestamps are not safe for IPC",
        ));
    }
    Ok(())
}

struct GenerationAttemptProjection<'a> {
    conversation_id: &'a ConversationId,
    source_branch_id: &'a ConversationBranchId,
    proposed_branch_id: &'a ConversationBranchId,
    generation_id: &'a GenerationId,
    aggregate_revision: u64,
    interaction_state_revision: u64,
    pending_proposal_count: u32,
    proposal_revision: u64,
}

fn validate_generation_attempt_projection(
    projection: GenerationAttemptProjection<'_>,
) -> ShellResult<()> {
    validate_identifier("conversation_id", &projection.conversation_id.0)?;
    validate_identifier("source_branch_id", &projection.source_branch_id.0)?;
    validate_identifier("proposed_branch_id", &projection.proposed_branch_id.0)?;
    validate_identifier("generation_id", &projection.generation_id.0)?;
    if projection.aggregate_revision == 0
        || projection.interaction_state_revision == 0
        || projection.proposal_revision == 0
    {
        return Err(generation_projection_error(
            "generation proposal returned a non-positive IPC revision",
        ));
    }
    if projection.pending_proposal_count > MAX_GENERATION_ATTEMPT_PENDING_PROPOSALS {
        return Err(generation_projection_error(
            "generation proposal aggregate exceeds the supported pending bound",
        ));
    }
    Ok(())
}

fn validate_generation_attempt_decision_evidence(
    pending_proposal_count: u32,
    approval_evidence_sha256: Option<&str>,
) -> ShellResult<()> {
    match (pending_proposal_count, approval_evidence_sha256) {
        (0, Some(digest)) => validate_lowercase_sha256("approval_evidence_sha256", digest),
        (0, None) => Err(generation_projection_error(
            "terminal generation approval evidence is missing",
        )),
        (_, Some(_)) => Err(generation_projection_error(
            "non-terminal generation proposal exposed approval evidence",
        )),
        (_, None) => Ok(()),
    }
}

fn parse_positive_decimal_revision(field: &str, value: &str) -> ShellResult<u64> {
    if value.is_empty()
        || value.len() > 20
        || value.starts_with('0')
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(ShellError::from(lorepia_core::CoreError::invalid(format!(
            "{field} must be a canonical positive u64 decimal string"
        ))));
    }
    value.parse::<u64>().map_err(|_| {
        ShellError::from(lorepia_core::CoreError::invalid(format!(
            "{field} must be a canonical positive u64 decimal string"
        )))
    })
}

fn validate_lowercase_sha256(field: &str, value: &str) -> ShellResult<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(generation_projection_error(format!(
            "{field} is not a canonical lowercase SHA-256 digest"
        )));
    }
    Ok(())
}

fn generation_projection_error(message: impl Into<String>) -> ShellError {
    ShellError::from(lorepia_core::CoreError::new(
        lorepia_core::CoreErrorCode::StorageCorrupted,
        message,
        false,
    ))
}

fn validate_room_and_limit(
    conversation_id: &str,
    branch_id: &str,
    limit: u32,
    maximum: u32,
) -> ShellResult<()> {
    validate_identifier("conversation_id", conversation_id)?;
    validate_identifier("branch_id", branch_id)?;
    if !(1..=maximum).contains(&limit) {
        return Err(ShellError::from(lorepia_core::CoreError::invalid(
            "interaction page limit is outside the supported bound",
        )));
    }
    Ok(())
}

fn validate_effect_text_for_projection(
    value: &str,
) -> Result<(), lorepia_core::OrchestrationValidationError> {
    lorepia_core::validate_interaction_native_text("interaction_effect", value)
}

fn project_proposal_text(
    title: String,
    body: String,
) -> (
    String,
    String,
    Option<InteractionProposalProjectionRejectionReasonDto>,
) {
    if lorepia_core::validate_interaction_native_text("proposal_title", &title).is_ok()
        && lorepia_core::validate_interaction_native_text("proposal_body", &body).is_ok()
    {
        (title, body, None)
    } else {
        (
            REDACTED_PROPOSAL_TITLE.to_owned(),
            REDACTED_PROPOSAL_BODY.to_owned(),
            Some(InteractionProposalProjectionRejectionReasonDto::UnsafeNativeText),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposal_decision_rejects_generic_actions_and_arguments() {
        for invalid in [
            r#"{"conversation_id":"c","branch_id":"b","proposal_record_id":"p","expected_state_revision":1,"expected_proposal_revision":1,"decision":"approve","action":"shell"}"#,
            r#"{"conversation_id":"c","branch_id":"b","proposal_record_id":"p","expected_state_revision":1,"expected_proposal_revision":1,"decision":"approve","arguments":{"path":"/tmp/x"}}"#,
            r#"{"conversation_id":"c","branch_id":"b","proposal_record_id":"p","expected_state_revision":1,"decision":"approve"}"#,
        ] {
            assert!(
                serde_json::from_str::<DecideInteractionProposalInput>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn generation_attempt_decision_is_exact_and_has_no_action_channel() {
        let valid = r#"{"conversation_id":"c","source_branch_id":"b","generation_id":"g","proposal_record_id":"p","expected_aggregate_revision":"18446744073709551615","expected_proposal_revision":"1","decision":"approve"}"#;
        let parsed = serde_json::from_str::<DecideGenerationAttemptProposalInput>(valid)
            .expect("strict exact attempt decision");
        assert_eq!(
            parse_positive_decimal_revision(
                "expected_aggregate_revision",
                &parsed.expected_aggregate_revision,
            )
            .expect("maximum u64 revision"),
            u64::MAX
        );
        for invalid in [
            r#"{"conversation_id":"c","source_branch_id":"b","generation_id":"g","proposal_record_id":"p","expected_aggregate_revision":"1","expected_proposal_revision":"1","decision":"approve","action":"shell"}"#,
            r#"{"conversation_id":"c","source_branch_id":"b","generation_id":"g","proposal_record_id":"p","expected_aggregate_revision":"1","expected_proposal_revision":"1","decision":"approve","arguments":{"path":"/tmp/x"}}"#,
            r#"{"conversation_id":"c","source_branch_id":"b","generation_id":"g","proposal_record_id":"p","expected_aggregate_revision":1,"expected_proposal_revision":"1","decision":"approve"}"#,
            r#"{"conversation_id":"c","source_branch_id":"b","generation_id":"g","proposal_record_id":"p","expected_aggregate_revision":"1","expected_proposal_revision":"1","decision":"expire"}"#,
        ] {
            assert!(
                serde_json::from_str::<DecideGenerationAttemptProposalInput>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn generation_attempt_cas_revisions_require_canonical_positive_decimals() {
        for valid in ["1", "42", "18446744073709551615"] {
            assert!(parse_positive_decimal_revision("revision", valid).is_ok());
        }
        for invalid in [
            "",
            "0",
            "01",
            "+1",
            "-1",
            "1.0",
            "18446744073709551616",
            "999999999999999999999",
        ] {
            assert!(
                parse_positive_decimal_revision("revision", invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn generation_attempt_room_requests_are_bounded_and_strict() {
        assert!(
            serde_json::from_str::<ListGenerationAttemptProposalsInput>(
                r#"{"conversation_id":"c","source_branch_id":"b","status":"pending","limit":100}"#
            )
            .is_ok()
        );
        assert!(
            serde_json::from_str::<ExpireGenerationAttemptProposalsInput>(
                r#"{"conversation_id":"c","source_branch_id":"b","limit":100}"#
            )
            .is_ok()
        );
        for invalid in [
            r#"{"conversation_id":"c","source_branch_id":"b","limit":100,"now_epoch_seconds":42}"#,
            r#"{"conversation_id":"c","source_branch_id":"b","limit":100,"event":{"kind":"user_action"}}"#,
        ] {
            assert!(
                serde_json::from_str::<ExpireGenerationAttemptProposalsInput>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn retryable_generation_attempt_request_is_strict_and_bounded() {
        let valid = serde_json::from_str::<ListRetryableGenerationAttemptsInput>(
            r#"{"conversation_id":"c","source_branch_id":"b","limit":100}"#,
        )
        .expect("bounded retryable attempt request");
        assert!(
            validate_room_and_limit(
                &valid.conversation_id,
                &valid.source_branch_id,
                valid.limit,
                MAX_PROPOSAL_PAGE_SIZE,
            )
            .is_ok()
        );
        assert!(
            serde_json::from_str::<ListRetryableGenerationAttemptsInput>(
                r#"{"conversation_id":"c","source_branch_id":"b","limit":100,"operation_nonce":"forbidden"}"#,
            )
            .is_err()
        );
        assert!(validate_room_and_limit("c", "b", 0, MAX_PROPOSAL_PAGE_SIZE).is_err());
        assert!(validate_room_and_limit("c", "b", 101, MAX_PROPOSAL_PAGE_SIZE).is_err());
    }

    #[test]
    fn retryable_generation_attempt_projection_is_minimal_and_status_closed() {
        let created_at = DateTime::parse_from_rfc3339("2026-08-10T00:00:00Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        let updated_at = DateTime::parse_from_rfc3339("2026-08-10T00:00:01Z")
            .expect("timestamp")
            .with_timezone(&Utc);
        let projection = RetryableGenerationAttemptProjection {
            generation_id: GenerationId("generation-retryable-1".to_owned()),
            status: GenerationAttemptStatus::BeforeGenerationApplied,
            created_at,
            updated_at,
        };
        let dto =
            project_retryable_generation_attempt(projection.clone()).expect("retryable projection");
        let encoded = serde_json::to_value(dto).expect("serialize projection");
        assert_eq!(encoded["generation_id"], "generation-retryable-1");
        assert_eq!(encoded["status"], "before_generation_applied");
        assert_eq!(
            encoded.as_object().expect("projection object").len(),
            4,
            "projection must contain only safe restart fields"
        );

        let mut dispatch_ready = projection.clone();
        dispatch_ready.status = GenerationAttemptStatus::DispatchReady;
        assert!(project_retryable_generation_attempt(dispatch_ready).is_ok());

        let mut invalid = projection;
        invalid.status = GenerationAttemptStatus::Prepared;
        assert!(project_retryable_generation_attempt(invalid.clone()).is_err());
        invalid.status = GenerationAttemptStatus::DispatchReady;
        invalid.updated_at = invalid.created_at - chrono::Duration::seconds(1);
        assert!(project_retryable_generation_attempt(invalid).is_err());
    }

    #[test]
    fn generation_attempt_timestamps_stay_inside_javascript_safe_integer_range() {
        let mut proposal = InteractionProposalRecord {
            id: InteractionProposalRecordId::from("proposal-1"),
            rule_set_id: lorepia_core::InteractionRuleSetId::from("rules-1"),
            rule_id: lorepia_core::InteractionRuleId::from("rule-1"),
            proposal_id: "action-1".to_owned(),
            title: "review".to_owned(),
            body: "bounded".to_owned(),
            status: InteractionProposalStatus::Pending,
            source_interaction_state_revision: 1,
            requested_at_epoch_seconds: MAX_SAFE_JAVASCRIPT_INTEGER,
            expires_at_epoch_seconds: None,
            decided_at_epoch_seconds: None,
        };
        assert!(validate_generation_attempt_proposal_timestamps(&proposal).is_ok());
        proposal.requested_at_epoch_seconds = MAX_SAFE_JAVASCRIPT_INTEGER + 1;
        assert!(validate_generation_attempt_proposal_timestamps(&proposal).is_err());
    }

    #[test]
    fn generation_attempt_terminal_evidence_is_exposed_only_after_the_last_decision() {
        let digest = "a".repeat(64);
        assert!(validate_generation_attempt_decision_evidence(0, Some(&digest)).is_ok());
        assert!(validate_generation_attempt_decision_evidence(0, None).is_err());
        assert!(validate_generation_attempt_decision_evidence(1, Some(&digest)).is_err());
        assert!(validate_generation_attempt_decision_evidence(1, None).is_ok());
        assert!(validate_generation_attempt_decision_evidence(0, Some(&"A".repeat(64))).is_err());
    }

    #[test]
    fn generation_attempt_projection_revisions_and_pending_count_are_bounded() {
        let conversation_id = ConversationId("conversation-1".to_owned());
        let source_branch_id = ConversationBranchId("branch-source".to_owned());
        let proposed_branch_id = ConversationBranchId("branch-proposed".to_owned());
        let generation_id = GenerationId("generation-1".to_owned());
        assert!(
            validate_generation_attempt_projection(GenerationAttemptProjection {
                conversation_id: &conversation_id,
                source_branch_id: &source_branch_id,
                proposed_branch_id: &proposed_branch_id,
                generation_id: &generation_id,
                aggregate_revision: 1,
                interaction_state_revision: 1,
                pending_proposal_count: MAX_GENERATION_ATTEMPT_PENDING_PROPOSALS,
                proposal_revision: 1,
            })
            .is_ok()
        );
        for (aggregate_revision, interaction_state_revision, proposal_revision) in
            [(0, 1, 1), (1, 0, 1), (1, 1, 0)]
        {
            assert!(
                validate_generation_attempt_projection(GenerationAttemptProjection {
                    conversation_id: &conversation_id,
                    source_branch_id: &source_branch_id,
                    proposed_branch_id: &proposed_branch_id,
                    generation_id: &generation_id,
                    aggregate_revision,
                    interaction_state_revision,
                    pending_proposal_count: 1,
                    proposal_revision,
                })
                .is_err()
            );
        }
        assert!(
            validate_generation_attempt_projection(GenerationAttemptProjection {
                conversation_id: &conversation_id,
                source_branch_id: &source_branch_id,
                proposed_branch_id: &proposed_branch_id,
                generation_id: &generation_id,
                aggregate_revision: 1,
                interaction_state_revision: 1,
                pending_proposal_count: MAX_GENERATION_ATTEMPT_PENDING_PROPOSALS + 1,
                proposal_revision: 1,
            })
            .is_err()
        );
    }

    #[test]
    fn variable_effect_projection_has_no_hidden_value_channel() {
        let serialized =
            serde_json::to_string(&InteractionEffectDto::StateChanged).expect("serialize");
        assert_eq!(serialized, r#"{"kind":"state_changed"}"#);
        assert!(!serialized.contains("value"));
        assert!(!serialized.contains("target"));
    }

    #[test]
    fn projection_rejection_is_typed_bounded_and_content_free() {
        let rejected = InteractionEffectDto::ProjectionRejected {
            reason: InteractionEffectProjectionRejectionReasonDto::UnsafeNativeText,
        };
        let serialized = serde_json::to_string(&rejected).expect("serialize rejection");
        assert_eq!(
            serialized,
            r#"{"kind":"projection_rejected","reason":"unsafe_native_text"}"#
        );
        assert!(!serialized.contains("\"text\":"));
        assert!(!serialized.contains("\"title\":"));
        assert!(!serialized.contains("\"body\":"));
        assert!(validate_effect_text_for_projection("안전한 기본 효과").is_ok());
    }

    #[test]
    fn proposal_projection_rejection_is_typed_and_uses_only_fixed_safe_text() {
        const {
            assert!(
                lorepia_core::MAX_INTERACTION_PROPOSAL_BODY_CHARS
                    > lorepia_core::MAX_INTERACTION_NATIVE_TEXT_CHARS,
                "legacy persisted evidence must remain decodable beyond the native projection bound"
            );
        }
        let rejected = InteractionProposalDto {
            id: "proposal-redacted".to_owned(),
            title: REDACTED_PROPOSAL_TITLE.to_owned(),
            body: REDACTED_PROPOSAL_BODY.to_owned(),
            projection_rejection_reason: Some(
                InteractionProposalProjectionRejectionReasonDto::UnsafeNativeText,
            ),
            status: InteractionProposalStatusDto::Pending,
            source_interaction_state_revision: 1,
            requested_at_epoch_seconds: 1,
            expires_at_epoch_seconds: None,
            decided_at_epoch_seconds: None,
        };
        let serialized = serde_json::to_value(&rejected).expect("serialize redacted proposal");
        assert_eq!(
            serialized["projection_rejection_reason"],
            serde_json::json!("unsafe_native_text")
        );
        assert_eq!(
            serialized["title"],
            serde_json::json!(REDACTED_PROPOSAL_TITLE)
        );
        assert_eq!(
            serialized["body"],
            serde_json::json!(REDACTED_PROPOSAL_BODY)
        );
        assert!(
            lorepia_core::validate_interaction_native_text(
                "redacted_proposal_title",
                REDACTED_PROPOSAL_TITLE,
            )
            .is_ok()
        );
        assert!(
            lorepia_core::validate_interaction_native_text(
                "redacted_proposal_body",
                REDACTED_PROPOSAL_BODY,
            )
            .is_ok()
        );

        let mut visible = rejected;
        visible.title = "검토 가능한 제안".to_owned();
        visible.body = "정상 제안 본문".to_owned();
        visible.projection_rejection_reason = None;
        let visible = serde_json::to_value(visible).expect("serialize visible proposal");
        assert!(visible.get("projection_rejection_reason").is_none());
    }

    #[test]
    fn choice_submission_cannot_inject_an_event_or_action_arguments() {
        for invalid in [
            r#"{"conversation_id":"c","branch_id":"b","effect_id":"e","choice_id":"choice","expected_state_revision":1,"event":{"kind":"lifecycle"}}"#,
            r#"{"conversation_id":"c","branch_id":"b","effect_id":"e","choice_id":"choice","expected_state_revision":1,"arguments":{"path":"/tmp/private"}}"#,
            r#"{"conversation_id":"c","branch_id":"b","effect_id":"e","choice_id":"choice"}"#,
        ] {
            assert!(
                serde_json::from_str::<SubmitInteractionChoiceInput>(invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn history_request_is_room_scoped_and_strict() {
        assert!(
            serde_json::from_str::<ListInteractionEffectHistoryInput>(
                r#"{"conversation_id":"c","branch_id":"b","after":null,"limit":50}"#
            )
            .is_ok()
        );
        assert!(serde_json::from_str::<ListInteractionEffectHistoryInput>(
            r#"{"conversation_id":"c","branch_id":"b","after":null,"limit":50,"path":"/private"}"#
        )
        .is_err());
    }

    #[test]
    fn proposal_expiry_request_is_room_scoped_bounded_and_strict() {
        assert!(
            serde_json::from_str::<ExpireInteractionProposalsInput>(
                r#"{"conversation_id":"c","branch_id":"b","limit":100}"#
            )
            .is_ok()
        );
        for invalid in [
            r#"{"conversation_id":"c","branch_id":"b","limit":100,"event":{"kind":"user_action"}}"#,
            r#"{"conversation_id":"c","branch_id":"b","limit":100,"now_epoch_seconds":42}"#,
            r#"{"conversation_id":"c","branch_id":"b"}"#,
        ] {
            assert!(
                serde_json::from_str::<ExpireInteractionProposalsInput>(invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
