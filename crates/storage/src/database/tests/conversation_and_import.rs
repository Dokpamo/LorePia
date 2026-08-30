#[test]
#[allow(clippy::too_many_lines)]
fn message_actions_preserve_rows_and_guard_branch_snapshots() {
    let (_root, storage, conversation, source_branch_id) = imported_storage();
    let (original_user, original_assistant) = append_complete_generation(
        &storage,
        &conversation.id,
        &source_branch_id,
        None,
        "original",
        "original response",
    );
    let context = storage
        .prepare_message_generation_action(
            &conversation.id,
            &source_branch_id,
            Some(&original_assistant.id),
            &original_user.id,
            MessageGenerationAction::EditUser,
        )
        .expect("prepare edit");
    assert!(context.fork_message_id.is_none());
    assert_eq!(context.user_text, "original");

    let edited_user = Message::user(conversation.id.clone(), "edited");
    let action_generation_id = GenerationId::new();
    let pending = Message::pending_assistant(
        conversation.id.clone(),
        edited_user.id.clone(),
        action_generation_id.clone(),
    );
    let now = Utc::now();
    let action_branch = ConversationBranch {
        id: ConversationBranchId::new(),
        conversation_id: conversation.id.clone(),
        title: None,
        fork_message_id: None,
        head_message_id: Some(pending.id.clone()),
        created_at: now,
        updated_at: now,
    };
    let generation = GenerationRecord {
        id: action_generation_id,
        conversation_id: conversation.id.clone(),
        branch_id: action_branch.id.clone(),
        user_message_id: edited_user.id.clone(),
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
    storage
        .append_message_generation_action(
            &source_branch_id,
            Some(&original_assistant.id),
            &original_user.id,
            MessageGenerationAction::EditUser,
            &action_branch,
            &edited_user,
            &pending,
            &generation,
        )
        .expect("append edit branch");
    assert_eq!(
        storage
            .get_conversation_state(&conversation.id)
            .expect("state")
            .active_branch_id,
        action_branch.id
    );
    assert_eq!(
        storage
            .list_branch_messages(&source_branch_id)
            .expect("source lineage")
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        ["original", "original response"]
    );

    let pending_error = storage
        .remove_message_from_branch(
            &conversation.id,
            &action_branch.id,
            Some(&pending.id),
            &pending.id,
        )
        .expect_err("pending branch must reject removal");
    assert_eq!(pending_error.code, CoreErrorCode::InvalidInput);
    assert!(pending_error.recoverable);

    let mut terminal = pending.clone();
    terminal.content = "edited response".to_owned();
    terminal.status = MessageStatus::Complete;
    storage
        .finalize_generation(&terminal, None, None, true)
        .expect("finalize edited response");
    let message_count = storage
        .list_messages(&conversation.id)
        .expect("all rows")
        .len();
    let rewound = storage
        .remove_message_from_branch(
            &conversation.id,
            &action_branch.id,
            Some(&terminal.id),
            &terminal.id,
        )
        .expect("rewind assistant");
    assert_eq!(rewound.head_message_id, Some(edited_user.id.clone()));
    assert_eq!(
        storage
            .list_branch_messages(&action_branch.id)
            .expect("rewound lineage"),
        vec![edited_user]
    );
    assert_eq!(
        storage
            .list_messages(&conversation.id)
            .expect("preserved rows")
            .len(),
        message_count,
        "logical removal must not delete immutable message rows"
    );

    let stale = storage
        .remove_message_from_branch(
            &conversation.id,
            &action_branch.id,
            Some(&terminal.id),
            &original_user.id,
        )
        .expect_err("stale head");
    assert_eq!(stale.code, CoreErrorCode::InvalidInput);
    assert!(stale.recoverable);
}

#[test]
fn message_action_lineage_validation_is_deep_and_cycle_safe() {
    let (_root, storage, conversation, branch_id) = imported_storage();
    let (first_message_id, last_message_id) = {
        let mut connection = storage.connection().expect("connection");
        let transaction = connection.transaction().expect("transaction");
        let mut parent_id = None;
        let mut first_message_id = None;
        let mut last_message_id = None;
        for index in 0..4_105 {
            let message = Message::user_after(
                conversation.id.clone(),
                parent_id.clone(),
                format!("message {index}"),
            );
            first_message_id.get_or_insert_with(|| message.id.clone());
            parent_id = Some(message.id.clone());
            last_message_id = Some(message.id.clone());
            insert_message(&transaction, &message).expect("insert deep message");
        }
        let last_message_id = last_message_id.expect("last message");
        transaction
            .execute(
                "UPDATE conversation_branches
                     SET head_message_id = ?2
                     WHERE id = ?1",
                params![branch_id.0, last_message_id.0],
            )
            .expect("update branch head");
        transaction.commit().expect("commit deep lineage");
        (first_message_id.expect("first message"), last_message_id)
    };

    let context = storage
        .prepare_message_generation_action(
            &conversation.id,
            &branch_id,
            Some(&last_message_id),
            &first_message_id,
            MessageGenerationAction::EditUser,
        )
        .expect("find a visible message beyond the former depth cutoff");
    assert!(context.fork_message_id.is_none());

    storage
        .connection()
        .expect("connection")
        .execute(
            "UPDATE messages
                 SET parent_id = ?2
                 WHERE conversation_id = ?1 AND id = ?3",
            params![conversation.id.0, last_message_id.0, first_message_id.0],
        )
        .expect("create synthetic corrupted cycle");
    let error = storage
        .prepare_message_generation_action(
            &conversation.id,
            &branch_id,
            Some(&last_message_id),
            &MessageId("missing-from-cycle".to_owned()),
            MessageGenerationAction::EditUser,
        )
        .expect_err("cycle-safe lookup must terminate");
    assert_eq!(error.code, CoreErrorCode::NotFound);
}

#[test]
fn persists_character_and_settings_across_reopen() {
    let root = tempdir().expect("temp root");
    let mut staged = NamedTempFile::new_in(root.path()).expect("staging");
    staged.write_all(b"character").expect("source");
    let character = Character::new("Segu", "Guide", hex::encode(Sha256::digest(b"character")));

    {
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .commit_character_import(
                staged.path(),
                &character,
                9,
                &Uuid::new_v4().to_string(),
                &[],
            )
            .expect("commit import");
        let conversation = Conversation::new(&character.id, &character.name);
        storage
            .save_conversation(&conversation)
            .expect("save conversation");
        let user = Message::user(conversation.id.clone(), "Hello");
        storage.save_message(&user).expect("save user");
        let pending = Message::pending_assistant(
            conversation.id.clone(),
            user.id.clone(),
            GenerationId::new(),
        );
        storage.save_message(&pending).expect("save pending");
        storage
            .save_provider_profile(&ProviderProfile {
                id: "local".to_owned(),
                display_name: "Local model".to_owned(),
                base_url: "http://127.0.0.1:11434/v1".to_owned(),
                model: "test".to_owned(),
                timeout_seconds: 30,
            })
            .expect("save provider");
        storage
            .save_settings(&AppSettings {
                preserve_partial_generations: false,
                selected_provider_profile_id: Some("local".to_owned()),
                ..AppSettings::default()
            })
            .expect("save settings");
    }

    let reopened = Storage::open(root.path()).expect("reopen storage");
    assert_eq!(reopened.list_characters().expect("list").len(), 1);
    assert!(
        !reopened
            .load_settings()
            .expect("load settings")
            .preserve_partial_generations
    );
    assert_eq!(
        reopened
            .list_provider_profiles()
            .expect("provider profiles")
            .len(),
        1
    );
    assert_eq!(
        reopened
            .list_messages(&reopened.list_conversations().expect("conversations")[0].id)
            .expect("messages")
            .len(),
        1,
        "discard policy removes interrupted assistant messages"
    );
}

#[test]
fn partial_checkpoint_cannot_overwrite_a_terminal_assistant_message() {
    let root = tempdir().expect("temp root");
    let mut staged = NamedTempFile::new_in(root.path()).expect("staging");
    staged.write_all(b"character").expect("source");
    let character = Character::new("Segu", "Guide", hex::encode(Sha256::digest(b"character")));
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .commit_character_import(
            staged.path(),
            &character,
            9,
            &Uuid::new_v4().to_string(),
            &[],
        )
        .expect("commit import");
    let conversation = Conversation::new(&character.id, &character.name);
    storage
        .save_conversation(&conversation)
        .expect("save conversation");
    let user = Message::user(conversation.id.clone(), "Hello");
    storage.save_message(&user).expect("save user");
    let mut pending =
        Message::pending_assistant(conversation.id.clone(), user.id, GenerationId::new());
    storage.save_message(&pending).expect("save pending");

    pending.content = "checkpoint".to_owned();
    storage
        .checkpoint_pending_assistant(&pending)
        .expect("checkpoint pending");
    let mut terminal = pending.clone();
    terminal.content = "final".to_owned();
    terminal.status = MessageStatus::Complete;
    storage.save_message(&terminal).expect("save terminal");

    pending.content = "stale checkpoint".to_owned();
    let error = storage
        .checkpoint_pending_assistant(&pending)
        .expect_err("terminal row must reject a stale checkpoint");
    assert_eq!(error.code, CoreErrorCode::NotFound);
    let messages = storage.list_messages(&conversation.id).expect("messages");
    assert_eq!(messages[1].content, "final");
    assert_eq!(messages[1].status, MessageStatus::Complete);
}

#[test]
fn restart_recovers_import_journal_cas_files_and_staging() {
    let root = tempdir().expect("temp root");
    let source_bytes = b"orphan source";
    let asset_bytes = b"orphan asset";
    let source_hash = hex::encode(Sha256::digest(source_bytes));
    let asset_hash = hex::encode(Sha256::digest(asset_bytes));
    let source_path = root
        .path()
        .join("sources")
        .join(content_relative_path(&source_hash).expect("source path"));
    let asset_path = root
        .path()
        .join("assets")
        .join(content_relative_path(&asset_hash).expect("asset path"));
    let staging_path = root.path().join("staging/inspection-recovery.json");
    let staged_asset = root
        .path()
        .join("staging/inspection-recovery-asset.partial");

    {
        let storage = Storage::open(root.path()).expect("open storage");
        fs::create_dir_all(source_path.parent().expect("source parent")).expect("source directory");
        fs::create_dir_all(asset_path.parent().expect("asset parent")).expect("asset directory");
        fs::write(&source_path, source_bytes).expect("source CAS");
        fs::write(&asset_path, asset_bytes).expect("asset CAS");
        fs::write(&staging_path, b"staging").expect("source staging");
        fs::write(&staged_asset, b"asset staging").expect("asset staging");
        storage
            .connection()
            .expect("connection")
            .execute(
                "INSERT INTO import_jobs
                     (id, source_hash, staging_path, state, updated_at, asset_hashes_json)
                     VALUES (?1, ?2, ?3, 'file_stored', ?4, ?5)",
                params![
                    "recovery-job",
                    source_hash,
                    staging_path.to_string_lossy(),
                    Utc::now().to_rfc3339(),
                    serde_json::to_string(&vec![asset_hash.clone()]).expect("asset hashes")
                ],
            )
            .expect("insert recovery journal");
    }

    let reopened = Storage::open(root.path()).expect("recover storage");
    assert!(!source_path.exists());
    assert!(!asset_path.exists());
    assert!(!staging_path.exists());
    assert!(!staged_asset.exists());
    assert!(!reopened.recovery_pending().expect("recovery status"));
    assert_eq!(
        reopened
            .schema_version()
            .expect("read durable schema version"),
        SCHEMA_VERSION
    );
}

#[test]
fn prompt_history_query_bounds_rows_and_multibyte_content_before_loading() {
    let root = tempdir().expect("temp root");
    let mut staged = NamedTempFile::new_in(root.path()).expect("staging");
    staged.write_all(b"character").expect("source");
    let character = Character::new("Segu", "Guide", hex::encode(Sha256::digest(b"character")));
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .commit_character_import(
            staged.path(),
            &character,
            9,
            &Uuid::new_v4().to_string(),
            &[],
        )
        .expect("commit import");
    let conversation = Conversation::new(&character.id, &character.name);
    storage
        .save_conversation(&conversation)
        .expect("save conversation");

    let base = Utc::now();
    for (index, content) in ["old", "😀😀", "😀😀😀", "latest"].into_iter().enumerate() {
        let mut message = Message::user(conversation.id.clone(), content);
        message.created_at =
            base + Duration::seconds(i64::try_from(index).expect("small fixture index"));
        storage.save_message(&message).expect("save message");
    }

    let history = storage
        .list_recent_messages_for_prompt(&conversation.id, 3, 8, 8)
        .expect("bounded history");
    let contents = history
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>();
    assert_eq!(contents, vec!["old", "😀😀", "latest"]);

    let recent = storage
        .list_recent_messages_for_prompt(&conversation.id, 2, 64, 64)
        .expect("recent history");
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[1].content, "latest");
}
