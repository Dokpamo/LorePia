#[test]
fn legacy_provider_profile_keeps_endpoint_identity_but_can_select_a_new_model_route() {
    let root = tempdir().expect("temporary core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let original = ProviderProfile {
        id: format!("legacy-{}", Uuid::new_v4()),
        display_name: "Legacy original".to_owned(),
        base_url: "http://127.0.0.1:65534/v1".to_owned(),
        model: "model-one".to_owned(),
        timeout_seconds: 30,
    };
    core.upsert_provider_profile(original.clone())
        .expect("create legacy provider");
    let connection_id = ProviderConnectionId::from(original.id.as_str());
    let original_route = core
        .list_model_routes(&connection_id)
        .expect("original routes")
        .into_iter()
        .find(|route| route.model_id == "model-one")
        .expect("original model route");

    let safe_update = ProviderProfile {
        display_name: "Legacy renamed".to_owned(),
        model: "model-two".to_owned(),
        timeout_seconds: 45,
        ..original.clone()
    };
    core.upsert_provider_profile(safe_update.clone())
        .expect("display, timeout, and selected model may change");
    let routes = core
        .list_model_routes(&connection_id)
        .expect("preserved legacy routes");
    let new_route = routes
        .iter()
        .find(|route| route.model_id == "model-two")
        .expect("new model route");
    assert_ne!(new_route.id, original_route.id);
    assert!(routes.iter().any(|route| route.id == original_route.id));

    let mut endpoint_rebind = safe_update.clone();
    endpoint_rebind.base_url = "http://127.0.0.1:65534/v2".to_owned();
    let error = core
        .upsert_provider_profile(endpoint_rebind)
        .expect_err("legacy endpoint mutation must require a new provider ID");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        error
            .message
            .contains("endpoint configuration is immutable")
    );
    assert_eq!(
        core.inner
            .storage
            .get_provider_profile(&safe_update.id)
            .expect("unchanged legacy profile"),
        safe_update
    );
    assert_eq!(
        core.inner
            .storage
            .get_provider_connection(&connection_id)
            .expect("unchanged legacy connection")
            .config
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/v1")
    );
}

struct RetainedLegacyCrudFixture {
    _root: TempDir,
    core: Core,
    connection_id: ProviderConnectionId,
    route_id: ModelRouteId,
    preset_id: GenerationPresetId,
    routes_before: Vec<ModelRoute>,
    presets_before: Vec<GenerationPreset>,
    cleared_settings: AppSettings,
}

fn retained_legacy_crud_fixture() -> RetainedLegacyCrudFixture {
    let root = tempdir().expect("temporary core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let original = ProviderProfile {
        id: format!("legacy-{}", Uuid::new_v4()),
        display_name: "Legacy protected target".to_owned(),
        base_url: "http://127.0.0.1:65534/v1".to_owned(),
        model: "model-one".to_owned(),
        timeout_seconds: 30,
    };
    core.upsert_provider_profile(original.clone())
        .expect("create legacy provider");
    let mut settings = core.get_settings().expect("initial settings");
    settings.selected_provider_profile_id = Some(original.id.clone());
    core.update_settings(&settings)
        .expect("select the retained legacy profile");

    core.upsert_provider_profile(ProviderProfile {
        model: "model-two".to_owned(),
        ..original.clone()
    })
    .expect("move the active legacy profile to a sibling route");
    let normalized = core.get_settings().expect("normalized legacy selection");
    let route_id = normalized
        .selected_model_route_id
        .clone()
        .expect("selected legacy route");
    let preset_id = normalized
        .selected_generation_preset_id
        .clone()
        .expect("selected legacy preset");
    assert_ne!(route_id.as_str(), original.id);
    assert_eq!(preset_id.as_str(), route_id.as_str());
    let connection_id = ProviderConnectionId::from(original.id.as_str());
    let routes_before = core
        .list_model_routes(&connection_id)
        .expect("legacy routes before rejected deletes");
    let presets_before = core
        .list_generation_presets(&route_id)
        .expect("legacy presets before rejected deletes");
    let selected = core
        .select_generation_target(None)
        .expect("clear the legacy selection without archiving its profile");
    assert!(selected.selected_provider_profile_id.is_none());

    RetainedLegacyCrudFixture {
        _root: root,
        core,
        connection_id,
        route_id,
        preset_id,
        routes_before,
        presets_before,
        cleared_settings: selected,
    }
}

