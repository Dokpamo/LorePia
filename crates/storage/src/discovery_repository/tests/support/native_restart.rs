use super::*;

pub(in crate::discovery_repository::tests) fn restart_started_native_credential_commit(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    retry_operation_id: DiscoveryOperationId,
) -> RestartedNativeCommitFixture {
    let (first_write, first_attestation) =
        native_no_effect_completion(storage, fixture, &fixture.session);
    let predecessor_action_id = first_write.transition.receipt.action_id.clone();
    storage
        .persist_native_no_effect_discovery_transition(&first_write, &first_attestation)
        .expect("interrupt initial native credential operation");

    let interrupted = storage
        .get_discovery_session(&fixture.session.id)
        .expect("load interrupted credential commit");
    let restart = apply(
        &interrupted.session,
        ProviderDiscoveryAction::RestartInterrupted,
        '6',
    );
    let attempt = storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load reusable credential commit attempt");
    let mut restart_write = write(restart, Some(retry_operation_id.clone()), None);
    restart_write.prepared_commit = Some(PreparedDiscoveryCommit {
        plan: attempt.plan,
        plan_sha256: attempt.plan_sha256,
        attempt_number: attempt.attempt_number,
        reuse_existing: true,
        compensation_steps: Vec::new(),
    });
    restart_write.occurred_at = now() + chrono::Duration::milliseconds(3);
    storage
        .persist_discovery_transition(&restart_write)
        .expect("persist exact interrupted credential retry");
    reserve_and_start_test_native_execution(
        storage,
        &restart_write.transition.session,
        &fixture.attempt_id,
        &fixture.plan_sha256,
        &retry_operation_id,
        now() + chrono::Duration::milliseconds(4),
    );

    RestartedNativeCommitFixture {
        session: storage
            .get_discovery_session(&fixture.session.id)
            .expect("load retrying credential commit")
            .session,
        operation_id: retry_operation_id,
        predecessor_action_id,
    }
}

pub(in crate::discovery_repository::tests) fn restart_prepared_native_credential_commit(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    retry_operation_id: DiscoveryOperationId,
) -> RestartedNativeCommitFixture {
    let interrupted = apply(
        &fixture.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '5',
    );
    let predecessor_action_id = interrupted.receipt.action_id.clone();
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
        .expect("interrupt prepared native credential operation");

    let interrupted = storage
        .get_discovery_session(&fixture.session.id)
        .expect("load prepared-interrupted credential commit");
    let restart = apply(
        &interrupted.session,
        ProviderDiscoveryAction::RestartInterrupted,
        '6',
    );
    let attempt = storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load reusable prepared credential commit attempt");
    let mut restart_write = write(restart, Some(retry_operation_id.clone()), None);
    restart_write.prepared_commit = Some(PreparedDiscoveryCommit {
        plan: attempt.plan,
        plan_sha256: attempt.plan_sha256,
        attempt_number: attempt.attempt_number,
        reuse_existing: true,
        compensation_steps: Vec::new(),
    });
    restart_write.occurred_at = now() + chrono::Duration::milliseconds(3);
    storage
        .persist_discovery_transition(&restart_write)
        .expect("persist prepared-interrupted credential retry");
    reserve_and_start_test_native_execution(
        storage,
        &restart_write.transition.session,
        &fixture.attempt_id,
        &fixture.plan_sha256,
        &retry_operation_id,
        now() + chrono::Duration::milliseconds(4),
    );

    RestartedNativeCommitFixture {
        session: storage
            .get_discovery_session(&fixture.session.id)
            .expect("load prepared-interrupted retrying credential commit")
            .session,
        operation_id: retry_operation_id,
        predecessor_action_id,
    }
}

