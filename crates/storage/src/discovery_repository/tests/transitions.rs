use super::support::*;
use super::*;

#[test]
fn terminal_discovery_history_cannot_be_inserted_as_initial_state() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("discovery-initial-state-guard");
    let input_json = canonical_json_result(
        serde_json::to_value(&draft.input),
        "initial-state guard input",
    )
    .expect("canonical input JSON");
    let timestamp = now().to_rfc3339();
    let database = storage.connection().expect("database connection");
    let session_error = database
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, revision, next_event_sequence, sanitized_input_json,
                 redaction_version, created_at, updated_at
             ) VALUES (?1, 'ready', 1, 2, ?2, 1, ?3, ?3)",
            rusqlite::params!["forged-ready-session", input_json, timestamp],
        )
        .expect_err("terminal discovery session insert must be rejected");
    assert!(
        session_error
            .to_string()
            .contains("provider discovery session must begin in its initial state")
    );
    drop(database);
    storage
        .create_discovery_session(&draft, now())
        .expect("create canonical draft session");
    let database = storage.connection().expect("database connection");
    let attempt_error = database
        .execute(
            "INSERT INTO provider_discovery_commit_attempts (
                 id, session_id, attempt_number, action_id, expected_revision,
                 plan_sha256, plan_json, phase, redaction_version,
                 created_at, updated_at, completed_at
             ) VALUES (?1, ?2, 1, ?3, 0, ?4, '{}', 'completed', 1, ?5, ?5, ?5)",
            rusqlite::params![
                "forged-completed-attempt",
                draft.id.as_str(),
                "forged-completed-action",
                "a".repeat(64),
                timestamp,
            ],
        )
        .expect_err("terminal discovery attempt insert must be rejected");
    assert!(
        attempt_error
            .to_string()
            .contains("provider discovery commit attempt must begin prepared")
    );
    let operation_error = database
        .execute(
            "INSERT INTO provider_discovery_operations (
                 id, session_id, operation_kind, side_effect_class, status,
                 action_id, expected_revision, request_sha256,
                 started_at, finished_at, created_at, updated_at
             ) VALUES (
                 ?1, ?2, 'atomic_commit', 'persistent', 'succeeded',
                 ?3, 1, ?4, ?5, ?5, ?5, ?5
             )",
            rusqlite::params![
                "forged-succeeded-operation",
                draft.id.as_str(),
                "forged-operation-action",
                "b".repeat(64),
                timestamp,
            ],
        )
        .expect_err("terminal discovery operation insert must be rejected");
    assert!(
        operation_error
            .to_string()
            .contains("provider discovery operation must begin prepared")
    );
}

#[test]
fn session_scoped_outbox_poll_bypasses_a_full_foreign_session_page() {
    const SESSION_COUNT: usize = 101;
    const GLOBAL_PAGE_SIZE: u32 = 100;

    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    for index in 0..SESSION_COUNT {
        let id = format!("session-outbox-starvation-{index:04}");
        let draft = draft_session(&id);
        let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'a');
        let operation_id =
            DiscoveryOperationId::parse(format!("operation-{id}")).expect("operation id");
        storage
            .begin_discovery_session(&draft, &write(begin, Some(operation_id), None))
            .expect("persist discovery begin event");
    }

    let selected_session = DiscoverySessionId::from("session-outbox-starvation-0100");
    let selected_snapshot = storage
        .get_discovery_session(&selected_session)
        .expect("load selected discovery session");
    let cancel = apply(
        &selected_snapshot.session,
        ProviderDiscoveryAction::Cancel,
        'b',
    );
    storage
        .persist_discovery_transition(&write(cancel, None, None))
        .expect("persist the selected session's next event");

    let first_global = storage
        .poll_discovery_events(GLOBAL_PAGE_SIZE, now())
        .expect("poll first global page");
    assert_eq!(first_global.len(), GLOBAL_PAGE_SIZE as usize);
    assert!(
        first_global
            .iter()
            .all(|event| event.event.session_id != selected_session),
        "the selected session is beyond the bounded global page"
    );

    let repeated_global = storage
        .poll_discovery_events(GLOBAL_PAGE_SIZE, now())
        .expect("repeat global page without acknowledgements");
    assert_eq!(
        repeated_global
            .iter()
            .map(|event| &event.event.id)
            .collect::<Vec<_>>(),
        first_global
            .iter()
            .map(|event| &event.event.id)
            .collect::<Vec<_>>(),
        "unacknowledged foreign sessions keep occupying the same global page"
    );
    assert!(
        repeated_global
            .iter()
            .all(|event| event.delivery_attempts == 2)
    );

    let selected_events = storage
        .poll_discovery_events_for_session(&selected_session, GLOBAL_PAGE_SIZE, now())
        .expect("poll the selected session independently of the global backlog");
    assert_eq!(selected_events.len(), 1);
    assert_eq!(selected_events[0].event.session_id, selected_session);
    assert_eq!(selected_events[0].delivery_attempts, 1);

    assert!(
        storage
            .ack_discovery_event(&selected_events[0].event.id, now())
            .expect("ack selected event")
    );

    let next_selected_events = storage
        .poll_discovery_events_for_session(&selected_session, GLOBAL_PAGE_SIZE, now())
        .expect("poll selected session after acknowledging its first event");
    assert_eq!(next_selected_events.len(), 1);
    assert_eq!(next_selected_events[0].event.session_id, selected_session);
    assert_eq!(next_selected_events[0].event.sequence, 2);
    assert!(
        storage
            .ack_discovery_event(&next_selected_events[0].event.id, now())
            .expect("ack selected session's next event")
    );
    assert!(
        storage
            .poll_discovery_events_for_session(&selected_session, GLOBAL_PAGE_SIZE, now())
            .expect("poll selected session after acknowledgement")
            .is_empty()
    );
}

