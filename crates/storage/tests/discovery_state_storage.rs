//! Migration and transactional persistence tests for the durable discovery
//! storage module.

use lorepia_domain::{
    CredentialRef, DiscoverySessionId, GenerationPresetId, ModelRouteId, ProviderConnectionId,
    ProviderTemplateId,
    discovery::{
        DiscoveryActionId, DiscoveryApprovalId, DiscoveryCommitAttemptId, DiscoveryCommitPlan,
        DiscoveryCompensationStatus, DiscoveryCompensationStep, DiscoveryCompensationTarget,
        DiscoveryPreviousSelection, SanitizedDiscoveryInput,
    },
};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

#[path = "../src/discovery.rs"]
mod discovery;

use discovery::{
    CompletedDiscoveryOperation, DISCOVERY_STATE_MACHINE_MIGRATION, DiscoveryRecoveryDisposition,
    DiscoveryStorageError, DurableDiscoveryEffect, DurableDiscoveryTransition,
    DurableOperationOutcome, NewDiscoveryApproval, NewDiscoveryCommitAttempt,
    NewDiscoveryCompensationStep, NewDiscoveryOperation, NewDiscoverySession,
    PersistDiscoveryTransition, insert_discovery_session, list_pending_discovery_events,
    list_unfinished_discovery_operations, load_discovery_failure, mark_discovery_event_delivered,
    mark_discovery_operation_started, persist_discovery_transition,
};

const MIGRATION_0004: &str = include_str!("../migrations/0004_provider_catalog.sql");
const MIGRATION_0026: &str =
    include_str!("../migrations/0026_provider_discovery_native_no_effect.sql");
const MIGRATION_0027: &str =
    include_str!("../migrations/0027_provider_discovery_native_attestations.sql");
const NOW: &str = "2026-07-31T00:00:00Z";
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const REVIEW_JSON: &str = r#"{"sha256":"abb8bda2360b026a390418abb0222c0f098e0e3d59579968011dbc9036454125","graph_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","changes":[],"unresolved_question_count":0,"warning_count":0}"#;
const REVIEW_GRANT: &str = r#"{"kind":"review","review_sha256":"abb8bda2360b026a390418abb0222c0f098e0e3d59579968011dbc9036454125","graph_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;

fn migrated_connection() -> Connection {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.execute_batch(MIGRATION_0004).unwrap();
    apply_migration_0005(&mut connection);
    connection.execute_batch(MIGRATION_0026).unwrap();
    connection.execute_batch(MIGRATION_0027).unwrap();
    connection
}

fn apply_migration_0005(connection: &mut Connection) {
    let transaction = connection.transaction().unwrap();
    transaction
        .execute_batch(DISCOVERY_STATE_MACHINE_MIGRATION)
        .unwrap();
    let foreign_key_violation = transaction
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()
        .unwrap();
    assert!(foreign_key_violation.is_none());
    transaction.commit().unwrap();
}

fn new_session(connection: &mut Connection, id: &str) {
    let input = serde_json::from_str::<SanitizedDiscoveryInput>(
        r#"{"connection_id":"connection-input","display_name":"Test Provider","site_url":"https://provider.example/","docs_url":null,"credential_ref":null,"preferred_assistant":null,"local_network_mode":false,"supplied_evidence_ids":[]}"#,
    )
    .unwrap();
    insert_discovery_session(
        connection,
        &NewDiscoverySession {
            id,
            input: &input,
            created_at: NOW,
        },
    )
    .unwrap();
}

fn sha256(json: &str) -> String {
    hex::encode(Sha256::digest(json.as_bytes()))
}

fn typed_plan_json(
    attempt_id: &str,
    session_id: &str,
    expected_revision: u64,
    connection_id: &str,
    credential_ref: Option<&str>,
    previous_selection: DiscoveryPreviousSelection,
) -> String {
    serde_json::to_string(&DiscoveryCommitPlan {
        attempt_id: DiscoveryCommitAttemptId::parse(attempt_id).unwrap(),
        session_id: DiscoverySessionId::from(session_id),
        expected_revision,
        manifest_sha256: HASH_A.to_owned(),
        graph_sha256: HASH_A.to_owned(),
        template_id: ProviderTemplateId::from("template-1"),
        template_version: 1,
        connection_id: ProviderConnectionId::from(connection_id),
        model_route_ids: vec![ModelRouteId::from("route-1")],
        credential_ref: credential_ref.map(|value| CredentialRef(value.to_owned())),
        credential_approval_id: credential_ref
            .map(|_| DiscoveryApprovalId::parse("credential-approval").unwrap()),
        review_sha256: HASH_A.to_owned(),
        previous_selection,
    })
    .unwrap()
}

fn typed_step_json(action_id: &str, ordinal: u32, target: DiscoveryCompensationTarget) -> String {
    serde_json::to_string(&DiscoveryCompensationStep {
        action_id: DiscoveryActionId::parse(action_id).unwrap(),
        ordinal,
        kind: target.kind(),
        target,
        status: DiscoveryCompensationStatus::Pending,
    })
    .unwrap()
}

fn transition<'a>(
    session_id: &'a str,
    expected_revision: u64,
    state: &'a str,
    event_id: &'a str,
    action_id: &'a str,
    request_sha256: &'a str,
) -> DurableDiscoveryTransition<'a> {
    let effect = effect_for_destination_state(state);
    DurableDiscoveryTransition {
        session_id,
        expected_revision,
        resulting_revision: expected_revision + 1,
        event_sequence: expected_revision + 1,
        next_event_sequence: expected_revision + 2,
        state,
        draft_json: None,
        review_diff_json: None,
        error_json: None,
        recovery_json: None,
        unknown_operation: None,
        manifest_sha256: None,
        commit_plan_sha256: None,
        commit_attempt_id: None,
        committed_connection_id: None,
        cancellation_pending: false,
        event_id,
        event_version: 1,
        event_json: r#"{"version":1}"#,
        effect,
        action_id,
        action_kind: "test_action",
        action_approval_id: None,
        request_sha256,
        response_json: r#"{"accepted":true}"#,
        receipt_outcome: "applied",
        audit_kind: "transition_applied",
        audit_summary_key: "discovery.audit.transition_applied",
        occurred_at: NOW,
        operation: operation_for_effect(effect, event_id),
        completed_operation: None,
        approval: None,
        commit: None,
    }
}

fn effect_for_destination_state(state: &str) -> DurableDiscoveryEffect {
    match state {
        "resolving_known_provider" => DurableDiscoveryEffect::ResolveKnownProvider,
        "fetching_documents" => DurableDiscoveryEffect::FetchDocuments,
        "extracting_evidence" => DurableDiscoveryEffect::ExtractEvidence,
        "building_deterministic_manifest_draft" => {
            DurableDiscoveryEffect::BuildDeterministicManifestDraft
        }
        "building_assistant_manifest_draft" => DurableDiscoveryEffect::BuildAssistantManifestDraft,
        "validating_manifest" => DurableDiscoveryEffect::ValidateManifest,
        "listing_models" => DurableDiscoveryEffect::ListModels,
        "probing_capabilities" => DurableDiscoveryEffect::ProbeCapabilities,
        "committing" => DurableDiscoveryEffect::CommitAtomically,
        "compensating" => DurableDiscoveryEffect::RunCompensation,
        _ => DurableDiscoveryEffect::None,
    }
}

fn operation_for_effect(
    effect: DurableDiscoveryEffect,
    operation_id: &str,
) -> Option<NewDiscoveryOperation<'_>> {
    let (operation_kind, side_effect_class) = match effect {
        DurableDiscoveryEffect::ResolveKnownProvider => ("resolve_known_provider", "read_only"),
        DurableDiscoveryEffect::FetchDocuments => ("fetch_documents", "read_only"),
        DurableDiscoveryEffect::ExtractEvidence => ("extract_evidence", "read_only"),
        DurableDiscoveryEffect::BuildDeterministicManifestDraft => {
            ("build_deterministic_manifest_draft", "local_deterministic")
        }
        DurableDiscoveryEffect::BuildAssistantManifestDraft => {
            ("build_assistant_manifest_draft", "billable_external")
        }
        DurableDiscoveryEffect::ValidateManifest => ("validate_manifest", "read_only"),
        DurableDiscoveryEffect::ListModels => ("list_models", "read_only"),
        DurableDiscoveryEffect::ProbeCapabilities => ("probe_capabilities", "billable_external"),
        DurableDiscoveryEffect::CommitAtomically => ("atomic_commit", "persistent"),
        DurableDiscoveryEffect::RunCompensation => ("compensation", "persistent"),
        _ => return None,
    };
    Some(NewDiscoveryOperation {
        id: operation_id,
        operation_kind,
        side_effect_class,
        approval_id: None,
        approval_grant_sha256: None,
    })
}

