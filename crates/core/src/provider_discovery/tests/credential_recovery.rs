fn persist_unsettled_credential_cancel(
    core: &crate::Core,
    snapshot: &DiscoverySessionSnapshot,
) -> DiscoverySessionSnapshot {
    let orchestrator = core.provider_discovery();
    let envelope = provider_discovery_action_envelope(
        DiscoveryActionId::new(),
        snapshot.session.revision,
        ProviderDiscoveryAction::Cancel,
    )
    .expect("build exact cancellation action");
    let mut draft = hydrate_working_draft(snapshot).expect("hydrate cancellation draft");
    let occurred_at = Utc::now();
    let (approval, review, prepared_commit) = orchestrator
        .prepare_user_action(snapshot, &envelope, &mut draft, occurred_at)
        .expect("prepare exact cancellation action");
    let transition = snapshot
        .session
        .apply(&envelope)
        .expect("apply exact cancellation action");
    orchestrator
        .storage
        .persist_discovery_transition(&DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(
                working_draft_value(&draft).expect("serialize cancellation draft"),
            ),
            review,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval,
            new_operation_id: None,
            completed_operation: None,
            prepared_commit,
            provider_graph: None,
            occurred_at,
        })
        .expect("persist cancellation before prepared-operation settlement");
    orchestrator
        .get(&snapshot.session.id)
        .expect("reload unsettled cancellation")
}
fn seed_started_cancellation_for_tamper(
    root: &std::path::Path,
    connection_id: &str,
) -> DiscoverySessionId {
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open tamper fixture Core");
    let committing = prepare_no_network_credential_commit(&core, connection_id);
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("load tamper fixture credential operation");
    let started = reserve_and_start_credential_install(&core, &prepared);
    core.cancel_provider_discovery(&committing.session.id, started.session_revision)
        .expect("persist tamper fixture cancellation");
    drop(core);
    checkpoint_test_database(&active_test_database_path(root));
    committing.session.id
}

fn restore_schema36_trigger(
    connection: &rusqlite::Connection,
    migration: &str,
    trigger_name: &str,
) {
    connection
        .execute_batch(&format!("DROP TRIGGER {trigger_name};"))
        .unwrap_or_else(|error| panic!("drop schema-37 trigger {trigger_name}: {error}"));
    let marker = format!("CREATE TRIGGER {trigger_name}\n");
    let start = migration
        .find(&marker)
        .unwrap_or_else(|| panic!("find schema-36 trigger {trigger_name}"));
    let tail = &migration[start..];
    let end = tail.find("\nEND;").map_or_else(
        || panic!("find end of schema-36 trigger {trigger_name}"),
        |offset| offset + "\nEND;".len(),
    );
    connection
        .execute_batch(&tail[..end])
        .unwrap_or_else(|error| panic!("restore schema-36 trigger {trigger_name}: {error}"));
}

