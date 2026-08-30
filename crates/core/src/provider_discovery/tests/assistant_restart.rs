struct ConstrainedAssistantCaptureProvider {
    plain_generate_called: Arc<AtomicBool>,
    captured_bodies: Arc<Mutex<Vec<(ApiFamily, Value)>>>,
    response: String,
}
#[async_trait::async_trait]
impl Provider for ConstrainedAssistantCaptureProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        _sink: lorepia_providers::ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        self.plain_generate_called.store(true, Ordering::SeqCst);
        Err(CoreError::internal(
            "bare setup-assistant generation must never be called",
        ))
    }

    async fn generate_with_internal_plan(
        &self,
        request: GenerationRequest,
        _credential: Option<&str>,
        sink: lorepia_providers::ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
        request_plan: lorepia_providers::parameter_mapping::ProviderRequestPlan,
    ) -> CoreResult<GenerationUsage> {
        let mut body = json!({"model": request.model});
        request_plan
            .apply_to_body(&mut body)
            .map_err(|error| CoreError::invalid(error.to_string()))?;
        self.captured_bodies
            .lock()
            .expect("capture setup-assistant body")
            .push((request_plan.family(), body));
        sink.send(ProviderEvent::TextDelta(self.response.clone()))
            .await
            .map_err(|_| CoreError::internal("setup-assistant event receiver closed"))?;
        Ok(GenerationUsage {
            input_tokens: Some(8),
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: Some(8),
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
        })
    }
}

struct PlainOnlyAssistantProvider {
    plain_generate_called: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Provider for PlainOnlyAssistantProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        _sink: lorepia_providers::ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        self.plain_generate_called.store(true, Ordering::SeqCst);
        Ok(GenerationUsage {
            input_tokens: None,
            cached_read_tokens: None,
            cached_write_tokens: None,
            output_tokens: None,
            reasoning_tokens: None,
            tool_tokens: None,
            provider_raw_summary: None,
        })
    }
}

fn assert_file_tree_omits(root: &std::path::Path, forbidden: &[u8]) {
    for entry in std::fs::read_dir(root).expect("read test data root") {
        let entry = entry.expect("read test data entry");
        let path = entry.path();
        if path.is_dir() {
            assert_file_tree_omits(&path, forbidden);
        } else {
            let bytes = std::fs::read(&path).expect("read test data file");
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden),
                "forbidden provider output persisted in {}",
                path.display()
            );
        }
    }
}

fn assistant_manifest_and_claims() -> (ProviderManifest, Vec<EvidenceClaim>) {
    let mut manifest = AdapterRegistry::built_in_templates()
        .unwrap()
        .into_iter()
        .find(|template| template.api_family == ApiFamily::OpenAiChatCompletions)
        .unwrap()
        .default_manifest;
    manifest.default_api_origin =
        Some(CanonicalOrigin::parse("https://api.assistant.example").unwrap());
    manifest.sources = vec![lorepia_domain::ManifestSource {
        kind: lorepia_domain::ManifestSourceKind::OfficialDocumentation,
        url: HttpUrl::parse("https://docs.assistant.example/").unwrap(),
        content_sha256: Some("a".repeat(64)),
    }];
    manifest.endpoints.models = None;
    manifest.decoders.streaming = None;
    manifest.parameters.clear();
    let fields = [
        (
            DraftField::ApiFamily,
            api_family_slug(manifest.api_family).to_owned(),
        ),
        (
            DraftField::DefaultApiOrigin,
            manifest
                .default_api_origin
                .as_ref()
                .unwrap()
                .as_str()
                .to_owned(),
        ),
        (
            DraftField::Auth,
            serde_json::to_string(&manifest.auth).unwrap(),
        ),
        (
            DraftField::GenerateEndpoint,
            endpoint_claim(
                manifest.endpoints.generate.method,
                manifest.endpoints.generate.path.as_str(),
            ),
        ),
        (
            DraftField::ResponseDecoder,
            decoder_slug(manifest.decoders.response).to_owned(),
        ),
    ];
    let claims = fields
        .into_iter()
        .map(|(field, value)| EvidenceClaim::new(field, value).unwrap())
        .collect();
    (manifest, claims)
}

