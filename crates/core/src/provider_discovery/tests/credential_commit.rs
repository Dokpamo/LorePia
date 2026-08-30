fn finish_no_network_credential_commit(
    core: &crate::Core,
    template: &ProviderTemplate,
    selecting: &DiscoverySessionSnapshot,
) -> DiscoverySessionSnapshot {
    let candidate = core
        .list_provider_discovery_candidates(&selecting.session.id)
        .expect("list template candidates")
        .into_iter()
        .find(|candidate| {
            matches!(
                &candidate.candidate.summary,
                DiscoveryCandidateSummary::ProviderTemplate {
                    template_id,
                    template_version,
                } if template_id == &template.id
                    && *template_version == template.manifest_version
            )
        })
        .expect("exact OpenRouter template candidate");
    let selected = core
        .continue_provider_discovery(
            &selecting.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                selecting.session.revision,
                ProviderDiscoveryAction::SelectTemplate {
                    candidate_id: candidate.candidate.id,
                },
            )
            .expect("select-template action"),
            None,
        )
        .expect("select no-network provider template");
    let credential_proposal = core
        .get_provider_discovery_approval_proposal(&selected.session.id)
        .expect("load credential-origin proposal")
        .expect("credential-origin proposal");
    let listed = approve_credential_and_seed_model_listing(
        core,
        &selected,
        credential_proposal.id,
        &[exact_openrouter_listed_model()],
    );
    let reviewed = core
        .continue_provider_discovery(
            &listed.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                listed.session.revision,
                ProviderDiscoveryAction::SkipProbes,
            )
            .expect("skip-probes action"),
            None,
        )
        .expect("skip no-network capability probes");
    let proposal = core
        .get_provider_discovery_review_proposal(&reviewed.session.id)
        .expect("load review proposal")
        .expect("review proposal");
    let committing = core
        .continue_provider_discovery(
            &reviewed.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                reviewed.session.revision,
                ProviderDiscoveryAction::ApproveReview {
                    approval_id: proposal.approval.id,
                    commit_attempt_id: proposal.commit_attempt_id,
                    commit_plan_sha256: proposal.commit_plan_sha256,
                    graph_sha256: proposal.review.graph_sha256,
                },
            )
            .expect("approve-review action"),
            None,
        )
        .expect("prepare no-network credential commit");
    assert_eq!(committing.session.state, DiscoveryState::Committing);
    committing
}

