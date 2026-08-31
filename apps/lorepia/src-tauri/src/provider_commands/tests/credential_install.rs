
    #[test]
    fn precommit_capture_supplies_only_the_exact_session_and_restart_requires_recapture() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let session = prepare_precommit_discovery(&shell, "precommit-capture");
        let state = AppState::new(root.path().to_path_buf());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));

        let capture = tauri::async_runtime::block_on(capture_precommit_discovery_credential_with(
            &vault,
            &state,
            &shell,
            &session.id,
            session.revision,
        ))
        .expect("capture process-local discovery credential");
        assert_eq!(capture.clipboard_cleanup, ClipboardCleanupStatus::Cleared);
        let credential = credential_for_discovery_action(
            &state,
            &shell,
            &session,
            session.revision,
            &shell::ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin {
                approval_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            },
        )
        .expect("borrow exact discovery credential")
        .expect("credential-bound action receives a credential");
        assert_eq!(format!("{credential:?}"), "SecretCredential([REDACTED])");
        assert_eq!(*calls.lock().expect("fake calls"), vec!["capture"]);

        let mut unbound = session.clone();
        unbound.credential_binding_requested = false;
        unbound.state = "awaiting_probe_consent".to_owned();
        assert!(
            credential_for_discovery_action(
                &state,
                &shell,
                &unbound,
                unbound.revision,
                &shell::ContinueProviderDiscoveryActionInput::ApproveProbes {
                    approval_id: "00000000-0000-4000-8000-000000000011".to_owned(),
                    approval_grant_sha256: "d".repeat(64),
                },
            )
            .expect("credential-free probes must not consult a credential lease")
            .is_none()
        );

        let restarted = AppState::new(root.path().to_path_buf());
        assert!(
            credential_for_discovery_action(
                &restarted,
                &shell,
                &session,
                session.revision,
                &shell::ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin {
                    approval_id: "00000000-0000-4000-8000-000000000001".to_owned(),
                },
            )
            .is_err(),
            "a process restart must fail closed and require recapture"
        );
    }

    #[tokio::test]
    async fn capture_rejects_exact_bound_slot_appearing_during_clipboard_read() {
        let context = install_context("started");
        let authority = discovery_credential_authority(&context).expect("started authority");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_raw(&context.connection_id);
        vault.insert_bound_during_capture(&context.connection_id, &authority);

        let result = capture_discovery_credential_for_empty_bound_slot_with(
            &vault,
            &context.connection_id,
            &authority,
        )
        .await;
        assert!(
            result.is_err(),
            "the second pre-store observation must reject a newly appeared exact slot"
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec!["vault_bound_status", "capture", "vault_bound_status"]
        );
        assert_eq!(
            vault.raw_status(&context.connection_id),
            CredentialStatus::Available,
            "direct capture must never inspect or mutate the legacy raw slot"
        );
        assert_eq!(
            vault.bound_status(&context.connection_id, &authority),
            CredentialStatus::Available
        );
    }

    #[tokio::test]
    async fn raw_available_exact_missing_handoff_starts_then_stores_once() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        let binding = commit_lease_binding(&context);
        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_raw(&context.connection_id);
        vault.fail_store_after_effect();
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));

        assert!(
            promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
                .await
                .expect("authoritative exact postflight wins over ambiguous store return")
        );
        let started = journal.context.lock().expect("fake journal").clone();
        let current_execution_authority =
            discovery_credential_authority(&started).expect("current B execution authority");
        assert_eq!(
            vault.bound_status(&candidate.connection_id, &current_execution_authority),
            CredentialStatus::Available,
            "a mutate-then-error store succeeds only from an exact Match for current B"
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status",
                "vault_prepare_store",
                "wal_started",
                "vault_store",
                "vault_observe"
            ]
        );
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Missing,
            "handoff moves the runtime secret exactly once"
        );
        assert_eq!(
            vault.raw_status(&candidate.connection_id),
            CredentialStatus::Available,
            "authority-scoped handoff must preserve the independent raw legacy slot"
        );
        assert!(
            !promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
                .await
                .expect("started handoff replay is a no-op"),
            "a later commit command must not repeat the vault store"
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status",
                "vault_prepare_store",
                "wal_started",
                "vault_store",
                "vault_observe",
                "wal_context"
            ]
        );
    }

    #[tokio::test]
    async fn discovery_store_recovery_required_never_adopts_visible_match() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        state
            .install_discovery_credential_lease(
                commit_lease_binding(&context),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.require_recovery_after_store_effect();
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));

        let error = promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("visible Match cannot override explicit recovery-required");
        assert_eq!(error.code, "credential_recovery_required");
        assert_eq!(
            journal
                .context
                .lock()
                .expect("fake journal")
                .operation_status,
            "outcome_unknown"
        );
        let calls = calls.lock().expect("fake calls");
        assert_eq!(calls.last(), Some(&"wal_unknown"));
        assert!(
            !calls.contains(&"vault_observe"),
            "durability-unknown WAL settlement must precede and suppress native observation"
        );
    }

    #[tokio::test]
    async fn mismatched_started_discovery_authority_never_reaches_native_store() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        state
            .install_discovery_credential_lease(
                commit_lease_binding(&context),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));
        journal.mismatch_next_started_context();

        promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("mismatched Started authority must fail before native mutation");
        assert!(
            !calls.lock().expect("fake calls").contains(&"vault_store"),
            "native store is downstream of exact durable Started validation"
        );
    }

    #[tokio::test]
    async fn prepared_store_validation_failure_never_crosses_started_cutpoint() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        let binding = commit_lease_binding(&context);
        state
            .install_discovery_credential_lease(
                binding,
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.fail_prepare_store();
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));

        promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("fallible platform preparation must fail before Started");

        let persisted = journal.context.lock().expect("fake journal").clone();
        assert_eq!(persisted.operation_status, "prepared");
        assert!(persisted.native_execution_reservation_id.is_some());
        assert!(persisted.native_execution_id.is_none());
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status",
                "vault_prepare_store"
            ],
            "no journal start or native store may follow a preparation failure"
        );
    }

    #[tokio::test]
    async fn exact_current_execution_slot_blocks_handoff_before_vault_store() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        let binding = commit_lease_binding(&context);
        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));
        let connection_binding_sha256 = journal
            .context
            .lock()
            .expect("fake journal")
            .connection_binding_sha256
            .clone();
        let current_authority = CredentialAuthority::new(
            journal.next_native_execution_id(),
            connection_binding_sha256,
        )
        .expect("current execution authority");
        vault.insert_bound(&candidate.connection_id, &current_authority);

        promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("the exact current physical slot must never be overwritten or adopted");
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec!["wal_context", "wal_reserved", "vault_bound_status"]
        );
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Available,
            "refusal must not silently discard the recapturable runtime lease"
        );
    }

    #[tokio::test]
    async fn exact_slot_appearing_between_reserved_pre_start_guards_is_never_stored() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let context = install_context("prepared");
        let candidate = commit_candidate(&context);
        let binding = commit_lease_binding(&context);
        state
            .install_discovery_credential_lease(
                binding.clone(),
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        let journal = FakeDiscoveryJournal::new(context, Arc::clone(&calls));
        let authority = CredentialAuthority::new(
            journal.next_native_execution_id(),
            journal
                .context
                .lock()
                .expect("fake journal")
                .connection_binding_sha256
                .clone(),
        )
        .expect("current execution authority");
        vault.insert_bound_after_next_status(&candidate.connection_id, &authority);

        promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate)
            .await
            .expect_err("the second reserved pre-start status must reject a newly appeared slot");
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status"
            ]
        );
        assert_eq!(
            journal
                .context
                .lock()
                .expect("fake journal")
                .operation_status,
            "prepared",
            "a failed reservation barrier must remain before the Started store-attempt cutpoint"
        );
        assert_eq!(
            state.discovery_credential_lease_status(&binding),
            CredentialStatus::Available,
            "the runtime lease is not consumed when the second guard rejects"
        );
        assert_eq!(
            vault.bound_status(&candidate.connection_id, &authority),
            CredentialStatus::Available
        );

        let restarted_context = journal.context.lock().expect("fake journal").clone();
        assert!(restarted_context.native_execution_reservation_id.is_some());
        assert!(restarted_context.native_execution_id.is_none());
        require_started_discovery_credential_install(&restarted_context)
            .expect_err("a reserved Prepared operation cannot publish a native slot");
        let reopened = AppState::new(root.path().to_path_buf());
        promote_discovery_credential_lease_with(&vault, &reopened, &journal, &candidate)
            .await
            .expect_err("reopen must not reuse an existing Prepared reservation");
        assert_eq!(
            calls.lock().expect("fake calls").last(),
            Some(&"wal_context"),
            "reopen fails before reserve, platform observation, or store"
        );
        let calls_before_recovery = calls.lock().expect("fake calls").clone();
        let recovery_status =
            discovery_committing_credential_status_with(&vault, &restarted_context)
                .await
                .expect("project crash-after-reserve recovery status");
        assert_eq!(recovery_status, CredentialStatus::Missing);
        assert_eq!(
            *calls.lock().expect("fake calls"),
            calls_before_recovery,
            "bootstrap must not inspect a reserved Prepared physical slot"
        );
        assert_eq!(
            credential_install_recovery_action(true, "prepared", recovery_status)
                .expect("classify cancelled crash after reserve and before Started"),
            CredentialInstallRecoveryAction::DeferToCore,
            "a crash at the reserved-Prepared barrier cannot adopt B"
        );
    }

    #[tokio::test]
    async fn rolled_back_prior_execution_slot_is_not_adopted_by_new_install_execution() {
        let root = tempdir().expect("temporary root");
        let state = AppState::new(root.path().to_path_buf());
        let prior_started = install_context("started");
        let prior_execution_authority =
            discovery_credential_authority(&prior_started).expect("prior execution authority");
        // Simulate a database/context rollback across A's Started cutpoint.
        // The old native item survives, while the restored Prepared operation
        // has neither a reservation nor usable physical authority.
        let mut rolled_back_prepared = prior_started.clone();
        rolled_back_prepared.operation_status = "prepared".to_owned();
        rolled_back_prepared.native_execution_reservation_id = None;
        rolled_back_prepared.native_execution_id = None;
        let candidate = commit_candidate(&rolled_back_prepared);
        let binding = commit_lease_binding(&rolled_back_prepared);
        state
            .install_discovery_credential_lease(
                binding,
                NativeCredential::new("synthetic-discovery-secret".to_owned()),
            )
            .expect("install runtime lease after restored Prepared snapshot");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(
            &rolled_back_prepared.connection_id,
            &prior_execution_authority,
        );
        vault.restore_rolled_back_bound_slot_before_next_store(
            &rolled_back_prepared.connection_id,
            &prior_execution_authority,
        );
        let journal = FakeDiscoveryJournal::new(rolled_back_prepared, Arc::clone(&calls));

        let result =
            promote_discovery_credential_lease_with(&vault, &state, &journal, &candidate).await;

        assert!(
            result.is_err(),
            "a prior execution envelope restored after a database rollback must not prove the new store"
        );
        let execution_b = journal.context.lock().expect("fake journal").clone();
        let current_execution_authority =
            discovery_credential_authority(&execution_b).expect("current execution B authority");
        assert_ne!(prior_execution_authority, current_execution_authority);
        assert_eq!(
            vault.bound_status(&candidate.connection_id, &prior_execution_authority),
            CredentialStatus::Available,
            "the stale A slot is neither adopted nor overwritten"
        );
        assert_eq!(
            vault.bound_status(&candidate.connection_id, &current_execution_authority),
            CredentialStatus::Missing,
            "a no-effect B store must not be published from stale A evidence"
        );
        assert_eq!(
            credential_install_recovery_action(false, "started", CredentialStatus::Missing)
                .expect("classify crash after Started with no B effect"),
            CredentialInstallRecoveryAction::DeferToCore,
            "bare Started is intent-only and visibility cannot settle it"
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec![
                "wal_context",
                "wal_reserved",
                "vault_bound_status",
                "vault_bound_status",
                "vault_prepare_store",
                "wal_started",
                "vault_store",
                "vault_observe",
                "wal_no_effect"
            ]
        );
    }

    #[tokio::test]
    async fn crash_after_started_before_store_recovers_only_from_exact_b() {
        let mut stale_a = install_context("started");
        stale_a.native_execution_reservation_id = Some("native-execution-A".to_owned());
        stale_a.native_execution_id = Some("native-execution-A".to_owned());
        let stale_authority = discovery_credential_authority(&stale_a).expect("stale A authority");

        let mut current_b = stale_a.clone();
        current_b.native_execution_reservation_id = Some("native-execution-B".to_owned());
        current_b.native_execution_id = Some("native-execution-B".to_owned());
        let current_authority =
            discovery_credential_authority(&current_b).expect("current B authority");
        assert_ne!(stale_authority, current_authority);

        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&current_b.connection_id, &stale_authority);

        let status = discovery_committing_credential_status_with(&vault, &current_b)
            .await
            .expect("observe current B after crash before store");
        assert_eq!(status, CredentialStatus::Missing);
        assert_eq!(
            credential_install_recovery_action(true, "started", status)
                .expect("classify cancelled exact B no-effect"),
            CredentialInstallRecoveryAction::DeferToCore
        );
        assert_eq!(
            *calls.lock().expect("fake calls"),
            vec!["vault_observe"],
            "recovery observes current B once and never falls back to stale A"
        );
        assert_eq!(
            vault.bound_status(&current_b.connection_id, &stale_authority),
            CredentialStatus::Available
        );
        assert_eq!(
            vault.bound_status(&current_b.connection_id, &current_authority),
            CredentialStatus::Missing
        );
    }

    #[tokio::test]
    async fn migrated_pre37_started_without_execution_defers_without_vault_access() {
        let mut legacy_started = install_context("started");
        legacy_started.native_execution_reservation_id = None;
        legacy_started.native_execution_id = None;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));

        let status = discovery_committing_credential_status_with(&vault, &legacy_started)
            .await
            .expect("classify sealed pre37 Started context");
        assert_eq!(status, CredentialStatus::Unreadable);
        assert!(
            calls.lock().expect("fake calls").is_empty(),
            "legacy Started without exact B must not inspect or adopt any vault slot"
        );
        assert_eq!(
            credential_install_recovery_action(true, "started", status)
                .expect("defer cancelled legacy Started recovery to Core"),
            CredentialInstallRecoveryAction::DeferToCore
        );
        require_started_discovery_credential_install(&legacy_started)
            .expect_err("legacy Started cannot produce confirmation authority");
        shell::ProviderDiscoveryCredentialCommitConfirmationDto::try_from(&legacy_started)
            .expect_err("legacy Started cannot forge a native execution confirmation");
    }

    #[test]
    fn migrated_pre37_started_runs_full_adapter_recovery_without_vault_authority() {
        let root = tempdir().expect("temporary root");
        let fixture =
            shell::test_support::seed_synthetic_migrated_pre37_started_discovery(root.path())
                .expect("seed migration-sealed pre37 Started discovery");
        let legacy = fixture
            .shell
            .get_provider_discovery_credential_install_recovery_context(&fixture.session_id)
            .expect("load recovery-only legacy context");
        assert_eq!(legacy.operation_status, "started");
        assert!(legacy.native_execution_reservation_id.is_none());
        assert!(legacy.native_execution_id.is_none());

        let recovered = recover_provider_discovery_credential_installs(&fixture.shell)
            .expect("Tauri startup adapter defers sealed legacy recovery to Core");
        assert_eq!(recovered.len(), 1);
        let unknown = fixture
            .shell
            .get_provider_discovery(&fixture.session_id)
            .expect("load recovered legacy session");
        assert_eq!(unknown.state, "unknown_outcome");
        assert_eq!(unknown.unknown_operation.as_deref(), Some("atomic_commit"));
        assert!(unknown.active_operation_id.is_none());
        assert!(
            fixture
                .shell
                .list_provider_connections()
                .expect("list provider connections")
                .iter()
                .all(|connection| connection.id != legacy.connection_id),
            "sealed pre37 Started recovery cannot publish or adopt a provider graph"
        );
        assert!(
            fixture
                .shell
                .list_provider_discovery_credential_recovery_candidates()
                .expect("list terminal credential recovery candidates")
                .is_empty()
        );
    }

    #[test]
    fn exact_started_discovery_restart_settles_unknown_without_native_observation() {
        let root = tempdir().expect("temporary root");
        let fixture =
            shell::test_support::seed_synthetic_started_discovery_credential_install(root.path())
                .expect("seed exact Started discovery");
        let session = fixture
            .shell
            .get_provider_discovery(&fixture.install.session_id)
            .expect("load Started discovery session");

        settle_started_discovery_credential_recovery(&fixture.shell, &session, &fixture.install)
            .expect("startup settles exact Started as durability unknown");

        let unknown = fixture
            .shell
            .get_provider_discovery(&fixture.install.session_id)
            .expect("reload durability-unknown session");
        assert_eq!(unknown.state, "unknown_outcome");
        assert_eq!(unknown.unknown_operation.as_deref(), Some("atomic_commit"));
        assert!(unknown.active_operation_id.is_none());
        assert!(
            fixture
                .shell
                .list_provider_connections()
                .expect("list connections")
                .iter()
                .all(|connection| connection.id != fixture.install.connection_id),
            "bare Started recovery cannot publish or adopt the provider graph"
        );
    }

    #[tokio::test]
    async fn rolled_back_prepared_wal_never_adopts_future_exact_envelope() {
        let started = install_context("started");
        let stale_authority =
            discovery_credential_authority(&started).expect("stale execution authority");
        let mut context = started;
        context.operation_status = "prepared".to_owned();
        context.native_execution_reservation_id = None;
        context.native_execution_id = None;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&context.connection_id, &stale_authority);
        assert_eq!(
            discovery_committing_credential_status_with(&vault, &context)
                .await
                .expect("project rollback-visible prepared status"),
            CredentialStatus::Missing,
            "Prepared has no physical authority and cannot inspect or adopt stale A"
        );
        assert!(calls.lock().expect("fake calls").is_empty());
        discovery_credential_authority(&context)
            .expect_err("Prepared cannot invent a physical authority");
        require_started_discovery_credential_install(&context)
            .expect_err("commit confirmation must reject a Prepared WAL");

        let missing_calls = Arc::new(Mutex::new(Vec::new()));
        let missing = FakeDiscoveryVault::new(Arc::clone(&missing_calls));
        assert_eq!(
            discovery_committing_credential_status_with(&missing, &context)
                .await
                .expect("project safe prepared missing status"),
            CredentialStatus::Missing
        );

        let error_calls = Arc::new(Mutex::new(Vec::new()));
        let error = FakeDiscoveryVault::new(Arc::clone(&error_calls));
        error.fail_status();
        assert_eq!(
            discovery_committing_credential_status_with(&error, &context)
                .await
                .expect("Prepared does not consult a nonexistent physical authority"),
            CredentialStatus::Missing
        );
        assert!(error_calls.lock().expect("error calls").is_empty());
    }

    #[tokio::test]
    async fn retry_started_ignores_restored_prior_operation_slot() {
        let mut first = install_context("started");
        first.operation_id = "00000000-0000-4000-8000-000000000041".to_owned();
        first.native_execution_reservation_id = Some("native-execution-A".to_owned());
        first.native_execution_id = Some("native-execution-A".to_owned());
        let mut retry = first.clone();
        retry.operation_id = "00000000-0000-4000-8000-000000000042".to_owned();
        retry.native_execution_reservation_id = Some("native-execution-B".to_owned());
        retry.native_execution_id = Some("native-execution-B".to_owned());
        assert_eq!(first.commit_attempt_id, retry.commit_attempt_id);

        let first_authority =
            discovery_credential_authority(&first).expect("prior operation authority");
        let retry_authority =
            discovery_credential_authority(&retry).expect("retry operation authority");
        assert_ne!(first_authority, retry_authority);

        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&retry.connection_id, &first_authority);

        // A process restart reconstructs the current authority from durable
        // operation context. Restoring the prior operation's envelope must
        // therefore look missing, never resumable or publishable.
        let reopened_context = retry.clone();
        let stale_status = discovery_committing_credential_status_with(&vault, &reopened_context)
            .await
            .expect("observe retry authority after reopen");
        assert_eq!(stale_status, CredentialStatus::Missing);
        assert_eq!(
            credential_install_recovery_action(false, "started", stale_status)
                .expect("classify restored prior slot"),
            CredentialInstallRecoveryAction::DeferToCore
        );
        assert_eq!(
            vault.bound_status(&retry.connection_id, &first_authority),
            CredentialStatus::Available
        );
        assert_eq!(
            vault.bound_status(&retry.connection_id, &retry_authority),
            CredentialStatus::Missing
        );

        vault.insert_bound(&retry.connection_id, &retry_authority);
        let exact_status = discovery_committing_credential_status_with(&vault, &reopened_context)
            .await
            .expect("observe exact retry authority");
        assert_eq!(exact_status, CredentialStatus::Available);
        assert_eq!(
            credential_install_recovery_action(false, "started", exact_status)
                .expect("classify exact retry slot"),
            CredentialInstallRecoveryAction::DeferToCore
        );
        assert_eq!(
            credential_install_recovery_action(true, "started", exact_status)
                .expect("classify cancelled exact retry slot"),
            CredentialInstallRecoveryAction::DeferToCore
        );
    }
