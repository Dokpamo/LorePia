#[test]
fn regenerate_revalidates_copied_user_text_before_creating_a_branch() {
    let (_root, core, character) = imported_core();
    for (index, invalid_text) in ["   ".to_owned(), "x".repeat(MAX_USER_MESSAGE_BYTES + 1)]
        .into_iter()
        .enumerate()
    {
        let conversation = core
            .create_conversation(
                &character.id,
                format!("비정상 원본 {index}"),
                ConversationMode::Chat,
            )
            .expect("conversation");
        let state = core
            .get_conversation_state(&conversation.id)
            .expect("state");
        let user = Message::user(conversation.id.clone(), invalid_text);
        let generation_id = GenerationId::new();
        let pending = Message::pending_assistant(
            conversation.id.clone(),
            user.id.clone(),
            generation_id.clone(),
        );
        let generation = GenerationRecord {
            id: generation_id,
            conversation_id: conversation.id.clone(),
            branch_id: state.active_branch_id.clone(),
            user_message_id: user.id.clone(),
            assistant_message_id: Some(pending.id.clone()),
            mode: ConversationMode::Chat,
            model: "synthetic".to_owned(),
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
            started_at: pending.created_at,
            finished_at: None,
        };
        core.inner
            .storage
            .append_generation(&state.active_branch_id, None, &user, &pending, &generation)
            .expect("append abnormal legacy generation");
        let mut assistant = pending;
        assistant.content = "legacy response".to_owned();
        assistant.status = MessageStatus::Complete;
        core.inner
            .storage
            .finalize_generation(&assistant, None, None, true)
            .expect("finalize abnormal legacy generation");

        let branches_before = core
            .list_conversation_branches(&conversation.id)
            .expect("branches before");
        let messages_before = core
            .list_messages(&conversation.id)
            .expect("messages before");
        let error = core
            .regenerate_assistant_message_with_provider(
                &conversation.id,
                &state.active_branch_id,
                Some(&assistant.id),
                &assistant.id,
                "unused".to_owned(),
                None,
                Arc::new(StaticProvider::new("unused")),
            )
            .expect_err("invalid copied user text");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            core.list_conversation_branches(&conversation.id)
                .expect("unchanged branches"),
            branches_before
        );
        assert_eq!(
            core.list_messages(&conversation.id)
                .expect("unchanged messages"),
            messages_before
        );
    }
}

