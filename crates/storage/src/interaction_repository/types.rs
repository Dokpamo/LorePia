use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, CoreError, CoreResult, GenerationId, InteractionAction,
    InteractionEffect, InteractionEvent, InteractionProposalRecord, InteractionProposalRecordId,
    InteractionRuleId, InteractionRuleSetId, InteractionState, KnowledgeEntryId, MessageId,
    Sha256Digest, VersionedJson,
};
use lorepia_orchestration::{AppliedModuleRuntimePlan, ModuleMergeReview};
use serde::{Deserialize, Serialize};

use crate::{
    GenerationApprovalEvidence, GenerationAttemptDerivedClosure, GenerationBeforeEventEvidence,
    InteractionEvaluationSeal, MemoryRecordsAtHeadSnapshot,
};

pub(super) use super::derived_outbox::{
    InteractionDerivedEventCommit, InteractionDerivedEventWrite,
    MAX_INTERACTION_DERIVED_CHAIN_EVENTS,
};
use super::{
    encode_json, sha256_hex, validate_knowledge_bindings, validate_policy_shape, validate_state,
};

pub(super) const MAX_STATE_JSON_BYTES: usize = 8 * 1_024 * 1_024;
pub(super) const MAX_EVENT_JSON_BYTES: usize = 1_024 * 1_024;
pub(super) const MAX_AUDIT_JSON_BYTES: usize = 256 * 1_024;
pub(super) const MAX_JSON_DEPTH: usize = 32;
pub(super) const MAX_JSON_NODES: usize = 200_000;
pub(super) const MAX_ACTION_RESULTS_PER_EVENT: usize = 1_024;
pub(super) const MAX_EFFECTS_PER_EVENT: usize = 1_024;

/// Stable identity of one interaction state row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionStateKey {
    pub state_id: String,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
}

/// Derives the one canonical durable interaction-state identity for a branch.
pub fn interaction_state_key_for_branch(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> CoreResult<InteractionStateKey> {
    let encoded = serde_json::to_vec(&("lorepia.interaction-state.v1", conversation_id, branch_id))
        .map_err(|error| {
            CoreError::internal(format!("cannot hash interaction state key: {error}"))
        })?;
    Ok(InteractionStateKey {
        state_id: format!("interaction-state-{}", sha256_hex(&encoded)),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
    })
}

/// Revision-pinned knowledge entry represented in the normalized state table.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionKnowledgeBinding {
    pub book_revision_id: String,
    pub entry_id: KnowledgeEntryId,
}

/// Durable status of one evaluated declarative action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionActionResultStatus {
    Proposed,
    Applied,
    Skipped,
    Failed,
}

/// One normalized action result associated with a durable interaction event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionActionResultWrite {
    pub set_revision_id: String,
    pub rule_id: InteractionRuleId,
    pub action_ordinal: u32,
    pub status: InteractionActionResultStatus,
    pub result: VersionedJson,
}

/// Metadata that binds a newly requested proposal to its exact reviewed rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalWrite {
    pub record: InteractionProposalRecord,
    pub rule_set_revision_id: String,
    pub action_ordinal: u32,
    /// SHA-256 of the canonical serialized [`InteractionProposalRecord`].
    pub review_payload_sha256: String,
}

/// Exact immutable identity of one ordered rule-set revision used to evaluate
/// an interaction event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionPolicyRuleSetRevision {
    pub rule_set_id: InteractionRuleSetId,
    pub revision_id: String,
    pub sha256: String,
}

/// Exact module-plan and ordered rule-set policy used for one interaction
/// evaluation. `None` means the canonical no-active-module-plan policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionPolicySnapshot {
    pub module_plan_sha256: Option<String>,
    pub rule_sets: Vec<InteractionPolicyRuleSetRevision>,
}

/// Atomic write for one ordinary interaction event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEventCommit {
    pub event_id: String,
    pub idempotency_key: String,
    pub key: InteractionStateKey,
    pub expected_state_revision: u64,
    pub event: InteractionEvent,
    pub generation_attempt_id: Option<GenerationId>,
    pub owner_message_id: Option<MessageId>,
    pub policy: InteractionPolicySnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluation_seal: Option<InteractionEvaluationSeal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deterministic_seed: Option<u64>,
    pub next_state: InteractionState,
    pub knowledge: Vec<InteractionKnowledgeBinding>,
    pub action_results: Vec<InteractionActionResultWrite>,
    pub effects: Vec<InteractionEffect>,
    #[serde(default)]
    pub derived_events: Vec<InteractionDerivedEventWrite>,
    pub proposals: Vec<InteractionProposalWrite>,
    pub created_at: DateTime<Utc>,
}

