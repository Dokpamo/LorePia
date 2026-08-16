//! Restart and tamper invariants for state-derived interaction events.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Duration, Utc};
use lorepia_domain::{
    Character, Conversation, ConversationMode, CoreErrorCode, InteractionAction, InteractionEffect,
    InteractionEvent, InteractionRule, InteractionRuleId, InteractionRuleSet, InteractionRuleSetId,
    InteractionState, Provenance, Sha256Digest, SourceKind, ValueExpr, VariableMap, VariableRef,
    VariableScope, VariableValue, VersionedJson,
};
use lorepia_storage::{
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionDerivedEventWrite,
    InteractionDerivedOccurrenceCommit, InteractionEvaluationLimits, InteractionEvaluationSeal,
    InteractionEvaluationTemplateValues, InteractionEventCommit, InteractionPolicyRuleSetRevision,
    InteractionPolicySnapshot, InteractionStateKey, Storage, StoredInteractionDerivedEvent,
    StoredInteractionEvent, interaction_action_sha256, interaction_policy_sha256,
    interaction_state_key_for_branch,
};
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, TempDir, tempdir};
use uuid::Uuid;

struct Fixture {
    root: TempDir,
    storage: Storage,
    key: InteractionStateKey,
    policy: InteractionPolicySnapshot,
    revision_id: String,
    rules: Vec<InteractionRule>,
}

#[derive(Clone)]
struct VariableChange {
    rule_id: InteractionRuleId,
    action_ordinal: u32,
    target: VariableRef,
    previous: Option<VariableValue>,
    value: VariableValue,
}

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = std::fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<Value>(
                &std::fs::read(entry.path().join("generation-manifest.json"))
                    .expect("read generation manifest"),
            )
            .expect("parse generation manifest");
            let sequence = manifest["activation_sequence"]
                .as_u64()
                .expect("generation activation sequence");
            let relative = manifest["active_database_relative_path"]
                .as_str()
                .expect("active database relative path")
                .to_owned();
            (sequence, relative)
        })
        .max_by_key(|(sequence, _)| *sequence)
        .expect("at least one committed database generation");
    root.join(relative)
}

fn provenance() -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: None,
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    }
}

fn variable(id: impl Into<String>) -> VariableRef {
    VariableRef {
        scope: VariableScope::Conversation,
        namespace: None,
        id: id.into().into(),
    }
}

fn set_variable_action(target: &VariableRef, value: i64) -> InteractionAction {
    InteractionAction::SetVariable {
        target: target.clone(),
        value: ValueExpr::Literal {
            value: VariableValue::Integer(value),
        },
    }
}

fn rule(
    id: impl Into<String>,
    event: InteractionEvent,
    actions: Vec<InteractionAction>,
) -> InteractionRule {
    let id = id.into();
    InteractionRule {
        id: InteractionRuleId::from(id),
        name: "Synthetic derived-event rule".to_owned(),
        enabled: true,
        imported_author_enabled: false,
        event,
        condition: None,
        actions,
        priority: 0,
        stop_after_match: false,
        provenance: provenance(),
    }
}

