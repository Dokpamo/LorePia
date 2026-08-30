use super::support::*;
use super::*;

#[test]
fn reconciled_outcome_unknown_discovery_credential_authority_remains_valid() {
    let fixture = seed_completed_discovery_authority_with_mode(
        "discovery-authority-reconciled",
        CompletedDiscoveryAuthorityMode::Reconciled,
    );
    validate_discovery_credential_ownership_authority(
        &fixture.storage.connection().expect("authority database"),
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("exact confirmed commit completion remains valid operation-scoped authority");
}

#[test]
fn confirmed_no_effect_restart_discovery_credential_authority_remains_valid() {
    let fixture = seed_completed_discovery_authority_with_mode(
        "discovery-authority-no-effect-retry",
        CompletedDiscoveryAuthorityMode::UnknownNoEffectRetry,
    );
    validate_discovery_credential_ownership_authority(
        &fixture.storage.connection().expect("authority database"),
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("confirmed no-effect restart and final success remain valid authority");
}

#[test]
fn missing_abandonment_ancestor_revokes_active_and_archived_authority() {
    for archived in [false, true] {
        let fixture = seed_completed_discovery_authority_with_mode(
            if archived {
                "missing-abandonment-ancestor-archived"
            } else {
                "missing-abandonment-ancestor-active"
            },
            CompletedDiscoveryAuthorityMode::PreparedInterruptedRetry,
        );
        project_completed_discovery_credential_authority(&fixture);
        if archived {
            archive_credential_bound_connection(&fixture.storage, &fixture.connection_id);
            fixture
                .storage
                .list_provider_credential_slot_garbage()
                .expect("intact abandonment ancestor authorizes archived slot cleanup");
        } else {
            fixture
                .storage
                .ensure_provider_credential_access_settled(&fixture.connection_id)
                .expect("intact abandonment ancestor authorizes active access");
        }

        let database = fixture.storage.connection().expect("abandonment database");
        let delete = || {
            database.execute(
                "DELETE FROM provider_discovery_native_credential_abandoned_reservations
                 WHERE operation_id = ?1",
                [fixture.operation_id.as_str()],
            )
        };
        delete().expect_err("abandonment deletion guard must preserve retry ancestry");
        let guard = suspend_test_trigger(
            &database,
            "provider_discovery_native_credential_abandonment_no_delete",
        );
        delete().expect("inject missing abandonment ancestor after test-only guard bypass");
        restore_test_trigger(&database, &guard);
        drop(database);

        let error = if archived {
            fixture
                .storage
                .list_provider_credential_slot_garbage()
                .expect_err("archived GC must reject a missing abandonment ancestor")
        } else {
            fixture
                .storage
                .ensure_provider_credential_access_settled(&fixture.connection_id)
                .expect_err("active access must reject a missing abandonment ancestor")
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }
}

#[test]
fn confirmed_commit_compensation_uses_original_operation_authority_after_reopen() {
    let fixture = seed_completed_discovery_authority_with_mode(
        "discovery-compensation-confirmed-commit",
        CompletedDiscoveryAuthorityMode::ConfirmedCommitCompensation,
    );
    let attempt = fixture
        .storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load confirmed-commit compensation attempt");
    let authority = fixture
        .storage
        .get_discovery_credential_compensation_operation_id(
            &fixture.session_id,
            &fixture.attempt_id,
            &attempt.plan_sha256,
        )
        .expect("load confirmed-commit compensation authority");
    assert_eq!(authority, fixture.operation_id);

    let root_path = fixture.root.path().to_path_buf();
    let session_id = fixture.session_id.clone();
    let attempt_id = fixture.attempt_id.clone();
    let operation_id = fixture.operation_id.clone();
    let plan_sha256 = attempt.plan_sha256;
    drop(fixture.storage);
    let reopened = Storage::open_with_deferred_discovery_recovery(root_path)
        .expect("reopen confirmed-commit compensation with Core-owned recovery");
    assert_eq!(
        reopened
            .get_discovery_credential_compensation_operation_id(
                &session_id,
                &attempt_id,
                &plan_sha256,
            )
            .expect("reload confirmed-commit compensation authority"),
        operation_id
    );
}

#[test]
fn confirmed_no_effect_compensation_uses_original_operation_authority_after_reopen() {
    let fixture = seed_completed_discovery_authority_with_mode(
        "discovery-compensation-confirmed-no-effect",
        CompletedDiscoveryAuthorityMode::ConfirmedNoEffectCompensation,
    );
    let attempt = fixture
        .storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load confirmed-no-effect compensation attempt");
    let authority = fixture
        .storage
        .get_discovery_credential_compensation_operation_id(
            &fixture.session_id,
            &fixture.attempt_id,
            &attempt.plan_sha256,
        )
        .expect("load confirmed-no-effect compensation authority");
    assert_eq!(authority, fixture.operation_id);

    let root_path = fixture.root.path().to_path_buf();
    let session_id = fixture.session_id.clone();
    let attempt_id = fixture.attempt_id.clone();
    let operation_id = fixture.operation_id.clone();
    let plan_sha256 = attempt.plan_sha256;
    drop(fixture.storage);
    let reopened = Storage::open_with_deferred_discovery_recovery(root_path)
        .expect("reopen confirmed-no-effect compensation with Core-owned recovery");
    assert_eq!(
        reopened
            .get_discovery_credential_compensation_operation_id(
                &session_id,
                &attempt_id,
                &plan_sha256,
            )
            .expect("reload confirmed-no-effect compensation authority"),
        operation_id
    );
}

#[test]
fn recovery_authority_accepts_exact_cancelled_prepared_reservation() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture =
        seed_prepared_native_credential_commit(&storage, "cancelled-prepared-reservation");
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
        .expect("reserve prepared native execution");
    let cancel = apply(&fixture.session, ProviderDiscoveryAction::Cancel, 'c');
    storage
        .persist_discovery_transition(&write(cancel, None, None))
        .expect("persist cancellation before settling prepared operation");

    let strict_error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect_err("normal install authority must reject cancelled Prepared state");
    assert_eq!(strict_error.code, CoreErrorCode::StorageCorrupted);
    storage
        .validate_discovery_credential_install_recovery_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect("recovery accepts exact cancelled Prepared reservation");
    let execution = storage
        .get_discovery_native_credential_execution(&fixture.operation_id)
        .expect("load cancelled reservation")
        .expect("cancelled reservation remains durable");
    assert_eq!(
        execution.physical_authority_id,
        reserved.physical_authority_id
    );
    assert!(execution.store_started_at.is_none());
    drop(storage);

    let reopened = Storage::open_with_deferred_discovery_recovery(root.path())
        .expect("reopen cancelled reservation before recovery");
    reopened
        .validate_discovery_credential_install_recovery_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect("reopened recovery preserves exact cancelled Prepared authority");
    assert_eq!(
        reopened
            .get_discovery_native_credential_execution(&fixture.operation_id)
            .expect("load reopened cancelled reservation")
            .expect("reopened reservation exists")
            .physical_authority_id,
        reserved.physical_authority_id
    );
}

#[test]
fn recovery_authority_accepts_exact_cancelled_started_execution() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "cancelled-started-execution");
    let execution = storage
        .get_discovery_native_credential_execution(&fixture.operation_id)
        .expect("load started native execution")
        .expect("started execution exists");
    let cancel = apply(&fixture.session, ProviderDiscoveryAction::Cancel, 'd');
    storage
        .persist_discovery_transition(&write(cancel, None, None))
        .expect("persist cancellation while native execution is Started");

    storage
        .validate_discovery_credential_install_recovery_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect("recovery accepts exact cancelled Started execution");
    drop(storage);

    let reopened = Storage::open_with_deferred_discovery_recovery(root.path())
        .expect("reopen cancelled execution before recovery");
    reopened
        .validate_discovery_credential_install_recovery_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &fixture.operation_id,
        )
        .expect("reopened recovery preserves exact cancelled Started authority");
    assert_eq!(
        reopened
            .get_discovery_native_credential_execution(&fixture.operation_id)
            .expect("load reopened cancelled execution")
            .expect("reopened started execution exists"),
        execution
    );
}