fn seed_assistant_route(core: &crate::Core) {
    let template = core
        .list_provider_templates()
        .unwrap()
        .into_iter()
        .find(|template| template.api_family == ApiFamily::OpenAiChatCompletions)
        .unwrap();
    let api_origin = CanonicalOrigin::parse("https://api.openai.com").unwrap();
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from("assistant-recovery-provider"),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: "Assistant recovery provider".to_owned(),
            api_origin: api_origin.clone(),
            api_base_path: Some(EndpointPath::parse("/v1").unwrap()),
            network_mode: ProviderNetworkMode::Public,
            values: vec![lorepia_domain::ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(format!("{}/v1", api_origin.as_str())),
            }],
            approved_credential_origin: Some(api_origin),
            local_network_approval: None,
            timeout_seconds: 5,
        })
        .unwrap();
    let now = Utc::now();
    core.upsert_model_route(ModelRoute {
        id: ModelRouteId::from("assistant-route"),
        connection_id: connection.id,
        api_family: ApiFamily::OpenAiChatCompletions,
        model_id: "assistant-model".to_owned(),
        display_name: Some("Assistant model".to_owned()),
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
    .unwrap();
}

#[allow(clippy::too_many_lines)]
fn seed_ready_assistant(root: &std::path::Path) -> (crate::Core, DiscoverySessionId) {
    let core = crate::Core::open(crate::CoreConfig::new(root)).unwrap();
    seed_assistant_route(&core);
    let storage = core.storage();
    let session_id = DiscoverySessionId::from(Uuid::new_v4().to_string());
    let input = SanitizedDiscoveryInput {
        connection_id: ProviderConnectionId::from("assistant-recovery-connection"),
        display_name: "Assistant recovery".to_owned(),
        site_url: HttpUrl::parse("https://docs.assistant.example/").unwrap(),
        docs_url: None,
        credential_ref: None,
        preferred_assistant: Some(ModelRouteId::from("assistant-route")),
        connection_options: ProviderDiscoveryConnectionOptions::default(),
        supplied_evidence_ids: Vec::new(),
    };
    let initial = ProviderDiscoverySession::new(session_id.clone(), input).unwrap();
    let mut draft = DiscoveryWorkingDraft::new(DiscoverySourceIntent::Site);
    let evidence_id = EvidenceId::from("assistant-recovery-evidence");
    let begin = initial
        .apply(
            &provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                0,
                ProviderDiscoveryAction::Begin,
            )
            .unwrap(),
        )
        .unwrap();
    storage
        .begin_discovery_session(
            &initial,
            &DiscoveryTransitionWrite {
                transition: begin,
                draft: DiscoveryJsonUpdate::Replace(working_draft_value(&draft).unwrap()),
                review: DiscoveryJsonUpdate::Clear,
                new_evidence: Vec::new(),
                new_candidates: Vec::new(),
                approval: None,
                new_operation_id: Some(DiscoveryOperationId::new()),
                completed_operation: None,
                prepared_commit: None,
                provider_graph: None,
                occurred_at: Utc::now(),
            },
        )
        .unwrap();
    let orchestrator = core.provider_discovery();

    let mut snapshot = orchestrator.get(&session_id).unwrap();
    let operation = storage
        .get_current_discovery_operation(&session_id)
        .unwrap()
        .unwrap();
    storage
        .mark_discovery_operation_started(&operation.id, Utc::now())
        .unwrap();
    draft = hydrate_working_draft(&snapshot).unwrap();
    let (_, claims) = assistant_manifest_and_claims();
    draft
        .assistant_evidence_claims
        .insert(evidence_id.clone(), claims);
    orchestrator
        .persist_operation_completion(
            &snapshot,
            &operation.id,
            &mut draft,
            ProviderDiscoveryAction::KnownProviderCandidatesResolved { candidate_count: 0 },
            DurableOperationOutcome::Succeeded,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )
        .unwrap();

    snapshot = orchestrator.get(&session_id).unwrap();
    let operation = storage
        .get_current_discovery_operation(&session_id)
        .unwrap()
        .unwrap();
    storage
        .mark_discovery_operation_started(&operation.id, Utc::now())
        .unwrap();
    draft = hydrate_working_draft(&snapshot).unwrap();
    draft.evidence_ids = vec![evidence_id.clone()];
    let evidence = DiscoveryEvidenceRecord {
        id: evidence_id,
        session_id: session_id.clone(),
        kind: DiscoveryEvidenceKind::PlainTextDocument,
        source_url: HttpUrl::parse("https://docs.assistant.example/").unwrap(),
        content_sha256: "a".repeat(64),
        extracted_json: json!({"summary": "bounded official provider documentation"}),
        fetched_at: Utc::now(),
    };
    orchestrator
        .persist_operation_completion(
            &snapshot,
            &operation.id,
            &mut draft,
            ProviderDiscoveryAction::DocumentsFetched { evidence_count: 1 },
            DurableOperationOutcome::Succeeded,
            vec![evidence],
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )
        .unwrap();

    snapshot = orchestrator.get(&session_id).unwrap();
    let operation = storage
        .get_current_discovery_operation(&session_id)
        .unwrap()
        .unwrap();
    storage
        .mark_discovery_operation_started(&operation.id, Utc::now())
        .unwrap();
    draft = hydrate_working_draft(&snapshot).unwrap();
    initialize_assistant(storage, &snapshot, &mut draft).unwrap();
    orchestrator
        .persist_operation_completion(
            &snapshot,
            &operation.id,
            &mut draft,
            ProviderDiscoveryAction::EvidenceExtracted {
                resolution: DiscoveryEvidenceResolution::AssistantRecommended,
            },
            DurableOperationOutcome::Succeeded,
            Vec::new(),
            Vec::new(),
            DiscoveryJsonUpdate::Preserve,
        )
        .unwrap();

    snapshot = orchestrator.get(&session_id).unwrap();
    let proposal = orchestrator
        .approval_proposal(&session_id)
        .unwrap()
        .unwrap();
    orchestrator
        .continue_discovery(
            &session_id,
            provider_discovery_action_envelope(
                DiscoveryActionId::new(),
                snapshot.session.revision,
                ProviderDiscoveryAction::ApproveAssistant {
                    approval_id: proposal.id,
                    approval_grant_sha256: proposal.grant_sha256,
                },
            )
            .unwrap(),
            None,
        )
        .unwrap();
    (core, session_id)
}