fn empty_state(revision: u64) -> InteractionState {
    InteractionState {
        variables: VariableMap::default(),
        manually_active_knowledge: Vec::new(),
        proposals: Vec::new(),
        revision,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn fixture(rules: Vec<InteractionRule>) -> Fixture {
    let root = tempdir().expect("create synthetic data root");
    let source_bytes = b"synthetic derived-event character";
    let mut staged = NamedTempFile::new_in(root.path()).expect("create staged character");
    staged
        .write_all(source_bytes)
        .expect("write staged character");
    let character = Character::new(
        "Derived Event Test",
        "Synthetic fixture",
        sha256_hex(source_bytes),
    );
    let storage = Storage::open(root.path()).expect("open synthetic storage");
    storage
        .commit_character_import(
            staged.path(),
            &character,
            u64::try_from(source_bytes.len()).expect("source length"),
            &Uuid::new_v4().to_string(),
            &[],
        )
        .expect("commit synthetic character");
    let conversation = Conversation::new(&character.id, "Derived Event Test");
    let (_, conversation_state) = storage
        .save_conversation_with_mode(&conversation, ConversationMode::Chat)
        .expect("save synthetic conversation");
    let key =
        interaction_state_key_for_branch(&conversation.id, &conversation_state.active_branch_id)
            .expect("derive interaction state key");
    storage
        .get_or_init_interaction_state(&key, &empty_state(0), &[], Utc::now())
        .expect("initialize interaction state");

    let rule_set_id = InteractionRuleSetId::from("derived-event-storage-rules");
    let stored = storage
        .save_interaction_rule_set(
            &InteractionRuleSet {
                id: rule_set_id.clone(),
                name: "Derived event storage rules".to_owned(),
                schema_version: 1,
                rules: rules.clone(),
                max_actions_per_event: 1_024,
                provenance: provenance(),
            },
            None,
        )
        .expect("save synthetic interaction rules");
    let revision_id = stored.revision_id.expect("immutable rule-set revision");
    let connection = Connection::open(active_database_path(root.path()))
        .expect("open policy evidence connection");
    let revision_sha256 = connection
        .query_row(
            "SELECT document_sha256
             FROM content_revisions
             WHERE id = ?1 AND object_id = ?2",
            params![revision_id, rule_set_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("read immutable rule-set digest");
    drop(connection);
    let policy = InteractionPolicySnapshot {
        module_plan_sha256: None,
        rule_sets: vec![InteractionPolicyRuleSetRevision {
            rule_set_id,
            revision_id: revision_id.clone(),
            sha256: revision_sha256,
        }],
    };

    Fixture {
        root,
        storage,
        key,
        policy,
        revision_id,
        rules,
    }
}

impl Fixture {
    fn database_path(&self) -> std::path::PathBuf {
        active_database_path(self.root.path())
    }

    fn action(&self, rule_id: &InteractionRuleId, action_ordinal: u32) -> &InteractionAction {
        self.rules
            .iter()
            .find(|rule| &rule.id == rule_id)
            .and_then(|rule| {
                usize::try_from(action_ordinal)
                    .ok()
                    .and_then(|ordinal| rule.actions.get(ordinal))
            })
            .expect("fixture action authority")
    }

    fn evaluation_seal(&self, event_epoch_seconds: i64) -> InteractionEvaluationSeal {
        InteractionEvaluationSeal {
            schema_version: 1,
            engine_contract_version: 1,
            policy_sha256: Sha256Digest::parse(
                interaction_policy_sha256(&self.policy).expect("hash fixture policy"),
            )
            .expect("parse fixture policy hash"),
            executable_rule_sets_sha256: Sha256Digest::parse(sha256_hex(
                b"synthetic derived-event executable policy",
            ))
            .expect("parse executable policy hash"),
            knowledge_revisions: Vec::new(),
            asset_action_diagnostics: Vec::new(),
            approved_import_source_ids: Vec::new(),
            policy_variables: VariableMap::default(),
            supported_capabilities: Vec::new(),
            template_values: InteractionEvaluationTemplateValues {
                character_name: None,
                user_name: None,
                persona_name: None,
                persona_description: None,
                current_date: None,
                current_time: None,
            },
            event_epoch_seconds,
            limits: InteractionEvaluationLimits {
                max_rule_sets: 1_024,
                max_rules: 1_024,
                max_actions_per_event: 1_024,
                max_actions_per_rule: 1_024,
                max_condition_depth: 32,
                max_condition_nodes: 4_096,
                max_template_depth: 32,
                max_template_parts: 4_096,
                max_variables: 4_096,
                max_proposals: 1_024,
                max_pending_proposals: 1_024,
                max_effects: 1_024,
                max_choices: 1_024,
                max_dice_count: 1_024,
                max_dice_sides: 1_000_000,
                max_text_chars: 1_048_576,
                max_identifier_bytes: 1_024,
            },
            seed_contract_version: 1,
        }
    }

    fn artifacts(
        &self,
        changes: &[VariableChange],
    ) -> (
        Vec<InteractionActionResultWrite>,
        Vec<InteractionEffect>,
        Vec<InteractionDerivedEventWrite>,
    ) {
        let mut action_results = Vec::with_capacity(changes.len());
        let mut effects = Vec::with_capacity(changes.len());
        let mut derived_events = Vec::with_capacity(changes.len());
        for (effect_ordinal, change) in changes.iter().enumerate() {
            let action = self.action(&change.rule_id, change.action_ordinal);
            action_results.push(InteractionActionResultWrite {
                set_revision_id: self.revision_id.clone(),
                rule_id: change.rule_id.clone(),
                action_ordinal: change.action_ordinal,
                status: InteractionActionResultStatus::Applied,
                result: VersionedJson {
                    schema_version: 1,
                    value: json!({"status": "applied"}),
                },
            });
            effects.push(InteractionEffect::VariableSet {
                target: change.target.clone(),
                previous: change.previous.clone(),
                value: change.value.clone(),
            });
            derived_events.push(InteractionDerivedEventWrite {
                event: InteractionEvent::VariableChanged {
                    variable: change.target.clone(),
                },
                deterministic_seed: u64::MAX
                    - u64::try_from(effect_ordinal).expect("effect ordinal fits u64"),
                source_set_revision_id: self.revision_id.clone(),
                source_rule_id: change.rule_id.clone(),
                source_action_ordinal: change.action_ordinal,
                source_effect_ordinal: u32::try_from(effect_ordinal)
                    .expect("effect ordinal fits u32"),
                source_action_sha256: interaction_action_sha256(action)
                    .expect("hash source action"),
            });
        }
        (action_results, effects, derived_events)
    }

    fn event_commit(
        &self,
        event_id: &str,
        event: InteractionEvent,
        expected_state_revision: u64,
        next_state: InteractionState,
        changes: &[VariableChange],
        created_at: DateTime<Utc>,
    ) -> InteractionEventCommit {
        let (action_results, effects, derived_events) = self.artifacts(changes);
        InteractionEventCommit {
            event_id: event_id.to_owned(),
            idempotency_key: format!("{event_id}-idempotency"),
            key: self.key.clone(),
            expected_state_revision,
            event,
            generation_attempt_id: None,
            owner_message_id: None,
            policy: self.policy.clone(),
            evaluation_seal: Some(self.evaluation_seal(created_at.timestamp())),
            deterministic_seed: Some(expected_state_revision),
            next_state,
            knowledge: Vec::new(),
            action_results,
            effects,
            derived_events,
            proposals: Vec::new(),
            created_at,
        }
    }

    fn occurrence_commit(
        &self,
        occurrence_id: &str,
        delivery_attempts: u64,
        expected_state_revision: u64,
        next_state: InteractionState,
        changes: &[VariableChange],
        committed_at: DateTime<Utc>,
    ) -> InteractionDerivedOccurrenceCommit {
        let (action_results, effects, derived_events) = self.artifacts(changes);
        InteractionDerivedOccurrenceCommit {
            occurrence_id: occurrence_id.to_owned(),
            expected_delivery_attempts: delivery_attempts,
            key: self.key.clone(),
            expected_state_revision,
            next_state,
            knowledge: Vec::new(),
            action_results,
            effects,
            derived_events,
            proposals: Vec::new(),
            committed_at,
        }
    }
}

fn state_with_changes(
    previous: &InteractionState,
    revision: u64,
    changes: &[VariableChange],
) -> InteractionState {
    let mut next = previous.clone();
    for change in changes {
        next.variables
            .insert(change.target.clone(), change.value.clone());
    }
    next.revision = revision;
    next
}

fn scalar_count(connection: &Connection, sql: &str) -> u64 {
    connection
        .query_row(sql, [], |row| row.get::<_, u64>(0))
        .expect("read synthetic row count")
}

fn assert_guard_evidence(
    fixture: &Fixture,
    guard_kind: &str,
    expected_outbox: u64,
    expected_acknowledged: u64,
    expected_suppressed: u64,
) {
    let connection = Connection::open(fixture.database_path()).expect("inspect guard evidence");
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_outbox"
        ),
        expected_outbox
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_outbox
             WHERE status = 'acknowledged'"
        ),
        expected_acknowledged
    );
    let suppressed = connection
        .query_row(
            "SELECT COALESCE(SUM(suppressed_count), 0)
             FROM interaction_derived_event_guard_audit
             WHERE guard_kind = ?1",
            [guard_kind],
            |row| row.get::<_, u64>(0),
        )
        .expect("read guard suppression count");
    assert_eq!(suppressed, expected_suppressed);
}

fn assert_no_interaction_transition_was_written(fixture: &Fixture) {
    let connection = Connection::open(fixture.database_path()).expect("inspect rollback");
    assert_eq!(
        connection
            .query_row(
                "SELECT revision FROM interaction_state WHERE id = ?1",
                [&fixture.key.state_id],
                |row| row.get::<_, u64>(0),
            )
            .expect("read state revision"),
        0
    );
    assert_eq!(
        scalar_count(&connection, "SELECT COUNT(*) FROM interaction_events"),
        0
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_outbox"
        ),
        0
    );
}

