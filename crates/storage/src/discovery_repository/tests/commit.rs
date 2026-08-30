use super::support::*;
use super::*;

#[test]
fn completed_discovery_credential_authority_revalidates_the_full_terminal_history() {
    let fixture = seed_completed_discovery_authority("discovery-authority-valid");
    let database = fixture.storage.connection().expect("authority database");
    validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("complete discovery authority is valid");

    let error = validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &"f".repeat(64),
    )
    .expect_err("stale connection binding must not retain discovery authority");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn direct_credential_commit_replay_preserves_later_active_ownership_tail() {
    let fixture = seed_completed_discovery_authority("direct-replay-active-tail");
    let replay = direct_completed_discovery_replay_write(&fixture);
    project_completed_discovery_credential_authority_at(&fixture, replay.occurred_at);
    assert!(matches!(
        fixture
            .storage
            .persist_credential_confirmed_discovery_commit(&replay)
            .expect("exact direct commit replay"),
        PersistDiscoveryTransition::Replayed { .. }
    ));

    complete_ordinary_credential_successor(&fixture);
    let later = fixture
        .storage
        .ensure_provider_credential_access_settled(&fixture.connection_id)
        .expect("ordinary successor owns current access");
    let tail_before = active_credential_ownership_tail(&fixture);

    assert!(matches!(
        fixture
            .storage
            .persist_credential_confirmed_discovery_commit(&replay)
            .expect("historical direct replay after active successor"),
        PersistDiscoveryTransition::Replayed { .. }
    ));
    assert_eq!(
        fixture
            .storage
            .ensure_provider_credential_access_settled(&fixture.connection_id)
            .expect("replay preserves ordinary successor"),
        later
    );
    assert_eq!(active_credential_ownership_tail(&fixture), tail_before);
}

#[test]
fn direct_credential_commit_replay_preserves_archived_ownership_tail() {
    let fixture = seed_completed_discovery_authority("direct-replay-archived-tail");
    let replay = direct_completed_discovery_replay_write(&fixture);
    project_completed_discovery_credential_authority_at(&fixture, replay.occurred_at);
    archive_credential_bound_connection(&fixture.storage, &fixture.connection_id);
    let garbage_before = fixture
        .storage
        .list_provider_credential_slot_garbage()
        .expect("load archived ownership tail");
    assert_eq!(garbage_before.len(), 1);

    assert!(matches!(
        fixture
            .storage
            .persist_credential_confirmed_discovery_commit(&replay)
            .expect("historical direct replay after archive"),
        PersistDiscoveryTransition::Replayed { .. }
    ));
    fixture
        .storage
        .get_provider_connection(&fixture.connection_id)
        .expect_err("historical replay must not unarchive provider connection");
    assert_eq!(
        fixture
            .storage
            .list_provider_credential_slot_garbage()
            .expect("replay preserves archived ownership history"),
        garbage_before
    );
}