// Keep the complete schema downgrade in one fixture transaction so callers cannot
// accidentally observe or reuse a partially reversed credential schema.
#[allow(clippy::too_many_lines)]
fn reverse_schema37_credential_migration(database: &std::path::Path) {
    const MIGRATION_0027: &str = include_str!(
        "../../../../storage/migrations/0027_provider_discovery_native_attestations.sql"
    );
    const MIGRATION_0037: &str =
        include_str!("../../../../storage/migrations/0037_provider_credential_operations.sql");

    let connection = rusqlite::Connection::open(database).expect("open current database");
    connection
        .execute_batch("PRAGMA foreign_keys = OFF; BEGIN IMMEDIATE;")
        .expect("begin exact schema-37 inverse");
    schema_fixture::drop_post_schema_37_additive_migrations(&connection);
    for trigger_name in [
        "provider_discovery_native_no_effect_attestation_binding",
        "provider_discovery_operation_legal_transition",
    ] {
        restore_schema36_trigger(&connection, MIGRATION_0027, trigger_name);
    }
    let replaced_objects = MIGRATION_0037
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_ascii_whitespace();
            (tokens.next() == Some("DROP")).then_some(())?;
            Some((tokens.next()?, tokens.next()?.trim_end_matches(';')))
        })
        .collect::<Vec<_>>();
    let created_objects = MIGRATION_0037
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_ascii_whitespace();
            (tokens.next() == Some("CREATE")).then_some(())?;
            let object_type = tokens.next()?;
            let (object_type, name) = if object_type == "UNIQUE" {
                (tokens.next()?, tokens.next()?)
            } else {
                (object_type, tokens.next()?)
            };
            let name = name.trim_end_matches(';');
            (!replaced_objects.contains(&(object_type, name))).then_some((object_type, name))
        })
        .collect::<Vec<_>>();
    assert!(
        created_objects.contains(&(
            "TABLE",
            "provider_discovery_native_credential_legacy_started_cutoff_snapshots"
        )),
        "schema-37 inverse must discover the legacy cutoff table"
    );
    for object_type in ["VIEW", "TRIGGER", "INDEX", "TABLE"] {
        for (_, name) in created_objects
            .iter()
            .rev()
            .filter(|(candidate_type, _)| *candidate_type == object_type)
        {
            connection
                .execute(&format!("DROP {object_type} \"{name}\""), [])
                .unwrap_or_else(|error| panic!("drop schema-37 {object_type} {name}: {error}"));
        }
    }
    assert_eq!(
        connection
            .execute("DELETE FROM schema_migrations WHERE version = 37", [])
            .expect("remove schema-37 migration registry row"),
        1
    );
    connection
        .execute_batch("COMMIT; PRAGMA foreign_keys = ON;")
        .expect("commit exact schema-37 inverse");
    assert_eq!(
        connection
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                row.get::<_, u32>(0)
            })
            .expect("read simulated schema version"),
        36
    );
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .expect("validate simulated schema-36 database"),
        "ok"
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, u64>(0)
            })
            .expect("validate simulated schema-36 foreign keys"),
        0
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn schema36_started_cancel_crash_upgrades_without_synthesizing_physical_authority() {
    let schema36_root = tempdir().expect("temporary schema-36 source root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(schema36_root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open current Core before exact schema inverse");
    let committing =
        prepare_no_network_credential_commit(&core, "schema36-started-cancel-cutoff");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("load schema-36 fixture commit operation");
    let schema36_database = active_test_database_path(schema36_root.path());
    reverse_schema37_credential_migration(&schema36_database);
    assert!(
        core.storage()
            .mark_discovery_operation_started(&prepared.operation_id, Utc::now())
            .expect("start exact credential operation under schema 36")
    );
    let schema36_snapshot = core
        .get_provider_discovery(&committing.session.id)
        .expect("load schema-36 Started discovery");
    let cancelling = core
        .provider_discovery()
        .continue_discovery(
            &committing.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                schema36_snapshot.session.revision,
                ProviderDiscoveryAction::Cancel,
            )
            .expect("build schema-36 cancellation action"),
            None,
        )
        .expect("persist schema-36 Started cancellation");
    assert!(cancelling.session.cancellation_pending);
    assert!(cancelling.session.revision > prepared.session_revision);
    drop(core);
    checkpoint_test_database(&schema36_database);

    let upgraded_root = tempdir().expect("temporary schema-37 upgrade root");
    let canonical_database = upgraded_root.path().join("db/lorepia.sqlite3");
    std::fs::create_dir_all(
        canonical_database
            .parent()
            .expect("canonical database parent"),
    )
    .expect("create canonical database directory");
    std::fs::copy(&schema36_database, &canonical_database)
        .expect("copy genuine schema-36 fixture into upgrade root");

    let upgraded = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(upgraded_root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("upgrade genuine schema-36 Started cancellation");
    assert_eq!(
        upgraded.storage().schema_version().expect("schema version"),
        40
    );
    upgraded
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect_err("legacy unbound Started lineage is never normal install authority");
    let recovery = upgraded
        .get_provider_discovery_credential_install_recovery_context(&committing.session.id)
        .expect("load sealed legacy Started recovery context");
    assert_eq!(recovery.operation_status, DiscoveryOperationStatus::Started);
    assert_eq!(recovery.operation_id, prepared.operation_id);
    assert_eq!(recovery.native_execution_reservation_id, None);
    assert_eq!(recovery.native_execution_id, None);
    assert!(
        ProviderDiscoveryCredentialCommitConfirmation::try_from(&recovery).is_err(),
        "legacy semantic start cannot become physical commit confirmation"
    );

    let upgraded_database = active_test_database_path(upgraded_root.path());
    let connection =
        rusqlite::Connection::open(&upgraded_database).expect("open upgraded database");
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*)
                 FROM provider_discovery_native_credential_legacy_started_cutoff_snapshots
                 WHERE operation_id = ?1",
                [recovery.operation_id.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .expect("count sealed legacy Started cutoff"),
        1
    );
    for table in [
        "provider_discovery_native_credential_executions",
        "provider_discovery_native_credential_store_attempts",
    ] {
        assert_eq!(
            connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE operation_id = ?1"),
                    [recovery.operation_id.as_str()],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap_or_else(|error| panic!("count {table}: {error}")),
            0,
            "schema upgrade must not synthesize a physical credential authority"
        );
    }
    drop(connection);

    upgraded
        .recover_provider_discovery(Utc::now())
        .expect("conservatively recover legacy Started lineage");
    let unknown = upgraded
        .get_provider_discovery(&committing.session.id)
        .expect("load legacy outcome-unknown recovery");
    assert_eq!(unknown.session.state, DiscoveryState::UnknownOutcome);
    assert_eq!(
        unknown.session.unknown_operation,
        Some(DiscoveryOperationKind::AtomicCommit)
    );
    let connection =
        rusqlite::Connection::open(&upgraded_database).expect("reopen upgraded database");
    assert_eq!(
        connection
            .query_row(
                "SELECT action_kind
                 FROM provider_discovery_action_receipts
                 WHERE session_id = ?1 AND resulting_revision = ?2",
                rusqlite::params![committing.session.id.as_str(), unknown.session.revision],
                |row| row.get::<_, String>(0),
            )
            .expect("load legacy Started terminal receipt kind"),
        "interrupt"
    );
    drop(connection);

    let resolution =
        lorepia_domain::discovery::DiscoveryUnknownOutcomeResolution::ConfirmedNoEffect;
    let proposal = approval_proposal_for(
        &unknown.session.id,
        unknown.session.revision,
        DiscoveryApprovalGrant::UnknownOutcomeResolution {
            operation: DiscoveryOperationKind::AtomicCommit,
            resolution: resolution.clone(),
        },
    )
    .expect("derive exact legacy no-effect approval");
    let interrupted = upgraded
        .continue_provider_discovery(
            &unknown.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                unknown.session.revision,
                ProviderDiscoveryAction::ResolveUnknownOutcome {
                    approval_id: proposal.id,
                    resolution,
                },
            )
            .expect("build legacy no-effect resolution action"),
            None,
        )
        .expect("resolve legacy Started outcome as no effect");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    assert!(
        upgraded
            .storage()
            .get_discovery_native_credential_execution(&recovery.operation_id)
            .expect("reload historical legacy execution after no-effect resolution")
            .is_none()
    );

    let restarted = upgraded
        .continue_provider_discovery(
            &interrupted.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                interrupted.session.revision,
                ProviderDiscoveryAction::RestartInterrupted,
            )
            .expect("build explicit legacy restart action"),
            None,
        )
        .expect("restart legacy no-effect recovery");
    assert_eq!(restarted.session.state, DiscoveryState::Compensating);
    assert_ne!(
        restarted
            .active_operation_id
            .as_ref()
            .expect("restarted legacy descendant operation"),
        &recovery.operation_id
    );
    assert!(
        upgraded
            .storage()
            .get_discovery_native_credential_execution(&recovery.operation_id)
            .expect("reload historical legacy execution after descendant")
            .is_none()
    );
    drop(upgraded);

    let reopened = open_core_after_drop(
        upgraded_root.path(),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    );
    assert_eq!(
        reopened
            .get_provider_discovery(&committing.session.id)
            .expect("reload migrated legacy descendant")
            .session
            .state,
        DiscoveryState::Compensating
    );
    assert!(
        reopened
            .storage()
            .get_discovery_native_credential_execution(&recovery.operation_id)
            .expect("reload legacy physical authority projection")
            .is_none(),
        "legacy recovery remains physically unbound after reopen"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn database_rollback_mints_a_new_physical_execution_for_the_same_prepared_operation() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-rollback-incarnation");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("prepared credential install context");
    assert_eq!(
        prepared.operation_status,
        DiscoveryOperationStatus::Prepared
    );
    assert_eq!(prepared.native_execution_reservation_id, None);
    assert_eq!(prepared.native_execution_id, None);
    drop(core);

    let database = active_test_database_path(root.path());
    checkpoint_test_database(&database);
    let prepared_backup = root.path().join("prepared-credential-rollback.sqlite3");
    std::fs::copy(&database, &prepared_backup).expect("snapshot prepared test database");

    let core = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    let execution_a = reserve_and_start_credential_install(&core, &prepared);
    let confirmation_a = credential_commit_confirmation(&execution_a);
    drop(core);

    restore_test_database(&database, &prepared_backup);
    let rolled_back =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    let restored_prepared = rolled_back
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("reload restored prepared context");
    assert_eq!(restored_prepared, prepared);

    let execution_b = reserve_and_start_credential_install(&rolled_back, &restored_prepared);
    assert_eq!(execution_b.operation_id, execution_a.operation_id);
    assert_eq!(execution_b.commit_attempt_id, execution_a.commit_attempt_id);
    assert_eq!(
        execution_b.commit_plan_sha256,
        execution_a.commit_plan_sha256
    );
    assert_ne!(
        execution_b.native_execution_id, execution_a.native_execution_id,
        "rolling durable state back to Prepared must not reuse execution A"
    );

    let error = rolled_back
        .commit_provider_discovery(&committing.session.id, Some(&confirmation_a))
        .expect_err("execution A must not confirm the rolled-back execution B");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        rolled_back
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("reload execution B after stale A confirmation"),
        execution_b
    );

    let stale_attestation = rolled_back
        .attest_provider_discovery_credential_install_no_effect(
            &committing.session.id,
            &execution_b.operation_id,
            &execution_b.commit_attempt_id,
            &execution_b.commit_plan_sha256,
            native_execution_id(&execution_a),
        )
        .expect_err("execution A cannot attest the rolled-back execution B missing");
    assert_eq!(stale_attestation.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        rolled_back
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("reload execution B after stale A attestation"),
        execution_b
    );
    let interrupted = rolled_back
        .attest_provider_discovery_credential_install_no_effect(
            &committing.session.id,
            &execution_b.operation_id,
            &execution_b.commit_attempt_id,
            &execution_b.commit_plan_sha256,
            native_execution_id(&execution_b),
        )
        .expect("execution B can attest its own exact slot missing");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    let attestation = rolled_back
        .storage()
        .get_discovery_native_no_effect_attestation(&execution_b.operation_id)
        .expect("load execution B no-effect evidence")
        .expect("execution B no-effect evidence");
    assert_eq!(
        attestation.physical_authority_id,
        native_execution_id(&execution_b)
    );
    drop(rolled_back);

    let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
    assert_eq!(
        reopened
            .get_provider_discovery(&committing.session.id)
            .expect("reload execution B interruption")
            .session
            .state,
        DiscoveryState::Interrupted
    );
    assert_eq!(
        reopened
            .storage()
            .get_discovery_native_no_effect_attestation(&execution_b.operation_id)
            .expect("reload execution B no-effect evidence")
            .expect("durable execution B no-effect evidence"),
        attestation
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn reopened_rolled_back_reservation_cannot_reuse_a_lost_store_attempt() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-reservation-rollback");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("prepared credential install context");
    let reserved_b = reserve_credential_install(&core, &prepared);

    let database = active_test_database_path(root.path());
    checkpoint_test_database(&database);
    let reserved_backup = root.path().join("reserved-credential-rollback.sqlite3");
    std::fs::copy(&database, &reserved_backup).expect("snapshot reserved test database");

    let started_b = start_reserved_credential_install(&core, &reserved_b);
    let confirmation_b = credential_commit_confirmation(&started_b);
    drop(core);

    restore_test_database(&database, &reserved_backup);
    let reopened =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    let rolled_back = reopened
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("load rolled-back reserved context");
    assert_eq!(rolled_back, reserved_b);
    assert_eq!(
        rolled_back.operation_status,
        DiscoveryOperationStatus::Prepared
    );
    assert!(rolled_back.native_execution_reservation_id.is_some());
    assert_eq!(rolled_back.native_execution_id, None);
    assert!(
        ProviderDiscoveryCredentialCommitConfirmation::try_from(&rolled_back).is_err(),
        "a rolled-back reservation is not external-effect authority"
    );

    let error = reopened
        .start_provider_discovery_credential_install(
            &rolled_back.session_id,
            rolled_back.session_revision,
            &rolled_back.operation_id,
            &rolled_back.commit_attempt_id,
            &rolled_back.commit_plan_sha256,
            rolled_back
                .native_execution_reservation_id
                .as_deref()
                .expect("rolled-back reservation B"),
        )
        .expect_err("a reopened Core must not start process-local reservation B");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    let error = reopened
        .commit_provider_discovery(&committing.session.id, Some(&confirmation_b))
        .expect_err("an externally available B cannot confirm a Prepared rollback");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    let error = reopened
        .reserve_provider_discovery_credential_install(
            &rolled_back.session_id,
            rolled_back.session_revision,
            &rolled_back.operation_id,
            &rolled_back.commit_attempt_id,
            &rolled_back.commit_plan_sha256,
        )
        .expect_err("a reopened Prepared reservation must not be reused");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);

    reopened
        .recover_provider_discovery(Utc::now())
        .expect("terminalize the unstarted rolled-back reservation");
    let interrupted = reopened
        .get_provider_discovery(&committing.session.id)
        .expect("load interrupted rolled-back reservation");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    drop(reopened);

    let reopened_after_recovery =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
    let interrupted = reopened_after_recovery
        .get_provider_discovery(&committing.session.id)
        .expect("reload interrupted rolled-back reservation");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    let abandoned_b = reopened_after_recovery
        .storage()
        .get_discovery_native_credential_execution(&rolled_back.operation_id)
        .expect("reload append-only abandoned reservation B")
        .expect("abandoned reservation B remains auditable");
    assert_eq!(
        Some(abandoned_b.physical_authority_id),
        rolled_back.native_execution_reservation_id
    );
    assert_eq!(abandoned_b.store_started_at, None);
    let restarted = reopened_after_recovery
        .continue_provider_discovery(
            &committing.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                interrupted.session.revision,
                ProviderDiscoveryAction::RestartInterrupted,
            )
            .expect("restart rolled-back reservation action"),
            None,
        )
        .expect("restart with a new semantic operation");
    let next_prepared = reopened_after_recovery
        .get_provider_discovery_credential_install_context(&restarted.session.id)
        .expect("load new prepared credential operation");
    assert_ne!(next_prepared.operation_id, rolled_back.operation_id);
    let reserved_c = reserve_credential_install(&reopened_after_recovery, &next_prepared);
    assert_ne!(
        reserved_c.native_execution_reservation_id,
        rolled_back.native_execution_reservation_id
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn prepared_reserved_cancel_crash_recovers_abandonment_without_reusing_b() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "prepared-cancel-crash-reservation");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("load prepared credential operation");
    let reserved = reserve_credential_install(&core, &prepared);
    let reservation_b = reserved
        .native_execution_reservation_id
        .clone()
        .expect("reserved physical B");
    let committing_snapshot = core
        .get_provider_discovery(&committing.session.id)
        .expect("load committing discovery before cancellation");
    let cancelling = persist_unsettled_credential_cancel(&core, &committing_snapshot);
    assert!(cancelling.session.cancellation_pending);
    assert!(cancelling.session.revision > reserved.session_revision);
    drop(core);

    let reopened =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    reopened
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect_err("normal install context must reject cancellation-pending reservation");
    let recovery = reopened
        .get_provider_discovery_credential_install_recovery_context(&committing.session.id)
        .expect("load exact prepared cancellation recovery context");
    assert_eq!(
        recovery.operation_status,
        DiscoveryOperationStatus::Prepared
    );
    assert_eq!(
        recovery.native_execution_reservation_id.as_deref(),
        Some(reservation_b.as_str())
    );
    assert_eq!(recovery.native_execution_id, None);

    reopened
        .recover_provider_discovery(Utc::now())
        .expect("recover cancellation-pending prepared reservation");
    let interrupted = reopened
        .get_provider_discovery(&committing.session.id)
        .expect("reload recovered prepared cancellation");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    let abandoned = reopened
        .storage()
        .get_discovery_native_credential_execution(&recovery.operation_id)
        .expect("load abandoned prepared reservation")
        .expect("append-only reservation B remains auditable");
    assert_eq!(abandoned.physical_authority_id, reservation_b);
    assert_eq!(abandoned.store_started_at, None);
    reopened
        .start_provider_discovery_credential_install(
            &recovery.session_id,
            recovery.session_revision,
            &recovery.operation_id,
            &recovery.commit_attempt_id,
            &recovery.commit_plan_sha256,
            &abandoned.physical_authority_id,
        )
        .expect_err("abandoned reservation B cannot be reused after recovery");
}