fn assert_exact_enqueue_evidence_and_tamper_guards(
    fixture: &Fixture,
    commit: &InteractionEventCommit,
    root_rule: &InteractionRule,
    stored: &StoredInteractionEvent,
) {
    let connection = Connection::open(fixture.database_path()).expect("inspect durable evidence");
    let evidence = connection
        .query_row(
            "SELECT outbox.status, outbox.parent_event_id, outbox.source_rule_id,
                    outbox.source_action_ordinal, outbox.source_effect_ordinal,
                    outbox.source_action_sha256, outbox.parent_event_commit_sha256,
                    parent.payload_json, outbox.evaluation_seal_version,
                    outbox.deterministic_seed_hex,
                    outbox.evaluation_seal_sha256,
                    parent.evaluation_seal_sha256,
                    parent.evaluation_seal_version
             FROM interaction_derived_event_outbox AS outbox
             JOIN interaction_events AS parent ON parent.id = outbox.parent_event_id",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, u32>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, u32>(12)?,
                ))
            },
        )
        .expect("read joined derived authority");
    let parent_payload: Value = serde_json::from_str(&evidence.7).expect("decode parent evidence");
    assert_eq!(evidence.0, "pending");
    assert_eq!(evidence.1, commit.event_id);
    assert_eq!(evidence.2, root_rule.id.as_str());
    assert_eq!(evidence.3, 0);
    assert_eq!(evidence.4, 0);
    assert_eq!(
        evidence.5,
        interaction_action_sha256(&root_rule.actions[0])
            .expect("hash expected action")
            .as_str()
    );
    assert_eq!(evidence.6, stored.commit_sha256);
    assert_eq!(evidence.8, 1);
    assert_eq!(evidence.9, "ffffffffffffffff");
    assert_eq!(evidence.10, evidence.11);
    assert_eq!(evidence.12, 1);
    assert_eq!(
        parent_payload["commit_sha256"].as_str(),
        Some(evidence.6.as_str())
    );
    assert_eq!(
        parent_payload["evaluation_seal_sha256"].as_str(),
        Some(evidence.10.as_str())
    );

    connection
        .execute(
            "UPDATE interaction_derived_event_outbox SET event_sha256 = ?1",
            ["f".repeat(64)],
        )
        .expect_err("immutable occurrence evidence must reject SQL tamper");
    connection
        .execute(
            "UPDATE interaction_derived_event_outbox SET source_effect_sha256 = ?1",
            ["e".repeat(64)],
        )
        .expect_err("immutable source-effect evidence must reject SQL tamper");
    let untouched = connection
        .query_row(
            "SELECT event_sha256, source_effect_sha256
             FROM interaction_derived_event_outbox",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("read untampered occurrence evidence");
    assert_ne!(untouched.0, "f".repeat(64));
    assert_ne!(untouched.1, "e".repeat(64));
}