fn unresolved_question(id: impl Into<String>) -> UnresolvedQuestion {
    UnresolvedQuestion {
        id: id.into(),
        field: None,
        question: "Which current provider contract detail is still unresolved?".to_owned(),
        required_evidence: "One bounded official provider document excerpt.".to_owned(),
    }
}

fn seed_pending_unresolved_questions_tool(
    root: &std::path::Path,
    questions: Vec<UnresolvedQuestion>,
) -> (crate::Core, DiscoverySessionId) {
    let (core, session_id) = seed_ready_assistant(root);
    let orchestrator = core.provider_discovery();
    let snapshot = orchestrator.get(&session_id).unwrap();
    let mut draft = hydrate_working_draft(&snapshot).unwrap();
    let mut engine = restored_assistant(&draft).unwrap();
    let estimate = AssistantCallEstimate {
        input_tokens: 16,
        maximum_output_tokens: 64,
        maximum_cost_micro_units: 100,
    };
    engine.begin_turn(estimate).unwrap();
    assert!(matches!(
        engine
            .submit_turn(AssistantTurn::NeedMoreEvidence {
                questions: questions.clone(),
            })
            .unwrap(),
        AssistantHostAction::RequestMoreEvidence { .. }
    ));
    engine.continue_after_more_evidence().unwrap();
    engine.begin_turn(estimate).unwrap();
    assert!(matches!(
        engine
            .submit_turn(AssistantTurn::CallTool {
                call: AssistantToolCall::ShowUnresolvedQuestions,
            })
            .unwrap(),
        AssistantHostAction::ExecuteTool {
            call: AssistantToolCall::ShowUnresolvedQuestions,
            ..
        }
    ));
    synchronize_assistant_snapshot(&mut draft, &engine);
    orchestrator
        .persist_assistant_checkpoint(
            &snapshot,
            &draft,
            DiscoveryAssistantCheckpoint::AwaitingToolResult,
        )
        .unwrap();
    (core, session_id)
}

