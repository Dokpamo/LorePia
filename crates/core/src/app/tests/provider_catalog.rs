#[test]
fn health_reports_storage_state() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let expected_schema_version = core
        .inner
        .storage
        .schema_version()
        .expect("storage schema version");
    let health = core.health_check().expect("health");
    assert!(health.database_open);
    assert!(health.data_root_writable);
    assert_eq!(health.schema_version, expected_schema_version);
}

#[test]
fn provider_template_listing_exposes_only_each_latest_manifest_version() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let built_in = core
        .list_provider_templates()
        .expect("built-in provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
        .expect("OpenAI-compatible template");
    assert_eq!(built_in.manifest_version, 3);

    let mut version_one = built_in.clone();
    version_one.id = "synthetic-template-history".into();
    version_one.display_name = "Synthetic template history".to_owned();
    version_one.manifest_version = 1;
    let mut version_two = version_one.clone();
    version_two.manifest_version = 2;
    core.inner
        .storage
        .save_provider_template(&version_one)
        .expect("save historical template");
    core.inner
        .storage
        .save_provider_template(&version_two)
        .expect("save latest template");

    let stored_versions = core
        .inner
        .storage
        .list_provider_templates()
        .expect("stored template history")
        .into_iter()
        .filter(|template| template.id == version_one.id)
        .map(|template| template.manifest_version)
        .collect::<Vec<_>>();
    assert_eq!(stored_versions, vec![2, 1]);

    let exposed = core
        .list_provider_templates()
        .expect("latest provider templates")
        .into_iter()
        .filter(|template| template.id == version_one.id)
        .collect::<Vec<_>>();
    assert_eq!(exposed.len(), 1);
    assert_eq!(exposed[0].manifest_version, 2);
}

#[test]
fn ollama_template_view_creates_a_loopback_connection_without_native_inference() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let ollama = core
        .list_provider_template_views()
        .expect("provider template views")
        .into_iter()
        .find(|view| view.template.id.as_str() == "ollama-native-v1")
        .expect("Ollama template view");
    assert_eq!(
        ollama.default_network_mode,
        ProviderNetworkMode::LocalLoopback
    );
    let api_origin = ollama
        .template
        .default_manifest
        .default_api_origin
        .clone()
        .expect("Ollama default origin");

    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from("ollama-create-regression"),
            template_id: ollama.template.id,
            template_version: ollama.template.manifest_version,
            display_name: "Local Ollama".to_owned(),
            api_origin,
            api_base_path: Some(EndpointPath::parse("/api").expect("Ollama base path")),
            network_mode: ollama.default_network_mode,
            values: Vec::new(),
            approved_credential_origin: None,
            local_network_approval: None,
            timeout_seconds: 30,
        })
        .expect("create Ollama loopback connection");
    assert_eq!(
        connection.config.network_mode,
        ProviderNetworkMode::LocalLoopback
    );
    assert_eq!(connection.api_origin.as_str(), "http://localhost:11434");
    assert!(connection.credential_ref.is_none());
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one vertical proves generic archive blocking, visibility, and identifier reuse"
)]
fn archived_provider_is_hidden_and_rejected_by_generation_and_model_sync() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let template = core
        .list_provider_templates()
        .expect("provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "ollama-native-v1")
        .expect("credentialless Ollama template");
    let api_origin = template
        .default_manifest
        .default_api_origin
        .clone()
        .expect("Ollama default origin");
    let draft = ProviderConnectionDraft {
        id: ProviderConnectionId::from("archived-core-provider"),
        template_id: template.id.clone(),
        template_version: template.manifest_version,
        display_name: "Archived Core provider".to_owned(),
        api_origin,
        api_base_path: Some(EndpointPath::parse("/api").expect("Ollama API base path")),
        network_mode: ProviderNetworkMode::LocalLoopback,
        values: Vec::new(),
        approved_credential_origin: None,
        local_network_approval: None,
        timeout_seconds: 30,
    };
    let connection = core
        .create_provider_connection(draft.clone())
        .expect("create credentialless provider");
    let connection_id = connection.id.clone();
    let now = Utc::now();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from("archived-core-provider-route"),
            connection_id: connection_id.clone(),
            api_family: template.api_family,
            model_id: "historical-model".to_owned(),
            display_name: Some("Historical model".to_owned()),
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
        .expect("save active route");
    let preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
        .expect("save active preset");
    core.validate_generation_preset(&route.id, &preset.id)
        .expect("active target");

    let unfinished_sync = core
        .inner
        .storage
        .create_model_sync_job(&connection)
        .expect("create durable model sync");
    let archive_error = core
        .delete_provider_connection(&connection_id)
        .expect_err("unfinished model sync must block Core archive");
    assert_eq!(archive_error.code, CoreErrorCode::InvalidInput);
    assert!(archive_error.recoverable);
    assert_eq!(
        archive_error.message,
        "provider connection cannot be archived while model synchronization is unfinished"
    );
    assert_eq!(
        core.list_provider_connections()
            .expect("active connections after rejected archive"),
        vec![connection]
    );
    core.cancel_provider_model_sync(&unfinished_sync.id)
        .expect("cancel durable model sync");
    core.delete_provider_connection(&connection_id)
        .expect("archive provider");
    assert!(
        core.list_provider_connections()
            .expect("active connections")
            .is_empty()
    );
    assert_eq!(
        core.inner
            .storage
            .get_provider_connection(&connection_id)
            .expect_err("archived provider is hidden")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        core.validate_generation_preset(&route.id, &preset.id)
            .expect_err("archived provider cannot generate")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        core.start_provider_model_sync(&connection_id, None)
            .expect_err("archived provider cannot synchronize")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        core.create_provider_connection(draft)
            .expect_err("archived provider id cannot be reused")
            .code,
        CoreErrorCode::InvalidInput
    );
}

