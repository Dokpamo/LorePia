fn new_test_generation_operation(nonce: &str) -> GenerationOperationContext<'_> {
    GenerationOperationContext::New {
        operation_nonce: nonce,
    }
}

fn open_core_after_drop(data_root: &Path) -> Core {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match Core::open(CoreConfig::new(data_root)) {
            Ok(core) => return core,
            Err(error)
                if error.code == CoreErrorCode::StorageUnavailable
                    && error.message == "data root is already owned by another LorePia process"
                    && Instant::now() < deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("open Core after prior owner drop: {error:?}"),
        }
    }
}

struct StallingProvider {
    partial: String,
    started: Mutex<Option<std_mpsc::Sender<()>>>,
}

struct CatchupSnapshotProvider {
    started: Mutex<Option<std_mpsc::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

struct CapturingProvider {
    response: String,
    captured: Mutex<Option<std_mpsc::Sender<Vec<String>>>>,
    captured_temperature: Mutex<Option<std_mpsc::Sender<Option<f64>>>>,
}

type OpaqueRequestCapture = (
    bool,
    Vec<OpaqueReasoningContext>,
    Option<GenerationProviderProvenance>,
);

struct OpaqueContinuityProvider {
    response: String,
    emitted_state: Option<OpaqueReasoningState>,
    captured_request: Mutex<Option<std_mpsc::Sender<OpaqueRequestCapture>>>,
}

struct OverflowUsageProvider;
struct SnapshotFailingProvider;
struct RejectingTaskCredentialBroker;

struct LeaseBarrierProvider {
    entered: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

impl crate::TaskCredentialBroker for RejectingTaskCredentialBroker {
    fn credential_for<'a>(
        &'a self,
        _connection_id: &'a ProviderConnectionId,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = CoreResult<ConnectionBoundCredential>> + Send + 'a>,
    > {
        Box::pin(async {
            Err(CoreError::internal(
                "credential broker was called without an embedding task",
            ))
        })
    }
}

fn read_http_headers(stream: &mut TcpStream) -> String {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set model-list read timeout");
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

fn spawn_model_list_provider(
    response_bodies: Vec<String>,
) -> (CanonicalOrigin, std_mpsc::Receiver<String>) {
    spawn_model_list_http_provider(
        response_bodies
            .into_iter()
            .map(|body| ("200 OK".to_owned(), body))
            .collect(),
    )
}

fn spawn_chat_completion_provider() -> (CanonicalOrigin, std_mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind chat provider");
    let address = listener.local_addr().expect("chat provider address");
    let (request_sender, request_receiver) = std_mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept chat request");
        let request = read_http_headers(&mut stream);
        request_sender
            .send(request)
            .expect("send captured chat request");
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"fresh authority reply\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write chat response");
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}")).expect("canonical chat origin"),
        request_receiver,
    )
}

fn spawn_blocking_chat_completion_provider() -> (
    CanonicalOrigin,
    std_mpsc::Receiver<String>,
    std_mpsc::Sender<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind blocking chat provider");
    let address = listener
        .local_addr()
        .expect("blocking chat provider address");
    let (request_sender, request_receiver) = std_mpsc::channel();
    let (release_sender, release_receiver) = std_mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept blocking chat request");
        let request = read_http_headers(&mut stream);
        request_sender
            .send(request)
            .expect("send captured blocking chat request");
        release_receiver
            .recv()
            .expect("release blocking chat response");
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"leased reply\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        );
        write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write blocking chat response");
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("canonical blocking chat origin"),
        request_receiver,
        release_sender,
    )
}

fn spawn_model_list_http_provider(
    responses: Vec<(String, String)>,
) -> (CanonicalOrigin, std_mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind model-list provider");
    let address = listener.local_addr().expect("model-list provider address");
    let (request_sender, request_receiver) = std_mpsc::channel();
    thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().expect("accept model-list request");
            let request = read_http_headers(&mut stream);
            request_sender
                .send(request)
                .expect("send captured model-list request");
            write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write model-list response");
        }
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}")).expect("canonical model-list origin"),
        request_receiver,
    )
}

