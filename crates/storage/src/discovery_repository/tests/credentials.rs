use super::support::*;
use super::*;

#[test]
fn archived_discovery_owned_slot_garbage_revalidates_after_reopen_without_granting_access() {
    let fixture = seed_completed_discovery_authority("archived-discovery-slot-gc-reopen");
    let authority_sequence = project_completed_discovery_credential_authority(&fixture);
    archive_credential_bound_connection(&fixture.storage, &fixture.connection_id);

    let database = fixture
        .storage
        .connection()
        .expect("archived authority database");
    let active_error = validate_discovery_credential_ownership_authority(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect_err("archived discovery authority must not grant current credential access");
    assert_eq!(active_error.code, CoreErrorCode::StorageCorrupted);
    validate_archived_discovery_credential_ownership_authority_for_slot_gc(
        &database,
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("archived discovery history retains bounded slot-GC authority");
    drop(database);

    let garbage = fixture
        .storage
        .list_provider_credential_slot_garbage()
        .expect("list archived discovery slot garbage");
    assert_eq!(garbage.len(), 1);
    assert_eq!(garbage[0].connection_id, fixture.connection_id);
    assert_eq!(garbage[0].authority_sequence, authority_sequence);
    assert_eq!(
        garbage[0].authority.authority_id,
        fixture.physical_authority_id
    );
    assert_eq!(
        garbage[0].authority.connection_binding_sha256,
        fixture.binding_sha256
    );
    assert_eq!(
        garbage[0].status,
        ProviderCredentialSlotGarbageStatus::Pending
    );

    let root_path = fixture.root.path().to_path_buf();
    let connection_id = fixture.connection_id.clone();
    drop(fixture.storage);
    let reopened = Storage::open(root_path).expect("reopen archived authority");
    let reopened_garbage = reopened
        .list_provider_credential_slot_garbage()
        .expect("revalidate archived slot garbage after reopen");
    assert_eq!(reopened_garbage, garbage);
    reopened
        .ensure_provider_credential_access_settled(&connection_id)
        .expect_err("reopen must not turn historical GC authority into current access");
}

#[test]
fn tampered_archived_connection_or_discovery_history_fails_slot_garbage_closed() {
    for (id, tamper) in [
        ("archived-discovery-slot-gc-connection-tamper", "connection"),
        ("archived-discovery-slot-gc-history-tamper", "history"),
    ] {
        let fixture = seed_completed_discovery_authority(id);
        project_completed_discovery_credential_authority(&fixture);
        archive_credential_bound_connection(&fixture.storage, &fixture.connection_id);
        fixture
            .storage
            .list_provider_credential_slot_garbage()
            .expect("untampered archived slot garbage is valid");

        let database = fixture
            .storage
            .connection()
            .expect("tamper authority database");
        if tamper == "connection" {
            database
                .execute(
                    "UPDATE provider_connections
                     SET api_origin = 'https://tampered.example'
                     WHERE id = ?1 AND archived_at IS NOT NULL",
                    [fixture.connection_id.as_str()],
                )
                .expect("inject archived connection tamper");
        } else {
            let delete_review_approval = || {
                database.execute(
                    "DELETE FROM provider_discovery_approvals
                     WHERE session_id = ?1 AND approval_kind = 'review'",
                    [fixture.session_id.as_str()],
                )
            };
            delete_review_approval()
                .expect_err("immutable discovery approval guard must reject history deletion");
            database
                .execute_batch("DROP TRIGGER provider_discovery_approval_no_delete;")
                .expect("drop approval delete guard only for corruption fixture");
            delete_review_approval()
                .expect("inject archived discovery-history tamper after dropping test guard");
            let remaining_review_approvals = database
                .query_row(
                    "SELECT COUNT(*)
                     FROM provider_discovery_approvals
                     WHERE session_id = ?1 AND approval_kind = 'review'",
                    [fixture.session_id.as_str()],
                    |row| row.get::<_, u32>(0),
                )
                .expect("count tampered discovery approvals");
            assert_eq!(remaining_review_approvals, 0);
        }
        drop(database);

        let error = fixture
            .storage
            .list_provider_credential_slot_garbage()
            .expect_err("tampered archived authority must fail slot garbage closed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted, "{tamper}");
    }
}

#[test]
fn model_sync_mutation_does_not_expire_discovery_credential_authority() {
    let fixture = seed_completed_discovery_authority("discovery-authority-model-sync");
    let before = fixture
        .storage
        .list_model_routes(&fixture.connection_id)
        .expect("load routes before model sync");
    let observed_at = before
        .iter()
        .flat_map(|route| [Some(route.first_seen_at), route.last_seen_at])
        .flatten()
        .max()
        .unwrap_or_else(now)
        + chrono::Duration::minutes(1);
    fixture
        .storage
        .reconcile_model_routes(&fixture.connection_id, &before, observed_at)
        .expect("reconcile mutable discovery routes");
    let after = fixture
        .storage
        .list_model_routes(&fixture.connection_id)
        .expect("load routes after model sync");
    assert_ne!(before, after, "model sync must mutate the live route graph");

    validate_discovery_credential_ownership_authority(
        &fixture.storage.connection().expect("authority database"),
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("mutable model-sync graph state does not replace credential authority");
}

fn insert_test_native_no_effect_execution_binding(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    write: &DiscoveryTransitionWrite,
    attestation: &DiscoveryNativeNoEffectAttestationWrite,
) {
    let execution = storage
        .get_discovery_native_credential_execution(&fixture.operation_id)
        .expect("load native execution for typed binding")
        .expect("started native execution");
    let execution_binding_sha256 = super::super::native_no_effect_execution_binding_sha256(
        attestation,
        &execution.connection_binding_sha256,
        write.occurred_at,
    )
    .expect("derive exact native execution binding");
    let connection = storage.connection().expect("database connection");
    connection
        .execute(
            "INSERT INTO provider_discovery_native_no_effect_execution_bindings (
                 operation_id, physical_authority_id, session_id,
                 commit_attempt_id, commit_plan_sha256, connection_id,
                 connection_binding_sha256, attestation_evidence_sha256,
                 execution_binding_sha256, attested_at,
                 schema_version, redaction_version
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 1, 1)",
            rusqlite::params![
                attestation.operation_id.as_str(),
                attestation.physical_authority_id,
                attestation.session_id.as_str(),
                attestation.commit_attempt_id.as_str(),
                attestation.commit_plan_sha256,
                attestation.connection_id.as_str(),
                execution.connection_binding_sha256,
                attestation.evidence_sha256,
                execution_binding_sha256,
                write.occurred_at.to_rfc3339(),
            ],
        )
        .expect("insert exact physical execution companion");
}

#[test]
fn native_no_effect_attestation_trigger_requires_exact_typed_binding() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "native-trigger-binding");
    let (write, attestation) = native_no_effect_completion(&storage, &fixture, &fixture.session);
    let attested_at = write.occurred_at.to_rfc3339();
    insert_test_native_no_effect_execution_binding(&storage, &fixture, &write, &attestation);
    let connection = storage.connection().expect("database connection");

    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'interrupted', finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                rusqlite::params![fixture.operation_id.as_str(), now().to_rfc3339()],
            )
            .is_err(),
        "a persistent started operation cannot be interrupted without an attestation"
    );
    let insert = |connection_id: &str, owner: &str, evidence_sha256: &str| {
        connection.execute(
            "INSERT INTO provider_discovery_native_no_effect_attestations (
                 operation_id, session_id, commit_attempt_id, commit_plan_sha256,
                 connection_id, attestation_kind, evidence_sha256, recovery_owner,
                 schema_version, redaction_version, attested_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 'credential_slot_missing', ?6, ?7, 1, 1, ?8)",
            rusqlite::params![
                attestation.operation_id.as_str(),
                attestation.session_id.as_str(),
                attestation.commit_attempt_id.as_str(),
                attestation.commit_plan_sha256,
                connection_id,
                evidence_sha256,
                owner,
                attested_at,
            ],
        )
    };
    assert!(
        insert(
            attestation.connection_id.as_str(),
            "core",
            &attestation.evidence_sha256,
        )
        .is_err(),
        "only the typed native recovery owner is accepted"
    );
    assert!(
        insert(
            "wrong-native-slot",
            "native_platform",
            &attestation.evidence_sha256,
        )
        .is_err(),
        "the attestation must bind the credential slot in the commit plan"
    );
    assert!(
        insert(
            attestation.connection_id.as_str(),
            "native_platform",
            &"0".repeat(64),
        )
        .is_err(),
        "the attestation evidence digest must be recomputed from its exact binding"
    );
    insert(
        attestation.connection_id.as_str(),
        "native_platform",
        &attestation.evidence_sha256,
    )
    .expect("insert exact native attestation binding");
    connection
        .execute(
            "UPDATE provider_discovery_sessions
             SET revision = revision + 1,
                 next_event_sequence = next_event_sequence + 1,
                 commit_plan_sha256 = ?2
             WHERE id = ?1",
            rusqlite::params![fixture.session.id.as_str(), "8".repeat(64)],
        )
        .expect("simulate a detached session binding");
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_operations
                 SET status = 'interrupted', finished_at = ?2, updated_at = ?2
                 WHERE id = ?1",
                rusqlite::params![fixture.operation_id.as_str(), attested_at],
            )
            .is_err(),
        "the transition trigger must re-check the exact current commit binding"
    );
    drop(connection);
    assert_eq!(operation_status(&storage, &fixture.operation_id), "started");
    drop(storage);
    let Err(error) = Storage::open(root.path()) else {
        panic!("detached native attestation unexpectedly reopened");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn native_execution_reservation_guards_and_runtime_validation_fail_closed() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "execution-table-guards");
    let database = storage.connection().expect("execution database");
    assert_native_execution_table_is_append_only(
        &database,
        "provider_discovery_native_credential_executions",
        &fixture.operation_id,
    );
    bypass_native_execution_table_version_guard(
        &database,
        "provider_discovery_native_credential_executions",
        "provider_discovery_native_credential_execution_no_update",
        &fixture.operation_id,
    );
    drop(database);
    let error = storage
        .get_discovery_native_credential_execution(&fixture.operation_id)
        .expect_err("unsupported execution version must fail runtime validation");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn native_store_attempt_guards_and_runtime_validation_fail_closed() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "store-attempt-table-guards");
    let database = storage.connection().expect("store-attempt database");
    assert_native_execution_table_is_append_only(
        &database,
        "provider_discovery_native_credential_store_attempts",
        &fixture.operation_id,
    );
    bypass_native_execution_table_version_guard(
        &database,
        "provider_discovery_native_credential_store_attempts",
        "provider_discovery_native_credential_store_attempt_no_update",
        &fixture.operation_id,
    );
    drop(database);
    let error = storage
        .get_discovery_native_credential_execution(&fixture.operation_id)
        .expect_err("unsupported store-attempt version must fail runtime validation");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn native_abandonment_guards_and_runtime_validation_fail_closed() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_prepared_native_credential_commit(&storage, "abandonment-table-guards");
    storage
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
        .expect("reserve execution before abandonment");
    let interrupted = apply(
        &fixture.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        'e',
    );
    let mut interrupted_write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: fixture.operation_id.clone(),
            outcome: DurableOperationOutcome::Interrupted,
        }),
    );
    interrupted_write.occurred_at = now() + chrono::Duration::milliseconds(2);
    storage
        .persist_discovery_transition(&interrupted_write)
        .expect("capture exact abandoned reservation");

    let database = storage.connection().expect("abandonment database");
    assert_native_execution_table_is_append_only(
        &database,
        "provider_discovery_native_credential_abandoned_reservations",
        &fixture.operation_id,
    );
    bypass_native_execution_table_version_guard(
        &database,
        "provider_discovery_native_credential_abandoned_reservations",
        "provider_discovery_native_credential_abandonment_no_update",
        &fixture.operation_id,
    );
    drop(database);
    let error = storage
        .get_discovery_native_credential_execution(&fixture.operation_id)
        .expect_err("unsupported abandonment version must fail runtime validation");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn native_no_effect_binding_guards_and_runtime_validation_fail_closed() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "binding-table-guards");
    let (write, attestation) = native_no_effect_completion(&storage, &fixture, &fixture.session);
    storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect("persist exact no-effect execution binding");
    let database = storage.connection().expect("binding database");
    assert_native_execution_table_is_append_only(
        &database,
        "provider_discovery_native_no_effect_execution_bindings",
        &fixture.operation_id,
    );
    bypass_native_execution_table_version_guard(
        &database,
        "provider_discovery_native_no_effect_execution_bindings",
        "provider_discovery_native_no_effect_execution_binding_no_update",
        &fixture.operation_id,
    );
    drop(database);
    let error = storage
        .get_discovery_native_no_effect_attestation(&fixture.operation_id)
        .expect_err("unsupported no-effect execution binding must fail runtime validation");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn native_no_effect_attestation_is_immutable_and_restart_durable() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "native-restart-durable");
    let (write, attestation) = native_no_effect_completion(&storage, &fixture, &fixture.session);

    storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect("persist exact native attestation and interrupt");
    storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect("exact replay remains idempotent");
    assert_eq!(
        operation_status(&storage, &fixture.operation_id),
        "interrupted"
    );
    let record = storage
        .get_discovery_native_no_effect_attestation(&fixture.operation_id)
        .expect("load native attestation")
        .expect("native attestation record");
    assert_eq!(record.evidence_sha256, attestation.evidence_sha256);
    let connection = storage.connection().expect("database connection");
    assert!(
        connection
            .execute(
                "UPDATE provider_discovery_native_no_effect_attestations
                 SET evidence_sha256 = ?2 WHERE operation_id = ?1",
                rusqlite::params![fixture.operation_id.as_str(), "6".repeat(64)],
            )
            .is_err()
    );
    assert!(
        connection
            .execute(
                "DELETE FROM provider_discovery_native_no_effect_attestations
                 WHERE operation_id = ?1",
                [fixture.operation_id.as_str()],
            )
            .is_err()
    );
    drop(connection);
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen storage");
    let reopened_record = reopened
        .get_discovery_native_no_effect_attestation(&fixture.operation_id)
        .expect("load attestation after restart")
        .expect("durable native attestation");
    assert_eq!(reopened_record, record);
    assert_eq!(
        reopened
            .get_discovery_session(&fixture.session.id)
            .expect("load interrupted session after restart")
            .session
            .state,
        DiscoveryState::Interrupted
    );
}