#[test]
fn startup_recovery_records_interruption_without_replaying_effect() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-recovery");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'd');
    let operation_id = DiscoveryOperationId::parse("operation-recovery").expect("operation id");
    storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id.clone()), None))
        .expect("persist begin");
    assert!(
        storage
            .mark_discovery_operation_started(&operation_id, now())
            .expect("mark operation started")
    );

    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen and recover storage");
    let recovered = reopened
        .get_discovery_session(&draft.id)
        .expect("load recovered discovery session");
    assert_eq!(
        recovered.session.state,
        lorepia_domain::discovery::DiscoveryState::Interrupted
    );
    assert!(
        reopened
            .get_current_discovery_operation(&draft.id)
            .expect("query active operation")
            .is_none()
    );
    let operation_status = reopened
        .connection()
        .expect("database connection")
        .query_row(
            "SELECT status FROM provider_discovery_operations WHERE id = ?1",
            [operation_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("recovered operation status");
    assert_eq!(operation_status, "interrupted");
}

#[test]
fn deferred_open_leaves_recovery_untouched_until_core_classification() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-deferred-recovery");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'e');
    let operation_id =
        DiscoveryOperationId::parse("operation-deferred-recovery").expect("operation id");
    storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id.clone()), None))
        .expect("persist begin");
    assert!(
        storage
            .mark_discovery_operation_started(&operation_id, now())
            .expect("mark operation started")
    );
    drop(storage);

    let deferred = Storage::open_with_deferred_discovery_recovery(root.path())
        .expect("open storage with deferred discovery recovery");
    let untouched = deferred
        .get_discovery_session(&draft.id)
        .expect("load unrecovered session");
    assert_eq!(
        untouched.session.state,
        lorepia_domain::discovery::DiscoveryState::ResolvingKnownProvider
    );
    assert_eq!(untouched.active_operation_id.as_ref(), Some(&operation_id));
    assert_eq!(
        deferred
            .get_current_discovery_operation(&draft.id)
            .expect("load unrecovered operation")
            .expect("active operation")
            .status,
        super::super::DiscoveryOperationStatus::Started
    );

    let recovered = deferred
        .recover_unfinished_discovery_operations(now())
        .expect("apply explicit conservative recovery");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].operation_id, operation_id);
    assert_eq!(
        deferred
            .get_discovery_session(&draft.id)
            .expect("load explicitly recovered session")
            .session
            .state,
        lorepia_domain::discovery::DiscoveryState::Interrupted
    );
}

