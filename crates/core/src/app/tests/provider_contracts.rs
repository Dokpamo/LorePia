#[test]
fn generation_preset_validation_and_preview_share_the_route_plan() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let (api_origin, requests) =
        spawn_model_list_provider(vec![r#"{"data":[{"id":"preview-safe-model"}]}"#.to_owned()]);
    let (_template, connection) = create_openai_chat_connection(&core, &api_origin);

    let result = refresh_models_with_review(&core, &connection.id, Some("request-only-key"))
        .expect("refresh provider models");
    requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured model-list request");
    let route = result.model_routes.first().expect("refreshed model route");
    let preset = core
        .list_generation_presets(&route.id)
        .expect("generation presets")
        .into_iter()
        .next()
        .expect("initial generation preset");

    core.validate_generation_preset(&route.id, &preset.id)
        .expect("family-aware generation validation");
    let preview = core
        .preview_provider_request(&route.id, &preset.id)
        .expect("safe provider request preview");
    assert_eq!(preview.method(), lorepia_domain::HttpMethod::Post);
    assert_eq!(preview.origin(), &api_origin);
    assert_eq!(preview.path().as_str(), "/v1/chat/completions");
    assert!(preview.body().is_some());
    assert!(!format!("{preview:?}").contains("request-only-key"));

    let mut invalid = preset.clone();
    invalid.id = GenerationPresetId::from(format!("invalid-{}", Uuid::new_v4()));
    invalid.values = vec![lorepia_domain::ParameterValue {
        parameter_id: lorepia_domain::ParameterId::from("unknown-parameter"),
        state: lorepia_domain::ParameterValueState::Explicit(
            lorepia_domain::ParameterLiteral::Integer(1),
        ),
    }];
    let error = core
        .upsert_generation_preset(invalid.clone())
        .expect_err("invalid candidate must fail before persistence");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        core.list_generation_presets(&route.id)
            .expect("presets after rejected candidate")
            .iter()
            .all(|stored| stored.id != invalid.id)
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the table-like cross-family policy assertions share one catalog fixture"
)]
fn unsupported_opaque_continuity_is_normalized_or_rejected_before_generation() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let now = Utc::now();

    let (gemini_template, gemini_route) = create_built_in_public_route(
        &core,
        "gemini-generate-content-v1",
        "/v1beta",
        "gemini-2.5-flash",
    );
    let gemini_default = initial_generation_preset(&gemini_route.id, &gemini_template, now);
    assert!(!gemini_default.reasoning.preserve_opaque_state);
    let saved = core
        .upsert_generation_preset(gemini_default.clone())
        .expect("Gemini default with opaque continuity disabled");
    let resolved = resolve_generation_target(
        &core,
        &GenerationTarget {
            model_route_id: gemini_route.id.clone(),
            generation_preset_id: saved.id.clone(),
        },
    )
    .expect("Gemini target resolves without deferred continuity failure");
    assert!(!resolved.preserve_opaque_reasoning_state);

    let mut direct = gemini_default.clone();
    direct.id = GenerationPresetId::from(format!("direct-{}", Uuid::new_v4()));
    direct.reasoning.preserve_opaque_state = true;
    let control = core
        .render_reasoning_control_for_preset(&direct)
        .expect("render normalized Gemini control");
    assert!(!control.settings.preserve_opaque_state);
    for error in [
        core.validate_generation_preset_candidate(&direct)
            .expect_err("direct Gemini continuity candidate"),
        core.preview_provider_request_candidate(&direct)
            .expect_err("Gemini preview must share the pre-network gate"),
        core.upsert_generation_preset(direct.clone())
            .expect_err("Gemini continuity must fail before persistence"),
    ] {
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(error.message, GEMINI_OPAQUE_REASONING_TOPOLOGY_ERROR);
    }
    assert!(
        core.list_generation_presets(&gemini_route.id)
            .expect("Gemini presets")
            .iter()
            .all(|preset| preset.id != direct.id)
    );

    let mut legacy = gemini_default;
    legacy.reasoning.preserve_opaque_state = true;
    core.inner
        .storage
        .save_generation_preset(&legacy)
        .expect("seed legacy Gemini preset");
    core.validate_generation_preset(&gemini_route.id, &legacy.id)
        .expect("legacy credential-bound preset is normalized off");
    let legacy_resolved = resolve_generation_target(
        &core,
        &GenerationTarget {
            model_route_id: gemini_route.id.clone(),
            generation_preset_id: legacy.id,
        },
    )
    .expect("legacy credential-bound target resolves safely");
    assert!(!legacy_resolved.preserve_opaque_reasoning_state);

    let (responses_template, responses_route) =
        create_built_in_public_route(&core, "openai-responses-v1", "/v1", "gpt-5-fixture");
    let mut responses_default =
        initial_generation_preset(&responses_route.id, &responses_template, now);
    assert!(!responses_default.reasoning.preserve_opaque_state);
    let responses_saved = core
        .upsert_generation_preset(responses_default.clone())
        .expect("OpenAI Responses default disables lossy opaque continuity");
    let responses_resolved = resolve_generation_target(
        &core,
        &GenerationTarget {
            model_route_id: responses_route.id,
            generation_preset_id: responses_saved.id,
        },
    )
    .expect("OpenAI Responses target without opaque continuity");
    assert!(!responses_resolved.preserve_opaque_reasoning_state);
    responses_default.reasoning.preserve_opaque_state = true;
    let responses_error = core
        .validate_generation_preset_candidate(&responses_default)
        .expect_err("OpenAI Responses cannot replay incomplete response topology");
    assert_eq!(
        responses_error.message,
        OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR
    );

    let (openrouter_template, openrouter_route) =
        create_built_in_public_route(&core, "openrouter-v1", "/api/v1", "openai/gpt-fixture");
    let mut openrouter_default =
        initial_generation_preset(&openrouter_route.id, &openrouter_template, now);
    assert!(!openrouter_default.reasoning.preserve_opaque_state);
    assert!(
        !core
            .render_reasoning_control_for_preset(&openrouter_default)
            .expect("render credential-bound OpenRouter control")
            .settings
            .preserve_opaque_state
    );
    let openrouter_preset = core
        .upsert_generation_preset(openrouter_default.clone())
        .expect("credential-bound OpenRouter disables opaque continuity");
    let openrouter = resolve_generation_target(
        &core,
        &GenerationTarget {
            model_route_id: openrouter_route.id.clone(),
            generation_preset_id: openrouter_preset.id,
        },
    )
    .expect("credential-bound OpenRouter target");
    assert!(!openrouter.preserve_opaque_reasoning_state);
    openrouter_default.reasoning.preserve_opaque_state = true;
    for error in [
        core.validate_generation_preset_candidate(&openrouter_default)
            .expect_err("OpenRouter continuity candidate must fail closed"),
        core.preview_provider_request_candidate(&openrouter_default)
            .expect_err("OpenRouter continuity preview must fail closed"),
        core.upsert_generation_preset(openrouter_default.clone())
            .expect_err("OpenRouter continuity save must fail closed"),
    ] {
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(error.message, OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR);
    }

    let loopback = CanonicalOrigin::parse("http://127.0.0.1:65534").expect("loopback origin");
    let (generic_template, generic_connection) = create_openai_chat_connection(&core, &loopback);
    let generic_route = ModelRoute {
        id: ModelRouteId::from(format!("route-{}", Uuid::new_v4())),
        connection_id: generic_connection.id,
        api_family: generic_template.api_family,
        model_id: "generic-chat".to_owned(),
        display_name: None,
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
    };
    core.upsert_model_route(generic_route.clone())
        .expect("save generic Chat Completions route");
    let mut generic_default = initial_generation_preset(&generic_route.id, &generic_template, now);
    assert!(!generic_default.reasoning.preserve_opaque_state);
    let generic_saved = core
        .upsert_generation_preset(generic_default.clone())
        .expect("generic Chat Completions default");
    let generic_resolved = resolve_generation_target(
        &core,
        &GenerationTarget {
            model_route_id: generic_route.id.clone(),
            generation_preset_id: generic_saved.id,
        },
    )
    .expect("generic Chat Completions target");
    assert!(!generic_resolved.preserve_opaque_reasoning_state);
    generic_default.reasoning.preserve_opaque_state = true;
    let generic_error = core
        .validate_generation_preset_candidate(&generic_default)
        .expect_err("generic Chat Completions cannot advertise OpenRouter continuity");
    assert_eq!(
        generic_error.message,
        OPAQUE_REASONING_STATE_UNSUPPORTED_ERROR
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "credential rotation, a legacy row, and reopen assertions must share one fixture"
)]
fn opaque_preset_is_provenance_only_but_credential_targets_never_load_or_persist_it() {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(&character.id, "Opaque continuity", ConversationMode::Chat)
        .expect("conversation");
    let state = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state");
    let (template, route) =
        create_built_in_public_route(&core, "openrouter-v1", "/api/v1", "openrouter/test-model");
    let model = route.model_id.clone();
    let route_id = route.id.clone();
    let source_preset = core
        .upsert_generation_preset(initial_generation_preset(&route_id, &template, Utc::now()))
        .expect("source preset");
    let source_preset_id = source_preset.id.clone();
    assert!(!source_preset.reasoning.preserve_opaque_state);
    let source_target = GenerationTarget {
        model_route_id: route_id.clone(),
        generation_preset_id: source_preset_id.clone(),
    };
    let retained_state = OpaqueReasoningState::OpenRouterReasoning {
        topology: OpenRouterReasoningTopology::new(
            None,
            Some(vec![
                OpenRouterReasoningDetail::from_value(&serde_json::json!({
                    "type": "reasoning.encrypted",
                    "data": "opaque-state",
                    "id": "detail-1",
                    "format": "openrouter-v1",
                    "index": 0
                }))
                .expect("OpenRouter opaque detail"),
            ]),
        )
        .expect("OpenRouter opaque topology"),
    };
    let (source_capture_sender, source_capture_receiver) = std_mpsc::channel();
    let source_provider = Arc::new(OpaqueContinuityProvider {
        response: "source response".to_owned(),
        emitted_state: Some(retained_state.clone()),
        captured_request: Mutex::new(Some(source_capture_sender)),
    });

    // Even an internal caller asking to preserve state is overridden when
    // the actual borrowed credential is non-empty.
    let source_generation_id = core
        .send_message_to_branch_with_provider_options(
            &conversation.id,
            &state.active_branch_id,
            None,
            ConversationMode::Chat,
            "first",
            new_test_generation_operation("opaque-first-v1"),
            model.clone(),
            Some(&source_target),
            Some(ApiFamily::OpenAiChatCompletions),
            true,
            None,
            Some(128),
            Some("credential-a".to_owned()),
            None,
            false,
            source_provider,
        )
        .expect("source generation");
    let (source_preserve, source_contexts, _) = source_capture_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("captured source request");
    assert!(!source_preserve);
    assert!(source_contexts.is_empty());
    let source_generation =
        wait_for_generation_status(&core, &source_generation_id, GenerationStatus::Complete);
    assert!(source_generation.opaque_reasoning_state.is_empty());
    let source_assistant = core
        .list_branch_messages(&state.active_branch_id)
        .expect("source branch messages")
        .into_iter()
        .find(|message| {
            message.role == MessageRole::Assistant
                && message.generation_id.as_ref() == Some(&source_generation_id)
        })
        .expect("source assistant");

    // Simulate a completed row written by an older release while key A was
    // active. Credentials are intentionally absent from generation rows,
    // so Core must never infer that this state is safe for key B.
    let legacy_generation_id = GenerationId::new();
    let legacy_user = Message::user_after(
        conversation.id.clone(),
        Some(source_assistant.id.clone()),
        "legacy credential-A turn",
    );
    let legacy_assistant = Message::pending_assistant(
        conversation.id.clone(),
        legacy_user.id.clone(),
        legacy_generation_id.clone(),
    );
    let legacy_generation = GenerationRecord {
        id: legacy_generation_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: state.active_branch_id.clone(),
        user_message_id: legacy_user.id.clone(),
        assistant_message_id: Some(legacy_assistant.id.clone()),
        mode: ConversationMode::Chat,
        model: model.clone(),
        model_route_id: Some(route_id.clone()),
        generation_preset_id: Some(source_preset_id.clone()),
        provider_family: Some(ApiFamily::OpenAiChatCompletions),
        status: GenerationStatus::Running,
        input_tokens: None,
        cached_read_tokens: None,
        cached_write_tokens: None,
        output_tokens: None,
        reasoning_tokens: None,
        tool_tokens: None,
        provider_raw_summary: None,
        opaque_reasoning_state: Vec::new(),
        error_code: None,
        started_at: Utc::now(),
        finished_at: None,
    };
    core.inner
        .storage
        .append_generation(
            &state.active_branch_id,
            Some(&source_assistant.id),
            &legacy_user,
            &legacy_assistant,
            &legacy_generation,
        )
        .expect("seed running legacy generation");
    let mut legacy_terminal = legacy_assistant;
    legacy_terminal.content = "legacy response".to_owned();
    legacy_terminal.status = MessageStatus::Complete;
    core.inner
        .storage
        .finalize_generation_with_protocol_state(
            &legacy_terminal,
            Some(&GenerationUsage::default()),
            std::slice::from_ref(&retained_state),
            None,
            true,
        )
        .expect("seed legacy credential-A opaque state");
    assert_eq!(
        core.inner
            .storage
            .get_generation(&legacy_generation_id)
            .expect("legacy generation")
            .opaque_reasoning_state,
        vec![retained_state.clone()]
    );

    // Preset ID remains source provenance rather than continuity identity.
    // This dormant loader may match the exact family/model/route/source
    // under a different current preset, while the credential gate below
    // ensures production requests never receive that context.
    let different_current_target = GenerationTarget {
        model_route_id: route_id.clone(),
        generation_preset_id: GenerationPresetId::from("different-current-preset"),
    };
    let dormant_context = load_opaque_reasoning_context(
        &core.inner.storage,
        std::slice::from_ref(&legacy_terminal),
        ApiFamily::OpenAiChatCompletions,
        &model,
        &different_current_target,
    )
    .expect("load dormant context under a different current preset");
    assert_eq!(dormant_context.len(), 1);
    assert_eq!(dormant_context[0].source_message_id, legacy_terminal.id);
    assert_eq!(dormant_context[0].model_route_id, route_id);
    assert_eq!(dormant_context[0].generation_preset_id, source_preset_id);
    assert_ne!(
        dormant_context[0].generation_preset_id,
        different_current_target.generation_preset_id
    );

    let resolved = resolve_generation_target(&core, &source_target)
        .expect("credential-bound target resolves with continuity disabled");
    assert!(!resolved.preserve_opaque_reasoning_state);
    let next_state = OpaqueReasoningState::OpenRouterReasoning {
        topology: OpenRouterReasoningTopology::new(
            Some("new key-B reasoning".to_owned()),
            Some(Vec::new()),
        )
        .expect("new OpenRouter topology"),
    };
    let (capture_sender, capture_receiver) = std_mpsc::channel();
    let next_provider = Arc::new(OpaqueContinuityProvider {
        response: "next response".to_owned(),
        emitted_state: Some(next_state),
        captured_request: Mutex::new(Some(capture_sender)),
    });
    let next_generation_id = core
        .send_message_to_branch_with_provider_options(
            &conversation.id,
            &state.active_branch_id,
            Some(&legacy_terminal.id),
            ConversationMode::Chat,
            "second",
            new_test_generation_operation("opaque-second-v1"),
            model.clone(),
            Some(&source_target),
            Some(ApiFamily::OpenAiChatCompletions),
            true,
            None,
            Some(128),
            Some("credential-b".to_owned()),
            None,
            false,
            next_provider,
        )
        .expect("next generation with a different credential");
    let (preserve, contexts, current_provenance) = capture_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("captured next request");
    assert!(!preserve);
    assert!(contexts.is_empty());
    let next_generation =
        wait_for_generation_status(&core, &next_generation_id, GenerationStatus::Complete);
    assert!(next_generation.opaque_reasoning_state.is_empty());

    assert_eq!(
        current_provenance,
        Some(GenerationProviderProvenance {
            api_family: ApiFamily::OpenAiChatCompletions,
            model_route_id: route_id.clone(),
            generation_preset_id: source_preset_id,
        })
    );
    wait_for_generation_registry_to_drain(&core);
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    assert_eq!(
        reopened
            .inner
            .storage
            .get_generation(&legacy_generation_id)
            .expect("reopened legacy generation")
            .opaque_reasoning_state,
        vec![retained_state]
    );
    assert!(
        reopened
            .inner
            .storage
            .get_generation(&next_generation_id)
            .expect("reopened key-B generation")
            .opaque_reasoning_state
            .is_empty()
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "route construction and durable no-auth assertions intentionally share one fixture"
)]
fn nonempty_raw_credential_disables_opaque_state_on_a_no_auth_connection() {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "No-auth raw credential",
            ConversationMode::Chat,
        )
        .expect("conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let template = core
        .list_provider_templates()
        .expect("provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "ollama-native-v1")
        .expect("Ollama template");
    let api_origin = CanonicalOrigin::parse("http://127.0.0.1:11434").expect("loopback origin");
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(format!("no-auth-{}", Uuid::new_v4())),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: "No-auth Ollama".to_owned(),
            api_origin,
            api_base_path: Some(EndpointPath::parse("/api").expect("API base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            values: Vec::new(),
            approved_credential_origin: None,
            local_network_approval: None,
            timeout_seconds: 5,
        })
        .expect("create no-auth connection");
    assert!(connection.credential_ref.is_none());
    let now = Utc::now();
    let route = ModelRoute {
        id: ModelRouteId::from(format!("no-auth-route-{}", Uuid::new_v4())),
        connection_id: connection.id,
        api_family: ApiFamily::OllamaNative,
        model_id: "llama-no-auth".to_owned(),
        display_name: None,
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
    };
    core.upsert_model_route(route.clone())
        .expect("save no-auth route");
    let preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
        .expect("save no-auth preset");
    let target = GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: preset.id,
    };

    let (capture_sender, capture_receiver) = std_mpsc::channel();
    let provider = Arc::new(OpaqueContinuityProvider {
        response: "safe response".to_owned(),
        emitted_state: Some(OpaqueReasoningState::GeminiThoughtSignature {
            part_index: 0,
            signature: lorepia_domain::OpaqueReasoningData::parse("safe-signature")
                .expect("signature"),
        }),
        captured_request: Mutex::new(Some(capture_sender)),
    });
    let generation_id = core
        .send_message_to_branch_with_provider_options(
            &conversation.id,
            &branch,
            None,
            ConversationMode::Chat,
            "hello",
            new_test_generation_operation("no-auth-raw-credential-v1"),
            "llama-no-auth".to_owned(),
            Some(&target),
            Some(ApiFamily::OllamaNative),
            true,
            None,
            Some(128),
            Some("unexpected-raw-credential".to_owned()),
            None,
            false,
            provider,
        )
        .expect("start no-auth generation");
    let (preserve, contexts, provenance) = capture_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("captured no-auth request");
    assert!(!preserve);
    assert!(contexts.is_empty());
    assert_eq!(
        provenance,
        Some(GenerationProviderProvenance {
            api_family: ApiFamily::OllamaNative,
            model_route_id: target.model_route_id,
            generation_preset_id: target.generation_preset_id,
        })
    );
    let generation = wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
    assert!(generation.opaque_reasoning_state.is_empty());
    wait_for_generation_registry_to_drain(&core);
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    assert!(
        reopened
            .inner
            .storage
            .get_generation(&generation_id)
            .expect("reopened no-auth generation")
            .opaque_reasoning_state
            .is_empty()
    );
}