pub(in crate::discovery_repository::tests) fn restart_unstarted_prepared_native_commit(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    current_session: &ProviderDiscoverySession,
    current_operation_id: &DiscoveryOperationId,
    step: UnstartedPreparedNativeRetryStep,
) -> RestartedNativeCommitFixture {
    let interrupted = apply(
        current_session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        step.interrupt_hash_byte,
    );
    let predecessor_action_id = interrupted.receipt.action_id.clone();
    let mut interrupted_write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: current_operation_id.clone(),
            outcome: DurableOperationOutcome::Interrupted,
        }),
    );
    interrupted_write.occurred_at =
        now() + chrono::Duration::milliseconds(step.interrupted_at_millis);
    storage
        .persist_discovery_transition(&interrupted_write)
        .expect("interrupt unstarted prepared native credential operation");

    let interrupted = storage
        .get_discovery_session(&fixture.session.id)
        .expect("load unstarted prepared-interrupted credential commit");
    let restart = apply(
        &interrupted.session,
        ProviderDiscoveryAction::RestartInterrupted,
        step.restart_hash_byte,
    );
    let attempt = storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load reusable unstarted prepared credential commit attempt");
    let mut restart_write = write(restart, Some(step.next_operation_id.clone()), None);
    restart_write.prepared_commit = Some(PreparedDiscoveryCommit {
        plan: attempt.plan,
        plan_sha256: attempt.plan_sha256,
        attempt_number: attempt.attempt_number,
        reuse_existing: true,
        compensation_steps: Vec::new(),
    });
    restart_write.occurred_at = now() + chrono::Duration::milliseconds(step.restarted_at_millis);
    storage
        .persist_discovery_transition(&restart_write)
        .expect("persist unstarted prepared-interrupted credential retry");

    RestartedNativeCommitFixture {
        session: storage
            .get_discovery_session(&fixture.session.id)
            .expect("load unstarted prepared retrying credential commit")
            .session,
        operation_id: step.next_operation_id,
        predecessor_action_id,
    }
}

pub(in crate::discovery_repository::tests) fn restart_attested_native_retry(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    current_retry: &RestartedNativeCommitFixture,
    next_operation_id: DiscoveryOperationId,
) -> RestartedNativeCommitFixture {
    let interrupted = apply(
        &current_retry.session,
        ProviderDiscoveryAction::Interrupt {
            operation: DiscoveryOperationKind::AtomicCommit,
            outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
        },
        '8',
    );
    let predecessor_action_id = interrupted.receipt.action_id.clone();
    let mut interrupted_write = write(
        interrupted,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: current_retry.operation_id.clone(),
            outcome: DurableOperationOutcome::AttestedNoExternalEffect,
        }),
    );
    interrupted_write.occurred_at = now() + chrono::Duration::milliseconds(5);
    let attestation = DiscoveryNativeNoEffectAttestationWrite::credential_slot_missing(
        current_retry.operation_id.clone(),
        test_native_physical_authority_id(storage, &current_retry.operation_id),
        fixture.session.id.clone(),
        fixture.attempt_id.clone(),
        fixture.plan_sha256.clone(),
        fixture.session.input.connection_id.clone(),
    )
    .expect("first retry native no-effect attestation");
    storage
        .persist_native_no_effect_discovery_transition(&interrupted_write, &attestation)
        .expect("interrupt first retry with exact native attestation");

    let interrupted = storage
        .get_discovery_session(&fixture.session.id)
        .expect("load twice-interrupted credential commit");
    let restart = apply(
        &interrupted.session,
        ProviderDiscoveryAction::RestartInterrupted,
        '9',
    );
    let attempt = storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load reusable twice-interrupted commit attempt");
    let mut restart_write = write(restart, Some(next_operation_id.clone()), None);
    restart_write.prepared_commit = Some(PreparedDiscoveryCommit {
        plan: attempt.plan,
        plan_sha256: attempt.plan_sha256,
        attempt_number: attempt.attempt_number,
        reuse_existing: true,
        compensation_steps: Vec::new(),
    });
    restart_write.occurred_at = now() + chrono::Duration::milliseconds(6);
    storage
        .persist_discovery_transition(&restart_write)
        .expect("persist second credential retry");
    reserve_and_start_test_native_execution(
        storage,
        &restart_write.transition.session,
        &fixture.attempt_id,
        &fixture.plan_sha256,
        &next_operation_id,
        now() + chrono::Duration::milliseconds(7),
    );

    RestartedNativeCommitFixture {
        session: storage
            .get_discovery_session(&fixture.session.id)
            .expect("load second retrying credential commit")
            .session,
        operation_id: next_operation_id,
        predecessor_action_id,
    }
}