#[test]
fn unfinished_recovery_scan_is_not_bounded_by_latest_history() {
    const SESSION_COUNT: usize = 1_001;

    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    for index in 0..SESSION_COUNT {
        let session_id = format!("session-recovery-boundary-{index:04}");
        let draft = draft_session(&session_id);
        let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'b');
        let operation_id =
            DiscoveryOperationId::parse(format!("operation-recovery-boundary-{index:04}"))
                .expect("operation id");
        let mut transition_write = write(begin, Some(operation_id), None);
        transition_write.occurred_at =
            now() + chrono::Duration::seconds(i64::try_from(index).expect("bounded index"));
        storage
            .begin_discovery_session(&draft, &transition_write)
            .expect("persist unfinished discovery");
    }

    let latest = storage
        .list_discovery_sessions(1_000)
        .expect("list bounded discovery history");
    assert_eq!(latest.len(), 1_000);
    assert!(
        latest
            .iter()
            .all(|snapshot| snapshot.session.id.as_str() != "session-recovery-boundary-0000")
    );

    let recovery = storage
        .list_unfinished_discovery_sessions_for_recovery()
        .expect("scan every unfinished discovery");
    assert_eq!(recovery.len(), SESSION_COUNT);
    assert_eq!(
        recovery
            .first()
            .expect("oldest unfinished discovery")
            .session
            .id
            .as_str(),
        "session-recovery-boundary-0000"
    );
}

#[test]
fn recovery_exception_rejects_a_non_assistant_operation_id() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-invalid-recovery-exception");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'd');
    let operation_id =
        DiscoveryOperationId::parse("operation-not-assistant").expect("operation id");
    storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id.clone()), None))
        .expect("persist begin");
    let error = storage
        .recover_unfinished_discovery_operations_except(
            now(),
            &std::collections::BTreeSet::from([operation_id.clone()]),
        )
        .expect_err("a read-only operation must not bypass recovery");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    let unchanged = storage
        .get_discovery_session(&draft.id)
        .expect("load unchanged discovery");
    assert_eq!(
        unchanged.session.state,
        lorepia_domain::discovery::DiscoveryState::ResolvingKnownProvider
    );
    assert_eq!(unchanged.active_operation_id.as_ref(), Some(&operation_id));
}
