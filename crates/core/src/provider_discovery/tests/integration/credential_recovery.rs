
#[test]
#[allow(clippy::too_many_lines)]
fn native_owned_startup_preserves_credential_wal_until_vault_reconciliation() {
    #[derive(Clone, Copy)]
    enum WalState {
        Prepared,
        Started,
    }

    #[derive(Clone, Copy)]
    enum VaultState {
        Missing,
        Available,
    }

    let provider = SyntheticProvider::start();
    let cases = [
        ("prepared-missing", WalState::Prepared, VaultState::Missing),
        (
            "prepared-available",
            WalState::Prepared,
            VaultState::Available,
        ),
        ("started-missing", WalState::Started, VaultState::Missing),
        (
            "started-available",
            WalState::Started,
            VaultState::Available,
        ),
    ];

    for (connection_id, wal_state, vault_state) in cases {
        let root = tempdir().expect("temporary Core root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
        let started = core
            .begin_provider_discovery_site(discovery_input(&provider, connection_id))
            .expect("begin credential recovery discovery");
        let reviewed = approve_to_review(&core, &started, &provider, SECRET_CANARY, false);
        let _committing = approve_review(&core, &reviewed, &provider);
        let prepared = core
            .get_provider_discovery_credential_install_context(&started.session.id)
            .expect("prepared credential install context");
        if matches!(wal_state, WalState::Started) {
            let durable_started = reserve_and_start_credential_install(&core, prepared.clone());
            assert_eq!(
                durable_started.operation_status,
                DiscoveryOperationStatus::Started
            );
        }
        drop(core);

        let reopened = Core::open_with_discovery_recovery_owner(
            CoreConfig::new(root.path()),
            DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core for native recovery");
        let preserved = reopened
            .get_provider_discovery(&started.session.id)
            .expect("load preserved discovery");
        assert_eq!(
            preserved.session.state,
            DiscoveryState::Committing,
            "native-owned open must not prematurely classify {connection_id}"
        );
        let recovery_candidates = reopened
            .list_provider_discovery_credential_recovery_candidates()
            .expect("list complete credential recovery candidates");
        assert!(
            recovery_candidates
                .iter()
                .any(|candidate| candidate.session.id == started.session.id),
            "native-owned startup must preserve this credential WAL for vault reconciliation"
        );
        let context = reopened
            .get_provider_discovery_credential_install_context(&started.session.id)
            .expect("load preserved credential install context");
        assert_eq!(
            context.operation_status,
            match wal_state {
                WalState::Prepared => DiscoveryOperationStatus::Prepared,
                WalState::Started => DiscoveryOperationStatus::Started,
            }
        );

        let terminal = match (wal_state, vault_state) {
            (WalState::Started, VaultState::Available) => {
                let confirmation =
                    ProviderDiscoveryCredentialCommitConfirmation::try_from(&context)
                        .expect("started recovery context has physical authority");
                reopened
                    .commit_provider_discovery(&started.session.id, Some(&confirmation))
                    .expect("resume exact started credential commit");
                reopened
                    .get_provider_discovery(&started.session.id)
                    .expect("load resumed discovery")
            }
            (WalState::Started, VaultState::Missing) => reopened
                .attest_provider_discovery_credential_install_no_effect(
                    &started.session.id,
                    &context.operation_id,
                    &context.commit_attempt_id,
                    &context.commit_plan_sha256,
                    native_execution_id(&context),
                )
                .expect("attest exact missing credential slot"),
            (WalState::Prepared, VaultState::Missing | VaultState::Available) => {
                reopened
                    .recover_provider_discovery(Utc::now())
                    .expect("conservatively recover prepared operation");
                reopened
                    .get_provider_discovery(&started.session.id)
                    .expect("load interrupted prepared discovery")
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
                .all(|candidate| candidate.session.id != started.session.id)
        );
        if !matches!(expected_state, DiscoveryState::Ready) {
            assert!(
                reopened
                    .list_provider_connections()
                    .expect("list provider connections")
                    .iter()
                    .all(|connection| connection.id != started.session.input.connection_id)
            );
        }
        drop(reopened);

        let final_reopen =
            Core::open(CoreConfig::new(root.path())).expect("reopen reconciled Core");
        assert_eq!(
            final_reopen
                .get_provider_discovery(&started.session.id)
                .expect("load reconciled discovery")
                .session
                .state,
            expected_state
        );
    }
}

#[test]
fn core_owned_startup_conservatively_classifies_started_credential_wal() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let started = core
        .begin_provider_discovery_site(discovery_input(&provider, "core-owned-started-credential"))
        .expect("begin credential recovery discovery");
    let reviewed = approve_to_review(&core, &started, &provider, SECRET_CANARY, false);
    let _committing = approve_review(&core, &reviewed, &provider);
    let prepared = core
        .get_provider_discovery_credential_install_context(&started.session.id)
        .expect("prepared credential install context");
    reserve_and_start_credential_install(&core, prepared);
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen Core-owned recovery");
    let recovered = reopened
        .get_provider_discovery(&started.session.id)
        .expect("load recovered discovery");
    assert_eq!(recovered.session.state, DiscoveryState::UnknownOutcome);
    assert_eq!(
        recovered.session.unknown_operation,
        Some(DiscoveryOperationKind::AtomicCommit)
    );
    assert!(
        reopened
            .list_provider_connections()
            .expect("list provider connections")
            .iter()
            .all(|connection| connection.id != started.session.input.connection_id)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn cancelled_commit_reopens_and_completes_explicit_compensation_restart() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let started = core
        .begin_provider_discovery_site(discovery_input(&provider, "cancelled-discovery-connection"))
        .expect("begin cancellable discovery");
    let review = approve_to_review(&core, &started, &provider, SECRET_CANARY, true);
    let committing = approve_review(&core, &review, &provider);
    assert_eq!(committing.session.state, DiscoveryState::Committing);

    let interrupted = core
        .cancel_provider_discovery(&started.session.id, committing.session.revision)
        .expect("cancel prepared commit");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    assert!(interrupted.session.cancellation_pending);
    assert_eq!(
        interrupted
            .session
            .recovery
            .as_ref()
            .map(|checkpoint| checkpoint.operation),
        Some(DiscoveryOperationKind::Compensation)
    );
    assert!(
        core.list_provider_connections()
            .expect("list connections before compensation")
            .iter()
            .all(|connection| connection.id != started.session.input.connection_id)
    );
    assert_data_root_is_secret_free(root.path());
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen cancelled commit");
    let persisted = reopened
        .get_provider_discovery(&started.session.id)
        .expect("load cancelled commit");
    assert_eq!(persisted.session.state, DiscoveryState::Interrupted);
    let compensating = continue_with(
        &reopened,
        &persisted,
        ProviderDiscoveryAction::RestartInterrupted,
        None,
    );
    assert_eq!(compensating.session.state, DiscoveryState::Compensating);
    let awaiting_native = reopened
        .continue_provider_discovery_compensation(&started.session.id)
        .expect("run Core-owned compensation steps");
    assert_eq!(awaiting_native.session.state, DiscoveryState::Compensating);
    let attempt_id = awaiting_native
        .session
        .commit_attempt_id
        .as_ref()
        .expect("compensation commit attempt");
    let steps = reopened
        .list_provider_discovery_compensation_steps(attempt_id)
        .expect("list compensation steps");
    let credential_step = steps
        .iter()
        .find(|step| step.kind == DiscoveryCompensationKind::RemoveCredentialSlot)
        .expect("native credential compensation step");
    let started_step = reopened
        .start_provider_discovery_credential_compensation(&started.session.id, &credential_step.id)
        .expect("start native credential compensation");
    assert_eq!(started_step.status, DiscoveryCompensationStatus::InProgress);
    let cancelled = reopened
        .complete_provider_discovery_credential_compensation(
            &started.session.id,
            &credential_step.id,
        )
        .expect("complete native credential compensation");
    assert_eq!(cancelled.session.state, DiscoveryState::Cancelled);
    assert!(!cancelled.session.cancellation_pending);
    assert!(
        reopened
            .list_provider_discovery_compensation_steps(attempt_id)
            .expect("list completed compensation steps")
            .iter()
            .all(|step| step.status == DiscoveryCompensationStatus::Completed)
    );
    assert!(
        reopened
            .list_provider_connections()
            .expect("list connections after compensation")
            .iter()
            .all(|connection| connection.id != started.session.input.connection_id)
    );
    assert_public_surfaces_are_secret_free(&reopened);
    assert_prompt_bodies_are_secret_free(&provider);
    assert_probe_requests_borrow_credentials(&provider);
    assert_data_root_is_secret_free(root.path());
    drop(reopened);

    let final_reopen = Core::open(CoreConfig::new(root.path())).expect("reopen compensated Core");
    assert_eq!(
        final_reopen
            .get_provider_discovery(&started.session.id)
            .expect("load compensated discovery")
            .session
            .state,
        DiscoveryState::Cancelled
    );
    drop(final_reopen);
    assert_data_root_is_secret_free(root.path());
}

#[test]
fn credential_bearing_curl_requires_inspection_before_fresh_evidence_submission() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let awaiting = core
        .begin_provider_discovery_site(evidence_starved_input(
            &provider,
            "fresh-evidence-connection",
        ))
        .expect("begin evidence-starved discovery");
    assert_eq!(awaiting.session.state, DiscoveryState::AwaitingMoreEvidence);
    let evidence_before = core
        .list_provider_discovery_evidence(&awaiting.session.id)
        .expect("list initial evidence");
    let raw_curl = || {
        SecretCurlInput::new(format!(
            "curl -X POST '{}/v1/chat/completions' \
             -H 'Authorization: Bearer {SECRET_CANARY}' \
             -H 'Content-Type: application/json' \
             --data-raw '{{\"model\":\"synthetic-model\",\"messages\":[]}}'",
            provider.origin
        ))
    };

    let begin_error = core
        .begin_provider_discovery_curl(
            curl_discovery_input(&provider, "uninspected-curl-connection"),
            raw_curl(),
        )
        .expect_err("initial raw credential-bearing cURL must fail closed");
    assert_eq!(begin_error.code, lorepia_core::CoreErrorCode::InvalidInput);
    assert!(begin_error.message.contains("inspected first"));
    assert_no_secret(&begin_error.message, "initial credential handoff error");

    let error = core
        .supply_provider_discovery_evidence(
            &awaiting.session.id,
            awaiting.session.revision,
            ProviderDiscoveryAdditionalEvidence::curl(raw_curl()),
        )
        .expect_err("raw credential-bearing cURL must fail closed");
    assert_eq!(error.code, lorepia_core::CoreErrorCode::InvalidInput);
    assert!(error.message.contains("inspected first"));
    assert_no_secret(&error.message, "credential handoff error");
    let unchanged = core
        .get_provider_discovery(&awaiting.session.id)
        .expect("reload unchanged discovery");
    assert_eq!(
        unchanged.session.state,
        DiscoveryState::AwaitingMoreEvidence
    );
    assert_eq!(unchanged.session.revision, awaiting.session.revision);
    assert_eq!(
        core.list_provider_discovery_evidence(&awaiting.session.id)
            .expect("list unchanged evidence"),
        evidence_before
    );

    let inspection = core
        .inspect_provider_curl(
            raw_curl(),
            awaiting.session.input.connection_options.clone(),
        )
        .expect("inspect credential-bearing cURL");
    assert_eq!(
        inspection.extracted_credential(),
        Some(SECRET_CANARY.as_bytes())
    );
    let redacted = inspection.redacted_curl().to_owned();
    drop(inspection);
    let supplied = core
        .supply_provider_discovery_evidence(
            &awaiting.session.id,
            awaiting.session.revision,
            ProviderDiscoveryAdditionalEvidence::curl(SecretCurlInput::new(redacted)),
        )
        .expect("submit inspected redacted cURL");
    assert_eq!(
        supplied.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval
    );
    assert_public_surfaces_are_secret_free(&core);
    assert_data_root_is_secret_free(root.path());
}
