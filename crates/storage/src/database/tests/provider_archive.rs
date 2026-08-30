#[test]
#[allow(clippy::too_many_lines)]
fn archived_connection_preserves_generation_and_discovery_history_after_reopen() {
    let (root, storage, conversation, branch_id) = imported_storage();
    let profile = ProviderProfile {
        id: "archived-history".to_owned(),
        display_name: "Archived history".to_owned(),
        base_url: "https://history.example.test/v1".to_owned(),
        model: "historical-model".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&profile)
        .expect("save historical provider");
    let connection_id = ProviderConnectionId::from(profile.id.as_str());
    let historical_connection = storage
        .get_provider_connection(&connection_id)
        .expect("historical connection");
    let route = storage
        .get_model_route(&ModelRouteId::from(profile.id.as_str()))
        .expect("historical route");
    let preset = storage
        .get_generation_preset(&GenerationPresetId::from(profile.id.as_str()))
        .expect("historical preset");

    let user = Message::user(conversation.id.clone(), "historical request");
    let generation_id = GenerationId::new();
    let pending = Message::pending_assistant(
        conversation.id.clone(),
        user.id.clone(),
        generation_id.clone(),
    );
    let generation = GenerationRecord {
        id: generation_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch_id.clone(),
        user_message_id: user.id.clone(),
        assistant_message_id: Some(pending.id.clone()),
        mode: ConversationMode::Chat,
        model: route.model_id.clone(),
        model_route_id: Some(route.id.clone()),
        generation_preset_id: Some(preset.id.clone()),
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
        started_at: pending.created_at,
        finished_at: None,
    };
    storage
        .append_generation(&branch_id, None, &user, &pending, &generation)
        .expect("append provider-target generation");
    let mut assistant = pending;
    assistant.content = "historical response".to_owned();
    assistant.status = MessageStatus::Complete;
    storage
        .finalize_generation(&assistant, None, None, true)
        .expect("finalize provider-target generation");

    let discovery_session_id = "archived-provider-discovery";
    let now = Utc::now().to_rfc3339();
    let sanitized_input = serde_json::json!({
        "connection_id": profile.id.clone(),
        "display_name": "Archived history",
    })
    .to_string();
    {
        let mut connection = storage.connection().expect("connection");
        let transaction = connection
            .transaction()
            .expect("historical discovery fixture transaction");
        let initial_state_guard_sql = transaction
            .query_row(
                "SELECT sql FROM sqlite_schema
                     WHERE type = 'trigger'
                       AND name = 'provider_discovery_session_initial_state_guard'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("load discovery initial-state guard");
        transaction
            .execute_batch("DROP TRIGGER provider_discovery_session_initial_state_guard;")
            .expect("suspend initial-state guard for synthetic historical fixture");
        transaction
            .execute(
                "INSERT INTO provider_discovery_sessions
                     (id, state, sanitized_input_json, committed_connection_id,
                      created_at, updated_at)
                     VALUES (?1, 'ready', ?2, ?3, ?4, ?4)",
                params![
                    discovery_session_id,
                    sanitized_input,
                    profile.id.as_str(),
                    now
                ],
            )
            .expect("seed discovery session reference");
        transaction
            .execute_batch(&initial_state_guard_sql)
            .expect("restore discovery initial-state guard");
        transaction
            .execute(
                "INSERT INTO provider_discovery_audit_log
                     (session_id, audit_sequence, session_revision, audit_kind,
                      action_id, subject_id, summary_key, created_at)
                     VALUES (
                       ?1, 1, 0, 'session_created', NULL, ?2,
                       'discovery.audit.session_created', ?3
                     )",
                params![discovery_session_id, profile.id.as_str(), now],
            )
            .expect("seed discovery audit reference");
        transaction
            .commit()
            .expect("commit synthetic historical discovery fixture");
    }
    storage
        .save_settings(&AppSettings {
            selected_provider_profile_id: Some(profile.id.clone()),
            ..AppSettings::default()
        })
        .expect("select provider before archive");

    let archive = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare credential-aware provider archive");
    storage
        .finish_provider_credential_archive(
            &archive.plan.operation_id,
            &archive.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("archive provider connection");
    assert!(
        storage
            .list_provider_connections()
            .expect("active list")
            .is_empty()
    );
    assert_eq!(
        storage
            .get_provider_connection(&connection_id)
            .expect_err("archived connection is inactive")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        storage
            .create_model_sync_job(&historical_connection)
            .expect_err("archived connection cannot start model sync")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        storage
            .load_settings()
            .expect("settings after archive")
            .selected_model_route_id,
        None
    );
    let stored_generation = storage.get_generation(&generation_id).expect("generation");
    assert_eq!(stored_generation.status, GenerationStatus::Complete);
    assert_eq!(stored_generation.model_route_id, Some(route.id.clone()));
    assert_eq!(
        stored_generation.generation_preset_id,
        Some(preset.id.clone())
    );
    assert_eq!(
        storage
            .list_branch_messages(&branch_id)
            .expect("conversation history")
            .len(),
        2
    );
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen archived history");
    assert_eq!(
        reopened
            .schema_version()
            .expect("read durable schema version"),
        SCHEMA_VERSION
    );
    assert!(
        reopened
            .list_provider_connections()
            .expect("reopened active list")
            .is_empty()
    );
    assert_eq!(
        reopened
            .get_provider_connection(&connection_id)
            .expect_err("reopened archived connection is inactive")
            .code,
        CoreErrorCode::NotFound
    );
    assert!(reopened.get_model_route(&route.id).is_ok());
    assert!(reopened.get_generation_preset(&preset.id).is_ok());
    assert!(reopened.get_generation(&generation_id).is_ok());
    assert_eq!(
        reopened
            .list_branch_messages(&branch_id)
            .expect("reopened conversation history")
            .len(),
        2
    );
    let (archived_rows, discovery_rows, audit_rows) = reopened
        .connection()
        .expect("connection")
        .query_row(
            "SELECT
                   (SELECT COUNT(*) FROM provider_connections
                    WHERE id = ?1 AND archived_at IS NOT NULL),
                   (SELECT COUNT(*) FROM provider_discovery_sessions
                    WHERE id = ?2 AND committed_connection_id = ?1),
                   (SELECT COUNT(*) FROM provider_discovery_audit_log
                    WHERE session_id = ?2)",
            params![connection_id.as_str(), discovery_session_id],
            |row| {
                Ok((
                    row.get::<_, u32>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, u32>(2)?,
                ))
            },
        )
        .expect("historical row counts");
    assert_eq!((archived_rows, discovery_rows, audit_rows), (1, 1, 1));
}

#[test]
fn credential_archive_prepare_blocks_provider_generation_append_without_rows() {
    let (_root, storage, conversation, branch_id) = imported_storage();
    let profile = ProviderProfile {
        id: "archive-before-generation".to_owned(),
        display_name: "Archive before generation".to_owned(),
        base_url: "https://api.example.test/v1".to_owned(),
        model: "blocked-model".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&profile)
        .expect("save provider profile");
    let connection_id = ProviderConnectionId::from(profile.id.as_str());
    let archive = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare credential archive");
    assert_eq!(
        archive.status,
        crate::ProviderCredentialOperationStatus::Prepared
    );
    let (user, pending, generation) = provider_generation_record(
        &conversation,
        &branch_id,
        ModelRouteId::from(profile.id.as_str()),
        GenerationPresetId::from(profile.id.as_str()),
        &profile.model,
        "must remain transient",
    );
    let before_message_count = storage
        .list_messages(&conversation.id)
        .expect("messages before rejected append")
        .len();
    let before_branch_head = storage
        .get_conversation_branch(&branch_id)
        .expect("branch before rejected append")
        .head_message_id;

    let error = storage
        .append_generation(&branch_id, None, &user, &pending, &generation)
        .expect_err("credential archive must block provider generation append");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(
        storage
            .list_messages(&conversation.id)
            .expect("messages after rejected append")
            .len(),
        before_message_count
    );
    assert_eq!(
        storage
            .connection()
            .expect("generation count connection")
            .query_row(
                "SELECT COUNT(*) FROM generations WHERE id = ?1",
                [generation.id.0.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .expect("generation row count"),
        0
    );
    assert_eq!(
        storage
            .connection()
            .expect("message identity count connection")
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id = ?1 OR id = ?2",
                params![user.id.0.as_str(), pending.id.0.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .expect("user and assistant row count"),
        0
    );
    assert_eq!(
        storage
            .get_conversation_branch(&branch_id)
            .expect("branch after rejected append")
            .head_message_id,
        before_branch_head
    );
    assert_eq!(
        storage
            .get_conversation_state(&conversation.id)
            .expect("conversation state after rejected append")
            .active_branch_id,
        branch_id
    );
    assert_eq!(
        storage
            .get_provider_credential_operation(&archive.plan.operation_id)
            .expect("archive after rejected append")
            .status,
        crate::ProviderCredentialOperationStatus::Prepared
    );
}

#[test]
fn running_provider_generation_blocks_credential_archive() {
    let (_root, storage, conversation, branch_id) = imported_storage();
    let profile = ProviderProfile {
        id: "generation-before-archive".to_owned(),
        display_name: "Generation before archive".to_owned(),
        base_url: "https://api.example.test/v1".to_owned(),
        model: "running-model".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&profile)
        .expect("save provider profile");
    let connection_id = ProviderConnectionId::from(profile.id.as_str());
    let (user, pending, generation) = provider_generation_record(
        &conversation,
        &branch_id,
        ModelRouteId::from(profile.id.as_str()),
        GenerationPresetId::from(profile.id.as_str()),
        &profile.model,
        "remain running",
    );
    storage
        .append_generation(&branch_id, None, &user, &pending, &generation)
        .expect("append running provider generation");

    let error = storage
        .prepare_provider_credential_operation(
            &connection_id,
            ProviderCredentialOperationKind::RemoveForArchive,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect_err("running provider generation must block credential archive");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert!(
        storage
            .list_unresolved_provider_credential_operations()
            .expect("list credential operations")
            .is_empty()
    );
    assert_eq!(
        storage
            .get_generation(&generation.id)
            .expect("running generation remains durable")
            .status,
        GenerationStatus::Running
    );
}

#[test]
fn concurrent_provider_selection_and_delete_cannot_leave_dangling_settings() {
    let root = tempdir().expect("temp root");
    let storage = Arc::new(Storage::open(root.path()).expect("open storage"));

    for index in 0..32 {
        let profile = ProviderProfile {
            id: format!("provider-{index}"),
            display_name: format!("Provider {index}"),
            base_url: "http://127.0.0.1:11434/v1".to_owned(),
            model: "synthetic".to_owned(),
            timeout_seconds: 30,
        };
        storage
            .save_provider_profile(&profile)
            .expect("save provider");
        storage
            .save_settings(&AppSettings {
                preserve_partial_generations: true,
                selected_provider_profile_id: None,
                ..AppSettings::default()
            })
            .expect("reset settings");

        let barrier = Arc::new(Barrier::new(3));
        let selecting_storage = Arc::clone(&storage);
        let selecting_barrier = Arc::clone(&barrier);
        let selected_id = profile.id.clone();
        let selection = thread::spawn(move || {
            selecting_barrier.wait();
            selecting_storage.save_settings(&AppSettings {
                preserve_partial_generations: true,
                selected_provider_profile_id: Some(selected_id),
                ..AppSettings::default()
            })
        });
        let deleting_storage = Arc::clone(&storage);
        let deleting_barrier = Arc::clone(&barrier);
        let deleted_id = profile.id.clone();
        let deletion = thread::spawn(move || {
            deleting_barrier.wait();
            deleting_storage.delete_provider_profile(&deleted_id)
        });
        barrier.wait();

        let selection = selection.join().expect("selection thread");
        deletion
            .join()
            .expect("deletion thread")
            .expect("delete provider");
        if let Err(error) = selection {
            assert_eq!(error.code, CoreErrorCode::NotFound);
        }
        assert_eq!(
            storage
                .get_provider_profile(&profile.id)
                .expect_err("provider must be deleted")
                .code,
            CoreErrorCode::NotFound
        );
        assert_eq!(
            storage
                .load_settings()
                .expect("settings after concurrent operations")
                .selected_provider_profile_id,
            None
        );
    }
}