/// Exact read-only identity of one already-attempted interaction occurrence.
///
/// Core uses this before re-evaluation so a crash after commit but before
/// outbox acknowledgement returns the stored transition instead of evaluating
/// against an advanced state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEventOccurrenceLookup {
    pub event_id: String,
    pub idempotency_key: String,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub event: InteractionEvent,
    pub generation_attempt_id: Option<GenerationId>,
    pub owner_message_id: Option<MessageId>,
    pub occurred_at: DateTime<Utc>,
}

/// State and revision-pinned knowledge read under one storage lock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredInteractionState {
    pub key: InteractionStateKey,
    pub state: InteractionState,
    pub knowledge: Vec<InteractionKnowledgeBinding>,
}

/// Result of an ordinary event commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredInteractionEvent {
    pub event_id: String,
    pub idempotency_key: String,
    pub interaction_state_id: String,
    pub expected_state_revision: u64,
    pub resulting_state_revision: u64,
    pub exact_replay: bool,
    pub generation_attempt_id: Option<GenerationId>,
    pub owner_message_id: Option<MessageId>,
    pub commit_sha256: String,
    pub resulting_state_snapshot_sha256: String,
    pub proposal_review_sha256s: Vec<String>,
    pub policy: InteractionPolicySnapshot,
    pub policy_sha256: String,
    pub created_at: DateTime<Utc>,
}

/// Stages one generation-owned `BeforeGeneration` review without mutating a
/// live conversation branch.
///
/// The exact previous boundary may belong to the current branch head or to a
/// historical checkpoint. Storage re-reads and verifies that boundary before
/// accepting the staged outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptBeforeReviewCommit {
    pub generation_id: GenerationId,
    pub expected_attempt_revision: u64,
    pub event_id: String,
    pub occurred_at: DateTime<Utc>,
    pub context_head_message_id: Option<MessageId>,
    pub context_checkpoint_sha256: String,
    pub previous_state: InteractionState,
    pub previous_knowledge: Vec<InteractionKnowledgeBinding>,
    /// Exact target-context module review, including for the no-module
    /// sentinel. Dispatch freshness re-resolves and compares this review.
    pub module_runtime_review: ModuleMergeReview,
    pub memory_head_snapshot: MemoryRecordsAtHeadSnapshot,
    pub applied_runtime_plan: Option<AppliedModuleRuntimePlan>,
    pub policy: InteractionPolicySnapshot,
    pub evaluation_seal: InteractionEvaluationSeal,
    pub derived_closure: GenerationAttemptDerivedClosure,
    pub next_state: InteractionState,
    pub knowledge: Vec<InteractionKnowledgeBinding>,
    pub action_results: Vec<InteractionActionResultWrite>,
    pub effects: Vec<InteractionEffect>,
    #[serde(default)]
    pub derived_events: Vec<InteractionDerivedEventWrite>,
    pub proposals: Vec<InteractionProposalWrite>,
    /// Core's immutable `InteractionEventReview.review_sha256`.
    pub review_sha256: String,
}

/// Immutable attempt-owned `BeforeGeneration` snapshot and attempt evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredGenerationAttemptBeforeReview {
    pub generation_id: GenerationId,
    pub event_id: String,
    /// Full staged-commit fingerprint, not merely the event JSON hash.
    pub event_sha256: Sha256Digest,
    /// Generation-bound storage review identity used by attempt-owned FKs.
    pub review_sha256: Sha256Digest,
    /// Original Core review digest preserved as immutable review evidence.
    pub domain_review_sha256: Sha256Digest,
    pub storage_identity_version: u32,
    pub closure_authority_version: u32,
    pub evaluation_seal: InteractionEvaluationSeal,
    pub evaluation_seal_sha256: Sha256Digest,
    pub derived_closure: GenerationAttemptDerivedClosure,
    pub derived_closure_sha256: Sha256Digest,
    /// Exact attempt-owned module plan reviewed before provider dispatch.
    /// This can precede its publication to the ordinary historical-plan table.
    pub applied_runtime_plan: Option<AppliedModuleRuntimePlan>,
    /// Exact at-head memory revision set reviewed before any approval pause.
    pub memory_head_snapshot: MemoryRecordsAtHeadSnapshot,
    pub prompt_selection_authority: crate::GenerationPromptSelectionAuthority,
    pub previous_state_revision: u64,
    pub previous_state_snapshot_sha256: Sha256Digest,
    pub resulting_state_revision: u64,
    pub resulting_state_snapshot_sha256: Sha256Digest,
    pub proposal_review_sha256s: Vec<Sha256Digest>,
    pub pending_proposal_count: u32,
    pub evidence: GenerationBeforeEventEvidence,
    pub evidence_sha256: Sha256Digest,
    pub approval_evidence: Option<GenerationApprovalEvidence>,
    pub approval_evidence_sha256: Option<Sha256Digest>,
    pub exact_replay: bool,
    pub created_at: DateTime<Utc>,
}