#[test]
fn started_cancel_crash_reopens_with_exact_b_for_compensation() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "started-cancel-crash-authority");
    let credential_authority = core
        .get_provider_discovery_credential_lease_context(&committing.session.id)
        .expect("load immutable credential origin authority before compensation");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("load prepared credential operation");
    let started = reserve_and_start_credential_install(&core, &prepared);
    let physical_b = native_execution_id(&started).to_owned();
    let cancelling = core
        .cancel_provider_discovery(&committing.session.id, started.session_revision)
        .expect("persist Started cancellation intent");
    assert!(cancelling.session.cancellation_pending);
    assert!(cancelling.session.revision > started.session_revision);
    drop(core);

    let reopened =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    reopened
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect_err("normal install context must reject cancellation-pending Started WAL");
    let recovery = reopened
        .get_provider_discovery_credential_install_recovery_context(&committing.session.id)
        .expect("load exact Started cancellation recovery context");
    assert_eq!(recovery.operation_status, DiscoveryOperationStatus::Started);
    assert_eq!(
        recovery.native_execution_reservation_id.as_deref(),
        Some(physical_b.as_str())
    );
    assert_eq!(
        recovery.native_execution_id.as_deref(),
        Some(physical_b.as_str())
    );

    reopened
        .commit_provider_discovery(&committing.session.id, None)
        .expect_err("cancellation-pending Started WAL must enter compensation");
    let authority = reopened
        .get_provider_discovery_credential_compensation_authority(&committing.session.id)
        .expect("load exact physical compensation authority B");
    assert_eq!(authority.operation_id, started.operation_id);
    assert_eq!(authority.native_execution_id, physical_b);
    assert_eq!(
        authority.credential_api_origin,
        credential_authority.credential_api_origin
    );
    assert_eq!(
        authority.credential_origin_approval_id,
        credential_authority.credential_origin_approval_id
    );
    assert_eq!(
        authority.credential_origin_grant_sha256,
        credential_authority.credential_origin_grant_sha256
    );
    assert_eq!(
        authority.connection_binding_sha256,
        credential_authority.connection_binding_sha256
    );
    drop(reopened);

    let reopened =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    assert_eq!(
        reopened
            .get_provider_discovery_credential_compensation_authority(&committing.session.id)
            .expect("reload exact physical compensation authority B"),
        authority
    );
}