#[test]
fn native_no_effect_attestation_no_replace_guard_preserves_original_row() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "native-no-replace-guard");
    let (write, attestation) = native_no_effect_completion(&storage, &fixture, &fixture.session);
    storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect("persist exact native attestation and physical execution binding");
    let database = storage.connection().expect("database connection");
    let binding_guard = suspend_test_trigger(
        &database,
        "provider_discovery_native_no_effect_attestation_binding",
    );
    let companion_guard = suspend_test_trigger(
        &database,
        "provider_discovery_native_no_effect_schema37_companion_required",
    );
    let replace_error = database
        .execute(
            "INSERT OR REPLACE INTO provider_discovery_native_no_effect_attestations
             SELECT * FROM provider_discovery_native_no_effect_attestations
             WHERE operation_id = ?1",
            [fixture.operation_id.as_str()],
        )
        .expect_err("native attestation REPLACE must be rejected");
    assert!(
        replace_error.to_string().contains("cannot replace history"),
        "unexpected native attestation REPLACE rejection: {replace_error}"
    );
    restore_test_trigger(&database, &binding_guard);
    restore_test_trigger(&database, &companion_guard);
    let stored_hash = database
        .query_row(
            "SELECT evidence_sha256
             FROM provider_discovery_native_no_effect_attestations
             WHERE operation_id = ?1",
            [fixture.operation_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load preserved native attestation row");
    assert_eq!(stored_hash, attestation.evidence_sha256);
}

#[test]
fn replaced_native_no_effect_attestation_fails_runtime_validation() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "native-replace-runtime");
    let (write, attestation) = native_no_effect_completion(&storage, &fixture, &fixture.session);
    storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect("persist exact native attestation");
    let original = storage
        .get_discovery_native_no_effect_attestation(&fixture.operation_id)
        .expect("load original native attestation")
        .expect("original native attestation");
    let database = storage.connection().expect("database connection");
    database
        .execute_batch(
            "DROP TRIGGER provider_discovery_native_no_effect_attestation_no_replace;
             DROP TRIGGER provider_discovery_native_no_effect_attestation_binding;
             DROP TRIGGER provider_discovery_native_no_effect_schema37_companion_required;",
        )
        .expect("drop insert guards only for native attestation corruption fixture");
    database
        .execute(
            "INSERT OR REPLACE INTO provider_discovery_native_no_effect_attestations (
                 operation_id, session_id, commit_attempt_id, commit_plan_sha256,
                 connection_id, attestation_kind, evidence_sha256, recovery_owner,
                 schema_version, redaction_version, attested_at
             )
             SELECT operation_id, session_id, commit_attempt_id, commit_plan_sha256,
                    connection_id, attestation_kind, ?2, recovery_owner,
                    schema_version, redaction_version, attested_at
             FROM provider_discovery_native_no_effect_attestations
             WHERE operation_id = ?1",
            rusqlite::params![fixture.operation_id.as_str(), "f".repeat(64)],
        )
        .expect("inject replaced native attestation after dropping insert guards");
    drop(database);
    let error = storage
        .get_discovery_native_no_effect_attestation(&fixture.operation_id)
        .expect_err("replaced native attestation must fail runtime validation");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    assert!(
        error.message.contains("evidence hash")
            || error.message.contains("execution binding differs"),
        "unexpected replaced-attestation rejection: {error}"
    );
    assert_ne!(original.evidence_sha256, "f".repeat(64));
}

