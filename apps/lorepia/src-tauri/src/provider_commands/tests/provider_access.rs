
    #[test]
    fn discovery_compensation_confirmation_displays_backend_credential_api_origin() {
        let root = tempdir().expect("temporary root");
        let (shell, session, _authority) = compensating_started_discovery_fixture(root.path());
        let authority = shell
            .get_provider_discovery_credential_compensation_authority(&session.id)
            .expect("load compensation credential authority");
        let context = discovery_compensation_confirmation_context(&session, &authority)
            .expect("build compensation confirmation");

        assert_eq!(session.site_url, "https://docs.openrouter.example/");
        assert_eq!(
            context.origin(),
            "https://openrouter.ai",
            "credential deletion prompt must display the API origin bound to the slot"
        );
        let trusted_revision = context.revision().to_owned();
        let mut substituted_grant = authority.clone();
        substituted_grant.credential_origin_grant_sha256 = "f".repeat(64);
        assert_ne!(
            discovery_compensation_confirmation_context(&session, &substituted_grant)
                .expect("build substituted-grant compensation context")
                .revision(),
            trusted_revision,
            "compensation confirmation cannot be replayed with a substituted origin grant"
        );
        let mut substituted_binding = authority.clone();
        substituted_binding.connection_binding_sha256 = "e".repeat(64);
        assert_ne!(
            discovery_compensation_confirmation_context(&session, &substituted_binding)
                .expect("build substituted-binding compensation context")
                .revision(),
            trusted_revision,
            "compensation confirmation cannot be replayed with a substituted slot binding"
        );

        let mut same_origin_site = session.clone();
        same_origin_site.site_url = "https://openrouter.ai/".to_owned();
        let same_origin_context =
            discovery_compensation_confirmation_context(&same_origin_site, &authority)
                .expect("same-origin compensation remains valid");
        assert_eq!(same_origin_context.origin(), "https://openrouter.ai");
    }

    impl DiscoveryCredentialInstallJournal for FakeDiscoveryJournal {
        fn install_context(
            &self,
            session_id: &str,
        ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
            self.calls.lock().expect("fake calls").push("wal_context");
            let context = self.context.lock().expect("fake journal").clone();
            if context.session_id != session_id {
                return Err(CommandError::invalid_input());
            }
            Ok(context)
        }

        fn reserve_install(
            &self,
            session_id: &str,
            expected_revision: u64,
            operation_id: &str,
            commit_attempt_id: &str,
            commit_plan_sha256: &str,
        ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
            self.calls.lock().expect("fake calls").push("wal_reserved");
            let mut context = self.context.lock().expect("fake journal");
            if context.session_id != session_id
                || context.session_revision != expected_revision
                || context.operation_id != operation_id
                || context.commit_attempt_id != commit_attempt_id
                || context.commit_plan_sha256 != commit_plan_sha256
                || context.operation_status != "prepared"
                || context.native_execution_id.is_some()
            {
                return Err(CommandError::invalid_input());
            }
            let reservation_id = self.next_native_execution_id();
            if context.native_execution_reservation_id.is_some() {
                return Err(CommandError::invalid_input());
            }
            context.native_execution_reservation_id = Some(reservation_id);
            Ok(context.clone())
        }

        fn start_install(
            &self,
            session_id: &str,
            expected_revision: u64,
            operation_id: &str,
            commit_attempt_id: &str,
            commit_plan_sha256: &str,
            native_execution_reservation_id: &str,
        ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
            self.calls.lock().expect("fake calls").push("wal_started");
            let mut context = self.context.lock().expect("fake journal");
            if context.session_id != session_id
                || context.session_revision != expected_revision
                || context.operation_id != operation_id
                || context.commit_attempt_id != commit_attempt_id
                || context.commit_plan_sha256 != commit_plan_sha256
                || context.operation_status != "prepared"
                || context.native_execution_reservation_id.as_deref()
                    != Some(native_execution_reservation_id)
                || context.native_execution_id.is_some()
            {
                return Err(CommandError::invalid_input());
            }
            context.operation_status = "started".to_owned();
            context.native_execution_id = Some(native_execution_reservation_id.to_owned());
            let mut returned = context.clone();
            if std::mem::take(
                &mut *self
                    .mismatch_started_context
                    .lock()
                    .expect("fake started mismatch"),
            ) {
                returned.connection_binding_sha256 = "f".repeat(64);
            }
            Ok(returned)
        }

        fn attest_no_effect(
            &self,
            session_id: &str,
            operation_id: &str,
            commit_attempt_id: &str,
            commit_plan_sha256: &str,
            native_execution_id: &str,
        ) -> CommandResult<()> {
            self.calls.lock().expect("fake calls").push("wal_no_effect");
            let context = self.context.lock().expect("fake journal");
            if context.session_id != session_id
                || context.operation_id != operation_id
                || context.commit_attempt_id != commit_attempt_id
                || context.commit_plan_sha256 != commit_plan_sha256
                || context.operation_status != "started"
                || context.native_execution_id.as_deref() != Some(native_execution_id)
            {
                return Err(CommandError::invalid_input());
            }
            Ok(())
        }

        fn mark_durability_unknown(
            &self,
            session_id: &str,
            expected_revision: u64,
            operation_id: &str,
            commit_attempt_id: &str,
            commit_plan_sha256: &str,
            native_execution_id: &str,
            connection_id: &str,
            connection_binding_sha256: &str,
        ) -> CommandResult<()> {
            self.calls.lock().expect("fake calls").push("wal_unknown");
            let mut context = self.context.lock().expect("fake journal");
            if context.session_id != session_id
                || context.session_revision != expected_revision
                || context.operation_id != operation_id
                || context.commit_attempt_id != commit_attempt_id
                || context.commit_plan_sha256 != commit_plan_sha256
                || context.operation_status != "started"
                || context.native_execution_reservation_id.as_deref() != Some(native_execution_id)
                || context.native_execution_id.as_deref() != Some(native_execution_id)
                || context.connection_id != connection_id
                || context.connection_binding_sha256 != connection_binding_sha256
            {
                return Err(CommandError::invalid_input());
            }
            context.operation_status = "outcome_unknown".to_owned();
            Ok(())
        }
    }

    #[test]
    fn product_create_rejects_retained_orphan_before_shell_insert_and_missing_proceeds() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");

        for (status, connection_id) in [
            (CredentialStatus::Available, "orphan-create"),
            (CredentialStatus::Unreadable, "unreadable-create"),
        ] {
            let guard = FakeNewConnectionSlotGuard::new(status);
            let result =
                tauri::async_runtime::block_on(create_provider_connection_with_slot_guard(
                    &shell,
                    credential_connection_input(&shell, connection_id),
                    &guard,
                ));
            match result {
                Ok(_) => panic!("a retained or unreadable slot must block product create"),
                Err(error) => assert_eq!(error.code, "invalid_input"),
            }
            assert_eq!(
                *guard.calls.lock().expect("fake slot calls"),
                vec![connection_id.to_owned()]
            );
        }
        assert!(
            shell
                .list_provider_connections()
                .expect("list rejected product creates")
                .is_empty(),
            "the Shell insert must remain downstream of the slot guard"
        );

        let missing = FakeNewConnectionSlotGuard::new(CredentialStatus::Missing);
        let created = tauri::async_runtime::block_on(create_provider_connection_with_slot_guard(
            &shell,
            credential_connection_input(&shell, "missing-create"),
            &missing,
        ))
        .expect("a missing slot permits product create");
        assert_eq!(created.id, "missing-create");

        // A reset database does not authorize a retained item, even when a
        // renderer proposes a different origin for the reused slot ID.
        let reset_root = tempdir().expect("reset database root");
        let reset_shell = shell::ShellApi::open_data_root(reset_root.path()).expect("reset Shell");
        let retained = FakeNewConnectionSlotGuard::new(CredentialStatus::Available);
        let mut reset_input = credential_connection_input(&reset_shell, "retained-after-reset");
        reset_input.api_origin = "https://different-origin.example.test".to_owned();
        reset_input.approved_credential_origin = Some(reset_input.api_origin.clone());
        let reset_result = tauri::async_runtime::block_on(
            create_provider_connection_with_slot_guard(&reset_shell, reset_input, &retained),
        );
        assert!(reset_result.is_err());
        assert!(
            reset_shell
                .list_provider_connections()
                .expect("list reset product creates")
                .is_empty()
        );
    }

    #[derive(Clone, Copy)]
    enum ProductDiscoveryStart {
        Known,
        Site,
        Curl,
    }

    async fn begin_product_discovery_with_reader<R: ExistingConnectionCredentialReader + ?Sized>(
        shell: &shell::ShellApi,
        connection: &shell::ProviderConnectionDto,
        start: ProductDiscoveryStart,
        reader: &R,
    ) -> CommandResult<shell::ProviderDiscoverySessionDto> {
        match start {
            ProductDiscoveryStart::Known => {
                begin_provider_discovery_with_reader(
                    shell,
                    discovery_input(
                        connection,
                        shell::BeginProviderDiscoverySourceInput::KnownProvider {
                            template_id: connection.template_id.clone(),
                        },
                    ),
                    reader,
                )
                .await
            }
            ProductDiscoveryStart::Site => {
                begin_provider_discovery_with_reader(
                    shell,
                    discovery_input(connection, shell::BeginProviderDiscoverySourceInput::Site),
                    reader,
                )
                .await
            }
            ProductDiscoveryStart::Curl => {
                begin_provider_discovery_curl_with_reader(
                    shell,
                    discovery_curl_input(connection),
                    shell::SecretProviderCurl::new(format!(
                        "curl {}{}/models",
                        connection.api_origin.trim_end_matches('/'),
                        connection.api_base_path.as_deref().unwrap_or_default()
                    )),
                    reader,
                )
                .await
            }
        }
    }

    #[test]
    fn product_discovery_forwards_exact_authority_and_rejects_stale_reads_for_all_sources() {
        for (suffix, start) in [
            ("known", ProductDiscoveryStart::Known),
            ("site", ProductDiscoveryStart::Site),
            ("curl", ProductDiscoveryStart::Curl),
        ] {
            let root = tempdir().expect("temporary root");
            let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
            let connection_id = format!("stale-product-discovery-{suffix}");
            let connection = shell
                .create_provider_connection(credential_connection_input(&shell, &connection_id))
                .expect("create credential-bound connection");
            let cached_authority = install_provider_credential(&shell, &connection_id);
            let reader = FakeExistingConnectionCredentialReader::new(Some(
                crate::credential_operations::ProviderConnectionCredentialRead {
                    credential: Some(NativeCredential::new("cached-discovery-secret".to_owned())),
                    access_authority: cached_authority,
                },
            ));
            remove_provider_credential(&shell, &connection_id);

            let error = tauri::async_runtime::block_on(begin_product_discovery_with_reader(
                &shell,
                &connection,
                start,
                &reader,
            ))
            .expect_err("terminal removal must reject the exact cached authority");
            assert_eq!(error.code, "invalid_input");
            assert_eq!(
                *reader.calls.lock().expect("fake reader calls"),
                vec![connection_id]
            );
            assert!(
                shell
                    .list_provider_discoveries(32)
                    .expect("list rejected discoveries")
                    .is_empty()
            );
            assert!(
                shell
                    .poll_provider_discovery_events(32)
                    .expect("poll rejected discovery events")
                    .is_empty()
            );
        }
    }

    #[test]
    fn product_discovery_forwards_current_exact_authority_for_all_sources() {
        for (suffix, start) in [
            ("known", ProductDiscoveryStart::Known),
            ("site", ProductDiscoveryStart::Site),
            ("curl", ProductDiscoveryStart::Curl),
        ] {
            let root = tempdir().expect("temporary root");
            let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
            let connection_id = format!("current-product-discovery-{suffix}");
            let connection = shell
                .create_provider_connection(credential_connection_input(&shell, &connection_id))
                .expect("create credential-bound connection");
            let current_authority = install_provider_credential(&shell, &connection_id);
            let reader = FakeExistingConnectionCredentialReader::new(Some(
                crate::credential_operations::ProviderConnectionCredentialRead {
                    credential: Some(NativeCredential::new("current-discovery-secret".to_owned())),
                    access_authority: current_authority,
                },
            ));

            let session = tauri::async_runtime::block_on(begin_product_discovery_with_reader(
                &shell,
                &connection,
                start,
                &reader,
            ))
            .unwrap_or_else(|error| {
                panic!("current exact authority starts {suffix} discovery: {error:?}")
            });
            assert_eq!(session.connection_id, connection_id);
            assert_eq!(
                *reader.calls.lock().expect("fake reader calls"),
                vec![connection_id]
            );
            assert_eq!(
                shell
                    .list_provider_discoveries(32)
                    .expect("list admitted discoveries")
                    .len(),
                1
            );
            assert!(
                !shell
                    .poll_provider_discovery_events(32)
                    .expect("poll admitted discovery events")
                    .is_empty()
            );
        }
    }

    #[test]
    fn product_discovery_and_model_sync_keep_credentialless_authority_absent() {
        for (suffix, start) in [
            ("known", ProductDiscoveryStart::Known),
            ("site", ProductDiscoveryStart::Site),
            ("curl", ProductDiscoveryStart::Curl),
        ] {
            let root = tempdir().expect("temporary root");
            let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
            let connection_id = format!("credentialless-product-discovery-{suffix}");
            let connection = shell
                .create_provider_connection(credentialless_connection_input(&shell, &connection_id))
                .expect("create credentialless connection");
            let reader = FakeExistingConnectionCredentialReader::new(None);

            let session = tauri::async_runtime::block_on(begin_product_discovery_with_reader(
                &shell,
                &connection,
                start,
                &reader,
            ))
            .expect("credentialless discovery starts without reading an authority");
            assert_eq!(session.connection_id, connection_id);
            assert!(reader.calls.lock().expect("fake reader calls").is_empty());
        }

        let root = tempdir().expect("temporary model-sync root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let connection_id = "credentialless-product-model-sync";
        shell
            .create_provider_connection(credentialless_connection_input(&shell, connection_id))
            .expect("create credentialless model-sync connection");
        let reader = FakeExistingConnectionCredentialReader::new(None);
        tauri::async_runtime::block_on(start_provider_model_sync_with_reader(
            &shell,
            connection_id,
            &reader,
            None,
        ))
        .expect("credentialless model sync starts without reading an authority");
        assert!(reader.calls.lock().expect("fake reader calls").is_empty());
    }

    #[test]
    fn product_model_sync_forwards_exact_authority_and_rejects_a_stale_read() {
        let current_root = tempdir().expect("temporary current root");
        let current_shell =
            shell::ShellApi::open_data_root(current_root.path()).expect("open current Shell");
        let current_connection_id = "current-product-model-sync";
        current_shell
            .create_provider_connection(credential_connection_input(
                &current_shell,
                current_connection_id,
            ))
            .expect("create current credential-bound connection");
        let current_authority = install_provider_credential(&current_shell, current_connection_id);
        let current_reader = FakeExistingConnectionCredentialReader::new(Some(
            crate::credential_operations::ProviderConnectionCredentialRead {
                credential: Some(NativeCredential::new(
                    "current-model-sync-secret".to_owned(),
                )),
                access_authority: current_authority,
            },
        ));
        let started = tauri::async_runtime::block_on(start_provider_model_sync_with_reader(
            &current_shell,
            current_connection_id,
            &current_reader,
            None,
        ))
        .expect("current exact authority starts product model sync");
        assert!(!started.job_id.is_empty());
        assert_eq!(
            *current_reader.calls.lock().expect("fake reader calls"),
            vec![current_connection_id.to_owned()]
        );

        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let connection_id = "stale-product-model-sync";
        shell
            .create_provider_connection(credential_connection_input(&shell, connection_id))
            .expect("create credential-bound connection");
        let cached_authority = install_provider_credential(&shell, connection_id);
        let reader = FakeExistingConnectionCredentialReader::new(Some(
            crate::credential_operations::ProviderConnectionCredentialRead {
                credential: Some(NativeCredential::new("cached-model-sync-secret".to_owned())),
                access_authority: cached_authority,
            },
        ));
        remove_provider_credential(&shell, connection_id);

        let error = tauri::async_runtime::block_on(start_provider_model_sync_with_reader(
            &shell,
            connection_id,
            &reader,
            None,
        ))
        .expect_err("terminal removal must reject the exact cached model-sync authority");
        assert_eq!(error.code, "invalid_input");
        assert_eq!(
            *reader.calls.lock().expect("fake reader calls"),
            vec![connection_id.to_owned()]
        );
    }

    #[tokio::test]
    async fn model_sync_carrier_blocks_credential_mutation_until_provider_finishes() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let state = Arc::new(AppState::new(root.path().to_path_buf()));
        let connection_id = "leased-product-model-sync";
        let (origin, request_receiver, provider_release, provider_thread) =
            spawn_blocking_model_list_provider();
        shell
            .create_provider_connection(local_model_sync_connection_input(
                &shell,
                connection_id,
                &origin,
            ))
            .expect("create leased model-sync connection");
        let authority = install_provider_credential(&shell, connection_id);
        let reader = FakeExistingConnectionCredentialReader::new(Some(
            crate::credential_operations::ProviderConnectionCredentialRead {
                credential: Some(NativeCredential::new(
                    "synthetic-leased-model-sync-secret".to_owned(),
                )),
                access_authority: authority,
            },
        ));
        let dispatch_lease = state.lease_provider_credential_operation().await;
        let started = start_provider_model_sync_with_reader(
            &shell,
            connection_id,
            &reader,
            Some(shell::TaskCredentialLease::new(dispatch_lease)),
        )
        .await
        .expect("start leased model sync");
        assert!(!started.job_id.is_empty());
        let request = request_receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("model-list provider entered");
        assert!(request.starts_with("GET /v1/models HTTP/1.1\r\n"));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer synthetic-leased-model-sync-secret\r\n")
        );

        let (mutation_entered_sender, mutation_entered_receiver) = tokio::sync::oneshot::channel();
        let (mutation_acquired_sender, mut mutation_acquired_receiver) =
            tokio::sync::oneshot::channel();
        let mutation_state = Arc::clone(&state);
        let mutation = tokio::spawn(async move {
            mutation_entered_sender
                .send(())
                .expect("signal credential mutation entry");
            let _operation = mutation_state.lock_provider_credential_operation().await;
            let _ = mutation_acquired_sender.send(());
        });
        mutation_entered_receiver
            .await
            .expect("credential mutation entered");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut mutation_acquired_receiver)
                .await
                .is_err(),
            "credential replacement/removal must wait while model sync owns in-memory A"
        );

        provider_release
            .send(())
            .expect("finish model-list provider");
        tokio::time::timeout(Duration::from_secs(2), &mut mutation_acquired_receiver)
            .await
            .expect("credential mutation released after model listing")
            .expect("credential mutation acquired write lease");
        mutation.await.expect("credential mutation task");
        provider_thread.join().expect("model-list provider thread");
    }

    #[test]
    fn product_discovery_sync_core_work_runs_outside_the_tauri_runtime() {
        tauri::async_runtime::block_on(async {
            run_shell_discovery_off_runtime(|| {
                let nested = tokio::runtime::Runtime::new()
                    .expect("create the same private runtime shape used by Core");
                nested.block_on(async {});
                Ok::<_, shell::ShellError>(())
            })
            .await
            .expect("run discovery operation off runtime");
        });
    }

    #[test]
    fn product_continue_discovery_avoids_nested_runtime_execution() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let selecting = shell
            .begin_provider_discovery(shell::BeginProviderDiscoveryInput {
                connection_id: "off-runtime-continue".to_owned(),
                display_name: "Off-runtime continue".to_owned(),
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
            .expect("begin discovery outside Tokio");
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
        let next = tauri::async_runtime::block_on(continue_provider_discovery_off_runtime(
            &shell,
            shell::ContinueProviderDiscoveryInput {
                session_id: selecting.id,
                action_id: "00000000-0000-4000-8000-000000000099".to_owned(),
                expected_revision: selecting.revision,
                action: shell::ContinueProviderDiscoveryActionInput::SelectTemplate {
                    candidate_id: candidate.id,
                },
            },
            None,
        ))
        .expect("continue discovery without nesting Core runtime");
        assert_eq!(next.state, "awaiting_credential_origin_approval");
        let proposal = shell
            .get_provider_discovery_approval_proposal(&next.id)
            .expect("load credential-origin proposal")
            .expect("credential-origin proposal");
        let interrupted = tauri::async_runtime::block_on(continue_provider_discovery_off_runtime(
            &shell,
            shell::ContinueProviderDiscoveryInput {
                session_id: next.id,
                action_id: "00000000-0000-4000-8000-000000000100".to_owned(),
                expected_revision: next.revision,
                action: shell::ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin {
                    approval_id: proposal.id,
                },
            },
            Some(shell::SecretCredential::new(
                "synthetic-off-runtime-credential".to_owned(),
            )),
        ))
        .expect("credential-free listing interruption must not nest Core runtime");
        assert_eq!(interrupted.state, "interrupted");
    }

    #[test]
    fn product_supply_curl_evidence_crosses_the_off_runtime_boundary() {
        let root = tempdir().expect("temporary root");
        let shell = shell::ShellApi::open_data_root(root.path()).expect("open Shell");
        let reader = FakeExistingConnectionCredentialReader::new(None);
        let awaiting = tauri::async_runtime::block_on(begin_provider_discovery_curl_with_reader(
            &shell,
            shell::BeginProviderDiscoveryCurlInput {
                connection_id: "off-runtime-curl-evidence".to_owned(),
                display_name: "Off-runtime cURL evidence".to_owned(),
                docs_url: None,
                credential_binding_requested: false,
                preferred_assistant: None,
                connection_options: shell::ProviderDiscoveryConnectionOptionsInput {
                    values: Vec::new(),
                    api_base_path: Some("/v1".to_owned()),
                    timeout_seconds: 30,
                    network_mode: shell::ProviderNetworkModeInput::Public,
                    local_network_approval: None,
                },
                supplied_evidence_ids: Vec::new(),
            },
            shell::SecretProviderCurl::new("curl https://api.example.com/v1/models"),
            &reader,
        ))
        .expect("begin unknown cURL discovery outside Tokio");
        assert_eq!(awaiting.state, "awaiting_more_evidence");
        let progressed =
            tauri::async_runtime::block_on(supply_provider_discovery_curl_evidence_off_runtime(
                &shell,
                awaiting.id,
                awaiting.revision,
                shell::SecretProviderCurl::new(
                    "curl https://api.example.com/v1/chat/completions \
                     -H 'content-type: application/json' \
                     --data '{\"model\":\"synthetic\",\"messages\":[]}'",
                ),
            ))
            .expect("supplemental cURL executes deterministically off runtime");
        assert!(progressed.revision > awaiting.revision);
        assert!(
            !shell
                .list_provider_discovery_evidence(&progressed.id)
                .expect("list supplemental evidence")
                .is_empty()
        );
    }
