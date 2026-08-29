//! Upgrade/reopen smoke coverage for every durable schema cut point.

use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{Duration, Utc};
use lorepia_domain::{
    Character, ConversationBranchId, ConversationId, ConversationMode, CoreErrorCode,
    InteractionEvent, InteractionState, MessageId, Sha256Digest, VariableMap,
    prompt_local_user_id_sha256,
};
use lorepia_storage::{
    GenerationApprovalEvidence, GenerationAttemptInput, GenerationAttemptStatus,
    GenerationBeforeEventEvidence, GenerationDispatchSeal, GenerationPromptQuickSettingsAuthority,
    GenerationPromptSelectionAuthority, GenerationProviderTargetAuthority, InteractionEventCommit,
    InteractionPolicySnapshot, PromptResponseLength, Storage, built_in_prompt_presets,
    deterministic_generation_id, generation_approval_evidence_sha256, generation_attempt_sha256,
    generation_before_event_evidence_sha256, generation_dispatch_seal_sha256,
    interaction_policy_sha256, interaction_state_key_for_branch,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use tempfile::tempdir;

const MIGRATION_TIME: &str = "2026-08-09T00:00:00Z";
const HASH_A: &str = "ca978112ca1bbdcafac231b39a23dc4da786eff8147c4e72b9807785afee48bb";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const HASH_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const EMPTY_INTERACTION_MODULE_SHA256: &str =
    "8dcf5fc8df8f1f93618b7686c3e5333d7426c81df6e6a15aaf857ce308537787";
const CONVERSATION_ID: &str = "schema-35-conversation";
const BRANCH_ID: &str = "schema-35-branch";
const HEAD_MESSAGE_ID: &str = "schema-35-head";

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_import_asset_recovery.sql"),
    include_str!("../migrations/0003_conversation_branches.sql"),
    include_str!("../migrations/0004_provider_catalog.sql"),
    include_str!("../migrations/0005_discovery_state_machine.sql"),
    include_str!("../migrations/0006_generation_provider_provenance.sql"),
    include_str!("../migrations/0007_signed_catalog_history.sql"),
    include_str!("../migrations/0008_generation_protocol_state.sql"),
    include_str!("../migrations/0009_model_sync_jobs.sql"),
    include_str!("../migrations/0010_provider_connection_tombstones.sql"),
    include_str!("../migrations/0011_provider_local_network_approvals.sql"),
    include_str!("../migrations/0012_content_package_foundation.sql"),
    include_str!("../migrations/0013_prompt_pipeline.sql"),
    include_str!("../migrations/0014_knowledge.sql"),
    include_str!("../migrations/0015_memory.sql"),
    include_str!("../migrations/0016_transforms.sql"),
    include_str!("../migrations/0017_interactions_modules.sql"),
    include_str!("../migrations/0018_persona_selection.sql"),
    include_str!("../migrations/0019_lifecycle_outbox.sql"),
    include_str!("../migrations/0020_package_cas_promotion_journal.sql"),
    include_str!("../migrations/0021_interaction_checkpoints.sql"),
    include_str!("../migrations/0022_memory_vector_space.sql"),
    include_str!("../migrations/0023_applied_module_runtime_plans.sql"),
    include_str!("../migrations/0024_generation_attempt_proposals.sql"),
    include_str!("../migrations/0025_conversation_greeting_bindings.sql"),
    include_str!("../migrations/0026_provider_discovery_native_no_effect.sql"),
    include_str!("../migrations/0027_provider_discovery_native_attestations.sql"),
    include_str!("../migrations/0028_generation_attempt_storage_identities.sql"),
    include_str!("../migrations/0029_generation_attempt_decision_handshake.sql"),
    include_str!("../migrations/0030_package_document_target_reviews.sql"),
    include_str!("../migrations/0031_message_display_projections.sql"),
    include_str!("../migrations/0032_knowledge_vector_space.sql"),
    include_str!("../migrations/0033_interaction_derived_event_outbox.sql"),
    include_str!("../migrations/0034_generation_attempt_derived_event_authority.sql"),
    include_str!("../migrations/0035_interaction_derived_event_quarantine.sql"),
    include_str!("../migrations/0036_generation_attempt_derived_closure.sql"),
    include_str!("../migrations/0037_provider_credential_operations.sql"),
    include_str!("../migrations/0038_conversation_speakers.sql"),
    include_str!("../migrations/0039_runtime_model_audit.sql"),
    include_str!("../migrations/0040_portable_runtime_state.sql"),
];

