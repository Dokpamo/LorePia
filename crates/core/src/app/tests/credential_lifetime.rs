#[tokio::test]
async fn legacy_admission_lease_releases_after_durable_attempt_before_async_planning() {
    struct DropProbe(Option<std_mpsc::SyncSender<()>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    let (_root, core, character) = imported_core();
    let provider_profile_id = "legacy-admission-lease-profile";
    core.upsert_provider_profile(ProviderProfile {
        id: provider_profile_id.to_owned(),
        display_name: "Legacy admission lease".to_owned(),
        base_url: "http://127.0.0.1:9/v1".to_owned(),
        model: "lease-model".to_owned(),
        timeout_seconds: 1,
    })
    .expect("legacy provider profile");
    let conversation = core
        .create_conversation(
            &character.id,
            "Legacy admission lease",
            ConversationMode::Chat,
        )
        .expect("conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let (dropped_sender, dropped_receiver) = std_mpsc::sync_channel(1);
    let generation_id = core
        .send_message_to_branch_async_with_credential_admission_lease(
            &conversation.id,
            &branch,
            None,
            ConversationMode::Chat,
            "admit before prompt tasks",
            new_test_generation_operation("legacy-admission-lease-v1"),
            provider_profile_id,
            None,
            GenerationCredentialAdmissionLease::new(DropProbe(Some(dropped_sender))),
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("start legacy generation");
    dropped_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("admission lease released before async call returned");
    assert!(
        core.inner
            .storage
            .get_generation_attempt(&generation_id)
            .is_ok()
    );
}

#[tokio::test]
async fn legacy_message_action_releases_admission_only_after_durable_attempt() {
    let (_root, core, character) = imported_core();
    let provider_profile_id = "legacy-action-admission-profile";
    core.upsert_provider_profile(ProviderProfile {
        id: provider_profile_id.to_owned(),
        display_name: "Legacy action admission".to_owned(),
        base_url: "http://127.0.0.1:9/v1".to_owned(),
        model: "lease-action-model".to_owned(),
        timeout_seconds: 1,
    })
    .expect("legacy provider profile");
    let conversation = core
        .create_conversation(
            &character.id,
            "Legacy action admission lease",
            ConversationMode::Chat,
        )
        .expect("conversation");
    let source_generation_id = core
        .send_message_with_provider(
            &conversation.id,
            "original action message",
            "source-model".to_owned(),
            None,
            Arc::new(StaticProvider::new("source reply")),
        )
        .expect("source generation");
    wait_for_generation_status(&core, &source_generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let source_branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let source_messages = core
        .list_branch_messages(&source_branch)
        .expect("source messages");
    let operation_nonce = "legacy-action-admission-v1";
    let action_identity = core
        .prepare_message_generation_action_identity(MessageGenerationActionIdentityInput {
            conversation_id: &conversation.id,
            source_branch_id: &source_branch,
            expected_source_head_message_id: Some(&source_messages[1].id),
            target_message_id: &source_messages[0].id,
            action: MessageGenerationAction::EditUser,
            replacement_text: Some("edited through legacy admission"),
            operation_context: new_test_generation_operation(operation_nonce),
            target: GenerationActionTargetIdentity::ProviderProfile {
                provider_profile_id: provider_profile_id.to_owned(),
            },
        })
        .expect("resolve action operation identity");
    let (drop_sender, drop_receiver) = std_mpsc::sync_channel(1);
    let action = core
        .edit_user_message_async_with_credential_admission_lease(
            &conversation.id,
            &source_branch,
            Some(&source_messages[1].id),
            &source_messages[0].id,
            "edited through legacy admission",
            new_test_generation_operation(operation_nonce),
            provider_profile_id,
            None,
            GenerationCredentialAdmissionLease::new(DurableAttemptDropProbe {
                storage: Arc::clone(&core.inner.storage),
                conversation_id: conversation.id.clone(),
                operation_id: action_identity.operation_id,
                sender: Some(drop_sender),
            }),
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("start legacy edit generation");
    assert!(
        drop_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("observe admission lease release"),
        "message-action admission lease released before its attempt became durable"
    );
    assert!(
        core.inner
            .storage
            .get_generation_attempt(&action.generation_id)
            .is_ok()
    );
}

#[test]
fn connection_credential_presence_and_reference_match_connection_policy() {
    let root = tempdir().expect("temporary core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let api_origin = CanonicalOrigin::parse("http://127.0.0.1:39491").expect("loopback origin");
    let (_template, credential_connection) = create_openai_chat_connection(&core, &api_origin);
    let credential_canary = "synthetic-bound-credential";
    assert!(
        !format!(
            "{:?}",
            ConnectionBoundCredential::new(
                credential_connection.id.clone(),
                Some(credential_canary.to_owned()),
            )
        )
        .contains(credential_canary)
    );
    let credential_reference = credential_connection
        .credential_ref
        .as_ref()
        .expect("credential-requiring connection reference");
    assert_eq!(
        credential_reference.as_str(),
        credential_connection.id.as_str()
    );

    let missing = validate_connection_credential_binding(
        &credential_connection,
        &ConnectionBoundCredential::new(credential_connection.id.clone(), None),
    )
    .expect_err("credential-requiring connection rejects missing material");
    assert_eq!(missing.code, CoreErrorCode::ProviderAuthFailed);
    assert_eq!(missing.message, "provider credential is required");

    let mut mismatched_reference = credential_connection.clone();
    mismatched_reference.credential_ref =
        Some(CredentialRef("different-vault-reference".to_owned()));
    let mismatch = validate_connection_credential_binding(
        &mismatched_reference,
        &ConnectionBoundCredential::new(
            credential_connection.id,
            Some("synthetic-credential".to_owned()),
        ),
    )
    .expect_err("stored credential reference must match the bound connection");
    assert_eq!(mismatch.code, CoreErrorCode::InvalidInput);

    let no_auth_template = core
        .list_provider_templates()
        .expect("provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "ollama-native-v1")
        .expect("Ollama template");
    let no_auth_origin = CanonicalOrigin::parse("http://127.0.0.1:11434").expect("Ollama origin");
    let no_auth_connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from("no-auth-bound-credential"),
            template_id: no_auth_template.id,
            template_version: no_auth_template.manifest_version,
            display_name: "No-auth bound credential".to_owned(),
            api_origin: no_auth_origin,
            api_base_path: Some(EndpointPath::parse("/api").expect("Ollama API base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            values: Vec::new(),
            approved_credential_origin: None,
            local_network_approval: None,
            timeout_seconds: 5,
        })
        .expect("create no-auth connection");
    let unexpected = validate_connection_credential_binding(
        &no_auth_connection,
        &ConnectionBoundCredential::new(
            no_auth_connection.id.clone(),
            Some("synthetic-unexpected-credential".to_owned()),
        ),
    )
    .expect_err("credentialless connection rejects unexpected material");
    assert_eq!(unexpected.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        unexpected.message,
        "this provider connection does not permit a credential"
    );
    validate_connection_credential_binding(
        &no_auth_connection,
        &ConnectionBoundCredential::new(no_auth_connection.id.clone(), None),
    )
    .expect("credentialless connection accepts absent material");
}

#[tokio::test]
async fn primary_generation_retains_credential_carrier_until_provider_finishes() {
    let (_root, core, character) = imported_core();
    let (api_origin, requests, release_provider) = spawn_blocking_chat_completion_provider();
    let (template, connection) = create_openai_chat_connection(&core, &api_origin);
    let connection_id = connection.id.clone();
    let now = Utc::now();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from("primary-credential-carrier-route"),
            connection_id: connection_id.clone(),
            api_family: template.api_family,
            model_id: "primary-credential-carrier-model".to_owned(),
            display_name: Some("Primary credential carrier model".to_owned()),
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
        .expect("save primary credential carrier route");
    let preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
        .expect("save primary credential carrier preset");
    let target = GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: preset.id,
    };
    let authority = install_provider_credential_authority(&core, &connection_id);
    let conversation = core
        .create_conversation(
            &character.id,
            "Primary credential dispatch lease",
            ConversationMode::Chat,
        )
        .expect("create credential dispatch conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("credential dispatch conversation state")
        .active_branch_id;
    let operation_lock = Arc::new(tokio::sync::Mutex::new(()));
    let dispatch_lease = Arc::clone(&operation_lock).lock_owned().await;
    let credential = ConnectionBoundCredential::new_with_access_authority(
        connection_id,
        Some("synthetic-primary-leased-secret".to_owned()),
        authority,
    )
    .with_dispatch_lease(dispatch_lease);

    let generation_id = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &branch,
            None,
            ConversationMode::Chat,
            "retain the primary credential carrier",
            new_test_generation_operation("primary-credential-carrier-v1"),
            &target,
            credential,
        )
        .expect("start connection-bound primary generation");
    let request = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("primary provider future starts");
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer synthetic-primary-leased-secret\r\n")
    );
    let retained_during_provider = Arc::clone(&operation_lock).try_lock_owned().is_err();

    release_provider
        .send(())
        .expect("finish blocking primary provider");
    wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    assert!(
        retained_during_provider,
        "connection-bound carrier dropped before the primary provider future finished"
    );
    assert!(
        Arc::clone(&operation_lock).try_lock_owned().is_ok(),
        "primary provider completion must release the credential carrier"
    );
}

#[tokio::test]
async fn provider_dispatch_retains_credential_lease_until_attempt_finishes() {
    for credential_value in [Some("synthetic-leased-secret".to_owned()), None] {
        let operation_lock = Arc::new(tokio::sync::Mutex::new(()));
        let dispatch_lease = Arc::clone(&operation_lock).lock_owned().await;
        let credential = ConnectionBoundCredential::new_with_dispatch_lease(
            ProviderConnectionId::from("leased-dispatch-connection"),
            credential_value,
            dispatch_lease,
        );
        let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = tokio::sync::oneshot::channel();
        let provider = Arc::new(LeaseBarrierProvider {
            entered: Mutex::new(Some(entered_sender)),
            release: Mutex::new(Some(release_receiver)),
        });
        let request = GenerationRequest {
            generation_id: GenerationId::new(),
            conversation_id: ConversationId::new(),
            model: "lease-barrier-model".to_owned(),
            messages: Vec::new(),
            resolved_prompt_plan: None,
            provider_execution_plan_hash: None,
            temperature: None,
            max_output_tokens: None,
            provider_provenance: None,
            preserve_opaque_reasoning_state: false,
            opaque_reasoning_context: Vec::new(),
        };
        let (_cancel_sender, cancelled) = watch::channel(false);
        let dispatch = tokio::spawn(dispatch_auxiliary_task_provider(
            provider, request, credential, 5_000, cancelled,
        ));

        entered_receiver.await.expect("provider dispatch entered");
        assert!(
            Arc::clone(&operation_lock).try_lock_owned().is_err(),
            "archive/delete operation lock must remain unavailable during provider dispatch"
        );
        release_sender.send(()).expect("release provider dispatch");
        let outcome = dispatch.await.expect("Send provider dispatch task");
        assert!(matches!(outcome, TaskExecutionOutcome::Completed { .. }));
        assert!(
            Arc::clone(&operation_lock).try_lock_owned().is_ok(),
            "provider completion must release the in-process credential lease"
        );
    }
}

#[tokio::test]
async fn cancelled_provider_dispatch_drops_credential_before_releasing_mutation_gate() {
    let operation_lock = Arc::new(tokio::sync::Mutex::new(()));
    let dispatch_lease = Arc::clone(&operation_lock).lock_owned().await;
    let credential = ConnectionBoundCredential::new_with_dispatch_lease(
        ProviderConnectionId::from("cancelled-leased-dispatch-connection"),
        Some("synthetic-cancelled-leased-secret".to_owned()),
        dispatch_lease,
    );
    let (entered_sender, entered_receiver) = tokio::sync::oneshot::channel();
    let (_release_sender, release_receiver) = tokio::sync::oneshot::channel();
    let provider = Arc::new(LeaseBarrierProvider {
        entered: Mutex::new(Some(entered_sender)),
        release: Mutex::new(Some(release_receiver)),
    });
    let request = GenerationRequest {
        generation_id: GenerationId::new(),
        conversation_id: ConversationId::new(),
        model: "cancelled-lease-barrier-model".to_owned(),
        messages: Vec::new(),
        resolved_prompt_plan: None,
        provider_execution_plan_hash: None,
        temperature: None,
        max_output_tokens: None,
        provider_provenance: None,
        preserve_opaque_reasoning_state: false,
        opaque_reasoning_context: Vec::new(),
    };
    let (cancel_sender, cancelled) = watch::channel(false);
    let dispatch = tokio::spawn(dispatch_auxiliary_task_provider(
        provider, request, credential, 5_000, cancelled,
    ));

    entered_receiver.await.expect("provider dispatch entered");
    assert!(Arc::clone(&operation_lock).try_lock_owned().is_err());
    cancel_sender.send(true).expect("cancel provider dispatch");
    let outcome = dispatch.await.expect("join cancelled provider dispatch");
    assert!(matches!(
        outcome,
        TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::UnknownOutcome,
            error: CoreError {
                code: CoreErrorCode::Cancelled,
                ..
            },
        }
    ));
    assert!(
        Arc::clone(&operation_lock).try_lock_owned().is_ok(),
        "cancellation must drop and zeroize the credential carrier before mutation resumes"
    );
}

#[tokio::test]
async fn pre_cancelled_runtime_dispatch_never_enters_the_provider() {
    let (provider, captured) = CapturingProvider::new("must not be emitted");
    let request = runtime_generation_request(
        "runtime-model".to_owned(),
        vec![RuntimePromptMessage {
            role: MessageRole::User,
            content: "bounded prompt".to_owned(),
        }],
        Some(u32::MAX),
        None,
    );
    assert_eq!(request.max_output_tokens, Some(RUNTIME_MAX_OUTPUT_TOKENS));
    let (cancel_sender, cancelled) = watch::channel(false);
    cancel_sender
        .send(true)
        .expect("mark request cancelled before dispatch");

    let outcome = dispatch_auxiliary_task_provider(
        provider,
        request,
        ConnectionBoundCredential::new(
            ProviderConnectionId::from("pre-cancelled-runtime-connection"),
            Some("synthetic-pre-cancelled-secret".to_owned()),
        ),
        5_000,
        cancelled,
    )
    .await;

    assert!(matches!(
        outcome,
        TaskExecutionOutcome::Failed {
            classification: TaskDispatchClassification::KnownNoSideEffect,
            error: CoreError {
                code: CoreErrorCode::Cancelled,
                ..
            },
        }
    ));
    assert!(
        captured.recv_timeout(Duration::from_millis(50)).is_err(),
        "pre-cancelled runtime request reached the provider"
    );
}

#[test]
fn runtime_unknown_provider_outcome_is_non_recoverable() {
    let error =
        runtime_generation_result(unknown_task_outcome("synthetic post-dispatch cancellation"))
            .expect_err("unknown provider outcome must not become a successful runtime result");

    assert_eq!(error.code, CoreErrorCode::Internal);
    assert!(!error.recoverable);
    assert!(error.message.contains("outcome is unknown after dispatch"));
}

#[test]
fn provider_connection_update_cannot_rebind_endpoint_or_credential_identity() {
    let root = tempdir().expect("temporary core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let (api_origin, _) = spawn_model_list_provider(Vec::new());
    let (template, connection) = create_openai_chat_connection(&core, &api_origin);

    let mut ordinary_update = connection.clone();
    ordinary_update.display_name = "Renamed connection".to_owned();
    ordinary_update.timeout_seconds = 9;
    ordinary_update.status = ConnectionStatus::Connected;
    ordinary_update.created_at -= chrono::Duration::days(1);
    let updated = core
        .upsert_provider_connection(ordinary_update)
        .expect("safe connection update");
    assert_eq!(updated.display_name, "Renamed connection");
    assert_eq!(updated.timeout_seconds, 9);
    assert_eq!(updated.status, connection.status);
    assert_eq!(updated.created_at, connection.created_at);

    let mut origin_rebind = updated.clone();
    origin_rebind.api_origin =
        CanonicalOrigin::parse("http://127.0.0.1:65534").expect("other loopback origin");
    let error = core
        .upsert_provider_connection(origin_rebind)
        .expect_err("origin rebinding must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);

    let mut base_path_rebind = updated.clone();
    base_path_rebind.config.api_base_path =
        Some(EndpointPath::parse("/alternate-v1").expect("alternate base path"));
    let error = core
        .upsert_provider_connection(base_path_rebind)
        .expect_err("base-path rebinding must require a new connection");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.message.contains("endpoint configuration"));

    let mut value_rebind = updated.clone();
    value_rebind.config.values = vec![ConnectionConfigEntry {
        key: "api_base_url".to_owned(),
        value: ConnectionConfigValue::Text(format!("{}/alternate", api_origin.as_str())),
    }];
    let error = core
        .upsert_provider_connection(value_rebind)
        .expect_err("endpoint-affecting config values must require a new connection");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.message.contains("endpoint configuration"));

    let duplicate_create = core
        .create_provider_connection(ProviderConnectionDraft {
            id: updated.id.clone(),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: "Duplicate endpoint".to_owned(),
            api_origin: api_origin.clone(),
            api_base_path: Some(EndpointPath::parse("/alternate-v1").expect("alternate base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            values: vec![ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(format!("{}/alternate-v1", api_origin.as_str())),
            }],
            approved_credential_origin: Some(api_origin),
            local_network_approval: None,
            timeout_seconds: 5,
        })
        .expect_err("create cannot be used as an endpoint-identity upsert");
    assert_eq!(duplicate_create.code, CoreErrorCode::InvalidInput);
    assert!(
        duplicate_create
            .message
            .contains("identifier already exists")
    );

    let mut credential_rebind = updated.clone();
    credential_rebind.credential_ref = Some(CredentialRef("another-secret".to_owned()));
    let error = core
        .upsert_provider_connection(credential_rebind)
        .expect_err("credential rebinding must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);

    assert_eq!(
        core.inner
            .storage
            .get_provider_connection(&updated.id)
            .expect("unchanged provider identity")
            .config,
        updated.config
    );
}
