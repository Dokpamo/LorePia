#[test]
fn one_character_can_own_multiple_explicit_rooms_with_independent_modes() {
    let (_root, core, character) = imported_core();
    let chat = core
        .create_conversation(&character.id, "첫 번째 방", ConversationMode::Chat)
        .expect("chat room");
    let story = core
        .create_conversation(&character.id, "두 번째 방", ConversationMode::Story)
        .expect("story room");

    assert_ne!(chat.id, story.id);
    assert_eq!(
        core.list_conversations_for_character(&character.id)
            .expect("character rooms")
            .len(),
        2
    );
    assert_eq!(
        core.get_conversation_state(&chat.id)
            .expect("chat state")
            .selected_mode,
        ConversationMode::Chat
    );
    assert_eq!(
        core.get_conversation_state(&story.id)
            .expect("story state")
            .selected_mode,
        ConversationMode::Story
    );
    assert_eq!(
        core.list_conversation_branches(&chat.id)
            .expect("default branch")
            .len(),
        1
    );
}

#[test]
fn generation_assembly_preserves_validated_temperature_and_default_omission() {
    let (_root, core, character) = imported_core();
    let finite_conversation = core
        .create_conversation(&character.id, "온도 검증", ConversationMode::Chat)
        .expect("finite-temperature conversation");
    let finite_state = core
        .get_conversation_state(&finite_conversation.id)
        .expect("finite-temperature state");
    let (provider, _messages, captured_temperature) =
        CapturingProvider::new_with_temperature_capture("응답");

    let invalid = core
        .send_message_to_branch_with_provider_options(
            &finite_conversation.id,
            &finite_state.active_branch_id,
            None,
            ConversationMode::Chat,
            "전송되면 안 됨",
            new_test_generation_operation("invalid-temperature-v1"),
            "model".to_owned(),
            None,
            None,
            false,
            Some(f64::NAN),
            Some(1),
            None,
            None,
            false,
            Arc::new(StaticProvider::new("unused")),
        )
        .expect_err("non-finite temperature must fail before persistence");
    assert_eq!(invalid.code, CoreErrorCode::InvalidInput);
    assert!(
        core.list_branch_messages(&finite_state.active_branch_id)
            .expect("unchanged branch")
            .is_empty()
    );

    // This synthetic direct-provider path has no compiled route schema.
    // Core therefore validates finiteness and preserves the exact value;
    // route-backed family-specific bounds are enforced before assembly.
    core.send_message_to_branch_with_provider_options(
        &finite_conversation.id,
        &finite_state.active_branch_id,
        None,
        ConversationMode::Chat,
        "유한 온도",
        new_test_generation_operation("finite-temperature-v1"),
        "model".to_owned(),
        None,
        None,
        false,
        Some(3.0),
        Some(1),
        None,
        None,
        false,
        provider,
    )
    .expect("finite-temperature generation");
    assert_eq!(
        captured_temperature
            .recv_timeout(Duration::from_secs(2))
            .expect("captured finite temperature"),
        Some(3.0)
    );

    let default_conversation = core
        .create_conversation(&character.id, "기본 온도", ConversationMode::Chat)
        .expect("default conversation");
    let default_state = core
        .get_conversation_state(&default_conversation.id)
        .expect("default state");
    let (provider, _messages, captured_temperature) =
        CapturingProvider::new_with_temperature_capture("응답");
    core.send_message_to_branch_with_provider_options(
        &default_conversation.id,
        &default_state.active_branch_id,
        None,
        ConversationMode::Chat,
        "기본값",
        new_test_generation_operation("default-temperature-v1"),
        "model".to_owned(),
        None,
        None,
        false,
        None,
        Some(1),
        None,
        None,
        false,
        provider,
    )
    .expect("default generation");
    assert_eq!(
        captured_temperature
            .recv_timeout(Duration::from_secs(2))
            .expect("captured omitted temperature"),
        None
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn forked_branch_uses_only_its_parent_lineage_and_rejects_a_stale_head() {
    let (_root, core, character) = imported_core();
    let conversation = core
        .create_conversation(&character.id, "분기 테스트", ConversationMode::Chat)
        .expect("conversation");
    core.send_message_with_provider(
        &conversation.id,
        "공통 시작",
        "static".to_owned(),
        None,
        Arc::new(StaticProvider::new("원본 답변")),
    )
    .expect("initial generation");
    let deadline = Instant::now() + Duration::from_secs(2);
    let original = loop {
        let messages = core.list_messages(&conversation.id).expect("messages");
        if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
            break messages;
        }
        assert!(Instant::now() < deadline, "initial generation timed out");
        thread::sleep(Duration::from_millis(10));
    };

    let fork = core
        .create_conversation_branch(
            &conversation.id,
            Some(&original[0].id),
            Some("다른 선택".to_owned()),
        )
        .expect("fork");
    let (provider, captured) = CapturingProvider::new("분기 답변");
    let generation_id = core
        .send_message_to_branch_with_provider(
            &conversation.id,
            &fork.id,
            Some(&original[0].id),
            ConversationMode::Story,
            "분기 질문",
            new_test_generation_operation("branch-question-v1"),
            "captured".to_owned(),
            None,
            provider,
        )
        .expect("branch generation");
    let request_messages = captured
        .recv_timeout(Duration::from_secs(2))
        .expect("captured prompt");
    assert!(
        request_messages
            .first()
            .is_some_and(|message| message.contains("Story mode:")),
        "the provider prompt must use the generation snapshot mode"
    );
    assert!(
        request_messages
            .iter()
            .any(|message| message == "공통 시작")
    );
    assert!(
        request_messages
            .iter()
            .any(|message| message == "분기 질문")
    );
    assert!(
        !request_messages
            .iter()
            .any(|message| message == "원본 답변"),
        "a sibling assistant response must not leak into the fork prompt"
    );

    let deadline = Instant::now() + Duration::from_secs(2);
    let forked = loop {
        let messages = core.list_branch_messages(&fork.id).expect("fork messages");
        if messages.len() == 3
            && messages
                .last()
                .is_some_and(|message| message.status == MessageStatus::Complete)
        {
            break messages;
        }
        assert!(Instant::now() < deadline, "branch generation timed out");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        forked
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        ["공통 시작", "분기 질문", "분기 답변"]
    );
    assert_eq!(
        core.inner
            .storage
            .get_generation(&generation_id)
            .expect("generation snapshot")
            .mode,
        ConversationMode::Story
    );
    assert_eq!(
        core.list_branch_messages(
            &core
                .get_conversation_state(&conversation.id)
                .expect("state")
                .active_branch_id
        )
        .expect("original branch")
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>(),
        ["공통 시작", "원본 답변"]
    );

    let stale = core
        .send_message_to_branch_with_provider(
            &conversation.id,
            &fork.id,
            Some(&original[0].id),
            ConversationMode::Story,
            "오래된 head",
            new_test_generation_operation("stale-branch-head-v1"),
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("should not run")),
        )
        .expect_err("stale branch head");
    assert_eq!(stale.code, CoreErrorCode::InvalidInput);
    assert!(stale.recoverable);
    assert_eq!(
        core.list_branch_messages(&fork.id)
            .expect("unchanged fork")
            .len(),
        3
    );

    core.select_conversation_branch(&conversation.id, &fork.id)
        .expect("select fork");
    assert_eq!(
        core.list_messages(&conversation.id)
            .expect("active branch messages"),
        forked
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn message_actions_fork_immutable_lineage_and_rewind_without_deleting_rows() {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(&character.id, "메시지 액션", ConversationMode::Chat)
        .expect("conversation");
    core.send_message_with_provider(
        &conversation.id,
        "원본 질문",
        "static".to_owned(),
        None,
        Arc::new(StaticProvider::new("원본 답변")),
    )
    .expect("initial generation");
    let deadline = Instant::now() + Duration::from_secs(2);
    let original = loop {
        let messages = core.list_messages(&conversation.id).expect("messages");
        if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
            break messages;
        }
        assert!(Instant::now() < deadline, "initial generation timed out");
        thread::sleep(Duration::from_millis(10));
    };
    let source_branch_id = core
        .get_conversation_state(&conversation.id)
        .expect("source state")
        .active_branch_id;
    core.set_conversation_mode(&conversation.id, ConversationMode::Story)
        .expect("story mode");

    let (edit_provider, edited_prompt) = CapturingProvider::new("수정 답변");
    let edited = core
        .edit_user_message_with_provider(
            &conversation.id,
            &source_branch_id,
            Some(&original[1].id),
            &original[0].id,
            "수정 질문",
            "edited-model".to_owned(),
            None,
            edit_provider,
        )
        .expect("edit user");
    let edited_request = edited_prompt
        .recv_timeout(Duration::from_secs(2))
        .expect("edited prompt");
    assert!(
        edited_request
            .first()
            .is_some_and(|message| message.contains("Story mode:"))
    );
    assert!(edited_request.iter().any(|message| message == "수정 질문"));
    assert!(!edited_request.iter().any(|message| message == "원본 질문"));
    let deadline = Instant::now() + Duration::from_secs(2);
    let edited_messages = loop {
        let messages = core
            .list_branch_messages(&edited.branch.id)
            .expect("edited branch");
        if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
            break messages;
        }
        assert!(Instant::now() < deadline, "edited generation timed out");
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        edited_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        ["수정 질문", "수정 답변"]
    );
    assert_eq!(
        core.get_conversation_state(&conversation.id)
            .expect("edited state")
            .active_branch_id,
        edited.branch.id
    );
    assert_eq!(
        core.inner
            .storage
            .get_generation(&edited.generation_id)
            .expect("edited generation")
            .mode,
        ConversationMode::Story
    );
    assert_eq!(
        core.list_branch_messages(&source_branch_id)
            .expect("original branch"),
        original
    );

    core.select_conversation_branch(&conversation.id, &source_branch_id)
        .expect("select original");
    let (regenerate_provider, regenerated_prompt) = CapturingProvider::new("새 답변");
    let regenerated = core
        .regenerate_assistant_message_with_provider(
            &conversation.id,
            &source_branch_id,
            Some(&original[1].id),
            &original[1].id,
            "regenerated-model".to_owned(),
            None,
            regenerate_provider,
        )
        .expect("regenerate assistant");
    let regenerated_request = regenerated_prompt
        .recv_timeout(Duration::from_secs(2))
        .expect("regenerated prompt");
    assert!(
        regenerated_request
            .iter()
            .any(|message| message == "원본 질문")
    );
    assert!(
        !regenerated_request
            .iter()
            .any(|message| message == "원본 답변")
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    let regenerated_messages = loop {
        let messages = core
            .list_branch_messages(&regenerated.branch.id)
            .expect("regenerated branch");
        if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
            break messages;
        }
        assert!(
            Instant::now() < deadline,
            "regenerated generation timed out"
        );
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(regenerated_messages[0].content, "원본 질문");
    assert_ne!(regenerated_messages[0].id, original[0].id);
    assert_eq!(regenerated_messages[1].content, "새 답변");
    assert_eq!(
        core.list_branch_messages(&source_branch_id)
            .expect("preserved original"),
        original
    );

    let rows_before_remove = core.database_stats().expect("stats").messages;
    let rewound = core
        .remove_message_from_branch(
            &conversation.id,
            &regenerated.branch.id,
            Some(&regenerated_messages[1].id),
            &regenerated_messages[1].id,
        )
        .expect("remove regenerated assistant");
    assert_eq!(
        rewound.head_message_id,
        Some(regenerated_messages[0].id.clone())
    );
    assert_eq!(
        core.list_branch_messages(&regenerated.branch.id)
            .expect("rewound branch"),
        vec![regenerated_messages[0].clone()]
    );
    assert_eq!(
        core.database_stats().expect("stats").messages,
        rows_before_remove,
        "logical removal must preserve immutable message rows"
    );

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen");
    assert_eq!(
        reopened
            .get_conversation_state(&conversation.id)
            .expect("restored state")
            .active_branch_id,
        regenerated.branch.id
    );
    assert_eq!(
        reopened
            .list_branch_messages(&source_branch_id)
            .expect("restored original"),
        original
    );
    assert_eq!(
        reopened.database_stats().expect("restored stats").messages,
        rows_before_remove
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn message_actions_reject_wrong_roles_stale_context_foreign_rooms_and_pending_heads() {
    let (_root, core, character) = imported_core();
    let conversation = core
        .create_conversation(&character.id, "거절 테스트", ConversationMode::Chat)
        .expect("conversation");
    core.send_message_with_provider(
        &conversation.id,
        "질문",
        "static".to_owned(),
        None,
        Arc::new(StaticProvider::new("답변")),
    )
    .expect("initial generation");
    let deadline = Instant::now() + Duration::from_secs(2);
    let messages = loop {
        let messages = core.list_messages(&conversation.id).expect("messages");
        if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
            break messages;
        }
        assert!(Instant::now() < deadline, "initial generation timed out");
        thread::sleep(Duration::from_millis(10));
    };
    let branch_id = core
        .get_conversation_state(&conversation.id)
        .expect("state")
        .active_branch_id;

    let edit_assistant = core
        .edit_user_message_with_provider(
            &conversation.id,
            &branch_id,
            Some(&messages[1].id),
            &messages[1].id,
            "잘못된 편집",
            "unused".to_owned(),
            None,
            Arc::new(StaticProvider::new("unused")),
        )
        .expect_err("assistant cannot be edited");
    assert_eq!(edit_assistant.code, CoreErrorCode::InvalidInput);
    let regenerate_user = core
        .regenerate_assistant_message_with_provider(
            &conversation.id,
            &branch_id,
            Some(&messages[1].id),
            &messages[0].id,
            "unused".to_owned(),
            None,
            Arc::new(StaticProvider::new("unused")),
        )
        .expect_err("user cannot be regenerated");
    assert_eq!(regenerate_user.code, CoreErrorCode::InvalidInput);

    let stale = core
        .remove_message_from_branch(
            &conversation.id,
            &branch_id,
            Some(&messages[0].id),
            &messages[1].id,
        )
        .expect_err("stale expected head");
    assert_eq!(stale.code, CoreErrorCode::InvalidInput);
    assert!(stale.recoverable);

    let foreign = core
        .create_conversation(&character.id, "다른 방", ConversationMode::Chat)
        .expect("foreign conversation");
    let foreign_error = core
        .remove_message_from_branch(
            &foreign.id,
            &branch_id,
            Some(&messages[1].id),
            &messages[1].id,
        )
        .expect_err("foreign conversation");
    assert_eq!(foreign_error.code, CoreErrorCode::NotFound);

    let (stalling, started) = StallingProvider::new("생성 중");
    core.send_message_to_branch_with_provider(
        &conversation.id,
        &branch_id,
        Some(&messages[1].id),
        ConversationMode::Chat,
        "다음 질문",
        new_test_generation_operation("pending-generation-v1"),
        "stalling".to_owned(),
        None,
        stalling,
    )
    .expect("pending generation");
    started
        .recv_timeout(Duration::from_secs(2))
        .expect("provider started");
    let pending_head = core
        .list_branch_messages(&branch_id)
        .expect("pending lineage")
        .last()
        .expect("pending assistant")
        .id
        .clone();
    let pending_error = core
        .remove_message_from_branch(
            &conversation.id,
            &branch_id,
            Some(&pending_head),
            &pending_head,
        )
        .expect_err("pending generation");
    assert_eq!(pending_error.code, CoreErrorCode::InvalidInput);
    assert!(pending_error.recoverable);
}

#[test]
fn provider_snapshot_failure_leaves_generation_tables_empty() {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(&character.id, "snapshot failure", ConversationMode::Chat)
        .expect("conversation");
    let state = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state");

    let error = core
        .send_message_to_branch_with_provider_options(
            &conversation.id,
            &state.active_branch_id,
            None,
            ConversationMode::Chat,
            "must remain transient",
            new_test_generation_operation("snapshot-failure-v1"),
            "snapshot-failure".to_owned(),
            None,
            None,
            false,
            None,
            Some(128),
            None,
            None,
            false,
            Arc::new(SnapshotFailingProvider),
        )
        .expect_err("snapshot preflight must fail");
    assert_eq!(error.code, CoreErrorCode::Internal);

    let connection =
        rusqlite::Connection::open(root.path().join("db/lorepia.sqlite3")).expect("database");
    for table in [
        "messages",
        "generations",
        "generation_prompt_plans",
        "provider_request_snapshots",
        "knowledge_activation_logs",
        "generation_prompt_plan_knowledge_selections",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("table count");
        assert_eq!(count, 0, "{table} must remain empty");
    }
    assert!(
        core.inner
            .storage
            .get_conversation_branch(&state.active_branch_id)
            .expect("branch")
            .head_message_id
            .is_none()
    );
}

#[test]
fn identical_send_retry_with_original_head_replays_the_existing_generation() {
    let (_root, core, character) = imported_core();
    let conversation = core
        .create_conversation(&character.id, "response loss", ConversationMode::Chat)
        .expect("conversation");
    let state = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state");
    let original_head = core
        .inner
        .storage
        .get_conversation_branch(&state.active_branch_id)
        .expect("original branch")
        .head_message_id;
    let (provider, started) = StallingProvider::new("in flight");
    let first_generation = core
        .send_message_to_branch_with_provider(
            &conversation.id,
            &state.active_branch_id,
            original_head.as_ref(),
            ConversationMode::Chat,
            "exact same request",
            new_test_generation_operation("same-branch-response-loss-v1"),
            "response-loss-model".to_owned(),
            None,
            provider,
        )
        .expect("first send");
    started
        .recv_timeout(Duration::from_secs(2))
        .expect("provider started");
    let first_messages = core
        .list_branch_messages(&state.active_branch_id)
        .expect("first append");
    assert_eq!(first_messages.len(), 2);

    let replayed_generation = core
        .send_message_to_branch_with_provider(
            &conversation.id,
            &state.active_branch_id,
            original_head.as_ref(),
            ConversationMode::Chat,
            "exact same request",
            new_test_generation_operation("same-branch-response-loss-v1"),
            "response-loss-model".to_owned(),
            None,
            Arc::new(SnapshotFailingProvider),
        )
        .expect("response-loss retry");

    assert_eq!(replayed_generation, first_generation);
    assert_eq!(
        core.list_branch_messages(&state.active_branch_id)
            .expect("replayed messages"),
        first_messages,
        "an identical retry must not append or relaunch"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one restart fixture proves lost-response replay for both immutable action kinds"
)]
fn message_action_retries_survive_response_loss_and_reopen_without_relaunch() {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "Action response loss",
            ConversationMode::Chat,
        )
        .expect("conversation");
    let source_model = "action-source-model";
    let source_generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "original question",
            source_model.to_owned(),
            None,
            Arc::new(StaticProvider::new("original reply")),
        )
        .expect("initial generation");
    let source_branch = core
        .get_conversation_state(&conversation.id)
        .expect("source state")
        .active_branch_id;
    wait_for_generation_status(&core, &source_generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let source_generation = core
        .list_branch_messages(&source_branch)
        .expect("source messages");
    assert_eq!(source_generation.len(), 2);
    drop(core);

    let core = Core::open(CoreConfig::new(root.path()))
        .expect("reopen after lost same-branch send response");
    let replayed_source_generation_id = core
        .send_message_to_branch_with_provider(
            &conversation.id,
            &source_branch,
            None,
            ConversationMode::Chat,
            "original question",
            new_test_generation_operation("core-direct-send-v1"),
            source_model.to_owned(),
            None,
            Arc::new(SnapshotFailingProvider),
        )
        .expect("replay send after response loss");
    assert_eq!(replayed_source_generation_id, source_generation_id);
    assert_eq!(
        core.list_branch_messages(&source_branch)
            .expect("replayed source messages"),
        source_generation
    );

    let edit_model = "action-edit-response-loss-model";
    let edited = core
        .edit_user_message_with_provider(
            &conversation.id,
            &source_branch,
            Some(&source_generation[1].id),
            &source_generation[0].id,
            "edited question",
            edit_model.to_owned(),
            None,
            Arc::new(StaticProvider::new("edited reply")),
        )
        .expect("edit generation");
    wait_for_generation_status(&core, &edited.generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let branches_after_edit = core
        .list_conversation_branches(&conversation.id)
        .expect("branches after edit");
    let messages_after_edit = core
        .list_branch_messages(&edited.branch.id)
        .expect("edited messages");
    drop(core);

    let reopened =
        Core::open(CoreConfig::new(root.path())).expect("reopen after lost edit response");
    let replayed_edit = reopened
        .edit_user_message_with_provider(
            &conversation.id,
            &source_branch,
            Some(&source_generation[1].id),
            &source_generation[0].id,
            "edited question",
            edit_model.to_owned(),
            None,
            Arc::new(SnapshotFailingProvider),
        )
        .expect("replay edit after response loss");
    assert_eq!(replayed_edit, edited);
    assert_eq!(
        reopened
            .list_conversation_branches(&conversation.id)
            .expect("replayed edit branches"),
        branches_after_edit
    );
    assert_eq!(
        reopened
            .list_branch_messages(&edited.branch.id)
            .expect("replayed edit messages"),
        messages_after_edit
    );
    let changed_edit = reopened
        .edit_user_message_with_provider(
            &conversation.id,
            &source_branch,
            Some(&source_generation[1].id),
            &source_generation[0].id,
            "different edited question",
            edit_model.to_owned(),
            None,
            Arc::new(SnapshotFailingProvider),
        )
        .expect_err("changed edit input must not replay the completed operation");
    assert_eq!(changed_edit.code, CoreErrorCode::InvalidInput);

    reopened
        .select_conversation_branch(&conversation.id, &source_branch)
        .expect("restore source for regenerate");
    let regenerate_model = "action-regenerate-response-loss-model";
    let regenerated = reopened
        .regenerate_assistant_message_with_provider(
            &conversation.id,
            &source_branch,
            Some(&source_generation[1].id),
            &source_generation[1].id,
            regenerate_model.to_owned(),
            None,
            Arc::new(StaticProvider::new("regenerated reply")),
        )
        .expect("regenerate assistant");
    wait_for_generation_status(
        &reopened,
        &regenerated.generation_id,
        GenerationStatus::Complete,
    );
    wait_for_generation_registry_to_drain(&reopened);
    let branches_after_regenerate = reopened
        .list_conversation_branches(&conversation.id)
        .expect("branches after regenerate");
    let messages_after_regenerate = reopened
        .list_branch_messages(&regenerated.branch.id)
        .expect("regenerated messages");
    drop(reopened);

    let reopened =
        Core::open(CoreConfig::new(root.path())).expect("reopen after lost regenerate response");
    let replayed_regenerate = reopened
        .regenerate_assistant_message_with_provider(
            &conversation.id,
            &source_branch,
            Some(&source_generation[1].id),
            &source_generation[1].id,
            regenerate_model.to_owned(),
            None,
            Arc::new(SnapshotFailingProvider),
        )
        .expect("replay regenerate after response loss");
    assert_eq!(replayed_regenerate, regenerated);
    assert_eq!(
        reopened
            .list_conversation_branches(&conversation.id)
            .expect("replayed regenerate branches"),
        branches_after_regenerate
    );
    assert_eq!(
        reopened
            .list_branch_messages(&regenerated.branch.id)
            .expect("replayed regenerate messages"),
        messages_after_regenerate
    );
    let changed_regenerate = reopened
        .regenerate_assistant_message_with_provider(
            &conversation.id,
            &source_branch,
            Some(&source_generation[1].id),
            &source_generation[1].id,
            "different-regenerate-model".to_owned(),
            None,
            Arc::new(SnapshotFailingProvider),
        )
        .expect_err("changed regenerate target must not replay the completed operation");
    assert_eq!(changed_regenerate.code, CoreErrorCode::InvalidInput);
}

#[test]
#[allow(clippy::too_many_lines)]
fn generation_launch_preflight_prevents_failed_sends_and_actions_from_mutating_storage() {
    let (_send_root, send_core, send_character) = imported_core();
    let send_conversation = send_core
        .create_conversation(&send_character.id, "전송 preflight", ConversationMode::Chat)
        .expect("send conversation");
    let send_state = send_core
        .get_conversation_state(&send_conversation.id)
        .expect("send state");
    poison_generation_registry(&send_core);
    let send_error = send_core
        .send_message_with_provider(
            &send_conversation.id,
            "저장되면 안 됨",
            "unused".to_owned(),
            None,
            Arc::new(StaticProvider::new("unused")),
        )
        .expect_err("launch preflight must fail");
    assert_eq!(send_error.code, CoreErrorCode::Internal);
    assert!(
        send_core
            .list_messages(&send_conversation.id)
            .expect("send messages")
            .is_empty()
    );
    assert!(
        send_core
            .inner
            .storage
            .get_conversation_branch(&send_state.active_branch_id)
            .expect("send branch")
            .head_message_id
            .is_none()
    );

    let (_action_root, action_core, action_character) = imported_core();
    let action_conversation = action_core
        .create_conversation(
            &action_character.id,
            "액션 preflight",
            ConversationMode::Chat,
        )
        .expect("action conversation");
    action_core
        .send_message_with_provider(
            &action_conversation.id,
            "원본",
            "static".to_owned(),
            None,
            Arc::new(StaticProvider::new("답변")),
        )
        .expect("initial generation");
    let deadline = Instant::now() + Duration::from_secs(2);
    let original = loop {
        let messages = action_core
            .list_messages(&action_conversation.id)
            .expect("action messages");
        if messages.len() == 2 && messages[1].status == MessageStatus::Complete {
            break messages;
        }
        assert!(Instant::now() < deadline, "initial generation timed out");
        thread::sleep(Duration::from_millis(10));
    };
    let action_state = action_core
        .get_conversation_state(&action_conversation.id)
        .expect("action state");
    let branch_count = action_core
        .list_conversation_branches(&action_conversation.id)
        .expect("action branches")
        .len();
    let message_count = action_core.database_stats().expect("action stats").messages;
    poison_generation_registry(&action_core);
    let action_error = action_core
        .edit_user_message_with_provider(
            &action_conversation.id,
            &action_state.active_branch_id,
            Some(&original[1].id),
            &original[0].id,
            "수정본",
            "unused".to_owned(),
            None,
            Arc::new(StaticProvider::new("unused")),
        )
        .expect_err("action launch preflight must fail");
    assert_eq!(action_error.code, CoreErrorCode::Internal);
    assert_eq!(
        action_core
            .get_conversation_state(&action_conversation.id)
            .expect("unchanged action state")
            .active_branch_id,
        action_state.active_branch_id
    );
    assert_eq!(
        action_core
            .list_conversation_branches(&action_conversation.id)
            .expect("unchanged action branches")
            .len(),
        branch_count
    );
    assert_eq!(
        action_core
            .database_stats()
            .expect("unchanged stats")
            .messages,
        message_count
    );
    assert_eq!(
        action_core
            .list_messages(&action_conversation.id)
            .expect("unchanged action messages"),
        original
    );
}