#[test]
fn confirmed_commit_completion_public_transition_projects_exact_operation_authority() {
    let fixture = seed_completed_discovery_authority_with_mode(
        "public-confirmed-completion",
        CompletedDiscoveryAuthorityMode::PendingReconciled,
    );
    persist_pending_confirmed_commit_completion(
        &fixture,
        "public-confirmed-completion-action",
        "public-confirmed-completion-approval",
    );

    let authority = fixture
        .storage
        .ensure_provider_credential_access_settled(&fixture.connection_id)
        .expect("public confirmed completion grants exact credential authority");
    assert_eq!(authority.authority_id, fixture.physical_authority_id);
    let source = fixture
        .storage
        .connection()
        .expect("ownership database")
        .query_row(
            "SELECT source_kind, source_id
             FROM provider_credential_ownership_events
             WHERE connection_id = ?1 AND authority_id = ?2",
            rusqlite::params![fixture.connection_id.as_str(), authority.authority_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("load projected discovery ownership event");
    assert_eq!(source.0, "discovery_commit");
    assert_eq!(source.1, fixture.operation_id.as_str());

    let root_path = fixture.root.path().to_path_buf();
    let connection_id = fixture.connection_id.clone();
    let physical_authority_id = fixture.physical_authority_id.clone();
    drop(fixture.storage);
    let reopened = Storage::open(&root_path).expect("reopen confirmed-completion authority");
    let reopened_authority = reopened
        .ensure_provider_credential_access_settled(&connection_id)
        .expect("reopen preserves exact confirmed-completion authority");
    assert_eq!(reopened_authority.authority_id, physical_authority_id);
}

#[test]
fn replaced_discovery_attempt_cannot_forge_credential_authority() {
    let fixture = seed_completed_discovery_authority("discovery-authority-attempt-replace");
    let database = fixture.storage.connection().expect("authority database");
    let replace_attempt = || {
        database.execute(
            "INSERT OR REPLACE INTO provider_discovery_commit_attempts (
                     id, session_id, attempt_number, action_id, expected_revision,
                     plan_sha256, plan_json, phase, redaction_version,
                     created_at, updated_at, completed_at
                 )
                 SELECT id, session_id, attempt_number, 'detached-forged-action',
                        expected_revision, plan_sha256, plan_json, phase,
                        redaction_version, created_at, updated_at, completed_at
                 FROM provider_discovery_commit_attempts WHERE id = ?1",
            [fixture.attempt_id.as_str()],
        )
    };
    replace_attempt().expect_err("attempt REPLACE guard must preserve history");
    validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("rejected attempt replacement preserves authority");
    database
        .execute_batch("DROP TRIGGER provider_discovery_commit_attempt_no_replace;")
        .expect("drop attempt guard only for runtime corruption fixture");
    database
        .pragma_update(None, "foreign_keys", false)
        .expect("suspend foreign keys only for replaced-attempt corruption fixture");
    replace_attempt().expect("inject replaced authority attempt after dropping test guard");
    database
        .pragma_update(None, "foreign_keys", true)
        .expect("restore foreign keys after replaced-attempt corruption fixture");
    let error = validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect_err("replaced terminal attempt must not authorize a credential");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn noncanonical_rehashed_discovery_plan_cannot_authorize_a_credential() {
    let fixture = seed_completed_discovery_authority("discovery-authority-plan-canonical");
    let database = fixture.storage.connection().expect("authority database");
    let plan_json = database
        .query_row(
            "SELECT plan_json FROM provider_discovery_commit_attempts WHERE id = ?1",
            [fixture.attempt_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load canonical authority plan");
    let pretty_plan = serde_json::to_string_pretty(
        &serde_json::from_str::<Value>(&plan_json).expect("parse authority plan"),
    )
    .expect("encode noncanonical authority plan");
    assert_ne!(pretty_plan, plan_json);
    database
        .execute_batch("DROP TRIGGER provider_discovery_commit_attempt_no_replace;")
        .expect("drop attempt guard only for canonical corruption fixture");
    database
        .pragma_update(None, "foreign_keys", false)
        .expect("suspend foreign keys only for canonical corruption fixture");
    database
        .execute(
            "INSERT OR REPLACE INTO provider_discovery_commit_attempts (
                 id, session_id, attempt_number, action_id, expected_revision,
                 plan_sha256, plan_json, phase, redaction_version,
                 created_at, updated_at, completed_at
             )
             SELECT id, session_id, attempt_number, action_id, expected_revision,
                    ?2, ?3, phase, redaction_version,
                    created_at, updated_at, completed_at
             FROM provider_discovery_commit_attempts WHERE id = ?1",
            rusqlite::params![
                fixture.attempt_id.as_str(),
                sha256_hex(pretty_plan.as_bytes()),
                pretty_plan,
            ],
        )
        .expect("inject noncanonical rehashed authority plan");
    database
        .pragma_update(None, "foreign_keys", true)
        .expect("restore foreign keys after canonical corruption fixture");
    let error = validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect_err("noncanonical rehashed plan must not authorize a credential");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn detached_ready_session_cannot_forge_discovery_credential_authority() {
    let fixture = seed_completed_discovery_authority("discovery-authority-session-detach");
    let database = fixture.storage.connection().expect("authority database");
    database
        .execute(
            "INSERT OR REPLACE INTO provider_discovery_sessions (
                 id, state, revision, next_event_sequence, sanitized_input_json,
                 draft_json, review_diff_json, error_json, recovery_json,
                 unknown_operation, manifest_sha256, commit_plan_sha256,
                 commit_attempt_id, committed_connection_id, cancellation_pending,
                 active_operation_id, active_effect_approval_json, redaction_version,
                 created_at, updated_at
             )
             SELECT id, state, revision, next_event_sequence, sanitized_input_json,
                    draft_json, review_diff_json, error_json, recovery_json,
                    unknown_operation, manifest_sha256, commit_plan_sha256,
                    commit_attempt_id, committed_connection_id, cancellation_pending,
                    active_operation_id, active_effect_approval_json, redaction_version,
                    created_at, updated_at
             FROM provider_discovery_sessions WHERE commit_attempt_id = ?1",
            [fixture.attempt_id.as_str()],
        )
        .expect_err("session REPLACE guard must preserve history");
    validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("rejected session replacement preserves authority");
    database
        .execute_batch("DROP TRIGGER provider_discovery_session_revision_guard;")
        .expect("drop session guard only for corruption fixture");
    database
        .execute(
            "UPDATE provider_discovery_sessions
             SET committed_connection_id = NULL
             WHERE commit_attempt_id = ?1",
            [fixture.attempt_id.as_str()],
        )
        .expect("detach authority session corruption fixture");
    let error = validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect_err("detached ready session must not authorize a credential");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn replaced_terminal_operation_and_graph_audit_fail_discovery_authority_closed() {
    for (id, corrupt) in [
        ("discovery-authority-operation-replace", "operation"),
        ("discovery-authority-audit-replace", "audit"),
    ] {
        let fixture = seed_completed_discovery_authority(id);
        let database = fixture.storage.connection().expect("authority database");
        if corrupt == "operation" {
            let replace_operation = || {
                database.execute(
                    "INSERT OR REPLACE INTO provider_discovery_operations (
                         id, session_id, operation_kind, side_effect_class, status,
                         action_id, expected_revision, request_sha256, approval_id,
                         approval_grant_sha256, started_at, finished_at,
                         created_at, updated_at
                     )
                     SELECT id, session_id, operation_kind, side_effect_class, 'failed',
                            action_id, expected_revision, request_sha256, approval_id,
                            approval_grant_sha256, started_at, finished_at,
                            created_at, updated_at
                     FROM provider_discovery_operations WHERE id = ?1",
                    [fixture.operation_id.as_str()],
                )
            };
            replace_operation().expect_err("operation REPLACE guard must preserve history");
            validate_discovery_credential_ownership_authority(
                &database,
                &fixture.connection_id,
                &fixture.physical_authority_id,
                fixture.authority_operation_id.as_str(),
                &fixture.binding_sha256,
            )
            .expect("rejected operation replacement preserves authority");
            database
                .execute_batch("DROP TRIGGER provider_discovery_operation_no_replace;")
                .expect("drop operation guard only for runtime corruption fixture");
            database
                .pragma_update(None, "foreign_keys", false)
                .expect("suspend foreign keys only for replaced-operation fixture");
            replace_operation()
                .expect("inject replaced terminal operation after dropping test guard");
            database
                .pragma_update(None, "foreign_keys", true)
                .expect("restore foreign keys after replaced-operation fixture");
        } else {
            let replace_audit = || {
                database.execute(
                    "INSERT OR REPLACE INTO provider_discovery_audit_log (
                         id, session_id, audit_sequence, session_revision, audit_kind,
                         action_id, subject_id, summary_key, created_at
                     )
                     SELECT id, session_id, audit_sequence, session_revision, audit_kind,
                            action_id, ?2, summary_key, created_at
                     FROM provider_discovery_audit_log
                     WHERE session_id = ?1
                       AND summary_key = 'discovery.audit.provider_graph_applied'",
                    rusqlite::params![fixture.session_id.as_str(), "f".repeat(64)],
                )
            };
            replace_audit().expect_err("audit REPLACE guard must preserve history");
            validate_discovery_credential_ownership_authority(
                &database,
                &fixture.connection_id,
                &fixture.physical_authority_id,
                fixture.authority_operation_id.as_str(),
                &fixture.binding_sha256,
            )
            .expect("rejected audit replacement preserves authority");
            database
                .execute_batch("DROP TRIGGER provider_discovery_audit_no_replace;")
                .expect("drop audit guard only for runtime corruption fixture");
            replace_audit().expect("inject replaced graph audit after dropping test guard");
        }
        let error = validate_discovery_credential_ownership_authority(
            &database,
            &fixture.connection_id,
            &fixture.physical_authority_id,
            fixture.authority_operation_id.as_str(),
            &fixture.binding_sha256,
        )
        .expect_err("replaced terminal authority evidence must fail closed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }
}

#[test]
fn commit_succeeded_cannot_publish_ready_without_its_graph() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-missing-commit-graph");
    let mut forged = write(
        apply(&draft, ProviderDiscoveryAction::Begin, 'b'),
        Some(DiscoveryOperationId::parse("operation-missing-commit-graph").expect("operation id")),
        None,
    );
    forged.transition.receipt.action_kind = "commit_succeeded".to_owned();

    let error = storage
        .persist_discovery_transition(&forged)
        .expect_err("Ready commit bookkeeping without a graph must be rejected");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_discovery_session(&draft.id)
            .expect_err("rejected publication must leave no partial session")
            .code,
        CoreErrorCode::NotFound
    );
}

#[test]
fn cross_session_commit_attempt_binding_fails_closed_before_restart_shortcut() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let owner = draft_session("session-attempt-owner");
    let other = draft_session("session-attempt-other");
    storage
        .create_discovery_session(&owner, now())
        .expect("create attempt owner");
    storage
        .create_discovery_session(&other, now())
        .expect("create other session");

    let attempt_id =
        DiscoveryCommitAttemptId::parse("attempt-owned-by-first-session").expect("attempt id");
    let plan = DiscoveryCommitPlan {
        attempt_id: attempt_id.clone(),
        session_id: owner.id.clone(),
        expected_revision: 0,
        manifest_sha256: "1".repeat(64),
        graph_sha256: "2".repeat(64),
        template_id: ProviderTemplateId::from("template-attempt-owner"),
        template_version: 1,
        connection_id: owner.input.connection_id.clone(),
        model_route_ids: vec![ModelRouteId::from("route-attempt-owner")],
        credential_ref: None,
        credential_approval_id: None,
        review_sha256: "3".repeat(64),
        catalog_authority: None,
        previous_selection: DiscoveryPreviousSelection::None,
    };
    plan.validate().expect("valid commit plan");
    let plan_json = serde_json::to_string(&plan).expect("commit plan JSON");
    let plan_sha256 = sha256_hex(plan_json.as_bytes());

    let mut connection = storage.connection().expect("database connection");
    let attempt_guard = suspend_test_trigger(
        &connection,
        "provider_discovery_commit_attempt_initial_state_guard",
    );
    let transaction = connection.transaction().expect("transaction");
    transaction
        .execute(
            "INSERT INTO provider_discovery_commit_attempts (
                 id, session_id, attempt_number, action_id, expected_revision,
                 plan_sha256, plan_json, phase, redaction_version,
                 created_at, updated_at, completed_at
             ) VALUES (
                 ?1, ?2, 1, 'action-prepare-owned-attempt', 0,
                 ?3, ?4, 'compensating', 1, ?5, ?5, NULL
             )",
            rusqlite::params![
                attempt_id.as_str(),
                owner.id.as_str(),
                plan_sha256,
                plan_json,
                now().to_rfc3339(),
            ],
        )
        .expect("insert owned commit attempt");
    restore_test_trigger(&transaction, &attempt_guard);

    let mut restart = write(
        apply(&other, ProviderDiscoveryAction::Begin, '7'),
        None,
        None,
    );
    restart.transition.session.commit_attempt_id = Some(attempt_id.clone());
    restart.transition.session.commit_plan_sha256 = Some(plan_sha256.clone());
    restart.transition.receipt.action_kind = "restart_interrupted".to_owned();
    let error = super::super::prepare_compensation_ledger(&transaction, &restart)
        .expect_err("another session cannot reuse a compensating attempt");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    let error = super::super::validate_failed_compensation_ledger(&transaction, &restart)
        .expect_err("another session cannot validate a foreign compensation ledger");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);

    transaction
        .execute(
            "UPDATE provider_discovery_sessions
             SET revision = revision + 1,
                 next_event_sequence = next_event_sequence + 1,
                 commit_plan_sha256 = ?2,
                 commit_attempt_id = ?3,
                 updated_at = ?4
             WHERE id = ?1",
            rusqlite::params![
                other.id.as_str(),
                plan_sha256,
                attempt_id.as_str(),
                now().to_rfc3339(),
            ],
        )
        .expect("seed corrupt cross-session binding");
    transaction.commit().expect("commit corrupt fixture");
    drop(connection);

    let error = storage
        .get_discovery_session(&other.id)
        .expect_err("cross-session attempt binding must fail during hydration");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