#[test]
#[allow(clippy::too_many_lines)] // Keeps the real lock/expiry timeline visible in one fixture.
fn credential_graph_publication_rechecks_lan_expiry_after_sqlite_write_lock_wait() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
        .expect("custom OpenAI-compatible template");
    let connection_id = ProviderConnectionId::from("lan-lock-expiry-publication");
    let selecting = core
        .begin_provider_discovery_known(
            SanitizedDiscoveryInput {
                connection_id: connection_id.clone(),
                display_name: "LAN lock expiry provider".to_owned(),
                site_url: HttpUrl::parse("https://models.lan:8443/")
                    .expect("approved LAN site URL"),
                docs_url: None,
                credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                preferred_assistant: None,
                connection_options: ProviderDiscoveryConnectionOptions {
                    network_mode: ProviderNetworkMode::ApprovedLocalNetwork,
                    local_network_approval: Some(ProviderLocalNetworkApproval {
                        origin: CanonicalOrigin::parse("https://models.lan:8443")
                            .expect("approved credential-bearing LAN origin"),
                        addresses: vec!["192.168.10.20".parse::<IpAddr>().unwrap()],
                    }),
                    local_network_approved_at: Some(Utc::now()),
                    ..ProviderDiscoveryConnectionOptions::default()
                },
                supplied_evidence_ids: Vec::new(),
            },
            template.id.clone(),
        )
        .expect("begin LAN provider discovery");

    // Preparing the credential-backed graph can contend with other long-running
    // Core tests. Keep a wide setup margin while still crossing a real expiry
    // boundary under the SQLite write lock below.
    let expires_at = Utc::now() + chrono::Duration::seconds(60);
    let approved_at = expires_at - chrono::Duration::hours(24);
    let mut aged_input = selecting.session.input.clone();
    aged_input.connection_options.local_network_approved_at = Some(approved_at);
    let mut input_json = String::new();
    write_canonical_json(
        &serde_json::to_value(&aged_input).expect("serialize aged LAN input"),
        &mut input_json,
    )
    .expect("canonicalize aged LAN input");
    let database_path = active_test_database_path(root.path());
    let fixture =
        rusqlite::Connection::open(&database_path).expect("open LAN fixture database");
    let revision_guard = fixture
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type = 'trigger' AND name = 'provider_discovery_session_revision_guard'",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("load discovery revision guard");
    fixture
        .execute_batch("DROP TRIGGER provider_discovery_session_revision_guard")
        .expect("suspend revision guard for immutable-time fixture");
    assert_eq!(
        fixture
            .execute(
                "UPDATE provider_discovery_sessions
                 SET sanitized_input_json = ?2, created_at = ?3
                 WHERE id = ?1",
                rusqlite::params![
                    selecting.session.id.as_str(),
                    input_json,
                    approved_at.to_rfc3339(),
                ],
            )
            .expect("age LAN session authority"),
        1
    );
    fixture
        .execute_batch(&revision_guard)
        .expect("restore discovery revision guard");
    drop(fixture);

    let committing = finish_no_network_credential_commit(&core, &template, &selecting);
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("prepared credential install context");
    let started = reserve_and_start_credential_install(&core, &prepared);
    let confirmation = credential_commit_confirmation(&started);
    let lock_at = expires_at - chrono::Duration::seconds(5);
    assert!(
        Utc::now() < lock_at,
        "fixture must finish before the bounded SQLite lock-wait window"
    );
    while Utc::now() < lock_at {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }

    let blocker =
        rusqlite::Connection::open(&database_path).expect("open SQLite write-lock blocker");
    blocker
        .execute_batch("BEGIN IMMEDIATE")
        .expect("acquire SQLite write lock before LAN expiry");
    let error = std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            core.commit_provider_discovery(&committing.session.id, Some(&confirmation))
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            !worker.is_finished(),
            "publication must be waiting on the real SQLite write lock"
        );
        while Utc::now() < expires_at {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        blocker
            .execute_batch("COMMIT")
            .expect("release SQLite write lock after LAN expiry");
        worker
            .join()
            .expect("publication worker")
            .expect_err("expired LAN authority must not publish a provider graph")
    });
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert!(
        core.storage()
            .get_provider_connection(&connection_id)
            .is_err(),
        "expired authority must leave the provider graph unpublished"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn credential_bound_commit_rejects_prepared_wal_until_native_install_is_started() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-commit-start-required");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("prepared credential install context");
    assert_eq!(
        prepared.operation_status,
        DiscoveryOperationStatus::Prepared
    );
    assert_eq!(prepared.native_execution_reservation_id, None);
    assert_eq!(prepared.native_execution_id, None);

    assert!(
        ProviderDiscoveryCredentialCommitConfirmation::try_from(&prepared).is_err(),
        "a prepared operation has no physical native authority to confirm"
    );
    let prepared_confirmation = ProviderDiscoveryCredentialCommitConfirmation {
        operation_id: prepared.operation_id.clone(),
        native_execution_id: "rolled-back-native-execution-A".to_owned(),
        commit_attempt_id: prepared.commit_attempt_id.clone(),
        commit_plan_sha256: prepared.commit_plan_sha256.clone(),
        connection_id: prepared.connection_id.clone(),
        connection_binding_sha256: prepared.connection_binding_sha256.clone(),
    };
    let error = core
        .commit_provider_discovery(&committing.session.id, Some(&prepared_confirmation))
        .expect_err("a future exact envelope must not adopt a rolled-back prepared WAL");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    let unchanged = core
        .get_provider_discovery(&committing.session.id)
        .expect("reload rejected prepared commit");
    assert_eq!(unchanged.session.state, DiscoveryState::Committing);
    assert_eq!(
        core.get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("reload prepared credential install context")
            .operation_status,
        DiscoveryOperationStatus::Prepared
    );
    assert!(
        core.list_provider_connections()
            .expect("list provider connections")
            .iter()
            .all(|connection| connection.id != prepared.connection_id),
        "rejected prepared WAL must publish no provider graph"
    );

    let reserved = reserve_credential_install(&core, &prepared);
    let exact_reservation_id = reserved
        .native_execution_reservation_id
        .as_deref()
        .expect("exact reserved physical authority");
    let stale_operation = core
        .start_provider_discovery_credential_install(
            &reserved.session_id,
            reserved.session_revision,
            &DiscoveryOperationId::new(),
            &reserved.commit_attempt_id,
            &reserved.commit_plan_sha256,
            exact_reservation_id,
        )
        .expect_err("stale semantic provenance must fail before consuming exact B");
    assert_eq!(stale_operation.code, CoreErrorCode::InvalidInput);
    let forged = core
        .start_provider_discovery_credential_install(
            &reserved.session_id,
            reserved.session_revision,
            &reserved.operation_id,
            &reserved.commit_attempt_id,
            &reserved.commit_plan_sha256,
            "discovery-native-00000000-0000-4000-8000-000000000000",
        )
        .expect_err("an unregistered physical reservation cannot start a store");
    assert_eq!(forged.code, CoreErrorCode::InvalidInput);
    let cloned_core = core.clone();
    let started = start_reserved_credential_install(&cloned_core, &reserved);
    assert!(started.native_execution_id.is_some());
    let mut legacy_unbound_started = started.clone();
    legacy_unbound_started.native_execution_reservation_id = None;
    legacy_unbound_started.native_execution_id = None;
    assert!(
        ProviderDiscoveryCredentialCommitConfirmation::try_from(&legacy_unbound_started)
            .is_err(),
        "a migrated Started lineage without physical authority is recovery-only"
    );
    let confirmation = credential_commit_confirmation(&started);
    let mut stale_physical_confirmation = confirmation.clone();
    stale_physical_confirmation.native_execution_id =
        "rolled-back-native-execution-A".to_owned();
    let error = core
        .commit_provider_discovery(&committing.session.id, Some(&stale_physical_confirmation))
        .expect_err("semantic commit provenance must not adopt another physical incarnation");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        core.get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("reload started context after stale physical confirmation"),
        started
    );
    let committed = core
        .commit_provider_discovery(&committing.session.id, Some(&confirmation))
        .expect("commit explicitly-started credential installation");
    assert_eq!(committed.id, prepared.connection_id);
    assert!(
        core.list_provider_connections()
            .expect("list committed provider connections")
            .iter()
            .any(|connection| connection.id == committed.id)
    );
}

