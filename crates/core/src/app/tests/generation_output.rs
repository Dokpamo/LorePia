#[test]
fn provider_output_limit_failure_obeys_the_partial_persistence_policy() {
    let conversation_id = ConversationId::new();
    let parent_id = lorepia_domain::MessageId::new();
    let generation_id = GenerationId::new();
    let failure = GenerationFailure {
        error: CoreError::new(
            CoreErrorCode::ProviderUnavailable,
            lorepia_chat::OUTPUT_LIMIT_ERROR_MESSAGE,
            false,
        ),
        partial_text: "safe prefix 😀".to_owned(),
        last_sequence: 7,
    };

    let mut preserved = Message::pending_assistant(
        conversation_id.clone(),
        parent_id.clone(),
        generation_id.clone(),
    );
    let (sequence, terminal, should_commit) =
        apply_generation_result(&mut preserved, Err(failure.clone()), true);
    assert_eq!(sequence, 8);
    assert_eq!(preserved.status, MessageStatus::Failed);
    assert_eq!(preserved.content, "safe prefix 😀");
    assert!(should_commit);
    assert!(matches!(
        terminal,
        ChatEventKind::GenerationFailed { code, message }
            if code == "provider_unavailable"
                && message == lorepia_chat::OUTPUT_LIMIT_ERROR_MESSAGE
    ));

    let mut discarded = Message::pending_assistant(conversation_id, parent_id, generation_id);
    let (_, _, should_commit) = apply_generation_result(&mut discarded, Err(failure), false);
    assert!(!should_commit);
}

#[test]
fn static_provider_persists_assistant_message() {
    let (root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let mut events = core.subscribe_events();
    let generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "Hello",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("Hi there")),
        )
        .expect("send");

    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let messages = core.list_messages(&conversation.id).expect("messages");
        if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
            assert_eq!(messages[1].content, "Hi there");
            break;
        }
        assert!(Instant::now() < deadline, "generation timed out");
        thread::sleep(Duration::from_millis(10));
    }

    wait_for_generation_registry_to_drain(&core);
    let events = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
    let committed = events
        .iter()
        .position(|event| matches!(event.kind, ChatEventKind::MessageCommitted { .. }))
        .expect("message committed event");
    let finished = events
        .iter()
        .position(|event| matches!(event.kind, ChatEventKind::GenerationFinished))
        .expect("generation finished event");
    assert!(committed < finished);
    assert!(events.windows(2).all(|events| {
        events[0].generation_id != events[1].generation_id
            || events[0].sequence < events[1].sequence
    }));
    let state = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state");
    let generation = core
        .inner
        .storage
        .get_generation(&generation_id)
        .expect("generation snapshot");
    assert_eq!(generation.mode, ConversationMode::Chat);
    assert!(events.iter().all(|event| {
        event.branch_id.as_ref() == Some(&state.active_branch_id)
            && event.assistant_message_id.as_ref() == generation.assistant_message_id.as_ref()
    }));

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
    let restored = reopened
        .list_messages(&conversation.id)
        .expect("restored messages");
    assert_eq!(restored.len(), 2);
    assert_eq!(restored[1].content, "Hi there");
}

#[test]
fn display_only_terminal_stream_matches_hash_verified_reopen_projection() {
    let (root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let transform_set = display_only_generation_transform_set();
    let transform_revision_id = install_generation_transform_fixture(
        &core,
        &conversation.id,
        &transform_set,
        "synthetic.display-only.preset",
        "synthetic.display-only.binding",
    );

    let mut events = core.subscribe_events();
    let generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "Render the synthetic projection",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("Synthetic reply")),
        )
        .expect("start DisplayOnly generation");
    wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let events = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
    let streamed_display = assert_display_only_events(&events, &generation_id);
    let canonical = core
        .list_messages(&conversation.id)
        .expect("canonical messages");
    assert_eq!(canonical[1].content, "Synthetic reply");
    let projected = core
        .list_message_presentations(&conversation.id)
        .expect("projected messages");
    assert_display_only_projection(&projected[1], &transform_revision_id, &streamed_display);

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    let canonical_after_reopen = reopened
        .list_messages(&conversation.id)
        .expect("reopened canonical messages");
    assert_eq!(canonical_after_reopen[1].content, "Synthetic reply");
    let projected_after_reopen = reopened
        .list_message_presentations(&conversation.id)
        .expect("reopened projected messages");
    assert_eq!(projected_after_reopen[1].display_content, streamed_display);
    assert_eq!(
        projected_after_reopen[1].display_content_sha256,
        transform_content_sha256(&streamed_display)
    );
    assert_eq!(
        projected_after_reopen[1].projection_diagnostics_sha256,
        projected[1].projection_diagnostics_sha256
    );
    assert_eq!(
        projected_after_reopen[1].transform_diagnostics,
        projected[1].transform_diagnostics
    );
}