#[allow(clippy::too_many_lines)]
fn compensation_step_failure_and_session_transition_commit_atomically() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let mut session = draft_session("session-atomic-compensation-failure");
    storage
        .create_discovery_session(&session, now())
        .expect("create session");

    let attempt_id =
        DiscoveryCommitAttemptId::parse("attempt-atomic-compensation").expect("attempt id");
    let plan = DiscoveryCommitPlan {
        attempt_id: attempt_id.clone(),
        session_id: session.id.clone(),
        expected_revision: 0,
        manifest_sha256: "8".repeat(64),
        graph_sha256: "9".repeat(64),
        template_id: ProviderTemplateId::from("template-atomic-compensation"),
        template_version: 1,
        connection_id: session.input.connection_id.clone(),
        model_route_ids: vec![ModelRouteId::from("route-atomic-compensation")],
        credential_ref: None,
        credential_approval_id: None,
        review_sha256: "a".repeat(64),
        catalog_authority: None,
        previous_selection: DiscoveryPreviousSelection::None,
    };
    plan.validate().expect("valid commit plan");
    let plan_json = serde_json::to_string(&plan).expect("plan JSON");
    let plan_sha256 = sha256_hex(plan_json.as_bytes());
    let operation_id =
        DiscoveryOperationId::parse("operation-atomic-compensation").expect("operation id");
    let step_id = "step-atomic-compensation";
    let step = DiscoveryCompensationStep {
        action_id: DiscoveryActionId::parse("action-step-atomic-compensation")
            .expect("step action id"),
        ordinal: 0,
        kind: DiscoveryCompensationKind::RestorePreviousSelection,
        target: DiscoveryCompensationTarget::RestorePreviousSelection {
            previous_selection: DiscoveryPreviousSelection::None,
        },
        status: DiscoveryCompensationStatus::Pending,
    };
    step.validate_against(&plan)
        .expect("valid compensation step");
    let step_json = serde_json::to_string(&step).expect("step JSON");
    {
        let mut connection = storage.connection().expect("database connection");
        let attempt_guard = suspend_test_trigger(
            &connection,
            "provider_discovery_commit_attempt_initial_state_guard",
        );
        let operation_guard = suspend_test_trigger(
            &connection,
            "provider_discovery_operation_initial_state_guard",
        );
        let transaction = connection.transaction().expect("transaction");
        transaction
            .execute(
                "INSERT INTO provider_discovery_commit_attempts (
                     id, session_id, attempt_number, action_id, expected_revision,
                     plan_sha256, plan_json, phase, redaction_version,
                     created_at, updated_at, completed_at
                 ) VALUES (
                     ?1, ?2, 1, 'action-prepare-atomic-compensation', 0,
                     ?3, ?4, 'compensating', 1, ?5, ?5, NULL
                 )",
                rusqlite::params![
                    attempt_id.as_str(),
                    session.id.as_str(),
                    plan_sha256,
                    plan_json,
                    now().to_rfc3339(),
                ],
            )
            .expect("insert commit attempt");
        transaction
            .execute(
                "INSERT INTO provider_discovery_compensation_steps (
                     id, commit_attempt_id, ordinal, action_id, step_kind,
                     step_json, status, attempt_count, last_failure_json,
                     redaction_version, created_at, updated_at, completed_at
                 ) VALUES (
                     ?1, ?2, 0, ?3, 'restore_previous_selection',
                     ?4, 'in_progress', 1, NULL, 1, ?5, ?5, NULL
                 )",
                rusqlite::params![
                    step_id,
                    attempt_id.as_str(),
                    step.action_id.as_str(),
                    step_json,
                    now().to_rfc3339(),
                ],
            )
            .expect("insert in-progress compensation step");
        transaction
            .execute(
                "INSERT INTO provider_discovery_operations (
                     id, session_id, operation_kind, side_effect_class, status,
                     action_id, expected_revision, request_sha256, approval_id,
                     approval_grant_sha256, started_at, finished_at, created_at, updated_at
                 ) VALUES (
                     ?1, ?2, 'compensation', 'persistent', 'started',
                     'action-run-atomic-compensation', 0, ?3, NULL, NULL,
                     ?4, NULL, ?4, ?4
                 )",
                rusqlite::params![
                    operation_id.as_str(),
                    session.id.as_str(),
                    "b".repeat(64),
                    now().to_rfc3339(),
                ],
            )
            .expect("insert started compensation operation");
        restore_test_trigger(&transaction, &attempt_guard);
        restore_test_trigger(&transaction, &operation_guard);
        transaction
            .execute(
                "UPDATE provider_discovery_sessions
                 SET state = 'compensating',
                     revision = 1,
                     next_event_sequence = 2,
                     commit_plan_sha256 = ?2,
                     commit_attempt_id = ?3,
                     active_operation_id = ?4,
                     updated_at = ?5
                 WHERE id = ?1",
                rusqlite::params![
                    session.id.as_str(),
                    plan_sha256,
                    attempt_id.as_str(),
                    operation_id.as_str(),
                    now().to_rfc3339(),
                ],
            )
            .expect("activate compensation fixture");
        transaction.commit().expect("commit fixture");
    }

    session.state = DiscoveryState::Compensating;
    session.revision = 1;
    session.next_event_sequence = 2;
    session.commit_plan_sha256 = Some(plan_sha256);
    session.commit_attempt_id = Some(attempt_id);
    session.validate().expect("valid compensating session");
    let failure = DiscoveryFailure {
        code: "compensation_failed".to_owned(),
        message_key: "discovery.compensation.failed".to_owned(),
        recoverable: true,
    };
    assert!(
        storage
            .update_discovery_compensation_status(
                step_id,
                super::super::DiscoveryCompensationStatus::InProgress,
                super::super::DiscoveryCompensationStatus::OutcomeUnknown,
                None,
                now(),
            )
            .is_err(),
        "an unknown step outcome cannot be split from its session and operation transition"
    );
    assert!(
        storage
            .update_discovery_compensation_status(
                step_id,
                super::super::DiscoveryCompensationStatus::InProgress,
                super::super::DiscoveryCompensationStatus::Failed,
                Some(&failure),
                now(),
            )
            .is_err(),
        "a step failure cannot be split from its session transition"
    );
    assert_eq!(
        storage
            .connection()
            .expect("database connection")
            .query_row(
                "SELECT status FROM provider_discovery_compensation_steps WHERE id = ?1",
                [step_id],
                |row| row.get::<_, String>(0),
            )
            .expect("unchanged compensation step"),
        "in_progress"
    );
    let transition = apply(
        &session,
        ProviderDiscoveryAction::CompensationFailed {
            failure: failure.clone(),
        },
        'c',
    );
    let failure_write = write(
        transition,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: operation_id.clone(),
            outcome: super::super::DurableOperationOutcome::Failed,
        }),
    );
    assert!(matches!(
        storage
            .fail_discovery_compensation_and_persist_transition(step_id, &failure_write)
            .expect("atomically fail compensation"),
        PersistDiscoveryTransition::Applied { .. }
    ));
    assert!(matches!(
        storage
            .fail_discovery_compensation_and_persist_transition(step_id, &failure_write)
            .expect("idempotently replay atomic failure"),
        PersistDiscoveryTransition::Replayed { .. }
    ));

    let snapshot = storage
        .get_discovery_session(&session.id)
        .expect("load failed compensation session");
    assert_eq!(snapshot.session.failure, Some(failure.clone()));
    assert!(snapshot.active_operation_id.is_none());
    let stored = storage
        .connection()
        .expect("database connection")
        .query_row(
            "SELECT step.status, step.last_failure_json, operation.status
             FROM provider_discovery_compensation_steps AS step
             JOIN provider_discovery_operations AS operation
               ON operation.id = ?2
             WHERE step.id = ?1",
            rusqlite::params![step_id, operation_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .expect("load atomic failure rows");
    assert_eq!(stored.0, "failed");
    assert_eq!(
        serde_json::from_str::<DiscoveryFailure>(&stored.1).expect("stored failure"),
        failure
    );
    assert_eq!(stored.2, "failed");
}
