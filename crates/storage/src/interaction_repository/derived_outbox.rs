//! Durable derived-event outbox contracts and materialization.

mod claim;
mod commit;
mod enqueue;
mod quarantine;
mod recovery;
mod row_mapping;
mod validation;

pub(in crate::interaction_repository) use enqueue::{
    DerivedChainParent, DerivedEventOutboxWrite, write_derived_event_outbox,
};
pub(in crate::interaction_repository) use validation::{
    require_no_pending_derived_predecessor, require_no_pending_derived_predecessor_through,
    validate_derived_event_writes,
};

use chrono::{DateTime, Utc};
use lorepia_domain::{
    ConversationBranchId, ConversationId, InteractionEffect, InteractionEvent, InteractionRuleId,
    InteractionState, Sha256Digest,
};
use serde::{Deserialize, Serialize};

use crate::InteractionEvaluationSeal;

use super::types::{
    InteractionActionResultWrite, InteractionKnowledgeBinding, InteractionPolicySnapshot,
    InteractionProposalWrite, InteractionStateKey,
};

pub const MAX_INTERACTION_DERIVED_CHAIN_DEPTH: u32 = 16;
pub const MAX_INTERACTION_DERIVED_CHAIN_EVENTS: u32 = 256;
const MAX_INTERACTION_DERIVED_CLAIM: u32 = 64;

/// Exact action/effect authority for one typed derived event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionDerivedEventWrite {
    pub event: InteractionEvent,
    pub deterministic_seed: u64,
    pub source_set_revision_id: String,
    pub source_rule_id: InteractionRuleId,
    pub source_action_ordinal: u32,
    pub source_effect_ordinal: u32,
    pub source_action_sha256: Sha256Digest,
}

/// The optional event portion of a proposal approval.
///
/// There is intentionally no `event` field. Storage derives the only allowed
/// event, `UserAction(stored_proposal_id)`, from the durable proposal row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionDerivedEventCommit {
    pub event_id: String,
    pub idempotency_key: String,
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

/// One claimed durable derived-event occurrence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredInteractionDerivedEvent {
    pub occurrence_id: String,
    pub chain_id: String,
    pub root_event_id: String,
    pub parent_event_id: String,
    pub parent_occurrence_id: Option<String>,
    pub conversation_id: ConversationId,
    pub branch_id: ConversationBranchId,
    pub depth: u32,
    pub chain_ordinal: u32,
    pub source_effect_ordinal: u32,
    pub parent_event_commit_sha256: Sha256Digest,
    pub parent_resulting_state_revision: u64,
    pub source_effect_sha256: Sha256Digest,
    pub source_action_sha256: Sha256Digest,
    pub source_set_revision_id: String,
    pub source_rule_id: InteractionRuleId,
    pub source_action_ordinal: u32,
    pub event: InteractionEvent,
    pub event_sha256: Sha256Digest,
    pub visited_event_sha256s: Vec<Sha256Digest>,
    pub policy: InteractionPolicySnapshot,
    pub policy_sha256: Sha256Digest,
    pub evaluation_seal: InteractionEvaluationSeal,
    pub evaluation_seal_sha256: Sha256Digest,
    pub deterministic_seed: u64,
    pub occurred_at: DateTime<Utc>,
    pub available_at: DateTime<Utc>,
    pub delivery_attempts: u64,
    pub lease_until: Option<DateTime<Utc>>,
}

/// Immutable terminal evidence for a claimed occurrence whose sealed policy
/// could not be recovered. Quarantined occurrences are never acknowledged as
/// successful events and no longer block later work on the branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredInteractionDerivedEventQuarantine {
    pub occurrence_id: String,
    pub delivery_attempts: u64,
    pub sealed_policy_sha256: Sha256Digest,
    pub active_policy_sha256: Option<Sha256Digest>,
    pub source_effect_sha256: Sha256Digest,
    pub source_action_sha256: Sha256Digest,
    pub evidence_sha256: Sha256Digest,
    pub quarantined_at: DateTime<Utc>,
    pub exact_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionDerivedEventSupervisorStatus {
    pub pending_count: u64,
    pub next_available_at: Option<DateTime<Utc>>,
}

/// Atomic materialization of one claimed derived occurrence.
///
/// The caller supplies only the evaluated result. Storage derives the event,
/// policy, event ID and idempotency key from the immutable outbox row and
/// acknowledges that row in the same transaction as the state/event commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionDerivedOccurrenceCommit {
    pub occurrence_id: String,
    pub expected_delivery_attempts: u64,
    pub key: InteractionStateKey,
    pub expected_state_revision: u64,
    pub next_state: InteractionState,
    pub knowledge: Vec<InteractionKnowledgeBinding>,
    pub action_results: Vec<InteractionActionResultWrite>,
    pub effects: Vec<InteractionEffect>,
    pub derived_events: Vec<InteractionDerivedEventWrite>,
    pub proposals: Vec<InteractionProposalWrite>,
    pub committed_at: DateTime<Utc>,
}
