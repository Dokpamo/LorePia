use super::support::*;
use super::*;

#[test]
fn prepared_interruption_restart_discovery_credential_authority_remains_valid() {
    let fixture = seed_completed_discovery_authority_with_mode(
        "discovery-authority-prepared-retry",
        CompletedDiscoveryAuthorityMode::PreparedInterruptedRetry,
    );
    validate_discovery_credential_ownership_authority(
        &fixture.storage.connection().expect("authority database"),
        &fixture.connection_id,
        &fixture.physical_authority_id,
        fixture.authority_operation_id.as_str(),
        &fixture.binding_sha256,
    )
    .expect("prepared interruption restart and final success remain valid authority");
}

#[test]
fn restarted_atomic_commit_accepts_exact_native_no_effect_attestation() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(&storage, "native-retry-attestation");
    let retry_operation_id =
        DiscoveryOperationId::parse("operation-native-retry-attestation-retry")
            .expect("retry operation id");
    let retrying = restart_started_native_credential_commit(&storage, &fixture, retry_operation_id);
    let retry_interrupted = apply(
        &retrying.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '7',
    );
    let mut retry_write = write(
        retry_interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: retrying.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    retry_write.occurred_at = now() + chrono::Duration::milliseconds(5);
    let retry_attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        retrying.operation_id.clone(),
        test_native_physical_authority_id(&storage, &retrying.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("retry native no-effect attestation");
    storage
        .persist_native_no_effect_discovery_transition(&retry_write, &retry_attestation)
        .expect("persist retry operation native no-effect attestation");

    let stored = storage
        .get_discovery_native_no_effect_attestation(&retrying.operation_id)
        .expect("load retry native attestation")
        .expect("durable retry native attestation");
    assert_eq!(stored.evidence_sha256, retry_attestation.evidence_sha256);
    assert_eq!(
        operation_status(&storage, &retrying.operation_id),
        "interrupted"
    );
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen retry attestation storage");
    assert_eq!(
        reopened
            .get_discovery_native_no_effect_attestation(&retrying.operation_id)
            .expect("validate retry attestation after reopen")
            .expect("reopened retry attestation"),
        stored
    );
    assert_eq!(
        reopened
            .get_discovery_session(&fixture.session.id)
            .expect("load reopened interrupted retry")
            .session
            .state,
        DiscoveryState::Interrupted
    );
}

#[test]
fn reconciled_unknown_commit_accepts_exact_retry_native_no_effect_attestation() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture =
        seed_started_native_credential_commit(&storage, "native-reconciled-retry-attestation");
    let retry = restart_unknown_native_credential_commit(
        &storage,
        &fixture,
        DiscoveryOperationId::parse("operation-native-reconciled-retry-attestation-retry")
            .expect("retry operation id"),
    );
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect("approved no-effect reconciliation grants retry install authority");

    let retry_interrupted = apply(
        &retry.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '8',
    );
    let mut retry_write = write(
        retry_interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: retry.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    retry_write.occurred_at = now() + chrono::Duration::milliseconds(6);
    let retry_attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        retry.operation_id.clone(),
        test_native_physical_authority_id(&storage, &retry.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("reconciled retry native no-effect attestation");
    storage
        .persist_native_no_effect_discovery_transition(&retry_write, &retry_attestation)
        .expect("persist reconciled retry native no-effect attestation");
    let stored = storage
        .get_discovery_native_no_effect_attestation(&retry.operation_id)
        .expect("load reconciled retry native attestation")
        .expect("durable reconciled retry native attestation");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen reconciled retry storage");
    assert_eq!(
        reopened
            .get_discovery_native_no_effect_attestation(&retry.operation_id)
            .expect("validate reconciled retry attestation after reopen")
            .expect("reopened reconciled retry attestation"),
        stored
    );
}

#[test]
fn twice_restarted_commit_accepts_recursive_native_authority() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture =
        seed_started_native_credential_commit(&storage, "native-recursive-retry-authority");
    let first_retry = restart_started_native_credential_commit(
        &storage,
        &fixture,
        DiscoveryOperationId::parse("operation-native-recursive-retry-authority-first")
            .expect("first retry operation id"),
    );
    let second_retry = restart_attested_native_retry(
        &storage,
        &fixture,
        &first_retry,
        DiscoveryOperationId::parse("operation-native-recursive-retry-authority-second")
            .expect("second retry operation id"),
    );
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &second_retry.operation_id,
        )
        .expect("recursive retry chain grants exact install authority");

    let interrupted = apply(
        &second_retry.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        'a',
    );
    let mut write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: second_retry.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    write.occurred_at = now() + chrono::Duration::milliseconds(8);
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        second_retry.operation_id.clone(),
        test_native_physical_authority_id(&storage, &second_retry.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("second retry native attestation");
    storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect("persist recursively authorized native attestation");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen recursive retry storage");
    assert!(
        reopened
            .get_discovery_native_no_effect_attestation(&second_retry.operation_id)
            .expect("validate recursive attestation after reopen")
            .is_some()
    );
}

fn persist_recursive_prepared_retry_attestation(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    retry: &RestartedNativeCommitFixture,
) -> super::super::DiscoveryNativeNoEffectAttestationRecord {
    let interrupted = apply(
        &retry.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '9',
    );
    let mut write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: retry.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    write.occurred_at = now() + chrono::Duration::milliseconds(7);
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        retry.operation_id.clone(),
        test_native_physical_authority_id(storage, &retry.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("final prepared retry native attestation");
    storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect("persist final attestation across two prepared retry edges");
    storage
        .get_discovery_native_no_effect_attestation(&retry.operation_id)
        .expect("load final prepared retry attestation")
        .expect("durable final prepared retry attestation")
}

#[test]
fn twice_restarted_unstarted_prepared_commit_accepts_recursive_native_authority() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_prepared_native_credential_commit(
        &storage,
        "native-recursive-prepared-retry-authority",
    );
    let first_retry = restart_unstarted_prepared_native_commit(
        &storage,
        &fixture,
        &fixture.session,
        &fixture.operation_id,
        UnstartedPreparedNativeRetryStep {
            next_operation_id: DiscoveryOperationId::parse(
                "operation-native-recursive-prepared-retry-authority-first",
            )
            .expect("first prepared retry operation id"),
            interrupt_hash_byte: '5',
            restart_hash_byte: '6',
            interrupted_at_millis: 2,
            restarted_at_millis: 3,
        },
    );
    let second_retry = restart_unstarted_prepared_native_commit(
        &storage,
        &fixture,
        &first_retry.session,
        &first_retry.operation_id,
        UnstartedPreparedNativeRetryStep {
            next_operation_id: DiscoveryOperationId::parse(
                "operation-native-recursive-prepared-retry-authority-second",
            )
            .expect("second prepared retry operation id"),
            interrupt_hash_byte: '7',
            restart_hash_byte: '8',
            interrupted_at_millis: 4,
            restarted_at_millis: 5,
        },
    );

    assert_unstarted_prepared_retry_predecessors(&storage, &fixture, &first_retry);

    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &second_retry.operation_id,
        )
        .expect("two prepared retry edges grant exact install authority");
    assert_eq!(
        operation_status(&storage, &second_retry.operation_id),
        "prepared"
    );
    reserve_and_start_test_native_execution(
        &storage,
        &second_retry.session,
        &fixture.attempt_id,
        &fixture.plan_sha256,
        &second_retry.operation_id,
        now() + chrono::Duration::milliseconds(6),
    );

    let stored = persist_recursive_prepared_retry_attestation(&storage, &fixture, &second_retry);
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen recursive prepared retry storage");
    assert_eq!(
        reopened
            .get_discovery_native_no_effect_attestation(&second_retry.operation_id)
            .expect("validate recursive prepared retry attestation after reopen")
            .expect("reopened recursive prepared retry attestation"),
        stored
    );
}