#[test]
fn show_unresolved_questions_returns_exact_canonical_durable_ids() {
    let root = tempdir().unwrap();
    let questions = vec![
        unresolved_question("question-01"),
        unresolved_question("question-02"),
    ];
    let (core, session_id) = seed_pending_unresolved_questions_tool(root.path(), questions);

    let result = core
        .provider_discovery()
        .execute_assistant_tool(&session_id, &AssistantToolCall::ShowUnresolvedQuestions)
        .unwrap();

    assert_eq!(
        result,
        AssistantToolResult::UnresolvedQuestions {
            question_ids: vec!["question-01".to_owned(), "question-02".to_owned()],
        }
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn show_unresolved_questions_rejects_wrong_session_stale_or_invalid_durable_sets() {
    let root = tempdir().unwrap();
    let questions = vec![
        unresolved_question("question-01"),
        unresolved_question("question-02"),
    ];
    let (core, session_id) = seed_pending_unresolved_questions_tool(root.path(), questions);
    let orchestrator = core.provider_discovery();
    let current = orchestrator.get(&session_id).unwrap();
    let draft = hydrate_working_draft(&current).unwrap();
    let assert_rejected = |requested_session_id: &DiscoverySessionId,
                           observed_revision: u64,
                           candidate: &DiscoveryWorkingDraft| {
        let error = ProviderDiscoveryOrchestrator::validated_assistant_unresolved_question_ids(
            requested_session_id,
            observed_revision,
            &current,
            candidate,
        )
        .unwrap_err();
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    };

    assert_rejected(
        &DiscoverySessionId::from("another-session"),
        current.session.revision,
        &draft,
    );
    assert_rejected(
        &session_id,
        current.session.revision.saturating_sub(1),
        &draft,
    );

    let mut empty = draft.clone();
    empty.assistant_more_evidence_questions.clear();
    assert_rejected(&session_id, current.session.revision, &empty);

    let mut too_many = draft.clone();
    too_many.assistant_more_evidence_questions = (0..129)
        .map(|index| unresolved_question(format!("question-{index:03}")))
        .collect();
    assert_rejected(&session_id, current.session.revision, &too_many);

    let mut oversized_text = draft.clone();
    oversized_text.assistant_more_evidence_questions[0].question = "x".repeat(2 * 1024 + 1);
    assert_rejected(&session_id, current.session.revision, &oversized_text);

    let mut oversized_result = draft.clone();
    oversized_result.assistant_more_evidence_questions = (0..40)
        .map(|index| unresolved_question(format!("q-{index:03}-{}", "x".repeat(118))))
        .collect();
    assert_rejected(&session_id, current.session.revision, &oversized_result);

    let mut malformed = draft.clone();
    malformed.assistant_more_evidence_questions[0].id = "question with spaces".to_owned();
    assert_rejected(&session_id, current.session.revision, &malformed);

    let mut duplicate = draft.clone();
    duplicate.assistant_more_evidence_questions[1].id = "question-01".to_owned();
    assert_rejected(&session_id, current.session.revision, &duplicate);

    let mut out_of_order = draft;
    out_of_order.assistant_more_evidence_questions.swap(0, 1);
    assert_rejected(&session_id, current.session.revision, &out_of_order);
}

#[test]
#[allow(clippy::too_many_lines)]
fn selected_assistant_route_uses_exact_family_plan_and_decodes_only_the_envelope() {
    let root = tempdir().unwrap();
    let (core, session_id) = seed_ready_assistant(root.path());
    let estimate = AssistantCallEstimate {
        input_tokens: 16,
        maximum_output_tokens: 2_048,
        maximum_cost_micro_units: 100,
    };
    let mut prompt = core
        .provider_discovery()
        .begin_assistant_turn(&session_id, estimate)
        .unwrap();
    prompt.allowed_api_families = vec![ApiFamily::OpenAiChatCompletions];
    let expected_turn = AssistantTurn::NeedMoreEvidence {
        questions: vec![unresolved_question("need-current-contract")],
    };
    let response = serde_json::to_string(&json!({"turn": &expected_turn})).unwrap();
    let (mut outside_manifest, _) = assistant_manifest_and_claims();
    outside_manifest.api_family = ApiFamily::AnthropicMessages;
    let outside_allowlist_turn = AssistantTurn::SubmitDraft {
        draft: Box::new(AssistantManifestDraft {
            manifest: outside_manifest,
            evidence_mappings: Vec::new(),
            conflicts: Vec::new(),
            unresolved_questions: Vec::new(),
            confidence: Vec::new(),
            summary: "This family is intentionally outside the prompt allowlist.".to_owned(),
        }),
    };
    let outside_allowlist_response =
        serde_json::to_string(&json!({"turn": outside_allowlist_turn})).unwrap();
    let expected_family_enum = prompt
        .allowed_api_families
        .iter()
        .map(|family| api_family_slug(*family))
        .collect::<Vec<_>>();
    let mut route = core
        .storage()
        .get_model_route(&ModelRouteId::from("assistant-route"))
        .unwrap();

    for family in [
        ApiFamily::OpenAiResponses,
        ApiFamily::OpenAiChatCompletions,
        ApiFamily::AnthropicMessages,
        ApiFamily::GeminiGenerateContent,
        ApiFamily::OllamaNative,
    ] {
        route.api_family = family;
        let plain_generate_called = Arc::new(AtomicBool::new(false));
        let captured_bodies = Arc::new(Mutex::new(Vec::new()));
        let provider = Arc::new(ConstrainedAssistantCaptureProvider {
            plain_generate_called: Arc::clone(&plain_generate_called),
            captured_bodies: Arc::clone(&captured_bodies),
            response: response.clone(),
        });
        let output = core
            .runtime_handle()
            .block_on(run_setup_assistant_provider_call(
                provider,
                &route,
                &prompt,
                estimate,
                Some("borrowed-only-credential"),
            ))
            .unwrap();
        assert_eq!(output, expected_turn);
        assert!(!plain_generate_called.load(Ordering::SeqCst));

        let rejected_provider = Arc::new(ConstrainedAssistantCaptureProvider {
            plain_generate_called: Arc::clone(&plain_generate_called),
            captured_bodies: Arc::clone(&captured_bodies),
            response: outside_allowlist_response.clone(),
        });
        let error = core
            .runtime_handle()
            .block_on(run_setup_assistant_provider_call(
                rejected_provider,
                &route,
                &prompt,
                estimate,
                None,
            ))
            .expect_err("target family outside the prompt allowlist must be rejected");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert!(error.recoverable);
        assert!(!plain_generate_called.load(Ordering::SeqCst));

        let captured = captured_bodies
            .lock()
            .expect("read setup-assistant capture");
        assert_eq!(captured.len(), 2);
        assert_eq!(captured[0].0, family);
        assert_eq!(captured[1], captured[0]);
        let body = &captured[0].1;
        let schema = match family {
            ApiFamily::OpenAiResponses => {
                let format = &body["text"]["format"];
                assert_eq!(format["type"], "json_schema");
                assert_eq!(format["name"], "lorepia_setup_assistant_turn_v1");
                assert_eq!(format["strict"], true);
                &format["schema"]
            }
            ApiFamily::OpenAiChatCompletions => {
                let format = &body["response_format"];
                assert_eq!(format["type"], "json_schema");
                assert_eq!(
                    format["json_schema"]["name"],
                    "lorepia_setup_assistant_turn_v1"
                );
                assert_eq!(format["json_schema"]["strict"], true);
                &format["json_schema"]["schema"]
            }
            ApiFamily::AnthropicMessages => {
                let format = &body["output_config"]["format"];
                assert_eq!(format["type"], "json_schema");
                &format["schema"]
            }
            ApiFamily::GeminiGenerateContent => {
                assert_eq!(
                    body["generationConfig"]["responseMimeType"],
                    "application/json"
                );
                &body["generationConfig"]["responseJsonSchema"]
            }
            ApiFamily::OllamaNative => &body["format"],
        };
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["$defs"]["api_family"]["enum"],
            json!(expected_family_enum)
        );
        assert!(
            !serde_json::to_string(body)
                .unwrap()
                .contains("borrowed-only-credential")
        );
    }
}

#[test]
fn provider_without_internal_plan_support_fails_without_bare_generation_fallback() {
    let root = tempdir().unwrap();
    let (core, session_id) = seed_ready_assistant(root.path());
    let estimate = AssistantCallEstimate {
        input_tokens: 16,
        maximum_output_tokens: 64,
        maximum_cost_micro_units: 100,
    };
    let prompt = core
        .provider_discovery()
        .begin_assistant_turn(&session_id, estimate)
        .unwrap();
    let route = core
        .storage()
        .get_model_route(&ModelRouteId::from("assistant-route"))
        .unwrap();
    let plain_generate_called = Arc::new(AtomicBool::new(false));
    let error = core
        .runtime_handle()
        .block_on(run_setup_assistant_provider_call(
            Arc::new(PlainOnlyAssistantProvider {
                plain_generate_called: Arc::clone(&plain_generate_called),
            }),
            &route,
            &prompt,
            estimate,
            None,
        ))
        .unwrap_err();

    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    assert!(!plain_generate_called.load(Ordering::SeqCst));
}

#[test]
fn nested_schema_escape_secret_is_rejected_without_error_or_storage_persistence() {
    const SECRET_CANARY: &str = "sk-schema-escape-canary-abcdefghijklmnopqrstuvwxyz";

    let root = tempdir().unwrap();
    let (core, session_id) = seed_ready_assistant(root.path());
    let route = core
        .storage()
        .get_model_route(&ModelRouteId::from("assistant-route"))
        .unwrap();
    let estimate = AssistantCallEstimate {
        input_tokens: 16,
        maximum_output_tokens: 256,
        maximum_cost_micro_units: 100,
    };
    let response = json!({
        "turn": {
            "type": "need_more_evidence",
            "questions": [{
                "id": "need-current-contract",
                "field": {
                    "kind": "parameter",
                    "parameter_id": "temperature",
                    "credential": SECRET_CANARY
                },
                "question": "Which parameter contract is current?",
                "required_evidence": "A current official parameter table."
            }]
        }
    })
    .to_string();
    let plain_generate_called = Arc::new(AtomicBool::new(false));
    let captured_bodies = Arc::new(Mutex::new(Vec::new()));
    let error = core
        .provider_discovery()
        .run_assistant_with_provider(
            &session_id,
            &route,
            Arc::new(ConstrainedAssistantCaptureProvider {
                plain_generate_called: Arc::clone(&plain_generate_called),
                captured_bodies: Arc::clone(&captured_bodies),
                response,
            }),
            estimate,
            None,
        )
        .expect_err("nested schema escape must fail before assistant state submission");

    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert!(!format!("{error:?}").contains(SECRET_CANARY));
    assert!(!plain_generate_called.load(Ordering::SeqCst));
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&session_id)
            .unwrap()
            .unwrap()
            .action,
        ProviderDiscoveryAssistantResumeAction::ApproveRetry
    );
    let snapshot = core.get_provider_discovery(&session_id).unwrap();
    assert!(!format!("{snapshot:?}").contains(SECRET_CANARY));
    assert!(
        captured_bodies
            .lock()
            .unwrap()
            .iter()
            .all(|(_, body)| !body.to_string().contains(SECRET_CANARY))
    );

    drop(core);
    assert_file_tree_omits(root.path(), SECRET_CANARY.as_bytes());
}