#[test]
fn native_no_effect_attestation_hash_tampering_fails_closed_after_restart() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "native-hash-tamper");
    let (write, attestation) = native_no_effect_completion(&storage, &fixture, &fixture.session);
    storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect("persist native attestation");
    let database_path = storage
        .connection()
        .expect("active database connection")
        .path()
        .expect("active database path")
        .to_owned();
    drop(storage);

    let connection = rusqlite::Connection::open(database_path).expect("open tamper connection");
    connection
        .execute_batch(
            "DROP TRIGGER provider_discovery_native_no_effect_attestation_no_update;
             UPDATE provider_discovery_native_no_effect_attestations
             SET evidence_sha256 = '7777777777777777777777777777777777777777777777777777777777777777';",
        )
        .expect("simulate direct database tampering");
    drop(connection);

    let Err(error) = Storage::open(root.path()) else {
        panic!("tampered native attestation unexpectedly reopened");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    assert!(
        error.message.contains("evidence hash")
            || error.message.contains("execution binding differs"),
        "unexpected tamper rejection: {error}"
    );
}

#[test]
fn prepared_credential_archive_blocks_new_discovery_session() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-prepared-archive-guard");
    let connection_id = draft.input.connection_id.clone();
    storage
        .save_provider_profile(&ProviderProfile {
            id: connection_id.as_str().to_owned(),
            display_name: "Prepared archive guard".to_owned(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("save provider");
    let archive = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare credential archive before discovery");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'f');
    let operation_id =
        DiscoveryOperationId::parse("operation-prepared-archive-guard").expect("operation id");

    let error = storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id), None))
        .expect_err("prepared credential archive must reserve the connection");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(
        error.message,
        "provider connection cannot begin discovery while its credential operation is unresolved"
    );
    assert_eq!(
        storage
            .get_discovery_session(&draft.id)
            .expect_err("rejected begin must not leave a discovery session")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        storage
            .list_unresolved_provider_credential_operations()
            .expect("list unresolved credential operations")
            .iter()
            .map(|operation| operation.plan.operation_id.as_str())
            .collect::<Vec<_>>(),
        vec![archive.plan.operation_id.as_str()]
    );
}