#[test]
fn legacy_unbound_started_execution_is_exposed_only_for_conservative_recovery() {
    let started_at = Utc::now();
    assert!(
        native_credential_execution_context_ids(
            DiscoveryOperationStatus::Started,
            Some(&started_at),
            None,
            false,
        )
        .is_err(),
        "normal install context must reject a Started lineage without physical authority"
    );
    assert_eq!(
        native_credential_execution_context_ids(
            DiscoveryOperationStatus::Started,
            Some(&started_at),
            None,
            true,
        )
        .expect("sealed legacy Started lineage is readable only for recovery"),
        (None, None)
    );
}

#[test]
fn repeated_pre_store_recovery_does_not_leak_process_local_reservations() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-reservation-cleanup");
    let mut reserved_ids = BTreeSet::new();

    for cycle in 0..4 {
        let prepared = core
            .get_provider_discovery_credential_install_context(&committing.session.id)
            .expect("load prepared credential operation");
        let reserved = reserve_credential_install(&core, &prepared);
        assert!(
            reserved_ids.insert(
                reserved
                    .native_execution_reservation_id
                    .clone()
                    .expect("fresh physical reservation"),
            ),
            "every restarted semantic operation must reserve a fresh physical id"
        );
        assert_eq!(core.pending_discovery_credential_reservation_count(), 1);

        core.recover_provider_discovery(Utc::now())
            .expect("recover abandoned pre-store reservation");
        assert_eq!(core.pending_discovery_credential_reservation_count(), 0);
        let interrupted = core
            .get_provider_discovery(&committing.session.id)
            .expect("load interrupted pre-store reservation");
        assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
        if cycle < 3 {
            core.continue_provider_discovery(
                &committing.session.id,
                provider_discovery_action_envelope(
                    DiscoveryActionId::new(),
                    interrupted.session.revision,
                    ProviderDiscoveryAction::RestartInterrupted,
                )
                .expect("restart abandoned reservation action"),
                None,
            )
            .expect("restart abandoned pre-store reservation");
        }
    }
    assert_eq!(reserved_ids.len(), 4);
}