#[test]
#[allow(clippy::too_many_lines)]
fn assistant_restart_boundaries_preserve_only_durably_safe_checkpoints() {
    let ready_root = tempdir().unwrap();
    let (ready_core, ready_id) = seed_ready_assistant(ready_root.path());
    drop(ready_core);
    let ready_core =
        open_core_after_drop(ready_root.path(), crate::DiscoveryRecoveryOwner::Core);
    assert_eq!(
        ready_core
            .get_provider_discovery_assistant_resume_boundary(&ready_id)
            .unwrap()
            .unwrap()
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );

    let pending_root = tempdir().unwrap();
    let (pending_core, pending_id) = seed_ready_assistant(pending_root.path());
    pending_core
        .provider_discovery()
        .begin_assistant_turn(
            &pending_id,
            AssistantCallEstimate {
                input_tokens: 16,
                maximum_output_tokens: 64,
                maximum_cost_micro_units: 100,
            },
        )
        .unwrap();
    drop(pending_core);
    let pending_core =
        open_core_after_drop(pending_root.path(), crate::DiscoveryRecoveryOwner::Core);
    let pending = pending_core.get_provider_discovery(&pending_id).unwrap();
    assert_eq!(pending.session.state, DiscoveryState::UnknownOutcome);
    assert_eq!(
        pending_core
            .get_provider_discovery_assistant_resume_boundary(&pending_id)
            .unwrap()
            .unwrap()
            .action,
        ProviderDiscoveryAssistantResumeAction::ResolveUnknownOutcome
    );

    let tool_root = tempdir().unwrap();
    let (tool_core, tool_id) = seed_ready_assistant(tool_root.path());
    {
        let orchestrator = tool_core.provider_discovery();
        orchestrator
            .begin_assistant_turn(
                &tool_id,
                AssistantCallEstimate {
                    input_tokens: 16,
                    maximum_output_tokens: 64,
                    maximum_cost_micro_units: 100,
                },
            )
            .unwrap();
        let tool_turn = serde_json::to_vec(&AssistantTurn::CallTool {
            call: AssistantToolCall::ListManifestAdapterFamilies,
        })
        .unwrap();
        assert!(matches!(
            orchestrator
                .submit_assistant_turn_json(&tool_id, &tool_turn)
                .unwrap(),
            AssistantHostAction::ExecuteTool { .. }
        ));
    }
    drop(tool_core);
    let tool_core = open_core_after_drop(tool_root.path(), crate::DiscoveryRecoveryOwner::Core);
    assert_eq!(
        tool_core
            .get_provider_discovery_assistant_resume_boundary(&tool_id)
            .unwrap()
            .unwrap()
            .action,
        ProviderDiscoveryAssistantResumeAction::ResumeCoreHostAction
    );
    tool_core
        .resume_provider_discovery_assistant_core_host_action(&tool_id)
        .unwrap();
    assert_eq!(
        tool_core
            .get_provider_discovery_assistant_resume_boundary(&tool_id)
            .unwrap()
            .unwrap()
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );

    let draft_root = tempdir().unwrap();
    let (draft_core, draft_id) = seed_ready_assistant(draft_root.path());
    {
        let orchestrator = draft_core.provider_discovery();
        orchestrator
            .begin_assistant_turn(
                &draft_id,
                AssistantCallEstimate {
                    input_tokens: 16,
                    maximum_output_tokens: 256,
                    maximum_cost_micro_units: 100,
                },
            )
            .unwrap();
        let (manifest, claims) = assistant_manifest_and_claims();
        let evidence_id = EvidenceId::from("assistant-recovery-evidence");
        let mappings = claims
            .iter()
            .map(|claim| FieldEvidenceMapping {
                field: claim.field().clone(),
                evidence_ids: vec![evidence_id.clone()],
                explanation: "The deterministic evidence supports this exact value.".to_owned(),
            })
            .collect::<Vec<_>>();
        let confidence = claims
            .iter()
            .map(|claim| FieldConfidence {
                field: claim.field().clone(),
                level: ConfidenceLevel::High,
                rationale: "Deterministic structural evidence.".to_owned(),
            })
            .collect();
        let turn = AssistantTurn::SubmitDraft {
            draft: Box::new(AssistantManifestDraft {
                manifest,
                evidence_mappings: mappings,
                conflicts: Vec::new(),
                unresolved_questions: Vec::new(),
                confidence,
                summary: "A deterministic evidence-backed provider draft.".to_owned(),
            }),
        };
        assert!(matches!(
            orchestrator
                .submit_assistant_turn_json(&draft_id, &serde_json::to_vec(&turn).unwrap())
                .unwrap(),
            AssistantHostAction::ReviewDraft(_)
        ));
    }
    drop(draft_core);
    let draft_core =
        open_core_after_drop(draft_root.path(), crate::DiscoveryRecoveryOwner::Core);
    let boundary = draft_core
        .get_provider_discovery_assistant_resume_boundary(&draft_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        boundary.action,
        ProviderDiscoveryAssistantResumeAction::ReviewDraft
    );
    assert!(boundary.draft_review.is_some());
}