#[test]
fn provider_model_refresh_lists_routes_with_non_secret_provenance() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let body = r#"{"data":[{"id":"zeta-model"},{"id":"alpha-model"}]}"#.to_owned();
    let response_bytes = u64::try_from(body.len()).expect("response size");
    let (api_origin, requests) = spawn_model_list_provider(vec![body]);
    let (template, connection) = create_openai_chat_connection(&core, &api_origin);
    let secret = "model-refresh-listing-key";

    let result = refresh_models_with_review(&core, &connection.id, Some(secret))
        .expect("refresh provider models");

    let request = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured model-list request");
    let request = request.to_ascii_lowercase();
    assert!(request.starts_with("get /v1/models http/1.1\r\n"));
    assert!(request.contains("authorization: bearer model-refresh-listing-key\r\n"));
    assert_eq!(result.connection_id, connection.id);
    assert_eq!(result.pages_fetched, 1);
    assert_eq!(result.response_bytes, response_bytes);
    assert_eq!(result.provenance.source, "provider_api");
    assert_eq!(result.provenance.api_family, template.api_family);
    assert_eq!(result.provenance.api_origin, api_origin);
    assert_eq!(result.provenance.endpoint_path.as_str(), "/v1/models");
    assert_eq!(
        result
            .model_routes
            .iter()
            .map(|route| route.model_id.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha-model", "zeta-model"]
    );
    assert!(result.model_routes.iter().all(|route| {
        route.status == ModelAvailability::Available
            && route.api_family == template.api_family
            && route.connection_id == connection.id
    }));
    assert_eq!(result.newly_seen_model_route_ids.len(), 2);
    assert_eq!(result.created_generation_preset_ids.len(), 2);
    assert!(result.routes_requiring_preset_configuration.is_empty());
    for route in &result.model_routes {
        let expected_id =
            deterministic_model_route_id(&connection.id, template.api_family, &route.model_id);
        assert_eq!(route.id, expected_id);
        let presets = core
            .list_generation_presets(&route.id)
            .expect("initial preset");
        assert_eq!(presets.len(), 1);
        assert!(presets[0].values.is_empty());
    }
    assert_eq!(
        core.inner
            .storage
            .get_provider_connection(&connection.id)
            .expect("refreshed connection")
            .status,
        ConnectionStatus::Connected
    );
    assert!(!format!("{result:?}").contains(secret));
}