#[test]
fn provider_model_sync_rejects_reflected_credential_without_persisting_it() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let secret = format!("reflected-secret-{}", Uuid::new_v4());
    let body = serde_json::json!({
        "data": [{"id": secret.clone()}],
    })
    .to_string();
    let (api_origin, requests) = spawn_model_list_provider(vec![body]);
    let (_template, connection) = create_openai_chat_connection(&core, &api_origin);

    let error = refresh_models_with_review(&core, &connection.id, Some(&secret))
        .expect_err("credential reflection must fail closed");
    requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured credential-bearing request");
    assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
    let jobs = core
        .list_provider_model_syncs(&connection.id, 4)
        .expect("durable failed job");
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, ModelSyncState::Failed);
    assert!(jobs[0].review.is_none());
    assert!(!format!("{jobs:?}").contains(&secret));

    drop(core);
    assert_directory_does_not_contain(root.path(), secret.as_bytes());
}

#[test]
fn job_scoped_model_sync_event_poll_does_not_consume_another_job() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    for (id, origin) in [
        ("event-job-a", "https://events-a.example.com/v1"),
        ("event-job-b", "https://events-b.example.com/v1"),
    ] {
        core.upsert_provider_profile(ProviderProfile {
            id: id.to_owned(),
            display_name: id.to_owned(),
            base_url: origin.to_owned(),
            model: "existing-model".to_owned(),
            timeout_seconds: 30,
        })
        .expect("seed provider graph");
    }
    let first_connection = core
        .inner
        .storage
        .get_provider_connection(&ProviderConnectionId::from("event-job-a"))
        .expect("first connection");
    let second_connection = core
        .inner
        .storage
        .get_provider_connection(&ProviderConnectionId::from("event-job-b"))
        .expect("second connection");
    let first_job = core
        .inner
        .storage
        .create_model_sync_job(&first_connection)
        .expect("first model sync job");
    let second_job = core
        .inner
        .storage
        .create_model_sync_job(&second_connection)
        .expect("second model sync job");

    let first_events = core
        .poll_provider_model_sync_events(&first_job.id, 16)
        .expect("poll first job");
    assert_eq!(first_events.len(), 1);
    assert_eq!(first_events[0].job_id, first_job.id);
    assert!(
        core.ack_provider_model_sync_event(&first_job.id, first_events[0].sequence)
            .expect("ack first job")
    );

    let second_events = core
        .poll_provider_model_sync_events(&second_job.id, 16)
        .expect("poll second job");
    assert_eq!(second_events.len(), 1);
    assert_eq!(second_events[0].job_id, second_job.id);
    assert_eq!(
        core.poll_provider_model_sync_events(&second_job.id, 16)
            .expect("second event remains until acknowledged"),
        second_events
    );
    assert!(
        core.ack_provider_model_sync_event(&second_job.id, second_events[0].sequence)
            .expect("ack second job")
    );
}