/// Current isolated interaction state for one generation attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredGenerationAttemptInteractionAggregate {
    pub generation_id: GenerationId,
    pub aggregate_revision: u64,
    pub state: InteractionState,
    pub knowledge: Vec<InteractionKnowledgeBinding>,
    pub state_snapshot_sha256: Sha256Digest,
    pub evaluation_seal_sha256: Sha256Digest,
    pub derived_chain_sha256: Sha256Digest,
    pub derived_event_count: u32,
    pub derived_guard_count: u32,
    pub closure_authority_version: u32,
    pub pending_proposal_count: u32,
    pub terminal_decision_count: u32,
    pub decision_event_ids: Vec<String>,
    pub decision_event_sha256s: Vec<Sha256Digest>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One immutable attempt-owned proposal and its current decision CAS state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredGenerationAttemptProposal {
    pub generation_id: GenerationId,
    pub conversation_id: ConversationId,
    pub source_branch_id: ConversationBranchId,
    pub proposed_branch_id: ConversationBranchId,
    pub ordinal: u32,
    pub record: InteractionProposalRecord,
    /// Original deterministic engine identity before Storage namespaces the
    /// attempt-owned record for ID-only lookup across concurrent attempts.
    pub domain_proposal_record_id: InteractionProposalRecordId,
    pub before_event_snapshot_sha256: Sha256Digest,
    pub origin_policy: InteractionPolicySnapshot,
    pub origin_policy_sha256: Sha256Digest,
    pub origin_event_id: String,
    pub origin_chain_ordinal: u32,
    pub origin_aggregate_revision: u64,
    pub origin_evaluation_seal: InteractionEvaluationSeal,
    pub origin_evaluation_seal_sha256: Sha256Digest,
    pub rule_set_revision_id: String,
    pub action_ordinal: u32,
    pub action_payload_sha256: Sha256Digest,
    pub proposal_revision: u64,
    pub proposal_review_sha256: Sha256Digest,
    /// Original engine review hash before Storage assigns the globally unique
    /// attempt-owned proposal record identity.
    pub domain_proposal_review_sha256: Sha256Digest,
    pub storage_identity_version: u32,
    pub decision_idempotency_key: Option<String>,
    pub decision_event_id: Option<String>,
    pub decision_event_sha256: Option<Sha256Digest>,
    pub resulting_aggregate_revision: Option<u64>,
    pub decided_at_epoch_seconds: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Terminal attempt-owned proposal decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationAttemptProposalDecision {
    Approve,
    Reject,
    Expire,
}

/// Atomic proposal decision against the isolated generation aggregate.
///
/// Approval requires one exact derived `UserAction(stored proposal id)`
/// materialization. Rejection and expiry require `derived = None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalDecisionCommit {
    pub proposal_record_id: InteractionProposalRecordId,
    pub expected_proposal_revision: u64,
    pub expected_aggregate_revision: u64,
    pub decision: GenerationAttemptProposalDecision,
    pub decision_idempotency_key: String,
    pub decided_at_epoch_seconds: i64,
    pub decision_state: InteractionState,
    pub current_policy: Option<InteractionPolicySnapshot>,
    pub evaluation_seal: Option<InteractionEvaluationSeal>,
    pub derived_closure: Option<GenerationAttemptDerivedClosure>,
    pub derived: Option<InteractionDerivedEventCommit>,
    pub updated_at: DateTime<Utc>,
}

/// Result of one exact isolated proposal decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationAttemptProposalDecisionReceipt {
    pub proposal: StoredGenerationAttemptProposal,
    pub aggregate: StoredGenerationAttemptInteractionAggregate,
    pub approval_evidence: Option<GenerationApprovalEvidence>,
    pub approval_evidence_sha256: Option<Sha256Digest>,
    pub exact_replay: bool,
}

