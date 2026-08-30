use std::{
    io::Write,
    sync::{Arc, Barrier},
    thread,
};

use chrono::Duration;
use lorepia_domain::{
    ApiFamily, AssetId, Character, CharacterPromptContent, ChoiceSpec, Conversation,
    ConversationMode, DiceExpression, InteractionAction, InteractionRule, InteractionRuleSet,
    InteractionRuleSetId, Message, MessageId, PresetMetadata, PromptConversationMessage,
    PromptMessageRole, PromptPresetId, PromptResolutionContext, PromptResolveRequest, ProposalSpec,
    Provenance, ProviderMessageRole, ProviderPromptContract, SafeTemplate, SourceKind,
    TemplatePart, UiRegion, UnsupportedRolePolicy,
};
use serde_json::json;
use tempfile::{NamedTempFile, TempDir, tempdir};
use uuid::Uuid;

use super::*;
use crate::{
    GenerationAttemptInput, GenerationAttemptStatus,
    orchestration::{GenerationPromptPlanRecord, ProviderRequestSnapshotRecord},
};

mod checkpoints;
mod derived_outbox;
mod effects;
mod generation_materialization;
mod generation_review;
mod generation_support;
mod proposals;
mod recovery;

fn interaction_storage() -> (TempDir, Storage, ConversationId, ConversationBranchId) {
    let root = tempdir().expect("temp root");
    let mut staged = NamedTempFile::new_in(root.path()).expect("staging file");
    staged.write_all(b"character").expect("write character");
    let character = Character::new("Segu", "Guide", sha256_hex(b"character"));
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .commit_character_import(
            staged.path(),
            &character,
            9,
            &Uuid::new_v4().to_string(),
            &[],
        )
        .expect("commit character");
    let conversation = Conversation::new(&character.id, &character.name);
    let (_, state) = storage
        .save_conversation_with_mode(&conversation, ConversationMode::Chat)
        .expect("save conversation");
    (root, storage, conversation.id, state.active_branch_id)
}

fn empty_state(revision: u64) -> InteractionState {
    InteractionState {
        variables: lorepia_domain::VariableMap::default(),
        manually_active_knowledge: Vec::new(),
        proposals: Vec::new(),
        revision,
    }
}

fn empty_policy() -> InteractionPolicySnapshot {
    InteractionPolicySnapshot {
        module_plan_sha256: None,
        rule_sets: Vec::new(),
    }
}

fn policy_for_rule_set(
    storage: &Storage,
    rule_set_id: &InteractionRuleSetId,
    revision_id: &str,
) -> InteractionPolicySnapshot {
    let revision_sha256 = storage
        .connection()
        .expect("open policy test connection")
        .query_row(
            "SELECT document_sha256
             FROM content_revisions
             WHERE id = ?1 AND object_id = ?2",
            params![revision_id, rule_set_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load rule-set revision hash");
    InteractionPolicySnapshot {
        module_plan_sha256: None,
        rule_sets: vec![InteractionPolicyRuleSetRevision {
            rule_set_id: rule_set_id.clone(),
            revision_id: revision_id.to_owned(),
            sha256: revision_sha256,
        }],
    }
}

fn choice_spec(id: &str, label: &str) -> ChoiceSpec {
    ChoiceSpec {
        id: id.to_owned(),
        label: label.to_owned(),
        value: VariableValue::Text(id.to_owned()),
        enabled_when: None,
    }
}

fn persist_effect_bundle(
    storage: &Storage,
    key: &InteractionStateKey,
    effects: Vec<InteractionEffect>,
    created_at: DateTime<Utc>,
) {
    storage
        .get_or_init_interaction_state(key, &empty_state(0), &[], created_at)
        .expect("initialize interaction effect state");
    storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: format!("{}-event", key.state_id),
            idempotency_key: format!("{}-event-key", key.state_id),
            key: key.clone(),
            expected_state_revision: 0,
            event: InteractionEvent::ConversationOpened,
            generation_attempt_id: None,
            owner_message_id: None,
            policy: empty_policy(),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: empty_state(1),
            knowledge: Vec::new(),
            action_results: Vec::new(),
            effects,
            derived_events: Vec::new(),
            proposals: Vec::new(),
            created_at,
        })
        .expect("persist interaction effect bundle");
}

fn text_template(value: &str) -> SafeTemplate {
    SafeTemplate {
        parts: vec![TemplatePart::Text {
            value: value.to_owned(),
        }],
        max_output_chars: 128,
    }
}