#[test]
fn provider_model_token_limits_become_bounded_route_observations() {
    let observed_at = Utc::now();
    let route = ModelRoute {
        id: ModelRouteId::from("token-route"),
        connection_id: ProviderConnectionId::from("token-connection"),
        api_family: ApiFamily::GeminiGenerateContent,
        model_id: "models/token-model".to_owned(),
        display_name: None,
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::Legacy,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: observed_at,
        last_seen_at: Some(observed_at),
    };
    let listed = ListedModel {
        model_id: route.model_id.clone(),
        display_name: None,
        max_input_tokens: Some(1_000_000),
        max_output_tokens: Some(65_536),
        supported_generation_methods: vec!["generateContent".to_owned()],
        capabilities: lorepia_providers::ListedModelCapabilities::default(),
        source: ModelRecordSource::ProviderApi,
        availability: ModelAvailability::Available,
    };
    let observations =
        provider_api_capability_observations(std::slice::from_ref(&route), &[listed], observed_at)
            .expect("provider API observations");
    assert_eq!(observations.len(), 2);
    assert!(observations.iter().all(|observation| {
        observation.model_route_id == route.id
            && observation.source == ObservationSource::ProviderApi
            && observation.status == SupportStatus::Verified
            && observation.confidence == Confidence::High
            && observation.expires_at == Some(observed_at + PROVIDER_API_CAPABILITY_FRESHNESS)
    }));
    assert_eq!(
        observations
            .iter()
            .find(|observation| observation.key == CapabilityKey::ContextWindow)
            .map(|observation| &observation.value),
        Some(&CapabilityValue::Integer(1_000_000))
    );
    assert_eq!(
        observations
            .iter()
            .find(|observation| observation.key == CapabilityKey::MaxOutputTokens)
            .map(|observation| &observation.value),
        Some(&CapabilityValue::Integer(65_536))
    );
    assert_eq!(
        provider_api_capability_observations(
            &[route],
            &[ListedModel {
                model_id: "models/token-model".to_owned(),
                display_name: None,
                max_input_tokens: Some(0),
                max_output_tokens: None,
                supported_generation_methods: Vec::new(),
                capabilities: lorepia_providers::ListedModelCapabilities::default(),
                source: ModelRecordSource::ProviderApi,
                availability: ModelAvailability::Available,
            }],
            observed_at,
        )
        .expect_err("zero token limits must fail closed")
        .code,
        CoreErrorCode::ProviderUnavailable
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one contract-matrix regression covers source, freshness, alias, and bound interactions"
)]
fn openrouter_parameter_specs_intersect_exact_metadata_and_fail_closed_by_source() {
    let template = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter)
        .expect("OpenRouter template");
    let now = Utc::now();
    let model = listed_openrouter_model(
        "openai/exact-parameter-model",
        vec![
            OpenRouterSupportedParameter::FrequencyPenalty,
            OpenRouterSupportedParameter::Logprobs,
            OpenRouterSupportedParameter::MaxCompletionTokens,
            OpenRouterSupportedParameter::MaxTokens,
            OpenRouterSupportedParameter::ParallelToolCalls,
            OpenRouterSupportedParameter::Stop,
            OpenRouterSupportedParameter::Temperature,
            OpenRouterSupportedParameter::ToolChoice,
            OpenRouterSupportedParameter::Tools,
        ],
        None,
        Some(8_192),
    );
    let mut route = provider_api_openrouter_route(
        ProviderConnectionId::from("openrouter-parameter-connection"),
        &model,
        now,
    );
    let mut base = template.default_manifest.parameters.clone();
    base.push(compiled_openrouter_parameter_spec(
        "alternate_output",
        "max_completion_tokens",
        ParameterType::Integer,
        Some(1.0),
        Some(16_384.0),
        Some(1.0),
        UiParameterLevel::Basic,
    ));
    base.push(compiled_openrouter_parameter_spec(
        "logprobs",
        "logprobs",
        ParameterType::Boolean,
        None,
        None,
        None,
        UiParameterLevel::Advanced,
    ));
    base.push(compiled_openrouter_parameter_spec(
        "parallel_tool_calls",
        "parallel_tool_calls",
        ParameterType::Boolean,
        None,
        None,
        None,
        UiParameterLevel::Advanced,
    ));
    base.push(compiled_openrouter_parameter_spec(
        "tool_choice",
        "tool_choice",
        ParameterType::ToolPolicy,
        None,
        None,
        None,
        UiParameterLevel::Advanced,
    ));
    let specs = effective_route_parameter_specs(&route, &template, &base, &[], now)
        .expect("fresh exact parameter specs");
    let ids = specs
        .iter()
        .map(|spec| spec.id.as_str())
        .collect::<Vec<_>>();
    assert!(ids.contains(&"temperature"));
    assert!(ids.contains(&"frequency_penalty"));
    assert!(ids.contains(&"stop"));
    assert!(!ids.contains(&"top_p"));
    assert!(!ids.contains(&"logprobs"));
    assert!(!ids.contains(&"parallel_tool_calls"));
    assert!(!ids.contains(&"tool_choice"));
    let output = specs
        .iter()
        .find(|spec| spec.id.as_str() == "max_output_tokens")
        .expect("stable output-token control");
    assert_eq!(output.provider_mapping.field_name, "max_completion_tokens");
    assert_eq!(output.maximum, Some(8_192.0));
    assert_eq!(
        specs
            .iter()
            .filter(|spec| {
                matches!(
                    spec.provider_mapping.field_name.as_str(),
                    "max_tokens" | "max_completion_tokens"
                )
            })
            .count(),
        1
    );
    for (parameters, expected_field) in [
        (
            vec![OpenRouterSupportedParameter::MaxTokens],
            Some("max_tokens"),
        ),
        (
            vec![OpenRouterSupportedParameter::MaxCompletionTokens],
            Some("max_completion_tokens"),
        ),
        (
            vec![
                OpenRouterSupportedParameter::MaxTokens,
                OpenRouterSupportedParameter::MaxCompletionTokens,
            ],
            Some("max_completion_tokens"),
        ),
        (Vec::new(), None),
    ] {
        let alias_model =
            listed_openrouter_model("openai/alias-model", parameters, None, Some(u64::MAX));
        let alias_route = provider_api_openrouter_route(
            ProviderConnectionId::from("openrouter-alias-connection"),
            &alias_model,
            now,
        );
        let alias_specs = effective_route_parameter_specs(
            &alias_route,
            &template,
            &template.default_manifest.parameters,
            &[],
            now,
        )
        .expect("alias parameter contract");
        let output = alias_specs
            .iter()
            .find(|spec| spec.id.as_str() == "max_output_tokens");
        assert_eq!(
            output.map(|spec| spec.provider_mapping.field_name.as_str()),
            expected_field
        );
        if let Some(output) = output {
            assert_eq!(output.maximum, Some(f64::from(u32::MAX)));
        }
    }
    let no_numeric_cap = listed_openrouter_model(
        "openai/no-numeric-cap",
        vec![OpenRouterSupportedParameter::MaxTokens],
        None,
        None,
    );
    let no_numeric_route = provider_api_openrouter_route(
        ProviderConnectionId::from("openrouter-no-numeric-cap"),
        &no_numeric_cap,
        now,
    );
    let no_numeric_specs = effective_route_parameter_specs(
        &no_numeric_route,
        &template,
        &template.default_manifest.parameters,
        &[],
        now,
    )
    .expect("missing numeric cap retains the local safe ceiling");
    assert_eq!(
        no_numeric_specs
            .iter()
            .find(|spec| spec.id.as_str() == "max_output_tokens")
            .expect("output control without provider numeric cap")
            .maximum,
        Some(f64::from(u32::MAX))
    );

    route.metadata_observed_at = Some(now - chrono::Duration::hours(25));
    assert!(
        effective_route_parameter_specs(&route, &template, &base, &[], now)
            .expect("stale bundled-only contract")
            .is_empty()
    );
    let signed_max_tokens = compiled_openrouter_parameter_spec(
        "signed_output",
        "max_tokens",
        ParameterType::Integer,
        Some(1.0),
        None,
        Some(1.0),
        UiParameterLevel::Basic,
    );
    let mut signed_max_completion = signed_max_tokens.clone();
    signed_max_completion.id = ParameterId::from("signed_completion");
    signed_max_completion.provider_mapping.field_name = "max_completion_tokens".to_owned();
    signed_max_completion.maximum = Some(12_345.0);
    let signed_unsafe = compiled_openrouter_parameter_spec(
        "signed_logprobs",
        "logprobs",
        ParameterType::Boolean,
        None,
        None,
        None,
        UiParameterLevel::Advanced,
    );
    let signed_parallel = compiled_openrouter_parameter_spec(
        "signed_parallel_tool_calls",
        "parallel_tool_calls",
        ParameterType::Boolean,
        None,
        None,
        None,
        UiParameterLevel::Advanced,
    );
    let signed_tool_choice = compiled_openrouter_parameter_spec(
        "signed_tool_choice",
        "tool_choice",
        ParameterType::ToolPolicy,
        None,
        None,
        None,
        UiParameterLevel::Advanced,
    );
    let signed = openrouter_safe_signed_parameter_specs(&[
        signed_max_tokens,
        signed_max_completion,
        signed_unsafe,
        signed_parallel,
        signed_tool_choice,
    ]);
    assert_eq!(signed.len(), 1);
    assert_eq!(signed[0].id.as_str(), "max_output_tokens");
    assert_eq!(
        signed[0].provider_mapping.field_name,
        "max_completion_tokens"
    );
    assert_eq!(signed[0].maximum, Some(12_345.0));
    assert_eq!(
        effective_route_parameter_specs(&route, &template, &base, &signed, now)
            .expect("fresh signed fallback"),
        signed
    );
    let canonical_raw = route.raw_metadata.clone();
    route.raw_metadata = Some(
        BoundedJson::from_value(&serde_json::json!({"malformed": true}))
            .expect("bounded malformed metadata fixture"),
    );
    assert_eq!(
        effective_route_parameter_specs(&route, &template, &base, &signed, now)
            .expect_err("stale malformed ProviderApi metadata cannot use signed fallback")
            .code,
        CoreErrorCode::StorageCorrupted
    );
    route.raw_metadata = canonical_raw;

    route.status = ModelAvailability::MissingTemporarily;
    assert!(
        effective_route_parameter_specs(&route, &template, &base, &signed, now)
            .expect("unavailable routes remain nonactionable")
            .is_empty()
    );
    route.status = ModelAvailability::Available;
    route.metadata_observed_at = Some(now);
    route.raw_metadata = None;
    assert_eq!(
        effective_route_parameter_specs(&route, &template, &base, &signed, now)
            .expect_err("fresh ProviderApi provenance without metadata is corrupt")
            .code,
        CoreErrorCode::StorageCorrupted
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the test keeps raw model metadata, its observation, UI, and request wire in one atomic matrix"
)]
fn openrouter_reasoning_requires_matching_fresh_raw_metadata_and_uses_exact_wire_style() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let (template, mut route) =
        create_built_in_public_route(&core, "openrouter-v1", "/api/v1", "openai/reasoning");
    let now = Utc::now();
    let reasoning = ListedModelReasoningCapability {
        supported_efforts: OpenRouterReasoningEffortSupport::Exact(vec![
            lorepia_providers::OpenRouterReasoningEffort::High,
        ]),
        default_effort: Some(lorepia_providers::OpenRouterReasoningEffort::High),
        default_enabled: Some(true),
        supports_max_tokens: Some(true),
        mandatory: Some(false),
    };
    let model = listed_openrouter_model(
        &route.model_id,
        vec![
            OpenRouterSupportedParameter::MaxCompletionTokens,
            OpenRouterSupportedParameter::Reasoning,
            OpenRouterSupportedParameter::ReasoningEffort,
            OpenRouterSupportedParameter::Temperature,
        ],
        Some(reasoning),
        Some(4_096),
    );
    route.raw_metadata = Some(listed_model_metadata(&model).expect("normalized metadata"));
    route.metadata_source = ModelMetadataSource::ProviderApi;
    route.metadata_observed_at = Some(now);
    route.last_seen_at = Some(now);
    core.inner
        .storage
        .save_model_route(&route)
        .expect("save trusted route fixture");
    let observations = provider_api_capability_observations(
        std::slice::from_ref(&route),
        std::slice::from_ref(&model),
        now,
    )
    .expect("provider observations");
    core.record_provider_api_capability_observations(observations)
        .expect("persist provider observations");

    let mut preset = initial_generation_preset(&route.id, &template, now);
    preset.reasoning.mode = GenerationReasoningMode::Enabled;
    let rendered = core
        .render_reasoning_control_for_preset(&preset)
        .expect("render default-effort adoption");
    assert_eq!(
        rendered.settings.effort,
        Some(lorepia_providers::parameter_mapping::ReasoningEffort::High)
    );
    assert_eq!(
        core.validate_generation_preset_candidate(&preset)
            .expect_err("render-only default must not become an implicit request")
            .code,
        CoreErrorCode::InvalidInput
    );

    preset.reasoning.effort = Some(GenerationReasoningEffort::High);
    preset.values = vec![lorepia_domain::ParameterValue {
        parameter_id: ParameterId::from("max_output_tokens"),
        state: lorepia_domain::ParameterValueState::Explicit(
            lorepia_domain::ParameterLiteral::Integer(2_048),
        ),
    }];
    let preview = core
        .preview_provider_request_candidate(&preset)
        .expect("preview unified OpenRouter request");
    let lorepia_providers::RequestBodyShape::Object { fields, .. } =
        preview.body().expect("preview body")
    else {
        panic!("OpenRouter preview body must be an object");
    };
    assert!(fields.iter().any(|field| {
        field.name() == "max_completion_tokens"
            && field.shape() == &lorepia_providers::RequestBodyShape::Number
    }));
    assert!(
        fields
            .iter()
            .all(|field| field.name() != "max_tokens" && field.name() != "reasoning_effort")
    );
    let reasoning = fields
        .iter()
        .find(|field| field.name() == "reasoning")
        .expect("nested reasoning field");
    let lorepia_providers::RequestBodyShape::Object {
        fields: reasoning_fields,
        ..
    } = reasoning.shape()
    else {
        panic!("reasoning preview shape must be an object");
    };
    assert!(
        reasoning_fields
            .iter()
            .any(|field| field.name() == "effort")
    );

    route.metadata_observed_at = Some(now - chrono::Duration::hours(25));
    core.inner
        .storage
        .save_model_route(&route)
        .expect("make raw metadata stale");
    assert_eq!(
        core.render_reasoning_control_for_preset(&preset)
            .expect("stale control renders hidden")
            .state,
        lorepia_providers::parameter_mapping::UiControlState::Hidden
    );
    assert_eq!(
        core.validate_generation_preset_candidate(&preset)
            .expect_err("stale raw metadata cannot drive reasoning")
            .code,
        CoreErrorCode::InvalidInput
    );

    route.metadata_observed_at = Some(now);
    let legacy_model = listed_openrouter_model(
        &route.model_id,
        vec![OpenRouterSupportedParameter::ReasoningEffort],
        Some(ListedModelReasoningCapability {
            supported_efforts: OpenRouterReasoningEffortSupport::AllGateway,
            default_effort: None,
            default_enabled: None,
            supports_max_tokens: None,
            mandatory: Some(false),
        }),
        None,
    );
    route.raw_metadata = Some(listed_model_metadata(&legacy_model).expect("legacy raw metadata"));
    core.inner
        .storage
        .save_model_route(&route)
        .expect("save mismatched raw style");
    assert_eq!(
        core.render_reasoning_control_for_preset(&preset)
            .expect("mismatched observation is hidden")
            .state,
        lorepia_providers::parameter_mapping::UiControlState::Hidden
    );

    route.raw_metadata = Some(listed_model_metadata(&model).expect("canonical raw metadata"));
    route.metadata_observed_at = Some(now - chrono::Duration::seconds(1));
    core.inner
        .storage
        .save_model_route(&route)
        .expect("save timestamp-mismatched raw metadata");
    assert_eq!(
        core.render_reasoning_control_for_preset(&preset)
            .expect("timestamp-mismatched observation is hidden")
            .state,
        lorepia_providers::parameter_mapping::UiControlState::Hidden
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the capability conflict scenario is clearer as one chronological state transition"
)]
fn effective_capabilities_gate_reasoning_and_cache_with_exact_fresh_metadata() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let api_origin = CanonicalOrigin::parse("http://127.0.0.1:39491").expect("loopback origin");
    let (target, route) = create_openai_chat_generation_target(&core, &api_origin);
    let preset = core
        .inner
        .storage
        .get_generation_preset(&target.generation_preset_id)
        .expect("seeded generation preset");
    assert_eq!(
        core.render_reasoning_control_for_preset(&preset)
            .expect("hidden reasoning controls")
            .state,
        lorepia_providers::parameter_mapping::UiControlState::Hidden
    );
    assert_eq!(
        core.render_prompt_cache_control_for_preset(&preset)
            .expect("hidden cache controls")
            .state,
        lorepia_providers::parameter_mapping::UiControlState::Hidden
    );

    let error = resolve_generation_target(&core, &target)
        .err()
        .expect("family alone must not enable reasoning or prompt caching");
    assert!(error.message.contains("no observed reasoning control"));

    let observed_at = Utc::now();
    let reasoning = CapabilityObservation {
        id: ObservationId::from("reasoning-provider-api"),
        model_route_id: route.id.clone(),
        key: CapabilityKey::Reasoning,
        value: CapabilityValue::Structured(
            serde_json::to_value(ReasoningWireDialect::OpenAiChatCompletions {
                efforts: vec![
                    lorepia_providers::parameter_mapping::ReasoningEffort::Low,
                    lorepia_providers::parameter_mapping::ReasoningEffort::High,
                ],
                supports_disabled: true,
            })
            .expect("reasoning dialect JSON"),
        ),
        status: SupportStatus::Verified,
        source: ObservationSource::ProviderApi,
        confidence: Confidence::High,
        observed_at,
        expires_at: Some(observed_at + chrono::Duration::hours(1)),
        evidence_ref: None,
    };
    core.record_provider_api_capability_observations(vec![reasoning.clone()])
        .expect("store reasoning observation");
    let reasoning_control = core
        .render_reasoning_control_for_preset(&preset)
        .expect("render reasoning controls");
    assert_eq!(
        reasoning_control.state,
        lorepia_providers::parameter_mapping::UiControlState::Ready
    );
    assert_eq!(
        reasoning_control.allowed_efforts,
        vec![
            lorepia_providers::parameter_mapping::ReasoningEffort::Low,
            lorepia_providers::parameter_mapping::ReasoningEffort::High,
        ]
    );
    assert!(reasoning_control.issues.is_empty());
    let error = resolve_generation_target(&core, &target)
        .err()
        .expect("cache control must remain gated independently");
    assert!(
        error.message.contains("no provider prompt-cache control"),
        "{}",
        error.message
    );

    let prompt_cache = CapabilityObservation {
        id: ObservationId::from("cache-provider-api"),
        model_route_id: route.id.clone(),
        key: CapabilityKey::PromptCaching,
        value: CapabilityValue::Structured(
            serde_json::to_value(PromptCacheWireDialect::OpenAiAutomatic {
                supports_24_hour_retention: false,
            })
            .expect("prompt-cache dialect JSON"),
        ),
        status: SupportStatus::Verified,
        source: ObservationSource::ProviderApi,
        confidence: Confidence::High,
        observed_at,
        expires_at: Some(observed_at + chrono::Duration::hours(1)),
        evidence_ref: None,
    };
    core.record_provider_api_capability_observations(vec![prompt_cache])
        .expect("store cache observation");
    let cache_control = core
        .render_prompt_cache_control_for_preset(&preset)
        .expect("render cache controls");
    assert_eq!(
        cache_control.state,
        lorepia_providers::parameter_mapping::UiControlState::Ready
    );
    assert!(
        cache_control
            .allowed_modes
            .contains(&lorepia_providers::parameter_mapping::PromptCacheMode::Automatic)
    );
    assert!(cache_control.issues.is_empty());
    resolve_generation_target(&core, &target)
        .expect("exact reasoning and cache metadata unlock request mapping");

    let mut invalid_preset = preset.clone();
    invalid_preset.reasoning.effort = Some(GenerationReasoningEffort::Minimal);
    let invalid_control = core
        .render_reasoning_control_for_preset(&invalid_preset)
        .expect("render invalid reasoning controls");
    assert_eq!(
        invalid_control.state,
        lorepia_providers::parameter_mapping::UiControlState::Invalid
    );
    assert!(!invalid_control.issues.is_empty());

    let conflicting = CapabilityObservation {
        id: ObservationId::from("reasoning-probe-conflict"),
        model_route_id: route.id.clone(),
        key: CapabilityKey::Reasoning,
        value: CapabilityValue::Boolean(false),
        status: SupportStatus::Unsupported,
        source: ObservationSource::CapabilityProbe,
        confidence: Confidence::High,
        observed_at: observed_at + chrono::Duration::seconds(1),
        expires_at: Some(observed_at + chrono::Duration::hours(1)),
        evidence_ref: None,
    };
    core.record_probe_capability_observations(vec![conflicting])
        .expect("store conflicting probe");
    let effective = core
        .effective_capability(&route.id, CapabilityKey::Reasoning)
        .expect("effective capability")
        .expect("reasoning capability");
    assert_eq!(
        effective.selected.source,
        ObservationSource::CapabilityProbe
    );
    assert!(!effective.selected_is_stale);
    assert!(effective.has_conflict);
    let error = resolve_generation_target(&core, &target)
        .err()
        .expect("fresh conflicts must fail closed");
    assert!(error.message.contains("no observed reasoning control"));

    core.delete_capability_observation(&effective.selected.id)
        .expect("remove conflicting observation");
    resolve_generation_target(&core, &target).expect("removing conflict restores exact mapping");

    let mut wrong_family = reasoning;
    wrong_family.id = ObservationId::from("wrong-family-dialect");
    wrong_family.observed_at += chrono::Duration::seconds(2);
    wrong_family.value = CapabilityValue::Structured(
        serde_json::to_value(ReasoningWireDialect::GeminiThinkingBudget {
            minimum_budget_tokens: 1,
            maximum_budget_tokens: 1024,
            supports_zero_to_disable: true,
            supports_automatic: true,
            summaries: Vec::new(),
        })
        .expect("wrong-family dialect JSON"),
    );
    assert!(
        core.upsert_capability_observation(wrong_family)
            .expect_err("family-mismatched dialect must be rejected")
            .message
            .contains("does not match the API family")
    );
}