/// One claimed or pending durable UI effect.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredInteractionEffect {
    /// Stable occurrence identity, derived from `(event_id, sequence)`.
    pub effect_id: String,
    pub event_id: String,
    pub sequence: u64,
    pub interaction_state_id: String,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub resulting_state_revision: u64,
    pub event_created_at: DateTime<Utc>,
    pub policy: InteractionPolicySnapshot,
    pub policy_sha256: String,
    pub effect: InteractionEffect,
    pub available_at: DateTime<Utc>,
    pub delivery_attempts: u64,
    pub delivered_at: Option<DateTime<Utc>>,
    pub choice_status: Option<InteractionChoiceEffectStatus>,
    pub selected_choice_id: Option<String>,
    pub choice_decided_at_epoch_seconds: Option<i64>,
}

/// Durable lifecycle of one `ChoicesPresented` effect.
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

/// Immutable effect history plus its mutable delivery and choice-lifecycle
/// metadata. Delivery acknowledgement never removes the history record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredInteractionEffectHistory {
    pub stored: StoredInteractionEffect,
    /// False only for one-shot audio. Reopen reconstruction can safely replay
    /// every other durable effect while respecting choice lifecycle status.
    pub replay_on_reopen: bool,
}

/// Immutable interaction state captured at one committed message boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredInteractionStateCheckpoint {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub message_id: MessageId,
    pub source_interaction_state_id: String,
    pub state: InteractionState,
    pub knowledge: Vec<InteractionKnowledgeBinding>,
    pub checkpoint_sha256: String,
    pub created_at: DateTime<Utc>,
}

/// Verified interaction boundary frozen for a generation attempt review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredGenerationAttemptInteractionBoundary {
    pub state: StoredInteractionState,
    /// Exact source authority. For a fork this may hash the unpruned
    /// historical checkpoint while `state` is the safe branch-local clone.
    pub context_checkpoint_sha256: String,
}

/// Verified result of consuming one isolated generation interaction chain at
/// the atomic generation-append boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenerationAttemptInteractionMaterializationReceipt {
    pub final_state_revision: u64,
    pub final_state_snapshot_sha256: Sha256Digest,
}

/// Exact source checkpoint and the branch-local state cloned from it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ClonedInteractionCheckpoint {
    pub source: Option<StoredInteractionStateCheckpoint>,
    pub cloned: StoredInteractionState,
    pub checkpoint_sha256: String,
    pub cloned_state_document_sha256: String,
    pub cloned_state_snapshot_sha256: String,
}

/// Atomic selection of one exact choice from one exact durable choice effect.
///
/// There is intentionally no caller-controlled event or action argument. The
/// repository derives `UserAction(selected_stored_choice_id)` from the durable
/// effect payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionChoiceSelectionCommit {
    pub effect_id: String,
    pub choice_id: String,
    pub expected_state_revision: u64,
    /// Trusted timestamp supplied by Rust Core, never by the frontend.
    pub selected_at_epoch_seconds: i64,
    /// Freshly resolved policy. Storage compares it byte-for-byte with the
    /// originating effect policy before deriving the fixed `UserAction`.
    pub current_policy: InteractionPolicySnapshot,
    pub derived: InteractionDerivedEventCommit,
}

/// Result of consuming one durable choice exactly once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionChoiceSelectionReceipt {
    pub choice_effect: StoredInteractionEffectHistory,
    pub event: StoredInteractionEvent,
    pub resulting_state_revision: u64,
}

/// Storage-only expiration of a still-pending choice effect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionChoiceExpirationCommit {
    pub effect_id: String,
    /// Trusted timestamp supplied by Rust Core, never by the frontend.
    pub expired_at_epoch_seconds: i64,
}

/// Canonical durable proposal plus persistence-only CAS and dispatch metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredInteractionProposal {
    pub record: InteractionProposalRecord,
    pub interaction_state_id: String,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    /// Current revision of the containing interaction state at read time.
    pub state_revision: u64,
    pub origin_policy: InteractionPolicySnapshot,
    pub origin_policy_sha256: String,
    pub rule_set_revision_id: String,
    pub action_ordinal: u32,
    pub proposal_revision: u64,
    pub payload_sha256: String,
    pub dispatched_at_epoch_seconds: Option<i64>,
}

/// Pending-proposal rejection. The supplied state is checked against the only
/// state the repository can derive from the durable pending proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalRejectionCommit {
    pub proposal_record_id: InteractionProposalRecordId,
    pub expected_state_revision: u64,
    pub expected_proposal_revision: u64,
    /// Trusted timestamp supplied by Rust Core, never by the frontend.
    pub decided_at_epoch_seconds: i64,
    pub decision_state: InteractionState,
    pub updated_at: DateTime<Utc>,
}

