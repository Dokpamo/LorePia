#[test]
fn session_scoped_outbox_poll_isolates_fifo_delivery_without_draining_foreign_events() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let begin_offline = |connection_id: &str, template_id: &str| {
        core.begin_provider_discovery_known(
            SanitizedDiscoveryInput {
                connection_id: ProviderConnectionId::from(connection_id),
                display_name: format!("Synthetic {connection_id}"),
                site_url: HttpUrl::parse("https://provider.example/").expect("site URL"),
                docs_url: None,
                credential_ref: None,
                preferred_assistant: None,
                connection_options: ProviderDiscoveryConnectionOptions::default(),
                supplied_evidence_ids: Vec::new(),
            },
            ProviderTemplateId::from(template_id),
        )
        .expect("begin offline discovery")
    };
    let selected = begin_offline("selected-connection", "missing-selected-template");
    let foreign = begin_offline("foreign-connection", "missing-foreign-template");
    let available_at = Utc::now() + chrono::Duration::days(1);

    let selected_first = core
        .poll_provider_discovery_events_for_session(&selected.session.id, 100, available_at)
        .expect("poll selected discovery");
    assert_eq!(selected_first.len(), 1);
    assert_eq!(selected_first[0].event.session_id, selected.session.id);
    assert_eq!(selected_first[0].event.sequence, 1);

    let global = core
        .poll_provider_discovery_events(100, available_at)
        .expect("poll global discovery outbox");
    assert_eq!(global.len(), 2);
    assert!(
        global
            .iter()
            .any(|event| event.event.session_id == foreign.session.id),
        "session-scoped polling must not drain a foreign event"
    );

    assert!(
        core.ack_provider_discovery_event(&selected_first[0].event.id, available_at)
            .expect("ack selected discovery event")
    );
    let selected_next = core
        .poll_provider_discovery_events_for_session(&selected.session.id, 100, available_at)
        .expect("poll selected discovery after acknowledgement");
    assert_eq!(selected_next.len(), 1);
    assert_eq!(selected_next[0].event.session_id, selected.session.id);
    assert_eq!(selected_next[0].event.sequence, 2);

    let foreign_still_pending = core
        .poll_provider_discovery_events_for_session(&foreign.session.id, 100, available_at)
        .expect("poll foreign discovery independently");
    assert_eq!(foreign_still_pending.len(), 1);
    assert_eq!(
        foreign_still_pending[0].event.session_id,
        foreign.session.id
    );
    assert_eq!(foreign_still_pending[0].event.sequence, 1);
}