#[test]
fn prepared_reservation_cancel_validates_revision_before_process_cleanup() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-reservation-cancel-cleanup");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("load prepared credential operation");
    let reserved = reserve_credential_install(&core, &prepared);
    assert_eq!(core.pending_discovery_credential_reservation_count(), 1);

    core.cancel_provider_discovery(
        &committing.session.id,
        reserved.session_revision.saturating_add(1),
    )
    .expect_err("stale cancellation must fail before reservation cleanup");
    assert_eq!(core.pending_discovery_credential_reservation_count(), 1);

    let cancelled = core
        .cancel_provider_discovery(&committing.session.id, reserved.session_revision)
        .expect("cancel exact prepared reservation");
    assert_eq!(core.pending_discovery_credential_reservation_count(), 0);
    assert!(matches!(
        cancelled.session.state,
        DiscoveryState::Interrupted | DiscoveryState::Cancelled
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn restarted_atomic_credential_install_uses_exact_restart_receipt_authority() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-install-restart-authority");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("initial credential install context");
    let first_started = reserve_and_start_credential_install(&core, &prepared);
    let first_interrupted = core
        .attest_provider_discovery_credential_install_no_effect(
            &committing.session.id,
            &first_started.operation_id,
            &prepared.commit_attempt_id,
            &prepared.commit_plan_sha256,
            native_execution_id(&first_started),
        )
        .expect("attest initial credential installation had no effect");
    assert_eq!(first_interrupted.session.state, DiscoveryState::Interrupted);

    let restarted = core
        .continue_provider_discovery(
            &committing.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                first_interrupted.session.revision,
                ProviderDiscoveryAction::RestartInterrupted,
            )
            .expect("restart-interrupted action"),
            None,
        )
        .expect("restart interrupted credential commit");
    assert_eq!(restarted.session.state, DiscoveryState::Committing);

    let retry_prepared = core
        .get_provider_discovery_credential_install_context(&restarted.session.id)
        .expect("retry credential install context");
    assert_ne!(retry_prepared.operation_id, first_started.operation_id);
    assert_eq!(retry_prepared.session_revision, restarted.session.revision);
    assert_eq!(retry_prepared.commit_attempt_id, prepared.commit_attempt_id);
    assert_eq!(
        retry_prepared.commit_plan_sha256,
        prepared.commit_plan_sha256
    );
    assert_eq!(
        retry_prepared.operation_status,
        DiscoveryOperationStatus::Prepared
    );
    assert_eq!(retry_prepared.native_execution_reservation_id, None);
    assert_eq!(retry_prepared.native_execution_id, None);

    let retry_reserved = reserve_credential_install(&core, &retry_prepared);
    let stale_start = core
        .start_provider_discovery_credential_install(
            &restarted.session.id,
            restarted.session.revision,
            &first_started.operation_id,
            &retry_reserved.commit_attempt_id,
            &retry_reserved.commit_plan_sha256,
            retry_reserved
                .native_execution_reservation_id
                .as_deref()
                .expect("retry native execution reservation"),
        )
        .expect_err("a prior operation must not start the retry credential effect");
    assert_eq!(stale_start.code, CoreErrorCode::InvalidInput);

    let retry_started = start_reserved_credential_install(&core, &retry_reserved);
    assert_eq!(
        retry_started.operation_status,
        DiscoveryOperationStatus::Started
    );
    assert!(retry_started.native_execution_id.is_some());
    assert_ne!(
        retry_started.native_execution_id, first_started.native_execution_id,
        "a restarted semantic commit must mint a new physical native authority"
    );
    let stale_attestation = core
        .attest_provider_discovery_credential_install_no_effect(
            &restarted.session.id,
            &first_started.operation_id,
            &retry_started.commit_attempt_id,
            &retry_started.commit_plan_sha256,
            native_execution_id(&first_started),
        )
        .expect_err("a prior operation must not attest the retry credential slot");
    assert_eq!(stale_attestation.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        core.get_provider_discovery_credential_install_context(&restarted.session.id)
            .expect("reload retry after stale attestation")
            .operation_status,
        DiscoveryOperationStatus::Started
    );
    let retry_interrupted = core
        .attest_provider_discovery_credential_install_no_effect(
            &restarted.session.id,
            &retry_started.operation_id,
            &retry_started.commit_attempt_id,
            &retry_started.commit_plan_sha256,
            native_execution_id(&retry_started),
        )
        .expect("attest retry credential installation had no effect");
    assert_eq!(retry_interrupted.session.state, DiscoveryState::Interrupted);
    let first_attestation = core
        .storage()
        .get_discovery_native_no_effect_attestation(&first_started.operation_id)
        .expect("load initial native no-effect attestation")
        .expect("initial native no-effect attestation");
    assert_eq!(
        first_attestation.physical_authority_id,
        native_execution_id(&first_started)
    );
    let retry_attestation = core
        .storage()
        .get_discovery_native_no_effect_attestation(&retry_started.operation_id)
        .expect("load retry native no-effect attestation")
        .expect("retry native no-effect attestation");
    assert_eq!(
        retry_attestation.physical_authority_id,
        native_execution_id(&retry_started)
    );
    drop(core);

    let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
    assert_eq!(
        reopened
            .get_provider_discovery(&committing.session.id)
            .expect("load twice-interrupted discovery")
            .session
            .state,
        DiscoveryState::Interrupted
    );
    assert_eq!(
        reopened
            .storage()
            .get_discovery_native_no_effect_attestation(&retry_started.operation_id)
            .expect("load retry attestation after reopen")
            .expect("durable retry attestation"),
        retry_attestation
    );
}