#[test]
fn prepared_credential_removal_blocks_new_discovery_session() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-prepared-removal-guard");
    let connection_id = draft.input.connection_id.clone();
    storage
        .save_provider_profile(&ProviderProfile {
            id: connection_id.as_str().to_owned(),
            display_name: "Prepared credential removal guard".to_owned(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("save provider");
    let removal = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare credential removal before discovery");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'd');
    let operation_id =
        DiscoveryOperationId::parse("operation-prepared-removal-guard").expect("operation id");

    let error = storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id), None))
        .expect_err("prepared credential removal must reserve the connection");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(
        storage
            .get_discovery_session(&draft.id)
            .expect_err("rejected begin must not leave a discovery session")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        storage
            .list_unresolved_provider_credential_operations()
            .expect("list unresolved credential operations")
            .iter()
            .map(|operation| operation.plan.operation_id.as_str())
            .collect::<Vec<_>>(),
        vec![removal.plan.operation_id.as_str()]
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn terminal_credential_removal_invalidates_cached_discovery_start_authority() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-stale-credential-authority");
    let connection_id = draft.input.connection_id.clone();
    storage
        .save_provider_profile(&ProviderProfile {
            id: connection_id.as_str().to_owned(),
            display_name: "Stale discovery credential authority".to_owned(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("save credential-bound provider");

    let install_authority = storage
        .propose_provider_credential_install_authority(&connection_id)
        .expect("propose credential install authority");
    let install = storage
        .prepare_provider_credential_operation_with_install_authority(
            &connection_id,
            ProviderCredentialOperationKind::Install,
            ProviderCredentialObservedStatus::Missing,
            Some(&install_authority),
        )
        .expect("prepare credential install");
    storage
        .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
        .expect("start credential install");
    storage
        .finish_provider_credential_operation(
            &install.plan.operation_id,
            &install.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("finish credential install");
    let cached_authority = storage
        .ensure_provider_credential_access_settled(&connection_id)
        .expect("read installed credential authority");

    let removal = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare ordinary credential removal");
    storage
        .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
        .expect("start ordinary credential removal");
    storage
        .finish_provider_credential_operation(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("terminalize ordinary credential removal");

    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'a');
    let operation_id =
        DiscoveryOperationId::parse("operation-stale-discovery-credential-authority")
            .expect("operation id");
    let error = storage
        .begin_discovery_session_with_credential_authority(
            &draft,
            &write(begin, Some(operation_id), None),
            Some(&cached_authority),
        )
        .expect_err("removed credential authority must not begin discovery");
    assert_eq!(error.code, CoreErrorCode::InvalidInput, "{error:?}");
    assert!(error.recoverable);
    assert_eq!(
        storage
            .get_discovery_session(&draft.id)
            .expect_err("rejected start must not persist a discovery session")
            .code,
        CoreErrorCode::NotFound
    );
    assert!(
        storage
            .poll_discovery_events(10, now())
            .expect("poll empty discovery outbox")
            .is_empty(),
        "rejected start must publish no discovery work"
    );
}

#[test]
fn begun_discovery_session_blocks_credential_archive_prepare() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-before-archive-guard");
    let connection_id = draft.input.connection_id.clone();
    storage
        .save_provider_profile(&ProviderProfile {
            id: connection_id.as_str().to_owned(),
            display_name: "Begun discovery guard".to_owned(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("save provider");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'e');
    let operation_id =
        DiscoveryOperationId::parse("operation-before-archive-guard").expect("operation id");
    storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id), None))
        .expect("begin discovery before credential archive");

    let error = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("begun discovery must block credential archive prepare");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(
        error.message,
        "provider connection cannot be archived while provider discovery is unfinished"
    );
    assert!(
        storage
            .list_unresolved_provider_credential_operations()
            .expect("list unresolved credential operations")
            .is_empty()
    );
}

#[test]
fn begun_discovery_session_blocks_credential_removal_prepare() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-before-removal-guard");
    let connection_id = draft.input.connection_id.clone();
    storage
        .save_provider_profile(&ProviderProfile {
            id: connection_id.as_str().to_owned(),
            display_name: "Begun discovery removal guard".to_owned(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("save provider");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'c');
    let operation_id =
        DiscoveryOperationId::parse("operation-before-removal-guard").expect("operation id");
    storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id), None))
        .expect("begin discovery before credential removal");

    let error = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("begun discovery must block credential removal prepare");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(
        storage
            .get_discovery_session(&draft.id)
            .expect("discovery remains durable after rejected removal")
            .session
            .state,
        DiscoveryState::ResolvingKnownProvider
    );
    assert!(
        storage
            .list_unresolved_provider_credential_operations()
            .expect("list unresolved credential operations")
            .is_empty()
    );
}