#[test]
fn provider_model_refresh_records_safe_failure_statuses() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let secret = format!("failure-secret-{}", Uuid::new_v4());

    let (auth_origin, auth_requests) = spawn_model_list_http_provider(vec![(
        "401 Unauthorized".to_owned(),
        r#"{"error":"invalid credential"}"#.to_owned(),
    )]);
    let (_template, auth_connection) = create_openai_chat_connection(&core, &auth_origin);
    let auth_error = refresh_models_with_review(&core, &auth_connection.id, Some(&secret))
        .expect_err("401 model refresh must fail");
    auth_requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured auth-failing request");
    assert_eq!(auth_error.code, CoreErrorCode::ProviderAuthFailed);
    assert!(!format!("{auth_error:?}").contains(&secret));
    assert_eq!(
        core.inner
            .storage
            .get_provider_connection(&auth_connection.id)
            .expect("auth-failed connection")
            .status,
        ConnectionStatus::AuthFailed
    );

    let (unavailable_origin, unavailable_requests) = spawn_model_list_http_provider(vec![(
        "503 Service Unavailable".to_owned(),
        r#"{"error":"temporarily unavailable"}"#.to_owned(),
    )]);
    let (_template, unavailable_connection) =
        create_openai_chat_connection(&core, &unavailable_origin);
    let unavailable_error =
        refresh_models_with_review(&core, &unavailable_connection.id, Some(&secret))
            .expect_err("503 model refresh must fail");
    unavailable_requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured unavailable request");
    assert_eq!(unavailable_error.code, CoreErrorCode::ProviderUnavailable);
    assert!(!format!("{unavailable_error:?}").contains(&secret));
    assert_eq!(
        core.inner
            .storage
            .get_provider_connection(&unavailable_connection.id)
            .expect("unavailable connection")
            .status,
        ConnectionStatus::Unavailable
    );
}

#[test]
fn initial_model_preset_is_deferred_when_template_requires_an_explicit_value() {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let templates = core.list_provider_templates().expect("provider templates");
    let anthropic = templates
        .iter()
        .find(|template| template.id.as_str() == "anthropic-messages-v1")
        .expect("Anthropic template");
    let openai_chat = templates
        .iter()
        .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
        .expect("OpenAI-compatible template");

    assert!(!template_accepts_empty_preset(anthropic).expect("Anthropic preset requirement"));
    assert!(
        template_accepts_empty_preset(openai_chat).expect("OpenAI-compatible preset requirement")
    );
}
