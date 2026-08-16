//! Restart liveness and fail-closed evidence checks for derived interactions.

use std::{
    io::Write,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, Utc};
use lorepia_core::{
    ContentModuleActivationRequest, ContentModuleBindingDraft, ContentModuleRuntimeTarget, Core,
    CoreConfig, CoreErrorCode, CoreLifecycleDeliveryStatus, ModuleActivationApproval,
    ModuleMergeResolutionSet,
};
use lorepia_domain::{
    ContentCapability, ContentModule, ContentModuleId, ConversationBranchId, ConversationId,
    ConversationMode, InteractionAction, InteractionEffect, InteractionEvent, InteractionRule,
    InteractionRuleId, InteractionRuleSet, InteractionRuleSetId, ModuleBindingId,
    ModuleRevisionResolutionMode, ModuleScope, PackageMetadata, Provenance, SourceKind, ValueExpr,
    VariableId, VariableMap, VariableRef, VariableScope, VariableValue, VersionedJson,
};
use lorepia_storage::{
    InteractionActionResultStatus, InteractionActionResultWrite, InteractionDerivedEventWrite,
    InteractionEvaluationSeal, InteractionEventCommit, InteractionPolicySnapshot, Storage,
    interaction_action_sha256,
};
use rusqlite::{Connection, params};
use serde_json::json;
use tempfile::{NamedTempFile, TempDir, tempdir};

const ROOT_ACTION_ID: &str = "synthetic-derived-supervisor-root";
const RULE_SET_ID: &str = "synthetic.core.derived-supervisor.rules";
const RULE_ID: &str = "synthetic.core.derived-supervisor.root";

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = std::fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
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

#[derive(Clone)]
struct BranchKey {
    conversation_id: ConversationId,
    branch_id: ConversationBranchId,
}

struct SeededBacklog {
    root: TempDir,
    storage: Storage,
    branches: Vec<BranchKey>,
}

impl SeededBacklog {
    fn database_path(&self) -> PathBuf {
        active_database_path(self.root.path())
    }
}

fn provenance(source_id: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic Test Author".to_owned()),
        license: Some("LicenseRef-Synthetic".to_owned()),
        imported_at: None,
    }
}

fn import_synthetic_character(core: &Core) -> String {
    let mut source = NamedTempFile::new().expect("create synthetic character source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Ari","description":"Synthetic derived supervisor fixture."}}}}"#
    )
    .expect("write synthetic character source");
    let review = core
        .inspect_import(source.path())
        .expect("inspect synthetic character");
    core.commit_import(&review.id)
        .expect("commit synthetic character")
        .id
}

fn variable(index: usize) -> VariableRef {
    VariableRef {
        scope: VariableScope::Branch,
        namespace: None,
        id: VariableId::from(format!("synthetic_supervisor_{index:03}")),
    }
}

fn interaction_rule_set(action_count: usize) -> (InteractionRuleSet, Vec<VariableRef>) {
    let variables = (0..action_count).map(variable).collect::<Vec<_>>();
    let actions = variables
        .iter()
        .map(|target| InteractionAction::SetVariable {
            target: target.clone(),
            value: ValueExpr::Literal {
                value: VariableValue::Integer(1),
            },
        })
        .collect();
    (
        InteractionRuleSet {
            id: InteractionRuleSetId::from(RULE_SET_ID),
            name: "Synthetic derived supervisor rules".to_owned(),
            schema_version: 1,
            rules: vec![InteractionRule {
                id: InteractionRuleId::from(RULE_ID),
                name: "Emit state-derived occurrences".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::UserAction {
                    action_id: ROOT_ACTION_ID.to_owned(),
                },
                condition: None,
                actions,
                priority: 0,
                stop_after_match: false,
                provenance: provenance(RULE_ID),
            }],
            max_actions_per_event: 16,
            provenance: provenance(RULE_SET_ID),
        },
        variables,
    )
}

fn interaction_module(rule_set_id: &InteractionRuleSetId) -> ContentModule {
    ContentModule {
        id: ContentModuleId::from("synthetic.core.derived-supervisor.module"),
        name: "Synthetic derived supervisor module".to_owned(),
        version: "1.0.0".to_owned(),
        schema_version: 1,
        prompt_fragments: Vec::new(),
        knowledge_book_ids: Vec::new(),
        control_specs: Vec::new(),
        transform_set_ids: Vec::new(),
        interaction_rule_set_ids: vec![rule_set_id.clone()],
        asset_ids: Vec::new(),
        imported_components_enabled: true,
        required_capabilities: vec![ContentCapability::DeclarativeInteractions],
        metadata: PackageMetadata {
            author: Some("Synthetic Test Author".to_owned()),
            license: "LicenseRef-Unknown".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: "Local-only derived supervisor acceptance fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: provenance("synthetic.core.derived-supervisor.module"),
        },
    }
}