#[test]
fn unfinished_discovery_blocks_provider_archive_until_cancelled() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-archive-guard");
    let connection_id = draft.input.connection_id.clone();
    let profile = ProviderProfile {
        id: connection_id.as_str().to_owned(),
        display_name: "Discovery archive guard".to_owned(),
        base_url: "https://provider.example/v1".to_owned(),
        model: "synthetic".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&profile)
        .expect("save provider");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'f');
    let operation_id =
        DiscoveryOperationId::parse("operation-archive-guard").expect("operation id");
    storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id), None))
        .expect("persist begin");

    let error = storage
        .delete_provider_profile(&profile.id)
        .expect_err("unfinished discovery must block provider archive");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(
        error.message,
        "provider connection cannot be archived while provider discovery is unfinished"
    );
    assert_eq!(
        storage
            .get_provider_profile(&profile.id)
            .expect("provider remains active after rejected archive"),
        profile
    );

    let resolving = storage
        .get_discovery_session(&draft.id)
        .expect("load resolving session");
    let cancel = apply(&resolving.session, ProviderDiscoveryAction::Cancel, '0');
    storage
        .persist_discovery_transition(&write(cancel, None, None))
        .expect("persist cancellation request");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen storage");
    assert_eq!(
        reopened
            .get_discovery_session(&draft.id)
            .expect("load cancelled discovery")
            .session
            .state,
        DiscoveryState::Cancelled
    );
    reopened
        .delete_provider_profile(&profile.id)
        .expect("terminal discovery permits provider archive");
    assert_eq!(
        reopened
            .get_provider_connection(&connection_id)
            .expect_err("archived provider is hidden")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        reopened
            .get_discovery_session(&draft.id)
            .expect("terminal discovery history remains readable")
            .session
            .state,
        DiscoveryState::Cancelled
    );
}