fn assert_retained_legacy_fixture_unchanged(fixture: &RetainedLegacyCrudFixture) {
    assert_eq!(
        fixture
            .core
            .get_settings()
            .expect("settings after rejected operation"),
        fixture.cleared_settings
    );
    assert_eq!(
        fixture
            .core
            .list_model_routes(&fixture.connection_id)
            .expect("routes after rejected operation"),
        fixture.routes_before
    );
    assert_eq!(
        fixture
            .core
            .list_generation_presets(&fixture.route_id)
            .expect("presets after rejected operation"),
        fixture.presets_before
    );
}

#[test]
fn retained_legacy_profile_current_sibling_deletes_are_rejected() {
    let fixture = retained_legacy_crud_fixture();

    let route_error = fixture
        .core
        .delete_model_route(&fixture.route_id)
        .expect_err("the active legacy sibling route must be protected");
    assert_eq!(route_error.code, CoreErrorCode::InvalidInput);
    assert_retained_legacy_fixture_unchanged(&fixture);

    let preset_error = fixture
        .core
        .delete_generation_preset(&fixture.preset_id)
        .expect_err("the active legacy sibling preset must be protected");
    assert_eq!(preset_error.code, CoreErrorCode::InvalidInput);
    assert_retained_legacy_fixture_unchanged(&fixture);
}

#[test]
fn retained_legacy_profile_rejects_ordinary_route_and_preset_upserts() {
    let fixture = retained_legacy_crud_fixture();

    let mut route_update = fixture
        .routes_before
        .iter()
        .find(|route| route.id == fixture.route_id)
        .expect("current legacy route")
        .clone();
    route_update.display_name = Some("ordinary mutation".to_owned());
    let route_update_error = fixture
        .core
        .upsert_model_route(route_update)
        .expect_err("ordinary route mutation must not alter the legacy graph");
    assert_eq!(route_update_error.code, CoreErrorCode::InvalidInput);

    let mut extra_route = fixture
        .routes_before
        .iter()
        .find(|route| route.id == fixture.route_id)
        .expect("current legacy route")
        .clone();
    extra_route.id = ModelRouteId::from(format!("ordinary-{}", Uuid::new_v4()));
    extra_route.model_id = "ordinary-extra-model".to_owned();
    extra_route.display_name = Some("Ordinary extra route".to_owned());
    extra_route.metadata_source = ModelMetadataSource::UserOverride;
    let route_create_error = fixture
        .core
        .upsert_model_route(extra_route)
        .expect_err("ordinary route creation must not extend the legacy graph");
    assert_eq!(route_create_error.code, CoreErrorCode::InvalidInput);

    let mut extra_preset = fixture
        .presets_before
        .first()
        .expect("current legacy preset")
        .clone();
    extra_preset.id = GenerationPresetId::from(format!("ordinary-{}", Uuid::new_v4()));
    extra_preset.display_name = "Ordinary extra preset".to_owned();
    let preset_create_error = fixture
        .core
        .upsert_generation_preset(extra_preset)
        .expect_err("ordinary preset creation must not extend the legacy graph");
    assert_eq!(preset_create_error.code, CoreErrorCode::InvalidInput);
    assert_retained_legacy_fixture_unchanged(&fixture);
}