fn install_approval_rules(
    storage: &Storage,
) -> (
    InteractionRuleSetId,
    InteractionRuleId,
    InteractionRuleId,
    String,
) {
    let provenance = Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: None,
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    };
    let rule_set_id = InteractionRuleSetId::from("approval-rules");
    let request_rule_id = InteractionRuleId::from("request-rule");
    let approve_rule_id = InteractionRuleId::from("approve-rule");
    let rule_set = InteractionRuleSet {
        id: rule_set_id.clone(),
        name: "Approval rules".to_owned(),
        schema_version: 1,
        rules: vec![
            InteractionRule {
                id: request_rule_id.clone(),
                name: "Request approval".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::ConversationOpened,
                condition: None,
                actions: vec![InteractionAction::RequestUserApproval {
                    proposal: ProposalSpec {
                        id: "approve-change".to_owned(),
                        title: "Approve change".to_owned(),
                        body: text_template("Allow this change?"),
                        expires_after_seconds: Some(60),
                    },
                }],
                priority: 0,
                stop_after_match: false,
                provenance: provenance.clone(),
            },
            InteractionRule {
                id: approve_rule_id.clone(),
                name: "Apply approval".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::UserAction {
                    action_id: "approve-change".to_owned(),
                },
                condition: None,
                actions: vec![InteractionAction::AppendVisibleSystemEvent {
                    text: text_template("Change approved"),
                }],
                priority: 1,
                stop_after_match: false,
                provenance: provenance.clone(),
            },
        ],
        max_actions_per_event: 8,
        provenance,
    };
    let revision_id = storage
        .save_interaction_rule_set(&rule_set, None)
        .expect("save rules")
        .revision_id
        .expect("immutable rule-set revision");
    (rule_set_id, request_rule_id, approve_rule_id, revision_id)
}

fn persist_proposal_request(
    storage: &Storage,
    key: InteractionStateKey,
    record_id: &str,
    rule_set_id: &InteractionRuleSetId,
    request_rule_id: &InteractionRuleId,
    rule_set_revision_id: &str,
) -> InteractionState {
    storage
        .get_or_init_interaction_state(&key, &empty_state(0), &[], Utc::now())
        .expect("initialize state");
    let proposal = InteractionProposalRecord {
        id: interaction_proposal_record_id(rule_set_id, request_rule_id, "approve-change", 0)
            .expect("derive proposal record id"),
        rule_set_id: rule_set_id.clone(),
        rule_id: request_rule_id.clone(),
        proposal_id: "approve-change".to_owned(),
        title: "Approve change".to_owned(),
        body: "Allow this change?".to_owned(),
        status: InteractionProposalStatus::Pending,
        source_interaction_state_revision: 0,
        requested_at_epoch_seconds: 100,
        expires_at_epoch_seconds: Some(160),
        decided_at_epoch_seconds: None,
    };
    let mut requested_state = empty_state(1);
    requested_state.proposals.push(proposal.clone());
    storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: format!("{record_id}-request-event"),
            idempotency_key: format!("{record_id}-request-key"),
            key: key.clone(),
            expected_state_revision: 0,
            event: InteractionEvent::ConversationOpened,
            generation_attempt_id: None,
            owner_message_id: None,
            policy: policy_for_rule_set(storage, rule_set_id, rule_set_revision_id),
            evaluation_seal: None,
            deterministic_seed: None,
            next_state: requested_state.clone(),
            knowledge: Vec::new(),
            action_results: vec![InteractionActionResultWrite {
                set_revision_id: rule_set_revision_id.to_owned(),
                rule_id: request_rule_id.clone(),
                action_ordinal: 0,
                status: InteractionActionResultStatus::Proposed,
                result: VersionedJson {
                    schema_version: 1,
                    value: json!({"status": "proposal_requested"}),
                },
            }],
            effects: vec![InteractionEffect::ApprovalRequested {
                rule_set_id: rule_set_id.clone(),
                rule_id: request_rule_id.clone(),
                proposal_id: "approve-change".to_owned(),
                title: "Approve change".to_owned(),
                body: "Allow this change?".to_owned(),
                expires_after_seconds: Some(60),
            }],
            derived_events: Vec::new(),
            proposals: vec![InteractionProposalWrite {
                review_payload_sha256: interaction_proposal_review_sha256(&proposal)
                    .expect("proposal digest"),
                record: proposal,
                rule_set_revision_id: rule_set_revision_id.to_owned(),
                action_ordinal: 0,
            }],
            created_at: Utc::now(),
        })
        .expect("commit proposal request");
    requested_state
}