fn insert_started_atomic_operation(
    connection: &Connection,
    session_id: &str,
    operation_id: &str,
    action_id: &str,
    request_sha256: &str,
) {
    connection
        .execute(
            "INSERT INTO provider_discovery_operations (
                 id, session_id, operation_kind, side_effect_class, status,
                 action_id, expected_revision, request_sha256, approval_id,
                 approval_grant_sha256, started_at, finished_at, created_at, updated_at
             ) VALUES (
                 ?1, ?2, 'atomic_commit', 'persistent', 'started',
                 ?3, 0, ?4, NULL, NULL, ?5, NULL, ?5, ?5
             )",
            params![operation_id, session_id, action_id, request_sha256, NOW],
        )
        .unwrap();
}

#[test]
fn v26_upgrade_preserves_legacy_interrupt_without_fabricating_attestation() {
    let root = tempfile::tempdir().unwrap();
    let database_path = root.path().join("provider-discovery-v26.sqlite3");
    {
        let mut connection = Connection::open(&database_path).unwrap();
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        connection.execute_batch(MIGRATION_0004).unwrap();
        apply_migration_0005(&mut connection);
        connection.execute_batch(MIGRATION_0026).unwrap();
        new_session(&mut connection, "legacy-v26-interrupt");
        insert_started_atomic_operation(
            &connection,
            "legacy-v26-interrupt",
            "legacy-v26-operation",
            "legacy-v26-action",
            HASH_A,
        );
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'interrupted', finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params!["legacy-v26-operation", NOW],
            )
            .expect("version 26 allowed the untyped native exception");

        connection.execute_batch(MIGRATION_0027).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*)
                     FROM provider_discovery_native_no_effect_attestations",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .unwrap(),
            0,
            "migration cannot fabricate evidence for a historical transition"
        );
        new_session(&mut connection, "post-v27-interrupt");
        insert_started_atomic_operation(
            &connection,
            "post-v27-interrupt",
            "post-v27-operation",
            "post-v27-action",
            HASH_B,
        );
        assert!(
            connection
                .execute(
                    "UPDATE provider_discovery_operations
                     SET status = 'interrupted', finished_at = ?2, updated_at = ?2
                     WHERE id = ?1",
                    params!["post-v27-operation", NOW],
                )
                .is_err()
        );
    }

    let reopened = Connection::open(database_path).unwrap();
    assert_eq!(
        reopened
            .query_row(
                "SELECT status FROM provider_discovery_operations
                 WHERE id = 'legacy-v26-operation'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "interrupted"
    );
    assert_eq!(
        reopened
            .query_row(
                "SELECT COUNT(*)
                 FROM provider_discovery_native_no_effect_attestations",
                [],
                |row| row.get::<_, u32>(0),
            )
            .unwrap(),
        0
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn v4_upgrade_redacts_evidence_and_archives_unverifiable_work_without_retrying() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.execute_batch(MIGRATION_0004).unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, created_at, updated_at
             ) VALUES
                 ('fetch', 'fetching_documents',
                    '{\"site_url\":\"https://alice:password@provider.example/private/sk-proj-site?api_key=secret#section\",\"docs_url\":\"https://bob:token@provider.example/docs/sk-proj-doc?token=secret#fragment\"}',
                    ?1, ?1),
                 ('build', 'building_manifest_draft', '{}', ?1, ?1),
                 ('probe', 'probing_capabilities', '{}', ?1, ?1),
                 ('commit', 'committing', '{}', ?1, ?1),
                 ('legacy-ready', 'ready', '{}', ?1, ?1),
                 ('review', 'awaiting_review', '{}', ?1, ?1)",
            [NOW],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, created_at, updated_at
             ) VALUES (NULL, 'draft', '{}', ?1, ?1)",
            [NOW],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_evidence (
                 id, session_id, kind, source_url, content_sha256,
                 extracted_json, fetched_at
             ) VALUES ('evidence-1', 'fetch', 'openapi',
                 'https://bob:token@provider.example/openapi/sk-proj-evidence?api_key=secret#section',
                 ?1, '{\"api_key\":\"legacy-evidence-secret\"}', ?2)",
            params![HASH_A, NOW],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE provider_discovery_sessions
             SET sanitized_input_json =
                    json_set(
                        sanitized_input_json,
                        '$.credential_ref', 'sk-proj-legacy-secret',
                        '$.unexpected_secret', 'legacy-input-secret'
                    ),
                 draft_json =
                    '{\"authorization\":\"Bearer legacy-draft-secret\"}'
             WHERE id = 'fetch'",
            [],
        )
        .unwrap();

    apply_migration_0005(&mut connection);

    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT id, state, revision, recovery_json, unknown_operation
                 FROM provider_discovery_sessions
                 ORDER BY id",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(
        rows,
        vec![
            (
                "legacy-v5-session-0000000000000001".to_owned(),
                "failed".to_owned(),
                0,
                None,
                None
            ),
            (
                "legacy-v5-session-0000000000000002".to_owned(),
                "failed".to_owned(),
                0,
                None,
                None
            ),
            (
                "legacy-v5-session-0000000000000003".to_owned(),
                "failed".to_owned(),
                0,
                None,
                None
            ),
            (
                "legacy-v5-session-0000000000000004".to_owned(),
                "failed".to_owned(),
                0,
                None,
                None
            ),
            (
                "legacy-v5-session-0000000000000005".to_owned(),
                "failed".to_owned(),
                0,
                None,
                None
            ),
            (
                "legacy-v5-session-0000000000000006".to_owned(),
                "failed".to_owned(),
                0,
                None,
                None
            ),
            (
                "legacy-v5-session-0000000000000007".to_owned(),
                "failed".to_owned(),
                0,
                None,
                None
            ),
        ]
    );
    assert_eq!(
        load_discovery_failure(&connection, "legacy-v5-session-0000000000000004")
            .unwrap()
            .unwrap()
            .code,
        "legacy.discovery_failure"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_evidence",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert!(
        connection
            .query_row(
                "SELECT draft_json IS NULL
                 FROM provider_discovery_sessions
                 WHERE id = 'legacy-v5-session-0000000000000001'",
                [],
                |row| row.get::<_, bool>(0)
            )
            .unwrap()
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT
                     json_extract(sanitized_input_json, '$.site_url'),
                     json_extract(sanitized_input_json, '$.docs_url')
                 FROM provider_discovery_sessions
                 WHERE id = 'legacy-v5-session-0000000000000001'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            )
            .unwrap(),
        (
            "https://redacted.invalid/".to_owned(),
            "https://redacted.invalid/".to_owned()
        )
    );
    let sanitized_input = connection
        .query_row(
            "SELECT sanitized_input_json
             FROM provider_discovery_sessions
             WHERE id = 'legacy-v5-session-0000000000000001'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert!(!sanitized_input.contains("legacy-input-secret"));
    assert!(!sanitized_input.contains("sk-proj-legacy-secret"));
    assert!(!sanitized_input.contains("unexpected_secret"));
    assert!(!sanitized_input.contains("alice"));
    assert!(!sanitized_input.contains("password"));
    assert!(!sanitized_input.contains("sk-proj-site"));
    assert!(
        connection
            .execute(
                "INSERT INTO provider_discovery_sessions (
                     id, state, revision, next_event_sequence,
                     sanitized_input_json, cancellation_pending,
                     redaction_version, created_at, updated_at
                 ) VALUES (
                     'unsafe-input-query', 'draft', 0, 1,
                     '{\"connection_id\":\"unsafe-query-connection\",\"display_name\":\"Unsafe Query\",\"site_url\":\"https://provider.example/?token=secret\"}',
                     0, 1, ?1, ?1
                 )",
                [NOW],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO provider_discovery_sessions (
                     id, state, revision, next_event_sequence,
                     sanitized_input_json, cancellation_pending,
                     redaction_version, created_at, updated_at
                 ) VALUES (
                     'mismatched-credential-slot', 'draft', 0, 1,
                     '{\"connection_id\":\"selected-connection\",\"display_name\":\"Selected Provider\",\"site_url\":\"https://provider.example/\",\"credential_ref\":\"different-slot\"}',
                     0, 1, ?1, ?1
                 )",
                [NOW],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO provider_discovery_sessions (
                     id, state, revision, next_event_sequence,
                     sanitized_input_json, cancellation_pending,
                     redaction_version, created_at, updated_at
                 ) VALUES (
                     'unsafe-input-fragment', 'draft', 0, 1,
                     '{\"connection_id\":\"unsafe-fragment-connection\",\"display_name\":\"Unsafe Fragment\",\"site_url\":\"https://provider.example/\",\"docs_url\":\"https://provider.example/docs#secret\"}',
                     0, 1, ?1, ?1
                 )",
                [NOW],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO provider_discovery_evidence (
                     id, session_id, kind, source_url, content_sha256,
                     extracted_json, redaction_version, fetched_at
                 ) VALUES (
                     'unsafe-query',
                     'legacy-v5-session-0000000000000001', 'openapi',
                     'https://provider.example/openapi.json?token=secret',
                     ?1, '{}', 1, ?2
                 )",
                params![HASH_A, NOW],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO provider_discovery_evidence (
                     id, session_id, kind, source_url, content_sha256,
                     extracted_json, redaction_version, fetched_at
                 ) VALUES (
                     'unsafe-fragment',
                     'legacy-v5-session-0000000000000001', 'openapi',
                     'https://provider.example/openapi.json#secret',
                     ?1, '{}', 1, ?2
                 )",
                params![HASH_A, NOW],
            )
            .is_err()
    );
    connection
        .execute(
            "INSERT INTO provider_discovery_evidence (
                 id, session_id, kind, source_url, content_sha256,
                 extracted_json, redaction_version, fetched_at
             ) VALUES (
                 'post-migration-evidence',
                 'legacy-v5-session-0000000000000001', 'open_api',
                 'https://provider.example/openapi.json',
                 ?1, '{}', 1, ?2
             )",
            params![HASH_A, NOW],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_evidence
                 SET extracted_json = '{\"retargeted\":true}'
                 WHERE id = 'post-migration-evidence'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM provider_discovery_evidence
                 WHERE id = 'post-migration-evidence'",
                [],
            )
            .is_err()
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_event_outbox",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_audit_log",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        7
    );
    let foreign_key_violation = connection
        .query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
        .optional()
        .unwrap();
    assert!(foreign_key_violation.is_none());
}