#[test]
#[allow(clippy::too_many_lines)]
fn unknown_and_known_discovery_approve_probe_review_commit_and_reopen() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start_with_base("/api/v2");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let redacted_curl = inspect_one_shot_curl(&core, &provider);
    let mut curl_input = curl_discovery_input(&provider, "curl-discovery-connection");
    curl_input.connection_options.api_base_path =
        Some(EndpointPath::parse("/api/v2").expect("explicit custom base"));
    let curl_discovery = core
        .begin_provider_discovery_curl(curl_input, SecretCurlInput::new(redacted_curl))
        .expect("begin public cURL discovery");
    assert!(
        curl_discovery
            .session
            .input
            .connection_options
            .values
            .is_empty()
    );
    assert_eq!(
        curl_discovery
            .session
            .input
            .connection_options
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/api/v2")
    );
    let curl_discovery = select_only_template(&core, &curl_discovery);
    assert_eq!(
        curl_discovery.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval,
        "cURL discovery failed: {:?}",
        curl_discovery.session.failure
    );
    let curl_review = approve_to_review(&core, &curl_discovery, &provider, SECRET_CANARY, false);
    let curl_committing = approve_review(&core, &curl_review, &provider);
    let curl_connection = commit_credential_bound_discovery(&core, &curl_discovery.session.id);
    assert_eq!(curl_committing.session.state, DiscoveryState::Committing);
    assert_eq!(curl_connection.api_origin.as_str(), provider.origin);
    assert_eq!(
        curl_connection
            .config
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        None
    );
    assert!(curl_connection.config.values.iter().any(|entry| {
        entry.key == "api_base_url"
            && matches!(
                &entry.value,
                ConnectionConfigValue::Text(value)
                    if value == &provider.origin
            )
    }));

    let unknown = core
        .begin_provider_discovery_site(discovery_input(&provider, "unknown-discovery-connection"))
        .expect("begin unknown site discovery");
    let unknown = if unknown.session.state == DiscoveryState::AwaitingTemplateSelection {
        continue_with(
            &core,
            &unknown,
            ProviderDiscoveryAction::ContinueWithoutTemplate,
            None,
        )
    } else {
        unknown
    };
    assert!(unknown.session.input.connection_options.values.is_empty());
    assert!(
        unknown
            .session
            .input
            .connection_options
            .api_base_path
            .is_none()
    );
    assert_eq!(
        unknown.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval,
        "unknown discovery failed: {:?}; requests: {:?}",
        unknown.session.failure,
        provider
            .captured_requests()
            .iter()
            .map(|request| String::from_utf8_lossy(request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned())
            .collect::<Vec<_>>()
    );
    assert!(
        !core
            .list_provider_discovery_evidence(&unknown.session.id)
            .expect("unknown discovery evidence")
            .is_empty()
    );
    let unknown_review = approve_to_review(&core, &unknown, &provider, SECRET_CANARY, true);
    let unknown_committing = approve_review(&core, &unknown_review, &provider);
    assert_eq!(unknown_committing.session.state, DiscoveryState::Committing);
    let discovered_connection = commit_credential_bound_discovery(&core, &unknown.session.id);
    assert_eq!(
        discovered_connection
            .config
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        None
    );
    assert!(discovered_connection.config.values.iter().any(|entry| {
        entry.key == "api_base_url"
            && matches!(
                &entry.value,
                ConnectionConfigValue::Text(value)
                    if value == &provider.origin
            )
    }));
    assert_eq!(
        core.get_provider_discovery(&unknown.session.id)
            .expect("load committed unknown discovery")
            .session
            .state,
        DiscoveryState::Ready
    );
    assert_eq!(
        core.list_provider_discovery_approvals(&unknown.session.id)
            .expect("unknown discovery approvals")
            .len(),
        3
    );
    assert!(
        !core
            .list_model_routes(&discovered_connection.id)
            .expect("unknown discovery model routes")
            .is_empty()
    );
    assert_public_surfaces_are_secret_free(&core);
    assert_prompt_bodies_are_secret_free(&provider);
    assert_probe_requests_borrow_credentials(&provider);
    assert_data_root_is_secret_free(root.path());
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen after unknown commit");
    assert_eq!(
        reopened
            .get_provider_discovery(&unknown.session.id)
            .expect("reopen unknown discovery")
            .session
            .state,
        DiscoveryState::Ready
    );
    assert!(
        reopened
            .list_provider_templates()
            .expect("list templates after unknown commit")
            .iter()
            .any(|template| template.id == discovered_connection.template_id)
    );

    let known = reopened
        .begin_provider_discovery_known(
            discovery_input(&provider, "known-discovery-connection"),
            discovered_connection.template_id.clone(),
        )
        .expect("begin known provider discovery");
    let known = select_known_template(&reopened, &known, &discovered_connection.template_id);
    let known_review = approve_to_review(&reopened, &known, &provider, SECRET_CANARY, true);
    let known_committing = approve_review(&reopened, &known_review, &provider);
    assert_eq!(known_committing.session.state, DiscoveryState::Committing);
    let known_session_id = known.session.id.clone();
    drop(reopened);

    let recovered = Core::open(CoreConfig::new(root.path())).expect("reopen interrupted commit");
    let interrupted = recovered
        .get_provider_discovery(&known_session_id)
        .expect("load interrupted known discovery");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    assert_eq!(
        interrupted
            .session
            .recovery
            .as_ref()
            .map(|checkpoint| checkpoint.operation),
        Some(DiscoveryOperationKind::AtomicCommit)
    );
    let restarted = continue_with(
        &recovered,
        &interrupted,
        ProviderDiscoveryAction::RestartInterrupted,
        None,
    );
    assert_eq!(restarted.session.state, DiscoveryState::Committing);
    let known_connection = commit_credential_bound_discovery(&recovered, &known_session_id);
    assert_eq!(
        recovered
            .list_provider_discovery_approvals(&known_session_id)
            .expect("known discovery approvals")
            .len(),
        4
    );
    assert!(
        !recovered
            .list_model_routes(&known_connection.id)
            .expect("known discovery model routes")
            .is_empty()
    );
    assert_public_surfaces_are_secret_free(&recovered);
    assert_prompt_bodies_are_secret_free(&provider);
    assert_probe_requests_borrow_credentials(&provider);
    assert_data_root_is_secret_free(root.path());
    drop(recovered);

    let final_reopen = Core::open(CoreConfig::new(root.path())).expect("final Core reopen");
    assert_eq!(
        final_reopen
            .get_provider_discovery(&known_session_id)
            .expect("load final known discovery")
            .session
            .state,
        DiscoveryState::Ready
    );
    assert!(
        final_reopen
            .list_provider_connections()
            .expect("list final provider connections")
            .iter()
            .any(|connection| connection.id == known_connection.id)
    );
    assert_public_surfaces_are_secret_free(&final_reopen);
    assert_data_root_is_secret_free(root.path());
    drop(final_reopen);
    assert_data_root_is_secret_free(root.path());
}