struct CredentialReflectingErrorProvider {
    credential: String,
}

#[async_trait::async_trait]
impl Provider for CredentialReflectingErrorProvider {
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            streaming: true,
            reasoning: false,
            max_context_tokens: None,
        }
    }

    async fn generate(
        &self,
        _request: GenerationRequest,
        _credential: Option<&str>,
        _sink: lorepia_providers::ProviderEventSender,
        _cancelled: watch::Receiver<bool>,
    ) -> CoreResult<GenerationUsage> {
        Err(CoreError {
            code: CoreErrorCode::ProviderAuthFailed,
            message: format!("provider reflected {}", self.credential),
            recoverable: false,
            operation_id: format!("operation-{}", self.credential),
        })
    }

    async fn generate_with_internal_plan(
        &self,
        request: GenerationRequest,
        credential: Option<&str>,
        sink: lorepia_providers::ProviderEventSender,
        cancelled: watch::Receiver<bool>,
        _request_plan: lorepia_providers::parameter_mapping::ProviderRequestPlan,
    ) -> CoreResult<GenerationUsage> {
        self.generate(request, credential, sink, cancelled).await
    }
}

#[test]
fn assistant_provider_error_reflection_is_replaced_before_return() {
    let root = tempdir().unwrap();
    let (core, session_id) = seed_ready_assistant(root.path());
    let prompt = core
        .provider_discovery()
        .begin_assistant_turn(
            &session_id,
            AssistantCallEstimate {
                input_tokens: 16,
                maximum_output_tokens: 64,
                maximum_cost_micro_units: 100,
            },
        )
        .unwrap();
    let route = core
        .storage()
        .get_model_route(&ModelRouteId::from("assistant-route"))
        .unwrap();
    let credential = "assistant-error-reflection-canary";
    let error = core
        .runtime_handle()
        .block_on(run_setup_assistant_provider_call(
            Arc::new(CredentialReflectingErrorProvider {
                credential: credential.to_owned(),
            }),
            &route,
            &prompt,
            AssistantCallEstimate {
                input_tokens: 16,
                maximum_output_tokens: 64,
                maximum_cost_micro_units: 100,
            },
            Some(credential),
        ))
        .unwrap_err();

    assert_eq!(error.code, CoreErrorCode::ProviderUnavailable);
    assert_eq!(
        error.message,
        "setup assistant provider error reflected credential material"
    );
    assert!(!format!("{error:?}").contains(credential));
}