#[test]
fn nonterminal_discovery_committed_reference_blocks_provider_archive() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let connection_id = ProviderConnectionId::from("committed-archive-guard");
    storage
        .save_provider_profile(&ProviderProfile {
            id: connection_id.as_str().to_owned(),
            display_name: "Committed discovery archive guard".to_owned(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("save provider");
    let unrelated = draft_session("session-committed-reference");
    let input_json =
        serde_json::to_string(&unrelated.input).expect("encode sanitized discovery input");
    let connection = storage.connection().expect("database connection");
    let guard = suspend_test_trigger(
        &connection,
        "provider_discovery_session_initial_state_guard",
    );
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                 id, state, sanitized_input_json, unknown_operation,
                 committed_connection_id, created_at, updated_at
             ) VALUES (
                 ?1, 'unknown_outcome', ?2, 'atomic_commit', ?3, ?4, ?4
             )",
            rusqlite::params![
                unrelated.id.as_str(),
                input_json,
                connection_id.as_str(),
                now().to_rfc3339(),
            ],
        )
        .expect("seed nonterminal committed discovery reference");
    restore_test_trigger(&connection, &guard);
    drop(connection);
    let snapshot = storage
        .get_discovery_session(&unrelated.id)
        .expect("hydrate nonterminal committed discovery");
    assert_eq!(snapshot.session.state, DiscoveryState::UnknownOutcome);
    assert_eq!(
        snapshot.session.committed_connection_id.as_ref(),
        Some(&connection_id)
    );
    assert_ne!(snapshot.session.input.connection_id, connection_id);

    let error = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("committed nonterminal discovery must block provider archive");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(
        error.message,
        "provider connection cannot be archived while provider discovery is unfinished"
    );
    assert!(
        storage.get_provider_connection(&connection_id).is_ok(),
        "provider remains active after rejected archive"
    );
}