fn exercise_expiry_retry_and_stale_token(
    fixture: &Fixture,
    root_state: &InteractionState,
    now: DateTime<Utc>,
) -> StoredInteractionDerivedEvent {
    let first = fixture
        .storage
        .claim_interaction_derived_events(now, now + Duration::seconds(30), 1)
        .expect("claim first delivery")
        .pop()
        .expect("first derived occurrence");
    assert_eq!(first.delivery_attempts, 1);
    assert_eq!(first.deterministic_seed, u64::MAX);
    assert!(
        fixture
            .storage
            .claim_interaction_derived_events(
                now + Duration::seconds(29),
                now + Duration::seconds(59),
                1,
            )
            .expect("claim before lease expiry")
            .is_empty()
    );
    let second = fixture
        .storage
        .claim_interaction_derived_events(
            now + Duration::seconds(30),
            now + Duration::seconds(60),
            1,
        )
        .expect("reclaim expired lease")
        .pop()
        .expect("expired occurrence");
    assert_eq!(second.occurrence_id, first.occurrence_id);
    assert_eq!(second.delivery_attempts, 2);
    assert_eq!(second.deterministic_seed, first.deterministic_seed);
    assert_eq!(second.evaluation_seal, first.evaluation_seal);
    assert_eq!(second.evaluation_seal_sha256, first.evaluation_seal_sha256);

    let mut stale_state = root_state.clone();
    stale_state.revision = 2;
    let stale_error = fixture
        .storage
        .commit_interaction_derived_occurrence(&InteractionDerivedOccurrenceCommit {
            occurrence_id: second.occurrence_id.clone(),
            expected_delivery_attempts: first.delivery_attempts,
            key: fixture.key.clone(),
            expected_state_revision: 1,
            next_state: stale_state,
            knowledge: Vec::new(),
            action_results: Vec::new(),
            effects: Vec::new(),
            derived_events: Vec::new(),
            proposals: Vec::new(),
            committed_at: now + Duration::seconds(30),
        })
        .expect_err("stale delivery token must not mutate state");
    assert_eq!(stale_error.code, CoreErrorCode::InvalidInput);
    assert!(stale_error.recoverable);

    let retry_at = now + Duration::seconds(90);
    fixture
        .storage
        .retry_interaction_derived_event_after(
            &second.occurrence_id,
            second.delivery_attempts,
            retry_at,
        )
        .expect("defer reclaimed delivery");
    assert!(
        fixture
            .storage
            .claim_interaction_derived_events(
                retry_at - Duration::milliseconds(1),
                retry_at + Duration::seconds(30),
                1,
            )
            .expect("claim before retry deadline")
            .is_empty()
    );
    let third = fixture
        .storage
        .claim_interaction_derived_events(retry_at, retry_at + Duration::seconds(30), 1)
        .expect("claim retry delivery")
        .pop()
        .expect("retry occurrence");
    assert_eq!(third.occurrence_id, first.occurrence_id);
    assert_eq!(third.delivery_attempts, 3);
    assert_eq!(third.deterministic_seed, first.deterministic_seed);
    assert_eq!(third.evaluation_seal_sha256, first.evaluation_seal_sha256);
    first
}

fn assert_acknowledged_materialization_and_replay(
    storage: &Storage,
    database_path: &std::path::Path,
    key: &InteractionStateKey,
    root_state: &InteractionState,
    recovered: &StoredInteractionDerivedEvent,
    recovered_at: DateTime<Utc>,
) {
    let mut materialized_state = root_state.clone();
    materialized_state.revision = 2;
    let materialization = InteractionDerivedOccurrenceCommit {
        occurrence_id: recovered.occurrence_id.clone(),
        expected_delivery_attempts: recovered.delivery_attempts,
        key: key.clone(),
        expected_state_revision: 1,
        next_state: materialized_state,
        knowledge: Vec::new(),
        action_results: Vec::new(),
        effects: Vec::new(),
        derived_events: Vec::new(),
        proposals: Vec::new(),
        committed_at: recovered_at,
    };
    let committed = storage
        .commit_interaction_derived_occurrence(&materialization)
        .expect("materialize and acknowledge occurrence");
    assert!(!committed.exact_replay);
    let replay = storage
        .commit_interaction_derived_occurrence(&materialization)
        .expect("recover response-lost acknowledgement");
    assert!(replay.exact_replay);
    assert_eq!(replay.event_id, committed.event_id);
    assert_eq!(replay.commit_sha256, committed.commit_sha256);
    let mut mismatched_replay = materialization.clone();
    mismatched_replay.next_state.variables.insert(
        variable("response-loss-retarget"),
        VariableValue::Integer(99),
    );
    let mismatch_error = storage
        .commit_interaction_derived_occurrence(&mismatched_replay)
        .expect_err("acknowledged replay must match its exact committed materialization");
    assert_eq!(mismatch_error.code, CoreErrorCode::InvalidInput);
    assert!(
        storage
            .claim_interaction_derived_events(
                recovered_at + Duration::seconds(31),
                recovered_at + Duration::seconds(61),
                1,
            )
            .expect("claim after acknowledgement")
            .is_empty()
    );

    let connection = Connection::open(database_path).expect("inspect acknowledged occurrence");
    let (status, attempts, acknowledged_at) = connection
        .query_row(
            "SELECT status, delivery_attempts, acknowledged_at
             FROM interaction_derived_event_outbox",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .expect("read acknowledged occurrence");
    assert_eq!(status, "acknowledged");
    assert_eq!(attempts, 4);
    assert!(acknowledged_at.is_some());
    assert_eq!(
        scalar_count(&connection, "SELECT COUNT(*) FROM interaction_events"),
        2
    );
}

fn commit_and_claim_root(
    fixture: &Fixture,
    root_rule: &InteractionRule,
    target: &VariableRef,
    event_id: &str,
    now: DateTime<Utc>,
) -> (InteractionState, StoredInteractionDerivedEvent) {
    let root_change = VariableChange {
        rule_id: root_rule.id.clone(),
        action_ordinal: 0,
        target: target.clone(),
        previous: None,
        value: VariableValue::Integer(1),
    };
    let root_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&root_change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            event_id,
            InteractionEvent::ConversationOpened,
            0,
            root_state.clone(),
            std::slice::from_ref(&root_change),
            now,
        ))
        .expect("commit claimed-root parent");
    let occurrence = fixture
        .storage
        .claim_interaction_derived_events(now, now + Duration::seconds(30), 1)
        .expect("claim root derived occurrence")
        .pop()
        .expect("root derived occurrence");
    (root_state, occurrence)
}

