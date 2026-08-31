#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end contract proves nonce isolation across preview, resume, storage, and dispatch"
)]
fn reviewed_operation_nonce_changes_only_the_durable_operation_identity() {
    const NONCE_A: &str = "reviewed-nonce-isolation-A-7d4f";
    const NONCE_B: &str = "reviewed-nonce-isolation-B-9a21";

    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_stoppable_provider();
    let target = provider_fixture(&core, &origin);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic reviewed nonce isolation",
            ConversationMode::Chat,
        )
        .expect("create reviewed nonce conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list reviewed nonce branch")
        .into_iter()
        .next()
        .expect("root branch");
    let preset = prompt_preset("synthetic.core.reviewed-nonce-isolation");
    core.upsert_prompt_preset(&preset, None)
        .expect("save reviewed nonce prompt preset");
    let request = PromptPlanRequest {
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        expected_head: None,
        user_text: USER_TEXT_CANARY.to_owned(),
        generation_target: target.clone(),
        prompt_preset_id: Some(preset.id.clone()),
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };

    let preview_a = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: NONCE_A,
            },
        )
        .expect("resolve reviewed preview with nonce A");
    let preview_b = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: NONCE_B,
            },
        )
        .expect("resolve reviewed preview with nonce B");
    assert_ne!(
        preview_a.generation_attempt_id, preview_b.generation_attempt_id,
        "rotating only the caller nonce must allocate a new durable attempt"
    );
    assert_eq!(preview_a.plan, preview_b.plan);
    assert_eq!(preview_a.effective_messages, preview_b.effective_messages);
    assert_eq!(preview_a.provider_request, preview_b.provider_request);
    assert_eq!(preview_a.applied_parameters, preview_b.applied_parameters);
    assert_eq!(preview_a.prompt_diff, preview_b.prompt_diff);

    let trace_a = core
        .explain_prompt_plan(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview_a.generation_attempt_id,
            },
            &preview_a.plan.plan_hash,
        )
        .expect("explain nonce A preview");
    let trace_b = core
        .explain_prompt_plan(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview_b.generation_attempt_id,
            },
            &preview_b.plan.plan_hash,
        )
        .expect("explain nonce B preview");
    assert_eq!(trace_a, trace_b);
    assert!(trace_a.session_seed.is_some());

    let other_conversation = core
        .create_conversation(
            &character_id,
            "Synthetic reviewed nonce cross-room rejection",
            ConversationMode::Chat,
        )
        .expect("create cross-room resume target");
    let other_branch = core
        .list_conversation_branches(&other_conversation.id)
        .expect("list cross-room branch")
        .into_iter()
        .next()
        .expect("cross-room root branch");
    let mut cross_room = request.clone();
    cross_room.conversation_id = other_conversation.id;
    cross_room.branch_id = other_branch.id;
    let mut changed_text = request.clone();
    changed_text.user_text = "A different caller-owned reviewed message".to_owned();
    let mut changed_target = request.clone();
    changed_target.generation_target = GenerationTarget {
        model_route_id: ModelRouteId::from("synthetic-reviewed-hijack-route"),
        generation_preset_id: "synthetic-reviewed-hijack-preset".into(),
    };
    for (case, mismatched_request) in [
        ("cross-room", cross_room),
        ("changed text", changed_text),
        ("changed target", changed_target),
    ] {
        let error = core
            .resolve_prompt_preview(
                &mismatched_request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &preview_a.generation_attempt_id,
                },
            )
            .expect_err("a reviewed resume cannot hijack another caller-owned request");
        assert_eq!(error.code, CoreErrorCode::InvalidInput, "{case}");
        assert!(error.recoverable, "{case}");
        assert!(
            error.message.contains("start a new generation operation"),
            "{case}: {}",
            error.message
        );
    }
    assert_eq!(generation_attempt_count(root.path()), 2);
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    for (label, encoded) in [
        (
            "prompt plan A",
            serde_json::to_string(&preview_a.plan).expect("serialize prompt plan A"),
        ),
        (
            "prompt trace A",
            serde_json::to_string(&trace_a).expect("serialize prompt trace A"),
        ),
        (
            "provider request A",
            serde_json::to_string(&preview_a.provider_request)
                .expect("serialize provider request A"),
        ),
        (
            "prompt plan B",
            serde_json::to_string(&preview_b.plan).expect("serialize prompt plan B"),
        ),
        (
            "prompt trace B",
            serde_json::to_string(&trace_b).expect("serialize prompt trace B"),
        ),
        (
            "provider request B",
            serde_json::to_string(&preview_b.provider_request)
                .expect("serialize provider request B"),
        ),
    ] {
        assert!(!encoded.contains(NONCE_A), "{label} leaked nonce A");
        assert!(!encoded.contains(NONCE_B), "{label} leaked nonce B");
    }

    let mut approved = request.clone();
    approved.expected_plan_hash = Some(preview_b.plan.plan_hash.clone());
    let generation_id = core
        .send_message_with_prompt_plan(
            &approved,
            &preview_b.generation_attempt_id,
            reviewed_provider_credential(&core),
        )
        .expect("send reviewed nonce B attempt");
    wait_for_generation(&core, &branch.id, &generation_id);
    let captured = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("capture reviewed nonce provider request");
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    provider.stop();
    let captured_body = request_body(&captured);
    let captured_json = serde_json::to_string(&captured_body).expect("serialize wire request");
    assert!(!captured_json.contains(NONCE_A));
    assert!(!captured_json.contains(NONCE_B));

    drop(core);
    let storage = Storage::open(root.path()).expect("open typed nonce-isolation storage");
    let attempt_a = storage
        .get_generation_attempt(&preview_a.generation_attempt_id)
        .expect("load nonce A attempt");
    let attempt_b = storage
        .get_generation_attempt(&preview_b.generation_attempt_id)
        .expect("load nonce B attempt");
    assert_ne!(attempt_a.input.operation_id, attempt_b.input.operation_id);
    assert_eq!(
        attempt_a.input.base_request_fingerprint_sha256,
        attempt_b.input.base_request_fingerprint_sha256
    );
    let mut nonce_free_input_a = attempt_a.input.clone();
    let mut nonce_free_input_b = attempt_b.input.clone();
    nonce_free_input_a.operation_id.clear();
    nonce_free_input_b.operation_id.clear();
    assert_eq!(
        nonce_free_input_a, nonce_free_input_b,
        "only the derived operation id may differ between nonce variants"
    );
    let generation = storage
        .get_generation(&generation_id)
        .expect("load reviewed nonce generation payload");
    let stored_plan = storage
        .get_generation_prompt_plan_by_generation(&generation_id)
        .expect("load reviewed nonce prompt snapshot");
    assert_eq!(stored_plan.random_seed, trace_a.session_seed);
    assert_eq!(stored_plan.random_seed, trace_b.session_seed);
    assert_eq!(stored_plan.provider_request.request.value, captured_body);

    for (label, encoded) in [
        (
            "stored attempt A input",
            serde_json::to_string(&attempt_a.input).expect("serialize attempt A input"),
        ),
        (
            "stored attempt B input",
            serde_json::to_string(&attempt_b.input).expect("serialize attempt B input"),
        ),
        (
            "stored prompt plan",
            serde_json::to_string(&stored_plan).expect("serialize stored prompt plan"),
        ),
        (
            "generation payload",
            serde_json::to_string(&generation).expect("serialize generation payload"),
        ),
    ] {
        assert!(!encoded.contains(NONCE_A), "{label} leaked nonce A");
        assert!(!encoded.contains(NONCE_B), "{label} leaked nonce B");
    }
    drop(storage);
    assert_tree_excludes(root.path(), NONCE_A.as_bytes());
    assert_tree_excludes(root.path(), NONCE_B.as_bytes());
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end contract compares preview, durable snapshot, and provider payload"
)]
fn preview_send_provider_and_snapshot_share_one_hash_bound_plan() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_stoppable_provider();
    let target = provider_fixture(&core, &origin);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic prompt identity",
            lorepia_core::ConversationMode::Chat,
        )
        .expect("create prompt identity conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list prompt identity branch")
        .into_iter()
        .next()
        .expect("root branch");
    let preset = prompt_preset("synthetic.core.prompt-identity");
    let stored_preset = core
        .upsert_prompt_preset(&preset, None)
        .expect("save identity prompt preset");
    let request = PromptPlanRequest {
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        expected_head: None,
        user_text: USER_TEXT_CANARY.to_owned(),
        generation_target: target.clone(),
        prompt_preset_id: Some(preset.id.clone()),
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };

    let expert_preview = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: "prompt-identity-preview-v1",
            },
        )
        .expect("resolve expert prompt preview");
    let preview = core
        .render_prompt_preview(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &expert_preview.generation_attempt_id,
            },
        )
        .expect("render prompt preview");
    assert_eq!(expert_preview.plan, preview);
    assert_eq!(
        preview,
        core.render_prompt_preview(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &expert_preview.generation_attempt_id,
            },
        )
        .expect("repeat prompt preview")
    );
    let redacted_preview = serde_json::to_string(&preview).expect("serialize redacted preview");
    assert!(!redacted_preview.contains(USER_TEXT_CANARY));
    assert!(!redacted_preview.contains(CREDENTIAL_CANARY));

    let mut tampered = request.clone();
    tampered.expected_plan_hash = Some("00".repeat(32));
    let mismatch = core
        .send_message_with_prompt_plan(
            &tampered,
            &expert_preview.generation_attempt_id,
            reviewed_provider_credential(&core),
        )
        .expect_err("tampered reviewed hash must fail before send");
    assert_eq!(mismatch.code, CoreErrorCode::InvalidInput);
    assert!(
        core.list_branch_messages(&branch.id)
            .expect("messages after rejected send")
            .is_empty()
    );
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        generation_attempt_count(root.path()),
        1,
        "review and hash rejection must retain exactly one original attempt"
    );

    let connection_id = ProviderConnectionId::from("synthetic-orchestration-connection");
    let mut changed_connection = core
        .list_provider_connections()
        .expect("list provider connections before drift")
        .into_iter()
        .find(|candidate| candidate.id == connection_id)
        .expect("reviewed connection used by preview");
    changed_connection.timeout_seconds = 7;
    core.upsert_provider_connection(changed_connection)
        .expect("change connection timeout under the same connection id");

    let mut changed_route = core
        .list_model_routes(&connection_id)
        .expect("list model routes before drift")
        .into_iter()
        .find(|candidate| candidate.id == target.model_route_id)
        .expect("model route used by preview");
    changed_route.display_name = Some("Synthetic model after reviewed drift".to_owned());
    core.upsert_model_route(changed_route)
        .expect("change route metadata under the same route id");

    core.upsert_user_capability_override(CapabilityObservation {
        id: ObservationId::from("synthetic-reviewed-context-window-drift"),
        model_route_id: target.model_route_id.clone(),
        key: CapabilityKey::ContextWindow,
        value: CapabilityValue::Integer(16_384),
        status: SupportStatus::Verified,
        source: ObservationSource::UserOverride,
        confidence: Confidence::Low,
        observed_at: Utc::now(),
        expires_at: None,
        evidence_ref: None,
    })
    .expect("change capability input under the same route id");

    let mut changed_generation_preset = core
        .list_generation_presets(&target.model_route_id)
        .expect("list generation presets")
        .into_iter()
        .find(|candidate| candidate.id == target.generation_preset_id)
        .expect("generation preset used by preview");
    changed_generation_preset.values = vec![ParameterValue {
        parameter_id: ParameterId::from("temperature"),
        state: ParameterValueState::Explicit(ParameterLiteral::Number(0.25)),
    }];
    changed_generation_preset.updated_at = Utc::now();
    core.upsert_generation_preset(changed_generation_preset)
        .expect("change exact request-plan input under the same preset id");

    let mut stale_review = request.clone();
    stale_review.expected_plan_hash = Some(preview.plan_hash.clone());
    let drift = core
        .send_message_with_prompt_plan(
            &stale_review,
            &expert_preview.generation_attempt_id,
            reviewed_provider_credential(&core),
        )
        .expect_err("provider mapping drift must fail before reviewed send");
    assert_eq!(drift.code, CoreErrorCode::InvalidInput);
    assert!(drift.recoverable);
    assert!(drift.message.contains("new generation operation"));
    assert!(
        core.list_branch_messages(&branch.id)
            .expect("messages after rejected provider mapping drift")
            .is_empty()
    );
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        generation_attempt_count(root.path()),
        1,
        "drift rejection must not synthesize a replacement attempt"
    );

    let fresh_expert_preview = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: "prompt-identity-preview-v2",
            },
        )
        .expect("render fresh expert execution preview");
    let fresh_preview = &fresh_expert_preview.plan;
    assert_ne!(
        fresh_preview.plan_hash, preview.plan_hash,
        "request-plan input changes must alter the composite execution hash"
    );
    assert_ne!(
        fresh_expert_preview.generation_attempt_id, expert_preview.generation_attempt_id,
        "an explicit new nonce must create a newly sealed attempt"
    );
    assert_eq!(
        generation_attempt_count(root.path()),
        2,
        "only an explicit new nonce may add the fresh attempt"
    );
    assert_eq!(
        fresh_expert_preview.effective_messages, expert_preview.effective_messages,
        "provider parameter changes must not rewrite effective prompt messages"
    );
    let mut approved = request;
    approved.expected_plan_hash = Some(fresh_preview.plan_hash.clone());
    let generation_id = core
        .send_message_with_prompt_plan(
            &approved,
            &fresh_expert_preview.generation_attempt_id,
            reviewed_provider_credential(&core),
        )
        .expect("send reviewed prompt plan");
    wait_for_generation(&core, &branch.id, &generation_id);
    let captured = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("capture exact provider request");
    provider.stop();
    let captured_text = String::from_utf8_lossy(&captured);
    assert!(captured_text.to_ascii_lowercase().contains(&format!(
        "authorization: bearer {}",
        CREDENTIAL_CANARY.to_ascii_lowercase()
    )));
    let wire_body = request_body(&captured);
    let wire_messages = wire_body
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .expect("OpenAI-compatible messages array");
    assert_eq!(fresh_preview.provider_messages.len(), wire_messages.len());
    assert_eq!(
        wire_body
            .get("temperature")
            .and_then(serde_json::Value::as_f64),
        Some(0.25)
    );
    assert!(wire_messages.iter().any(|message| {
        message.get("role").and_then(serde_json::Value::as_str) == Some("user")
            && message.get("content").and_then(serde_json::Value::as_str) == Some(USER_TEXT_CANARY)
    }));

    let snapshot = core
        .get_generation_prompt_plan(&generation_id)
        .expect("load immutable generation prompt plan");
    assert_eq!(snapshot.id, fresh_preview.plan_id);
    assert_eq!(snapshot.generation_id, generation_id);
    assert_eq!(snapshot.prompt_preset_id, preset.id);
    assert_eq!(fresh_preview.prompt_preset_revision, stored_preset.revision);
    assert!(!snapshot.prompt_preset_revision_id.is_empty());
    assert_eq!(
        snapshot.prompt_preset_revision_id,
        fresh_preview.prompt_preset_revision_id
    );
    assert_eq!(
        snapshot.model_route_id.as_ref(),
        Some(&target.model_route_id)
    );
    assert_eq!(
        snapshot.generation_preset_id.as_ref(),
        Some(&target.generation_preset_id)
    );
    assert_eq!(
        snapshot
            .plan
            .value
            .get("plan_hash")
            .and_then(serde_json::Value::as_str),
        Some(snapshot.plan_sha256.as_str())
    );
    let resolved: ResolvedPromptPlan =
        serde_json::from_value(snapshot.plan.value.clone()).expect("decode stored resolved plan");
    verify_resolved_prompt_plan(&resolved).expect("verify stored resolved plan");
    assert_eq!(snapshot.plan_sha256, fresh_preview.neutral_plan_hash);
    assert_eq!(snapshot.plan_sha256, resolved.plan_hash);
    assert_eq!(snapshot.input_fingerprint_sha256, fresh_preview.plan_hash);
    assert_eq!(resolved.effective_messages.len(), wire_messages.len());
    assert_eq!(snapshot.provider_request.request.value, wire_body);
    let serialized_snapshot =
        serde_json::to_string(&snapshot).expect("serialize generation prompt plan");
    assert!(!serialized_snapshot.contains(CREDENTIAL_CANARY));

    let mut preset_v2 = preset;
    "Synthetic prompt identity v2".clone_into(&mut preset_v2.name);
    preset_v2.metadata.updated_at = timestamp() + chrono::Duration::seconds(1);
    core.upsert_prompt_preset(&preset_v2, Some(stored_preset.revision))
        .expect("update prompt preset after generation");
    assert_eq!(
        core.get_generation_prompt_plan(&generation_id)
            .expect("snapshot survives preset update"),
        snapshot
    );
    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen Core");
    assert_eq!(
        reopened
            .get_generation_prompt_plan(&generation_id)
            .expect("snapshot survives Core reopen"),
        snapshot
    );
    drop(reopened);
    assert_tree_excludes(root.path(), CREDENTIAL_CANARY.as_bytes());
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the stale-revision fixture must prove every generation-side table rolls back together"
)]
fn stale_knowledge_revision_rejects_the_atomic_generation_append() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic stale knowledge",
            ConversationMode::Chat,
        )
        .expect("create stale-knowledge conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list stale-knowledge branch")
        .into_iter()
        .next()
        .expect("root branch");
    let preset = prompt_preset("synthetic.core.stale-knowledge-preset");
    let stored_preset = core
        .upsert_prompt_preset(&preset, None)
        .expect("save stale-knowledge prompt preset");
    let prompt_preset_revision_id = stored_preset
        .revision_id
        .clone()
        .expect("prompt preset immutable revision id");
    let mut book = knowledge_book();
    let first_book = core
        .upsert_knowledge_book(&book, None)
        .expect("save first knowledge revision");
    let stale_book_revision_id = first_book
        .revision_id
        .clone()
        .expect("first knowledge immutable revision id");

    let generation_id = GenerationId("generation-stale-knowledge".to_owned());
    let mut user = Message::user_after(
        conversation.id.clone(),
        None,
        "Synthetic stale knowledge request",
    );
    user.id = lorepia_domain::MessageId("message-stale-knowledge-user".to_owned());
    let assistant = Message::pending_assistant(
        conversation.id.clone(),
        user.id.clone(),
        generation_id.clone(),
    );
    let resolved = resolve_prompt_plan(&PromptResolveRequest {
        preset: preset.clone(),
        context: PromptResolutionContext {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
            character: CharacterPromptContent {
                character_id: character_id.clone(),
                name: "Ari".to_owned(),
                aliases: Vec::new(),
                description: "Entirely synthetic test character.".to_owned(),
                personality: String::new(),
                scenario: String::new(),
                first_message: String::new(),
                dialogue_examples: Vec::new(),
                system_instruction: String::new(),
                post_history_instruction: String::new(),
                alternate_greetings: Vec::new(),
                knowledge_book_ids: Vec::new(),
                asset_ids: Vec::new(),
            },
            persona: None,
            user_name: "Synthetic User".to_owned(),
            messages: vec![PromptConversationMessage {
                id: user.id.clone(),
                branch_id: branch.id.clone(),
                role: PromptMessageRole::User,
                content: user.content.clone(),
                turn_index: 1,
            }],
            latest_user_message_id: user.id.clone(),
            selected_knowledge: Vec::new(),
            selected_memory: Vec::new(),
            summary_boundaries: Vec::new(),
            conversation_summary: None,
            author_note: None,
            group_context: None,
            variables: VariableMap::default(),
            slots: Vec::new(),
            current_date: "2026-08-03".to_owned(),
            current_time: "12:00".to_owned(),
            supported_capabilities: Vec::new(),
            session_seed: Some(7),
            context_snapshot: None,
        },
        provider: ProviderPromptContract {
            supported_roles: vec![
                ProviderMessageRole::System,
                ProviderMessageRole::User,
                ProviderMessageRole::Assistant,
            ],
            provider_default_role: ProviderMessageRole::User,
            unsupported_role_policy: UnsupportedRolePolicy::MapDeveloperToSystem,
            supports_explicit_cache: false,
            max_cache_boundaries: 0,
        },
        generation_preset_id: None,
        max_context_tokens: 2_048,
        reserved_output_tokens: 256,
    })
    .expect("resolve stale-knowledge prompt");
    verify_resolved_prompt_plan(&resolved).expect("verify stale-knowledge prompt");
    let generation = GenerationRecord {
        id: generation_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        user_message_id: user.id.clone(),
        assistant_message_id: Some(assistant.id.clone()),
        mode: ConversationMode::Chat,
        model: "synthetic-storage-provider".to_owned(),
        model_route_id: None,
        generation_preset_id: None,
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
        started_at: assistant.created_at,
        finished_at: None,
    };
    let plan_value = serde_json::to_value(&resolved).expect("encode resolved prompt");
    let prompt_plan = GenerationPromptPlanRecord {
        id: "plan-stale-knowledge".to_owned(),
        generation_id: generation_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        head_message_id: None,
        latest_user_message_id: user.id.clone(),
        prompt_preset_id: preset.id,
        prompt_preset_revision_id,
        model_route_id: None,
        generation_preset_id: None,
        task_profile_revision_id: None,
        random_seed: resolved.trace.session_seed,
        tokenizer_id: "utf8_bytes_div_4_v1".to_owned(),
        tokenizer_version: "1".to_owned(),
        plan: VersionedJson {
            schema_version: resolved.schema_version,
            value: plan_value,
        },
        plan_sha256: resolved.plan_hash.clone(),
        input_fingerprint_sha256: "55".repeat(32),
        context_limit_tokens: resolved.trace.max_context_tokens,
        estimated_input_tokens: resolved.trace.estimated_input_tokens,
        reserved_output_tokens: resolved.trace.reserved_output_tokens,
        final_input_tokens: resolved.trace.estimated_input_tokens,
        cacheable_prefix_tokens: 0,
        provider_request: ProviderRequestSnapshotRecord {
            id: "provider-request-stale-knowledge".to_owned(),
            api_family: ApiFamily::OpenAiChatCompletions,
            request_schema_version: 1,
            request: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({
                    "model": "synthetic-storage-provider",
                    "messages": [{"role": "user", "content": user.content.clone()}]
                }),
            },
            mapping_diagnostics: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({"fixture": "stale-knowledge"}),
            },
            created_at: assistant.created_at,
        },
        created_at: assistant.created_at,
    };
    let stale_log = KnowledgeActivationLog {
        id: "knowledge-log-stale-revision".to_owned(),
        book_id: book.id.clone(),
        book_revision_id: stale_book_revision_id,
        entry_id: book.entries[0].id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        selected: true,
        reasons: vec![KnowledgeActivationReason::Always],
        estimated_tokens: 5,
        exclusion_reason: None,
        created_at: timestamp(),
    };

    "Synthetic Core knowledge v2".clone_into(&mut book.name);
    let second_book = core
        .upsert_knowledge_book(&book, Some(first_book.revision))
        .expect("advance knowledge book after prompt resolution");
    assert_ne!(
        second_book.revision_id.as_ref(),
        Some(&stale_log.book_revision_id)
    );
    drop(core);

    let storage = Storage::open(root.path()).expect("open storage for atomic append");
    let error = storage
        .append_generation_with_prompt_plan(
            &branch.id,
            None,
            &user,
            &assistant,
            &generation,
            &prompt_plan,
            &[stale_log],
        )
        .expect_err("stale knowledge revision must reject the whole append");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        storage
            .list_branch_messages(&branch.id)
            .expect("branch after stale append")
            .is_empty()
    );
    assert_eq!(
        storage
            .get_generation(&generation_id)
            .expect_err("generation row must roll back")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        storage
            .get_generation_prompt_plan_by_generation(&generation_id)
            .expect_err("prompt plan row must roll back")
            .code,
        CoreErrorCode::NotFound
    );
    let stats = storage
        .orchestration_stats()
        .expect("orchestration row counts after stale append");
    assert_eq!(stats.generations, 0);
    assert_eq!(stats.generation_prompt_plans, 0);
    assert_eq!(stats.knowledge_activation_logs, 0);
}