#[test]
fn cas_outbox_receipt_and_audit_are_one_idempotent_transaction() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-1");
    let first = transition(
        "session-1",
        0,
        "resolving_known_provider",
        "event-1",
        "action-1",
        HASH_A,
    );

    assert_eq!(
        persist_discovery_transition(&mut connection, &first).unwrap(),
        PersistDiscoveryTransition::Applied {
            revision: 1,
            event_sequence: 1
        }
    );
    assert_eq!(
        persist_discovery_transition(&mut connection, &first).unwrap(),
        PersistDiscoveryTransition::Replayed {
            revision: 1,
            event_sequence: 1,
            response_json: r#"{"accepted":true}"#.to_owned()
        }
    );

    let counts = connection
        .query_row(
            "SELECT
                 (SELECT COUNT(*) FROM provider_discovery_event_outbox),
                 (SELECT COUNT(*) FROM provider_discovery_action_receipts),
                 (SELECT COUNT(*) FROM provider_discovery_audit_log)",
            [],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(counts, (1, 1, 2));

    let conflicting_replay = DurableDiscoveryTransition {
        request_sha256: HASH_B,
        ..first.clone()
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &conflicting_replay),
        Err(DiscoveryStorageError::IdempotencyConflict { .. })
    ));

    new_session(&mut connection, "session-2");
    let cross_session_reuse = transition(
        "session-2",
        0,
        "resolving_known_provider",
        "event-2",
        "action-1",
        HASH_A,
    );
    assert!(matches!(
        persist_discovery_transition(&mut connection, &cross_session_reuse),
        Err(DiscoveryStorageError::IdempotencyConflict { .. })
    ));

    let stale = transition(
        "session-1",
        0,
        "fetching_documents",
        "event-stale",
        "action-stale",
        HASH_B,
    );
    assert!(matches!(
        persist_discovery_transition(&mut connection, &stale),
        Err(DiscoveryStorageError::RevisionConflict {
            expected: 0,
            actual: 1
        })
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn failed_outbox_insert_rolls_back_the_state_cas_and_commit_plan() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-rollback");
    let first = transition(
        "session-rollback",
        0,
        "awaiting_review",
        "event-shared",
        "action-first",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &first).unwrap();

    let graph_step_json = typed_step_json(
        "compensate-graph-1",
        1,
        DiscoveryCompensationTarget::RemoveConnectionGraph {
            connection_id: ProviderConnectionId::from("connection-1"),
        },
    );
    let selection_step_json = typed_step_json(
        "compensate-selection-1",
        0,
        DiscoveryCompensationTarget::RestorePreviousSelection {
            previous_selection: DiscoveryPreviousSelection::None,
        },
    );
    let steps = [
        NewDiscoveryCompensationStep {
            id: "step-graph-1",
            ordinal: 1,
            action_id: "compensate-graph-1",
            step_kind: "remove_connection_graph",
            step_json: &graph_step_json,
        },
        NewDiscoveryCompensationStep {
            id: "step-selection-1",
            ordinal: 0,
            action_id: "compensate-selection-1",
            step_kind: "restore_previous_selection",
            step_json: &selection_step_json,
        },
    ];
    let plan_json = typed_plan_json(
        "attempt-1",
        "session-rollback",
        1,
        "connection-1",
        None,
        DiscoveryPreviousSelection::None,
    );
    let plan_sha256 = sha256(&plan_json);
    let commit = NewDiscoveryCommitAttempt {
        id: "attempt-1",
        attempt_number: 1,
        plan_sha256: &plan_sha256,
        plan_json: &plan_json,
        reuse_existing: false,
        compensation_steps: &steps,
    };
    let second = DurableDiscoveryTransition {
        commit_plan_sha256: Some(&plan_sha256),
        commit_attempt_id: Some("attempt-1"),
        commit: Some(commit),
        ..transition(
            "session-rollback",
            1,
            "committing",
            // Duplicate event ID forces a failure after the session UPDATE and
            // commit-attempt INSERT.
            "event-shared",
            "action-second",
            HASH_B,
        )
    };

    assert!(matches!(
        persist_discovery_transition(&mut connection, &second),
        Err(DiscoveryStorageError::Database(_))
    ));
    let state = connection
        .query_row(
            "SELECT state, revision, next_event_sequence
             FROM provider_discovery_sessions
             WHERE id = 'session-rollback'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(state, ("awaiting_review".to_owned(), 1, 2));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_commit_attempts",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_operations
                 WHERE session_id = 'session-rollback'
                   AND id = 'event-shared'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_compensation_steps",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn failed_transition_rolls_back_operation_completion_with_the_session_state() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-completion-rollback");
    let resolving = transition(
        "session-completion-rollback",
        0,
        "resolving_known_provider",
        "shared-operation-event",
        "action-operation-start",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &resolving).unwrap();
    mark_discovery_operation_started(&mut connection, "shared-operation-event", NOW).unwrap();

    let completion = DurableDiscoveryTransition {
        action_kind: "known_provider_candidates_resolved",
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "shared-operation-event",
            outcome: DurableOperationOutcome::Succeeded,
        }),
        ..transition(
            "session-completion-rollback",
            1,
            "awaiting_template_selection",
            // Duplicate outbox ID fails after the operation/session updates.
            "shared-operation-event",
            "action-operation-complete",
            HASH_B,
        )
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &completion),
        Err(DiscoveryStorageError::Database(_))
    ));
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM provider_discovery_operations
                 WHERE id = 'shared-operation-event'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "started"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT state, revision FROM provider_discovery_sessions
                 WHERE id = 'session-completion-rollback'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            )
            .unwrap(),
        ("resolving_known_provider".to_owned(), 1)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn commit_plan_and_reverse_compensation_steps_are_prepared_atomically() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-commit");
    let review = transition(
        "session-commit",
        0,
        "awaiting_review",
        "event-review",
        "action-review-ready",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &review).unwrap();

    let previous_selection = DiscoveryPreviousSelection::RouteAndPreset {
        selected_provider_profile_id: None,
        model_route_id: ModelRouteId::from("previous-route"),
        generation_preset_id: GenerationPresetId::from("previous-preset"),
    };
    let credential_step_json = typed_step_json(
        "remove-credential",
        2,
        DiscoveryCompensationTarget::RemoveCredentialSlot {
            connection_id: ProviderConnectionId::from("connection-1"),
            credential_ref: CredentialRef("connection-1".to_owned()),
        },
    );
    let graph_step_json = typed_step_json(
        "remove-graph",
        1,
        DiscoveryCompensationTarget::RemoveConnectionGraph {
            connection_id: ProviderConnectionId::from("connection-1"),
        },
    );
    let selection_step_json = typed_step_json(
        "restore-selection",
        0,
        DiscoveryCompensationTarget::RestorePreviousSelection {
            previous_selection: previous_selection.clone(),
        },
    );
    let steps = [
        NewDiscoveryCompensationStep {
            id: "step-credential",
            ordinal: 2,
            action_id: "remove-credential",
            step_kind: "remove_credential_slot",
            step_json: &credential_step_json,
        },
        NewDiscoveryCompensationStep {
            id: "step-graph",
            ordinal: 1,
            action_id: "remove-graph",
            step_kind: "remove_connection_graph",
            step_json: &graph_step_json,
        },
        NewDiscoveryCompensationStep {
            id: "step-selection",
            ordinal: 0,
            action_id: "restore-selection",
            step_kind: "restore_previous_selection",
            step_json: &selection_step_json,
        },
    ];
    let plan_json = typed_plan_json(
        "attempt-commit",
        "session-commit",
        1,
        "connection-1",
        Some("connection-1"),
        previous_selection,
    );
    let plan_sha256 = sha256(&plan_json);

    let retargeted_graph_json = typed_step_json(
        "remove-graph",
        1,
        DiscoveryCompensationTarget::RemoveConnectionGraph {
            connection_id: ProviderConnectionId::from("different-connection"),
        },
    );
    let retargeted_steps = [
        NewDiscoveryCompensationStep {
            id: "step-credential",
            ordinal: 2,
            action_id: "remove-credential",
            step_kind: "remove_credential_slot",
            step_json: &credential_step_json,
        },
        NewDiscoveryCompensationStep {
            id: "step-graph",
            ordinal: 1,
            action_id: "remove-graph",
            step_kind: "remove_connection_graph",
            step_json: &retargeted_graph_json,
        },
        NewDiscoveryCompensationStep {
            id: "step-selection",
            ordinal: 0,
            action_id: "restore-selection",
            step_kind: "restore_previous_selection",
            step_json: &selection_step_json,
        },
    ];
    let retargeted_commit = NewDiscoveryCommitAttempt {
        id: "attempt-commit",
        attempt_number: 1,
        plan_sha256: &plan_sha256,
        plan_json: &plan_json,
        reuse_existing: false,
        compensation_steps: &retargeted_steps,
    };
    let retargeted = DurableDiscoveryTransition {
        commit_plan_sha256: Some(&plan_sha256),
        commit_attempt_id: Some("attempt-commit"),
        commit: Some(retargeted_commit),
        ..transition(
            "session-commit",
            1,
            "committing",
            "event-retargeted-commit",
            "action-retargeted-commit",
            HASH_B,
        )
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &retargeted),
        Err(DiscoveryStorageError::InvalidTransition(
            "compensation target does not match the approved commit plan"
        ))
    ));

    let commit = NewDiscoveryCommitAttempt {
        id: "attempt-commit",
        attempt_number: 1,
        plan_sha256: &plan_sha256,
        plan_json: &plan_json,
        reuse_existing: false,
        compensation_steps: &steps,
    };
    let committing = DurableDiscoveryTransition {
        commit_plan_sha256: Some(&plan_sha256),
        commit_attempt_id: Some("attempt-commit"),
        commit: Some(commit),
        ..transition(
            "session-commit",
            1,
            "committing",
            "event-commit",
            "action-commit",
            HASH_B,
        )
    };

    persist_discovery_transition(&mut connection, &committing).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT phase FROM provider_discovery_commit_attempts
                 WHERE id = 'attempt-commit'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "prepared"
    );
    let ordinals = {
        let mut statement = connection
            .prepare(
                "SELECT ordinal
                 FROM provider_discovery_compensation_steps
                 WHERE commit_attempt_id = 'attempt-commit'
                 ORDER BY ordinal DESC",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, u32>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(ordinals, vec![2, 1, 0]);
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_commit_attempts
                 SET plan_json = '{}'
                 WHERE id = 'attempt-commit'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_compensation_steps
                 SET step_json = '{}'
                 WHERE id = 'step-graph'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM provider_discovery_compensation_steps
                 WHERE id = 'step-graph'",
                [],
            )
            .is_err()
    );

    let cancel_commit = DurableDiscoveryTransition {
        action_kind: "cancel",
        effect: DurableDiscoveryEffect::RequestCancellation,
        operation: None,
        commit_plan_sha256: Some(&plan_sha256),
        commit_attempt_id: Some("attempt-commit"),
        cancellation_pending: true,
        ..transition(
            "session-commit",
            2,
            "committing",
            "event-cancel-commit",
            "action-cancel-commit",
            HASH_A,
        )
    };
    persist_discovery_transition(&mut connection, &cancel_commit).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_commit_attempts
                 WHERE session_id = 'session-commit'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_operations
                 WHERE session_id = 'session-commit'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn explicitly_restarted_unstarted_commit_reuses_plan_and_attempt() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-commit-restart");
    let review = transition(
        "session-commit-restart",
        0,
        "awaiting_review",
        "event-review-restart",
        "action-review-restart",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &review).unwrap();

    let graph_step_json = typed_step_json(
        "compensate-graph-restart",
        1,
        DiscoveryCompensationTarget::RemoveConnectionGraph {
            connection_id: ProviderConnectionId::from("connection-restart"),
        },
    );
    let selection_step_json = typed_step_json(
        "compensate-selection-restart",
        0,
        DiscoveryCompensationTarget::RestorePreviousSelection {
            previous_selection: DiscoveryPreviousSelection::None,
        },
    );
    let steps = [
        NewDiscoveryCompensationStep {
            id: "step-graph-restart",
            ordinal: 1,
            action_id: "compensate-graph-restart",
            step_kind: "remove_connection_graph",
            step_json: &graph_step_json,
        },
        NewDiscoveryCompensationStep {
            id: "step-selection-restart",
            ordinal: 0,
            action_id: "compensate-selection-restart",
            step_kind: "restore_previous_selection",
            step_json: &selection_step_json,
        },
    ];
    let plan_json = typed_plan_json(
        "attempt-restart",
        "session-commit-restart",
        1,
        "connection-restart",
        None,
        DiscoveryPreviousSelection::None,
    );
    let plan_sha256 = sha256(&plan_json);
    let commit = NewDiscoveryCommitAttempt {
        id: "attempt-restart",
        attempt_number: 1,
        plan_sha256: &plan_sha256,
        plan_json: &plan_json,
        reuse_existing: false,
        compensation_steps: &steps,
    };
    let committing = DurableDiscoveryTransition {
        commit_plan_sha256: Some(&plan_sha256),
        commit_attempt_id: Some("attempt-restart"),
        commit: Some(commit),
        ..transition(
            "session-commit-restart",
            1,
            "committing",
            "operation-commit-original",
            "action-commit-original",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &committing).unwrap();

    let interrupted = DurableDiscoveryTransition {
        action_kind: "interrupt",
        effect: DurableDiscoveryEffect::None,
        operation: None,
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "operation-commit-original",
            outcome: DurableOperationOutcome::Interrupted,
        }),
        recovery_json: Some(r#"{"interrupted_state":"committing","operation":"atomic_commit"}"#),
        commit_plan_sha256: Some(&plan_sha256),
        commit_attempt_id: Some("attempt-restart"),
        ..transition(
            "session-commit-restart",
            2,
            "interrupted",
            "event-commit-interrupted",
            "action-commit-interrupted",
            HASH_A,
        )
    };
    persist_discovery_transition(&mut connection, &interrupted).unwrap();

    let restarted_commit = NewDiscoveryCommitAttempt {
        id: "attempt-restart",
        attempt_number: 1,
        plan_sha256: &plan_sha256,
        plan_json: &plan_json,
        reuse_existing: true,
        compensation_steps: &[],
    };
    let restarted = DurableDiscoveryTransition {
        action_kind: "restart_interrupted",
        commit_plan_sha256: Some(&plan_sha256),
        commit_attempt_id: Some("attempt-restart"),
        commit: Some(restarted_commit),
        ..transition(
            "session-commit-restart",
            3,
            "committing",
            "operation-commit-restarted",
            "action-commit-restarted",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &restarted).unwrap();

    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_commit_attempts
                 WHERE session_id = 'session-commit-restart'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        1
    );
    let statuses = {
        let mut statement = connection
            .prepare(
                "SELECT status FROM provider_discovery_operations
                 WHERE session_id = 'session-commit-restart'
                 ORDER BY created_at, id",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    assert_eq!(statuses, vec!["interrupted", "prepared"]);
}

#[test]
fn startup_scan_classifies_prepared_and_started_operations_without_mutating_them() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-recovery");
    for (id, kind, side_effect, status, started_at) in [
        (
            "prepared-probe",
            "probe_capabilities",
            "billable_external",
            "prepared",
            None,
        ),
        (
            "started-fetch",
            "fetch_documents",
            "read_only",
            "started",
            Some(NOW),
        ),
        (
            "started-probe",
            "probe_capabilities",
            "billable_external",
            "started",
            Some(NOW),
        ),
        (
            "started-commit",
            "atomic_commit",
            "persistent",
            "started",
            Some(NOW),
        ),
    ] {
        connection
            .execute(
                "INSERT INTO provider_discovery_operations (
                     id, session_id, operation_kind, side_effect_class, status,
                     action_id, expected_revision, request_sha256, started_at,
                     finished_at, created_at, updated_at
                 ) VALUES (?1, 'session-recovery', ?2, ?3, ?4, ?1, 0, ?5,
                     ?6, NULL, ?7, ?7)",
                params![id, kind, side_effect, status, HASH_A, started_at, NOW],
            )
            .unwrap();
    }

    let work = list_unfinished_discovery_operations(&connection).unwrap();
    assert_eq!(work.len(), 4);
    assert_eq!(
        work.iter()
            .map(|item| (item.id.as_str(), item.disposition))
            .collect::<Vec<_>>(),
        vec![
            (
                "prepared-probe",
                DiscoveryRecoveryDisposition::MarkInterrupted
            ),
            (
                "started-commit",
                DiscoveryRecoveryDisposition::MarkUnknownOutcome
            ),
            (
                "started-fetch",
                DiscoveryRecoveryDisposition::MarkInterrupted
            ),
            (
                "started-probe",
                DiscoveryRecoveryDisposition::MarkUnknownOutcome
            ),
        ]
    );
    assert!(work.iter().all(|item| item.status != "outcome_unknown"));
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_operations
                 WHERE status IN ('prepared', 'started')",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        4
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn operation_lifecycle_is_durable_and_terminal_outcomes_are_not_replayed() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-operation");
    let resolving = transition(
        "session-operation",
        0,
        "resolving_known_provider",
        "operation-1",
        "action-operation",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &resolving).unwrap();

    assert!(mark_discovery_operation_started(&mut connection, "operation-1", NOW).unwrap());
    assert!(!mark_discovery_operation_started(&mut connection, "operation-1", NOW).unwrap());
    let completed = DurableDiscoveryTransition {
        action_kind: "known_provider_candidates_resolved",
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "operation-1",
            outcome: DurableOperationOutcome::Succeeded,
        }),
        ..transition(
            "session-operation",
            1,
            "awaiting_template_selection",
            "event-operation-complete",
            "action-operation-complete",
            HASH_B,
        )
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &completed).unwrap(),
        PersistDiscoveryTransition::Applied { .. }
    ));
    assert!(matches!(
        persist_discovery_transition(&mut connection, &completed).unwrap(),
        PersistDiscoveryTransition::Replayed { .. }
    ));
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM provider_discovery_operations
                 WHERE id = 'operation-1'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "succeeded"
    );

    new_session(&mut connection, "session-operation-failed");
    let resolving = transition(
        "session-operation-failed",
        0,
        "resolving_known_provider",
        "operation-failed",
        "action-operation-failed-start",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &resolving).unwrap();
    mark_discovery_operation_started(&mut connection, "operation-failed", NOW).unwrap();
    let failed = DurableDiscoveryTransition {
        action_kind: "fail",
        error_json: Some(
            r#"{"code":"network.timeout","message_key":"discovery.error.timeout","recoverable":true}"#,
        ),
        event_json: r#"{"version":2,"failure":{"code":"network.timeout","message_key":"discovery.error.timeout","recoverable":true}}"#,
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "operation-failed",
            outcome: DurableOperationOutcome::Failed,
        }),
        ..transition(
            "session-operation-failed",
            1,
            "failed",
            "event-operation-failed",
            "action-operation-failed",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &failed).unwrap();
    let persisted = load_discovery_failure(&connection, "session-operation-failed")
        .unwrap()
        .unwrap();
    assert_eq!(persisted.code, "network.timeout");
    assert_eq!(persisted.message_key, "discovery.error.timeout");
    assert!(persisted.recoverable);
    let event_json = connection
        .query_row(
            "SELECT event_json
             FROM provider_discovery_event_outbox
             WHERE id = 'event-operation-failed'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&event_json)
            .unwrap()
            .get("failure")
            .and_then(|failure| failure.get("code"))
            .and_then(serde_json::Value::as_str),
        Some("network.timeout")
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn cancellation_request_does_not_prepare_or_replay_the_active_operation() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-cancel-operation");
    let resolving = transition(
        "session-cancel-operation",
        0,
        "resolving_known_provider",
        "operation-active",
        "action-start-operation",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &resolving).unwrap();
    mark_discovery_operation_started(&mut connection, "operation-active", NOW).unwrap();

    let cancellation = DurableDiscoveryTransition {
        action_kind: "cancel",
        effect: DurableDiscoveryEffect::RequestCancellation,
        operation: None,
        cancellation_pending: true,
        ..transition(
            "session-cancel-operation",
            1,
            "resolving_known_provider",
            "event-cancel-operation",
            "action-cancel-operation",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &cancellation).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_operations
                 WHERE session_id = 'session-cancel-operation'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM provider_discovery_operations
                 WHERE id = 'operation-active'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "started"
    );
    let completed_after_cancel = DurableDiscoveryTransition {
        action_kind: "known_provider_candidates_resolved",
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "operation-active",
            outcome: DurableOperationOutcome::Succeeded,
        }),
        ..transition(
            "session-cancel-operation",
            2,
            "cancelled",
            "event-completed-after-cancel",
            "action-completed-after-cancel",
            HASH_A,
        )
    };
    persist_discovery_transition(&mut connection, &completed_after_cancel).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT state, active_operation_id
                 FROM provider_discovery_sessions
                 WHERE id = 'session-cancel-operation'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            )
            .unwrap(),
        ("cancelled".to_owned(), None)
    );

    new_session(&mut connection, "session-delayed-worker");
    let prepared = transition(
        "session-delayed-worker",
        0,
        "resolving_known_provider",
        "operation-delayed",
        "action-prepare-delayed",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &prepared).unwrap();
    let cancel_before_start = DurableDiscoveryTransition {
        action_kind: "cancel",
        effect: DurableDiscoveryEffect::RequestCancellation,
        operation: None,
        cancellation_pending: true,
        ..transition(
            "session-delayed-worker",
            1,
            "resolving_known_provider",
            "event-cancel-delayed",
            "action-cancel-delayed",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &cancel_before_start).unwrap();
    assert!(!mark_discovery_operation_started(&mut connection, "operation-delayed", NOW).unwrap());
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM provider_discovery_operations
                 WHERE id = 'operation-delayed'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "prepared"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn started_billable_operation_can_only_be_recovered_as_unknown_outcome() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-wrong-billable-hash");
    let grant_json = r#"{"kind":"capability_probe","model_route_ids":["route-1"],"budget":{"max_requests":5,"max_total_tokens_per_request":512,"max_output_tokens_per_request":32,"max_cost_micro_usd_per_request":100000,"max_duration_millis_per_request":30000,"max_calls_per_request":1}}"#;
    let grant_sha256 = sha256(grant_json);
    let mut wrong_hash = transition(
        "session-wrong-billable-hash",
        0,
        "probing_capabilities",
        "operation-wrong-billable-hash",
        "action-wrong-billable-hash",
        HASH_A,
    );
    wrong_hash.action_kind = "approve_probes";
    wrong_hash.action_approval_id = Some("approval-wrong-billable-hash");
    wrong_hash.approval = Some(NewDiscoveryApproval {
        id: "approval-wrong-billable-hash",
        approval_kind: "capability_probe",
        candidate_id: None,
        decision: "approved",
        grant_json,
    });
    let operation = wrong_hash.operation.as_mut().unwrap();
    operation.approval_id = Some("approval-wrong-billable-hash");
    operation.approval_grant_sha256 = Some(HASH_B);
    assert!(matches!(
        persist_discovery_transition(&mut connection, &wrong_hash),
        Err(DiscoveryStorageError::InvalidTransition(_))
    ));

    new_session(&mut connection, "session-billable");
    let mut probing = transition(
        "session-billable",
        0,
        "probing_capabilities",
        "operation-billable",
        "action-start-billable",
        HASH_A,
    );
    probing.action_kind = "approve_probes";
    probing.action_approval_id = Some("approval-billable");
    probing.approval = Some(NewDiscoveryApproval {
        id: "approval-billable",
        approval_kind: "capability_probe",
        candidate_id: None,
        decision: "approved",
        grant_json,
    });
    let operation = probing.operation.as_mut().unwrap();
    operation.approval_id = Some("approval-billable");
    operation.approval_grant_sha256 = Some(&grant_sha256);
    persist_discovery_transition(&mut connection, &probing).unwrap();
    let active_approval = connection
        .query_row(
            "SELECT active_effect_approval_json
             FROM provider_discovery_sessions
             WHERE id = 'session-billable'",
            [],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&active_approval)
            .unwrap()
            .get("grant_sha256")
            .and_then(serde_json::Value::as_str),
        Some(grant_sha256.as_str())
    );
    mark_discovery_operation_started(&mut connection, "operation-billable", NOW).unwrap();

    let unsafe_interrupt = DurableDiscoveryTransition {
        action_kind: "interrupt",
        recovery_json: Some(
            r#"{"interrupted_state":"probing_capabilities","operation":"probe_capabilities"}"#,
        ),
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "operation-billable",
            outcome: DurableOperationOutcome::Interrupted,
        }),
        ..transition(
            "session-billable",
            1,
            "interrupted",
            "event-unsafe-interrupt",
            "action-unsafe-interrupt",
            HASH_B,
        )
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &unsafe_interrupt),
        Err(DiscoveryStorageError::InvalidTransition(_))
    ));

    let forged_attestation = DurableDiscoveryTransition {
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "operation-billable",
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
        action_id: "action-forged-attestation",
        event_id: "event-forged-attestation",
        ..unsafe_interrupt
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &forged_attestation),
        Err(DiscoveryStorageError::InvalidTransition(_))
    ));

    let unknown = DurableDiscoveryTransition {
        action_kind: "interrupt",
        unknown_operation: Some("probe_capabilities"),
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "operation-billable",
            outcome: DurableOperationOutcome::OutcomeUnknown,
        }),
        ..transition(
            "session-billable",
            1,
            "unknown_outcome",
            "event-unknown-billable",
            "action-unknown-billable",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &unknown).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM provider_discovery_operations
                 WHERE id = 'operation-billable'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "outcome_unknown"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT active_operation_id, active_effect_approval_json IS NOT NULL
                 FROM provider_discovery_sessions
                 WHERE id = 'session-billable'",
                [],
                |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, bool>(1)?))
            )
            .unwrap(),
        (None, true)
    );
}

