fn input_with_options(
    site_url: &str,
    connection_options: ProviderDiscoveryConnectionOptions,
) -> SanitizedDiscoveryInput {
    SanitizedDiscoveryInput {
        connection_id: ProviderConnectionId::from("policy-test-connection"),
        display_name: "Policy test provider".to_owned(),
        site_url: HttpUrl::parse(site_url).unwrap(),
        docs_url: None,
        credential_ref: None,
        preferred_assistant: None,
        connection_options,
        supplied_evidence_ids: Vec::new(),
    }
}
#[test]
fn signed_discovery_template_without_current_operational_authority_fails_closed() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let now = Utc::now();
    let mut template =
        AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter).unwrap();
    template.source = TemplateSource::SignedCatalog;
    template.manifest_version += 1;
    let authority = DiscoveryCatalogAuthorityBinding::new(
        1,
        &template,
        now + chrono::Duration::minutes(10),
    )
    .unwrap();
    let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::KnownProvider {
        template_id: template.id.clone(),
    });
    draft.template = Some(template);
    draft.catalog_authority = Some(authority);

    let error = revalidate_discovery_catalog_authority(core.storage(), &draft, now)
        .expect_err("inactive signed template must not retain discovery authority");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
}

fn approved_lan_options() -> ProviderDiscoveryConnectionOptions {
    ProviderDiscoveryConnectionOptions {
        network_mode: ProviderNetworkMode::ApprovedLocalNetwork,
        local_network_approval: Some(ProviderLocalNetworkApproval {
            origin: CanonicalOrigin::parse("http://models.lan:8080").unwrap(),
            addresses: vec!["192.168.10.20".parse::<IpAddr>().unwrap()],
        }),
        local_network_approved_at: Some(Utc::now()),
        ..ProviderDiscoveryConnectionOptions::default()
    }
}

#[test]
fn supplemental_public_sources_may_use_another_public_origin() {
    let input = input_with_options(
        "https://console.example/",
        ProviderDiscoveryConnectionOptions::default(),
    );
    let docs_origin = CanonicalOrigin::parse("https://docs.example").unwrap();
    let api_origin = CanonicalOrigin::parse("https://api.example").unwrap();
    assert!(additional_document_url_policy(&input, &docs_origin).is_ok());
    assert!(additional_curl_url_policy(&input, &api_origin).is_ok());
}

#[test]
fn approved_lan_curl_is_exact_and_document_fetch_remains_disabled() {
    let options = approved_lan_options();
    let input = input_with_options("http://models.lan:8080/", options.clone());
    let approved_origin = CanonicalOrigin::parse("http://models.lan:8080").unwrap();
    let other_origin = CanonicalOrigin::parse("http://other.lan:8080").unwrap();

    assert!(additional_curl_url_policy(&input, &approved_origin).is_ok());
    assert!(additional_curl_url_policy(&input, &other_origin).is_err());
    assert!(additional_document_url_policy(&input, &approved_origin).is_err());

    assert!(
        ProviderDiscoverySource::curl(
            SecretCurlInput::new("curl http://models.lan:8080/v1/models".to_owned(),),
            options.clone(),
        )
        .is_ok()
    );
    assert!(
        ProviderDiscoverySource::curl(
            SecretCurlInput::new("curl http://other.lan:8080/v1/models".to_owned()),
            options,
        )
        .is_err()
    );
}

#[test]
fn legacy_or_expired_lan_authority_cannot_reach_a_network_policy() {
    let mut legacy = approved_lan_options();
    legacy.local_network_approved_at = None;
    assert!(legacy.validate().is_ok(), "legacy records remain readable");
    assert!(
        ProviderDiscoverySource::curl(
            SecretCurlInput::new("curl http://models.lan:8080/v1/models".to_owned()),
            legacy.clone(),
        )
        .is_ok(),
        "pre-session cURL parsing does not itself perform a network effect"
    );
    assert!(discovery_url_policy(&legacy).is_err());

    let mut expired = approved_lan_options();
    expired.local_network_approved_at = Some(Utc::now() - chrono::Duration::hours(25));
    assert!(discovery_url_policy(&expired).is_err());
}