#[test]
fn second_retry_rejects_detached_first_restart_receipt() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture =
        seed_started_native_credential_commit(&storage, "native-recursive-retry-detached-start");
    let first_retry = restart_started_native_credential_commit(
        &storage,
        &fixture,
        DiscoveryOperationId::parse("operation-native-recursive-detached-first")
            .expect("first retry operation id"),
    );
    let first_restart_action_id = storage
        .get_current_discovery_operation(&fixture.session.id)
        .expect("load first retry operation")
        .expect("first retry operation")
        .action_id;
    let second_retry = restart_attested_native_retry(
        &storage,
        &fixture,
        &first_retry,
        DiscoveryOperationId::parse("operation-native-recursive-detached-second")
            .expect("second retry operation id"),
    );
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &second_retry.operation_id,
        )
        .expect("intact recursive retry chain is authorized");

    let (write, attestation) = remove_retry_history_and_assert_schema_rejection(
        &storage,
        &fixture,
        &second_retry,
        &first_restart_action_id,
    );
    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect_err("public writer must reject detached recursive retry root");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);
    let error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &second_retry.operation_id,
        )
        .expect_err("detached recursive retry root must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn retry_rejects_broken_start_terminal_event_sequence_adjacency() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(
        &storage,
        "native-retry-detached-start-terminal-sequence",
    );
    let retry = restart_started_native_credential_commit(
        &storage,
        &fixture,
        DiscoveryOperationId::parse(
            "operation-native-retry-detached-start-terminal-sequence-retry",
        )
        .expect("detached sequence retry operation id"),
    );
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect("intact start-to-terminal event adjacency grants retry authority");

    let active = corrupt_retry_start_terminal_event_sequence(&storage, &fixture, &retry);
    let interrupted = apply(
        &active,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '7',
    );
    let attested_at = now() + chrono::Duration::milliseconds(5);
    let mut write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: retry.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    write.occurred_at = attested_at;
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        retry.operation_id.clone(),
        raw_test_native_physical_authority_id(&storage, &retry.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("detached sequence retry attestation");

    let database = storage.connection().expect("schema rejection database");
    assert_native_attestation_and_terminal_schema_rejected(&database, &attestation, attested_at);
    drop(database);

    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect_err("public writer must reject a detached start-to-terminal sequence");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);
    assert!(
        persist_error.message.contains("detached event sequence")
            || persist_error
                .message
                .contains("execution is detached from its immutable discovery commit"),
        "unexpected detached-sequence rejection: {persist_error}"
    );
    let authority_error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect_err("Rust authority must reject a detached start-to-terminal sequence");
    assert_eq!(authority_error.code, CoreErrorCode::StorageCorrupted);
    assert!(authority_error.message.contains("detached event sequence"));
}