fn create_openai_chat_connection(
    core: &Core,
    api_origin: &CanonicalOrigin,
) -> (ProviderTemplate, ProviderConnection) {
    let template = core
        .list_provider_templates()
        .expect("provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
        .expect("OpenAI-compatible template");
    let api_base_url = format!("{}/v1", api_origin.as_str());
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(format!("connection-{}", Uuid::new_v4())),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: "Synthetic OpenAI-compatible".to_owned(),
            api_origin: api_origin.clone(),
            api_base_path: Some(EndpointPath::parse("/v1").expect("API base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            values: vec![ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(api_base_url),
            }],
            approved_credential_origin: Some(api_origin.clone()),
            local_network_approval: None,
            timeout_seconds: 5,
        })
        .expect("create model-list connection");
    (template, connection)
}

fn create_built_in_public_route(
    core: &Core,
    template_id: &str,
    api_base_path: &str,
    model_id: &str,
) -> (ProviderTemplate, ModelRoute) {
    let template = core
        .list_provider_templates()
        .expect("provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == template_id)
        .expect("requested built-in template");
    let api_origin = template
        .default_manifest
        .default_api_origin
        .clone()
        .expect("built-in public origin");
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(format!("connection-{}", Uuid::new_v4())),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: format!("Synthetic {template_id}"),
            api_origin: api_origin.clone(),
            api_base_path: Some(
                EndpointPath::parse(api_base_path).expect("built-in API base path"),
            ),
            network_mode: ProviderNetworkMode::Public,
            values: Vec::new(),
            approved_credential_origin: Some(api_origin),
            local_network_approval: None,
            timeout_seconds: 5,
        })
        .expect("create built-in public connection");
    let now = Utc::now();
    let route = ModelRoute {
        id: ModelRouteId::from(format!("route-{}", Uuid::new_v4())),
        connection_id: connection.id,
        api_family: template.api_family,
        model_id: model_id.to_owned(),
        display_name: Some(model_id.to_owned()),
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
    };
    core.upsert_model_route(route.clone())
        .expect("save built-in model route");
    (template, route)
}