#[test]
fn restarted_atomic_commit_rejects_prior_operation_confirmation_before_publish() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-retry-operation-confirmation");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("initial credential install context");
    let first_started = reserve_and_start_credential_install(&core, &prepared);
    let interrupted = core
        .attest_provider_discovery_credential_install_no_effect(
            &committing.session.id,
            &first_started.operation_id,
            &first_started.commit_attempt_id,
            &first_started.commit_plan_sha256,
            native_execution_id(&first_started),
        )
        .expect("attest initial operation had no effect");
    let restarted = core
        .continue_provider_discovery(
            &committing.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                interrupted.session.revision,
                ProviderDiscoveryAction::RestartInterrupted,
            )
            .expect("restart action"),
            None,
        )
        .expect("restart credential commit");
    let retry_prepared = core
        .get_provider_discovery_credential_install_context(&restarted.session.id)
        .expect("retry credential install context");
    let retry_started = reserve_and_start_credential_install(&core, &retry_prepared);
    assert_ne!(first_started.operation_id, retry_started.operation_id);
    assert_eq!(
        first_started.commit_attempt_id,
        retry_started.commit_attempt_id
    );
    drop(core);

    let reopened =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    let reopened_retry = reopened
        .get_provider_discovery_credential_install_context(&restarted.session.id)
        .expect("reload exact started retry context");
    assert_eq!(reopened_retry, retry_started);

    let stale_confirmation = credential_commit_confirmation(&first_started);
    let error = reopened
        .commit_provider_discovery(&restarted.session.id, Some(&stale_confirmation))
        .expect_err("a prior operation's observed slot must not publish the retry graph");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        reopened
            .get_provider_discovery_credential_install_context(&restarted.session.id)
            .expect("reload retry after stale confirmation")
            .operation_status,
        DiscoveryOperationStatus::Started
    );
    assert!(
        reopened
            .list_provider_connections()
            .expect("list provider connections after stale confirmation")
            .iter()
            .all(|connection| connection.id != retry_started.connection_id)
    );

    let exact_confirmation = credential_commit_confirmation(&reopened_retry);
    let committed = reopened
        .commit_provider_discovery(&restarted.session.id, Some(&exact_confirmation))
        .expect("the exact retry operation confirmation publishes the graph");
    assert_eq!(committed.id, retry_started.connection_id);
}