fn activate_app_module(core: &Core, module: &ContentModule, target: &BranchKey) {
    core.upsert_content_module(module, None)
        .expect("save synthetic interaction module");
    let request = ContentModuleActivationRequest {
        runtime_target: ContentModuleRuntimeTarget {
            conversation_id: target.conversation_id.clone(),
            branch_id: target.branch_id.clone(),
        },
        expected_binding_revision: None,
        binding: ContentModuleBindingDraft {
            id: ModuleBindingId::from("synthetic.core.derived-supervisor.binding"),
            module_id: module.id.clone(),
            scope: ModuleScope::App,
            target_id: None,
            conversation_id: None,
            priority: 0,
            resolution_mode: ModuleRevisionResolutionMode::Active,
            pinned_revision_id: None,
            package_import_approval_id: None,
            variable_overrides: VariableMap::default(),
        },
    };
    let review = core
        .review_content_module_activation(&request)
        .expect("review synthetic module activation");
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let plan = core
        .resolve_content_module_activation(&request, &resolutions)
        .expect("resolve synthetic module activation");
    core.activate_content_module(
        &request,
        &resolutions,
        &ModuleActivationApproval {
            approval_id: "synthetic-derived-supervisor-approval".to_owned(),
            expected_review_sha256: review.review_sha256,
            expected_plan_sha256: plan.plan_sha256,
        },
    )
    .expect("activate synthetic interaction module")
    .verify()
    .expect("verify synthetic module activation receipt");
}

fn create_branch(core: &Core, character_id: &str, index: usize) -> BranchKey {
    let conversation = core
        .create_conversation(
            character_id,
            format!("Synthetic derived supervisor room {index:03}"),
            ConversationMode::Chat,
        )
        .expect("create synthetic conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list synthetic conversation branches")
        .into_iter()
        .next()
        .expect("synthetic root branch");
    BranchKey {
        conversation_id: conversation.id,
        branch_id: branch.id,
    }
}

fn read_policy(connection: &Connection, key: &BranchKey) -> InteractionPolicySnapshot {
    let policy_json = connection
        .query_row(
            "SELECT policy_json
             FROM interaction_events
             WHERE conversation_id = ?1 AND branch_id = ?2
             ORDER BY resulting_state_revision DESC
             LIMIT 1",
            params![key.conversation_id.0, key.branch_id.0],
            |row| row.get::<_, String>(0),
        )
        .expect("read sealed branch interaction policy");
    serde_json::from_str(&policy_json).expect("decode sealed branch interaction policy")
}

fn read_evaluation_seal(connection: &Connection, key: &BranchKey) -> InteractionEvaluationSeal {
    let seal_json = connection
        .query_row(
            "SELECT evaluation_seal_json
             FROM interaction_events
             WHERE conversation_id = ?1 AND branch_id = ?2
               AND evaluation_seal_version = 1
             ORDER BY resulting_state_revision DESC
             LIMIT 1",
            params![key.conversation_id.0, key.branch_id.0],
            |row| row.get::<_, String>(0),
        )
        .expect("read sealed branch interaction evaluation context");
    serde_json::from_str(&seal_json).expect("decode sealed branch interaction evaluation context")
}