#[test]
fn native_no_effect_attestation_and_transition_roll_back_together() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "native-atomic-rollback");

    let mut stale_session = fixture.session.clone();
    stale_session.revision = 0;
    stale_session.next_event_sequence = 1;
    stale_session
        .validate()
        .expect("valid stale session snapshot");
    let (stale_write, attestation) =
        native_no_effect_completion(&storage, &fixture, &stale_session);
    storage
        .persist_native_no_effect_discovery_transition(&stale_write, &attestation)
        .expect_err("stale session revision must roll back the whole transaction");
    assert_eq!(operation_status(&storage, &fixture.operation_id), "started");
    assert!(
        storage
            .get_discovery_native_no_effect_attestation(&fixture.operation_id)
            .expect("load rolled-back attestation")
            .is_none()
    );

    let (write, _) = native_no_effect_completion(&storage, &fixture, &fixture.session);
    let error = storage
        .persist_discovery_transition(&write)
        .expect_err("ordinary completion API cannot forge a native attestation");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(operation_status(&storage, &fixture.operation_id), "started");
}

#[test]
fn native_store_attempt_and_started_transition_roll_back_together() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_prepared_native_credential_commit(&storage, "store-attempt-rollback");
    let reserved = storage
        .reserve_discovery_credential_install_execution(
            &super::super::DiscoveryNativeCredentialExecutionReservation {
                operation_id: fixture.operation_id.clone(),
                session_id: fixture.session.id.clone(),
                commit_attempt_id: fixture.attempt_id.clone(),
                commit_plan_sha256: fixture.plan_sha256.clone(),
                connection_id: fixture.session.input.connection_id.clone(),
                connection_binding_sha256: "b".repeat(64),
                reserved_at: now() + chrono::Duration::milliseconds(1),
            },
        )
        .expect("reserve native execution before rollback test");
    let database = storage.connection().expect("rollback database");
    database
        .execute_batch(&format!(
            "CREATE TRIGGER synthetic_native_started_transition_abort
             BEFORE UPDATE OF status ON provider_discovery_operations
             WHEN NEW.id = '{}' AND NEW.status = 'started'
             BEGIN
                 SELECT RAISE(ABORT, 'synthetic Started transition failure');
             END;",
            fixture.operation_id.as_str(),
        ))
        .expect("install synthetic post-store-insert failure");
    drop(database);

    storage
        .start_reserved_discovery_credential_install_execution(
            &super::super::DiscoveryNativeCredentialStoreAttemptStart {
                operation_id: fixture.operation_id.clone(),
                physical_authority_id: reserved.physical_authority_id.clone(),
                started_at: now() + chrono::Duration::milliseconds(2),
            },
        )
        .expect_err("operation transition failure must roll back store attempt");
    let database = storage.connection().expect("verify rollback database");
    let store_attempts = database
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_native_credential_store_attempts
             WHERE operation_id = ?1",
            [fixture.operation_id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count rolled-back store attempts");
    assert_eq!(store_attempts, 0);
    database
        .execute_batch("DROP TRIGGER synthetic_native_started_transition_abort;")
        .expect("remove synthetic transition failure");
    drop(database);
    assert_eq!(
        operation_status(&storage, &fixture.operation_id),
        "prepared"
    );
    let execution = storage
        .get_discovery_native_credential_execution(&fixture.operation_id)
        .expect("load reservation after rollback")
        .expect("reservation survives rollback");
    assert_eq!(
        execution.physical_authority_id,
        reserved.physical_authority_id
    );
    assert!(execution.store_started_at.is_none());
}
