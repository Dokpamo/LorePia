fn read_probe_request_headers(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set probe request timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).expect("read probe request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(request).expect("probe request is UTF-8")
}
fn spawn_stalling_probe_provider() -> (
    String,
    std_mpsc::Receiver<String>,
    std_mpsc::Sender<()>,
    std_mpsc::Receiver<bool>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe provider");
    let address = listener.local_addr().expect("probe provider address");
    let (request_sender, request_receiver) = std_mpsc::channel();
    let (release_sender, release_receiver) = std_mpsc::channel();
    let (later_dispatch_sender, later_dispatch_receiver) = std_mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut first, _) = listener.accept().expect("accept first probe request");
        request_sender
            .send(read_probe_request_headers(&mut first))
            .expect("report first probe request");
        release_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("release first probe request");
        drop(first);

        listener
            .set_nonblocking(true)
            .expect("make probe listener nonblocking");
        let deadline = std::time::Instant::now() + Duration::from_millis(750);
        let mut later_dispatch = false;
        while std::time::Instant::now() < deadline {
            match listener.accept() {
                Ok((_stream, _)) => {
                    later_dispatch = true;
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept later probe request: {error}"),
            }
        }
        later_dispatch_sender
            .send(later_dispatch)
            .expect("report later probe dispatch");
    });
    (
        format!("http://{address}"),
        request_receiver,
        release_sender,
        later_dispatch_receiver,
        handle,
    )
}