#[test]
fn deterministic_manifest_build_is_local_and_never_requires_billable_consent() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-local-draft");
    let local = transition(
        "session-local-draft",
        0,
        "building_deterministic_manifest_draft",
        "operation-local-draft",
        "action-local-draft",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &local).unwrap();
    assert!(
        mark_discovery_operation_started(&mut connection, "operation-local-draft", NOW).unwrap()
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT side_effect_class, approval_id
                 FROM provider_discovery_operations
                 WHERE id = 'operation-local-draft'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            )
            .unwrap(),
        ("local_deterministic".to_owned(), None)
    );
    let operation = list_unfinished_discovery_operations(&connection)
        .unwrap()
        .into_iter()
        .find(|operation| operation.id == "operation-local-draft")
        .unwrap();
    assert_eq!(
        operation.disposition,
        DiscoveryRecoveryDisposition::MarkInterrupted
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn compensation_failure_is_persisted_without_retry_until_explicit_resume() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-compensation-failed");
    connection
        .execute(
            "INSERT INTO provider_discovery_commit_attempts (
                 id, session_id, attempt_number, action_id, expected_revision,
                 plan_sha256, plan_json, phase, redaction_version,
                 created_at, updated_at
             ) VALUES (
                 'attempt-compensation-failed',
                 'session-compensation-failed',
                 1,
                 'action-prepare-compensation',
                 0,
                 ?1,
                 '{}',
                 'compensation_required',
                 1,
                 ?2,
                 ?2
             )",
            params![HASH_A, NOW],
        )
        .unwrap();
    let compensating = DurableDiscoveryTransition {
        commit_plan_sha256: Some(HASH_A),
        commit_attempt_id: Some("attempt-compensation-failed"),
        ..transition(
            "session-compensation-failed",
            0,
            "compensating",
            "operation-compensation-failed",
            "action-start-compensation",
            HASH_A,
        )
    };
    persist_discovery_transition(&mut connection, &compensating).unwrap();
    assert!(
        mark_discovery_operation_started(&mut connection, "operation-compensation-failed", NOW)
            .unwrap()
    );

    let failed = DurableDiscoveryTransition {
        action_kind: "compensation_failed",
        effect: DurableDiscoveryEffect::None,
        operation: None,
        commit_plan_sha256: Some(HASH_A),
        commit_attempt_id: Some("attempt-compensation-failed"),
        error_json: Some(
            r#"{"code":"credential.rollback_failed","message_key":"discovery.error.rollback_failed","recoverable":false}"#,
        ),
        event_json: r#"{"version":2,"failure":{"code":"credential.rollback_failed","message_key":"discovery.error.rollback_failed","recoverable":false}}"#,
        completed_operation: Some(CompletedDiscoveryOperation {
            id: "operation-compensation-failed",
            outcome: DurableOperationOutcome::Failed,
        }),
        ..transition(
            "session-compensation-failed",
            1,
            "compensating",
            "event-compensation-failed",
            "action-compensation-failed",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &failed).unwrap();

    assert_eq!(
        load_discovery_failure(&connection, "session-compensation-failed")
            .unwrap()
            .unwrap()
            .code,
        "credential.rollback_failed"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM provider_discovery_operations
                 WHERE id = 'operation-compensation-failed'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "failed"
    );

    let resumed = DurableDiscoveryTransition {
        action_kind: "resume_compensation",
        commit_plan_sha256: Some(HASH_A),
        commit_attempt_id: Some("attempt-compensation-failed"),
        ..transition(
            "session-compensation-failed",
            2,
            "compensating",
            "operation-compensation-resumed",
            "action-compensation-resumed",
            HASH_A,
        )
    };
    persist_discovery_transition(&mut connection, &resumed).unwrap();
    assert!(
        load_discovery_failure(&connection, "session-compensation-failed")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM provider_discovery_operations
                 WHERE id = 'operation-compensation-resumed'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "prepared"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn v4_upgrade_deterministically_rekeys_all_session_ids() {
    let mut connection = Connection::open_in_memory().unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.execute_batch(MIGRATION_0004).unwrap();

    let reserved_id = "legacy-v5-session-0000000000000002";
    let oversized_id = format!("legacy-secret-id-{}", "x".repeat(160));
    let valid_secret_id = "sk-proj-valid-length-secret-session-id";
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, created_at, updated_at
             ) VALUES (?1, 'draft', '{}', ?2, ?2)",
            params![reserved_id, NOW],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, created_at, updated_at
             ) VALUES (?1, 'draft', '{}', ?2, ?2)",
            params![oversized_id, NOW],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, created_at, updated_at
             ) VALUES (?1, 'draft', '{}', ?2, ?2)",
            params![" padded-legacy-id ", NOW],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, created_at, updated_at
             ) VALUES (?1, 'draft', '{}', ?2, ?2)",
            params!["nul\u{0}legacy-id", NOW],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, created_at, updated_at
             ) VALUES (?1, 'draft', '{}', ?2, ?2)",
            params![valid_secret_id, NOW],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_evidence (
                 id, session_id, kind, source_url, content_sha256,
                 extracted_json, fetched_at
             ) VALUES
                 ('reserved-evidence', ?1, 'document',
                  'https://provider.example/docs', ?3, '{}', ?4),
                 ('oversized-evidence', ?2, 'document',
                  'https://provider.example/docs', ?3, '{}', ?4)",
            params![reserved_id, oversized_id, HASH_A, NOW],
        )
        .unwrap();

    apply_migration_0005(&mut connection);

    let migrated_ids = connection
        .prepare("SELECT id FROM provider_discovery_sessions ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        migrated_ids,
        vec![
            "legacy-v5-session-0000000000000001".to_owned(),
            "legacy-v5-session-0000000000000002".to_owned(),
            "legacy-v5-session-0000000000000003".to_owned(),
            "legacy-v5-session-0000000000000004".to_owned(),
            "legacy-v5-session-0000000000000005".to_owned(),
        ]
    );
    assert!(migrated_ids.iter().all(|id| id.len() <= 128));
    assert!(!migrated_ids.iter().any(|id| id == &oversized_id));
    assert!(!migrated_ids.iter().any(|id| id == valid_secret_id));

    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM provider_discovery_evidence",
                [],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        0
    );

    let audit_subjects = connection
        .prepare(
            "SELECT session_id, subject_id, summary_key
             FROM provider_discovery_audit_log
             ORDER BY session_id",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(audit_subjects.len(), 5);
    for (session_id, subject_id, summary_key) in audit_subjects {
        assert_eq!(session_id, subject_id);
        assert!(migrated_ids.contains(&session_id));
        assert_eq!(summary_key, "discovery.audit.session_created_before_v5");
        assert!(!session_id.contains("legacy-secret-id"));
        assert!(!session_id.contains("sk-proj"));
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn durable_identity_and_lifecycle_triggers_reject_retargeting_and_state_downgrades() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-ledger-triggers");
    connection
        .execute(
            "INSERT INTO provider_discovery_operations (
                 id, session_id, operation_kind, side_effect_class, status,
                 action_id, expected_revision, request_sha256,
                 created_at, updated_at
             ) VALUES (
                 'operation-ledger-trigger', 'session-ledger-triggers',
                 'fetch_documents', 'read_only', 'prepared',
                 'action-ledger-operation', 0, ?1, ?2, ?2
             )",
            params![HASH_A, NOW],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET id = 'operation-retargeted'
                 WHERE id = 'operation-ledger-trigger'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'succeeded', started_at = ?2,
                     finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params!["operation-ledger-trigger", NOW],
            )
            .is_err()
    );
    connection
        .execute(
            "UPDATE provider_discovery_operations
             SET status = 'started', started_at = ?2, updated_at = ?2
             WHERE id = ?1",
            params!["operation-ledger-trigger", NOW],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'outcome_unknown', finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params!["operation-ledger-trigger", NOW],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'prepared', started_at = NULL, updated_at = ?2
                 WHERE id = ?1",
                params!["operation-ledger-trigger", NOW],
            )
            .is_err()
    );
    connection
        .execute(
            "INSERT INTO provider_discovery_operations (
                 id, session_id, operation_kind, side_effect_class, status,
                 action_id, expected_revision, request_sha256, started_at,
                 created_at, updated_at
             ) VALUES (
                 'operation-billable-trigger', 'session-ledger-triggers',
                 'probe_capabilities', 'billable_external', 'started',
                 'action-billable-operation', 0, ?1, ?2, ?2, ?2
             )",
            params![HASH_B, NOW],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'interrupted', finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params!["operation-billable-trigger", NOW],
            )
            .is_err()
    );
    connection
        .execute(
            "UPDATE provider_discovery_operations
             SET status = 'outcome_unknown', finished_at = ?2, updated_at = ?2
             WHERE id = ?1",
            params!["operation-billable-trigger", NOW],
        )
        .unwrap();

    connection
        .execute(
            "INSERT INTO provider_discovery_commit_attempts (
                 id, session_id, attempt_number, action_id, expected_revision,
                 plan_sha256, plan_json, phase, redaction_version,
                 created_at, updated_at
             ) VALUES (
                 'attempt-ledger-trigger', 'session-ledger-triggers', 1,
                 'action-ledger-commit', 0, ?1, '{}', 'prepared', 1, ?2, ?2
             )",
            params![HASH_A, NOW],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_commit_attempts
                 SET id = 'attempt-retargeted'
                 WHERE id = 'attempt-ledger-trigger'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_commit_attempts
                 SET phase = 'completed', completed_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                params!["attempt-ledger-trigger", NOW],
            )
            .is_err()
    );
    connection
        .execute(
            "UPDATE provider_discovery_commit_attempts
             SET phase = 'database_applied', updated_at = ?2
             WHERE id = ?1",
            params!["attempt-ledger-trigger", NOW],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_commit_attempts
                 SET phase = 'prepared', updated_at = ?2
                 WHERE id = ?1",
                params!["attempt-ledger-trigger", NOW],
            )
            .is_err()
    );

    connection
        .execute(
            "INSERT INTO provider_discovery_compensation_steps (
                 id, commit_attempt_id, ordinal, action_id, step_kind,
                 step_json, status, attempt_count, redaction_version,
                 created_at, updated_at
             ) VALUES (
                 'step-ledger-trigger', 'attempt-ledger-trigger', 0,
                 'action-ledger-step', 'restore_previous_selection',
                 '{}', 'pending', 0, 1, ?1, ?1
             )",
            [NOW],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_compensation_steps
                 SET id = 'step-retargeted'
                 WHERE id = 'step-ledger-trigger'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_compensation_steps
                 SET status = 'failed',
                     last_failure_json = '{\"code\":\"invalid_jump\"}',
                     updated_at = ?2
                 WHERE id = ?1",
                params!["step-ledger-trigger", NOW],
            )
            .is_err()
    );
    connection
        .execute(
            "UPDATE provider_discovery_compensation_steps
             SET status = 'in_progress', attempt_count = 1, updated_at = ?2
             WHERE id = ?1",
            params!["step-ledger-trigger", NOW],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE provider_discovery_compensation_steps
             SET status = 'failed',
                 last_failure_json = '{\"code\":\"rollback_failed\"}',
                 updated_at = ?2
             WHERE id = ?1",
            params!["step-ledger-trigger", NOW],
        )
        .unwrap();
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_compensation_steps
                 SET status = 'in_progress', attempt_count = 2,
                     last_failure_json = NULL, updated_at = ?2
                 WHERE id = ?1",
                params!["step-ledger-trigger", NOW],
            )
            .is_err()
    );
    connection
        .execute(
            "UPDATE provider_discovery_compensation_steps
             SET status = 'pending', last_failure_json = NULL, updated_at = ?2
             WHERE id = ?1",
            params!["step-ledger-trigger", NOW],
        )
        .unwrap();
}