#[test]
fn discovery_begin_issues_lan_authority_at_the_immutable_session_time() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let mut options = approved_lan_options();
    options.local_network_approved_at = Some(Utc::now() - chrono::Duration::hours(48));
    let snapshot = core
        .provider_discovery()
        .begin_with_credential_authority(
            input_with_options("http://models.lan:8080/", options),
            ProviderDiscoverySource::known_provider_id(ProviderTemplateId::from(
                "unknown-lan-template",
            )),
            None,
        )
        .expect("persist LAN discovery before local template lookup fails closed");

    assert_eq!(
        snapshot
            .session
            .input
            .connection_options
            .local_network_approved_at,
        Some(snapshot.created_at),
        "Core must overwrite caller time and bind LAN authority to session creation"
    );
}

#[test]
fn approved_lan_graph_seed_does_not_refresh_session_authority() {
    let approved_at = Utc::now() - chrono::Duration::hours(1);
    let observed_at = approved_at + chrono::Duration::minutes(30);
    let mut options = approved_lan_options();
    options.local_network_approved_at = Some(approved_at);
    let session = ProviderDiscoverySession::new(
        DiscoverySessionId::from("approved-lan-authority-time"),
        input_with_options("http://models.lan:8080/", options),
    )
    .expect("approved LAN discovery session");
    let snapshot = DiscoverySessionSnapshot {
        session,
        active_operation_id: None,
        draft_json: None,
        review: None,
        created_at: approved_at,
        updated_at: approved_at,
    };
    let mut template =
        AdapterRegistry::built_in_template(BuiltInTemplateId::OllamaNative).unwrap();
    template.default_manifest.default_api_origin =
        Some(CanonicalOrigin::parse("http://models.lan:8080").unwrap());
    let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::KnownProvider {
        template_id: template.id.clone(),
    });

    install_graph_seed(&snapshot, &mut draft, template, observed_at)
        .expect("install approved LAN graph seed");

    assert_eq!(
        draft.connection.expect("seeded connection").created_at,
        approved_at,
        "graph seeding must carry the immutable LAN approval issue time"
    );
}