fn probe_route(id: &str, endpoint_path: &str) -> ModelRoute {
    let now = Utc::now();
    ModelRoute {
        id: ModelRouteId::from(id),
        connection_id: ProviderConnectionId::from("probe-connection"),
        api_family: ApiFamily::OpenAiChatCompletions,
        model_id: format!("{id}-model"),
        display_name: None,
        route_config: ModelRouteConfig {
            endpoint_path: Some(EndpointPath::parse(endpoint_path).expect("endpoint path")),
            ..ModelRouteConfig::default()
        },
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::Legacy,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: now,
        last_seen_at: Some(now),
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn cached_credential_authority_cannot_start_discovery_after_terminal_removal() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let connection_id = ProviderConnectionId::from("core-stale-discovery-authority");
    core.storage()
        .save_provider_profile(&ProviderProfile {
            id: connection_id.as_str().to_owned(),
            display_name: "Core stale discovery authority".to_owned(),
            base_url: "https://provider.example/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        })
        .expect("save credential-bound provider");
    let install_authority = core
        .storage()
        .propose_provider_credential_install_authority(&connection_id)
        .expect("propose credential install authority");
    let install = core
        .storage()
        .prepare_provider_credential_operation_with_install_authority(
            &connection_id,
            ProviderCredentialOperationKind::Install,
            ProviderCredentialObservedStatus::Missing,
            Some(&install_authority),
        )
        .expect("prepare credential install");
    core.storage()
        .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
        .expect("start credential install");
    core.storage()
        .finish_provider_credential_operation(
            &install.plan.operation_id,
            &install.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("finish credential install");
    let cached_authority = core
        .storage()
        .ensure_provider_credential_access_settled(&connection_id)
        .expect("capture credential read authority");
    let removal = core
        .storage()
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare credential removal");
    core.storage()
        .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
        .expect("start credential removal");
    core.storage()
        .finish_provider_credential_operation(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("terminalize credential removal");

    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
        .expect("OpenRouter template");
    let error = core
        .begin_provider_discovery_known_with_credential_authority(
            SanitizedDiscoveryInput {
                connection_id: connection_id.clone(),
                display_name: "Rejected cached discovery".to_owned(),
                site_url: HttpUrl::parse("https://openrouter.ai/")
                    .expect("OpenRouter site URL"),
                docs_url: None,
                credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                preferred_assistant: None,
                connection_options: ProviderDiscoveryConnectionOptions::default(),
                supplied_evidence_ids: Vec::new(),
            },
            template.id,
            Some(cached_authority),
        )
        .expect_err("terminal removal must invalidate cached discovery authority");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert!(
        core.list_provider_discoveries(10)
            .expect("list rejected discovery work")
            .is_empty(),
        "rejected admission must execute no provider discovery work"
    );
    assert!(
        core.poll_provider_discovery_events(10, Utc::now())
            .expect("poll rejected discovery outbox")
            .is_empty(),
        "rejected admission must publish no provider discovery event"
    );
}

#[test]
fn approved_probe_route_preflight_preserves_exact_route_and_rejects_scope_drift() {
    let first = probe_route("route-a", "/deployments/a/chat/completions");
    let second = probe_route("route-b", "/deployments/b/chat/completions");
    let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::Site);
    draft.routes = vec![first.clone(), second.clone()];
    draft.probe_route_ids = vec![first.id.clone(), second.id.clone()];
    let budget = standard_probe_budget(2).expect("standard budget");

    let approved = approved_probe_routes(&draft, budget).expect("approved routes");
    assert_eq!(approved, vec![first, second]);
    assert_eq!(
        approved[0]
            .route_config
            .endpoint_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/deployments/a/chat/completions")
    );
    assert_eq!(
        approved[1]
            .route_config
            .endpoint_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/deployments/b/chat/completions")
    );

    let mut duplicate = draft.clone();
    duplicate.probe_route_ids =
        vec![ModelRouteId::from("route-a"), ModelRouteId::from("route-a")];
    assert!(approved_probe_routes(&duplicate, budget).is_err());

    let mut outside_graph = draft.clone();
    outside_graph.probe_route_ids = vec![
        ModelRouteId::from("route-a"),
        ModelRouteId::from("route-outside"),
    ];
    assert!(approved_probe_routes(&outside_graph, budget).is_err());

    let one_route_budget = standard_probe_budget(1).expect("one-route budget");
    assert!(approved_probe_routes(&draft, one_route_budget).is_err());
}

fn prepare_openrouter_credential_origin_approval(
    core: &crate::Core,
    connection_id: &str,
) -> DiscoverySessionSnapshot {
    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
        .expect("OpenRouter template");
    let connection_id = ProviderConnectionId::from(connection_id);
    let selecting = core
        .begin_provider_discovery_known(
            SanitizedDiscoveryInput {
                connection_id: connection_id.clone(),
                display_name: "Pre-commit credential provider".to_owned(),
                site_url: HttpUrl::parse("https://openrouter.ai/")
                    .expect("OpenRouter site URL"),
                docs_url: None,
                credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
                preferred_assistant: None,
                connection_options: ProviderDiscoveryConnectionOptions::default(),
                supplied_evidence_ids: Vec::new(),
            },
            template.id.clone(),
        )
        .expect("begin provider discovery");
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
        .expect("select OpenRouter template");
    assert_eq!(
        selected.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval
    );
    selected
}

#[test]
#[allow(clippy::too_many_lines)]
fn cancellation_during_authenticated_probe_prevents_every_later_dispatch() {
    let (origin, request_receiver, release_sender, later_dispatch_receiver, server) =
        spawn_stalling_probe_provider();
    let api_origin = CanonicalOrigin::parse(&origin).expect("canonical probe origin");
    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
        .expect("OpenAI-compatible template");
    let auth = template.default_manifest.auth.clone();
    let connection_id = ProviderConnectionId::from("cancelled-authenticated-probes");
    let connection = ProviderConnection {
        id: connection_id.clone(),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        display_name: "Cancelled authenticated probes".to_owned(),
        api_origin: api_origin.clone(),
        config: ConnectionConfig {
            api_base_path: Some(EndpointPath::parse("/v1").expect("probe base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            local_network_approval: None,
            values: vec![lorepia_domain::ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(format!("{origin}/v1")),
            }],
        },
        credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
        credential_scope: Some(CredentialScope {
            allowed_origins: vec![api_origin],
            auth_binding: auth,
            redirect_policy: CredentialRedirectPolicy::Deny,
        }),
        timeout_seconds: 5,
        status: ConnectionStatus::Untested,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let route_id = ModelRouteId::from("cancelled-authenticated-probe-route");
    let route = ModelRoute {
        id: route_id.clone(),
        connection_id: connection_id.clone(),
        api_family: template.api_family,
        model_id: "cancelled-probe-model".to_owned(),
        display_name: None,
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::Legacy,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: Utc::now(),
        last_seen_at: None,
    };
    let options = ProviderDiscoveryConnectionOptions {
        network_mode: ProviderNetworkMode::LocalLoopback,
        ..ProviderDiscoveryConnectionOptions::default()
    };
    let session = ProviderDiscoverySession::new(
        DiscoverySessionId::from("cancelled-authenticated-probe-session"),
        SanitizedDiscoveryInput {
            connection_id: connection_id.clone(),
            display_name: "Cancelled authenticated probes".to_owned(),
            site_url: HttpUrl::parse(&format!("{origin}/")).expect("probe site URL"),
            docs_url: None,
            credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
            preferred_assistant: None,
            connection_options: options,
            supplied_evidence_ids: Vec::new(),
        },
    )
    .expect("probe session");
    let now = Utc::now();
    let snapshot = DiscoverySessionSnapshot {
        session,
        active_operation_id: None,
        draft_json: None,
        review: None,
        created_at: now,
        updated_at: now,
    };
    let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::KnownProvider {
        template_id: template.id.clone(),
    });
    draft.template = Some(template);
    draft.connection = Some(connection);
    draft.routes = vec![route];
    draft.probe_route_ids = vec![route_id];
    let budget = standard_probe_budget(1).expect("standard probe budget");
    let (cancel_sender, cancelled) = watch::channel(false);
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("probe runtime"));
    let worker_runtime = Arc::clone(&runtime);
    let worker = thread::spawn(move || {
        probe_draft(
            worker_runtime.handle(),
            &snapshot,
            &mut draft,
            Some("authenticated-probe-secret"),
            budget,
            cancelled,
        )
    });

    let request = request_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("first authenticated probe dispatched");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer authenticated-probe-secret\r\n")
    );
    cancel_sender
        .send(true)
        .expect("cancel in-flight authenticated probe");
    thread::sleep(Duration::from_millis(50));
    release_sender.send(()).expect("release first probe socket");

    let outcome = worker
        .join()
        .expect("join probe worker")
        .expect("probe cancellation outcome");
    assert!(matches!(outcome, ProbeExecution::Unknown));
    assert!(
        !later_dispatch_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("later dispatch observation"),
        "no later authenticated probe may dispatch after cancellation"
    );
    server.join().expect("join probe provider");
}

#[test]
#[allow(clippy::too_many_lines)]
fn request_cancellation_does_not_fake_completion_of_started_authenticated_listing() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let selected =
        prepare_openrouter_credential_origin_approval(&core, "started-list-cancellation");
    let proposal = core
        .get_provider_discovery_approval_proposal(&selected.session.id)
        .expect("load credential-origin proposal")
        .expect("credential-origin proposal");
    let orchestrator = core.provider_discovery();
    let envelope = provider_discovery_action_envelope(
        DiscoveryActionId::new(),
        selected.session.revision,
        ProviderDiscoveryAction::ApproveCredentialOrigin {
            approval_id: proposal.id,
        },
    )
    .expect("approve credential origin");
    let mut draft = hydrate_working_draft(&selected).expect("hydrate selected draft");
    let occurred_at = Utc::now();
    let (approval, review, prepared_commit) = orchestrator
        .prepare_user_action(&selected, &envelope, &mut draft, occurred_at)
        .expect("prepare credential approval");
    let transition = selected
        .session
        .apply(&envelope)
        .expect("apply credential approval");
    let operation_id = DiscoveryOperationId::new();
    orchestrator
        .storage
        .persist_discovery_transition(&DiscoveryTransitionWrite {
            transition,
            draft: DiscoveryJsonUpdate::Replace(
                working_draft_value(&draft).expect("serialize approved draft"),
            ),
            review,
            new_evidence: Vec::new(),
            new_candidates: Vec::new(),
            approval,
            new_operation_id: Some(operation_id.clone()),
            completed_operation: None,
            prepared_commit,
            provider_graph: None,
            occurred_at,
        })
        .expect("persist listing operation");
    assert!(
        orchestrator
            .storage
            .mark_discovery_operation_started(&operation_id, Utc::now())
            .expect("start authenticated listing")
    );
    let listing = core
        .get_provider_discovery(&selected.session.id)
        .expect("load started listing");

    let cancelling = core
        .cancel_provider_discovery(&listing.session.id, listing.session.revision)
        .expect("persist cancellation request");

    assert_eq!(cancelling.session.state, DiscoveryState::ListingModels);
    assert!(cancelling.session.cancellation_pending);
    let active = core
        .storage()
        .get_current_discovery_operation(&listing.session.id)
        .expect("load active listing")
        .expect("started listing remains active");
    assert_eq!(active.id, operation_id);
    assert_eq!(active.status, DiscoveryOperationStatus::Started);

    let rebased = orchestrator
        .inflight_completion_snapshot(&listing, &operation_id)
        .expect("rebase worker completion onto cancellation revision");
    assert_eq!(rebased.session.revision, cancelling.session.revision);
    let mut worker_draft =
        hydrate_working_draft(&listing).expect("hydrate in-flight worker draft");
    orchestrator
        .persist_operation_completion(
            &rebased,
            &operation_id,
            &mut worker_draft,
            ProviderDiscoveryAction::Interrupt {
                operation: DiscoveryOperationKind::ListModels,
                outcome: DiscoveryInterruptionOutcome::ConfirmedNoExternalEffect,
            },
            DurableOperationOutcome::Interrupted,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )
        .expect("settle actual cancelled worker outcome");
    let settled = core
        .get_provider_discovery(&listing.session.id)
        .expect("load settled cancellation");
    assert_eq!(settled.session.state, DiscoveryState::Cancelled);
    assert!(!settled.session.cancellation_pending);
}

#[test]
fn discovery_credential_lease_is_stable_from_origin_approval_through_review() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let selected =
        prepare_openrouter_credential_origin_approval(&core, "precommit-lease-stable");
    let proposal = core
        .get_provider_discovery_approval_proposal(&selected.session.id)
        .expect("load credential-origin proposal")
        .expect("credential-origin proposal");
    let before_approval = core
        .get_provider_discovery_credential_lease_context(&selected.session.id)
        .expect("prospective credential lease context");
    assert_eq!(before_approval.session_id, selected.session.id);
    assert_eq!(
        before_approval.connection_id.as_str(),
        "precommit-lease-stable"
    );
    assert_eq!(before_approval.credential_origin_approval_id, proposal.id);
    assert_eq!(
        before_approval.credential_origin_grant_sha256,
        proposal.grant_sha256
    );
    assert_eq!(before_approval.connection_binding_sha256.len(), 64);

    let listed = approve_credential_and_seed_model_listing(
        &core,
        &selected,
        proposal.id,
        &[exact_openrouter_listed_model()],
    );
    assert_eq!(listed.session.state, DiscoveryState::AwaitingProbeConsent);
    assert_eq!(
        core.get_provider_discovery_credential_lease_context(&listed.session.id)
            .expect("post-listing credential lease context"),
        before_approval
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
        .expect("skip capability probes");
    assert_eq!(reviewed.session.state, DiscoveryState::AwaitingReview);
    assert_eq!(
        core.get_provider_discovery_credential_lease_context(&reviewed.session.id)
            .expect("review credential lease context"),
        before_approval
    );

    drop(core);
    let core = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
    assert_eq!(
        core.get_provider_discovery_credential_lease_context(&reviewed.session.id)
            .expect("reopened review credential lease context"),
        before_approval
    );

    let cancelled = core
        .cancel_provider_discovery(&reviewed.session.id, reviewed.session.revision)
        .expect("cancel pre-commit discovery");
    assert_eq!(cancelled.session.state, DiscoveryState::Cancelled);
    assert!(
        core.get_provider_discovery_credential_lease_context(&cancelled.session.id)
            .is_err(),
        "terminal discovery must not retain credential lease authority"
    );
}

#[test]
fn discovery_credential_lease_survives_only_list_or_probe_interruption() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");

    let selected =
        prepare_openrouter_credential_origin_approval(&core, "precommit-list-interrupted");
    let proposal = core
        .get_provider_discovery_approval_proposal(&selected.session.id)
        .expect("load credential-origin proposal")
        .expect("credential-origin proposal");
    let expected = core
        .get_provider_discovery_credential_lease_context(&selected.session.id)
        .expect("prospective credential lease context");
    let interrupted = core
        .continue_provider_discovery(
            &selected.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                selected.session.revision,
                ProviderDiscoveryAction::ApproveCredentialOrigin {
                    approval_id: proposal.id,
                },
            )
            .expect("approve credential-origin action"),
            None,
        )
        .expect("interrupt credential-bound model listing without a credential");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    assert_eq!(
        interrupted
            .session
            .recovery
            .as_ref()
            .map(|recovery| recovery.operation),
        Some(DiscoveryOperationKind::ListModels)
    );
    assert_eq!(
        core.get_provider_discovery_credential_lease_context(&interrupted.session.id)
            .expect("interrupted listing credential lease context"),
        expected
    );

    let selected =
        prepare_openrouter_credential_origin_approval(&core, "precommit-probe-interrupted");
    let proposal = core
        .get_provider_discovery_approval_proposal(&selected.session.id)
        .expect("load second credential-origin proposal")
        .expect("second credential-origin proposal");
    let expected = core
        .get_provider_discovery_credential_lease_context(&selected.session.id)
        .expect("second prospective credential lease context");
    let listed = approve_credential_and_seed_model_listing(
        &core,
        &selected,
        proposal.id,
        &[exact_openrouter_listed_model()],
    );
    let probe = core
        .get_provider_discovery_approval_proposal(&listed.session.id)
        .expect("load probe proposal")
        .expect("probe proposal");
    let interrupted = core
        .continue_provider_discovery(
            &listed.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                listed.session.revision,
                ProviderDiscoveryAction::ApproveProbes {
                    approval_id: probe.id,
                    approval_grant_sha256: probe.grant_sha256,
                },
            )
            .expect("approve probes action"),
            None,
        )
        .expect("interrupt credential-bound probes without a credential");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    assert_eq!(
        interrupted
            .session
            .recovery
            .as_ref()
            .map(|recovery| recovery.operation),
        Some(DiscoveryOperationKind::ProbeCapabilities)
    );
    assert_eq!(
        core.get_provider_discovery_credential_lease_context(&interrupted.session.id)
            .expect("interrupted probe credential lease context"),
        expected
    );
}