fn seed_synthetic_discovery_session_state(
    storage: &Storage,
    draft: &ProviderDiscoverySession,
    state: DiscoveryState,
    state_label: &str,
) {
    let input_json = serde_json::to_string(&draft.input).expect("encode sanitized discovery input");
    let requires_commit_plan = matches!(
        state,
        DiscoveryState::Committing | DiscoveryState::Compensating
    );
    let recovery_json = (state == DiscoveryState::Interrupted).then_some("{}");
    let unknown_operation = (state == DiscoveryState::UnknownOutcome).then_some("atomic_commit");
    let commit_plan_sha256 = requires_commit_plan.then(|| "0".repeat(64));
    let commit_attempt_id = requires_commit_plan.then(|| format!("attempt-{state_label}"));
    let committed_connection_id =
        (state == DiscoveryState::Ready).then_some(draft.input.connection_id.as_str());
    let failure_json = (state == DiscoveryState::Failed).then(|| {
        serde_json::json!({
            "code": "synthetic_failure",
            "message_key": "discovery.failed",
            "recoverable": true,
        })
        .to_string()
    });
    let connection = storage.connection().expect("database connection");
    let guard = suspend_test_trigger(
        &connection,
        "provider_discovery_session_initial_state_guard",
    );
    connection
        .execute(
            "INSERT INTO provider_discovery_sessions (
                     id, state, sanitized_input_json, error_json, recovery_json,
                     unknown_operation, commit_plan_sha256, commit_attempt_id,
                     committed_connection_id, created_at, updated_at
                 ) VALUES (
                     ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10
                 )",
            rusqlite::params![
                draft.id.as_str(),
                state_label,
                input_json,
                failure_json,
                recovery_json,
                unknown_operation,
                commit_plan_sha256,
                commit_attempt_id,
                committed_connection_id,
                now().to_rfc3339(),
            ],
        )
        .expect("seed exact discovery state");
    restore_test_trigger(&connection, &guard);
    drop(connection);
}