#[test]
fn recovery_context_rejects_forged_cancel_revision_and_receipt_history() {
    let revision_root = tempdir().expect("temporary revision-tamper Core root");
    let revision_session = seed_started_cancellation_for_tamper(
        revision_root.path(),
        "started-cancel-revision-tamper",
    );
    let revision_database = active_test_database_path(revision_root.path());
    let revision_connection =
        rusqlite::Connection::open(&revision_database).expect("open revision-tamper database");
    assert_eq!(
        revision_connection
            .execute(
                "UPDATE provider_discovery_sessions
                 SET revision = revision + 1,
                     next_event_sequence = next_event_sequence + 1
                 WHERE id = ?1",
                [revision_session.as_str()],
            )
            .expect("forge unreceipted session revision"),
        1
    );
    drop(revision_connection);
    let revision_core = open_core_after_drop(
        revision_root.path(),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    );
    let revision_error = revision_core
        .get_provider_discovery_credential_install_recovery_context(&revision_session)
        .expect_err("unreceipted cancellation revision must not authorize recovery");
    assert_eq!(revision_error.code, CoreErrorCode::StorageCorrupted);

    let receipt_root = tempdir().expect("temporary receipt-tamper Core root");
    let receipt_session = seed_started_cancellation_for_tamper(
        receipt_root.path(),
        "started-cancel-receipt-tamper",
    );
    let receipt_database = active_test_database_path(receipt_root.path());
    let receipt_connection =
        rusqlite::Connection::open(&receipt_database).expect("open receipt-tamper database");
    receipt_connection
        .execute_batch("DROP TRIGGER provider_discovery_receipt_no_update;")
        .expect("open immutable receipt for synthetic tamper");
    assert_eq!(
        receipt_connection
            .execute(
                "UPDATE provider_discovery_action_receipts
                 SET action_kind = 'approve_review'
                 WHERE session_id = ?1 AND action_kind = 'cancel'",
                [receipt_session.as_str()],
            )
            .expect("forge cancellation receipt kind"),
        1
    );
    drop(receipt_connection);
    let receipt_core = open_core_after_drop(
        receipt_root.path(),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    );
    let receipt_error = receipt_core
        .get_provider_discovery_credential_install_recovery_context(&receipt_session)
        .expect_err("forged cancellation receipt must not authorize recovery");
    assert_eq!(receipt_error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn credential_durability_unknown_is_exact_and_survives_reopen_recovery() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-durability-unknown");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("load prepared credential operation");
    let started = reserve_and_start_credential_install(&core, &prepared);
    let native_execution_id = native_execution_id(&started).to_owned();

    core.mark_provider_discovery_credential_install_durability_unknown(
        &started.session_id,
        started.session_revision + 1,
        &started.operation_id,
        &started.commit_attempt_id,
        &started.commit_plan_sha256,
        &native_execution_id,
        &started.connection_id,
        &started.connection_binding_sha256,
    )
    .expect_err("a stale session revision cannot settle native authority");
    assert_eq!(
        core.get_provider_discovery_credential_install_context(&started.session_id)
            .expect("stale settlement leaves active operation intact")
            .operation_status,
        DiscoveryOperationStatus::Started
    );

    let unknown = core
        .mark_provider_discovery_credential_install_durability_unknown(
            &started.session_id,
            started.session_revision,
            &started.operation_id,
            &started.commit_attempt_id,
            &started.commit_plan_sha256,
            &native_execution_id,
            &started.connection_id,
            &started.connection_binding_sha256,
        )
        .expect("settle exact native durability failure");
    assert_eq!(unknown.session.state, DiscoveryState::UnknownOutcome);
    assert_eq!(
        unknown.session.unknown_operation,
        Some(DiscoveryOperationKind::AtomicCommit)
    );
    assert!(
        core.list_provider_connections()
            .expect("list connections before reopen")
            .iter()
            .all(|connection| connection.id != started.connection_id),
        "visible native bytes must not publish or adopt the provider graph"
    );
    drop(core);

    let reopened =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    reopened
        .recover_provider_discovery(Utc::now())
        .expect("generic recovery preserves explicit unknown outcome");
    let preserved = reopened
        .get_provider_discovery(&started.session_id)
        .expect("reload durability-unknown discovery");
    assert_eq!(preserved.session.state, DiscoveryState::UnknownOutcome);
    assert!(
        reopened
            .list_provider_discovery_credential_recovery_candidates()
            .expect("list recovery candidates")
            .iter()
            .all(|candidate| candidate.session.id != started.session_id),
        "startup must not turn visibility into a new install authority"
    );
    reopened
        .commit_provider_discovery(
            &started.session_id,
            Some(&credential_commit_confirmation(&started)),
        )
        .expect_err("unknown durability cannot be adopted by a later commit call");
}

#[test]
#[allow(clippy::too_many_lines)]
fn native_recovery_owner_reconciles_credential_wal_without_network() {
    #[derive(Debug, Clone, Copy)]
    enum WalState {
        Prepared,
        Started,
    }

    #[derive(Debug, Clone, Copy)]
    enum VaultState {
        Missing,
        Available,
    }

    for (case_index, wal_state, vault_state) in [
        (0, WalState::Prepared, VaultState::Missing),
        (1, WalState::Prepared, VaultState::Available),
        (2, WalState::Started, VaultState::Missing),
        (3, WalState::Started, VaultState::Available),
    ] {
        let root = tempdir().expect("temporary Core root");
        let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
        let committing = prepare_no_network_credential_commit(
            &core,
            &format!("native-recovery-no-network-{case_index}"),
        );
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("prepared credential install context");
        if matches!(wal_state, WalState::Started) {
            reserve_and_start_credential_install(&core, &prepared);
        }
        drop(core);

        let reopened =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
        let preserved = reopened
            .get_provider_discovery(&committing.session.id)
            .expect("load preserved credential commit");
        assert_eq!(preserved.session.state, DiscoveryState::Committing);
        assert_ne!(preserved.session.state, DiscoveryState::UnknownOutcome);
        assert!(
            reopened
                .list_provider_discovery_credential_recovery_candidates()
                .expect("list credential recovery candidates")
                .iter()
                .any(|candidate| candidate.session.id == committing.session.id)
        );
        let context = reopened
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load preserved credential install context");
        assert_eq!(
            context.operation_status,
            match wal_state {
                WalState::Prepared => DiscoveryOperationStatus::Prepared,
                WalState::Started => DiscoveryOperationStatus::Started,
            }
        );
        assert_eq!(
            context.native_execution_id.is_some(),
            matches!(wal_state, WalState::Started),
            "only a durably started WAL has physical native authority"
        );
        assert_eq!(
            context.native_execution_reservation_id.is_some(),
            matches!(wal_state, WalState::Started),
            "these recovery fixtures reserve only immediately before starting"
        );

        let terminal = match (wal_state, vault_state) {
            (WalState::Started, VaultState::Available) => {
                let confirmation = credential_commit_confirmation(&context);
                reopened
                    .commit_provider_discovery(&committing.session.id, Some(&confirmation))
                    .expect("resume exact started credential commit");
                reopened
                    .get_provider_discovery(&committing.session.id)
                    .expect("load resumed credential commit")
            }
            (WalState::Started, VaultState::Missing) => reopened
                .attest_provider_discovery_credential_install_no_effect(
                    &committing.session.id,
                    &context.operation_id,
                    &context.commit_attempt_id,
                    &context.commit_plan_sha256,
                    native_execution_id(&context),
                )
                .expect("attest exact missing credential slot"),
            (WalState::Prepared, VaultState::Missing | VaultState::Available) => {
                reopened
                    .recover_provider_discovery(Utc::now())
                    .expect("conservatively recover prepared credential operation");
                reopened
                    .get_provider_discovery(&committing.session.id)
                    .expect("load interrupted prepared credential operation")
            }
        };
        let expected_state = match (wal_state, vault_state) {
            (WalState::Started, VaultState::Available) => DiscoveryState::Ready,
            _ => DiscoveryState::Interrupted,
        };
        assert_eq!(terminal.session.state, expected_state);
        assert_ne!(terminal.session.state, DiscoveryState::UnknownOutcome);
        assert!(
            reopened
                .list_provider_discovery_credential_recovery_candidates()
                .expect("list reconciled credential recovery candidates")
                .iter()
                .all(|candidate| candidate.session.id != committing.session.id)
        );
        let attestation = reopened
            .storage()
            .get_discovery_native_no_effect_attestation(&context.operation_id)
            .expect("load native recovery attestation");
        assert_eq!(
            attestation.is_some(),
            matches!(
                (wal_state, vault_state),
                (WalState::Started, VaultState::Missing)
            )
        );
        if let Some(attestation) = &attestation {
            assert_eq!(
                attestation.physical_authority_id,
                native_execution_id(&context)
            );
            assert_eq!(attestation.session_id, committing.session.id);
            assert_eq!(attestation.commit_attempt_id, context.commit_attempt_id);
            assert_eq!(attestation.commit_plan_sha256, context.commit_plan_sha256);
            assert_eq!(attestation.connection_id, context.connection_id);
        }
        drop(reopened);

        let final_reopen =
            open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
        assert_eq!(
            final_reopen
                .get_provider_discovery(&committing.session.id)
                .expect("load reconciled discovery")
                .session
                .state,
            expected_state
        );
        assert_eq!(
            final_reopen
                .storage()
                .get_discovery_native_no_effect_attestation(&context.operation_id)
                .expect("load native attestation after final reopen"),
            attestation
        );
    }
}

#[test]
fn core_recovery_owner_conservatively_classifies_started_credential_wal_without_network() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let committing = prepare_no_network_credential_commit(&core, "core-recovery-no-network");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("prepared credential install context");
    let started = reserve_and_start_credential_install(&core, &prepared);
    let error = core
        .attest_provider_discovery_credential_install_no_effect(
            &committing.session.id,
            &prepared.operation_id,
            &prepared.commit_attempt_id,
            &prepared.commit_plan_sha256,
            native_execution_id(&started),
        )
        .expect_err("default Core must not claim native vault provenance");
    assert_eq!(error.code, CoreErrorCode::PermissionDenied);
    assert_eq!(
        core.get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("reload rejected credential attestation context")
            .operation_status,
        DiscoveryOperationStatus::Started
    );
    drop(core);

    let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
    let recovered = reopened
        .get_provider_discovery(&committing.session.id)
        .expect("load conservatively recovered discovery");
    assert_eq!(recovered.session.state, DiscoveryState::UnknownOutcome);
    assert_eq!(
        recovered.session.unknown_operation,
        Some(DiscoveryOperationKind::AtomicCommit)
    );
    assert!(
        reopened
            .get_provider_discovery_credential_lease_context(&committing.session.id)
            .is_err(),
        "unknown external outcomes must never authorize a pre-commit credential lease"
    );
    assert!(
        reopened
            .list_provider_discovery_credential_recovery_candidates()
            .expect("list credential recovery candidates")
            .iter()
            .all(|candidate| candidate.session.id != committing.session.id)
    );
}