fn install_provider_credential_authority(
    core: &Core,
    connection_id: &ProviderConnectionId,
) -> ProviderCredentialAccessAuthority {
    let authority = core
        .inner
        .storage
        .propose_provider_credential_install_authority(connection_id)
        .expect("propose credential install authority");
    let install = core
        .inner
        .storage
        .prepare_provider_credential_operation_with_install_authority(
            connection_id,
            ProviderCredentialOperationKind::Install,
            ProviderCredentialObservedStatus::Missing,
            Some(&authority),
        )
        .expect("prepare credential install");
    core.inner
        .storage
        .start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
        .expect("start credential install");
    core.inner
        .storage
        .finish_provider_credential_operation(
            &install.plan.operation_id,
            &install.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("finish credential install");
    core.inner
        .storage
        .ensure_provider_credential_access_settled(connection_id)
        .expect("read credential authority")
}

fn install_then_remove_provider_credential(
    core: &Core,
    connection_id: &ProviderConnectionId,
) -> ProviderCredentialAccessAuthority {
    let cached_authority = install_provider_credential_authority(core, connection_id);
    let removal = core
        .inner
        .storage
        .prepare_provider_credential_operation(
            connection_id,
            ProviderCredentialOperationKind::RemoveCredential,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("prepare credential removal");
    core.inner
        .storage
        .start_provider_credential_operation(&removal.plan.operation_id, &removal.plan_sha256)
        .expect("start credential removal");
    core.inner
        .storage
        .finish_provider_credential_operation(
            &removal.plan.operation_id,
            &removal.plan_sha256,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("finish credential removal");
    cached_authority
}

struct DurableAttemptDropProbe {
    storage: Arc<Storage>,
    conversation_id: ConversationId,
    operation_id: String,
    sender: Option<std_mpsc::SyncSender<bool>>,
}

impl Drop for DurableAttemptDropProbe {
    fn drop(&mut self) {
        let durable = self
            .storage
            .get_generation_attempt_by_operation_id(&self.conversation_id, &self.operation_id)
            .is_ok();
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(durable);
        }
    }
}

fn listed_openrouter_model(
    model_id: &str,
    mut parameters: Vec<OpenRouterSupportedParameter>,
    reasoning: Option<ListedModelReasoningCapability>,
    max_output_tokens: Option<u64>,
) -> ListedModel {
    parameters.sort();
    parameters.dedup();
    let mut supported = Vec::new();
    if parameters.iter().any(|parameter| {
        matches!(
            parameter,
            OpenRouterSupportedParameter::Reasoning | OpenRouterSupportedParameter::ReasoningEffort
        )
    }) {
        supported.push(ListedModelCapability::Reasoning);
    }
    if parameters.contains(&OpenRouterSupportedParameter::Tools) {
        supported.push(ListedModelCapability::ToolCalling);
    }
    if parameters.contains(&OpenRouterSupportedParameter::ParallelToolCalls) {
        supported.push(ListedModelCapability::ParallelToolCalling);
    }
    if parameters.contains(&OpenRouterSupportedParameter::StructuredOutputs) {
        supported.push(ListedModelCapability::StructuredOutput);
    }
    if parameters.contains(&OpenRouterSupportedParameter::ResponseFormat) {
        supported.push(ListedModelCapability::JsonMode);
    }
    if parameters.contains(&OpenRouterSupportedParameter::Logprobs) {
        supported.push(ListedModelCapability::Logprobs);
    }
    if parameters.contains(&OpenRouterSupportedParameter::Seed) {
        supported.push(ListedModelCapability::Seed);
    }
    supported.sort();
    ListedModel {
        model_id: model_id.to_owned(),
        display_name: Some(model_id.to_owned()),
        max_input_tokens: Some(128_000),
        max_output_tokens,
        supported_generation_methods: Vec::new(),
        capabilities: ListedModelCapabilities {
            supported,
            parameters: OpenRouterSupportedParameterSupport::Exact(parameters),
            reasoning,
        },
        source: ModelRecordSource::ProviderApi,
        availability: ModelAvailability::Available,
    }
}

fn provider_api_openrouter_route(
    connection_id: ProviderConnectionId,
    model: &ListedModel,
    observed_at: DateTime<Utc>,
) -> ModelRoute {
    ModelRoute {
        id: ModelRouteId::from(format!("route-{}", Uuid::new_v4())),
        connection_id,
        api_family: ApiFamily::OpenAiChatCompletions,
        model_id: model.model_id.clone(),
        display_name: model.display_name.clone(),
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: Some(listed_model_metadata(model).expect("listed model metadata")),
        metadata_source: ModelMetadataSource::ProviderApi,
        metadata_observed_at: Some(observed_at),
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: observed_at,
        last_seen_at: Some(observed_at),
    }
}

fn refresh_models_with_review(
    core: &Core,
    connection_id: &ProviderConnectionId,
    credential: Option<&str>,
) -> CoreResult<ProviderModelRefreshResult> {
    let job_id = core.start_provider_model_sync(connection_id, credential.map(str::to_owned))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let job = core.get_provider_model_sync(&job_id)?;
        match job.state {
            ModelSyncState::DiffReadyAwaitingReview => {
                let review = job
                    .review
                    .ok_or_else(|| CoreError::internal("review-ready model sync has no review"))?;
                core.approve_provider_model_sync(&job_id, &review.sha256)?;
                let diff = review.diff;
                return Ok(ProviderModelRefreshResult {
                    connection_id: diff.connection_id.clone(),
                    model_routes: core.list_model_routes(&diff.connection_id)?,
                    newly_seen_model_route_ids: diff.newly_seen_model_route_ids,
                    missing_model_route_ids: diff.missing_model_route_ids,
                    created_generation_preset_ids: diff
                        .initial_presets
                        .into_iter()
                        .map(|preset| preset.id)
                        .collect(),
                    routes_requiring_preset_configuration: diff
                        .routes_requiring_preset_configuration,
                    provenance: ProviderModelRefreshProvenance {
                        source: diff.provenance.source,
                        api_family: diff.provenance.api_family,
                        api_origin: diff.provenance.api_origin,
                        endpoint_path: diff.provenance.endpoint_path,
                    },
                    pages_fetched: diff.provenance.pages_fetched,
                    response_bytes: diff.provenance.response_bytes,
                    observed_at: diff.observed_at,
                });
            }
            ModelSyncState::Failed => {
                let failure = job
                    .failure
                    .ok_or_else(|| CoreError::internal("failed model sync has no failure"))?;
                let failure_code = match failure.code.as_str() {
                    "invalid_input" => CoreErrorCode::InvalidInput,
                    "unsupported_content" => CoreErrorCode::UnsupportedContent,
                    "unsafe_archive" => CoreErrorCode::UnsafeArchive,
                    "not_found" => CoreErrorCode::NotFound,
                    "permission_denied" => CoreErrorCode::PermissionDenied,
                    "storage_unavailable" => CoreErrorCode::StorageUnavailable,
                    "storage_corrupted" => CoreErrorCode::StorageCorrupted,
                    "provider_auth_failed" => CoreErrorCode::ProviderAuthFailed,
                    "provider_rate_limited" => CoreErrorCode::ProviderRateLimited,
                    "provider_unavailable" => CoreErrorCode::ProviderUnavailable,
                    "network_unavailable" => CoreErrorCode::NetworkUnavailable,
                    "cancelled" => CoreErrorCode::Cancelled,
                    _ => CoreErrorCode::Internal,
                };
                return Err(CoreError::new(
                    failure_code,
                    failure.message_key,
                    failure.recoverable,
                ));
            }
            ModelSyncState::Cancelled => {
                return Err(CoreError::new(
                    CoreErrorCode::Cancelled,
                    "model synchronization was cancelled",
                    true,
                ));
            }
            ModelSyncState::Interrupted => {
                return Err(CoreError::new(
                    CoreErrorCode::StorageUnavailable,
                    "model synchronization was interrupted",
                    true,
                ));
            }
            ModelSyncState::Created
            | ModelSyncState::Fetching
            | ModelSyncState::Committing
            | ModelSyncState::Completed => {}
        }
        if Instant::now() >= deadline {
            return Err(CoreError::internal(
                "model synchronization did not reach review state",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn create_openai_chat_generation_target(
    core: &Core,
    api_origin: &CanonicalOrigin,
) -> (GenerationTarget, ModelRoute) {
    let (template, connection) = create_openai_chat_connection(core, api_origin);
    let now = Utc::now();
    let route = ModelRoute {
        id: ModelRouteId::from(format!("route-{}", Uuid::new_v4())),
        connection_id: connection.id,
        api_family: template.api_family,
        model_id: "reasoning-model".to_owned(),
        display_name: Some("Reasoning Model".to_owned()),
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
    };
    core.upsert_model_route(route.clone())
        .expect("save model route");
    let preset = GenerationPreset {
        id: GenerationPresetId::from(format!("preset-{}", Uuid::new_v4())),
        model_route_id: route.id.clone(),
        display_name: "Reasoning and cache".to_owned(),
        values: Vec::new(),
        reasoning: GenerationReasoningSettings {
            mode: GenerationReasoningMode::Enabled,
            effort: Some(GenerationReasoningEffort::High),
            budget_tokens: None,
            summary: GenerationReasoningSummary::ProviderDefault,
            preserve_opaque_state: false,
        },
        prompt_cache: GenerationPromptCacheSettings {
            mode: GenerationPromptCacheMode::Automatic,
            ttl: GenerationPromptCacheTtl::ProviderDefault,
            context_reference: None,
        },
        created_at: now,
        updated_at: now,
    };
    // Seed a pre-gate stored candidate so the tests below can exercise
    // generation-time repair behavior. Public Core upserts now reject this
    // unsupported reasoning/cache combination before persistence.
    core.inner
        .storage
        .save_generation_preset(&preset)
        .expect("seed legacy generation preset");
    (
        GenerationTarget {
            model_route_id: route.id.clone(),
            generation_preset_id: preset.id,
        },
        route,
    )
}

#[test]
fn generation_operation_nonce_validation_is_bounded_and_core_owned() {
    let semantic_base_fingerprint_sha256 = Sha256Digest::parse(
        "b58c8a55aa6f52703d8c7c98f80690fb401e9867f30ed59ca4d4899749d50525".to_owned(),
    )
    .expect("valid semantic fingerprint fixture");
    let valid = "a".repeat(MAX_GENERATION_OPERATION_NONCE_CHARS);
    let operation_id = new_generation_operation_id(
        "generation-send-v5",
        &semantic_base_fingerprint_sha256,
        &valid,
    )
    .expect("accept a nonce at the Core character bound");
    assert!(operation_id.starts_with("generation-send-v5-"));

    for invalid in [
        String::new(),
        " padded".to_owned(),
        "control\nvalue".to_owned(),
        "a".repeat(MAX_GENERATION_OPERATION_NONCE_CHARS + 1),
        "가".repeat(43),
    ] {
        let error = new_generation_operation_id(
            "generation-send-v5",
            &semantic_base_fingerprint_sha256,
            &invalid,
        )
        .expect_err("Core must reject an invalid generation operation nonce");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
    }
}

#[test]
fn reviewed_prompt_session_seed_is_sqlite_safe_for_a_high_bit_digest() {
    const SQLITE_SIGNED_INTEGER_MAX: u64 = 0x7fff_ffff_ffff_ffff;
    let base_request_fingerprint_sha256 = Sha256Digest::parse(
        "1ac4b8f106727907443ce712070c9aa78bf9cd5b99a97af24efacc61a1276fb3".to_owned(),
    )
    .expect("valid SHA-256 fixture");
    let digest = Sha256::digest(
        format!(
            "reviewed-prompt-session-seed-v2:{}",
            base_request_fingerprint_sha256.as_str()
        )
        .as_bytes(),
    );
    let raw_seed = u64::from_be_bytes(
        digest[..8]
            .try_into()
            .expect("SHA-256 always contains eight seed bytes"),
    );
    assert!(
        raw_seed > SQLITE_SIGNED_INTEGER_MAX,
        "fixture must exercise the formerly rejected upper-half seed"
    );
    let bounded = reviewed_prompt_session_seed(&base_request_fingerprint_sha256);
    assert_eq!(bounded, raw_seed & SQLITE_SIGNED_INTEGER_MAX);
    assert!(bounded <= SQLITE_SIGNED_INTEGER_MAX);
    assert_eq!(
        bounded,
        reviewed_prompt_session_seed(&base_request_fingerprint_sha256)
    );
}

#[test]
fn connection_bound_credential_rejects_rebound_target_before_chat_mutation() {
    let (_root, core, character) = imported_core();
    let conversation = core
        .create_conversation(&character.id, "Bound credential", ConversationMode::Chat)
        .expect("conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let (template, route) =
        create_built_in_public_route(&core, "openai-responses-v1", "/v1", "gpt-bound-fixture");
    let preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
        .expect("generation preset");
    let expected_connection_id = route.connection_id.clone();
    let target = GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: preset.id,
    };
    let credential_canary = "synthetic-bound-credential";
    let wrong_connection_id = ProviderConnectionId::from("different-connection");

    let send_error = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &branch,
            None,
            ConversationMode::Chat,
            "must not be stored",
            new_test_generation_operation("bound-send-v1"),
            &target,
            ConnectionBoundCredential::new(
                wrong_connection_id.clone(),
                Some(credential_canary.to_owned()),
            ),
        )
        .expect_err("send must reject a credential bound to another connection");
    let edit_error = core
        .edit_user_message_with_connection_credential(
            &conversation.id,
            &branch,
            None,
            &MessageId("missing-user-message".to_owned()),
            "must not be stored",
            new_test_generation_operation("bound-edit-v1"),
            &target,
            ConnectionBoundCredential::new(
                wrong_connection_id.clone(),
                Some(credential_canary.to_owned()),
            ),
        )
        .expect_err("edit must reject a credential bound to another connection");
    let regenerate_error = core
        .regenerate_assistant_message_with_connection_credential(
            &conversation.id,
            &branch,
            None,
            &MessageId("missing-assistant-message".to_owned()),
            new_test_generation_operation("bound-regenerate-v1"),
            &target,
            ConnectionBoundCredential::new(wrong_connection_id, Some(credential_canary.to_owned())),
        )
        .expect_err("regenerate must reject a credential bound to another connection");

    for error in [send_error, edit_error, regenerate_error] {
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        assert_eq!(
            error.message,
            "credential does not belong to the selected provider connection"
        );
    }
    assert!(
        core.list_branch_messages(&branch)
            .expect("messages after rejected operations")
            .is_empty()
    );
    assert!(
        core.inner
            .active_generations
            .active
            .lock()
            .expect("generation registry")
            .is_empty()
    );

    let (resolved, credential) = resolve_generation_target_with_connection_credential(
        &core,
        &target,
        ConnectionBoundCredential::new(expected_connection_id, Some(credential_canary.to_owned())),
    )
    .expect("matching connection binding resolves");
    assert_eq!(resolved.model, "gpt-bound-fixture");
    assert_eq!(credential.as_deref(), Some(credential_canary));
}

#[test]
fn terminal_credential_removal_rejects_cached_generation_before_provider_work() {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "Stale generation credential",
            ConversationMode::Chat,
        )
        .expect("conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let (template, route) =
        create_built_in_public_route(&core, "openai-responses-v1", "/v1", "gpt-stale-authority");
    let preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
        .expect("generation preset");
    let connection_id = route.connection_id.clone();
    let target = GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: preset.id,
    };
    let cached_authority = install_then_remove_provider_credential(&core, &connection_id);

    let error = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &branch,
            None,
            ConversationMode::Chat,
            "must remain transient",
            new_test_generation_operation("stale-generation-authority-v1"),
            &target,
            ConnectionBoundCredential::new_with_access_authority(
                connection_id,
                Some("cached-secret".to_owned()),
                cached_authority,
            ),
        )
        .expect_err("terminal removal must reject cached generation authority");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert!(
        core.list_branch_messages(&branch)
            .expect("messages after rejected generation")
            .is_empty()
    );
    assert!(
        core.inner
            .active_generations
            .active
            .lock()
            .expect("generation registry")
            .is_empty()
    );
    let connection = rusqlite::Connection::open(root.path().join("db/lorepia.sqlite3"))
        .expect("open generation database");
    let (attempt_count, generation_count) = connection
        .query_row(
            "SELECT
                   (SELECT COUNT(*) FROM generation_attempt_intents),
                   (SELECT COUNT(*) FROM generations)",
            [],
            |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?)),
        )
        .expect("count rejected generation rows");
    assert_eq!((attempt_count, generation_count), (0, 0));
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one vertical proves exact credential authority epochs before generation admission"
)]
fn reinstalled_credential_rejects_cached_generation_authority_before_provider_work() {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "Reinstalled generation credential",
            ConversationMode::Chat,
        )
        .expect("conversation");
    let branch = core
        .get_conversation_state(&conversation.id)
        .expect("conversation state")
        .active_branch_id;
    let (api_origin, requests) = spawn_chat_completion_provider();
    let (template, connection) = create_openai_chat_connection(&core, &api_origin);
    let connection_id = connection.id.clone();
    let now = Utc::now();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from("reinstalled-credential-generation-route"),
            connection_id: connection_id.clone(),
            api_family: template.api_family,
            model_id: "reinstalled-credential-generation-model".to_owned(),
            display_name: Some("Reinstalled credential model".to_owned()),
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
        .expect("save generation route");
    let preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, now))
        .expect("save generation preset");
    let target = GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: preset.id,
    };

    let cached_authority = install_then_remove_provider_credential(&core, &connection_id);
    let current_authority = install_provider_credential_authority(&core, &connection_id);
    assert_ne!(
        cached_authority.authority_id,
        current_authority.authority_id
    );
    assert_eq!(
        cached_authority.connection_binding_sha256, current_authority.connection_binding_sha256,
        "reinstall must retain the same immutable connection binding"
    );

    let stale_error = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &branch,
            None,
            ConversationMode::Chat,
            "must remain transient",
            new_test_generation_operation("cached-reinstalled-authority-v1"),
            &target,
            ConnectionBoundCredential::new_with_access_authority(
                connection_id.clone(),
                Some("cached-secret".to_owned()),
                cached_authority,
            ),
        )
        .expect_err("cached pre-removal authority must not admit generation");
    assert_eq!(stale_error.code, CoreErrorCode::InvalidInput);
    assert!(stale_error.recoverable);
    assert_eq!(
        requests.recv_timeout(Duration::from_millis(250)),
        Err(std_mpsc::RecvTimeoutError::Timeout),
        "stale authority must not reach provider work"
    );
    assert!(
        core.list_branch_messages(&branch)
            .expect("messages after stale authority rejection")
            .is_empty()
    );
    let database = rusqlite::Connection::open(hard_crash_database_path(root.path()))
        .expect("open active generation database");
    let (attempt_count, generation_count, message_count) = database
        .query_row(
            "SELECT
                   (SELECT COUNT(*) FROM generation_attempt_intents),
                   (SELECT COUNT(*) FROM generations),
                   (SELECT COUNT(*) FROM messages)",
            [],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .expect("count rejected generation rows");
    assert_eq!((attempt_count, generation_count, message_count), (0, 0, 0));
    drop(database);

    let generation_id = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &branch,
            None,
            ConversationMode::Chat,
            "use the current authority",
            new_test_generation_operation("current-reinstalled-authority-v1"),
            &target,
            ConnectionBoundCredential::new_with_access_authority(
                connection_id,
                Some("fresh-secret".to_owned()),
                current_authority,
            ),
        )
        .expect("current authority admits generation");
    let request = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("current authority reaches provider");
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer fresh-secret\r\n")
    );
    wait_for_generation_status(&core, &generation_id, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let messages = core
        .list_branch_messages(&branch)
        .expect("completed generation messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1].content, "fresh authority reply");
}