pub(in crate::discovery_repository::tests) fn restart_unknown_native_credential_commit(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    retry_operation_id: DiscoveryOperationId,
) -> RestartedNativeCommitFixture {
    let unknown = apply(
        &fixture.session,
        ProviderDiscoveryAction::ExternalOutcomeBecameUnknown,
        '5',
    );
    let mut unknown_write = write(
        unknown,
        None,
        Some(DiscoveryCompletedOperationWrite {
            id: fixture.operation_id.clone(),
            outcome: DurableOperationOutcome::OutcomeUnknown,
        }),
    );
    unknown_write.occurred_at = now() + chrono::Duration::milliseconds(2);
    storage
        .persist_discovery_transition(&unknown_write)
        .expect("persist unknown native credential outcome");

    let unknown = storage
        .get_discovery_session(&fixture.session.id)
        .expect("load unknown credential commit");
    let approval_id = DiscoveryApprovalId::parse(format!(
        "approval-native-retry-resolution-{}",
        fixture.session.id.as_str()
    ))
    .expect("resolution approval id");
    let resolution_at = now() + chrono::Duration::milliseconds(3);
    let resolution = apply(
        &unknown.session,
        ProviderDiscoveryAction::ResolveUnknownOutcome {
            approval_id: approval_id.clone(),
            resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
        },
        '6',
    );
    let predecessor_action_id = resolution.receipt.action_id.clone();
    let mut resolution_write = write(resolution, None, None);
    resolution_write.approval = Some(DiscoveryApprovalRecord {
        id: approval_id,
        session_id: fixture.session.id.clone(),
        session_revision: unknown.session.revision,
        decision: DiscoveryApprovalDecision::Approved,
        grant: DiscoveryApprovalGrant::UnknownOutcomeResolution {
            operation: DiscoveryOperationKind::AtomicCommit,
            resolution: DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect,
        },
        created_at: resolution_at,
    });
    resolution_write.occurred_at = resolution_at;
    storage
        .persist_discovery_transition(&resolution_write)
        .expect("persist approved no-effect resolution");

    let interrupted = storage
        .get_discovery_session(&fixture.session.id)
        .expect("load reconciled interrupted credential commit");
    let restart = apply(
        &interrupted.session,
        ProviderDiscoveryAction::RestartInterrupted,
        '7',
    );
    let attempt = storage
        .get_discovery_commit_attempt(&fixture.attempt_id)
        .expect("load reusable reconciled commit attempt");
    let mut restart_write = write(restart, Some(retry_operation_id.clone()), None);
    restart_write.prepared_commit = Some(PreparedDiscoveryCommit {
        plan: attempt.plan,
        plan_sha256: attempt.plan_sha256,
        attempt_number: attempt.attempt_number,
        reuse_existing: true,
        compensation_steps: Vec::new(),
    });
    restart_write.occurred_at = now() + chrono::Duration::milliseconds(4);
    storage
        .persist_discovery_transition(&restart_write)
        .expect("persist reconciled credential retry");
    reserve_and_start_test_native_execution(
        storage,
        &restart_write.transition.session,
        &fixture.attempt_id,
        &fixture.plan_sha256,
        &retry_operation_id,
        now() + chrono::Duration::milliseconds(5),
    );

    RestartedNativeCommitFixture {
        session: storage
            .get_discovery_session(&fixture.session.id)
            .expect("load reconciled retrying credential commit")
            .session,
        operation_id: retry_operation_id,
        predecessor_action_id,
    }
}

pub(in crate::discovery_repository::tests) fn operation_status(
    storage: &Storage,
    operation_id: &DiscoveryOperationId,
) -> String {
    storage
        .connection()
        .expect("database connection")
        .query_row(
            "SELECT status FROM provider_discovery_operations WHERE id = ?1",
            [operation_id.as_str()],
            |row| row.get(0),
        )
        .expect("operation status")
}

pub(in crate::discovery_repository::tests) fn assert_unstarted_prepared_retry_predecessors(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    first_retry: &RestartedNativeCommitFixture,
) {
    let database = storage.connection().expect("prepared retry database");
    let prepared_interruptions = database
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_operations
             WHERE id IN (?1, ?2)
               AND status = 'interrupted'
               AND started_at = finished_at",
            rusqlite::params![
                fixture.operation_id.as_str(),
                first_retry.operation_id.as_str(),
            ],
            |row| row.get::<_, u64>(0),
        )
        .expect("count canonical prepared interruptions");
    assert_eq!(prepared_interruptions, 2);
    let start_audits = database
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_audit_log
             WHERE audit_kind = 'operation_started'
               AND subject_id IN (?1, ?2)",
            rusqlite::params![
                fixture.operation_id.as_str(),
                first_retry.operation_id.as_str(),
            ],
            |row| row.get::<_, u64>(0),
        )
        .expect("count prepared interruption start audits");
    assert_eq!(start_audits, 0);
    let predecessor_attestations = database
        .query_row(
            "SELECT COUNT(*)
             FROM provider_discovery_native_no_effect_attestations
             WHERE operation_id IN (?1, ?2)",
            rusqlite::params![
                fixture.operation_id.as_str(),
                first_retry.operation_id.as_str(),
            ],
            |row| row.get::<_, u64>(0),
        )
        .expect("count prepared interruption attestations");
    assert_eq!(predecessor_attestations, 0);
}