#[test]
fn every_durable_schema_cutpoint_upgrades_and_reopens_idempotently() {
    let latest = u32::try_from(MIGRATIONS.len()).expect("migration count fits u32");
    let canonical_inventory = verify_cutpoint_upgrade(1, latest);
    for cutpoint in 2..=latest {
        assert_eq!(
            verify_cutpoint_upgrade(cutpoint, latest),
            canonical_inventory,
            "schema inventory differs after upgrading cutpoint {cutpoint}"
        );
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn schema_thirty_five_active_attempts_fail_closed_and_require_a_fresh_operation() {
    let root = tempdir().expect("temporary schema-35 attempt root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create schema-35 attempt database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let legacy_attempts = {
        let mut connection = Connection::open(&database_path).expect("open schema-35 attempt DB");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("enable schema-35 attempt foreign keys");
        apply_through(&mut connection, 35);
        seed_chat_boundary(root.path(), &connection);
        [
            ("prepared", 3_u64),
            ("before_generation_applied", 5),
            ("awaiting_approval", 7),
            ("dispatch_ready", 11),
        ]
        .into_iter()
        .map(|(status, revision)| seed_legacy_attempt(&connection, status, revision))
        .collect::<Vec<_>>()
    };

    let storage = Storage::open(root.path()).expect("upgrade active attempts from schema 35");
    assert_eq!(
        storage
            .schema_version()
            .expect("read schema after active-attempt cutover"),
        40
    );
    for seeded in &legacy_attempts {
        let migrated = storage
            .get_generation_attempt(&seeded.generation_id)
            .expect("read failed-closed legacy attempt");
        assert_eq!(
            migrated.status,
            GenerationAttemptStatus::FailedBeforeDispatch
        );
        assert_eq!(migrated.revision, seeded.prior_revision + 1);
        assert_eq!(
            migrated.failure_code.as_deref(),
            Some("stale_generation_derived_closure_authority")
        );
        assert!(
            migrated.input.prompt_selection_authority.is_none(),
            "migration must not invent prompt authority for {}",
            seeded.prior_status
        );
    }

    let retry_target = &legacy_attempts[0];
    storage
        .retry_generation_attempt(
            &retry_target.generation_id,
            retry_target.prior_revision + 1,
            Utc::now(),
        )
        .expect_err("a schema-35 attempt must not resume under the same generation id");
    assert_eq!(
        storage
            .get_generation_attempt(&retry_target.generation_id)
            .expect("re-read failed-closed retry target")
            .status,
        GenerationAttemptStatus::FailedBeforeDispatch
    );

    let local_user_id = storage
        .load_settings()
        .expect("load regenerated local-user authority")
        .local_user_id;
    let prompt_authority =
        synthetic_prompt_selection_authority(prompt_local_user_id_sha256(&local_user_id));
    let mut reused_input = retry_target.input.clone();
    reused_input.prompt_selection_authority = Some(prompt_authority.clone());
    let reused_error = storage
        .prepare_generation_attempt(&reused_input, Utc::now())
        .expect_err("legacy operation id must not be rebound to reconstructed authority");
    assert_eq!(reused_error.code, CoreErrorCode::InvalidInput);

    let module_runtime_review_authority = lorepia_orchestration::review_module_merge(
        0,
        &lorepia_orchestration::ModuleResolutionContext {
            local_user_id,
            persona_id: None,
            character_id: Some("schema-35-character".to_owned()),
            conversation_id: Some(CONVERSATION_ID.to_owned()),
            branch_id: Some(BRANCH_ID.to_owned()),
            supported_capabilities: Vec::new(),
        },
        &[],
        &[],
    )
    .expect("review regenerated no-module authority");
    let regenerated_input = GenerationAttemptInput {
        operation_id: "explicit-regenerate-after-schema-36".to_owned(),
        prompt_selection_authority: Some(prompt_authority),
        module_plan_sha256: lorepia_orchestration::no_applied_module_runtime_plan_sha256(),
        module_runtime_review_authority: Some(module_runtime_review_authority),
        applied_runtime_plan_authority: None,
        ..retry_target.input.clone()
    };
    let regenerated = storage
        .prepare_generation_attempt(&regenerated_input, Utc::now())
        .expect("prepare an explicit fresh regeneration operation");
    assert_eq!(regenerated.status, GenerationAttemptStatus::Prepared);
    assert_ne!(regenerated.generation_id, retry_target.generation_id);
    assert!(regenerated.input.prompt_selection_authority.is_some());
    drop(storage);

    let active_database_path = resolved_database_path(root.path(), &database_path, 35);
    let connection =
        Connection::open(&active_database_path).expect("inspect legacy attempt audits");
    let audits = connection
        .prepare(
            "SELECT generation_id, prior_status, prior_revision, attempt_sha256,
                    before_generation_evidence_sha256,
                    approval_evidence_sha256, dispatch_seal_sha256,
                    reason_kind, recorded_at
             FROM generation_attempt_legacy_closure_interruptions
             ORDER BY generation_id",
        )
        .expect("prepare legacy attempt audit query")
        .query_map([], |row| {
            Ok(LegacyAttemptAudit {
                generation_id: row.get(0)?,
                prior_status: row.get(1)?,
                prior_revision: row.get(2)?,
                attempt_sha256: row.get(3)?,
                before_sha256: row.get(4)?,
                approval_sha256: row.get(5)?,
                dispatch_sha256: row.get(6)?,
                reason_kind: row.get(7)?,
                recorded_at: row.get(8)?,
            })
        })
        .expect("query legacy attempt audits")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect legacy attempt audits");
    assert_eq!(audits.len(), legacy_attempts.len());
    for seeded in &legacy_attempts {
        let audit = audits
            .iter()
            .find(|audit| audit.generation_id == seeded.generation_id.0)
            .expect("find exact legacy attempt audit");
        assert_eq!(audit.prior_status, seeded.prior_status);
        assert_eq!(audit.prior_revision, seeded.prior_revision);
        assert_eq!(audit.attempt_sha256, seeded.attempt_sha256);
        assert_eq!(audit.before_sha256, seeded.before_sha256);
        assert_eq!(audit.approval_sha256, seeded.approval_sha256);
        assert_eq!(audit.dispatch_sha256, seeded.dispatch_sha256);
        assert_eq!(
            audit.reason_kind,
            "stale_generation_derived_closure_authority"
        );
        assert_eq!(audit.recorded_at, MIGRATION_TIME);
    }
    assert!(
        connection
            .execute(
                "UPDATE generation_attempt_legacy_closure_interruptions
                 SET recorded_at = '2026-08-09T00:00:01Z'",
                [],
            )
            .is_err(),
        "legacy interruption evidence must be immutable"
    );
    assert!(
        connection
            .execute(
                "DELETE FROM generation_attempt_legacy_closure_interruptions",
                [],
            )
            .is_err(),
        "legacy interruption evidence must not be deletable"
    );
    assert!(
        connection
            .execute(
                "UPDATE generation_attempt_intents
                 SET status = 'prepared', failure_code = NULL,
                     revision = revision + 1, updated_at = ?2
                 WHERE generation_id = ?1",
                params![retry_target.generation_id.0, MIGRATION_TIME],
            )
            .is_err(),
        "the schema guard must reject reviving an interrupted legacy generation id"
    );
    drop(connection);

    let before_reopen = schema_inventory(&active_database_path);
    drop(Storage::open(root.path()).expect("reopen schema-36 attempt cutover"));
    let reopened_database_path = resolved_database_path(root.path(), &database_path, 35);
    assert_eq!(
        schema_inventory(&reopened_database_path),
        before_reopen,
        "reopen must not recreate legacy interruption authority"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn schema_thirty_five_ordinary_history_survives_and_unsealed_work_is_quarantined() {
    let root = tempdir().expect("temporary schema-35 outbox root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create schema-35 outbox database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    let state_key = interaction_state_key_for_branch(
        &ConversationId(CONVERSATION_ID.to_owned()),
        &ConversationBranchId(BRANCH_ID.to_owned()),
    )
    .expect("derive schema-35 interaction state key");
    {
        let mut connection = Connection::open(&database_path).expect("open schema-35 outbox DB");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("enable schema-35 outbox foreign keys");
        apply_through(&mut connection, 35);
        seed_chat_boundary(root.path(), &connection);
        seed_legacy_ordinary_history(&connection, &state_key.state_id);
    }

    let storage = Storage::open(root.path()).expect("upgrade ordinary history from schema 35");
    let active_database_path = resolved_database_path(root.path(), &database_path, 35);
    let historical = storage
        .get_interaction_event("schema-35-root-event")
        .expect("read historical ordinary event")
        .expect("historical ordinary event exists");
    assert_eq!(historical.event_id, "schema-35-root-event");
    assert_eq!(historical.resulting_state_revision, 1);

    let claim_at = Utc::now() + Duration::days(1);
    assert!(
        storage
            .claim_interaction_derived_events(claim_at, claim_at + Duration::seconds(30), 8,)
            .expect("terminally quarantine schema-35 unsealed work")
            .is_empty(),
        "unsealed legacy work must never reach an evaluator"
    );
    assert_eq!(
        storage
            .interaction_derived_event_supervisor_status()
            .expect("read supervisor after legacy quarantine")
            .pending_count,
        0
    );
    let quarantine_before_reopen = legacy_outbox_snapshot(&active_database_path);
    assert_eq!(quarantine_before_reopen.quarantines.len(), 2);
    assert_eq!(
        quarantine_before_reopen
            .quarantines
            .iter()
            .map(|row| (row.0.as_str(), row.1, row.2.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (
                "schema-35-claimed-occurrence",
                2,
                "sealed_policy_recovery_failed"
            ),
            (
                "schema-35-pending-occurrence",
                1,
                "sealed_policy_recovery_failed"
            ),
        ]
    );
    assert_eq!(
        quarantine_before_reopen.acknowledged_status,
        ("acknowledged".to_owned(), 1, None, None, 0, None)
    );
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen quarantined schema-35 work");
    let reopened_database_path = resolved_database_path(root.path(), &database_path, 35);
    assert!(
        reopened
            .claim_interaction_derived_events(
                claim_at + Duration::minutes(1),
                claim_at + Duration::minutes(2),
                8,
            )
            .expect("repeat legacy quarantine drain")
            .is_empty()
    );
    assert_eq!(
        legacy_outbox_snapshot(&reopened_database_path),
        quarantine_before_reopen,
        "reopen and repeat drain must preserve exact terminal evidence"
    );
    assert!(
        reopened
            .get_interaction_event("schema-35-root-event")
            .expect("re-read historical ordinary event after reopen")
            .is_some()
    );

    let next_state = InteractionState {
        variables: VariableMap::default(),
        manually_active_knowledge: Vec::new(),
        proposals: Vec::new(),
        revision: 2,
    };
    let successor = InteractionEventCommit {
        event_id: "schema-36-successor-event".to_owned(),
        idempotency_key: "schema-36-successor-idempotency".to_owned(),
        key: state_key,
        expected_state_revision: 1,
        event: InteractionEvent::ConversationStarted,
        generation_attempt_id: None,
        owner_message_id: None,
        policy: empty_interaction_policy(),
        evaluation_seal: None,
        deterministic_seed: None,
        next_state,
        knowledge: Vec::new(),
        action_results: Vec::new(),
        effects: Vec::new(),
        derived_events: Vec::new(),
        proposals: Vec::new(),
        created_at: claim_at + Duration::minutes(3),
    };
    let committed = reopened
        .commit_interaction_event(&successor)
        .expect("terminal quarantine must unblock a later ordinary root");
    assert!(!committed.exact_replay);
    assert!(
        reopened
            .commit_interaction_event(&successor)
            .expect("successor exact retry after branch unblock")
            .exact_replay
    );
}

#[derive(Debug)]
struct LegacyAttemptSeed {
    generation_id: lorepia_domain::GenerationId,
    input: GenerationAttemptInput,
    prior_status: String,
    prior_revision: u64,
    attempt_sha256: String,
    before_sha256: Option<String>,
    approval_sha256: Option<String>,
    dispatch_sha256: Option<String>,
}

#[derive(Debug)]
struct LegacyAttemptAudit {
    generation_id: String,
    prior_status: String,
    prior_revision: u64,
    attempt_sha256: String,
    before_sha256: Option<String>,
    approval_sha256: Option<String>,
    dispatch_sha256: Option<String>,
    reason_kind: String,
    recorded_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::type_complexity)]
struct LegacyOutboxSnapshot {
    quarantines: Vec<(String, u64, String, String)>,
    acknowledged_status: (
        String,
        u64,
        Option<String>,
        Option<String>,
        u64,
        Option<String>,
    ),
}

fn seed_chat_boundary(root: &Path, connection: &Connection) {
    let source_relative_path = format!("sources/sha256/{}/{}", &HASH_A[..2], &HASH_A[2..]);
    let source_path = root.join(&source_relative_path);
    fs::create_dir_all(source_path.parent().expect("schema-35 source CAS parent"))
        .expect("create schema-35 source CAS parent");
    fs::write(&source_path, b"a").expect("write schema-35 source CAS bytes");
    connection
        .execute_batch(&format!(
            "INSERT INTO content_sources
                 (sha256, relative_path, size_bytes, created_at)
             VALUES ('{HASH_A}', '{source_relative_path}', 1, '{MIGRATION_TIME}');
             INSERT INTO characters
                 (id, name, description, source_hash, avatar_asset_hash, created_at)
             VALUES (
                 'schema-35-character', 'Schema 35', 'synthetic migration fixture',
                 '{HASH_A}', NULL, '{MIGRATION_TIME}'
             );
             INSERT INTO conversations
                 (id, character_id, title, created_at, updated_at)
             VALUES (
                 '{CONVERSATION_ID}', 'schema-35-character', 'Schema 35 room',
                 '{MIGRATION_TIME}', '{MIGRATION_TIME}'
             );
             INSERT INTO messages
                 (id, conversation_id, parent_id, role, content, status,
                  generation_id, created_at)
             VALUES (
                 '{HEAD_MESSAGE_ID}', '{CONVERSATION_ID}', NULL, 'user',
                 'synthetic migration boundary', 'complete', NULL,
                 '{MIGRATION_TIME}'
             );
             INSERT INTO conversation_branches
                 (id, conversation_id, title, fork_message_id, head_message_id,
                  created_at, updated_at)
             VALUES (
                 '{BRANCH_ID}', '{CONVERSATION_ID}', NULL, NULL,
                 '{HEAD_MESSAGE_ID}', '{MIGRATION_TIME}', '{MIGRATION_TIME}'
             );
             INSERT INTO conversation_state
                 (conversation_id, active_branch_id, selected_mode, updated_at)
             VALUES (
                 '{CONVERSATION_ID}', '{BRANCH_ID}', 'chat', '{MIGRATION_TIME}'
             );"
        ))
        .expect("seed schema-35 chat boundary");
}

#[allow(clippy::too_many_lines)]
fn seed_legacy_attempt(connection: &Connection, status: &str, revision: u64) -> LegacyAttemptSeed {
    let operation_id = format!("schema-35-{status}-operation");
    let input = GenerationAttemptInput {
        operation_id: operation_id.clone(),
        conversation_id: ConversationId(CONVERSATION_ID.to_owned()),
        source_branch_id: ConversationBranchId(BRANCH_ID.to_owned()),
        proposed_branch_id: ConversationBranchId(BRANCH_ID.to_owned()),
        expected_head_message_id: Some(MessageId(HEAD_MESSAGE_ID.to_owned())),
        context_head_message_id: Some(MessageId(HEAD_MESSAGE_ID.to_owned())),
        module_plan_sha256: sha256(HASH_B),
        base_request_fingerprint_sha256: sha256(HASH_C),
        prompt_selection_authority: None,
        module_runtime_review_authority: None,
        applied_runtime_plan_authority: None,
    };
    let before = match status {
        "prepared" => None,
        "before_generation_applied" | "dispatch_ready" => Some(GenerationBeforeEventEvidence {
            event_id: format!("schema-35-{status}-before-event"),
            event_sha256: sha256(HASH_D),
            context_state_revision: 1,
            context_state_sha256: sha256(HASH_A),
            proposal_review_sha256s: Vec::new(),
            awaiting_approval: false,
        }),
        "awaiting_approval" => Some(GenerationBeforeEventEvidence {
            event_id: "schema-35-awaiting-approval-before-event".to_owned(),
            event_sha256: sha256(HASH_D),
            context_state_revision: 1,
            context_state_sha256: sha256(HASH_A),
            proposal_review_sha256s: vec![sha256(HASH_B)],
            awaiting_approval: true,
        }),
        other => panic!("unsupported synthetic legacy status {other}"),
    };
    let before_sha256 = before
        .as_ref()
        .map(generation_before_event_evidence_sha256)
        .transpose()
        .expect("hash legacy before-generation evidence");
    let approval = if status == "before_generation_applied" {
        Some(GenerationApprovalEvidence {
            before_event_sha256: before_sha256
                .clone()
                .expect("before-applied legacy evidence exists"),
            decision_event_ids: vec!["schema-35-approval-decision".to_owned()],
            decision_event_sha256s: vec![sha256(HASH_C)],
            resulting_state_revision: 2,
            resulting_state_sha256: sha256(HASH_D),
        })
    } else {
        None
    };
    let approval_sha256 = approval
        .as_ref()
        .map(generation_approval_evidence_sha256)
        .transpose()
        .expect("hash legacy approval evidence");
    let dispatch = if status == "dispatch_ready" {
        Some(GenerationDispatchSeal {
            final_prompt_plan_sha256: sha256(HASH_A),
            final_prompt_input_fingerprint_sha256: sha256(HASH_B),
            final_interaction_state_revision: 2,
            final_interaction_state_sha256: sha256(HASH_C),
            applied_module_plan_sha256: input.module_plan_sha256.clone(),
            before_generation_evidence_sha256: before_sha256
                .clone()
                .expect("dispatch-ready legacy before evidence exists"),
            approval_evidence_sha256: None,
            derived_chain_sha256: Some(sha256(HASH_D)),
            derived_event_count: Some(1),
            derived_guard_count: Some(0),
        })
    } else {
        None
    };
    let dispatch_sha256 = dispatch
        .as_ref()
        .map(generation_dispatch_seal_sha256)
        .transpose()
        .expect("hash legacy dispatch evidence");
    let attempt_sha256 = generation_attempt_sha256(&input).expect("hash legacy attempt input");
    let generation_id = deterministic_generation_id(&attempt_sha256);
    let before_json = before
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .expect("encode legacy before evidence");
    let approval_json = approval
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .expect("encode legacy approval evidence");
    let dispatch_json = dispatch
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .expect("encode legacy dispatch evidence");
    connection
        .execute(
            "INSERT INTO generation_attempt_intents
             (generation_id, operation_id, conversation_id,
              source_branch_id, proposed_branch_id,
              expected_head_message_id, context_head_message_id,
              module_plan_sha256, base_input_fingerprint_sha256,
              before_generation_evidence_json,
              before_generation_evidence_sha256,
              approval_evidence_json, approval_evidence_sha256,
              dispatch_seal_json, dispatch_seal_sha256,
              attempt_sha256, status, revision, failure_code,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?5, ?6, ?7,
                     ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     NULL, ?17, ?17)",
            params![
                generation_id.0,
                operation_id,
                CONVERSATION_ID,
                BRANCH_ID,
                HEAD_MESSAGE_ID,
                input.module_plan_sha256.as_str(),
                input.base_request_fingerprint_sha256.as_str(),
                before_json.as_deref(),
                before_sha256.as_ref().map(Sha256Digest::as_str),
                approval_json.as_deref(),
                approval_sha256.as_ref().map(Sha256Digest::as_str),
                dispatch_json.as_deref(),
                dispatch_sha256.as_ref().map(Sha256Digest::as_str),
                attempt_sha256.as_str(),
                status,
                i64::try_from(revision).expect("legacy attempt revision fits i64"),
                MIGRATION_TIME,
            ],
        )
        .expect("seed active schema-35 generation attempt");
    LegacyAttemptSeed {
        generation_id,
        input,
        prior_status: status.to_owned(),
        prior_revision: revision,
        attempt_sha256: attempt_sha256.as_str().to_owned(),
        before_sha256: before_sha256.map(|value| value.as_str().to_owned()),
        approval_sha256: approval_sha256.map(|value| value.as_str().to_owned()),
        dispatch_sha256: dispatch_sha256.map(|value| value.as_str().to_owned()),
    }
}

fn synthetic_prompt_selection_authority(
    local_user_id_sha256: String,
) -> GenerationPromptSelectionAuthority {
    GenerationPromptSelectionAuthority {
        schema_version: 1,
        mode: ConversationMode::Chat,
        local_user_id_sha256,
        character: Character {
            id: "schema-35-character".to_owned(),
            name: "Schema 35".to_owned(),
            description: "synthetic migration fixture".to_owned(),
            source_hash: HASH_A.to_owned(),
            avatar_asset_hash: None,
            created_at: MIGRATION_TIME
                .parse()
                .expect("synthetic character timestamp"),
        },
        character_content: None,
        character_knowledge_book: None,
        supported_capabilities: Vec::new(),
        quick_settings: GenerationPromptQuickSettingsAuthority {
            response_length: PromptResponseLength::Balanced,
            creativity: 50,
            reasoning_effort: None,
            memory_enabled: true,
            knowledge_enabled: true,
            supports_temperature: false,
            resolved_temperature: None,
            resolved_max_output_tokens: None,
        },
        provider_target_authority: Some(GenerationProviderTargetAuthority::DirectModel {
            model_sha256: sha256(HASH_D),
        }),
        explicit_preset_id: None,
        preset: built_in_prompt_presets()[0].clone(),
        preset_revision: 1,
        preset_revision_id: "schema-36-regenerated-prompt-revision".to_owned(),
        binding: None,
        persona_selection: None,
    }
}

#[allow(clippy::too_many_lines)]
fn seed_legacy_ordinary_history(connection: &Connection, state_id: &str) {
    let state = InteractionState {
        variables: VariableMap::default(),
        manually_active_knowledge: Vec::new(),
        proposals: Vec::new(),
        revision: 1,
    };
    let state_json = serde_json::to_string(&state).expect("encode legacy interaction state");
    let policy = empty_interaction_policy();
    let policy_json = serde_json::to_string(&policy).expect("encode legacy interaction policy");
    let policy_sha256 = interaction_policy_sha256(&policy).expect("hash legacy policy");
    let payload_json = json!({
        "schema_version": 1,
        "commit_sha256": HASH_B,
        "resulting_state_snapshot_sha256": HASH_C,
        "proposal_review_sha256s": []
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO interaction_state
             (id, conversation_id, branch_id, revision, document_json, updated_at)
             VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            params![
                state_id,
                CONVERSATION_ID,
                BRANCH_ID,
                state_json,
                MIGRATION_TIME
            ],
        )
        .expect("seed legacy interaction state");
    connection
        .execute(
            "INSERT INTO interaction_events
             (id, idempotency_key, interaction_state_id,
              expected_state_revision, resulting_state_revision,
              conversation_id, branch_id, event_kind, event_argument_json,
              module_plan_sha256, policy_json, policy_sha256,
              payload_json, created_at)
             VALUES (
              'schema-35-root-event', 'schema-35-root-idempotency', ?1,
              0, 1, ?2, ?3, 'conversation_opened', NULL,
              ?4, ?5, ?6, ?7, ?8)",
            params![
                state_id,
                CONVERSATION_ID,
                BRANCH_ID,
                EMPTY_INTERACTION_MODULE_SHA256,
                policy_json,
                policy_sha256,
                payload_json,
                MIGRATION_TIME,
            ],
        )
        .expect("seed legacy ordinary interaction event");
    seed_legacy_interaction_action(connection);
    for (occurrence_id, chain_id, source_effect_ordinal, status, attempts, lease, acknowledged) in [
        (
            "schema-35-pending-occurrence",
            "schema-35-pending-chain",
            0_i64,
            "pending",
            0_i64,
            None,
            None,
        ),
        (
            "schema-35-claimed-occurrence",
            "schema-35-claimed-chain",
            1,
            "claimed",
            1,
            Some("2026-08-09T00:05:00Z"),
            None,
        ),
        (
            "schema-35-acknowledged-occurrence",
            "schema-35-acknowledged-chain",
            2,
            "acknowledged",
            1,
            None,
            Some("2026-08-09T00:01:00Z"),
        ),
    ] {
        connection
            .execute(
                "INSERT INTO interaction_derived_event_outbox
                 (occurrence_id, chain_id, root_event_id, parent_event_id,
                  parent_occurrence_id, conversation_id, branch_id, depth,
                  chain_ordinal, source_effect_ordinal,
                  parent_event_commit_sha256, parent_resulting_state_revision,
                  source_effect_sha256, source_action_sha256,
                  source_set_revision_id, source_rule_id, source_action_ordinal,
                  event_kind, event_argument_json, event_sha256,
                  visited_event_sha256s_json, policy_json, policy_sha256,
                  occurred_at, available_at, status, delivery_attempts,
                  lease_until, acknowledged_at, created_at)
                 VALUES (
                  ?1, ?2, 'schema-35-root-event', 'schema-35-root-event',
                  NULL, ?3, ?4, 1, 1, ?5, ?6, 1, ?7, ?8,
                  'schema-35-rule-revision', 'schema-35-rule', 0,
                  'variable_changed', '{}', ?9, ?10, ?11, ?12,
                  ?13, ?13, ?14, ?15, ?16, ?17, ?13)",
                params![
                    occurrence_id,
                    chain_id,
                    CONVERSATION_ID,
                    BRANCH_ID,
                    source_effect_ordinal,
                    HASH_B,
                    HASH_A,
                    HASH_C,
                    HASH_D,
                    format!("[\"{HASH_A}\"]"),
                    policy_json,
                    policy_sha256,
                    MIGRATION_TIME,
                    status,
                    attempts,
                    lease,
                    acknowledged,
                ],
            )
            .expect("seed schema-35 derived outbox row");
    }
}

fn seed_legacy_interaction_action(connection: &Connection) {
    connection
        .execute_batch(&format!(
            "INSERT INTO content_objects
                 (id, object_kind, created_at, deleted_at)
             VALUES (
                 'schema-35-rule-set', 'interaction_rule_set',
                 '{MIGRATION_TIME}', NULL
             );
             INSERT INTO content_revisions
                 (id, object_id, object_kind, revision_no, parent_revision_id,
                  schema_version, document_json, document_sha256, source_kind,
                  source_hash, provenance_json, local_override_of_revision_id,
                  created_at)
             VALUES (
                 'schema-35-rule-revision', 'schema-35-rule-set',
                 'interaction_rule_set', 1, NULL, 1, '{{}}', '{HASH_A}',
                 'migrated', NULL, '{{}}', NULL, '{MIGRATION_TIME}'
             );
             INSERT INTO interaction_rule_sets
                 (id, name, schema_version, revision, max_actions_per_event,
                  document_json, provenance_json, source_kind, source_hash,
                  created_at, updated_at, deleted_at)
             VALUES (
                 'schema-35-rule-set', 'Schema 35 rule set', 1, 1, 1,
                 '{{}}', '{{}}', 'migrated', NULL,
                 '{MIGRATION_TIME}', '{MIGRATION_TIME}', NULL
             );
             INSERT INTO interaction_rule_set_revisions
                 (revision_id, interaction_rule_set_id, revision_no, name,
                  max_actions_per_event, source_kind, document_json)
             VALUES (
                 'schema-35-rule-revision', 'schema-35-rule-set', 1,
                 'Schema 35 rule set', 1, 'migrated', '{{}}'
             );
             INSERT INTO interaction_rules
                 (set_revision_id, rule_id, ordinal, name, enabled,
                  event_kind, event_argument_json, condition_json, priority,
                  stop_after_match, provenance_json, document_json)
             VALUES (
                 'schema-35-rule-revision', 'schema-35-rule', 0,
                 'Schema 35 rule', 1, 'variable_changed', '{{}}', NULL,
                 0, 0, '{{}}', '{{}}'
             );
             INSERT INTO interaction_actions
                 (set_revision_id, rule_id, ordinal, action_kind,
                  payload_json, knowledge_book_revision_id,
                  knowledge_entry_id, asset_descriptor_id, requires_approval)
             VALUES (
                 'schema-35-rule-revision', 'schema-35-rule', 0,
                 'append_visible_system_event', '{{}}', NULL, NULL, NULL, 0
             );"
        ))
        .expect("seed schema-35 interaction action authority");
}

fn legacy_outbox_snapshot(database_path: &Path) -> LegacyOutboxSnapshot {
    let connection = Connection::open(database_path).expect("inspect schema-35 outbox cutover");
    let quarantines = connection
        .prepare(
            "SELECT occurrence_id, delivery_attempts, reason_kind, evidence_sha256
             FROM interaction_derived_event_quarantines
             ORDER BY occurrence_id",
        )
        .expect("prepare legacy quarantine snapshot")
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("query legacy quarantine snapshot")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect legacy quarantine snapshot");
    let acknowledged_status = connection
        .query_row(
            "SELECT status, delivery_attempts, evaluation_seal_json,
                    evaluation_seal_sha256, evaluation_seal_version,
                    deterministic_seed_hex
             FROM interaction_derived_event_outbox
             WHERE occurrence_id = 'schema-35-acknowledged-occurrence'",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .expect("read acknowledged schema-35 outbox row");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*)
                 FROM interaction_derived_event_quarantines
                 WHERE occurrence_id = 'schema-35-acknowledged-occurrence'",
                [],
                |row| row.get::<_, u64>(0),
            )
            .expect("check acknowledged quarantine absence"),
        0
    );
    LegacyOutboxSnapshot {
        quarantines,
        acknowledged_status,
    }
}

fn empty_interaction_policy() -> InteractionPolicySnapshot {
    InteractionPolicySnapshot {
        module_plan_sha256: None,
        rule_sets: Vec::new(),
    }
}

fn sha256(value: &str) -> Sha256Digest {
    Sha256Digest::parse(value.to_owned()).expect("synthetic SHA-256")
}

fn verify_cutpoint_upgrade(cutpoint: u32, latest: u32) -> Vec<(String, String, String)> {
    let root = tempdir().expect("temporary cutpoint root");
    let database_dir = root.path().join("db");
    fs::create_dir_all(&database_dir).expect("create cutpoint database directory");
    let database_path = database_dir.join("lorepia.sqlite3");
    {
        let mut connection = Connection::open(&database_path).expect("open cutpoint fixture");
        connection
            .pragma_update(None, "foreign_keys", true)
            .expect("enable fixture foreign keys");
        apply_through(&mut connection, cutpoint);
    }

    let storage = Storage::open(root.path())
        .unwrap_or_else(|error| panic!("upgrade schema {cutpoint} to {latest}: {error}"));
    assert_eq!(
        storage
            .schema_version()
            .unwrap_or_else(|error| panic!("read upgraded schema {cutpoint}: {error}")),
        latest
    );
    drop(storage);

    let upgraded_database_path = resolved_database_path(root.path(), &database_path, cutpoint);
    validate_latest_database(&upgraded_database_path, latest, cutpoint);
    let before_reopen = schema_inventory(&upgraded_database_path);
    drop(
        Storage::open(root.path())
            .unwrap_or_else(|error| panic!("reopen upgraded schema {cutpoint}: {error}")),
    );
    let reopened_database_path = resolved_database_path(root.path(), &database_path, cutpoint);
    validate_latest_database(&reopened_database_path, latest, cutpoint);
    assert_eq!(
        schema_inventory(&reopened_database_path),
        before_reopen,
        "reopen changed schema objects for cutpoint {cutpoint}"
    );
    before_reopen
}

fn resolved_database_path(root: &Path, canonical: &Path, cutpoint: u32) -> PathBuf {
    let cutover_dir = root.join("db/schema-cutover");
    if !cutover_dir.exists() {
        return canonical.to_owned();
    }

    let mut committed_generations = fs::read_dir(&cutover_dir)
        .unwrap_or_else(|error| {
            panic!("read cutover generations after upgrading cutpoint {cutpoint}: {error}")
        })
        .map(|entry| {
            let entry = entry.unwrap_or_else(|error| {
                panic!("read cutover generation after upgrading cutpoint {cutpoint}: {error}")
            });
            let generation_dir = entry.path();
            assert!(
                generation_dir.is_dir(),
                "cutover entry is not a directory after upgrading cutpoint {cutpoint}"
            );
            assert!(
                generation_dir.join("generation-committed.json").is_file(),
                "cutover generation is not committed after upgrading cutpoint {cutpoint}"
            );
            let manifest_path = generation_dir.join("generation-manifest.json");
            let manifest: serde_json::Value =
                serde_json::from_slice(&fs::read(&manifest_path).unwrap_or_else(|error| {
                    panic!(
                        "read committed generation manifest after upgrading cutpoint \
                         {cutpoint}: {error}"
                    )
                }))
                .unwrap_or_else(|error| {
                    panic!(
                        "parse committed generation manifest after upgrading cutpoint \
                     {cutpoint}: {error}"
                    )
                });
            let activation_sequence =
                manifest["activation_sequence"].as_u64().unwrap_or_else(|| {
                    panic!(
                        "committed generation activation sequence is invalid after upgrading \
                     cutpoint {cutpoint}"
                    )
                });
            let relative_path = manifest["active_database_relative_path"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "committed generation database path is invalid after upgrading \
                         cutpoint {cutpoint}"
                    )
                });
            (activation_sequence, root.join(relative_path))
        })
        .collect::<Vec<_>>();
    committed_generations.sort_by_key(|(activation_sequence, _)| *activation_sequence);
    committed_generations
        .last()
        .map_or_else(|| canonical.to_owned(), |(_, path)| path.to_owned())
}

fn apply_through(connection: &mut Connection, target: u32) {
    let target = usize::try_from(target).expect("schema cutpoint fits usize");
    for (index, migration) in MIGRATIONS.iter().enumerate().take(target) {
        let version = u32::try_from(index + 1).expect("schema version fits u32");
        let transaction = connection.transaction().expect("begin fixture migration");
        transaction
            .execute_batch(migration)
            .unwrap_or_else(|error| panic!("apply fixture migration {version}: {error}"));
        transaction
            .execute(
                "INSERT OR IGNORE INTO schema_migrations(version, applied_at)
                 VALUES (?1, ?2)",
                params![version, MIGRATION_TIME],
            )
            .unwrap_or_else(|error| panic!("record fixture migration {version}: {error}"));
        assert!(
            transaction
                .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
                .optional()
                .expect("check fixture foreign keys")
                .is_none(),
            "fixture migration {version} produced a foreign-key violation"
        );
        transaction.commit().expect("commit fixture migration");
    }
}

fn validate_latest_database(database_path: &Path, latest: u32, cutpoint: u32) {
    let connection = Connection::open(database_path).expect("open upgraded database");
    let versions = connection
        .prepare("SELECT version FROM schema_migrations ORDER BY version")
        .expect("prepare durable registry")
        .query_map([], |row| row.get::<_, u32>(0))
        .expect("query durable registry")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect durable registry");
    assert_eq!(
        versions,
        (1..=latest).collect::<Vec<_>>(),
        "schema registry is not contiguous after cutpoint {cutpoint}"
    );
    assert!(
        connection
            .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
            .optional()
            .expect("check upgraded foreign keys")
            .is_none(),
        "foreign-key violation after upgrading cutpoint {cutpoint}"
    );
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .expect("check upgraded database integrity"),
        "ok",
        "integrity check failed after upgrading cutpoint {cutpoint}"
    );
}

fn schema_inventory(database_path: &Path) -> Vec<(String, String, String)> {
    let connection = Connection::open(database_path).expect("open schema inventory");
    let mut statement = connection
        .prepare(
            "SELECT type, name, COALESCE(sql, '')
             FROM sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .expect("prepare schema inventory");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query schema inventory")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect schema inventory")
}