fn seed_root_event(
    storage: &Storage,
    connection: &Connection,
    key: &BranchKey,
    root_index: usize,
    revision_id: &str,
    rule: &InteractionRule,
    variables: &[VariableRef],
) {
    let snapshot = storage
        .get_interaction_state_snapshot(&key.conversation_id, &key.branch_id)
        .expect("read initialized branch interaction state");
    let mut next_state = snapshot.state.clone();
    let mut action_results = Vec::with_capacity(variables.len());
    let mut effects = Vec::with_capacity(variables.len());
    let mut derived_events = Vec::with_capacity(variables.len());
    for (ordinal, target) in variables.iter().enumerate() {
        assert_eq!(next_state.variables.get(target), None);
        next_state
            .variables
            .insert(target.clone(), VariableValue::Integer(1));
        let action_ordinal = u32::try_from(ordinal).expect("action ordinal fits u32");
        let action = rule.actions.get(ordinal).expect("fixture action exists");
        action_results.push(InteractionActionResultWrite {
            set_revision_id: revision_id.to_owned(),
            rule_id: rule.id.clone(),
            action_ordinal,
            status: InteractionActionResultStatus::Applied,
            result: VersionedJson {
                schema_version: 1,
                value: json!({
                    "rule_status": "applied",
                    "state_changed": true,
                    "effect_count": variables.len(),
                }),
            },
        });
        effects.push(InteractionEffect::VariableSet {
            target: target.clone(),
            previous: None,
            value: VariableValue::Integer(1),
        });
        derived_events.push(InteractionDerivedEventWrite {
            event: InteractionEvent::VariableChanged {
                variable: target.clone(),
            },
            source_set_revision_id: revision_id.to_owned(),
            source_rule_id: rule.id.clone(),
            source_action_ordinal: action_ordinal,
            source_effect_ordinal: action_ordinal,
            source_action_sha256: interaction_action_sha256(action)
                .expect("hash synthetic source action"),
            deterministic_seed: u64::try_from(root_index)
                .expect("root index fits u64")
                .wrapping_shl(32)
                | u64::from(action_ordinal),
        });
    }
    next_state.revision = snapshot
        .state
        .revision
        .checked_add(1)
        .expect("fixture state revision");
    let event_id = format!("synthetic-derived-supervisor-root-{root_index:03}");
    let created_at = Utc::now();
    let mut evaluation_seal = read_evaluation_seal(connection, key);
    evaluation_seal.event_epoch_seconds = created_at.timestamp();
    evaluation_seal.template_values.current_date = Some(created_at.format("%Y-%m-%d").to_string());
    evaluation_seal.template_values.current_time =
        Some(created_at.format("%H:%M:%S%:z").to_string());
    storage
        .commit_interaction_event(&InteractionEventCommit {
            event_id: event_id.clone(),
            idempotency_key: format!("{event_id}-idempotency"),
            key: snapshot.key,
            expected_state_revision: snapshot.state.revision,
            event: InteractionEvent::UserAction {
                action_id: ROOT_ACTION_ID.to_owned(),
            },
            generation_attempt_id: None,
            owner_message_id: None,
            policy: read_policy(connection, key),
            evaluation_seal: Some(evaluation_seal),
            deterministic_seed: Some(
                u64::try_from(root_index).expect("root index fits deterministic seed"),
            ),
            next_state,
            knowledge: snapshot.knowledge,
            action_results,
            effects,
            derived_events,
            proposals: Vec::new(),
            created_at,
        })
        .expect("seed durable root interaction event");
}

fn prepare_backlog(branch_count: usize, actions_per_root: usize) -> SeededBacklog {
    assert!(branch_count > 0);
    assert!(actions_per_root > 0);
    let root = tempdir().expect("create temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open fixture Core");
    let character_id = import_synthetic_character(&core);
    let first = create_branch(&core, &character_id, 0);
    let (rule_set, variables) = interaction_rule_set(actions_per_root);
    let stored_rule_set = core
        .upsert_interaction_rule_set(&rule_set, None)
        .expect("save synthetic interaction rules");
    let revision_id = stored_rule_set
        .revision_id
        .expect("immutable interaction rule-set revision");
    activate_app_module(&core, &interaction_module(&rule_set.id), &first);

    let mut branches = Vec::with_capacity(branch_count);
    branches.push(first);
    for index in 1..branch_count {
        branches.push(create_branch(&core, &character_id, index));
    }
    let mut queue_idle = false;
    for _ in 0..8 {
        let receipt = core
            .drain_core_lifecycle_occurrences(64)
            .expect("drain synthetic conversation-start lifecycle");
        assert!(
            receipt
                .deliveries
                .iter()
                .all(|delivery| { delivery.status == CoreLifecycleDeliveryStatus::Acknowledged })
        );
        if receipt.queue_idle {
            queue_idle = true;
            break;
        }
    }
    assert!(queue_idle, "synthetic lifecycle fixture must become idle");
    for key in &branches {
        assert_eq!(
            core.get_interaction_state_revision(&key.conversation_id, &key.branch_id)
                .expect("read initialized interaction revision"),
            1
        );
    }
    drop(core);

    let storage = Storage::open(root.path()).expect("open fixture storage");
    let connection = Connection::open(active_database_path(root.path()))
        .expect("open fixture policy connection");
    let rule = rule_set.rules.first().expect("fixture root rule");
    for (index, key) in branches.iter().enumerate() {
        seed_root_event(
            &storage,
            &connection,
            key,
            index,
            &revision_id,
            rule,
            &variables,
        );
    }
    drop(connection);
    assert_eq!(
        storage
            .interaction_derived_event_supervisor_status()
            .expect("read seeded supervisor status")
            .pending_count,
        u64::try_from(branch_count * actions_per_root).expect("fixture count fits u64")
    );
    SeededBacklog {
        root,
        storage,
        branches,
    }
}