#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "one operating-path test keeps the async send and immutable action sequence visible"
)]
async fn async_generation_operating_paths_send_edit_and_regenerate() {
    let (_root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "Async generation paths",
            ConversationMode::Chat,
        )
        .expect("conversation");
    let source_branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let broker = RejectingTaskCredentialBroker;

    let (send_provider, send_prompt) = CapturingProvider::new("async send reply");
    let send_temporal_context =
        direct_model_temporal_context("async-send-model").expect("direct async send authority");
    let generation_id = core
        .send_message_to_branch_with_provider_options_and_contract_async(
            &conversation.id,
            &source_branch,
            None,
            ConversationMode::Chat,
            "async send question",
            new_test_generation_operation("async-send-v1"),
            "async-send-model".to_owned(),
            None,
            None,
            false,
            Some(1.0),
            Some(CORE_MAX_OUTPUT_TOKENS),
            &VariableMap::default(),
            None,
            None,
            false,
            None,
            send_provider,
            None,
            send_temporal_context,
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect("async send");
    let send_request = send_prompt
        .recv_timeout(Duration::from_secs(2))
        .expect("captured async send prompt");
    assert!(
        send_request
            .iter()
            .any(|message| message == "async send question")
    );
    wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let original = core
        .list_branch_messages(&source_branch)
        .expect("source messages");
    assert_eq!(original.len(), 2);
    assert_eq!(original[1].content, "async send reply");

    let edit_model = "async-edit-model";
    let (edit_provider, edit_prompt) = CapturingProvider::new("async edit reply");
    let edited = core
        .start_message_generation_action_with_provider_async(
            &conversation.id,
            &source_branch,
            Some(&original[1].id),
            &original[0].id,
            MessageGenerationAction::EditUser,
            Some("async edited question"),
            new_test_generation_operation("async-edit-v1"),
            GenerationActionTargetIdentity::DirectModel {
                model_sha256: format!("{:x}", Sha256::digest(edit_model.as_bytes())),
            },
            edit_model.to_owned(),
            None,
            edit_provider,
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect("async edit");
    let edit_request = edit_prompt
        .recv_timeout(Duration::from_secs(2))
        .expect("captured async edit prompt");
    assert!(
        edit_request
            .iter()
            .any(|message| message == "async edited question")
    );
    assert!(
        !edit_request
            .iter()
            .any(|message| message == "async send question")
    );
    wait_for_generation_status(&core, &edited.generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let edited_messages = core
        .list_branch_messages(&edited.branch.id)
        .expect("edited branch");
    assert_eq!(
        edited_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        ["async edited question", "async edit reply"]
    );

    core.select_conversation_branch(&conversation.id, &source_branch)
        .expect("restore source branch");
    let regenerate_model = "async-regenerate-model";
    let (regenerate_provider, regenerate_prompt) =
        CapturingProvider::new("async regenerated reply");
    let regenerated = core
        .start_message_generation_action_with_provider_async(
            &conversation.id,
            &source_branch,
            Some(&original[1].id),
            &original[1].id,
            MessageGenerationAction::RegenerateAssistant,
            None,
            new_test_generation_operation("async-regenerate-v1"),
            GenerationActionTargetIdentity::DirectModel {
                model_sha256: format!("{:x}", Sha256::digest(regenerate_model.as_bytes())),
            },
            regenerate_model.to_owned(),
            None,
            regenerate_provider,
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect("async regenerate");
    let regenerate_request = regenerate_prompt
        .recv_timeout(Duration::from_secs(2))
        .expect("captured async regenerate prompt");
    assert!(
        regenerate_request
            .iter()
            .any(|message| message == "async send question")
    );
    assert!(
        !regenerate_request
            .iter()
            .any(|message| message == "async send reply")
    );
    wait_for_generation_status(
        &core,
        &regenerated.generation_id,
        GenerationStatus::Complete,
    );
    wait_for_generation_registry_to_drain(&core);
    let regenerated_messages = core
        .list_branch_messages(&regenerated.branch.id)
        .expect("regenerated branch");
    assert_eq!(
        regenerated_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        ["async send question", "async regenerated reply"]
    );
    assert_eq!(
        core.list_branch_messages(&source_branch)
            .expect("preserved source branch"),
        original
    );
}

#[tokio::test]
async fn async_prompt_preview_resolves_without_chat_mutation() {
    let (_root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "Async prompt preview",
            ConversationMode::Chat,
        )
        .expect("conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let (template, route) = create_built_in_public_route(
        &core,
        "openai-responses-v1",
        "/v1",
        "gpt-async-preview-fixture",
    );
    let preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
        .expect("generation preset");
    let target = GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: preset.id,
    };
    let preview = core
        .resolve_prompt_preview_async(
            &crate::PromptPlanRequest {
                conversation_id: conversation.id.clone(),
                branch_id: branch.clone(),
                expected_head: None,
                user_text: "async preview question".to_owned(),
                generation_target: target.clone(),
                prompt_preset_id: None,
                variable_overrides: VariableMap::default(),
                expected_plan_hash: None,
            },
            new_test_generation_operation("async-preview-v1"),
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("async prompt preview");

    assert_eq!(preview.plan.generation_target.as_ref(), Some(&target));
    assert!(
        preview
            .effective_messages
            .iter()
            .any(|message| message.content == "async preview question")
    );
    assert!(
        core.list_branch_messages(&branch)
            .expect("preview must remain read-only")
            .is_empty()
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one vertical proves approval, temporal replay, variable/knowledge resolution, and atomic materialization"
)]
fn expert_preview_reuses_attempt_owned_before_generation_for_reviewed_send() {
    let (_root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "Attempt-owned expert preview",
            ConversationMode::Chat,
        )
        .expect("create attempt-owned preview conversation");
    let branch_id = core
        .get_conversation_state(&conversation.id)
        .expect("load attempt-owned preview room state")
        .active_branch_id;
    let (variable, temporal_roll, knowledge_entry_id, proposal_id) =
        install_prompt_attempt_parity_module(
            &core,
            ContentModuleRuntimeTarget {
                conversation_id: conversation.id.clone(),
                branch_id: branch_id.clone(),
            },
        );

    let api_origin =
        CanonicalOrigin::parse("http://127.0.0.1:9").expect("synthetic loopback origin");
    let (template, connection) = create_openai_chat_connection(&core, &api_origin);
    let connection_id = connection.id.clone();
    let now = Utc::now();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from("synthetic-prompt-attempt-route"),
            connection_id: connection.id,
            api_family: template.api_family,
            model_id: "synthetic-prompt-attempt-model".to_owned(),
            display_name: Some("Synthetic prompt-attempt model".to_owned()),
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
        .expect("save prompt-attempt route");
    let generation_preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
        .expect("save prompt-attempt generation preset");
    let idle_branch_updated_at = core
        .inner
        .storage
        .get_conversation_branch(&branch_id)
        .expect("load idle branch before review")
        .updated_at;
    thread::sleep(Duration::from_millis(1_100));
    let request = crate::PromptPlanRequest {
        conversation_id: conversation.id.clone(),
        branch_id: branch_id.clone(),
        expected_head: None,
        user_text: "Synthetic attempt-owned preview request".to_owned(),
        generation_target: GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: generation_preset.id,
        },
        prompt_preset_id: None,
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };

    let awaiting = core
        .resolve_prompt_preview(
            &request,
            new_test_generation_operation("attempt-owned-preview-v1"),
        )
        .expect_err("attempt-owned approval must block the first final preview");
    assert_eq!(
        awaiting.code,
        CoreErrorCode::PermissionDenied,
        "unexpected first-preview failure: {awaiting:?}"
    );
    assert!(
        core.list_branch_messages(&branch_id)
            .expect("messages before attempt approval")
            .is_empty(),
        "preview must not append chat rows"
    );
    assert!(
        core.list_interaction_effect_history(&conversation.id, &branch_id, None, 100)
            .expect("live effects before attempt approval")
            .is_empty(),
        "attempt-owned BeforeGeneration effects must remain isolated"
    );

    let proposals = core
        .list_generation_attempt_proposals_for_source_room(
            &conversation.id,
            &branch_id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list attempt-owned proposals");
    let [proposal] = proposals.as_slice() else {
        panic!("expected one attempt-owned proposal, got {proposals:?}");
    };
    assert_eq!(proposal.proposal.record.proposal_id, proposal_id);
    let generation_id = proposal.proposal.generation_id.clone();
    let attempt_created_at = core
        .inner
        .storage
        .get_generation_attempt(&generation_id)
        .expect("load isolated prompt attempt")
        .created_at;
    assert!(attempt_created_at > idle_branch_updated_at);
    let event_time_text = proposal.proposal.record.body.clone();
    assert!(
        event_time_text.ends_with("+00:00"),
        "approval body must retain the attempt's explicit UTC event time"
    );

    let live_before_send = core
        .inner
        .storage
        .get_interaction_state_snapshot(&conversation.id, &branch_id)
        .expect("live interaction state before attempt decision");
    assert_eq!(
        live_before_send.state.variables.get(&variable),
        Some(&VariableValue::Text("initial".to_owned()))
    );
    assert_eq!(
        live_before_send.state.variables.get(&temporal_roll),
        Some(&VariableValue::Integer(0))
    );
    assert!(
        !live_before_send
            .state
            .manually_active_knowledge
            .contains(&knowledge_entry_id)
    );

    let decision = core
        .decide_generation_attempt_proposal(
            &crate::orchestration_runtime::GenerationAttemptProposalDecisionRequest {
                conversation_id: conversation.id.clone(),
                source_branch_id: branch_id.clone(),
                generation_id: generation_id.clone(),
                proposal_record_id: proposal.proposal.record.id.clone(),
                expected_aggregate_revision: proposal.aggregate_revision,
                expected_proposal_revision: proposal.proposal.proposal_revision,
                decision: InteractionProposalDecision::Approve,
            },
        )
        .expect("approve attempt-owned proposal");
    assert_eq!(decision.pending_proposal_count, 0);
    let approved_aggregate = core
        .inner
        .storage
        .get_generation_attempt_interaction_aggregate(&generation_id)
        .expect("load approved attempt-owned interaction aggregate");
    assert!(
        approved_aggregate
            .state
            .manually_active_knowledge
            .contains(&knowledge_entry_id),
        "approved attempt aggregate must retain manual knowledge activation"
    );
    assert!(
        core.list_interaction_effect_history(&conversation.id, &branch_id, None, 100)
            .expect("live effects after isolated approval")
            .is_empty(),
        "approval must still leave live state untouched before append"
    );

    let preview = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &generation_id,
            },
        )
        .expect("resolve final attempt-owned expert preview");
    assert_eq!(preview.generation_attempt_id, generation_id);
    assert_eq!(
        preview,
        core.resolve_prompt_preview(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &generation_id,
            },
        )
        .expect("repeat exact attempt-owned expert preview"),
        "re-preview must reuse the same temporal interaction aggregate"
    );
    let preview_text = preview
        .effective_messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(preview_text.contains("SYNTHETIC_ATTEMPT_VARIABLE=approved-for-prompt"));
    assert!(
        preview_text.contains("SYNTHETIC_ATTEMPT_MANUAL_KNOWLEDGE_6A91"),
        "expert preview omitted attempt-owned knowledge:\n{preview_text}"
    );
    assert!(preview_text.contains(&format!(
        "SYNTHETIC_ATTEMPT_DATE={}",
        attempt_created_at.format("%Y-%m-%d")
    )));
    assert!(preview_text.contains(&format!(
        "SYNTHETIC_ATTEMPT_TIME={}",
        attempt_created_at.format("%H:%M:%S%:z")
    )));
    assert!(!preview_text.contains(&format!(
        "SYNTHETIC_ATTEMPT_TIME={}",
        idle_branch_updated_at.format("%H:%M:%S%:z")
    )));
    let temporal_roll_value = preview_text
        .split("SYNTHETIC_ATTEMPT_TIME_ROLL=")
        .nth(1)
        .and_then(|suffix| suffix.lines().next())
        .and_then(|line| line.split(';').next())
        .and_then(|value| value.trim().parse::<i64>().ok())
        .expect("expert preview contains the attempt-time-seeded roll");
    assert!((1..=10_000).contains(&temporal_roll_value));

    let mut reviewed = request;
    reviewed.expected_plan_hash = Some(preview.plan.plan_hash.clone());
    let credential_authority = install_provider_credential_authority(&core, &connection_id);
    let stale_attempt = core
        .send_message_with_prompt_plan(
            &reviewed,
            &GenerationId::new(),
            ConnectionBoundCredential::new_with_access_authority(
                connection_id.clone(),
                Some("synthetic-attempt-credential".to_owned()),
                credential_authority.clone(),
            ),
        )
        .expect_err("a reviewed send cannot substitute another attempt token");
    assert_eq!(stale_attempt.code, CoreErrorCode::InvalidInput);
    assert!(
        core.list_branch_messages(&branch_id)
            .expect("messages after stale attempt rejection")
            .is_empty()
    );
    let dispatched_generation_id = core
        .send_message_with_prompt_plan(
            &reviewed,
            &preview.generation_attempt_id,
            ConnectionBoundCredential::new_with_access_authority(
                connection_id,
                Some("synthetic-attempt-credential".to_owned()),
                credential_authority,
            ),
        )
        .expect("send exact attempt-owned expert preview");
    assert_eq!(dispatched_generation_id, generation_id);

    let stored_plan = core
        .get_generation_prompt_plan(&dispatched_generation_id)
        .expect("load attempt-owned generation prompt plan");
    let stored_attempt = core
        .inner
        .storage
        .get_generation_attempt(&generation_id)
        .expect("load attempt-owned semantic fingerprint");
    assert_eq!(stored_plan.id, preview.plan.plan_id);
    assert_eq!(stored_plan.plan_sha256, preview.plan.neutral_plan_hash);
    assert_eq!(
        stored_plan.random_seed,
        Some(reviewed_prompt_session_seed(
            &stored_attempt.input.base_request_fingerprint_sha256,
        ))
    );
    assert_eq!(
        stored_plan.provider_request.request.value,
        preview.provider_request
    );
    let resolved: ResolvedPromptPlan = serde_json::from_value(stored_plan.plan.value)
        .expect("decode stored attempt-owned resolved plan");
    let stored_messages = resolved
        .effective_messages
        .iter()
        .map(|message| {
            (
                message.sequence,
                message.block_id.clone(),
                message.content.clone(),
            )
        })
        .collect::<Vec<_>>();
    let preview_messages = preview
        .effective_messages
        .iter()
        .map(|message| {
            (
                message.sequence,
                message.block_id.clone(),
                message.content.clone(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(stored_messages, preview_messages);

    let live_after_send = core
        .inner
        .storage
        .get_interaction_state_snapshot(&conversation.id, &branch_id)
        .expect("live interaction state after atomic append");
    assert_eq!(
        live_after_send.state.variables.get(&variable),
        Some(&VariableValue::Text("approved-for-prompt".to_owned()))
    );
    assert_eq!(
        live_after_send.state.variables.get(&temporal_roll),
        Some(&VariableValue::Integer(temporal_roll_value))
    );
    assert!(
        live_after_send
            .state
            .manually_active_knowledge
            .contains(&knowledge_entry_id)
    );
    let visible_times = core
        .list_interaction_effect_history(&conversation.id, &branch_id, None, 100)
        .expect("materialized attempt-owned effects")
        .into_iter()
        .filter_map(|history| match history.stored.effect {
            InteractionEffect::VisibleSystemEvent { text } => Some(text),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(visible_times, vec![event_time_text]);
}