#[test]
fn every_current_discovery_state_obeys_archive_terminal_boundary() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let mut nonterminal_count = 0;

    for state in DiscoveryState::ALL {
        let serialized_state = serde_json::to_value(state).expect("serialize discovery state");
        let state_label = serialized_state
            .as_str()
            .expect("discovery state serializes as text");
        let session_id = format!("archive-discovery-{state_label}");
        let draft = draft_session(&session_id);
        let connection_id = draft.input.connection_id.clone();
        storage
            .save_provider_profile(&ProviderProfile {
                id: connection_id.as_str().to_owned(),
                display_name: format!("Archive boundary {state_label}"),
                base_url: "https://provider.example/v1".to_owned(),
                model: "boundary-model".to_owned(),
                timeout_seconds: 30,
            })
            .expect("seed provider");
        seed_synthetic_discovery_session_state(&storage, &draft, state, state_label);

        if state.is_terminal() {
            archive_credential_bound_connection(&storage, &connection_id);
            assert_eq!(
                storage
                    .get_provider_connection(&connection_id)
                    .expect_err("terminal state permits hidden archive")
                    .code,
                CoreErrorCode::NotFound
            );
        } else {
            nonterminal_count += 1;
            let error = storage
                .prepare_provider_credential_operation(
                    &connection_id,
                    ProviderCredentialOperationKind::RemoveForArchive,
                    ProviderCredentialObservedStatus::Missing,
                )
                .expect_err("nonterminal discovery must block archive");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
            assert!(error.recoverable);
            assert_eq!(
                error.message,
                "provider connection cannot be archived while provider discovery is unfinished"
            );
            assert!(
                storage.get_provider_connection(&connection_id).is_ok(),
                "rejected archive keeps provider active for {state_label}"
            );
        }
    }

    assert_eq!(
        nonterminal_count, 19,
        "the test fixture must cover every current nonterminal discovery state"
    );
}
