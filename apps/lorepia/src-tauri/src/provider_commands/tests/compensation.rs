
    #[test]
    fn startup_observe_only_leaves_matching_compensation_durable_without_native_effect() {
        let root = tempdir().expect("temporary root");
        let (shell, session, authority) = compensating_started_discovery_fixture(root.path());
        let attempt_id = session
            .commit_attempt_id
            .clone()
            .expect("compensation attempt");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&session.connection_id, &authority);
        let steps_before = shell
            .list_provider_discovery_compensation_steps(&attempt_id)
            .expect("pending compensation steps");

        tauri::async_runtime::block_on(async {
            let result = drive_provider_discovery_compensation_with(
                &vault,
                &shell,
                session.clone(),
                false,
                CompensationCredentialEffectPolicy::ObserveOnly,
                CompensationObserveErrorPolicy::Defer,
                None,
            )
            .await
            .expect("startup observation is non-mutating");

            match result {
                DiscoveryCompensationDriveResult::Finished(returned) => {
                    assert_eq!(returned, session);
                }
                DiscoveryCompensationDriveResult::NativeConfirmationRequired { .. } => {
                    panic!("bootstrap must never request or synthesize delete authority");
                }
            }
            assert_eq!(*calls.lock().expect("fake calls"), vec!["vault_observe"]);
            assert_eq!(
                vault.bound_status(&session.connection_id, &authority),
                CredentialStatus::Available
            );
            assert_eq!(
                shell
                    .list_provider_discovery_compensation_steps(&attempt_id)
                    .expect("unchanged compensation steps"),
                steps_before
            );
            assert_eq!(
                shell
                    .get_provider_discovery(&session.id)
                    .expect("unchanged compensating session"),
                session
            );
        });
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One end-to-end assertion chain pins both denial and success.
    fn explicit_compensation_rejects_stale_receipt_before_native_delete() {
        let root = tempdir().expect("temporary root");
        let (shell, session, authority) = compensating_started_discovery_fixture(root.path());
        let attempt_id = session
            .commit_attempt_id
            .clone()
            .expect("compensation attempt");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&session.connection_id, &authority);
        let steps_before = shell
            .list_provider_discovery_compensation_steps(&attempt_id)
            .expect("pending compensation steps");

        tauri::async_runtime::block_on(async {
            let initial = drive_provider_discovery_compensation_with(
                &vault,
                &shell,
                session.clone(),
                false,
                CompensationCredentialEffectPolicy::RequireNativeConfirmation,
                CompensationObserveErrorPolicy::Propagate,
                None,
            )
            .await
            .expect("preflight requests a native receipt without starting");
            let (prompted, exact_context) = match initial {
                DiscoveryCompensationDriveResult::NativeConfirmationRequired {
                    session,
                    context,
                } => (session, context),
                DiscoveryCompensationDriveResult::Finished(_) => {
                    panic!("matching slot must require fresh confirmation")
                }
            };
            assert_eq!(
                shell
                    .list_provider_discovery_compensation_steps(&attempt_id)
                    .expect("pre-prompt step remains pending"),
                steps_before
            );
            assert!(!calls.lock().expect("fake calls").contains(&"vault_delete"));

            let stale_context = NativeCredentialEffectContext::new(
                NativeCredentialEffect::DiscoveryCompensation,
                exact_context.target_id().to_owned(),
                exact_context.origin().to_owned(),
                format!("stale:{}", exact_context.revision()),
            )
            .expect("bounded stale authority context");
            let stale_receipt = vault
                .confirm_compensation(stale_context)
                .await
                .expect("simulate stale native approval");
            drive_provider_discovery_compensation_with(
                &vault,
                &shell,
                prompted.clone(),
                false,
                CompensationCredentialEffectPolicy::RequireNativeConfirmation,
                CompensationObserveErrorPolicy::Propagate,
                Some(stale_receipt),
            )
            .await
            .expect_err("stale authority receipt must fail before durable start or delete");
            assert_eq!(
                shell
                    .list_provider_discovery_compensation_steps(&attempt_id)
                    .expect("stale receipt leaves step pending"),
                steps_before
            );
            assert!(!calls.lock().expect("fake calls").contains(&"vault_delete"));
            assert_eq!(
                vault.bound_status(&session.connection_id, &authority),
                CredentialStatus::Available
            );

            let exact_receipt = vault
                .confirm_compensation(exact_context)
                .await
                .expect("fresh exact native approval");
            let completed = drive_provider_discovery_compensation_with(
                &vault,
                &shell,
                prompted,
                false,
                CompensationCredentialEffectPolicy::RequireNativeConfirmation,
                CompensationObserveErrorPolicy::Propagate,
                Some(exact_receipt),
            )
            .await
            .expect("fresh exact receipt permits one exact delete");
            assert!(matches!(
                completed,
                DiscoveryCompensationDriveResult::Finished(_)
            ));
            assert_eq!(
                vault.bound_status(&session.connection_id, &authority),
                CredentialStatus::Missing
            );
            assert_eq!(
                calls
                    .lock()
                    .expect("fake calls")
                    .iter()
                    .filter(|call| **call == "vault_delete")
                    .count(),
                1
            );
            let steps_after = shell
                .list_provider_discovery_compensation_steps(&attempt_id)
                .expect("completed compensation steps");
            assert!(
                steps_after
                    .iter()
                    .find(|step| step.kind == "remove_credential_slot")
                    .is_some_and(|step| step.status == "completed")
            );
        });
    }

    #[tokio::test]
    async fn compensation_deletes_only_the_producing_operation_slot() {
        let mut prior = install_context("started");
        prior.operation_id = "00000000-0000-4000-8000-000000000051".to_owned();
        prior.native_execution_reservation_id = Some("native-execution-A".to_owned());
        prior.native_execution_id = Some("native-execution-A".to_owned());
        let mut producing = prior.clone();
        producing.operation_id = "00000000-0000-4000-8000-000000000052".to_owned();
        producing.native_execution_reservation_id = Some("native-execution-B".to_owned());
        producing.native_execution_id = Some("native-execution-B".to_owned());
        assert_eq!(prior.commit_attempt_id, producing.commit_attempt_id);

        let prior_authority =
            discovery_credential_authority(&prior).expect("prior operation authority");
        let compensation_context = shell::ProviderDiscoveryCredentialAuthorityDto {
            operation_id: producing.operation_id.clone(),
            native_execution_id: producing
                .native_execution_id
                .clone()
                .expect("producing native execution authority"),
            commit_attempt_id: producing.commit_attempt_id.clone(),
            connection_id: producing.connection_id.clone(),
            credential_api_origin: "https://api.example".to_owned(),
            credential_origin_approval_id: "00000000-0000-4000-8000-000000000053".to_owned(),
            credential_origin_grant_sha256: "c".repeat(64),
            connection_binding_sha256: producing.connection_binding_sha256.clone(),
        };
        let producing_authority =
            discovery_compensation_credential_authority(&compensation_context)
                .expect("producing operation compensation authority");
        assert_eq!(
            producing_authority.authority_id(),
            producing
                .native_execution_id
                .as_deref()
                .expect("producing native execution")
        );
        assert_ne!(
            producing_authority.authority_id(),
            producing.commit_attempt_id.as_str(),
            "the reusable attempt ID must not select a physical compensation slot"
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let vault = FakeDiscoveryVault::new(Arc::clone(&calls));
        vault.insert_bound(&producing.connection_id, &prior_authority);
        vault.insert_bound(&producing.connection_id, &producing_authority);

        assert_eq!(
            observe_discovery_compensation_slot(
                &vault,
                &producing.connection_id,
                &producing_authority,
                CompensationObserveErrorPolicy::Propagate,
            )
            .await
            .expect("observe producing slot"),
            Some(BoundCredentialObservation::Match)
        );
        let (deleted, postflight) = delete_and_observe_discovery_bound_slot(
            &vault,
            &producing.connection_id,
            &producing_authority,
        )
        .await;
        deleted.expect("delete producing slot");
        assert_eq!(postflight, Ok(BoundCredentialObservation::Missing));
        assert_eq!(
            vault.bound_status(&producing.connection_id, &producing_authority),
            CredentialStatus::Missing
        );
        assert_eq!(
            vault.bound_status(&producing.connection_id, &prior_authority),
            CredentialStatus::Available,
            "compensation must not delete a prior retry operation's physical slot"
        );
    }

    #[tokio::test]
    async fn compensation_observe_error_defers_one_slot_while_another_advances() {
        let context = install_context("started");
        let authority = discovery_credential_authority(&context).expect("started authority");
        let blocked_calls = Arc::new(Mutex::new(Vec::new()));
        let blocked = FakeDiscoveryVault::new(Arc::clone(&blocked_calls));
        blocked.insert_raw(&context.connection_id);
        blocked.insert_bound(&context.connection_id, &authority);
        blocked.fail_observe();

        assert_eq!(
            observe_discovery_compensation_slot(
                &blocked,
                &context.connection_id,
                &authority,
                CompensationObserveErrorPolicy::Defer,
            )
            .await
            .expect("startup observation errors are deferred"),
            None,
            "a backend read error defers this pending compensation without claiming it"
        );
        assert!(
            observe_discovery_compensation_slot(
                &blocked,
                &context.connection_id,
                &authority,
                CompensationObserveErrorPolicy::Propagate,
            )
            .await
            .is_err(),
            "an explicit compensation command still surfaces the platform error"
        );
        assert_eq!(
            *blocked_calls.lock().expect("blocked calls"),
            vec!["vault_observe", "vault_observe"]
        );
        assert_eq!(
            blocked.bound_status(&context.connection_id, &authority),
            CredentialStatus::Available,
            "the exact slot remains retryable and no delete was attempted"
        );

        let ready_calls = Arc::new(Mutex::new(Vec::new()));
        let ready = FakeDiscoveryVault::new(Arc::clone(&ready_calls));
        ready.insert_raw(&context.connection_id);
        ready.insert_bound(&context.connection_id, &authority);
        ready.fail_delete_after_effect();
        assert_eq!(
            observe_discovery_compensation_slot(
                &ready,
                &context.connection_id,
                &authority,
                CompensationObserveErrorPolicy::Defer,
            )
            .await
            .expect("ready recovery observation"),
            Some(BoundCredentialObservation::Match),
            "a later recovery candidate can still advance"
        );
        let (delete_result, postflight) =
            delete_and_observe_discovery_bound_slot(&ready, &context.connection_id, &authority)
                .await;
        assert!(delete_result.is_err(), "simulate lost delete response");
        assert_eq!(
            credential_compensation_delete_outcome(&delete_result, &postflight),
            CredentialCompensationDeleteOutcome::Complete,
            "authoritative Missing postflight completes despite mutate-then-error"
        );
        assert_eq!(
            ready.raw_status(&context.connection_id),
            CredentialStatus::Available,
            "exact compensation deletion preserves the raw legacy sentinel"
        );

        blocked.restore_observe();
        assert_eq!(
            observe_discovery_compensation_slot(
                &blocked,
                &context.connection_id,
                &authority,
                CompensationObserveErrorPolicy::Defer,
            )
            .await
            .expect("retry deferred observation"),
            Some(BoundCredentialObservation::Match)
        );
        let (delete_result, postflight) =
            delete_and_observe_discovery_bound_slot(&blocked, &context.connection_id, &authority)
                .await;
        delete_result.expect("retry exact deferred slot");
        assert_eq!(postflight, Ok(BoundCredentialObservation::Missing));
        assert_eq!(
            blocked.raw_status(&context.connection_id),
            CredentialStatus::Available
        );
    }

    #[test]
    fn discovery_compensation_recovery_required_never_accepts_visible_missing() {
        let delete_result = Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
        let postflight = Ok(BoundCredentialObservation::Missing);

        assert_eq!(
            credential_compensation_delete_outcome(&delete_result, &postflight),
            CredentialCompensationDeleteOutcome::Unknown,
            "explicit recovery-required must outrank immediate visibility"
        );
    }

    #[test]
    fn provider_curl_ingress_is_nonempty_and_bounded() {
        assert!(bounded_secret_curl(String::new()).is_err());
        assert!(bounded_secret_curl(" \n".to_owned()).is_err());
        assert!(bounded_secret_curl("curl https://example.test".to_owned()).is_ok());
        assert!(bounded_secret_curl("x".repeat(MAXIMUM_PROVIDER_CURL_BYTES + 1)).is_err());
    }

    #[test]
    fn credential_install_recovery_requires_started_wal_provenance() {
        for cancellation_pending in [false, true] {
            for credential_status in [
                CredentialStatus::Missing,
                CredentialStatus::Available,
                CredentialStatus::Unreadable,
            ] {
                assert_eq!(
                    credential_install_recovery_action(
                        cancellation_pending,
                        "prepared",
                        credential_status,
                    )
                    .expect("prepared recovery state"),
                    CredentialInstallRecoveryAction::DeferToCore,
                    "a native effect cannot be inferred before the durable started marker"
                );

                let expected = CredentialInstallRecoveryAction::DeferToCore;
                assert_eq!(
                    credential_install_recovery_action(
                        cancellation_pending,
                        "started",
                        credential_status,
                    )
                    .expect("started recovery state"),
                    expected
                );
            }
        }

        for cancellation_pending in [false, true] {
            for operation_status in ["", "completed", "unknown_outcome"] {
                for credential_status in [
                    CredentialStatus::Missing,
                    CredentialStatus::Available,
                    CredentialStatus::Unreadable,
                ] {
                    assert!(
                        credential_install_recovery_action(
                            cancellation_pending,
                            operation_status,
                            credential_status,
                        )
                        .is_err(),
                        "an unrecognized WAL status must fail closed"
                    );
                }
            }
        }
    }

    #[test]
    fn unreadable_recovery_observation_defers_to_core_without_aborting_bootstrap() {
        let status = status_only_bound_observation(Err(PlatformError::new(
            PlatformErrorCode::StorageUnavailable,
        )));
        assert_eq!(status, CredentialStatus::Unreadable);
        assert_eq!(
            credential_install_recovery_action(false, "started", status)
                .expect("unreadable started recovery classification"),
            CredentialInstallRecoveryAction::DeferToCore
        );

        assert_eq!(
            status_only_bound_observation(Ok(BoundCredentialObservation::Match)),
            CredentialStatus::Available,
            "a legitimate exact envelope remains resumable"
        );
    }

    #[test]
    fn assistant_turn_request_rejects_renderer_estimates() {
        let request = json!({
            "session_id": "synthetic-session",
            "estimate": {
                "input_tokens": 1,
                "maximum_output_tokens": 1,
                "maximum_cost_micro_units": 0
            }
        });

        serde_json::from_value::<ProviderDiscoverySessionRequest>(request)
            .expect_err("renderer-authored estimates must not cross Tauri IPC");
    }

    #[test]
    fn session_scoped_outbox_poll_request_is_exact_and_bounded_by_rust() {
        let request =
            serde_json::from_value::<PollProviderDiscoveryEventsForSessionRequest>(json!({
                "session_id": "selected-session",
                "limit": 100
            }))
            .expect("decode session-scoped poll request");
        assert_eq!(request.session_id, "selected-session");
        assert_eq!(request.limit, 100);

        serde_json::from_value::<PollProviderDiscoveryEventsForSessionRequest>(json!({
            "limit": 100
        }))
        .expect_err("session-scoped polling requires a session id");
        serde_json::from_value::<PollProviderDiscoveryEventsForSessionRequest>(json!({
            "session_id": "selected-session",
            "limit": 100,
            "acknowledge_foreign_sessions": true
        }))
        .expect_err("the WebView cannot request foreign-session acknowledgement");
    }

    #[test]
    fn assistant_turn_fails_closed_without_application_or_platform_state() {
        let error = run_provider_discovery_assistant_turn(ProviderDiscoverySessionRequest {
            session_id: "synthetic-session".to_owned(),
        })
        .expect_err("remote assistant execution must remain unavailable");

        assert_eq!(error.code, "assistant_pricing_unavailable");
        assert_eq!(
            error.message_key,
            "provider.discovery.assistant_pricing_unavailable"
        );
        assert!(!error.recoverable);
    }

    fn prepare_precommit_discovery(
        shell: &shell::ShellApi,
        connection_id: &str,
    ) -> shell::ProviderDiscoverySessionDto {
        let selecting = shell
            .begin_provider_discovery(shell::BeginProviderDiscoveryInput {
                connection_id: connection_id.to_owned(),
                display_name: "Synthetic precommit discovery".to_owned(),
                site_url: "https://openrouter.ai/".to_owned(),
                docs_url: None,
                credential_binding_requested: true,
                preferred_assistant: None,
                connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                    values: Vec::new(),
                    api_base_path: None,
                    timeout_seconds: 30,
                    network_mode: shell::ProviderNetworkModeInput::Public,
                    local_network_approval: None,
                },
                supplied_evidence_ids: Vec::new(),
                source: shell::BeginProviderDiscoverySourceInput::KnownProvider {
                    template_id: "openrouter-v1".to_owned(),
                },
            })
            .expect("begin synthetic discovery");
        let candidate = shell
            .list_provider_discovery_candidates(&selecting.id)
            .expect("list discovery candidates")
            .into_iter()
            .find(|candidate| {
                matches!(
                    &candidate.summary,
                    shell::DiscoveryCandidateSummaryDto::ProviderTemplate { template_id, .. }
                        if template_id == "openrouter-v1"
                )
            })
            .expect("OpenRouter candidate");
        let approval = shell
            .continue_provider_discovery(
                shell::ContinueProviderDiscoveryInput {
                    session_id: selecting.id,
                    action_id: "00000000-0000-4000-8000-000000000010".to_owned(),
                    expected_revision: selecting.revision,
                    action: shell::ContinueProviderDiscoveryActionInput::SelectTemplate {
                        candidate_id: candidate.id,
                    },
                },
                None,
            )
            .expect("select credential-bound template");
        assert_eq!(approval.state, "awaiting_credential_origin_approval");
        approval
    }

    #[test]
    fn discovery_cancellation_transition_does_not_wait_for_global_credential_lock() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let awaiting = prepare_precommit_discovery(&shell, "prompt-cancellation");
        let state = AppState::new(root.path().to_path_buf());
        let _operation = tauri::async_runtime::block_on(state.lock_provider_credential_operation());

        let cancelled = request_provider_discovery_cancellation(
            &state,
            &shell,
            &CancelProviderDiscoveryRequest {
                session_id: awaiting.id,
                expected_revision: awaiting.revision,
            },
        )
        .expect("durable cancellation transition must not wait for the credential gate");

        assert_eq!(cancelled.state, "cancelled");
    }

    #[test]
    fn accepted_discovery_cancellation_revokes_the_registered_authenticated_request() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let awaiting = prepare_precommit_discovery(&shell, "registered-cancellation");
        let state = AppState::new(root.path().to_path_buf());
        let (_registration, cancelled) =
            register_active_discovery_request(&awaiting.id).expect("register active request");
        assert!(!*cancelled.borrow());

        let session = request_provider_discovery_cancellation(
            &state,
            &shell,
            &CancelProviderDiscoveryRequest {
                session_id: awaiting.id,
                expected_revision: awaiting.revision,
            },
        )
        .expect("accept cancellation");

        assert_eq!(session.state, "cancelled");
        assert!(
            *cancelled.borrow(),
            "an accepted cancellation must revoke the authenticated dispatch token"
        );
    }

    fn credential_connection_input(
        shell: &shell::ShellApi,
        connection_id: &str,
    ) -> shell::CreateProviderConnectionInput {
        let template = shell
            .list_provider_templates()
            .expect("list provider templates")
            .into_iter()
            .find(|template| template.id == "openrouter-v1")
            .expect("OpenRouter credential-bound public template");
        let origin = template.default_api_origin.expect("template origin");
        shell::CreateProviderConnectionInput {
            id: connection_id.to_owned(),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: format!("Synthetic {connection_id}"),
            api_origin: origin.clone(),
            api_base_path: None,
            network_mode: shell::ProviderNetworkModeInput::Public,
            local_network_approval: None,
            values: Vec::new(),
            approved_credential_origin: Some(origin),
            timeout_seconds: 30,
        }
    }

    fn local_model_sync_connection_input(
        shell: &shell::ShellApi,
        connection_id: &str,
        origin: &str,
    ) -> shell::CreateProviderConnectionInput {
        let template = shell
            .list_provider_templates()
            .expect("list provider templates")
            .into_iter()
            .find(|template| template.id == "openai-chat-compatible-v1")
            .expect("OpenAI-compatible model-list template");
        shell::CreateProviderConnectionInput {
            id: connection_id.to_owned(),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: "Synthetic leased model sync".to_owned(),
            api_origin: origin.to_owned(),
            api_base_path: Some("/v1".to_owned()),
            network_mode: shell::ProviderNetworkModeInput::LocalLoopback,
            local_network_approval: None,
            values: vec![shell::ConnectionConfigEntryDto {
                key: "api_base_url".to_owned(),
                value: shell::ConnectionConfigValueDto::Text(format!("{origin}/v1")),
            }],
            approved_credential_origin: Some(origin.to_owned()),
            timeout_seconds: 5,
        }
    }

    fn spawn_blocking_model_list_provider() -> (
        String,
        mpsc::Receiver<String>,
        mpsc::Sender<()>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind model-list provider");
        let address = listener.local_addr().expect("model-list provider address");
        let (request_sender, request_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept model-list request");
            let request = read_http_headers(&mut stream);
            request_sender
                .send(request)
                .expect("report model-list request");
            release_receiver
                .recv()
                .expect("release model-list response");
            let body = r#"{"data":[{"id":"leased-model"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write model-list response");
        });
        (
            format!("http://{address}"),
            request_receiver,
            release_sender,
            handle,
        )
    }

    fn read_http_headers(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("model-list request timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let read = stream.read(&mut buffer).expect("read model-list request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        String::from_utf8(request).expect("model-list request is UTF-8")
    }

    fn credentialless_connection_input(
        shell: &shell::ShellApi,
        connection_id: &str,
    ) -> shell::CreateProviderConnectionInput {
        let template = shell
            .list_provider_templates()
            .expect("list provider templates")
            .into_iter()
            .find(|template| !template.credential_required && template.default_api_origin.is_some())
            .expect("credentialless template with a default origin");
        let origin = template.default_api_origin.expect("template origin");
        let network_mode = match template.default_network_mode.as_str() {
            "public" => shell::ProviderNetworkModeInput::Public,
            "local_loopback" => shell::ProviderNetworkModeInput::LocalLoopback,
            other => panic!("unexpected credentialless network mode: {other}"),
        };
        shell::CreateProviderConnectionInput {
            id: connection_id.to_owned(),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: format!("Synthetic {connection_id}"),
            api_origin: origin,
            api_base_path: None,
            network_mode,
            local_network_approval: None,
            values: Vec::new(),
            approved_credential_origin: None,
            timeout_seconds: 30,
        }
    }

    fn install_provider_credential(
        shell: &shell::ShellApi,
        connection_id: &str,
    ) -> shell::ProviderCredentialAccessAuthorityContext {
        let authority = shell
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose test credential install authority");
        let install = shell
            .prepare_provider_credential_install_operation(
                connection_id,
                &authority,
                shell::ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("prepare test credential install");
        shell
            .start_provider_credential_operation(&install.operation_id, &install.plan_sha256)
            .expect("start test credential install");
        shell
            .finish_provider_credential_operation(
                &install.operation_id,
                &install.plan_sha256,
                shell::ProviderCredentialSlotStatusInput::Available,
            )
            .expect("finish test credential install");
        shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("read installed credential authority")
    }

    fn remove_provider_credential(shell: &shell::ShellApi, connection_id: &str) {
        let removal = shell
            .prepare_provider_credential_operation(
                connection_id,
                shell::ProviderCredentialOperationKindInput::RemoveCredential,
                shell::ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare test credential removal");
        shell
            .start_provider_credential_operation(&removal.operation_id, &removal.plan_sha256)
            .expect("start test credential removal");
        shell
            .finish_provider_credential_operation(
                &removal.operation_id,
                &removal.plan_sha256,
                shell::ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("finish test credential removal");
    }

    fn discovery_input(
        connection: &shell::ProviderConnectionDto,
        source: shell::BeginProviderDiscoverySourceInput,
    ) -> shell::BeginProviderDiscoveryInput {
        shell::BeginProviderDiscoveryInput {
            connection_id: connection.id.clone(),
            display_name: format!("Discovery {}", connection.id),
            site_url: connection.api_origin.clone(),
            docs_url: None,
            credential_binding_requested: connection.credential_binding_required,
            preferred_assistant: None,
            connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                values: Vec::new(),
                api_base_path: connection.api_base_path.clone(),
                timeout_seconds: connection.timeout_seconds,
                network_mode: match connection.network_mode.as_str() {
                    "public" => shell::ProviderNetworkModeInput::Public,
                    "local_loopback" => shell::ProviderNetworkModeInput::LocalLoopback,
                    other => panic!("unexpected discovery network mode: {other}"),
                },
                local_network_approval: None,
            },
            supplied_evidence_ids: Vec::new(),
            source,
        }
    }

    fn discovery_curl_input(
        connection: &shell::ProviderConnectionDto,
    ) -> shell::BeginProviderDiscoveryCurlInput {
        shell::BeginProviderDiscoveryCurlInput {
            connection_id: connection.id.clone(),
            display_name: format!("cURL discovery {}", connection.id),
            docs_url: None,
            credential_binding_requested: connection.credential_binding_required,
            preferred_assistant: None,
            connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                values: Vec::new(),
                api_base_path: connection.api_base_path.clone(),
                timeout_seconds: connection.timeout_seconds,
                network_mode: match connection.network_mode.as_str() {
                    "public" => shell::ProviderNetworkModeInput::Public,
                    "local_loopback" => shell::ProviderNetworkModeInput::LocalLoopback,
                    other => panic!("unexpected discovery network mode: {other}"),
                },
                local_network_approval: None,
            },
            supplied_evidence_ids: Vec::new(),
        }
    }

    fn install_context(
        operation_status: &str,
    ) -> shell::ProviderDiscoveryCredentialInstallContextDto {
        let native_execution_id =
            (operation_status == "started").then(|| Uuid::new_v4().to_string());
        shell::ProviderDiscoveryCredentialInstallContextDto {
            session_id: "handoff-session".to_owned(),
            session_revision: 9,
            operation_id: "handoff-operation".to_owned(),
            operation_status: operation_status.to_owned(),
            native_execution_reservation_id: native_execution_id.clone(),
            native_execution_id,
            commit_attempt_id: "00000000-0000-4000-8000-000000000020".to_owned(),
            commit_plan_sha256: "a".repeat(64),
            commit_phase: "prepared".to_owned(),
            connection_id: "handoff-connection".to_owned(),
            connection_binding_sha256: "b".repeat(64),
        }
    }

    fn commit_candidate(
        context: &shell::ProviderDiscoveryCredentialInstallContextDto,
    ) -> DiscoveryCredentialCommitCandidate {
        DiscoveryCredentialCommitCandidate {
            session_id: context.session_id.clone(),
            session_revision: context.session_revision,
            connection_id: context.connection_id.clone(),
            commit_attempt_id: context.commit_attempt_id.clone(),
            commit_plan_sha256: context.commit_plan_sha256.clone(),
        }
    }

    fn commit_lease_binding(
        context: &shell::ProviderDiscoveryCredentialInstallContextDto,
    ) -> DiscoveryCredentialLeaseBinding {
        DiscoveryCredentialLeaseBinding {
            session_id: context.session_id.clone(),
            connection_id: context.connection_id.clone(),
            credential_origin_approval_id: "00000000-0000-4000-8000-000000000030".to_owned(),
            credential_origin_grant_sha256: "c".repeat(64),
            connection_binding_sha256: context.connection_binding_sha256.clone(),
        }
    }