#[test]
fn initial_discovery_preserves_exact_bounded_openrouter_model_metadata() {
    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
        .expect("OpenRouter template");
    let observed_at = Utc::now();
    let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::KnownProvider {
        template_id: template.id.clone(),
    });
    draft.connection = Some(ProviderConnection {
        id: ProviderConnectionId::from("openrouter-initial-discovery"),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        display_name: "OpenRouter initial discovery".to_owned(),
        api_origin: CanonicalOrigin::parse("https://openrouter.ai").expect("OpenRouter origin"),
        config: ConnectionConfig::default(),
        credential_ref: None,
        credential_scope: None,
        timeout_seconds: 30,
        status: ConnectionStatus::Untested,
        created_at: observed_at,
        updated_at: observed_at,
    });
    draft.template = Some(template);

    let listed = lorepia_providers::ListedModel {
        model_id: "openai/exact-metadata-model".to_owned(),
        display_name: Some("Exact metadata model".to_owned()),
        max_input_tokens: Some(128_000),
        max_output_tokens: Some(16_384),
        supported_generation_methods: Vec::new(),
        capabilities: lorepia_providers::ListedModelCapabilities {
            supported: vec![
                lorepia_providers::ListedModelCapability::Reasoning,
                lorepia_providers::ListedModelCapability::ToolCalling,
            ],
            parameters: lorepia_providers::OpenRouterSupportedParameterSupport::Exact(vec![
                lorepia_providers::OpenRouterSupportedParameter::MaxCompletionTokens,
                lorepia_providers::OpenRouterSupportedParameter::Reasoning,
                lorepia_providers::OpenRouterSupportedParameter::Temperature,
                lorepia_providers::OpenRouterSupportedParameter::Tools,
            ]),
            reasoning: Some(lorepia_providers::ListedModelReasoningCapability {
                supported_efforts: lorepia_providers::OpenRouterReasoningEffortSupport::Exact(
                    vec![
                        lorepia_providers::OpenRouterReasoningEffort::High,
                        lorepia_providers::OpenRouterReasoningEffort::Low,
                    ],
                ),
                default_effort: Some(lorepia_providers::OpenRouterReasoningEffort::High),
                default_enabled: Some(true),
                supports_max_tokens: Some(false),
                mandatory: Some(false),
            }),
        },
        source: lorepia_providers::ModelRecordSource::ProviderApi,
        availability: ModelAvailability::Available,
    };

    apply_listed_models_to_draft(&mut draft, &[listed], observed_at)
        .expect("apply initial provider listing");

    assert_eq!(
        draft.connection.as_ref().unwrap().status,
        ConnectionStatus::Connected
    );
    assert_eq!(draft.routes.len(), 1);
    let route = &draft.routes[0];
    assert_eq!(route.metadata_source, ModelMetadataSource::ProviderApi);
    assert_eq!(route.metadata_observed_at, Some(observed_at));
    let metadata = route
        .raw_metadata
        .as_ref()
        .expect("normalized provider metadata");
    let metadata: Value =
        serde_json::from_str(metadata.as_str()).expect("normalized metadata JSON");
    assert_eq!(metadata["capabilities"]["parameters"]["kind"], "exact");
    assert_eq!(
        metadata["capabilities"]["reasoning"]["supported_efforts"]["values"],
        json!(["high", "low"])
    );
    assert_eq!(
        metadata["capabilities"]["reasoning"]["default_effort"],
        "high"
    );
    assert!(
        draft.observations.iter().any(|observation| {
            observation.model_route_id == route.id
                && observation.key == lorepia_domain::CapabilityKey::Reasoning
                && observation.source == lorepia_domain::ObservationSource::ProviderApi
        }),
        "initial discovery must retain provider API capability provenance"
    );
    assert!(
        draft
            .observations
            .iter()
            .all(|observation| observation.key != lorepia_domain::CapabilityKey::PromptCaching),
        "OpenRouter model metadata must not infer prompt caching"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn openrouter_discovery_commit_and_reopen_preserves_exact_bounded_model_metadata() {
    let root = tempdir().expect("temporary Core root");
    let core = crate::Core::open(crate::CoreConfig::new(root.path())).expect("open Core");
    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
        .expect("OpenRouter template");

    let connection_id = ProviderConnectionId::from("openrouter-discovery-reopen");
    let input = SanitizedDiscoveryInput {
        connection_id: connection_id.clone(),
        display_name: "OpenRouter discovery reopen".to_owned(),
        site_url: HttpUrl::parse("https://openrouter.ai/").expect("OpenRouter site URL"),
        docs_url: None,
        credential_ref: Some(CredentialRef(connection_id.as_str().to_owned())),
        preferred_assistant: None,
        connection_options: ProviderDiscoveryConnectionOptions::default(),
        supplied_evidence_ids: Vec::new(),
    };
    let selecting = core
        .begin_provider_discovery_known(input, template.id.clone())
        .expect("begin exact OpenRouter discovery");
    assert_eq!(
        selecting.session.state,
        DiscoveryState::AwaitingTemplateSelection
    );
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
        .expect("select exact OpenRouter template");
    assert_eq!(
        selected.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval
    );
    let credential_proposal = core
        .get_provider_discovery_approval_proposal(&selected.session.id)
        .expect("load credential-origin proposal")
        .expect("credential-origin proposal");
    let listed = approve_credential_and_seed_model_listing(
        &core,
        &selected,
        credential_proposal.id,
        &[exact_openrouter_listed_model()],
    );
    assert_eq!(listed.session.state, DiscoveryState::AwaitingProbeConsent);
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
        .expect("skip OpenRouter probes");
    assert_eq!(reviewed.session.state, DiscoveryState::AwaitingReview);
    let proposal = core
        .get_provider_discovery_review_proposal(&reviewed.session.id)
        .expect("load review proposal")
        .expect("OpenRouter review proposal");
    let expected_attempt_id = proposal.commit_attempt_id.clone();
    let expected_plan_sha256 = proposal.commit_plan_sha256.clone();
    let committing = core
        .continue_provider_discovery(
            &reviewed.session.id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                reviewed.session.revision,
                ProviderDiscoveryAction::ApproveReview {
                    approval_id: proposal.approval.id,
                    commit_attempt_id: expected_attempt_id.clone(),
                    commit_plan_sha256: expected_plan_sha256.clone(),
                    graph_sha256: proposal.review.graph_sha256,
                },
            )
            .expect("approve-review action"),
            None,
        )
        .expect("approve OpenRouter review");
    assert_eq!(committing.session.state, DiscoveryState::Committing);
    let prepared = core
        .get_provider_discovery_credential_install_context(&committing.session.id)
        .expect("credential install context");
    assert_eq!(prepared.session_revision, committing.session.revision);
    assert_eq!(prepared.commit_attempt_id, expected_attempt_id);
    assert_eq!(prepared.commit_plan_sha256, expected_plan_sha256);
    assert_eq!(prepared.commit_phase, DiscoveryCommitPhase::Prepared);
    assert_eq!(
        prepared.operation_status,
        DiscoveryOperationStatus::Prepared
    );
    let started = reserve_and_start_credential_install(&core, &prepared);
    assert_eq!(started.operation_status, DiscoveryOperationStatus::Started);
    assert_eq!(started.commit_phase, DiscoveryCommitPhase::Prepared);
    let confirmation = credential_commit_confirmation(&started);
    core.commit_provider_discovery(&committing.session.id, Some(&confirmation))
        .expect("commit exact OpenRouter graph");
    drop(core);

    let reopened = open_core_after_drop(root.path(), crate::DiscoveryRecoveryOwner::Core);
    reopened
        .storage()
        .ensure_provider_credential_access_settled(&connection_id)
        .expect("reopened discovery credential authority remains settled");
    let routes = reopened
        .list_model_routes(&connection_id)
        .expect("list reopened OpenRouter routes");
    assert_eq!(routes.len(), 1);
    let route = &routes[0];
    assert_eq!(route.metadata_source, ModelMetadataSource::ProviderApi);
    assert!(route.metadata_observed_at.is_some());
    let raw_metadata = route
        .raw_metadata
        .as_ref()
        .expect("reopened normalized metadata");
    assert!(!raw_metadata.as_str().contains("future_model_metadata"));
    assert!(!raw_metadata.as_str().contains("future_reasoning_metadata"));
    assert!(!raw_metadata.as_str().contains("future-effort-v9"));
    let metadata: Value =
        serde_json::from_str(raw_metadata.as_str()).expect("reopened metadata JSON");
    assert_eq!(
        metadata["capabilities"]["parameters"],
        json!({
            "kind": "exact",
            "values": [
                "logprobs",
                "max_completion_tokens",
                "max_tokens",
                "parallel_tool_calls",
                "reasoning",
                "response_format",
                "seed",
                "structured_outputs",
                "temperature",
                "tools",
                "top_p"
            ]
        })
    );
    assert_eq!(
        metadata["capabilities"]["reasoning"],
        json!({
            "supported_efforts": {
                "kind": "exact",
                "values": ["high", "low"]
            },
            "default_effort": "high",
            "default_enabled": true,
            "supports_max_tokens": true,
            "mandatory": false
        })
    );
}