#[test]
fn candidate_approvals_are_session_bound_and_append_only() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-candidate");
    new_session(&mut connection, "session-other");
    connection
        .execute(
            "INSERT INTO provider_discovery_candidates (
                 id, session_id, candidate_kind, summary_json,
                 evidence_ids_json, proposed_revision, redaction_version,
                 created_at
             ) VALUES (
                 'candidate-1', 'session-candidate', 'provider_template',
                 '{\"template_id\":\"template-1\",\"template_version\":1}',
                 '[]', 0, 1, ?1
             )",
            [NOW],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO provider_discovery_approvals (
                 id, session_id, approval_kind, candidate_id, decision,
                 grant_json, session_revision, grant_sha256,
                 redaction_version, created_at
             ) VALUES (
                 'approval-1', 'session-candidate', 'template_selection',
                 'candidate-1', 'approved',
                 '{\"candidate_id\":\"candidate-1\"}', 0, ?1, 1, ?2
             )",
            params![HASH_A, NOW],
        )
        .unwrap();

    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_candidates
             SET summary_json = '{}'
             WHERE id = 'candidate-1'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_approvals
             SET decision = 'rejected'
             WHERE id = 'approval-1'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "INSERT INTO provider_discovery_approvals (
                 id, session_id, approval_kind, candidate_id, decision,
                 grant_json, session_revision, grant_sha256,
                 redaction_version, created_at
             ) VALUES (
                 'approval-cross-session', 'session-other',
                 'template_selection', 'candidate-1', 'approved',
                 '{\"candidate_id\":\"candidate-1\"}', 0, ?1, 1, ?2
             )",
                params![HASH_A, NOW],
            )
            .is_err()
    );
}