#[test]
fn discovery_credential_lease_rejects_origin_auth_and_connection_binding_drift() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let selected =
        prepare_openrouter_credential_origin_approval(&core, "precommit-lease-drift");
    let proposal = core
        .get_provider_discovery_approval_proposal(&selected.session.id)
        .expect("load credential-origin proposal")
        .expect("credential-origin proposal");
    let listed = approve_credential_and_seed_model_listing(
        &core,
        &selected,
        proposal.id,
        &[exact_openrouter_listed_model()],
    );
    let approval = core
        .list_provider_discovery_approvals(&listed.session.id)
        .expect("list discovery approvals")
        .into_iter()
        .find(|approval| {
            matches!(
                &approval.grant,
                DiscoveryApprovalGrant::CredentialOrigin { .. }
            )
        })
        .expect("durable credential-origin approval");
    let draft = hydrate_working_draft(&listed).expect("hydrate approved draft");
    validate_credential_origin_approval(&listed, &draft, &approval)
        .expect("unchanged approval binding");
    let connection = draft.connection.as_ref().expect("approved connection");
    validated_discovery_credential_binding_sha256(&listed, &draft, connection)
        .expect("unchanged final binding");

    let mut origin_drift = draft.clone();
    origin_drift
        .connection
        .as_mut()
        .expect("connection")
        .api_origin = CanonicalOrigin::parse("https://drift.example").expect("drift origin");
    assert!(validate_credential_origin_approval(&listed, &origin_drift, &approval).is_err());

    let mut auth_drift = draft.clone();
    auth_drift
        .connection
        .as_mut()
        .expect("connection")
        .credential_scope
        .as_mut()
        .expect("credential scope")
        .auth_binding = AuthBinding::None;
    assert!(validate_credential_origin_approval(&listed, &auth_drift, &approval).is_err());

    let mut binding_drift = draft.clone();
    binding_drift
        .connection
        .as_mut()
        .expect("connection")
        .config
        .api_base_path = Some(EndpointPath::parse("/drift").expect("drift base path"));
    assert!(
        validated_discovery_credential_binding_sha256(
            &listed,
            &binding_drift,
            binding_drift.connection.as_ref().expect("connection"),
        )
        .is_err()
    );
}