#[test]
fn signed_catalog_observations_cannot_outlive_the_active_catalog_pointer() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let origin = CanonicalOrigin::parse("http://127.0.0.1:11434").expect("loopback origin");
    let (_target, route) = create_openai_chat_generation_target(&core, &origin);
    let observed_at = Utc::now();
    let observation = CapabilityObservation {
        id: ObservationId::from("detached-signed-catalog-observation"),
        model_route_id: route.id.clone(),
        key: CapabilityKey::Streaming,
        value: CapabilityValue::Boolean(true),
        status: SupportStatus::Documented,
        source: ObservationSource::SignedLorepiaCatalog,
        confidence: Confidence::High,
        observed_at,
        expires_at: Some(observed_at + chrono::Duration::days(1)),
        evidence_ref: None,
    };
    assert!(
        core.upsert_capability_observation(observation.clone())
            .expect_err("detached signed catalog facts must not be accepted")
            .message
            .contains("active verified catalog")
    );

    // Legacy rows from a pre-projection build are ignored as well. Only
    // the currently active, signature-verified snapshot may supply this
    // provenance, so rollback cannot leave a detached fact selected.
    core.inner
        .storage
        .upsert_capability_observation(&observation)
        .expect("inject legacy detached row");
    assert!(
        core.list_capability_observations(&route.id)
            .expect("effective observations")
            .iter()
            .all(|value| value.id != observation.id)
    );
    assert!(
        core.effective_capability(&route.id, CapabilityKey::Streaming)
            .expect("effective capability")
            .is_none()
    );
}