#[test]
fn derived_enqueue_is_atomic_and_immutable_with_exact_source_evidence() {
    let target = variable("atomic-target");
    let root_rule = rule(
        "atomic-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let fixture = fixture(vec![root_rule.clone()]);
    let change = VariableChange {
        rule_id: root_rule.id.clone(),
        action_ordinal: 0,
        target: target.clone(),
        previous: None,
        value: VariableValue::Integer(1),
    };
    let created_at = Utc::now();
    let next_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&change));
    let commit = fixture.event_commit(
        "atomic-root-event",
        InteractionEvent::ConversationOpened,
        0,
        next_state,
        std::slice::from_ref(&change),
        created_at,
    );

    let mut invalid = commit.clone();
    invalid.derived_events[0].source_action_sha256 =
        Sha256Digest::parse("0".repeat(64)).expect("syntactic test digest");
    let error = fixture
        .storage
        .commit_interaction_event(&invalid)
        .expect_err("stale action authority must reject the whole transition");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_no_interaction_transition_was_written(&fixture);

    let stored = fixture
        .storage
        .commit_interaction_event(&commit)
        .expect("atomically commit event and derived occurrence");
    assert_exact_enqueue_evidence_and_tamper_guards(&fixture, &commit, &root_rule, &stored);
}

#[test]
fn claim_retry_reopen_ack_and_response_replay_preserve_one_occurrence() {
    let target = variable("restart-target");
    let root_rule = rule(
        "restart-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let fixture = fixture(vec![root_rule.clone()]);
    let root_change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target,
        previous: None,
        value: VariableValue::Integer(1),
    };
    let now = Utc::now();
    let root_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&root_change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "restart-root-event",
            InteractionEvent::ConversationOpened,
            0,
            root_state.clone(),
            std::slice::from_ref(&root_change),
            now,
        ))
        .expect("commit root event");

    let first = exercise_expiry_retry_and_stale_token(&fixture, &root_state, now);

    let Fixture {
        root, storage, key, ..
    } = fixture;
    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen and recover abandoned lease");
    let recovered_at = Utc::now() + Duration::seconds(1);
    let recovered = reopened
        .claim_interaction_derived_events(recovered_at, recovered_at + Duration::seconds(30), 1)
        .expect("claim recovered occurrence")
        .pop()
        .expect("recovered occurrence");
    assert_eq!(recovered.occurrence_id, first.occurrence_id);
    assert_eq!(recovered.delivery_attempts, 4);
    assert_eq!(recovered.event, first.event);
    assert_eq!(recovered.deterministic_seed, first.deterministic_seed);
    assert_eq!(recovered.evaluation_seal, first.evaluation_seal);
    assert_eq!(
        recovered.evaluation_seal_sha256,
        first.evaluation_seal_sha256
    );
    assert_acknowledged_materialization_and_replay(
        &reopened,
        &active_database_path(root.path()),
        &key,
        &root_state,
        &recovered,
        recovered_at,
    );
}

#[test]
fn quarantined_authority_failure_replays_exactly_and_is_not_reclaimed_after_reopen() {
    let target = variable("quarantine-replay-target");
    let root_rule = rule(
        "quarantine-replay-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let fixture = fixture(vec![root_rule.clone()]);
    let now = Utc::now();
    let (_, occurrence) = commit_and_claim_root(
        &fixture,
        &root_rule,
        &target,
        "quarantine-replay-root-event",
        now,
    );
    let active_policy = InteractionPolicySnapshot {
        module_plan_sha256: None,
        rule_sets: Vec::new(),
    };
    let first = fixture
        .storage
        .quarantine_interaction_derived_event_authority_failure(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            Some(&active_policy),
            now + Duration::seconds(1),
        )
        .expect("quarantine terminal authority failure");
    assert!(!first.exact_replay);
    assert_eq!(first.delivery_attempts, occurrence.delivery_attempts);
    assert_eq!(first.sealed_policy_sha256, occurrence.policy_sha256);
    assert_eq!(first.source_effect_sha256, occurrence.source_effect_sha256);
    assert_eq!(first.source_action_sha256, occurrence.source_action_sha256);

    let Fixture { root, storage, .. } = fixture;
    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen quarantined storage");
    let replay = reopened
        .quarantine_interaction_derived_event_authority_failure(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            Some(&active_policy),
            now + Duration::seconds(30),
        )
        .expect("recover response-lost quarantine");
    assert!(replay.exact_replay);
    assert_eq!(replay.evidence_sha256, first.evidence_sha256);
    let mismatch = reopened
        .quarantine_interaction_derived_event_authority_failure(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            None,
            now + Duration::seconds(30),
        )
        .expect_err("quarantine replay evidence cannot change");
    assert_eq!(mismatch.code, CoreErrorCode::InvalidInput);
    assert!(
        reopened
            .claim_interaction_derived_events(
                now + Duration::hours(1),
                now + Duration::hours(1) + Duration::seconds(30),
                1,
            )
            .expect("claim after quarantine recovery")
            .is_empty()
    );

    let connection =
        Connection::open(active_database_path(root.path())).expect("inspect immutable quarantine");
    connection
        .execute(
            "UPDATE interaction_derived_event_quarantines SET evidence_sha256 = ?1",
            ["f".repeat(64)],
        )
        .expect_err("quarantine evidence is immutable");
    connection
        .execute("DELETE FROM interaction_derived_event_quarantines", [])
        .expect_err("quarantine evidence cannot be deleted");
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_quarantines"
        ),
        1
    );
}