fn rewrite_discovery_receipt_event_sequence(
    database: &rusqlite::Connection,
    action_id: &DiscoveryActionId,
    event_id: &str,
    event_sequence: u64,
    next_event_sequence: u64,
) {
    assert_eq!(
        database
            .execute(
                "UPDATE provider_discovery_action_receipts
                 SET event_sequence = ?2,
                     response_json = json_set(
                         response_json,
                         '$.receipt.event_sequence', ?2,
                         '$.event.sequence', ?2,
                         '$.session.next_event_sequence', ?3
                     )
                 WHERE action_id = ?1",
                rusqlite::params![action_id.as_str(), event_sequence, next_event_sequence,],
            )
            .expect("rewrite corrupted discovery receipt sequence"),
        1
    );
    assert_eq!(
        database
            .execute(
                "UPDATE provider_discovery_event_outbox
                 SET sequence = ?2,
                     event_json = json_set(event_json, '$.sequence', ?2)
                 WHERE id = ?1",
                rusqlite::params![event_id, event_sequence],
            )
            .expect("rewrite corrupted discovery event sequence"),
        1
    );
}

pub(in crate::discovery_repository::tests) fn corrupt_retry_start_terminal_event_sequence(
    storage: &Storage,
    fixture: &NativeNoEffectFixture,
    retry: &RestartedNativeCommitFixture,
) -> ProviderDiscoverySession {
    let restart_action_id = storage
        .get_current_discovery_operation(&fixture.session.id)
        .expect("load detached sequence retry operation")
        .expect("active detached sequence retry operation")
        .action_id;
    let database = storage.connection().expect("detached sequence database");
    let (terminal_event_id, terminal_sequence) = database
        .query_row(
            "SELECT event_id, event_sequence
             FROM provider_discovery_action_receipts
             WHERE action_id = ?1",
            [retry.predecessor_action_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .expect("load terminal receipt event identity");
    let (restart_event_id, restart_sequence) = database
        .query_row(
            "SELECT event_id, event_sequence
             FROM provider_discovery_action_receipts
             WHERE action_id = ?1",
            [restart_action_id.as_str()],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?)),
        )
        .expect("load restart receipt event identity");
    assert_eq!(restart_sequence, terminal_sequence + 1);

    let immutable_error = database
        .execute(
            "UPDATE provider_discovery_action_receipts
             SET event_sequence = event_sequence + 100
             WHERE action_id = ?1",
            [retry.predecessor_action_id.as_str()],
        )
        .expect_err("immutable receipt trigger must preserve terminal event sequence");
    assert!(
        immutable_error.to_string().contains("immutable"),
        "unexpected receipt mutation rejection: {immutable_error}"
    );
    database
        .execute_batch(
            "DROP TRIGGER provider_discovery_receipt_no_update;
             DROP TRIGGER provider_discovery_event_identity_no_update;
             DROP TRIGGER provider_discovery_session_revision_guard;",
        )
        .expect("drop immutable event guards only for corruption fixture");

    let shifted_terminal_sequence = terminal_sequence + 100;
    let shifted_restart_sequence = shifted_terminal_sequence + 1;
    rewrite_discovery_receipt_event_sequence(
        &database,
        &retry.predecessor_action_id,
        &terminal_event_id,
        shifted_terminal_sequence,
        shifted_restart_sequence,
    );
    rewrite_discovery_receipt_event_sequence(
        &database,
        &restart_action_id,
        &restart_event_id,
        shifted_restart_sequence,
        shifted_restart_sequence + 1,
    );
    database
        .execute(
            "UPDATE provider_discovery_sessions
             SET next_event_sequence = ?2
             WHERE id = ?1",
            rusqlite::params![fixture.session.id.as_str(), shifted_restart_sequence + 1,],
        )
        .expect("preserve active session cursor after corruption");
    drop(database);

    storage
        .get_discovery_session(&fixture.session.id)
        .expect("load active retry after event-sequence corruption")
        .session
}