#[test]
fn provider_model_refresh_preserves_missing_routes_and_their_presets() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let first = r#"{"data":[{"id":"keep-model"},{"id":"gone-model"}]}"#.to_owned();
    let second = r#"{"data":[{"id":"keep-model"}]}"#.to_owned();
    let (api_origin, requests) = spawn_model_list_provider(vec![first, second]);
    let (_template, connection) = create_openai_chat_connection(&core, &api_origin);

    let first_result = refresh_models_with_review(&core, &connection.id, Some("refresh-key"))
        .expect("initial model refresh");
    requests
        .recv_timeout(Duration::from_secs(2))
        .expect("initial model-list request");
    let keep_before = first_result
        .model_routes
        .iter()
        .find(|route| route.model_id == "keep-model")
        .expect("kept route")
        .clone();
    let gone_before = first_result
        .model_routes
        .iter()
        .find(|route| route.model_id == "gone-model")
        .expect("soon-missing route")
        .clone();
    let mut customized_preset = core
        .list_generation_presets(&gone_before.id)
        .expect("initial missing-route preset")
        .into_iter()
        .next()
        .expect("preset for soon-missing route");
    customized_preset.display_name = "Keep this preset".to_owned();
    customized_preset.updated_at = Utc::now();
    core.upsert_generation_preset(customized_preset.clone())
        .expect("customize missing-route preset");

    let second_result = refresh_models_with_review(&core, &connection.id, Some("refresh-key"))
        .expect("second model refresh");
    requests
        .recv_timeout(Duration::from_secs(2))
        .expect("second model-list request");

    assert!(second_result.newly_seen_model_route_ids.is_empty());
    assert!(second_result.created_generation_preset_ids.is_empty());
    assert_eq!(
        second_result.missing_model_route_ids,
        vec![gone_before.id.clone()]
    );
    let keep_after = second_result
        .model_routes
        .iter()
        .find(|route| route.model_id == "keep-model")
        .expect("kept route after refresh");
    assert_eq!(keep_after.id, keep_before.id);
    assert_eq!(keep_after.first_seen_at, keep_before.first_seen_at);
    assert_eq!(keep_after.status, ModelAvailability::Available);
    let gone_after = second_result
        .model_routes
        .iter()
        .find(|route| route.model_id == "gone-model")
        .expect("missing route remains");
    assert_eq!(gone_after.id, gone_before.id);
    assert_eq!(gone_after.first_seen_at, gone_before.first_seen_at);
    assert_eq!(gone_after.status, ModelAvailability::MissingTemporarily);
    for error in [
        core.validate_generation_preset_candidate(&customized_preset)
            .expect_err("missing route preset validation"),
        core.preview_provider_request_candidate(&customized_preset)
            .expect_err("missing route preview"),
        core.upsert_generation_preset(customized_preset.clone())
            .expect_err("missing route preset save"),
    ] {
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.message.contains("not currently available"));
    }
    assert_eq!(
        core.list_generation_presets(&gone_before.id)
            .expect("preserved missing-route presets"),
        vec![customized_preset]
    );
}

#[test]
fn provider_model_refresh_never_persists_the_borrowed_credential() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let (api_origin, requests) = spawn_model_list_provider(vec![
        r#"{"data":[{"id":"credential-safe-model"}]}"#.to_owned(),
    ]);
    let (_template, connection) = create_openai_chat_connection(&core, &api_origin);
    let secret = format!("refresh-secret-{}", Uuid::new_v4());

    let result = refresh_models_with_review(&core, &connection.id, Some(&secret))
        .expect("refresh provider models");
    let request = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured credential-bearing request");
    assert!(request.contains(&secret));
    assert!(!format!("{result:?}").contains(&secret));
    assert!(
        core.list_model_routes(&connection.id)
            .expect("persisted routes")
            .iter()
            .all(|route| !format!("{route:?}").contains(&secret))
    );

    drop(core);
    assert_directory_does_not_contain(root.path(), secret.as_bytes());
}