#[test]
fn restarted_atomic_compensation_keeps_retry_operation_physical_authority() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "credential-retry-compensation-authority");
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("initial credential install context");
    let first_started = reserve_and_start_credential_install(&core, &prepared);
    let interrupted = core
        .attest_provider_discovery_credential_install_no_effect(
            &committing.session.id,
            &first_started.operation_id,
            &first_started.commit_attempt_id,
            &first_started.commit_plan_sha256,
            native_execution_id(&first_started),
        )
        .expect("attest initial operation had no effect");
    let restarted = core
        .continue_provider_discovery(
            &committing.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                interrupted.session.revision,
                ProviderDiscoveryAction::RestartInterrupted,
            )
            .expect("restart action"),
            None,
        )
        .expect("restart credential commit");
    let retry_prepared = core
        .get_provider_discovery_credential_install_context(&restarted.session.id)
        .expect("retry credential install context");
    let retry_started = reserve_and_start_credential_install(&core, &retry_prepared);
    let cancelled = core
        .cancel_provider_discovery(&restarted.session.id, restarted.session.revision)
        .expect("request retry cancellation");
    assert!(cancelled.session.cancellation_pending);
    core.commit_provider_discovery(&restarted.session.id, None)
        .expect_err("started retry cancellation enters compensation");

    let authority = core
        .get_provider_discovery_credential_compensation_authority(&restarted.session.id)
        .expect("load retry compensation authority");
    assert_eq!(authority.operation_id, retry_started.operation_id);
    assert_ne!(authority.operation_id, first_started.operation_id);
    assert_eq!(
        Some(authority.native_execution_id.clone()),
        retry_started.native_execution_id
    );
    assert_ne!(
        Some(authority.native_execution_id.clone()),
        first_started.native_execution_id
    );
    assert_eq!(authority.commit_attempt_id, retry_started.commit_attempt_id);
    assert_eq!(
        authority.connection_binding_sha256,
        retry_started.connection_binding_sha256
    );
    drop(core);

    let reopened =
        open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::NativePlatform);
    assert_eq!(
        reopened
            .get_provider_discovery_credential_compensation_authority(&restarted.session.id)
            .expect("reload retry compensation authority"),
        authority
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn confirmed_commit_completion_rejects_a_started_wal_without_an_applied_graph() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open_with_discovery_recovery_owner(
        crate::CoreConfig::new(root.path()),
        crate::DiscoveryRecoveryOwner::NativePlatform,
    )
    .expect("open Core with native recovery ownership");
    let committing =
        prepare_no_network_credential_commit(&core, "confirmed-completion-operation-authority");
    let first_prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("load first credential install context");
    let first_started = reserve_and_start_credential_install(&core, &first_prepared);
    let interrupted = core
        .attest_provider_discovery_credential_install_no_effect(
            &committing.session.id,
            &first_started.operation_id,
            &first_started.commit_attempt_id,
            &first_started.commit_plan_sha256,
            native_execution_id(&first_started),
        )
        .expect("attest first credential install had no effect");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);

    let restarted = core
        .continue_provider_discovery(
            &committing.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                interrupted.session.revision,
                ProviderDiscoveryAction::RestartInterrupted,
            )
            .expect("build restart action"),
            None,
        )
        .expect("restart credential commit");
    let retry_prepared = core
        .get_provider_discovery_credential_install_context(&restarted.session.id)
        .expect("load retry credential install context");
    let retry_started = reserve_and_start_credential_install(&core, &retry_prepared);
    assert_ne!(retry_started.operation_id, first_started.operation_id);
    assert_ne!(
        retry_started.native_execution_id, first_started.native_execution_id,
        "unknown-outcome recovery must retain the retry's physical incarnation"
    );
    drop(core);

    let recovered = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
    let unknown = recovered
        .get_provider_discovery(&restarted.session.id)
        .expect("load unknown retry outcome");
    assert_eq!(unknown.session.state, DiscoveryState::UnknownOutcome);
    assert_eq!(
        unknown.session.unknown_operation,
        Some(DiscoveryOperationKind::AtomicCommit)
    );
    assert!(!unknown.session.cancellation_pending);

    let resolution = lorepia_domain::discovery::DiscoveryUnknownOutcomeResolution::
        ConfirmedCommitCompleted {
            connection_id: retry_started.connection_id.clone(),
        };
    // Unknown-outcome proposals are action-specific because the operator
    // must name the exact committed connection. Derive the canonical ID in
    // test code, then exercise the same public action boundary used by a
    // native client.
    let proposal = approval_proposal_for(
        &unknown.session.id,
        unknown.session.revision,
        DiscoveryApprovalGrant::UnknownOutcomeResolution {
            operation: DiscoveryOperationKind::AtomicCommit,
            resolution: resolution.clone(),
        },
    )
    .expect("derive exact confirmed-completion approval");
    let error = recovered
        .continue_provider_discovery(
            &unknown.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                unknown.session.revision,
                ProviderDiscoveryAction::ResolveUnknownOutcome {
                    approval_id: proposal.id.clone(),
                    resolution,
                },
            )
            .expect("build confirmed-completion action"),
            None,
        )
        .expect_err("a started WAL alone cannot prove that the graph was committed");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(error.message, "confirmed commit graph is missing");
    assert!(
        recovered
            .list_provider_discovery_approvals(&unknown.session.id)
            .expect("list approvals after rejected resolution")
            .iter()
            .all(|approval| approval.id != proposal.id),
        "a rejected resolution must not retain its approval"
    );
    assert_eq!(
        recovered
            .get_provider_discovery(&unknown.session.id)
            .expect("reload rejected unknown outcome")
            .session
            .state,
        DiscoveryState::UnknownOutcome
    );
    recovered
        .ensure_provider_credential_access_settled(&retry_started.connection_id)
        .expect_err("an uncommitted graph cannot grant credential access");
    drop(recovered);

    let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
    assert_eq!(
        reopened
            .get_provider_discovery(&unknown.session.id)
            .expect("reload unknown outcome after reopen")
            .session
            .state,
        DiscoveryState::UnknownOutcome
    );
    reopened
        .ensure_provider_credential_access_settled(&retry_started.connection_id)
        .expect_err("reopen must not invent credential ownership");
}