/// Atomic terminalization of all due pending proposals in one room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalExpiryCommit {
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub expected_state_revision: u64,
    /// Trusted timestamp supplied by Rust Core, never by the frontend.
    pub now_epoch_seconds: i64,
    pub updated_at: DateTime<Utc>,
}

/// Result of one synchronous due-proposal expiry pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalExpiryReceipt {
    pub state: InteractionState,
    pub expired_proposals: Vec<StoredInteractionProposal>,
}

/// Pending-proposal approval and its optional derived `UserAction` outcome.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalApprovalCommit {
    pub proposal_record_id: InteractionProposalRecordId,
    pub expected_state_revision: u64,
    pub expected_proposal_revision: u64,
    /// Trusted timestamp supplied by Rust Core, never by the frontend.
    pub decided_at_epoch_seconds: i64,
    /// Freshly resolved policy, compared with the immutable proposal-origin
    /// policy before dispatch.
    pub current_policy: InteractionPolicySnapshot,
    pub decision_state: InteractionState,
    pub derived: Option<InteractionDerivedEventCommit>,
    pub updated_at: DateTime<Utc>,
}

/// Result of an atomic proposal approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionProposalApprovalReceipt {
    pub proposal: StoredInteractionProposal,
    pub event: Option<StoredInteractionEvent>,
    pub resulting_state_revision: u64,
}

/// Computes the exact review digest required by [`InteractionProposalWrite`].
pub fn interaction_proposal_review_sha256(
    record: &InteractionProposalRecord,
) -> CoreResult<String> {
    let json = encode_json("interaction proposal", record, MAX_EVENT_JSON_BYTES)?;
    Ok(sha256_hex(json.as_bytes()))
}

/// Canonical digest of one immutable declarative action payload.
pub fn interaction_action_sha256(action: &InteractionAction) -> CoreResult<Sha256Digest> {
    let json = encode_json("interaction action", action, MAX_EVENT_JSON_BYTES)?;
    Sha256Digest::parse(sha256_hex(json.as_bytes())).map_err(CoreError::invalid)
}

pub(super) fn interaction_event_sha256(event: &InteractionEvent) -> CoreResult<Sha256Digest> {
    let json = encode_json("interaction derived event", event, MAX_EVENT_JSON_BYTES)?;
    Sha256Digest::parse(sha256_hex(json.as_bytes())).map_err(CoreError::invalid)
}

/// Computes the canonical immutable fingerprint of an interaction policy.
pub fn interaction_policy_sha256(policy: &InteractionPolicySnapshot) -> CoreResult<String> {
    validate_policy_shape(policy)?;
    let json = encode_json(
        "interaction policy",
        &InteractionPolicyFingerprint {
            schema_version: 1,
            module_plan_sha256: policy.module_plan_sha256.as_deref(),
            ordered_rule_sets: &policy.rule_sets,
        },
        MAX_EVENT_JSON_BYTES,
    )?;
    Ok(sha256_hex(json.as_bytes()))
}

/// SHA-256 of canonical interaction state and exact knowledge-revision
/// bindings. This is the state authority used by message checkpoints and
/// generation attempt evidence.
pub fn interaction_state_snapshot_sha256(
    state: &InteractionState,
    knowledge: &[InteractionKnowledgeBinding],
) -> CoreResult<String> {
    validate_state(state)?;
    validate_knowledge_bindings(state, knowledge)?;
    let mut ordered_knowledge = knowledge.to_vec();
    ordered_knowledge.sort();
    let json = encode_json(
        "interaction state snapshot fingerprint",
        &InteractionStateSnapshotFingerprint {
            schema_version: 1,
            state,
            ordered_knowledge: &ordered_knowledge,
        },
        MAX_STATE_JSON_BYTES,
    )?;
    Ok(sha256_hex(json.as_bytes()))
}

#[derive(Serialize)]
struct InteractionPolicyFingerprint<'a> {
    schema_version: u32,
    module_plan_sha256: Option<&'a str>,
    ordered_rule_sets: &'a [InteractionPolicyRuleSetRevision],
}

#[derive(Serialize)]
struct InteractionStateSnapshotFingerprint<'a> {
    schema_version: u32,
    state: &'a InteractionState,
    ordered_knowledge: &'a [InteractionKnowledgeBinding],
}
