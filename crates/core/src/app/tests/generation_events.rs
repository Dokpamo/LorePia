fn wait_for_generation_sequence_watermark(
    core: &Core,
    generation_id: &GenerationId,
    expected: u64,
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while core
        .inner
        .active_generations
        .sequence_watermark_for_test(generation_id)
        != Some(expected)
    {
        assert!(
            Instant::now() < deadline,
            "initial live events did not reach the registry"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn assert_cancelled_subscription_event(
    receiver: &mut broadcast::Receiver<ChatEvent>,
    generation_id: &GenerationId,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match receiver.try_recv() {
            Ok(event) => {
                assert_eq!(&event.generation_id, generation_id);
                assert_eq!(&event.conversation_id, conversation_id);
                assert_eq!(event.branch_id.as_ref(), Some(branch_id));
                if matches!(event.kind, ChatEventKind::GenerationCancelled) {
                    assert_eq!(event.sequence, 4);
                    break;
                }
            }
            Err(broadcast::error::TryRecvError::Empty) => {
                assert!(
                    Instant::now() < deadline,
                    "terminal event was lost at the subscription boundary"
                );
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("terminal subscription failed: {error:?}"),
        }
    }
}

struct PreparingGenerationFixture {
    launch: GenerationLaunchPermit,
    generation: GenerationRecord,
    user: Message,
    assistant: Message,
}

fn prepare_registered_generation(
    core: &Core,
    conversation: &Conversation,
) -> PreparingGenerationFixture {
    prepare_registered_generation_for_model(core, conversation, "synthetic")
        .expect("register preparing generation")
}

fn prepare_registered_generation_for_model(
    core: &Core,
    conversation: &Conversation,
    model: &str,
) -> CoreResult<PreparingGenerationFixture> {
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let user = Message::user(conversation.id.clone(), "prepare atomic subscription");
    let generation_id = GenerationId::new();
    let assistant = Message::pending_assistant(
        conversation.id.clone(),
        user.id.clone(),
        generation_id.clone(),
    );
    let generation = GenerationRecord {
        id: generation_id,
        conversation_id: conversation.id.clone(),
        branch_id: branch,
        user_message_id: user.id.clone(),
        assistant_message_id: Some(assistant.id.clone()),
        mode: ConversationMode::Chat,
        model: model.to_owned(),
        model_route_id: None,
        generation_preset_id: None,
        provider_family: None,
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
    let provider_target = GenerationActionTargetIdentity::DirectModel {
        model_sha256: model.to_owned(),
    };
    let launch = core.prepare_generation_launch_for_target(&generation, &provider_target)?;
    Ok(PreparingGenerationFixture {
        launch,
        generation,
        user,
        assistant,
    })
}

#[test]
fn generation_admission_scopes_provider_and_conversation() {
    let (_provider_root, provider_core, provider_character) = imported_core();
    let mut provider_permits = Vec::with_capacity(MAX_ACTIVE_GENERATIONS_PER_PROVIDER);
    for _ in 0..MAX_ACTIVE_GENERATIONS_PER_PROVIDER {
        let conversation = provider_core
            .open_conversation(&provider_character.id)
            .expect("provider-scope conversation");
        provider_permits.push(
            prepare_registered_generation_for_model(
                &provider_core,
                &conversation,
                "shared-provider-model",
            )
            .expect("generation within provider admission"),
        );
    }
    let overflow_conversation = provider_core
        .open_conversation(&provider_character.id)
        .expect("provider overflow conversation");
    let provider_error = prepare_registered_generation_for_model(
        &provider_core,
        &overflow_conversation,
        "shared-provider-model",
    )
    .err()
    .expect("provider admission must be bounded");
    assert_eq!(provider_error.code, CoreErrorCode::ProviderRateLimited);
    assert!(provider_error.message.contains("provider"));
    drop(provider_permits.pop());
    prepare_registered_generation_for_model(
        &provider_core,
        &overflow_conversation,
        "shared-provider-model",
    )
    .expect("dropping an unlaunched permit releases provider admission");

    let (_conversation_root, conversation_core, conversation_character) = imported_core();
    let conversation = conversation_core
        .open_conversation(&conversation_character.id)
        .expect("conversation-scope conversation");
    let mut conversation_permits = Vec::with_capacity(MAX_ACTIVE_GENERATIONS_PER_CONVERSATION);
    for index in 0..MAX_ACTIVE_GENERATIONS_PER_CONVERSATION {
        conversation_permits.push(
            prepare_registered_generation_for_model(
                &conversation_core,
                &conversation,
                &format!("conversation-model-{index}"),
            )
            .expect("generation within conversation admission"),
        );
    }
    let conversation_error = prepare_registered_generation_for_model(
        &conversation_core,
        &conversation,
        "conversation-overflow-model",
    )
    .err()
    .expect("conversation admission must be bounded");
    assert_eq!(conversation_error.code, CoreErrorCode::ProviderRateLimited);
    assert!(conversation_error.message.contains("conversation"));
    drop(conversation_permits.pop());
    prepare_registered_generation_for_model(
        &conversation_core,
        &conversation,
        "conversation-replacement-model",
    )
    .expect("dropping an unlaunched permit releases conversation admission");
}

#[test]
fn generation_subscription_accepts_durable_running_before_local_activation() {
    let (_root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let fixture = prepare_registered_generation(&core, &conversation);
    assert_eq!(
        core.inner
            .active_generations
            .phase_for_test(&fixture.generation.id),
        Some(GenerationDeliveryPhase::Preparing)
    );
    assert_eq!(
        core.subscribe_generation_events(
            &fixture.generation.id,
            &conversation.id,
            &fixture.generation.branch_id,
        )
        .err()
        .expect("pre-append generation cannot be subscribed")
        .code,
        CoreErrorCode::NotFound
    );

    core.inner
        .storage
        .append_generation(
            &fixture.generation.branch_id,
            None,
            &fixture.user,
            &fixture.assistant,
            &fixture.generation,
        )
        .expect("durably append generation before local activation");
    assert_eq!(
        core.inner
            .storage
            .get_generation(&fixture.generation.id)
            .expect("durable running generation")
            .status,
        GenerationStatus::Running
    );
    assert_eq!(
        core.inner
            .active_generations
            .phase_for_test(&fixture.generation.id),
        Some(GenerationDeliveryPhase::Preparing)
    );

    let subscription = core
        .subscribe_generation_events(
            &fixture.generation.id,
            &conversation.id,
            &fixture.generation.branch_id,
        )
        .expect("durable running generation is authoritative");
    let (_receiver, assistant_message_id, sequence_watermark, display_prefix, reasoning_prefix) =
        subscription.into_parts();
    assert_eq!(sequence_watermark, 0);
    assert_eq!(assistant_message_id, fixture.assistant.id);
    assert!(display_prefix.is_empty());
    assert!(reasoning_prefix.is_empty());
    drop(fixture.launch);
}

#[test]
fn generation_subscription_is_atomic_with_terminal_persistence_and_publish() {
    let (_root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let (provider, provider_started) = StallingProvider::new("in flight");
    let generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "start",
            "stalling".to_owned(),
            None,
            provider,
        )
        .expect("start generation");
    provider_started
        .recv_timeout(Duration::from_secs(2))
        .expect("provider started");
    wait_for_generation_sequence_watermark(&core, &generation_id, 2);

    let generation = core
        .inner
        .storage
        .get_generation(&generation_id)
        .expect("running generation");
    let branch_id = generation.branch_id.clone();
    let wrong_route = core
        .subscribe_generation_events(
            &generation_id,
            &conversation.id,
            &ConversationBranchId("wrong-branch".to_owned()),
        )
        .err()
        .expect("wrong route must not disclose a live generation");
    assert_eq!(wrong_route.code, CoreErrorCode::NotFound);
    let (subscription_entered, subscription_entered_receiver) = std_mpsc::channel();
    let (release_subscription, release_subscription_receiver) = std_mpsc::channel();
    core.inner
        .active_generations
        .pause_next_subscription_for_test(
            &generation_id,
            subscription_entered,
            release_subscription_receiver,
        )
        .expect("install subscription boundary pause");

    let subscribing_core = core.clone();
    let subscribing_generation_id = generation_id.clone();
    let subscribing_conversation_id = conversation.id.clone();
    let subscribing_branch_id = branch_id.clone();
    let subscription = thread::spawn(move || {
        subscribing_core.subscribe_generation_events(
            &subscribing_generation_id,
            &subscribing_conversation_id,
            &subscribing_branch_id,
        )
    });
    subscription_entered_receiver
        .recv_timeout(Duration::from_secs(2))
        .expect("subscription reached the receiver boundary");

    core.cancel_generation(&generation_id)
        .expect("cancel generation while subscription is paused");
    wait_for_generation_status(&core, &generation_id, GenerationStatus::Cancelled);
    release_subscription
        .send(())
        .expect("release subscription boundary");

    let subscription = subscription
        .join()
        .expect("subscription thread")
        .expect("atomic live subscription");
    let (mut receiver, assistant_message_id, sequence_watermark, display_prefix, reasoning_prefix) =
        subscription.into_parts();
    assert_eq!(sequence_watermark, 2);
    assert_eq!(display_prefix, "in flight");
    assert!(reasoning_prefix.is_empty());
    assert_eq!(
        Some(assistant_message_id),
        generation.assistant_message_id.clone()
    );
    assert_cancelled_subscription_event(
        &mut receiver,
        &generation_id,
        &conversation.id,
        &branch_id,
    );
    wait_for_generation_registry_to_drain(&core);
    let terminal = core
        .subscribe_generation_events(&generation_id, &conversation.id, &branch_id)
        .err()
        .expect("terminal generation cannot create an empty live subscription");
    assert_eq!(terminal.code, CoreErrorCode::NotFound);
}

#[test]
fn generation_subscription_recovers_uncheckpointed_reasoning_and_text_prefixes() {
    for preserve_partial in [false, true] {
        let (_root, core, character) = imported_core();
        let mut settings = core.get_settings().expect("load settings");
        settings.preserve_partial_generations = preserve_partial;
        core.update_settings(&settings)
            .expect("configure durable partial checkpoints");
        let conversation = core.open_conversation(&character.id).expect("conversation");
        let (provider, provider_started, release_provider) = CatchupSnapshotProvider::new();
        let generation_id = core
            .send_message_with_provider(
                &conversation.id,
                "start",
                "catch-up".to_owned(),
                None,
                provider,
            )
            .expect("start generation");
        let catchup_started_at = Instant::now();
        provider_started
            .recv_timeout(Duration::from_secs(2))
            .expect("provider emitted pre-subscription prefixes");
        wait_for_generation_sequence_watermark(&core, &generation_id, 3);
        if preserve_partial {
            assert!(
                catchup_started_at.elapsed() < PARTIAL_CHECKPOINT_INTERVAL,
                "the regression must subscribe before the 500 ms durable checkpoint"
            );
        }

        let generation = core
            .inner
            .storage
            .get_generation(&generation_id)
            .expect("running generation");
        let persisted_assistant = core
            .list_branch_messages(&generation.branch_id)
            .expect("durable branch messages")
            .into_iter()
            .find(|message| message.generation_id.as_ref() == Some(&generation_id))
            .expect("pending assistant");
        assert_eq!(persisted_assistant.status, MessageStatus::Pending);
        assert!(
            persisted_assistant.content.is_empty(),
            "the live prefixes must not rely on a durable partial checkpoint (preserve_partial={preserve_partial})"
        );

        let subscription = core
            .subscribe_generation_events(&generation_id, &conversation.id, &generation.branch_id)
            .expect("subscribe after live prefixes");
        let (
            mut receiver,
            assistant_message_id,
            sequence_watermark,
            display_prefix,
            reasoning_prefix,
        ) = subscription.into_parts();
        assert_eq!(assistant_message_id, persisted_assistant.id);
        assert_eq!(sequence_watermark, 3);
        assert_eq!(display_prefix, "text-prefix");
        assert_eq!(reasoning_prefix, "reasoning-prefix");
        release_provider.send(()).expect("release provider suffix");
        wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);

        let mut reasoning_suffix = String::new();
        let mut text_suffix = String::new();
        while let Ok(event) = receiver.try_recv() {
            match event.kind {
                ChatEventKind::ReasoningDelta(delta) => reasoning_suffix.push_str(&delta),
                ChatEventKind::TextDelta(delta) => text_suffix.push_str(&delta),
                _ => {}
            }
        }
        assert_eq!(
            (
                format!("{reasoning_prefix}{reasoning_suffix}"),
                format!("{display_prefix}{text_suffix}"),
            ),
            (
                "reasoning-prefix+reasoning-suffix".to_owned(),
                "text-prefix+text-suffix".to_owned(),
            ),
            "reattachment must reconstruct the exact live prefix plus suffix (preserve_partial={preserve_partial})",
        );
    }
}

#[test]
fn live_generation_prefix_accepts_the_maximum_valid_display_transform() {
    let mut transform_set = display_only_generation_transform_set();
    transform_set.rules[0].pattern = SafeRegex {
        pattern: "x".to_owned(),
        case_insensitive: false,
    };
    transform_set.rules[0].replacement =
        "😀".repeat(lorepia_orchestration::DEFAULT_MAX_REPLACEMENT_CHARS);
    transform_set.rules[0].max_replacements = 32;
    transform_set.rules[0].input_limit = 32;
    transform_set.rules[0].output_limit =
        u32::try_from(MAX_LIVE_DISPLAY_PREFIX_CHARS).expect("display char cap fits u32");
    transform_set.max_output_chars =
        u32::try_from(MAX_LIVE_DISPLAY_PREFIX_CHARS).expect("display char cap fits u32");
    let context = GenerationTransformContext {
        sets: vec![transform_set],
        variables: VariableMap::default(),
        supported_capabilities: Vec::new(),
        approved_import_source_ids: std::collections::BTreeSet::new(),
        display_context: None,
    };
    let (_, projection) = apply_generation_output_transforms(
        Ok(GenerationOutcome {
            text: "x".repeat(32),
            usage: GenerationUsage::default(),
            opaque_reasoning_state: Vec::new(),
            last_sequence: 2,
        }),
        &context,
    );
    let display = projection
        .expect("valid maximum DisplayOnly projection")
        .display_content;
    assert_eq!(display.chars().count(), MAX_LIVE_DISPLAY_PREFIX_CHARS);
    assert_eq!(display.len(), MAX_LIVE_DISPLAY_PREFIX_BYTES);

    let mut prefix = GenerationLivePrefix::default();
    let reasoning = "r".repeat(MAX_GENERATED_OUTPUT_CHARS);
    assert!(prefix.append(&ChatEventKind::ReasoningDelta(reasoning)));
    assert!(prefix.append(&ChatEventKind::TextDelta(display)));
    assert_eq!(prefix.reasoning_chars, MAX_GENERATED_OUTPUT_CHARS);
    assert_eq!(prefix.display_chars, MAX_LIVE_DISPLAY_PREFIX_CHARS);
    assert_eq!(prefix.display.len(), MAX_LIVE_DISPLAY_PREFIX_BYTES);
    assert!(!prefix.append(&ChatEventKind::TextDelta("overflow".to_owned())));
    assert!(!prefix.append(&ChatEventKind::ReasoningDelta("overflow".to_owned())));
}

#[test]
fn usage_overflow_is_compensated_as_failed_and_allows_the_next_send() {
    let (root, core, character) = imported_core();
    let conversation = core.open_conversation(&character.id).expect("conversation");
    let mut events = core.subscribe_events();
    let secret = "credential-must-not-leak";
    let failed_generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "first",
            "overflow".to_owned(),
            Some(secret.to_owned()),
            Arc::new(OverflowUsageProvider),
        )
        .expect("start overflow generation");

    let failed_generation =
        wait_for_generation_status(&core, &failed_generation_id, GenerationStatus::Failed);
    wait_for_generation_registry_to_drain(&core);
    assert_eq!(failed_generation.input_tokens, None);
    assert_eq!(failed_generation.output_tokens, None);
    assert_eq!(
        failed_generation.error_code.as_deref(),
        Some(CoreErrorCode::StorageUnavailable.as_str())
    );
    assert!(failed_generation.finished_at.is_some());

    let failed_messages = core
        .list_messages(&conversation.id)
        .expect("failed messages");
    assert_eq!(failed_messages.len(), 2);
    assert_eq!(failed_messages[1].status, MessageStatus::Failed);
    assert_eq!(failed_messages[1].content, "response before invalid usage");

    let observed = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
    assert!(observed.iter().any(|event| {
        matches!(
            &event.kind,
            ChatEventKind::GenerationFailed { code, message }
                if code == CoreErrorCode::StorageUnavailable.as_str()
                    && message == GENERATION_PERSISTENCE_FAILURE_MESSAGE
        )
    }));
    assert!(
        !format!("{observed:?}").contains(secret),
        "generation events must not expose credentials"
    );

    drop(core);
    let core = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    assert_eq!(
        core.inner
            .storage
            .get_generation(&failed_generation_id)
            .expect("restored failed generation")
            .status,
        GenerationStatus::Failed
    );
    assert_eq!(
        core.list_messages(&conversation.id)
            .expect("restored failed messages")[1]
            .status,
        MessageStatus::Failed
    );

    let next_generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "second",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("retry succeeded")),
        )
        .expect("start retry generation");
    wait_for_generation_status(&core, &next_generation_id, GenerationStatus::Complete);
    let messages = core
        .list_messages(&conversation.id)
        .expect("messages after retry");
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1].status, MessageStatus::Failed);
    assert_eq!(messages[3].status, MessageStatus::Complete);
    assert_eq!(messages[3].content, "retry succeeded");
    assert!(
        messages
            .iter()
            .all(|message| message.status != MessageStatus::Pending)
    );
}