#[test]
fn terminal_quarantine_unblocks_the_branch_for_a_later_root() {
    let target = variable("quarantine-unblock-target");
    let root_rule = rule(
        "quarantine-unblock-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let fixture = fixture(vec![root_rule.clone()]);
    let now = Utc::now();
    let (root_state, occurrence) = commit_and_claim_root(
        &fixture,
        &root_rule,
        &target,
        "quarantine-unblock-first-root",
        now,
    );
    fixture
        .storage
        .quarantine_interaction_derived_event_authority_failure(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            None,
            now + Duration::seconds(1),
        )
        .expect("quarantine branch predecessor");

    let successor_change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target,
        previous: Some(VariableValue::Integer(1)),
        value: VariableValue::Integer(1),
    };
    let successor_state =
        state_with_changes(&root_state, 2, std::slice::from_ref(&successor_change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "quarantine-unblock-second-root",
            InteractionEvent::ConversationOpened,
            1,
            successor_state,
            std::slice::from_ref(&successor_change),
            now + Duration::seconds(2),
        ))
        .expect("terminal quarantine must unblock the branch");
    let successor = fixture
        .storage
        .claim_interaction_derived_events(
            now + Duration::seconds(3),
            now + Duration::seconds(33),
            1,
        )
        .expect("claim successor after quarantine")
        .pop()
        .expect("successor occurrence");
    assert_eq!(successor.parent_event_id, "quarantine-unblock-second-root");
    let connection = Connection::open(fixture.database_path()).expect("inspect branch unblock");
    assert_eq!(
        scalar_count(&connection, "SELECT COUNT(*) FROM interaction_events"),
        2
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_quarantines"
        ),
        1
    );
}

#[test]
fn pending_derived_event_blocks_a_later_same_branch_root() {
    let target = variable("causal-target");
    let root_rule = rule(
        "causal-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let fixture = fixture(vec![root_rule.clone()]);
    let occurred_at = Utc::now();
    let first_change = VariableChange {
        rule_id: root_rule.id.clone(),
        action_ordinal: 0,
        target: target.clone(),
        previous: None,
        value: VariableValue::Integer(1),
    };
    let first_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&first_change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "causal-predecessor-event",
            InteractionEvent::ConversationOpened,
            0,
            first_state.clone(),
            std::slice::from_ref(&first_change),
            occurred_at,
        ))
        .expect("commit causal predecessor");
    let second_change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target,
        previous: Some(VariableValue::Integer(1)),
        value: VariableValue::Integer(2),
    };
    let second_state = state_with_changes(&first_state, 2, std::slice::from_ref(&second_change));
    let error = fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "causal-successor-event",
            InteractionEvent::ConversationOpened,
            1,
            second_state,
            std::slice::from_ref(&second_change),
            occurred_at,
        ))
        .expect_err("pending derived work must block a later same-branch root");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    let stored_state = fixture
        .storage
        .get_interaction_state(&fixture.key.conversation_id, &fixture.key.branch_id)
        .expect("read state after blocked successor");
    assert_eq!(stored_state.revision, 1);

    let claimed = fixture
        .storage
        .claim_interaction_derived_events(
            occurred_at + Duration::seconds(1),
            occurred_at + Duration::seconds(31),
            1,
        )
        .expect("claim causally earliest occurrence")
        .pop()
        .expect("causal occurrence");
    assert_eq!(claimed.parent_event_id, "causal-predecessor-event");
}

#[test]
fn tampered_parent_revision_authority_is_rejected_even_without_sql_trigger() {
    let target = variable("revision-authority-target");
    let root_rule = rule(
        "revision-authority-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let fixture = fixture(vec![root_rule.clone()]);
    let change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target,
        previous: None,
        value: VariableValue::Integer(1),
    };
    let occurred_at = Utc::now();
    let next_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "revision-authority-root-event",
            InteractionEvent::ConversationOpened,
            0,
            next_state,
            std::slice::from_ref(&change),
            occurred_at,
        ))
        .expect("commit revision-authority root");

    let connection = Connection::open(fixture.database_path()).expect("open tamper connection");
    connection
        .execute(
            "UPDATE interaction_derived_event_outbox
             SET parent_resulting_state_revision = 99",
            [],
        )
        .expect_err("identity trigger must reject parent revision tamper");
    connection
        .execute(
            "DROP TRIGGER interaction_derived_event_outbox_identity_guard",
            [],
        )
        .expect("remove test trigger to exercise read-side verification");
    connection
        .execute(
            "DROP TRIGGER interaction_derived_event_outbox_transition_guard",
            [],
        )
        .expect("remove transition trigger to inject synthetic corruption");
    connection
        .execute(
            "UPDATE interaction_derived_event_outbox
             SET parent_resulting_state_revision = 99",
            [],
        )
        .expect("inject synthetic corrupt parent revision");
    drop(connection);

    let error = fixture
        .storage
        .claim_interaction_derived_events(
            occurred_at + Duration::seconds(1),
            occurred_at + Duration::seconds(31),
            1,
        )
        .expect_err("read-side authority must reject the corrupt parent revision");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn independent_branches_can_be_claimed_in_one_batch() {
    let target = variable("parallel-target");
    let root_rule = rule(
        "parallel-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let fixture = fixture(vec![root_rule.clone()]);
    let second_branch = fixture
        .storage
        .create_conversation_branch(
            &fixture.key.conversation_id,
            None,
            Some("Independent derived branch".to_owned()),
        )
        .expect("create independent branch");
    let second_key =
        interaction_state_key_for_branch(&fixture.key.conversation_id, &second_branch.id)
            .expect("derive second interaction state key");
    fixture
        .storage
        .get_or_init_interaction_state(&second_key, &empty_state(0), &[], Utc::now())
        .expect("initialize second branch state");
    let change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target,
        previous: None,
        value: VariableValue::Integer(1),
    };
    let occurred_at = Utc::now();
    let next_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "parallel-first-root",
            InteractionEvent::ConversationOpened,
            0,
            next_state.clone(),
            std::slice::from_ref(&change),
            occurred_at,
        ))
        .expect("commit first branch root");
    let mut second_commit = fixture.event_commit(
        "parallel-second-root",
        InteractionEvent::ConversationOpened,
        0,
        next_state,
        std::slice::from_ref(&change),
        occurred_at,
    );
    second_commit.key = second_key.clone();
    fixture
        .storage
        .commit_interaction_event(&second_commit)
        .expect("commit second branch root");

    let claimed = fixture
        .storage
        .claim_interaction_derived_events(
            occurred_at + Duration::seconds(1),
            occurred_at + Duration::seconds(31),
            2,
        )
        .expect("claim independent branches");
    assert_eq!(claimed.len(), 2);
    let mut branch_ids = claimed
        .iter()
        .map(|occurrence| occurrence.branch_id.0.clone())
        .collect::<Vec<_>>();
    branch_ids.sort();
    let mut expected_branch_ids = vec![fixture.key.branch_id.0.clone(), second_key.branch_id.0];
    expected_branch_ids.sort();
    assert_eq!(branch_ids, expected_branch_ids);
}