#[test]
fn active_legacy_profile_reselection_preserves_its_sibling_target_family() {
    let root = tempdir().expect("temporary core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let original = ProviderProfile {
        id: format!("legacy-{}", Uuid::new_v4()),
        display_name: "Legacy reselection".to_owned(),
        base_url: "http://127.0.0.1:65534/v1".to_owned(),
        model: "model-one".to_owned(),
        timeout_seconds: 30,
    };
    core.upsert_provider_profile(original.clone())
        .expect("create legacy provider");
    let connection_id = ProviderConnectionId::from(original.id.as_str());
    let original_route = core
        .list_model_routes(&connection_id)
        .expect("original legacy routes")
        .into_iter()
        .find(|route| route.model_id == original.model)
        .expect("original legacy route");
    let original_target = GenerationTarget {
        model_route_id: original_route.id.clone(),
        generation_preset_id: GenerationPresetId::from(original_route.id.as_str()),
    };
    let mut settings = core.get_settings().expect("initial settings");
    settings.selected_provider_profile_id = Some(original.id.clone());
    core.update_settings(&settings)
        .expect("select the retained legacy profile");
    core.upsert_provider_profile(ProviderProfile {
        model: "model-two".to_owned(),
        ..original.clone()
    })
    .expect("move the active legacy profile to a sibling route");
    let selected = core.get_settings().expect("normalized legacy selection");
    let current_target = GenerationTarget {
        model_route_id: selected
            .selected_model_route_id
            .clone()
            .expect("selected legacy route"),
        generation_preset_id: selected
            .selected_generation_preset_id
            .clone()
            .expect("selected legacy preset"),
    };
    let cleared = core
        .select_generation_target(None)
        .expect("clear the legacy selection before generic reselection");
    assert!(cleared.selected_provider_profile_id.is_none());

    let reselected = core
        .select_generation_target(Some(current_target))
        .expect("reselect the exact normalized legacy target");
    assert_eq!(
        reselected.selected_provider_profile_id.as_deref(),
        Some(original.id.as_str())
    );

    let error = core
        .select_generation_target(Some(original_target))
        .expect_err("a retained sibling cannot replace the active legacy target");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        core.get_settings().expect("selection after rejection"),
        reselected
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the reopen scenario keeps one connection fixture and its durable assertions linear"
)]
fn approved_lan_connection_reopens_and_drives_preview_and_generation_validation() {
    let root = tempdir().expect("temporary core root");
    let connection_id = ProviderConnectionId::from("approved-lan-core");
    let route_id = ModelRouteId::from("approved-lan-route");
    let preset_id = GenerationPresetId::from("approved-lan-preset");
    {
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let template = core
            .list_provider_templates()
            .expect("provider templates")
            .into_iter()
            .find(|template| template.id.as_str() == "ollama-native-v1")
            .expect("Ollama template");
        let api_origin = CanonicalOrigin::parse("http://ollama.lan:11434").expect("LAN origin");
        let connection = core
            .create_provider_connection(ProviderConnectionDraft {
                id: connection_id.clone(),
                template_id: template.id.clone(),
                template_version: template.manifest_version,
                display_name: "Approved LAN Ollama".to_owned(),
                api_origin: api_origin.clone(),
                api_base_path: Some(EndpointPath::parse("/api").expect("API base path")),
                network_mode: ProviderNetworkMode::ApprovedLocalNetwork,
                local_network_approval: Some(ProviderLocalNetworkApproval {
                    origin: api_origin,
                    addresses: vec![
                        "192.168.10.21".parse().expect("LAN address"),
                        "192.168.10.20".parse().expect("LAN address"),
                        "192.168.10.21".parse().expect("duplicate LAN address"),
                    ],
                }),
                values: Vec::new(),
                approved_credential_origin: None,
                timeout_seconds: 5,
            })
            .expect("create approved LAN connection");
        assert_eq!(
            connection
                .config
                .local_network_approval
                .as_ref()
                .expect("normalized LAN approval")
                .addresses,
            vec![
                "192.168.10.20".parse::<IpAddr>().expect("LAN address"),
                "192.168.10.21".parse::<IpAddr>().expect("LAN address"),
            ]
        );
        assert_eq!(
            core.list_provider_connections()
                .expect("provider connections")
                .into_iter()
                .find(|candidate| candidate.id == connection_id)
                .expect("approved LAN connection"),
            connection
        );
        let now = Utc::now();
        core.upsert_model_route(ModelRoute {
            id: route_id.clone(),
            connection_id: connection_id.clone(),
            api_family: template.api_family,
            model_id: "llama-lan".to_owned(),
            display_name: Some("LAN Llama".to_owned()),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::Legacy,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: now,
            last_seen_at: Some(now),
        })
        .expect("save LAN model route");
        core.upsert_generation_preset(GenerationPreset {
            id: preset_id.clone(),
            model_route_id: route_id.clone(),
            display_name: "LAN defaults".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings {
                preserve_opaque_state: false,
                ..GenerationReasoningSettings::default()
            },
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: now,
            updated_at: now,
        })
        .expect("save LAN generation preset");
        core.preview_provider_request(&route_id, &preset_id)
            .expect("preview reconstructs persisted LAN policy");
        core.validate_generation_preset(&route_id, &preset_id)
            .expect("generation validation reconstructs persisted LAN policy");
    }
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    let connection = reopened
        .list_provider_connections()
        .expect("reopened provider connections")
        .into_iter()
        .find(|candidate| candidate.id == connection_id)
        .expect("reopened approved LAN connection");
    assert_eq!(
        connection.config.network_mode,
        ProviderNetworkMode::ApprovedLocalNetwork
    );
    reopened
        .preview_provider_request(&route_id, &preset_id)
        .expect("reopened preview reconstructs persisted LAN policy");
    reopened
        .validate_generation_preset(&route_id, &preset_id)
        .expect("reopened generation validation reconstructs persisted LAN policy");
}
