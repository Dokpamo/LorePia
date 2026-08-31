    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::Duration,
    };

    use lorepia_core::{
        Core, CoreConfig, ProviderProfile, VariableId, VariableMap, VariableRef, VariableScope,
        VariableValue,
    };
    use tempfile::{NamedTempFile, tempdir};

    use crate::{
        ChatStreamItem, ConversationGreetingSelectionInput, CreateConversationInput,
        EditUserMessageInput, GenerationCredential, GenerationSelectionInput, GenerationTargetDto,
        RegenerateAssistantMessageInput, RemoveMessageInput, SHELL_API_VERSION, SecretCredential,
        SendMessageInput, ShellApi, ShellErrorCode, StagedImportFile, dto::ConversationModeDto,
    };

    const LIVE_CREDENTIAL_CANARY: &str = "sk-shell-live-canary";

    struct MissingTaskCredentialReader;

    struct AdmissionDropCounter(Arc<AtomicUsize>);

    impl Drop for AdmissionDropCounter {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl crate::TaskCredentialReader for MissingTaskCredentialReader {
        fn credential_for<'a>(
            &'a self,
            _connection_id: &'a str,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = crate::TaskCredentialRead> + Send + 'a>,
        > {
            Box::pin(async { crate::TaskCredentialRead::Missing })
        }
    }

    #[test]
    fn operation_nonce_is_serde_compatible_but_strictly_bounded_before_use() {
        let legacy = serde_json::from_value::<SendMessageInput>(serde_json::json!({
            "conversation_id": "conversation-1",
            "branch_id": "branch-1",
            "expected_head": null,
            "mode": "chat",
            "text": "hello",
            "selection": {
                "kind": "legacy_profile",
                "provider_profile_id": "profile-1"
            }
        }))
        .expect("older send payloads must still decode");
        assert_eq!(legacy.operation_nonce, None);
        assert_eq!(legacy.generation_attempt_id, None);
        assert_eq!(legacy.variable_overrides, VariableMap::default());

        let valid_ascii = "n".repeat(64);
        let valid_unicode = "가".repeat(42);
        assert!(super::validate_required_operation_nonce(Some(&valid_ascii)).is_ok());
        assert!(super::validate_required_operation_nonce(Some(&valid_unicode)).is_ok());
        assert!(super::validate_required_operation_nonce(None).is_err());
        assert!(super::validate_required_operation_nonce(Some("   ")).is_err());
        assert!(super::validate_required_operation_nonce(Some(" padded")).is_err());
        assert!(super::validate_required_operation_nonce(Some("line\nbreak")).is_err());
        assert!(super::validate_required_operation_nonce(Some(&"n".repeat(65))).is_err());
        assert!(super::validate_required_operation_nonce(Some(&"가".repeat(43))).is_err());
        assert!(
            super::validate_generation_operation_context(
                Some("caller-owned-nonce"),
                Some("generation-attempt-1")
            )
            .is_err()
        );
        assert!(super::validate_generation_operation_context(None, None).is_err());
        assert!(
            super::validate_generation_operation_context(None, Some("generation-attempt-1"))
                .is_ok()
        );

        let resumed = serde_json::to_value(SendMessageInput {
            generation_attempt_id: Some("generation-attempt-1".to_owned()),
            ..legacy.clone()
        })
        .expect("resume-bearing send input must serialize");
        assert_eq!(resumed["generation_attempt_id"], "generation-attempt-1");

        let encoded = serde_json::to_value(SendMessageInput {
            operation_nonce: Some("caller-owned-nonce".to_owned()),
            ..legacy
        })
        .expect("nonce-bearing send input must serialize");
        assert_eq!(encoded["operation_nonce"], "caller-owned-nonce");
    }

    fn imported_shell() -> (tempfile::TempDir, ShellApi, String) {
        let root = tempdir().expect("temporary data root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open shell");
        let mut source = NamedTempFile::new().expect("temporary source");
        write!(
            source,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Shell","description":"Synthetic"}}}}"#
        )
        .expect("write source");
        let inspection = shell
            .inspect_import(&StagedImportFile::new(source.path()))
            .expect("inspect");
        let character = shell
            .commit_import(&inspection.inspection_id)
            .expect("commit");
        (root, shell, character.id)
    }

    #[test]
    fn bootstrap_and_library_projection_never_expose_data_root() {
        let (root, shell, character_id) = imported_shell();
        let bootstrap = shell.bootstrap().expect("bootstrap");
        assert_eq!(bootstrap.shell_api_version, SHELL_API_VERSION);
        assert_eq!(bootstrap.core_api_version, lorepia_core::CORE_API_VERSION);
        let characters = shell.list_characters().expect("characters");
        let json = serde_json::to_string(&(bootstrap, characters)).expect("serialize");

        assert!(!json.contains(root.path().to_string_lossy().as_ref()));
        assert!(json.contains(&character_id));
    }

    #[test]
    fn conversation_collections_remain_whole_vec_mappings() {
        let (_root, shell, character_id) = imported_shell();
        let first = shell
            .create_conversation(CreateConversationInput {
                character_id: character_id.clone(),
                title: "First".into(),
                mode: ConversationModeDto::Chat,
                greeting: None,
            })
            .expect("first conversation");
        shell
            .create_conversation(CreateConversationInput {
                character_id: character_id.clone(),
                title: "Second".into(),
                mode: ConversationModeDto::Story,
                greeting: None,
            })
            .expect("second conversation");

        let all = shell.list_conversations().expect("all conversations");
        let filtered = shell
            .list_conversations_for_character(&character_id)
            .expect("filtered conversations");
        assert_eq!(all.len(), 2);
        assert_eq!(filtered.len(), 2);
        assert!(shell.list_messages(&first.id).expect("messages").is_empty());
    }

    #[test]
    fn greeting_catalog_is_text_free_and_exact_alternate_selection_starts_the_room() {
        let root = tempdir().expect("temporary data root");
        let shell = ShellApi::open(CoreConfig::new(root.path())).expect("open shell");
        let default_canary = "SHELL PRIVATE DEFAULT GREETING";
        let alternate_canary = "SHELL PRIVATE ALTERNATE GREETING";
        let mut source = NamedTempFile::new().expect("temporary source");
        write!(
            source,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Greeting Shell","description":"Synthetic","first_mes":"{default_canary}","alternate_greetings":["{alternate_canary}"]}}}}"#
        )
        .expect("write source");
        let inspection = shell
            .inspect_import(&StagedImportFile::new(source.path()))
            .expect("inspect");
        let character = shell
            .commit_import(&inspection.inspection_id)
            .expect("commit");

        let catalog = shell
            .get_character_greeting_catalog(&character.id)
            .expect("safe greeting catalog");
        let revision_id = catalog
            .character_content_revision_id
            .clone()
            .expect("content revision");
        let json = serde_json::to_string(&catalog).expect("serialize catalog");
        assert!(!json.contains(default_canary));
        assert!(!json.contains(alternate_canary));
        assert_eq!(
            catalog
                .greetings
                .iter()
                .map(|greeting| greeting.id.as_str())
                .collect::<Vec<_>>(),
            ["default", "alternate-0"]
        );

        let conversation = shell
            .create_conversation(CreateConversationInput {
                character_id: character.id,
                title: "Alternate greeting".into(),
                mode: ConversationModeDto::Chat,
                greeting: Some(ConversationGreetingSelectionInput {
                    character_content_revision_id: Some(revision_id),
                    greeting_id: Some("alternate-0".into()),
                }),
            })
            .expect("exact alternate conversation start");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("conversation state");
        let messages = shell
            .list_branch_messages(&state.active_branch_id)
            .expect("greeting lineage");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, alternate_canary);
    }

    #[test]
    fn remove_message_passes_expected_head_instead_of_inventing_a_revision() {
        let (_root, shell, character_id) = imported_shell();
        let conversation = shell
            .open_conversation(&character_id)
            .expect("conversation");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("state");

        let error = shell
            .remove_message_from_branch(RemoveMessageInput {
                conversation_id: conversation.id,
                branch_id: state.active_branch_id,
                expected_head: Some("stale-head".into()),
                message_id: "missing-message".into(),
            })
            .expect_err("stale head must fail");

        assert_eq!(error.code, ShellErrorCode::InvalidInput);
    }

    #[test]
    fn target_selection_rejects_unbound_legacy_credential_context() {
        let (_root, shell, character_id) = imported_shell();
        let conversation = shell
            .open_conversation(&character_id)
            .expect("conversation");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("conversation state");

        let error = shell
            .send_message_to_branch(
                SendMessageInput {
                    conversation_id: conversation.id,
                    branch_id: state.active_branch_id.clone(),
                    expected_head: None,
                    mode: ConversationModeDto::Chat,
                    text: "must not be stored".to_owned(),
                    selection: GenerationSelectionInput::Target {
                        target: GenerationTargetDto {
                            model_route_id: "synthetic-route".to_owned(),
                            generation_preset_id: "synthetic-preset".to_owned(),
                        },
                    },
                    variable_overrides: VariableMap::default(),
                    operation_nonce: Some("shell-target-unbound-credential".to_owned()),
                    generation_attempt_id: None,
                },
                GenerationCredential::legacy(Some(SecretCredential::new(
                    "synthetic-unbound-credential",
                ))),
            )
            .expect_err("target selection must reject an unbound credential");

        assert_eq!(error.code, ShellErrorCode::InvalidInput);
        assert!(
            shell
                .list_branch_messages(&state.active_branch_id)
                .expect("messages after rejected credential context")
                .is_empty()
        );
    }

    #[tokio::test]
    #[allow(
        clippy::too_many_lines,
        reason = "one vertical-slice test keeps the complete immutable-branch action sequence visible"
    )]
    async fn synthetic_async_core_stream_exercises_send_edit_regenerate_and_remove() {
        let root = tempdir().expect("temporary data root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open core");
        let (base_url, provider_thread) = spawn_completed_provider(3);
        core.upsert_provider_profile(ProviderProfile {
            id: "synthetic-profile".into(),
            display_name: "Synthetic".into(),
            base_url,
            model: "synthetic-model".into(),
            timeout_seconds: 5,
        })
        .expect("save provider");
        let shell = ShellApi::from_core(core);
        let mut source = NamedTempFile::new().expect("temporary source");
        write!(
            source,
            r#"{{"spec":"chara_card_v3","data":{{"name":"Chat","description":"Synthetic"}}}}"#
        )
        .expect("write source");
        let inspection = shell
            .inspect_import(&StagedImportFile::new(source.path()))
            .expect("inspect");
        let character = shell
            .commit_import(&inspection.inspection_id)
            .expect("commit");
        let conversation = shell
            .open_conversation(&character.id)
            .expect("conversation");
        let state = shell
            .get_conversation_state(&conversation.id)
            .expect("state");
        let (_task_cancel, task_cancelled) = tokio::sync::watch::channel(false);
        let admission_drops = Arc::new(AtomicUsize::new(0));
        let mut runtime_variables = VariableMap::default();
        runtime_variables.insert(
            VariableRef {
                scope: VariableScope::Character,
                namespace: None,
                id: VariableId::from("background_music"),
            },
            VariableValue::Text("1".to_owned()),
        );

        let started = shell
            .send_message_to_branch_async(
                SendMessageInput {
                    conversation_id: conversation.id.clone(),
                    branch_id: state.active_branch_id.clone(),
                    expected_head: None,
                    mode: ConversationModeDto::Chat,
                    text: "first".into(),
                    selection: synthetic_profile_selection(),
                    variable_overrides: runtime_variables,
                    operation_nonce: Some("shell-send-first".to_owned()),
                    generation_attempt_id: None,
                },
                GenerationCredential::legacy_with_admission_lease(
                    Some(SecretCredential::new(LIVE_CREDENTIAL_CANARY)),
                    AdmissionDropCounter(Arc::clone(&admission_drops)),
                ),
                &MissingTaskCredentialReader,
                task_cancelled.clone(),
            )
            .await
            .expect("send");
        assert_eq!(admission_drops.load(Ordering::SeqCst), 1);
        let (started_response, stream) = started.into_parts();
        assert_stream_finishes(stream, &started_response.generation_id).await;

        let messages = shell
            .list_branch_messages(&state.active_branch_id)
            .expect("first messages");
        assert_eq!(messages.len(), 2);
        let first_user = &messages[0];
        let first_assistant = &messages[1];

        let edit = shell
            .edit_user_message_async(
                EditUserMessageInput {
                    conversation_id: conversation.id.clone(),
                    branch_id: state.active_branch_id,
                    expected_head: Some(first_assistant.id.clone()),
                    message_id: first_user.id.clone(),
                    replacement_text: "edited".into(),
                    selection: synthetic_profile_selection(),
                    operation_nonce: Some("shell-edit-first".to_owned()),
                    generation_attempt_id: None,
                },
                GenerationCredential::legacy_with_admission_lease(
                    None,
                    AdmissionDropCounter(Arc::clone(&admission_drops)),
                ),
                &MissingTaskCredentialReader,
                task_cancelled.clone(),
            )
            .await
            .expect("edit");
        assert_eq!(admission_drops.load(Ordering::SeqCst), 2);
        let (edit_response, stream) = edit.into_parts();
        assert_stream_finishes(stream, &edit_response.generation_id).await;

        let edited_messages = shell
            .list_branch_messages(&edit_response.branch.id)
            .expect("edited messages");
        assert_eq!(edited_messages.len(), 2);
        assert_eq!(edited_messages[0].content, "edited");
        let edited_assistant = &edited_messages[1];

        let regenerate = shell
            .regenerate_assistant_message_async(
                RegenerateAssistantMessageInput {
                    conversation_id: conversation.id.clone(),
                    branch_id: edit_response.branch.id,
                    expected_head: Some(edited_assistant.id.clone()),
                    message_id: edited_assistant.id.clone(),
                    selection: synthetic_profile_selection(),
                    operation_nonce: Some("shell-regenerate-first".to_owned()),
                    generation_attempt_id: None,
                },
                GenerationCredential::legacy_with_admission_lease(
                    None,
                    AdmissionDropCounter(Arc::clone(&admission_drops)),
                ),
                &MissingTaskCredentialReader,
                task_cancelled,
            )
            .await
            .expect("regenerate");
        assert_eq!(admission_drops.load(Ordering::SeqCst), 3);
        let (regenerate_response, stream) = regenerate.into_parts();
        assert_stream_finishes(stream, &regenerate_response.generation_id).await;

        let regenerated_messages = shell
            .list_branch_messages(&regenerate_response.branch.id)
            .expect("regenerated messages");
        let regenerated_assistant = regenerated_messages.last().expect("assistant");
        let shortened = shell
            .remove_message_from_branch(RemoveMessageInput {
                conversation_id: conversation.id,
                branch_id: regenerate_response.branch.id,
                expected_head: Some(regenerated_assistant.id.clone()),
                message_id: regenerated_assistant.id.clone(),
            })
            .expect("remove");
        assert_eq!(
            shortened.head_message_id.as_deref(),
            regenerated_assistant.parent_id.as_deref()
        );

        provider_thread.join().expect("provider thread");
    }

    fn synthetic_profile_selection() -> GenerationSelectionInput {
        GenerationSelectionInput::LegacyProfile {
            provider_profile_id: "synthetic-profile".into(),
        }
    }

    async fn assert_stream_finishes(
        mut stream: crate::ChatEventStream,
        expected_generation_id: &str,
    ) {
        let mut last_sequence = 0;
        loop {
            let item = tokio::time::timeout(Duration::from_secs(5), stream.recv())
                .await
                .expect("stream timeout");
            match item {
                ChatStreamItem::Event(event) => {
                    assert_eq!(event.generation_id, expected_generation_id);
                    assert!(event.sequence > last_sequence);
                    assert!(
                        !serde_json::to_string(&event)
                            .expect("serialize event")
                            .contains(LIVE_CREDENTIAL_CANARY)
                    );
                    last_sequence = event.sequence;
                    if matches!(event.kind, crate::ChatEventKindDto::GenerationFinished) {
                        break;
                    }
                }
                ChatStreamItem::ReconciliationRequired(required) => {
                    panic!("unexpected reconciliation: {required:?}");
                }
                ChatStreamItem::Closed => panic!("stream closed before terminal event"),
            }
        }
    }

    fn spawn_completed_provider(request_count: usize) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind provider");
        let address = listener.local_addr().expect("provider address");
        let handle = thread::spawn(move || {
            for _ in 0..request_count {
                let (mut stream, _) = listener.accept().expect("accept provider request");
                read_request(&mut stream);
                let body = concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"reply\"}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n",
                    "data: [DONE]\n\n"
                );
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write provider response");
            }
        });
        (format!("http://{address}/v1"), handle)
    }

    fn read_request(stream: &mut TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("request timeout");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        loop {
            let read = stream.read(&mut buffer).expect("read request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            let Some(header_end) = request.windows(4).position(|value| value == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
    }