fn remove_retry_history_and_assert_schema_rejection(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    retry: &RestartedNativeCommitFixture,
    deleted_action_id: &DiscoveryActionId,
) -> (
    DiscoveryTransitionWrite,
    DiscoveryNativeNoEffectAttestationWrite,
) {
    let attested_at = storage
        .get_current_discovery_operation(&fixture.session.id)
        .expect("load active retry operation")
        .expect("active retry operation")
        .started_at
        .expect("started retry operation")
        + chrono::Duration::milliseconds(1);
    let physical_authority_id = test_native_physical_authority_id(storage, &retry.operation_id);
    let database = storage.connection().expect("database connection");
    let delete_predecessor = || {
        database.execute(
            "DELETE FROM provider_discovery_action_receipts
             WHERE action_id = ?1",
            [deleted_action_id.as_str()],
        )
    };
    let trigger_error = delete_predecessor()
        .expect_err("immutable receipt trigger must preserve the retry predecessor");
    assert!(
        trigger_error.to_string().contains("immutable"),
        "unexpected predecessor deletion rejection: {trigger_error}"
    );
    database
        .execute_batch("DROP TRIGGER provider_discovery_receipt_no_delete;")
        .expect("drop receipt deletion guard only for corruption fixture");
    delete_predecessor().expect("remove retry predecessor after bypassing its history guard");
    let retry_interrupted = apply(
        &retry.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '7',
    );
    let mut retry_write = write(
        retry_interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: retry.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    retry_write.occurred_at = attested_at;
    let retry_attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        retry.operation_id.clone(),
        physical_authority_id,
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("retry native no-effect attestation");
    assert_native_attestation_and_terminal_schema_rejected(
        &database,
        &retry_attestation,
        attested_at,
    );
    drop(database);
    (retry_write, retry_attestation)
}

#[test]
fn restarted_native_commit_requires_immutable_interrupted_predecessor() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture =
        seed_started_native_credential_commit(&storage, "native-retry-predecessor-authority");
    let retry = restart_started_native_credential_commit(
        &storage,
        &fixture,
        DiscoveryOperationId::parse("operation-native-retry-predecessor-authority-retry")
            .expect("retry operation id"),
    );
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect("exact retry predecessor grants install authority");

    let (retry_write, retry_attestation) = remove_retry_history_and_assert_schema_rejection(
        &storage,
        &fixture,
        &retry,
        &retry.predecessor_action_id,
    );

    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&retry_write, &retry_attestation)
        .expect_err("public writer must reject a retry without its predecessor");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);

    let error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect_err("retry without its interrupted predecessor must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn retry_rejects_tampered_native_no_effect_predecessor_hash() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture = seed_started_native_credential_commit(
        &storage,
        "native-retry-predecessor-attestation-digest",
    );
    let retry = restart_started_native_credential_commit(
        &storage,
        &fixture,
        DiscoveryOperationId::parse("operation-native-retry-predecessor-attestation-digest-retry")
            .expect("retry operation id"),
    );
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect("exact predecessor attestation grants retry authority");

    let interrupted = apply(
        &retry.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '7',
    );
    let attested_at = now() + chrono::Duration::milliseconds(5);
    let mut write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: retry.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    write.occurred_at = attested_at;
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        retry.operation_id.clone(),
        test_native_physical_authority_id(&storage, &retry.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("retry native no-effect attestation");

    let database = storage.connection().expect("database connection");
    let mutate_hash = || {
        database.execute(
            "UPDATE provider_discovery_native_no_effect_attestations
             SET evidence_sha256 = ?2 WHERE operation_id = ?1",
            rusqlite::params![fixture.operation_id.as_str(), "7".repeat(64)],
        )
    };
    let immutable_error = mutate_hash()
        .expect_err("immutable attestation trigger must preserve predecessor evidence");
    assert!(immutable_error.to_string().contains("immutable"));
    database
        .execute_batch("DROP TRIGGER provider_discovery_native_no_effect_attestation_no_update;")
        .expect("drop attestation update guard only for corruption fixture");
    mutate_hash().expect("inject mismatched predecessor attestation digest");
    assert_native_attestation_and_terminal_schema_rejected(&database, &attestation, attested_at);
    drop(database);

    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&write, &attestation)
        .expect_err("public writer must reject a corrupted predecessor attestation");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);
    let authority_error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect_err("runtime authority must reject a corrupted predecessor attestation");
    assert_eq!(authority_error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn prepared_retry_requires_its_canonical_commit_start_receipt() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let fixture =
        seed_prepared_native_credential_commit(&storage, "native-prepared-retry-start-authority");
    let start_action_id = storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load initial prepared attempt")
        .action_id;
    let retry = restart_prepared_native_credential_commit(
        &storage,
        &fixture,
        DiscoveryOperationId::parse("operation-native-prepared-retry-start-authority-retry")
            .expect("retry operation id"),
    );
    storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect("exact prepared-interrupt history grants retry authority");

    let (retry_write, retry_attestation) = remove_retry_history_and_assert_schema_rejection(
        &storage,
        &fixture,
        &retry,
        &start_action_id,
    );
    let persist_error = storage
        .persist_native_no_effect_discovery_transition(&retry_write, &retry_attestation)
        .expect_err("public writer must reject retry with detached commit start");
    assert_eq!(persist_error.code, CoreErrorCode::StorageCorrupted);
    let error = storage
        .validate_discovery_credential_install_operation_authority(
            &fixture.session.id,
            &fixture.attempt_id,
            &fixture.plan_sha256,
            &retry.operation_id,
        )
        .expect_err("retry with detached commit start must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn cancel_reopen_automatically_interrupts_and_finishes_cancellation() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let draft = draft_session("session-cancel-reopen");
    let begin = apply(&draft, ProviderDiscoveryAction::Begin, 'a');
    let operation_id = DiscoveryOperationId::parse("operation-resolve").expect("operation id");
    storage
        .begin_discovery_session(&draft, &write(begin, Some(operation_id.clone()), None))
        .expect("persist begin");
    assert!(
        storage
            .mark_discovery_operation_started(&operation_id, now())
            .expect("mark operation started")
    );

    let resolving = storage
        .get_discovery_session(&draft.id)
        .expect("load resolving session");
    let cancel = apply(&resolving.session, ProviderDiscoveryAction::Cancel, 'b');
    storage
        .persist_discovery_transition(&write(cancel, None, None))
        .expect("persist cancellation request");
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen storage");
    let terminal = reopened
        .get_discovery_session(&draft.id)
        .expect("hydrate automatically recovered session");
    assert_eq!(
        terminal.session.state,
        lorepia_domain::discovery::DiscoveryState::Cancelled
    );
    assert!(!terminal.session.cancellation_pending);
    assert!(terminal.active_operation_id.is_none());
    assert!(
        reopened
            .get_current_discovery_operation(&draft.id)
            .expect("query current operation")
            .is_none()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn prepared_keyless_commit_cancellation_enters_durable_compensation() {
    let root = tempdir().expect("temp directory");
    let storage = Storage::open(root.path()).expect("open storage");
    let mut committing = draft_session("session-keyless-commit-cancel");
    storage
        .create_discovery_session(&committing, now())
        .expect("create discovery session");

    let attempt_id =
        DiscoveryCommitAttemptId::parse("attempt-keyless-commit-cancel").expect("attempt id");
    let plan = DiscoveryCommitPlan {
        attempt_id: attempt_id.clone(),
        session_id: committing.id.clone(),
        expected_revision: 0,
        manifest_sha256: "1".repeat(64),
        graph_sha256: "2".repeat(64),
        template_id: ProviderTemplateId::from("template-keyless-commit-cancel"),
        template_version: 1,
        connection_id: committing.input.connection_id.clone(),
        model_route_ids: vec![ModelRouteId::from("route-keyless-commit-cancel")],
        credential_ref: None,
        credential_approval_id: None,
        review_sha256: "3".repeat(64),
        catalog_authority: None,
        previous_selection: DiscoveryPreviousSelection::None,
    };
    plan.validate().expect("valid keyless commit plan");
    let plan_json = serde_json::to_string(&plan).expect("commit plan JSON");
    let plan_sha256 = sha256_hex(plan_json.as_bytes());
    let commit_operation_id =
        DiscoveryOperationId::parse("operation-keyless-atomic-commit").expect("operation id");
    let compensation_operation_id =
        DiscoveryOperationId::parse("operation-keyless-compensation").expect("operation id");
    let selection_step = DiscoveryCompensationStep {
        action_id: DiscoveryActionId::parse("action-keyless-restore-selection")
            .expect("step action id"),
        ordinal: 0,
        kind: DiscoveryCompensationKind::RestorePreviousSelection,
        target: DiscoveryCompensationTarget::RestorePreviousSelection {
            previous_selection: DiscoveryPreviousSelection::None,
        },
        status: DiscoveryCompensationStatus::Pending,
    };
    let graph_step = DiscoveryCompensationStep {
        action_id: DiscoveryActionId::parse("action-keyless-remove-graph").expect("step action id"),
        ordinal: 1,
        kind: DiscoveryCompensationKind::RemoveConnectionGraph,
        target: DiscoveryCompensationTarget::RemoveConnectionGraph {
            connection_id: committing.input.connection_id.clone(),
        },
        status: DiscoveryCompensationStatus::Pending,
    };
    for step in [&selection_step, &graph_step] {
        step.validate_against(&plan)
            .expect("valid compensation step");
    }

    {
        let mut connection = storage.connection().expect("database connection");
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
                     ?1, ?2, 1, 'action-prepare-keyless-commit', 0,
                     ?3, ?4, 'prepared', 1, ?5, ?5, NULL
                 )",
                rusqlite::params![
                    attempt_id.as_str(),
                    committing.id.as_str(),
                    plan_sha256,
                    plan_json,
                    now().to_rfc3339(),
                ],
            )
            .expect("insert prepared commit attempt");
        for (id, step) in [
            ("step-keyless-restore-selection", &selection_step),
            ("step-keyless-remove-graph", &graph_step),
        ] {
            let kind = serde_json::to_value(step.kind).expect("compensation kind serialization");
            transaction
                .execute(
                    "INSERT INTO provider_discovery_compensation_steps (
                         id, commit_attempt_id, ordinal, action_id, step_kind,
                         step_json, status, attempt_count, last_failure_json,
                         redaction_version, created_at, updated_at, completed_at
                     ) VALUES (
                         ?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0, NULL, 1, ?7, ?7, NULL
                     )",
                    rusqlite::params![
                        id,
                        attempt_id.as_str(),
                        step.ordinal,
                        step.action_id.as_str(),
                        kind.as_str().expect("wire compensation kind"),
                        serde_json::to_string(step).expect("compensation step JSON"),
                        now().to_rfc3339(),
                    ],
                )
                .expect("insert compensation step");
        }
        transaction
            .execute(
                "INSERT INTO provider_discovery_operations (
                     id, session_id, operation_kind, side_effect_class, status,
                     action_id, expected_revision, request_sha256, approval_id,
                     approval_grant_sha256, started_at, finished_at, created_at, updated_at
                 ) VALUES (
                     ?1, ?2, 'atomic_commit', 'persistent', 'started',
                     'action-run-keyless-commit', 1, ?3, NULL, NULL,
                     ?4, NULL, ?4, ?4
                 )",
                rusqlite::params![
                    commit_operation_id.as_str(),
                    committing.id.as_str(),
                    "4".repeat(64),
                    now().to_rfc3339(),
                ],
            )
            .expect("insert started atomic commit operation");
        restore_test_trigger(&transaction, &operation_guard);
        transaction
            .execute(
                "UPDATE provider_discovery_sessions
                 SET state = 'committing',
                     revision = 1,
                     next_event_sequence = 2,
                     commit_plan_sha256 = ?2,
                     commit_attempt_id = ?3,
                     cancellation_pending = 1,
                     active_operation_id = ?4,
                     updated_at = ?5
                 WHERE id = ?1",
                rusqlite::params![
                    committing.id.as_str(),
                    plan_sha256,
                    attempt_id.as_str(),
                    commit_operation_id.as_str(),
                    now().to_rfc3339(),
                ],
            )
            .expect("activate keyless committing session");
        transaction.commit().expect("commit fixture");
    }

    committing.state = DiscoveryState::Committing;
    committing.revision = 1;
    committing.next_event_sequence = 2;
    committing.commit_plan_sha256 = Some(plan_sha256);
    committing.commit_attempt_id = Some(attempt_id.clone());
    committing.cancellation_pending = true;
    committing.validate().expect("valid committing session");
    let transition = apply(
        &committing,
        ProviderDiscoveryAction::CompensationRequired,
        '5',
    );
    let cancellation = write(
        transition,
        Some(compensation_operation_id.clone()),
        Some(DiscoveryCompletedOperationWrite {
            id: commit_operation_id,
            outcome: super::super::DurableOperationOutcome::Failed,
        }),
    );
    storage
        .persist_discovery_transition(&cancellation)
        .expect("persist explicit keyless compensation transition");

    let compensating = storage
        .get_discovery_session(&committing.id)
        .expect("load compensating session");
    assert_eq!(compensating.session.state, DiscoveryState::Compensating);
    assert_eq!(
        storage
            .get_discovery_commit_attempt(&attempt_id)
            .expect("load compensated attempt")
            .phase,
        super::super::DiscoveryCommitPhase::CompensationRequired
    );
    assert!(
        storage
            .mark_discovery_operation_started(&compensation_operation_id, now())
            .expect("start compensation operation")
    );
    assert_eq!(
        storage
            .get_discovery_commit_attempt(&attempt_id)
            .expect("load started compensation attempt")
            .phase,
        super::super::DiscoveryCommitPhase::Compensating
    );
    storage
        .update_discovery_compensation_status(
            "step-keyless-remove-graph",
            super::super::DiscoveryCompensationStatus::Pending,
            super::super::DiscoveryCompensationStatus::InProgress,
            None,
            now(),
        )
        .expect("start graph compensation");
    storage
        .compensate_discovered_provider_graph(&attempt_id, now())
        .expect("complete absent graph compensation");
    storage
        .update_discovery_compensation_status(
            "step-keyless-restore-selection",
            super::super::DiscoveryCompensationStatus::Pending,
            super::super::DiscoveryCompensationStatus::InProgress,
            None,
            now(),
        )
        .expect("start selection compensation");
    storage
        .restore_discovery_previous_selection(&attempt_id, now())
        .expect("complete selection compensation");
    assert!(
        storage
            .list_discovery_compensation_steps(&attempt_id)
            .expect("load durable completed recipe")
            .iter()
            .all(|step| step.status == super::super::DiscoveryCompensationStatus::Completed)
    );

    // Simulate a crash after the last effect was durably confirmed but
    // before the aggregate CompensationSucceeded action was recorded.
    drop(storage);
    let reopened = Storage::open(root.path()).expect("recover completed compensation");
    let recovered = reopened
        .get_discovery_session(&committing.id)
        .expect("load recovered cancellation");
    assert_eq!(recovered.session.state, DiscoveryState::Cancelled);
    assert!(recovered.active_operation_id.is_none());
    assert_eq!(
        reopened
            .get_discovery_commit_attempt(&attempt_id)
            .expect("load recovered compensation attempt")
            .phase,
        super::super::DiscoveryCommitPhase::Compensated
    );
}