fn scalar_count(connection: &Connection, sql: &str) -> u64 {
    connection
        .query_row(sql, [], |row| row.get::<_, u64>(0))
        .expect("read synthetic row count")
}

fn assert_materialization_counts(root: &TempDir, expected: u64) {
    let connection = Connection::open(active_database_path(root.path()))
        .expect("inspect derived materialization");
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_events WHERE event_kind = 'variable_changed'"
        ),
        expected
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_outbox
             WHERE status = 'acknowledged'"
        ),
        expected
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_outbox
             WHERE status != 'acknowledged'"
        ),
        0
    );
    assert_eq!(
        scalar_count(
            &connection,
            "SELECT COUNT(*) FROM interaction_derived_event_quarantines"
        ),
        0
    );
}

fn wait_for_recovery_idle(core: &Core, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if !core
            .health_check()
            .expect("read Core health while waiting")
            .recovery_pending
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "derived interaction supervisor did not become idle"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_bounded_drop(core: Core) {
    let started = Instant::now();
    drop(core);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "Core shutdown exceeded the bounded runtime shutdown contract"
    );
}

fn reopen_core(root: &TempDir) -> Core {
    Core::open(CoreConfig::new(root.path())).expect("reopen fixture Core")
}

#[test]
fn startup_supervisor_drains_257_independent_occurrences_once_and_reopen_stays_idle() {
    let SeededBacklog {
        root,
        storage,
        branches,
    } = prepare_backlog(257, 1);
    assert_eq!(branches.len(), 257);
    drop(storage);

    let core = reopen_core(&root);
    wait_for_recovery_idle(&core, Duration::from_secs(5));
    assert!(
        !core
            .health_check()
            .expect("read drained health")
            .recovery_pending
    );
    assert_materialization_counts(&root, 257);
    assert_bounded_drop(core);

    let reopened = reopen_core(&root);
    assert!(
        !reopened
            .health_check()
            .expect("read idle health")
            .recovery_pending
    );
    thread::sleep(Duration::from_millis(50));
    assert_materialization_counts(&root, 257);
    assert_bounded_drop(reopened);
}

#[test]
fn future_branch_head_drives_deadline_and_wakes_its_immediate_successor() {
    let SeededBacklog {
        root,
        storage,
        branches,
    } = prepare_backlog(1, 2);
    let claimed_at = Utc::now();
    let occurrence = storage
        .claim_interaction_derived_events(claimed_at, claimed_at + ChronoDuration::seconds(30), 1)
        .expect("claim same-branch head")
        .pop()
        .expect("same-branch head occurrence");
    assert_eq!(occurrence.branch_id, branches[0].branch_id);
    assert_eq!(occurrence.chain_ordinal, 1);
    let retry_at = Utc::now() + ChronoDuration::seconds(2);
    storage
        .retry_interaction_derived_event_after(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            retry_at,
        )
        .expect("defer same-branch head");

    let status = storage
        .interaction_derived_event_supervisor_status()
        .expect("read causal supervisor deadline");
    assert_eq!(status.pending_count, 2);
    assert_eq!(status.next_available_at, Some(retry_at));
    assert!(
        storage
            .claim_interaction_derived_events(
                Utc::now(),
                Utc::now() + ChronoDuration::seconds(30),
                1,
            )
            .expect("attempt claim behind future predecessor")
            .is_empty(),
        "an immediate same-branch successor must remain behind its future predecessor"
    );
    drop(storage);

    let core = reopen_core(&root);
    assert!(
        core.health_check()
            .expect("read queued recovery health")
            .recovery_pending
    );
    let before = Connection::open(active_database_path(root.path()))
        .expect("inspect pre-deadline materialization");
    assert_eq!(
        scalar_count(
            &before,
            "SELECT COUNT(*) FROM interaction_events WHERE event_kind = 'variable_changed'"
        ),
        0
    );
    drop(before);
    wait_for_recovery_idle(&core, Duration::from_secs(5));
    assert!(
        !core
            .health_check()
            .expect("read drained health")
            .recovery_pending
    );
    assert_materialization_counts(&root, 2);
    assert_bounded_drop(core);
}