#[test]
fn legacy_unsealed_outbox_is_terminally_quarantined_before_claim_decode() {
    let target = variable("legacy-seal-target");
    let root_rule = rule(
        "legacy-seal-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let fixture = fixture(vec![root_rule.clone()]);
    let change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target,
        previous: None,
        value: VariableValue::Integer(1),
    };
    let now = Utc::now();
    let first_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "legacy-seal-root-event",
            InteractionEvent::ConversationOpened,
            0,
            first_state.clone(),
            std::slice::from_ref(&change),
            now,
        ))
        .expect("commit sealed root before synthetic downgrade");

    let connection = Connection::open(fixture.database_path()).expect("open downgrade database");
    connection
        .execute_batch(
            "DROP TRIGGER interaction_derived_event_outbox_identity_guard;
             DROP TRIGGER interaction_derived_event_outbox_transition_guard;",
        )
        .expect("remove immutable guards for synthetic legacy downgrade");
    connection
        .execute(
            "UPDATE interaction_derived_event_outbox
             SET evaluation_seal_json = NULL,
                 evaluation_seal_sha256 = NULL,
                 evaluation_seal_version = 0,
                 deterministic_seed_hex = NULL",
            [],
        )
        .expect("downgrade outbox row to schema-35 authority");
    drop(connection);

    let claimed = fixture
        .storage
        .claim_interaction_derived_events(
            now + Duration::seconds(1),
            now + Duration::seconds(31),
            8,
        )
        .expect("terminally quarantine legacy row before decoding claims");
    assert!(claimed.is_empty());
    assert_eq!(
        fixture
            .storage
            .interaction_derived_event_supervisor_status()
            .expect("read supervisor status after quarantine")
            .pending_count,
        0
    );
    let connection = Connection::open(fixture.database_path()).expect("inspect quarantine row");
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_quarantines
             WHERE reason_kind = 'sealed_policy_recovery_failed'"
        ),
        1
    );
    drop(connection);

    let mut second_state = first_state;
    second_state.revision = 2;
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "post-legacy-quarantine-event",
            InteractionEvent::ConversationStarted,
            1,
            second_state,
            &[],
            now + Duration::seconds(2),
        ))
        .expect("terminal legacy quarantine must unblock the branch");
}

#[test]
fn repeated_derived_event_is_cycle_suppressed_after_parent_acknowledgement() {
    let target = variable("cycle-target");
    let root_rule = rule(
        "cycle-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let cycle_rule = rule(
        "cycle-child",
        InteractionEvent::VariableChanged {
            variable: target.clone(),
        },
        vec![set_variable_action(&target, 2)],
    );
    let fixture = fixture(vec![root_rule.clone(), cycle_rule.clone()]);
    let root_change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target: target.clone(),
        previous: None,
        value: VariableValue::Integer(1),
    };
    let now = Utc::now();
    let root_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&root_change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "cycle-root-event",
            InteractionEvent::ConversationOpened,
            0,
            root_state.clone(),
            std::slice::from_ref(&root_change),
            now,
        ))
        .expect("commit cycle root");
    let occurrence = fixture
        .storage
        .claim_interaction_derived_events(now, now + Duration::seconds(30), 1)
        .expect("claim cycle candidate")
        .pop()
        .expect("cycle candidate occurrence");
    let cycle_change = VariableChange {
        rule_id: cycle_rule.id,
        action_ordinal: 0,
        target,
        previous: Some(VariableValue::Integer(1)),
        value: VariableValue::Integer(2),
    };
    let cycle_state = state_with_changes(&root_state, 2, std::slice::from_ref(&cycle_change));
    fixture
        .storage
        .commit_interaction_derived_occurrence(&fixture.occurrence_commit(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            1,
            cycle_state,
            std::slice::from_ref(&cycle_change),
            now + Duration::seconds(1),
        ))
        .expect("ack parent and suppress its cycle child");

    let connection = Connection::open(fixture.database_path()).expect("inspect cycle guard");
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_outbox"
        ),
        1
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_outbox
             WHERE status = 'acknowledged'"
        ),
        1
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_guard_audit
             WHERE guard_kind = 'cycle' AND suppressed_count = 1"
        ),
        1
    );
    drop(connection);
    assert!(
        fixture
            .storage
            .claim_interaction_derived_events(
                now + Duration::seconds(31),
                now + Duration::seconds(61),
                1,
            )
            .expect("claim after cycle suppression")
            .is_empty()
    );
}