#[test]
fn generation_transform_failures_preserve_provider_text_and_reopen_diagnostics() {
    const PROVIDER_TEXT: &str = "Synthetic reply";
    let (root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let transform_set = fail_open_generation_transform_set();
    install_generation_transform_fixture(
        &core,
        &conversation.id,
        &transform_set,
        "synthetic.fail-open.preset",
        "synthetic.fail-open.binding",
    );

    let mut events = core.subscribe_events();
    let generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "Exercise transform failure",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new(PROVIDER_TEXT)),
        )
        .expect("start fail-open generation");
    wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let generation_events = std::iter::from_fn(|| events.try_recv().ok())
        .filter(|event| event.generation_id == generation_id)
        .collect::<Vec<_>>();
    assert!(generation_events.iter().any(|event| {
        matches!(&event.kind, ChatEventKind::TextDelta(text) if text == PROVIDER_TEXT)
    }));
    assert!(
        generation_events
            .iter()
            .any(|event| matches!(event.kind, ChatEventKind::GenerationFinished))
    );

    let canonical = core
        .list_messages(&conversation.id)
        .expect("canonical messages");
    assert_eq!(canonical[1].content, PROVIDER_TEXT);
    assert_eq!(canonical[1].status, MessageStatus::Complete);
    let projected = core
        .list_message_presentations(&conversation.id)
        .expect("fail-open projection");
    assert_eq!(projected[1].display_content, PROVIDER_TEXT);
    assert!(projected[1].projection_diagnostics_sha256.is_some());
    assert_eq!(projected[1].transform_diagnostics.len(), 2);
    let invalid = projected[1]
        .transform_diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.rule_id.as_deref() == Some("synthetic.fail-open.invalid-regex")
        })
        .expect("invalid-regex diagnostic");
    assert_eq!(invalid.disposition, MessageTransformDisposition::Failed);
    assert_eq!(invalid.code.as_deref(), Some("invalid_regex"));
    let limited = projected[1]
        .transform_diagnostics
        .iter()
        .find(|diagnostic| {
            diagnostic.rule_id.as_deref() == Some("synthetic.fail-open.output-limit")
        })
        .expect("output-limit diagnostic");
    assert_eq!(
        limited.disposition,
        MessageTransformDisposition::LimitRejected
    );
    assert_eq!(limited.code.as_deref(), Some("output_limit_exceeded"));
    assert!(
        projected[1]
            .transform_diagnostics
            .iter()
            .all(
                |diagnostic| diagnostic.before_sha256 == transform_content_sha256(PROVIDER_TEXT)
                    && diagnostic.after_sha256.is_none()
            )
    );

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen fail-open core");
    let canonical_after_reopen = reopened
        .list_messages(&conversation.id)
        .expect("reopened canonical messages");
    assert_eq!(canonical_after_reopen[1].content, PROVIDER_TEXT);
    assert_eq!(
        reopened
            .list_message_presentations(&conversation.id)
            .expect("reopened fail-open projection")[1],
        projected[1]
    );
}

#[test]
fn prompt_preview_materializes_exact_current_room_sources_and_content_free_snapshot() {
    let (_root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let branch_id = core
        .get_conversation_state(&conversation.id)
        .expect("prompt-source room state")
        .active_branch_id;
    let first = core
        .send_message_with_provider(
            &conversation.id,
            "SUMMARY_RANGE_USER_CANARY_31A7",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("SUMMARY_RANGE_ASSISTANT_CANARY_72D4")),
        )
        .expect("start summary-range generation");
    wait_for_generation_status(&core, &first, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let first_turn = core
        .list_branch_messages(&branch_id)
        .expect("summary-range messages");
    let summary = save_prompt_source_summary(&core, &branch_id, &first_turn);
    let second = core
        .send_message_with_provider(
            &conversation.id,
            "SINCE_SUMMARY_USER_CANARY_54C9",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("SINCE_SUMMARY_ASSISTANT_CANARY_86E2")),
        )
        .expect("start since-summary generation");
    wait_for_generation_status(&core, &second, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let messages = core
        .list_branch_messages(&branch_id)
        .expect("complete prompt-source history");
    let binding =
        bind_prompt_source_test_preset(&core, &conversation.id, &branch_id, &summary.value.id);

    let (template, route) =
        create_built_in_public_route(&core, "openai-responses-v1", "/v1", "source-fixture");
    let generation_preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
        .expect("save prompt-source generation preset");
    let request = crate::PromptPlanRequest {
        conversation_id: conversation.id.clone(),
        branch_id: branch_id.clone(),
        expected_head: Some(messages[3].id.clone()),
        user_text: "LATEST_USER_SOURCE_CANARY_03F8".to_owned(),
        generation_target: GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: generation_preset.id,
        },
        prompt_preset_id: None,
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };
    let preview = core
        .resolve_prompt_preview(
            &request,
            new_test_generation_operation("prompt-source-preview-v1"),
        )
        .expect("resolve exact prompt-source preview");
    assert_prompt_source_preview(&preview);

    let trace = core
        .explain_prompt_plan(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview.generation_attempt_id,
            },
            &preview.plan.plan_hash,
        )
        .expect("explain exact prompt-source plan");
    let snapshot = trace
        .context_snapshot
        .expect("typed prompt context snapshot");
    assert_prompt_source_snapshot(
        &snapshot,
        &conversation.id,
        &branch_id,
        &messages,
        &summary,
        &binding,
    );
}