#[test]
fn independent_immediate_branch_is_not_hidden_by_a_future_branch_head() {
    let fixture = prepare_backlog(2, 1);
    let claimed_at = Utc::now();
    let future = fixture
        .storage
        .claim_interaction_derived_events(claimed_at, claimed_at + ChronoDuration::seconds(30), 1)
        .expect("claim one independent branch")
        .pop()
        .expect("first independent occurrence");
    let retry_at = Utc::now() + ChronoDuration::seconds(5);
    fixture
        .storage
        .retry_interaction_derived_event_after(
            &future.occurrence_id,
            future.delivery_attempts,
            retry_at,
        )
        .expect("defer one independent branch");

    let status = fixture
        .storage
        .interaction_derived_event_supervisor_status()
        .expect("read independent branch deadline");
    assert_eq!(status.pending_count, 2);
    assert!(
        status
            .next_available_at
            .is_some_and(|available_at| available_at <= Utc::now()),
        "an independent immediate branch must set the runnable deadline"
    );
    let immediate = fixture
        .storage
        .claim_interaction_derived_events(Utc::now(), Utc::now() + ChronoDuration::seconds(30), 1)
        .expect("claim independent immediate branch")
        .pop()
        .expect("independent immediate occurrence");
    assert_ne!(immediate.branch_id, future.branch_id);
}

#[test]
fn quarantine_replay_rejects_json_and_hash_corruption_as_storage_corrupted() {
    let fixture = prepare_backlog(1, 1);
    let claimed_at = Utc::now();
    let occurrence = fixture
        .storage
        .claim_interaction_derived_events(claimed_at, claimed_at + ChronoDuration::seconds(30), 1)
        .expect("claim occurrence for quarantine")
        .pop()
        .expect("occurrence for quarantine");
    fixture
        .storage
        .quarantine_interaction_derived_event_authority_failure(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            None,
            Utc::now(),
        )
        .expect("write canonical quarantine evidence");

    let connection =
        Connection::open(fixture.database_path()).expect("open quarantine corruption connection");
    connection
        .execute(
            "DROP TRIGGER interaction_derived_event_quarantine_no_update",
            [],
        )
        .expect("remove quarantine immutability trigger in synthetic fixture");
    let (evidence_json, evidence_sha256) = connection
        .query_row(
            "SELECT evidence_json, evidence_sha256
             FROM interaction_derived_event_quarantines
             WHERE occurrence_id = ?1",
            [&occurrence.occurrence_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("read canonical quarantine evidence");

    connection
        .execute(
            "UPDATE interaction_derived_event_quarantines
             SET evidence_json = ?2
             WHERE occurrence_id = ?1",
            params![occurrence.occurrence_id, format!("{evidence_json} ")],
        )
        .expect("tamper quarantine evidence JSON");
    let json_error = fixture
        .storage
        .quarantine_interaction_derived_event_authority_failure(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            None,
            Utc::now(),
        )
        .expect_err("non-canonical quarantine JSON must fail closed");
    assert_eq!(json_error.code, CoreErrorCode::StorageCorrupted);

    connection
        .execute(
            "UPDATE interaction_derived_event_quarantines
             SET evidence_json = ?2, evidence_sha256 = ?3
             WHERE occurrence_id = ?1",
            params![occurrence.occurrence_id, evidence_json, "f".repeat(64)],
        )
        .expect("tamper quarantine evidence hash");
    assert_ne!(evidence_sha256, "f".repeat(64));
    let hash_error = fixture
        .storage
        .quarantine_interaction_derived_event_authority_failure(
            &occurrence.occurrence_id,
            occurrence.delivery_attempts,
            None,
            Utc::now(),
        )
        .expect_err("quarantine evidence hash mismatch must fail closed");
    assert_eq!(hash_error.code, CoreErrorCode::StorageCorrupted);
}