#[test]
fn consent_action_and_approval_are_committed_in_the_same_transition() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-consent");
    connection
        .execute(
            "INSERT INTO provider_discovery_candidates (
                 id, session_id, candidate_kind, summary_json,
                 evidence_ids_json, proposed_revision, redaction_version,
                 created_at
             ) VALUES (
                 'candidate-consent', 'session-consent', 'provider_template',
                 '{\"template_id\":\"template-1\",\"template_version\":1}',
                 '[]', 0, 1, ?1
             )",
            [NOW],
        )
        .unwrap();

    let selecting = DurableDiscoveryTransition {
        action_kind: "select_template",
        action_approval_id: Some("approval-consent"),
        approval: Some(NewDiscoveryApproval {
            id: "approval-consent",
            approval_kind: "template_selection",
            candidate_id: Some("candidate-consent"),
            decision: "approved",
            grant_json: r#"{"kind":"template_selection","candidate_id":"candidate-consent"}"#,
        }),
        ..transition(
            "session-consent",
            0,
            "validating_manifest",
            "event-consent",
            "action-consent",
            HASH_A,
        )
    };
    persist_discovery_transition(&mut connection, &selecting).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT decision FROM provider_discovery_approvals
                 WHERE id = 'approval-consent'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "approved"
    );

    new_session(&mut connection, "session-missing-consent");
    let missing = DurableDiscoveryTransition {
        action_kind: "approve_review",
        ..transition(
            "session-missing-consent",
            0,
            "awaiting_review",
            "event-missing-consent",
            "action-missing-consent",
            HASH_B,
        )
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &missing),
        Err(DiscoveryStorageError::InvalidTransition(_))
    ));
    assert_eq!(
        connection
            .query_row(
                "SELECT revision FROM provider_discovery_sessions
                 WHERE id = 'session-missing-consent'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn sensitive_approvals_require_typed_canonical_session_revision_bound_grants() {
    let mut connection = migrated_connection();

    new_session(&mut connection, "session-credential-approval");
    let awaiting_credential = DurableDiscoveryTransition {
        effect: DurableDiscoveryEffect::None,
        operation: None,
        manifest_sha256: Some(HASH_A),
        ..transition(
            "session-credential-approval",
            0,
            "awaiting_credential_origin_approval",
            "event-awaiting-credential",
            "action-awaiting-credential",
            HASH_A,
        )
    };
    persist_discovery_transition(&mut connection, &awaiting_credential).unwrap();
    let credential_grant = r#"{"kind":"credential_origin","origin":"https://api.provider.example","auth_binding":{"kind":"bearer_header"},"manifest_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#;
    let credential_approval = DurableDiscoveryTransition {
        action_kind: "approve_credential_origin",
        action_approval_id: Some("approval-credential"),
        manifest_sha256: Some(HASH_A),
        approval: Some(NewDiscoveryApproval {
            id: "approval-credential",
            approval_kind: "credential_origin",
            candidate_id: None,
            decision: "approved",
            grant_json: credential_grant,
        }),
        ..transition(
            "session-credential-approval",
            1,
            "listing_models",
            "operation-list-models",
            "action-approve-credential",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &credential_approval).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT session_revision, grant_sha256
                 FROM provider_discovery_approvals
                 WHERE id = 'approval-credential'",
                [],
                |row| Ok((row.get::<_, u64>(0)?, row.get::<_, String>(1)?))
            )
            .unwrap(),
        (1, sha256(credential_grant))
    );

    new_session(&mut connection, "session-review-approval");
    let awaiting_review = transition(
        "session-review-approval",
        0,
        "awaiting_review",
        "event-awaiting-review",
        "action-awaiting-review",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &awaiting_review).unwrap();
    let review_approval = DurableDiscoveryTransition {
        action_kind: "approve_review",
        action_approval_id: Some("approval-review"),
        review_diff_json: Some(REVIEW_JSON),
        approval: Some(NewDiscoveryApproval {
            id: "approval-review",
            approval_kind: "review",
            candidate_id: None,
            decision: "approved",
            grant_json: REVIEW_GRANT,
        }),
        ..transition(
            "session-review-approval",
            1,
            "awaiting_review",
            "event-review-approved",
            "action-review-approved",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &review_approval).unwrap();

    new_session(&mut connection, "session-unknown-approval");
    let unknown = DurableDiscoveryTransition {
        effect: DurableDiscoveryEffect::None,
        operation: None,
        unknown_operation: Some("probe_capabilities"),
        ..transition(
            "session-unknown-approval",
            0,
            "unknown_outcome",
            "event-unknown",
            "action-unknown",
            HASH_A,
        )
    };
    persist_discovery_transition(&mut connection, &unknown).unwrap();
    let resolution_grant = r#"{"kind":"unknown_outcome_resolution","operation":"probe_capabilities","resolution":{"resolution":"manually_reconciled_as_failed"}}"#;
    let resolved = DurableDiscoveryTransition {
        action_kind: "resolve_unknown_outcome",
        action_approval_id: Some("approval-resolution"),
        approval: Some(NewDiscoveryApproval {
            id: "approval-resolution",
            approval_kind: "unknown_outcome_resolution",
            candidate_id: None,
            decision: "approved",
            grant_json: resolution_grant,
        }),
        ..transition(
            "session-unknown-approval",
            1,
            "failed",
            "event-resolved",
            "action-resolved",
            HASH_B,
        )
    };
    persist_discovery_transition(&mut connection, &resolved).unwrap();

    new_session(&mut connection, "session-noncanonical-approval");
    let noncanonical = DurableDiscoveryTransition {
        action_kind: "approve_review",
        action_approval_id: Some("approval-noncanonical"),
        review_diff_json: Some(REVIEW_JSON),
        approval: Some(NewDiscoveryApproval {
            id: "approval-noncanonical",
            approval_kind: "review",
            candidate_id: None,
            decision: "approved",
            grant_json: r#"{ "kind":"review","review_sha256":"abb8bda2360b026a390418abb0222c0f098e0e3d59579968011dbc9036454125"}"#,
        }),
        ..transition(
            "session-noncanonical-approval",
            0,
            "awaiting_review",
            "event-noncanonical",
            "action-noncanonical",
            HASH_A,
        )
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &noncanonical),
        Err(DiscoveryStorageError::InvalidTransition(_))
    ));
    assert_eq!(
        connection
            .query_row(
                "SELECT revision FROM provider_discovery_sessions
                 WHERE id = 'session-noncanonical-approval'",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );

    new_session(&mut connection, "session-untyped-review");
    let untyped_review = DurableDiscoveryTransition {
        action_kind: "approve_review",
        action_approval_id: Some("approval-untyped-review"),
        review_diff_json: Some(
            r#"{"sha256":"abb8bda2360b026a390418abb0222c0f098e0e3d59579968011dbc9036454125"}"#,
        ),
        approval: Some(NewDiscoveryApproval {
            id: "approval-untyped-review",
            approval_kind: "review",
            candidate_id: None,
            decision: "approved",
            grant_json: REVIEW_GRANT,
        }),
        ..transition(
            "session-untyped-review",
            0,
            "awaiting_review",
            "event-untyped-review",
            "action-untyped-review",
            HASH_A,
        )
    };
    assert!(matches!(
        persist_discovery_transition(&mut connection, &untyped_review),
        Err(DiscoveryStorageError::InvalidTransition(_))
    ));
}

#[test]
fn outbox_is_at_least_once_and_append_only_records_reject_mutation() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-outbox");
    let first = transition(
        "session-outbox",
        0,
        "resolving_known_provider",
        "event-outbox",
        "action-outbox",
        HASH_A,
    );
    persist_discovery_transition(&mut connection, &first).unwrap();

    let events = list_pending_discovery_events(&connection, 10).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, "event-outbox");
    assert!(mark_discovery_event_delivered(&connection, "event-outbox", NOW).unwrap());
    assert!(!mark_discovery_event_delivered(&connection, "event-outbox", NOW).unwrap());
    assert!(
        list_pending_discovery_events(&connection, 10)
            .unwrap()
            .is_empty()
    );

    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_action_receipts
             SET outcome = 'rejected'
             WHERE action_id = 'action-outbox'",
                [],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM provider_discovery_audit_log
             WHERE session_id = 'session-outbox'",
                [],
            )
            .is_err()
    );
}

#[test]
fn revision_guard_rejects_skipped_or_unsequenced_updates() {
    let mut connection = migrated_connection();
    new_session(&mut connection, "session-guard");

    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_sessions
             SET state = 'fetching_documents', revision = 2,
                 next_event_sequence = 2, updated_at = ?1
             WHERE id = 'session-guard'",
                [NOW],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_sessions
             SET state = 'fetching_documents', revision = 1,
                 next_event_sequence = 1, updated_at = ?1
             WHERE id = 'session-guard'",
                [NOW],
            )
            .is_err()
    );
}