#[test]
fn duplicate_cycle_candidates_are_aggregated_without_rolling_back_parent() {
    let target = variable("duplicate-cycle-target");
    let root_rule = rule(
        "duplicate-cycle-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&target, 1)],
    );
    let cycle_rule = rule(
        "duplicate-cycle-child",
        InteractionEvent::VariableChanged {
            variable: target.clone(),
        },
        vec![
            set_variable_action(&target, 2),
            set_variable_action(&target, 3),
        ],
    );
    let fixture = fixture(vec![root_rule.clone(), cycle_rule.clone()]);
    let root_change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target: target.clone(),
        previous: None,
        value: VariableValue::Integer(1),
    };
    let now = Utc::now();
    let root_state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&root_change));
    fixture
        .storage
        .commit_interaction_event(&fixture.event_commit(
            "duplicate-cycle-root-event",
            InteractionEvent::ConversationOpened,
            0,
            root_state.clone(),
            std::slice::from_ref(&root_change),
            now,
        ))
        .expect("commit duplicate-cycle root");
    let occurrence = fixture
        .storage
        .claim_interaction_derived_events(now, now + Duration::seconds(30), 1)
        .expect("claim duplicate-cycle parent")
        .pop()
        .expect("duplicate-cycle parent occurrence");
    let cycle_changes = vec![
        VariableChange {
            rule_id: cycle_rule.id.clone(),
            action_ordinal: 0,
            target: target.clone(),
            previous: Some(VariableValue::Integer(1)),
            value: VariableValue::Integer(2),
        },
        VariableChange {
            rule_id: cycle_rule.id,
            action_ordinal: 1,
            target,
            previous: Some(VariableValue::Integer(2)),
            value: VariableValue::Integer(3),
        },
    ];
    let cycle_state = state_with_changes(&root_state, 2, &cycle_changes);
    fixture
        .storage
        .commit_interaction_derived_occurrence(&fixture.occurrence_commit(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            1,
            cycle_state,
            &cycle_changes,
            now + Duration::seconds(1),
        ))
        .expect("aggregate duplicate cycle evidence while acknowledging parent");
    assert_guard_evidence(&fixture, "cycle", 1, 1, 2);
}

#[test]
fn derived_chain_records_depth_guard_audit() {
    let variables = (0..=16)
        .map(|index| variable(format!("depth-{index:02}")))
        .collect::<Vec<_>>();
    let root_rule = rule(
        "depth-root",
        InteractionEvent::ConversationOpened,
        vec![set_variable_action(&variables[0], 1)],
    );
    let mut rules = vec![root_rule.clone()];
    for index in 0..16 {
        rules.push(rule(
            format!("depth-child-{index:02}"),
            InteractionEvent::VariableChanged {
                variable: variables[index].clone(),
            },
            vec![set_variable_action(&variables[index + 1], 1)],
        ));
    }
    let depth_fixture = fixture(rules.clone());
    let root_change = VariableChange {
        rule_id: root_rule.id,
        action_ordinal: 0,
        target: variables[0].clone(),
        previous: None,
        value: VariableValue::Integer(1),
    };
    let now = Utc::now();
    let mut state = state_with_changes(&empty_state(0), 1, std::slice::from_ref(&root_change));
    depth_fixture
        .storage
        .commit_interaction_event(&depth_fixture.event_commit(
            "depth-root-event",
            InteractionEvent::ConversationOpened,
            0,
            state.clone(),
            std::slice::from_ref(&root_change),
            now,
        ))
        .expect("commit depth root");
    for index in 0..16 {
        let claim_at = now + Duration::seconds(i64::try_from(index + 1).expect("claim time"));
        let occurrence = depth_fixture
            .storage
            .claim_interaction_derived_events(claim_at, claim_at + Duration::seconds(30), 1)
            .expect("claim bounded depth occurrence")
            .pop()
            .expect("bounded depth occurrence");
        assert_eq!(
            occurrence.depth,
            u32::try_from(index + 1).expect("depth fits u32")
        );
        assert_eq!(
            occurrence.event,
            InteractionEvent::VariableChanged {
                variable: variables[index].clone(),
            }
        );
        let child_change = VariableChange {
            rule_id: rules[index + 1].id.clone(),
            action_ordinal: 0,
            target: variables[index + 1].clone(),
            previous: None,
            value: VariableValue::Integer(1),
        };
        let next_revision = state.revision + 1;
        state = state_with_changes(&state, next_revision, std::slice::from_ref(&child_change));
        depth_fixture
            .storage
            .commit_interaction_derived_occurrence(&depth_fixture.occurrence_commit(
                &occurrence.occurrence_id,
                occurrence.delivery_attempts,
                next_revision - 1,
                state.clone(),
                std::slice::from_ref(&child_change),
                claim_at,
            ))
            .expect("ack occurrence and bound its child depth");
    }
    assert_guard_evidence(&depth_fixture, "depth_limit", 16, 16, 1);
}

#[test]
fn derived_chain_records_count_guard_audit() {
    let count_variables = (0..257)
        .map(|index| variable(format!("count-{index:03}")))
        .collect::<Vec<_>>();
    let count_actions = count_variables
        .iter()
        .map(|target| set_variable_action(target, 1))
        .collect::<Vec<_>>();
    let count_rule = rule(
        "count-root",
        InteractionEvent::ConversationOpened,
        count_actions,
    );
    let count_fixture = fixture(vec![count_rule.clone()]);
    let count_changes = count_variables
        .iter()
        .enumerate()
        .map(|(ordinal, target)| VariableChange {
            rule_id: count_rule.id.clone(),
            action_ordinal: u32::try_from(ordinal).expect("action ordinal fits u32"),
            target: target.clone(),
            previous: None,
            value: VariableValue::Integer(1),
        })
        .collect::<Vec<_>>();
    let count_state = state_with_changes(&empty_state(0), 1, &count_changes);
    let now = Utc::now();
    count_fixture
        .storage
        .commit_interaction_event(&count_fixture.event_commit(
            "count-root-event",
            InteractionEvent::ConversationOpened,
            0,
            count_state,
            &count_changes,
            now,
        ))
        .expect("commit count-bounded root");
    assert_guard_evidence(&count_fixture, "count_limit", 256, 0, 1);
}
