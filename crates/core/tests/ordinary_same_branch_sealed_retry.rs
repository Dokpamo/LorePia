//! Ordinary same-branch sends retain their sealed target across approval pauses.

use std::{
    future::Future,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    pin::Pin,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use chrono::{Duration as ChronoDuration, Utc};
use lorepia_core::{
    ApiFamily, BoundedJson, CanonicalOrigin, CapabilityKey, CapabilityObservation, CapabilityValue,
    Confidence, ConnectionBoundCredential, ContentModuleActivationRequest,
    ContentModuleBindingDraft, ContentModuleRuntimeTarget, ConversationBranchId, ConversationId,
    ConversationMode, Core, CoreConfig, CoreError, CoreErrorCode, CoreResult, EndpointPath,
    GenerationId, GenerationOperationContext, GenerationPreset, GenerationPromptCacheSettings,
    GenerationReasoningEffort, GenerationReasoningSettings, GenerationTarget, MessageStatus,
    ModelAvailability, ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId,
    ModuleActivationApproval, ModuleMergeResolutionSet, ObservationId, ObservationSource,
    ParameterId, ParameterLiteral, ParameterValue, ParameterValueState, ProviderConnection,
    ProviderConnectionDraft, ProviderConnectionId, ProviderNetworkMode, ProviderProfile,
    ProviderTemplateId, RoomOrchestrationConfig, RoomOrchestrationConfigPatch, SupportStatus,
    TaskCredentialBroker,
};
use lorepia_domain::{
    ContentCapability, ContentModule, ContentModuleId, InteractionAction, InteractionEvent,
    InteractionProposalDecision, InteractionProposalStatus, InteractionRule, InteractionRuleId,
    InteractionRuleSet, InteractionRuleSetId, ModuleBindingId, ModuleRevisionResolutionMode,
    ModuleScope, PackageMetadata, ProposalSpec, Provenance, SafeTemplate, SourceKind, TemplatePart,
    VariableMap,
};
use lorepia_providers::{
    ListedModelCapabilities, ListedModelCapability, ListedModelReasoningCapability,
    OpenRouterReasoningEffort, OpenRouterReasoningEffortSupport, OpenRouterSupportedParameter,
    OpenRouterSupportedParameterSupport,
    parameter_mapping::{OpenRouterReasoningWireStyle, ReasoningWireDialect},
};
use lorepia_storage::{
    GenerationAttemptStatus, GenerationProviderTargetAuthority, PromptResponseLength,
    ProviderCredentialAccessAuthority, ProviderCredentialObservedStatus, Storage,
};
use rusqlite::{Connection, params};
use tempfile::{NamedTempFile, TempDir, tempdir};
use tokio::sync::watch;

const CONNECTION_ID: &str = "synthetic-ordinary-sealed-connection";
const PROFILE_ID: &str = "synthetic-ordinary-sealed-profile";
const REQUEST_TEXT: &str = "Synthetic ordinary sealed retry";
const CREDENTIAL_CANARY: &str = "synthetic-ordinary-credential-canary-71a9";
const TARGET_OPERATION_NONCE: &str = "ordinary-sealed-target-send-v1";
const PROFILE_OPERATION_NONCE: &str = "ordinary-sealed-profile-send-v1";

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = std::fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(entry.path().join("generation-manifest.json"))
                    .expect("read generation manifest"),
            )
            .expect("parse generation manifest");
            let sequence = manifest["activation_sequence"]
                .as_u64()
                .expect("generation activation sequence");
            let relative = manifest["active_database_relative_path"]
                .as_str()
                .expect("active database relative path")
                .to_owned();
            (sequence, relative)
        })
        .max_by_key(|(sequence, _)| *sequence)
        .expect("at least one committed database generation");
    root.join(relative)
}

struct RejectingTaskCredentialBroker;

impl TaskCredentialBroker for RejectingTaskCredentialBroker {
    fn credential_for<'a>(
        &'a self,
        _connection_id: &'a ProviderConnectionId,
    ) -> Pin<Box<dyn Future<Output = CoreResult<ConnectionBoundCredential>> + Send + 'a>> {
        Box::pin(async {
            Err(CoreError::invalid(
                "ordinary generation must use its explicitly bound credential",
            ))
        })
    }
}

struct OrdinaryFixture {
    root: TempDir,
    core: Core,
    conversation_id: ConversationId,
    branch_id: ConversationBranchId,
    target: GenerationTarget,
    credential_authority: ProviderCredentialAccessAuthority,
    provider_profile_id: String,
    requests: mpsc::Receiver<Vec<u8>>,
    provider: thread::JoinHandle<()>,
}

struct AuthorityFixture {
    root: TempDir,
    core: Core,
    conversation_id: ConversationId,
    branch_id: ConversationBranchId,
    target: GenerationTarget,
    credential_authority: ProviderCredentialAccessAuthority,
    provider_profile_id: String,
}

struct StopAwareLoopbackProvider {
    origin: CanonicalOrigin,
    requests: mpsc::Receiver<Vec<u8>>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl StopAwareLoopbackProvider {
    fn spawn(response_delay: Duration) -> Self {
        let listener =
            TcpListener::bind("127.0.0.1:0").expect("bind stop-aware synthetic authority provider");
        listener
            .set_nonblocking(true)
            .expect("make synthetic authority provider stop-aware");
        let address = listener
            .local_addr()
            .expect("read synthetic authority provider address");
        let origin = CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("parse synthetic authority provider origin");
        let (request_sender, requests) = mpsc::channel();
        let (stop_sender, stop_receiver) = mpsc::channel();
        let provider_thread = thread::spawn(move || {
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Darwin may inherit O_NONBLOCK from the listener; only accept needs polling.
                        stream
                            .set_nonblocking(false)
                            .expect("make accepted provider request blocking");
                        request_sender
                            .send(read_http_request(&mut stream))
                            .expect("capture synthetic authority provider request");
                        if !response_delay.is_zero() {
                            thread::sleep(response_delay);
                        }
                        let body = concat!(
                            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Synthetic authority reply\"}}]}\n\n",
                            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                            "data: [DONE]\n\n"
                        );
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        )
                        .expect("write synthetic authority provider response");
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        match stop_receiver.recv_timeout(Duration::from_millis(10)) {
                            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                        }
                    }
                    Err(error) => panic!("accept synthetic authority provider request: {error}"),
                }
            }
        });
        Self {
            origin,
            requests,
            stop: Some(stop_sender),
            thread: Some(provider_thread),
        }
    }

    fn assert_no_request(&self, stage: &str) {
        assert!(
            matches!(self.requests.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "provider unexpectedly received a request {stage}"
        );
    }

    fn receive_request(&self, stage: &str) -> Vec<u8> {
        self.requests
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|error| panic!("receive provider request {stage}: {error}"))
    }
}

impl Drop for StopAwareLoopbackProvider {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(provider_thread) = self.thread.take() {
            provider_thread
                .join()
                .expect("join stop-aware synthetic authority provider");
        }
    }
}

struct PausedAttempt {
    generation_id: GenerationId,
    operation_id: String,
}

fn provenance(source_id: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic ordinary sealed retry test".to_owned()),
        license: Some("LicenseRef-Synthetic-Test".to_owned()),
        imported_at: None,
    }
}

fn approval_rule_set() -> InteractionRuleSet {
    InteractionRuleSet {
        id: InteractionRuleSetId::from("synthetic.ordinary-sealed.rules"),
        name: "Synthetic ordinary sealed approval".to_owned(),
        schema_version: 1,
        rules: vec![
            InteractionRule {
                id: InteractionRuleId::from("synthetic.ordinary-sealed.before"),
                name: "Pause ordinary send for approval".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::BeforeGeneration,
                condition: None,
                actions: vec![InteractionAction::RequestUserApproval {
                    proposal: ProposalSpec {
                        id: "approve-ordinary-send".to_owned(),
                        title: "Approve ordinary synthetic send".to_owned(),
                        body: SafeTemplate {
                            parts: vec![TemplatePart::Text {
                                value: "SYNTHETIC_ORDINARY_APPROVAL".to_owned(),
                            }],
                            max_output_chars: 128,
                        },
                        expires_after_seconds: None,
                    },
                }],
                priority: 0,
                stop_after_match: false,
                provenance: provenance("synthetic.ordinary-sealed.before"),
            },
            InteractionRule {
                id: InteractionRuleId::from("synthetic.ordinary-sealed.approved"),
                name: "Record ordinary approval".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::UserAction {
                    action_id: "approve-ordinary-send".to_owned(),
                },
                condition: None,
                actions: vec![InteractionAction::AppendVisibleSystemEvent {
                    text: SafeTemplate {
                        parts: vec![TemplatePart::Text {
                            value: "SYNTHETIC_ORDINARY_APPROVED".to_owned(),
                        }],
                        max_output_chars: 128,
                    },
                }],
                priority: 0,
                stop_after_match: false,
                provenance: provenance("synthetic.ordinary-sealed.approved"),
            },
        ],
        max_actions_per_event: 4,
        provenance: provenance("synthetic.ordinary-sealed.rules"),
    }
}

fn approval_module(rule_set_id: InteractionRuleSetId) -> ContentModule {
    ContentModule {
        id: ContentModuleId::from("synthetic.ordinary-sealed.module"),
        name: "Synthetic ordinary sealed module".to_owned(),
        version: "1.0.0".to_owned(),
        schema_version: 1,
        prompt_fragments: Vec::new(),
        knowledge_book_ids: Vec::new(),
        control_specs: Vec::new(),
        transform_set_ids: Vec::new(),
        interaction_rule_set_ids: vec![rule_set_id],
        asset_ids: Vec::new(),
        imported_components_enabled: true,
        required_capabilities: vec![ContentCapability::DeclarativeInteractions],
        metadata: PackageMetadata {
            author: Some("Synthetic ordinary sealed retry test".to_owned()),
            license: "LicenseRef-Synthetic-Test".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: "Synthetic approval pause for an ordinary send".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: provenance("synthetic.ordinary-sealed.module"),
        },
    }
}

fn install_approval_module(core: &Core, runtime_target: ContentModuleRuntimeTarget) {
    let rules = approval_rule_set();
    let module = approval_module(rules.id.clone());
    core.upsert_interaction_rule_set(&rules, None)
        .expect("save ordinary approval rules");
    core.upsert_content_module(&module, None)
        .expect("save ordinary approval module");
    let request = ContentModuleActivationRequest {
        runtime_target,
        expected_binding_revision: None,
        binding: ContentModuleBindingDraft {
            id: ModuleBindingId::from("synthetic.ordinary-sealed.binding"),
            module_id: module.id,
            scope: ModuleScope::App,
            target_id: None,
            conversation_id: None,
            priority: 0,
            resolution_mode: ModuleRevisionResolutionMode::Active,
            pinned_revision_id: None,
            package_import_approval_id: None,
            variable_overrides: VariableMap::default(),
        },
    };
    let review = core
        .review_content_module_activation(&request)
        .expect("review ordinary approval activation");
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let plan = core
        .resolve_content_module_activation(&request, &resolutions)
        .expect("resolve ordinary approval activation");
    core.activate_content_module(
        &request,
        &resolutions,
        &ModuleActivationApproval {
            approval_id: "synthetic-ordinary-sealed-activation".to_owned(),
            expected_review_sha256: review.review_sha256,
            expected_plan_sha256: plan.plan_sha256,
        },
    )
    .expect("activate ordinary approval module")
    .verify()
    .expect("verify ordinary approval activation");
}

fn import_character(core: &Core) -> String {
    let mut source = NamedTempFile::new().expect("create ordinary character source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Ordinary Ari","description":"Entirely synthetic ordinary sealed retry character."}}}}"#
    )
    .expect("write ordinary character source");
    let review = core
        .inspect_import(source.path())
        .expect("inspect character");
    core.commit_import(&review.id)
        .expect("commit ordinary character")
        .id
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set provider request timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = stream.read(&mut buffer).expect("read provider request");
        if read == 0 {
            return request;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else {
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
            return request;
        }
    }
}

fn spawn_provider() -> (
    CanonicalOrigin,
    mpsc::Receiver<Vec<u8>>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ordinary synthetic provider");
    let address = listener.local_addr().expect("read provider address");
    let (sender, receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept provider request");
        sender
            .send(read_http_request(&mut stream))
            .expect("capture provider request");
        let body = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Synthetic ordinary reply\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write provider response");
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("parse ordinary provider origin"),
        receiver,
        handle,
    )
}

fn reasoning_metadata() -> (BoundedJson, CapabilityValue) {
    let supported_efforts =
        OpenRouterReasoningEffortSupport::Exact(vec![OpenRouterReasoningEffort::High]);
    let reasoning = ListedModelReasoningCapability {
        supported_efforts: supported_efforts.clone(),
        default_effort: Some(OpenRouterReasoningEffort::High),
        default_enabled: Some(true),
        supports_max_tokens: Some(false),
        mandatory: Some(false),
    };
    let capabilities = ListedModelCapabilities {
        supported: vec![ListedModelCapability::Reasoning],
        parameters: OpenRouterSupportedParameterSupport::Exact(vec![
            OpenRouterSupportedParameter::MaxCompletionTokens,
            OpenRouterSupportedParameter::ReasoningEffort,
            OpenRouterSupportedParameter::Temperature,
            OpenRouterSupportedParameter::TopP,
        ]),
        reasoning: Some(reasoning),
    };
    let raw_metadata = BoundedJson::from_value(&serde_json::json!({
        "max_input_tokens": 32_768,
        "max_output_tokens": 4_096,
        "supported_generation_methods": [],
        "capabilities": capabilities,
    }))
    .expect("encode ordinary provider metadata");
    let dialect = ReasoningWireDialect::OpenRouter {
        style: OpenRouterReasoningWireStyle::LegacyReasoningEffort,
        supported_efforts,
        default_effort: Some(OpenRouterReasoningEffort::High),
        default_enabled: Some(true),
        supports_max_tokens: Some(false),
        mandatory: Some(false),
    };
    (
        raw_metadata,
        CapabilityValue::Structured(
            serde_json::to_value(dialect).expect("encode ordinary reasoning capability"),
        ),
    )
}

fn provider_fixture(
    core: &Core,
    root: &std::path::Path,
    origin: &CanonicalOrigin,
) -> GenerationTarget {
    let template = core
        .list_provider_templates()
        .expect("list provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "openrouter-v1")
        .expect("OpenRouter provider template");
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(CONNECTION_ID),
            template_id: template.id,
            template_version: template.manifest_version,
            display_name: "Synthetic ordinary OpenRouter".to_owned(),
            api_origin: origin.clone(),
            api_base_path: Some(EndpointPath::parse("/api/v1").expect("parse API base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            local_network_approval: None,
            values: Vec::new(),
            approved_credential_origin: Some(origin.clone()),
            timeout_seconds: 5,
        })
        .expect("create ordinary provider connection");
    let now = Utc::now();
    let (raw_metadata, reasoning_capability) = reasoning_metadata();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from("synthetic-ordinary-sealed-route"),
            connection_id: connection.id,
            api_family: ApiFamily::OpenAiChatCompletions,
            model_id: "synthetic-ordinary-reasoning-model".to_owned(),
            display_name: Some("Synthetic ordinary reasoning model".to_owned()),
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
        .expect("save native ordinary model route");
    let connection = Connection::open(active_database_path(root))
        .expect("open provider metadata fixture connection");
    assert_eq!(
        connection
            .execute(
                "UPDATE provider_models
                 SET raw_metadata_json = ?1,
                     metadata_source_kind = 'provider_api',
                     metadata_observed_at = ?2
                 WHERE id = ?3",
                params![raw_metadata.as_str(), now.to_rfc3339(), route.id.as_str()],
            )
            .expect("install provider-observed model metadata"),
        1
    );
    core.record_provider_api_capability_observations(vec![CapabilityObservation {
        id: ObservationId::from("synthetic-ordinary-sealed-reasoning"),
        model_route_id: route.id.clone(),
        key: CapabilityKey::Reasoning,
        value: reasoning_capability,
        status: SupportStatus::Verified,
        source: ObservationSource::ProviderApi,
        confidence: Confidence::High,
        observed_at: now,
        expires_at: Some(now + ChronoDuration::hours(24)),
        evidence_ref: None,
    }])
    .expect("save ordinary reasoning observation");
    let preset = core
        .upsert_generation_preset(GenerationPreset {
            id: "synthetic-ordinary-sealed-preset".into(),
            model_route_id: route.id.clone(),
            display_name: "Synthetic ordinary sealed preset".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: now,
            updated_at: now,
        })
        .expect("save ordinary generation preset");
    GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: preset.id,
    }
}

fn install_provider_credential_authority(core: &Core) -> ProviderCredentialAccessAuthority {
    let connection_id = ProviderConnectionId::from(CONNECTION_ID);
    let proposed = core
        .propose_provider_credential_install_authority(&connection_id)
        .expect("propose synthetic credential installation authority");
    let prepared = core
        .prepare_provider_credential_install_operation(
            &connection_id,
            &proposed,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare synthetic credential installation");
    assert_eq!(
        prepared.plan.credential_authority_id.as_deref(),
        Some(proposed.authority_id.as_str())
    );
    assert_eq!(
        prepared.plan.credential_authority_binding_sha256.as_deref(),
        Some(proposed.connection_binding_sha256.as_str())
    );
    let started = core
        .start_provider_credential_operation(&prepared.plan.operation_id, &prepared.plan_sha256)
        .expect("start synthetic credential installation");
    if started.plan.predecessor_authority_id.is_some() {
        core.attest_provider_credential_predecessor_delete_intent(
            &started.plan.operation_id,
            &started.plan_sha256,
            ProviderCredentialObservedStatus::Available,
        )
        .expect("attest synthetic predecessor delete intent");
        core.attest_provider_credential_predecessor_missing(
            &started.plan.operation_id,
            &started.plan_sha256,
        )
        .expect("attest synthetic predecessor removal");
    }
    core.finish_provider_credential_operation(
        &started.plan.operation_id,
        &started.plan_sha256,
        ProviderCredentialObservedStatus::Available,
    )
    .expect("finish synthetic credential installation");
    let authority = core
        .ensure_provider_credential_access_settled(&connection_id)
        .expect("read synthetic credential access authority");
    assert_eq!(authority, proposed);
    authority
}

fn save_quick_reasoning(
    core: &Core,
    current: &RoomOrchestrationConfig,
    target: &GenerationTarget,
    reasoning_effort: GenerationReasoningEffort,
) -> RoomOrchestrationConfig {
    core.save_room_orchestration_config(
        &current.conversation_id,
        &current.branch_id,
        current.binding_revision,
        &RoomOrchestrationConfigPatch {
            prompt_preset_id: Some(current.prompt_preset_id.clone()),
            generation_preset_id: Some(target.generation_preset_id.clone()),
            creator_values: current.creator_values.clone(),
            response_length: PromptResponseLength::Short,
            creativity: 20,
            reasoning_effort: Some(reasoning_effort),
            memory_enabled: false,
            knowledge_enabled: false,
            user_name_override: current.user_name_override.clone(),
            author_note: current.author_note.clone(),
            group_context: current.group_context.clone(),
            template_slots: current.template_slots.clone(),
        },
    )
    .expect("save ordinary quick reasoning")
}

fn prepare_fixture() -> OrdinaryFixture {
    let temp_root = tempdir().expect("create ordinary Core root");
    let (origin, requests, provider) = spawn_provider();
    let core = Core::open(CoreConfig::new(temp_root.path())).expect("open ordinary Core");
    let character_id = import_character(&core);
    let target = provider_fixture(&core, temp_root.path(), &origin);
    let credential_authority = install_provider_credential_authority(&core);
    let provider_profile_id = core
        .upsert_provider_profile(ProviderProfile {
            id: PROFILE_ID.to_owned(),
            display_name: "Synthetic ordinary direct profile".to_owned(),
            base_url: format!("{}/v1", origin.as_str()),
            model: "synthetic-ordinary-direct-model".to_owned(),
            timeout_seconds: 5,
        })
        .expect("save ordinary direct provider profile")
        .id;
    let conversation = core
        .open_conversation(&character_id)
        .expect("open ordinary conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list ordinary branches")
        .into_iter()
        .next()
        .expect("ordinary root branch");
    assert!(
        core.drain_core_lifecycle_occurrences(64)
            .expect("initialize ordinary interaction state")
            .queue_idle
    );
    install_approval_module(
        &core,
        ContentModuleRuntimeTarget {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
        },
    );
    let room_config = core
        .get_room_orchestration_config(&conversation.id, &branch.id)
        .expect("load ordinary room");
    let sealed = save_quick_reasoning(
        &core,
        &room_config,
        &target,
        GenerationReasoningEffort::High,
    );
    assert_eq!(
        sealed.reasoning_effort,
        Some(GenerationReasoningEffort::High)
    );
    OrdinaryFixture {
        root: temp_root,
        core,
        conversation_id: conversation.id,
        branch_id: branch.id,
        target,
        credential_authority,
        provider_profile_id,
        requests,
        provider,
    }
}

fn prepare_authority_fixture(origin: &CanonicalOrigin) -> AuthorityFixture {
    let root = tempdir().expect("create provider-authority Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open provider-authority Core");
    let character_id = import_character(&core);
    let target = provider_fixture(&core, root.path(), origin);
    let credential_authority = install_provider_credential_authority(&core);
    let provider_profile_id = core
        .upsert_provider_profile(ProviderProfile {
            id: PROFILE_ID.to_owned(),
            display_name: "Synthetic authority direct profile".to_owned(),
            base_url: format!("{}/v1", origin.as_str()),
            model: "synthetic-ordinary-direct-model".to_owned(),
            timeout_seconds: 5,
        })
        .expect("save provider-authority direct profile")
        .id;
    let conversation = core
        .open_conversation(&character_id)
        .expect("open provider-authority conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list provider-authority branches")
        .into_iter()
        .next()
        .expect("provider-authority root branch");
    assert!(
        core.drain_core_lifecycle_occurrences(64)
            .expect("initialize provider-authority interaction state")
            .queue_idle
    );
    install_approval_module(
        &core,
        ContentModuleRuntimeTarget {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
        },
    );
    let room_config = core
        .get_room_orchestration_config(&conversation.id, &branch.id)
        .expect("load provider-authority room");
    let sealed = save_quick_reasoning(
        &core,
        &room_config,
        &target,
        GenerationReasoningEffort::High,
    );
    assert_eq!(
        sealed.reasoning_effort,
        Some(GenerationReasoningEffort::High)
    );
    AuthorityFixture {
        root,
        core,
        conversation_id: conversation.id,
        branch_id: branch.id,
        target,
        credential_authority,
        provider_profile_id,
    }
}

fn credential(authority: &ProviderCredentialAccessAuthority) -> ConnectionBoundCredential {
    ConnectionBoundCredential::new_with_access_authority(
        ProviderConnectionId::from(CONNECTION_ID),
        Some(CREDENTIAL_CANARY.to_owned()),
        authority.clone(),
    )
}

async fn ordinary_send(
    core: &Core,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    mode: ConversationMode,
    target: &GenerationTarget,
    credential_authority: &ProviderCredentialAccessAuthority,
    operation_context: GenerationOperationContext<'_>,
) -> CoreResult<GenerationId> {
    core.send_message_to_branch_with_connection_credential_async(
        conversation_id,
        branch_id,
        None,
        mode,
        REQUEST_TEXT,
        operation_context,
        target,
        credential(credential_authority),
        &RejectingTaskCredentialBroker,
        watch::channel(false).1,
    )
    .await
}

async fn profile_send(
    core: &Core,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    mode: ConversationMode,
    provider_profile_id: &str,
    operation_context: GenerationOperationContext<'_>,
) -> CoreResult<GenerationId> {
    core.send_message_to_branch_async(
        conversation_id,
        branch_id,
        None,
        mode,
        REQUEST_TEXT,
        operation_context,
        provider_profile_id,
        Some(CREDENTIAL_CANARY.to_owned()),
        &RejectingTaskCredentialBroker,
        watch::channel(false).1,
    )
    .await
}

fn approve_pending_authority_attempt(
    core: &Core,
    root: &std::path::Path,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> PausedAttempt {
    let pending = core
        .list_generation_attempt_proposals_for_source_room(
            conversation_id,
            branch_id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list provider-authority generation approval");
    let [pending] = pending.as_slice() else {
        panic!("expected one provider-authority approval, got {pending:?}");
    };
    let generation_id = pending.proposal.generation_id.clone();
    let operation_id = Connection::open(active_database_path(root))
        .expect("open provider-authority attempt evidence")
        .query_row(
            "SELECT operation_id FROM generation_attempt_intents WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load provider-authority operation identity");
    core.decide_generation_attempt_proposal(
        &lorepia_core::GenerationAttemptProposalDecisionRequest {
            conversation_id: conversation_id.clone(),
            source_branch_id: branch_id.clone(),
            generation_id: generation_id.clone(),
            proposal_record_id: pending.proposal.record.id.clone(),
            expected_aggregate_revision: pending.aggregate_revision,
            expected_proposal_revision: pending.proposal.proposal_revision,
            decision: InteractionProposalDecision::Approve,
        },
    )
    .expect("approve provider-authority generation attempt");
    PausedAttempt {
        generation_id,
        operation_id,
    }
}

async fn begin_target_authority_attempt(
    core: &Core,
    root: &std::path::Path,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    target: &GenerationTarget,
    credential_authority: &ProviderCredentialAccessAuthority,
    operation_nonce: &str,
) -> PausedAttempt {
    let blocked = ordinary_send(
        core,
        conversation_id,
        branch_id,
        ConversationMode::Chat,
        target,
        credential_authority,
        GenerationOperationContext::New { operation_nonce },
    )
    .await
    .expect_err("new target operation must pause for approval");
    assert_eq!(blocked.code, CoreErrorCode::PermissionDenied);
    assert!(blocked.recoverable);
    approve_pending_authority_attempt(core, root, conversation_id, branch_id)
}

async fn begin_profile_authority_attempt(
    core: &Core,
    root: &std::path::Path,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    provider_profile_id: &str,
    operation_nonce: &str,
) -> PausedAttempt {
    let blocked = profile_send(
        core,
        conversation_id,
        branch_id,
        ConversationMode::Chat,
        provider_profile_id,
        GenerationOperationContext::New { operation_nonce },
    )
    .await
    .expect_err("new profile operation must pause for approval");
    assert_eq!(blocked.code, CoreErrorCode::PermissionDenied);
    assert!(blocked.recoverable);
    approve_pending_authority_attempt(core, root, conversation_id, branch_id)
}

fn assert_provider_authority_drift_error(error: &CoreError) {
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(
        error.message,
        "provider configuration changed after generation review; start a new generation operation"
    );
}

fn authority_runtime_counts(
    root: &std::path::Path,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> (u64, u64, u64) {
    let connection = Connection::open(active_database_path(root))
        .expect("open provider-authority runtime evidence");
    let attempts = connection
        .query_row(
            "SELECT COUNT(*) FROM generation_attempt_intents
             WHERE conversation_id = ?1 AND source_branch_id = ?2",
            params![conversation_id.0.as_str(), branch_id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count provider-authority attempts");
    let generations = connection
        .query_row(
            "SELECT COUNT(*) FROM generations
             WHERE conversation_id = ?1 AND branch_id = ?2",
            params![conversation_id.0.as_str(), branch_id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count provider-authority generations");
    let messages = connection
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1",
            [conversation_id.0.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("count provider-authority messages");
    (attempts, generations, messages)
}

fn assert_only_old_authority_attempt(
    root: &std::path::Path,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    old_generation_id: &GenerationId,
) {
    assert_eq!(
        authority_runtime_counts(root, conversation_id, branch_id),
        (1, 0, 0),
        "authority drift must not create a second attempt, generation, or message"
    );
    let stored_generation_id = Connection::open(active_database_path(root))
        .expect("open exact old-attempt evidence")
        .query_row(
            "SELECT generation_id FROM generation_attempt_intents
             WHERE conversation_id = ?1 AND source_branch_id = ?2",
            params![conversation_id.0.as_str(), branch_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load exact old provider-authority attempt");
    assert_eq!(stored_generation_id, old_generation_id.0);
}

fn assert_fresh_authority_append(
    root: &std::path::Path,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) {
    assert_eq!(
        authority_runtime_counts(root, conversation_id, branch_id),
        (2, 1, 2),
        "one explicit fresh operation must be the only dispatched append"
    );
}

async fn wait_for_generation(core: &Core, branch_id: &ConversationBranchId, id: &GenerationId) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let messages = core
            .list_branch_messages(branch_id)
            .expect("read ordinary messages while waiting");
        if let Some(assistant) = messages
            .iter()
            .find(|message| message.generation_id.as_ref() == Some(id))
            && assistant.status != MessageStatus::Pending
        {
            assert_eq!(assistant.status, MessageStatus::Complete);
            return;
        }
        assert!(Instant::now() < deadline, "ordinary generation timed out");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn request_parts(request: &[u8]) -> (String, serde_json::Value) {
    let header_end = request
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .expect("provider request header terminator");
    let headers = String::from_utf8_lossy(&request[..header_end]).into_owned();
    let body = serde_json::from_slice(&request[header_end + 4..])
        .expect("decode ordinary provider request JSON");
    (headers, body)
}

fn assert_single_append(
    root: &std::path::Path,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) {
    let connection = Connection::open(active_database_path(root))
        .expect("open ordinary append evidence connection");
    for (sql, expected, label) in [
        (
            "SELECT COUNT(*) FROM generation_attempt_intents WHERE conversation_id = ?1 AND source_branch_id = ?2",
            1_u64,
            "generation attempts",
        ),
        (
            "SELECT COUNT(*) FROM generations WHERE conversation_id = ?1 AND branch_id = ?2",
            1_u64,
            "generations",
        ),
        (
            "SELECT COUNT(*) FROM messages
             WHERE conversation_id = ?1
               AND EXISTS (
                   SELECT 1 FROM conversation_branches
                   WHERE id = ?2 AND conversation_id = ?1
               )",
            2_u64,
            "messages",
        ),
    ] {
        let count = connection
            .query_row(
                sql,
                params![conversation_id.0.as_str(), branch_id.0.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .unwrap_or_else(|error| panic!("count {label}: {error}"));
        assert_eq!(count, expected, "unexpected {label} count");
    }
}

fn assert_exact_wire(
    core: &Core,
    generation_id: &GenerationId,
    request: &[u8],
) -> serde_json::Value {
    let (headers, body) = request_parts(request);
    assert!(
        headers
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {CREDENTIAL_CANARY}")),
        "provider request omitted the exact credential canary"
    );
    assert!(!body.to_string().contains(CREDENTIAL_CANARY));
    assert_eq!(
        body.get("reasoning_effort")
            .and_then(serde_json::Value::as_str),
        Some("high")
    );
    assert_ne!(
        body.get("reasoning_effort")
            .and_then(serde_json::Value::as_str),
        Some("low")
    );
    assert_eq!(
        body.get("temperature").and_then(serde_json::Value::as_f64),
        Some(0.3)
    );
    assert_eq!(
        body.get("max_completion_tokens")
            .or_else(|| body.get("max_tokens"))
            .and_then(serde_json::Value::as_u64),
        Some(512),
        "sealed response-length budget was not preserved: {body}"
    );
    let stored = core
        .get_generation_prompt_plan(generation_id)
        .expect("load ordinary stored prompt plan");
    assert_eq!(stored.provider_request.request.value, body);
    body
}

fn assert_sealed_attempt(root: &std::path::Path, generation_id: &GenerationId, operation_id: &str) {
    let storage = Storage::open(root).expect("open ordinary attempt evidence storage");
    assert_eq!(
        storage
            .get_generation(generation_id)
            .expect("load ordinary generation")
            .mode,
        ConversationMode::Chat
    );
    let attempt = storage
        .get_generation_attempt(generation_id)
        .expect("load ordinary generation attempt");
    assert_eq!(attempt.status, GenerationAttemptStatus::Completed);
    assert_eq!(attempt.input.operation_id, operation_id);
    let before = storage
        .get_generation_attempt_before_review(generation_id)
        .expect("load ordinary BeforeGeneration review")
        .expect("ordinary BeforeGeneration review exists");
    assert_eq!(
        before.prompt_selection_authority.mode,
        ConversationMode::Chat
    );
    assert_eq!(
        before
            .prompt_selection_authority
            .quick_settings
            .reasoning_effort,
        Some(GenerationReasoningEffort::High)
    );
}

fn assert_exact_profile_wire(
    core: &Core,
    generation_id: &GenerationId,
    request: &[u8],
) -> serde_json::Value {
    let (headers, body) = request_parts(request);
    assert!(
        headers
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {CREDENTIAL_CANARY}"))
    );
    assert_eq!(
        body.get("model").and_then(serde_json::Value::as_str),
        Some("synthetic-ordinary-direct-model")
    );
    let rendered = body.to_string();
    assert!(
        rendered.contains(REQUEST_TEXT),
        "sealed user request missing from direct-profile prompt: {body}"
    );
    assert!(
        !rendered.contains("Story mode:"),
        "live story mode leaked into sealed prompt: {body}"
    );
    let stored = core
        .get_generation_prompt_plan(generation_id)
        .expect("load ordinary direct stored prompt plan");
    assert_eq!(stored.provider_request.request.value, body);
    body
}

async fn pause_and_approve(fixture: &OrdinaryFixture) -> PausedAttempt {
    let blocked = ordinary_send(
        &fixture.core,
        &fixture.conversation_id,
        &fixture.branch_id,
        ConversationMode::Chat,
        &fixture.target,
        &fixture.credential_authority,
        GenerationOperationContext::New {
            operation_nonce: TARGET_OPERATION_NONCE,
        },
    )
    .await
    .expect_err("ordinary send must pause for approval");
    assert_eq!(blocked.code, CoreErrorCode::PermissionDenied);
    assert!(blocked.recoverable);
    let pending = fixture
        .core
        .list_generation_attempt_proposals_for_source_room(
            &fixture.conversation_id,
            &fixture.branch_id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list ordinary generation approval");
    let [pending] = pending.as_slice() else {
        panic!("expected one ordinary approval, got {pending:?}");
    };
    let generation_id = pending.proposal.generation_id.clone();
    let operation_id = Connection::open(active_database_path(fixture.root.path()))
        .expect("open paused ordinary attempt evidence")
        .query_row(
            "SELECT operation_id FROM generation_attempt_intents WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load paused ordinary operation identity");
    let room = fixture
        .core
        .get_room_orchestration_config(&fixture.conversation_id, &fixture.branch_id)
        .expect("load sealed ordinary room");
    let drifted = save_quick_reasoning(
        &fixture.core,
        &room,
        &fixture.target,
        GenerationReasoningEffort::Low,
    );
    assert_eq!(
        drifted.reasoning_effort,
        Some(GenerationReasoningEffort::Low)
    );
    assert_eq!(
        fixture
            .core
            .set_conversation_mode(&fixture.conversation_id, ConversationMode::Story)
            .expect("drift ordinary conversation mode")
            .selected_mode,
        ConversationMode::Story
    );
    fixture
        .core
        .decide_generation_attempt_proposal(
            &lorepia_core::GenerationAttemptProposalDecisionRequest {
                conversation_id: fixture.conversation_id.clone(),
                source_branch_id: fixture.branch_id.clone(),
                generation_id: generation_id.clone(),
                proposal_record_id: pending.proposal.record.id.clone(),
                expected_aggregate_revision: pending.aggregate_revision,
                expected_proposal_revision: pending.proposal.proposal_revision,
                decision: InteractionProposalDecision::Approve,
            },
        )
        .expect("approve ordinary generation attempt");
    PausedAttempt {
        generation_id,
        operation_id,
    }
}

async fn pause_profile_and_approve(fixture: &OrdinaryFixture) -> PausedAttempt {
    let blocked = profile_send(
        &fixture.core,
        &fixture.conversation_id,
        &fixture.branch_id,
        ConversationMode::Chat,
        &fixture.provider_profile_id,
        GenerationOperationContext::New {
            operation_nonce: PROFILE_OPERATION_NONCE,
        },
    )
    .await
    .expect_err("ordinary direct send must pause for approval");
    assert_eq!(blocked.code, CoreErrorCode::PermissionDenied);
    assert!(blocked.recoverable);
    let pending = fixture
        .core
        .list_generation_attempt_proposals_for_source_room(
            &fixture.conversation_id,
            &fixture.branch_id,
            InteractionProposalStatus::Pending,
            10,
        )
        .expect("list ordinary direct generation approval");
    let [pending] = pending.as_slice() else {
        panic!("expected one ordinary direct approval, got {pending:?}");
    };
    let generation_id = pending.proposal.generation_id.clone();
    let operation_id = Connection::open(active_database_path(fixture.root.path()))
        .expect("open paused ordinary direct attempt evidence")
        .query_row(
            "SELECT operation_id FROM generation_attempt_intents WHERE generation_id = ?1",
            [generation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load paused ordinary direct operation identity");
    assert_eq!(
        fixture
            .core
            .set_conversation_mode(&fixture.conversation_id, ConversationMode::Story)
            .expect("drift ordinary direct conversation mode")
            .selected_mode,
        ConversationMode::Story
    );
    fixture
        .core
        .decide_generation_attempt_proposal(
            &lorepia_core::GenerationAttemptProposalDecisionRequest {
                conversation_id: fixture.conversation_id.clone(),
                source_branch_id: fixture.branch_id.clone(),
                generation_id: generation_id.clone(),
                proposal_record_id: pending.proposal.record.id.clone(),
                expected_aggregate_revision: pending.aggregate_revision,
                expected_proposal_revision: pending.proposal.proposal_revision,
                decision: InteractionProposalDecision::Approve,
            },
        )
        .expect("approve ordinary direct generation attempt");
    PausedAttempt {
        generation_id,
        operation_id,
    }
}

#[tokio::test]
async fn ordinary_async_retry_reuses_sealed_reasoning_mode_and_exact_append() {
    let fixture = prepare_fixture();
    let paused = pause_and_approve(&fixture).await;
    let dispatched = ordinary_send(
        &fixture.core,
        &fixture.conversation_id,
        &fixture.branch_id,
        ConversationMode::Story,
        &fixture.target,
        &fixture.credential_authority,
        GenerationOperationContext::Resume {
            generation_attempt_id: &paused.generation_id,
        },
    )
    .await
    .expect("resume ordinary send from its sealed attempt");
    assert_eq!(dispatched, paused.generation_id);
    wait_for_generation(&fixture.core, &fixture.branch_id, &paused.generation_id).await;
    let provider_request = fixture
        .requests
        .recv_timeout(Duration::from_secs(5))
        .expect("receive ordinary provider request");
    let wire_body = assert_exact_wire(&fixture.core, &paused.generation_id, &provider_request);
    let messages = fixture
        .core
        .list_branch_messages(&fixture.branch_id)
        .expect("load completed ordinary messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, REQUEST_TEXT);
    assert_eq!(messages[1].content, "Synthetic ordinary reply");
    drop(fixture.core);
    fixture.provider.join().expect("join ordinary provider");
    assert_sealed_attempt(
        fixture.root.path(),
        &paused.generation_id,
        &paused.operation_id,
    );

    let reopened = Core::open(CoreConfig::new(fixture.root.path())).expect("reopen ordinary Core");
    let replayed = ordinary_send(
        &reopened,
        &fixture.conversation_id,
        &fixture.branch_id,
        ConversationMode::Story,
        &fixture.target,
        &fixture.credential_authority,
        GenerationOperationContext::Resume {
            generation_attempt_id: &paused.generation_id,
        },
    )
    .await
    .expect("replay completed ordinary send");
    assert_eq!(replayed, paused.generation_id);
    assert_eq!(
        reopened
            .list_branch_messages(&fixture.branch_id)
            .expect("load replayed ordinary messages"),
        messages
    );
    assert_eq!(
        reopened
            .get_generation_prompt_plan(&paused.generation_id)
            .expect("reload ordinary prompt plan")
            .provider_request
            .request
            .value,
        wire_body
    );
    assert_single_append(
        fixture.root.path(),
        &fixture.conversation_id,
        &fixture.branch_id,
    );
}

#[tokio::test]
async fn public_profile_retry_reuses_sealed_mode_and_exact_append() {
    let fixture = prepare_fixture();
    let paused = pause_profile_and_approve(&fixture).await;
    let dispatched = profile_send(
        &fixture.core,
        &fixture.conversation_id,
        &fixture.branch_id,
        ConversationMode::Story,
        &fixture.provider_profile_id,
        GenerationOperationContext::Resume {
            generation_attempt_id: &paused.generation_id,
        },
    )
    .await
    .expect("resume ordinary direct send from its sealed attempt");
    assert_eq!(dispatched, paused.generation_id);
    wait_for_generation(&fixture.core, &fixture.branch_id, &paused.generation_id).await;
    let provider_request = fixture
        .requests
        .recv_timeout(Duration::from_secs(5))
        .expect("receive ordinary direct provider request");
    let wire_body =
        assert_exact_profile_wire(&fixture.core, &paused.generation_id, &provider_request);
    let messages = fixture
        .core
        .list_branch_messages(&fixture.branch_id)
        .expect("load completed ordinary direct messages");
    drop(fixture.core);
    fixture
        .provider
        .join()
        .expect("join ordinary direct provider");
    assert_sealed_attempt(
        fixture.root.path(),
        &paused.generation_id,
        &paused.operation_id,
    );

    let reopened =
        Core::open(CoreConfig::new(fixture.root.path())).expect("reopen ordinary direct Core");
    let replayed = profile_send(
        &reopened,
        &fixture.conversation_id,
        &fixture.branch_id,
        ConversationMode::Story,
        &fixture.provider_profile_id,
        GenerationOperationContext::Resume {
            generation_attempt_id: &paused.generation_id,
        },
    )
    .await
    .expect("replay completed ordinary direct send");
    assert_eq!(replayed, paused.generation_id);
    assert_eq!(
        reopened
            .list_branch_messages(&fixture.branch_id)
            .expect("load replayed ordinary direct messages"),
        messages
    );
    assert_eq!(
        reopened
            .get_generation_prompt_plan(&paused.generation_id)
            .expect("reload ordinary direct prompt plan")
            .provider_request
            .request
            .value,
        wire_body
    );
    assert_single_append(
        fixture.root.path(),
        &fixture.conversation_id,
        &fixture.branch_id,
    );
}

enum ExpectedProviderAuthority<'a> {
    Profile(&'a str),
    Target(&'a GenerationTarget),
}

fn assert_fresh_operation_resealed_current_authority(
    root: &std::path::Path,
    old: &PausedAttempt,
    fresh: &PausedAttempt,
    expected: ExpectedProviderAuthority<'_>,
) {
    let storage = Storage::open(root).expect("open provider-authority reseal evidence");
    let old_attempt = storage
        .get_generation_attempt(&old.generation_id)
        .expect("load old provider-authority attempt");
    let fresh_attempt = storage
        .get_generation_attempt(&fresh.generation_id)
        .expect("load fresh provider-authority attempt");
    assert_eq!(
        old_attempt.input.base_request_fingerprint_sha256,
        fresh_attempt.input.base_request_fingerprint_sha256,
        "provider drift and nonce rotation must not alter semantic request identity"
    );
    assert_ne!(old_attempt.generation_id, fresh_attempt.generation_id);
    assert_ne!(
        old_attempt.input.operation_id,
        fresh_attempt.input.operation_id
    );
    assert_eq!(old_attempt.input.operation_id, old.operation_id);
    assert_eq!(fresh_attempt.input.operation_id, fresh.operation_id);
    let old_authority = old_attempt
        .input
        .prompt_selection_authority
        .as_ref()
        .and_then(|authority| authority.provider_target_authority.as_ref())
        .expect("old provider target authority");
    let fresh_authority = fresh_attempt
        .input
        .prompt_selection_authority
        .as_ref()
        .and_then(|authority| authority.provider_target_authority.as_ref())
        .expect("fresh provider target authority");
    match (expected, old_authority, fresh_authority) {
        (
            ExpectedProviderAuthority::Profile(expected_id),
            GenerationProviderTargetAuthority::ProviderProfile {
                provider_profile_id: old_id,
                dispatch_snapshot_sha256: old_snapshot,
            },
            GenerationProviderTargetAuthority::ProviderProfile {
                provider_profile_id: fresh_id,
                dispatch_snapshot_sha256: fresh_snapshot,
            },
        ) => {
            assert_eq!(old_id, expected_id);
            assert_eq!(fresh_id, expected_id);
            assert_ne!(old_snapshot, fresh_snapshot);
        }
        (
            ExpectedProviderAuthority::Target(expected_target),
            GenerationProviderTargetAuthority::GenerationTarget {
                target: old_target,
                resolved_snapshot_sha256: old_snapshot,
            },
            GenerationProviderTargetAuthority::GenerationTarget {
                target: fresh_target,
                resolved_snapshot_sha256: fresh_snapshot,
            },
        ) => {
            assert_eq!(old_target, expected_target);
            assert_eq!(fresh_target, expected_target);
            assert_ne!(old_snapshot, fresh_snapshot);
        }
        (_, old_authority, fresh_authority) => panic!(
            "unexpected provider authority variants: old={old_authority:?}, fresh={fresh_authority:?}"
        ),
    }
}

#[derive(Debug, Clone, Copy)]
enum LegacyProfileDrift {
    Model,
    BaseUrl,
    Timeout,
}

impl LegacyProfileDrift {
    const fn response_delay(self) -> Duration {
        match self {
            Self::Timeout => Duration::from_millis(1_200),
            Self::Model | Self::BaseUrl => Duration::ZERO,
        }
    }

    const fn old_nonce(self) -> &'static str {
        match self {
            Self::Model => "profile-model-old-v1",
            Self::BaseUrl => "profile-base-url-old-v1",
            Self::Timeout => "profile-timeout-old-v1",
        }
    }

    const fn fresh_nonce(self) -> &'static str {
        match self {
            Self::Model => "profile-model-fresh-v2",
            Self::BaseUrl => "profile-base-url-fresh-v2",
            Self::Timeout => "profile-timeout-fresh-v2",
        }
    }
}

fn load_profile(core: &Core, provider_profile_id: &str) -> ProviderProfile {
    core.list_provider_profiles()
        .expect("list provider profiles")
        .into_iter()
        .find(|profile| profile.id == provider_profile_id)
        .expect("selected provider profile")
}

#[allow(
    clippy::too_many_lines,
    reason = "one table-driven flow proves every legacy field across pause, restart, rejection, and fresh dispatch"
)]
async fn run_legacy_profile_authority_drift(drift: LegacyProfileDrift) {
    let primary_provider = StopAwareLoopbackProvider::spawn(drift.response_delay());
    let replacement_provider = matches!(drift, LegacyProfileDrift::BaseUrl)
        .then(|| StopAwareLoopbackProvider::spawn(Duration::ZERO));
    let AuthorityFixture {
        root,
        core,
        conversation_id,
        branch_id,
        target: _,
        credential_authority: _,
        provider_profile_id,
    } = prepare_authority_fixture(&primary_provider.origin);
    assert_eq!(provider_profile_id, PROFILE_ID);

    if matches!(drift, LegacyProfileDrift::Timeout) {
        let mut profile = load_profile(&core, &provider_profile_id);
        profile.timeout_seconds = 1;
        core.upsert_provider_profile(profile)
            .expect("seal one-second legacy profile timeout");
    }

    let old = begin_profile_authority_attempt(
        &core,
        root.path(),
        &conversation_id,
        &branch_id,
        &provider_profile_id,
        drift.old_nonce(),
    )
    .await;
    assert_only_old_authority_attempt(
        root.path(),
        &conversation_id,
        &branch_id,
        &old.generation_id,
    );
    primary_provider.assert_no_request("before legacy profile drift");
    if let Some(provider) = replacement_provider.as_ref() {
        provider.assert_no_request("before legacy endpoint drift");
    }

    match drift {
        LegacyProfileDrift::Model => {
            let mut profile = load_profile(&core, &provider_profile_id);
            "synthetic-ordinary-direct-model-current".clone_into(&mut profile.model);
            core.upsert_provider_profile(profile)
                .expect("change legacy profile model");
        }
        LegacyProfileDrift::BaseUrl => {
            let replacement = replacement_provider
                .as_ref()
                .expect("replacement endpoint provider");
            assert_eq!(
                Connection::open(active_database_path(root.path()))
                    .expect("open legacy endpoint drift seam")
                    .execute(
                        "UPDATE provider_profiles SET base_url = ?1 WHERE id = ?2",
                        params![format!("{}/v1", replacement.origin.as_str()), PROFILE_ID],
                    )
                    .expect("change legacy profile endpoint through synthetic seam"),
                1
            );
        }
        LegacyProfileDrift::Timeout => {
            let mut profile = load_profile(&core, &provider_profile_id);
            profile.timeout_seconds = 3;
            core.upsert_provider_profile(profile)
                .expect("change legacy profile timeout");
        }
    }

    drop(core);
    let core = Core::open(CoreConfig::new(root.path()))
        .expect("restart Core after legacy provider-authority drift");
    let current_profile = load_profile(&core, &provider_profile_id);
    assert_eq!(current_profile.id, provider_profile_id);
    match drift {
        LegacyProfileDrift::Model => assert_eq!(
            current_profile.model,
            "synthetic-ordinary-direct-model-current"
        ),
        LegacyProfileDrift::BaseUrl => assert_eq!(
            current_profile.base_url,
            format!(
                "{}/v1",
                replacement_provider
                    .as_ref()
                    .expect("replacement endpoint provider")
                    .origin
                    .as_str()
            )
        ),
        LegacyProfileDrift::Timeout => assert_eq!(current_profile.timeout_seconds, 3),
    }

    let rejected = profile_send(
        &core,
        &conversation_id,
        &branch_id,
        ConversationMode::Chat,
        &provider_profile_id,
        GenerationOperationContext::Resume {
            generation_attempt_id: &old.generation_id,
        },
    )
    .await
    .expect_err("legacy provider drift must reject exact attempt resume");
    assert_provider_authority_drift_error(&rejected);
    assert_only_old_authority_attempt(
        root.path(),
        &conversation_id,
        &branch_id,
        &old.generation_id,
    );
    assert!(
        core.list_branch_messages(&branch_id)
            .expect("messages after rejected legacy resume")
            .is_empty()
    );
    primary_provider.assert_no_request("after rejected legacy profile resume");
    if let Some(provider) = replacement_provider.as_ref() {
        provider.assert_no_request("after rejected legacy endpoint resume");
    }

    let fresh = begin_profile_authority_attempt(
        &core,
        root.path(),
        &conversation_id,
        &branch_id,
        &provider_profile_id,
        drift.fresh_nonce(),
    )
    .await;
    assert_ne!(fresh.generation_id, old.generation_id);
    assert_ne!(fresh.operation_id, old.operation_id);
    assert_eq!(
        authority_runtime_counts(root.path(), &conversation_id, &branch_id),
        (2, 0, 0)
    );
    primary_provider.assert_no_request("while fresh legacy operation awaits dispatch");
    if let Some(provider) = replacement_provider.as_ref() {
        provider.assert_no_request("while fresh legacy endpoint operation awaits dispatch");
    }

    let dispatched = profile_send(
        &core,
        &conversation_id,
        &branch_id,
        ConversationMode::Chat,
        &provider_profile_id,
        GenerationOperationContext::Resume {
            generation_attempt_id: &fresh.generation_id,
        },
    )
    .await
    .expect("dispatch fresh legacy profile operation");
    assert_eq!(dispatched, fresh.generation_id);
    wait_for_generation(&core, &branch_id, &fresh.generation_id).await;
    let request = match drift {
        LegacyProfileDrift::BaseUrl => {
            primary_provider.assert_no_request("after current endpoint dispatch");
            replacement_provider
                .as_ref()
                .expect("replacement endpoint provider")
                .receive_request("at current legacy endpoint")
        }
        LegacyProfileDrift::Model | LegacyProfileDrift::Timeout => {
            primary_provider.receive_request("with current legacy profile")
        }
    };
    let (_, body) = request_parts(&request);
    let expected_model = match drift {
        LegacyProfileDrift::Model => "synthetic-ordinary-direct-model-current",
        LegacyProfileDrift::BaseUrl | LegacyProfileDrift::Timeout => {
            "synthetic-ordinary-direct-model"
        }
    };
    assert_eq!(
        body.get("model").and_then(serde_json::Value::as_str),
        Some(expected_model)
    );
    primary_provider.assert_no_request("after sole legacy dispatch was consumed");
    if let Some(provider) = replacement_provider.as_ref() {
        provider.assert_no_request("after sole replacement dispatch was consumed");
    }
    assert_fresh_authority_append(root.path(), &conversation_id, &branch_id);
    drop(core);
    assert_fresh_operation_resealed_current_authority(
        root.path(),
        &old,
        &fresh,
        ExpectedProviderAuthority::Profile(&provider_profile_id),
    );
}

#[derive(Debug, Clone, Copy)]
enum ModernTargetDrift {
    Route,
    Connection,
    Preset,
    Capability,
    TemplateRebind,
}

impl ModernTargetDrift {
    const fn response_delay(self) -> Duration {
        match self {
            Self::Connection => Duration::from_millis(1_200),
            Self::Route | Self::Preset | Self::Capability | Self::TemplateRebind => Duration::ZERO,
        }
    }

    const fn old_nonce(self) -> &'static str {
        match self {
            Self::Route => "target-route-old-v1",
            Self::Connection => "target-connection-old-v1",
            Self::Preset => "target-preset-old-v1",
            Self::Capability => "target-capability-old-v1",
            Self::TemplateRebind => "target-template-old-v1",
        }
    }

    const fn fresh_nonce(self) -> &'static str {
        match self {
            Self::Route => "target-route-fresh-v2",
            Self::Connection => "target-connection-fresh-v2",
            Self::Preset => "target-preset-fresh-v2",
            Self::Capability => "target-capability-fresh-v2",
            Self::TemplateRebind => "target-template-fresh-v2",
        }
    }
}

fn load_target_connection(core: &Core) -> ProviderConnection {
    core.list_provider_connections()
        .expect("list target provider connections")
        .into_iter()
        .find(|connection| connection.id.as_str() == CONNECTION_ID)
        .expect("selected target provider connection")
}

fn load_target_route(core: &Core, target: &GenerationTarget) -> ModelRoute {
    core.list_model_routes(&ProviderConnectionId::from(CONNECTION_ID))
        .expect("list target model routes")
        .into_iter()
        .find(|route| route.id == target.model_route_id)
        .expect("selected target model route")
}

fn load_target_preset(core: &Core, target: &GenerationTarget) -> GenerationPreset {
    core.list_generation_presets(&target.model_route_id)
        .expect("list target generation presets")
        .into_iter()
        .find(|preset| preset.id == target.generation_preset_id)
        .expect("selected target generation preset")
}

fn adversarially_rebind_target_template(root: &std::path::Path) {
    let storage = Storage::open(root).expect("open immutable-template rebind seam");
    let connection = storage
        .list_provider_connections()
        .expect("list connections for immutable-template rebind")
        .into_iter()
        .find(|connection| connection.id.as_str() == CONNECTION_ID)
        .expect("target connection for immutable-template rebind");
    let mut template = storage
        .get_provider_template(&connection.template_id, connection.template_version)
        .expect("load original immutable provider template");
    template.id = ProviderTemplateId::from("synthetic-authority-template-rebind");
    "Synthetic adversarial template rebind".clone_into(&mut template.display_name);
    storage
        .save_provider_template(&template)
        .expect("save cloned immutable provider template under a fresh identity");
    drop(storage);
    assert_eq!(
        Connection::open(active_database_path(root))
            .expect("open provider connection template-rebind seam")
            .execute(
                "UPDATE provider_connections
                 SET template_id = ?1, template_version = ?2
                 WHERE id = ?3",
                params![
                    template.id.as_str(),
                    template.manifest_version,
                    CONNECTION_ID
                ],
            )
            .expect("adversarially rebind provider connection template"),
        1
    );
}

#[allow(
    clippy::too_many_lines,
    reason = "one table-driven flow proves every modern authority layer across pause, restart, rejection, and fresh dispatch"
)]
async fn run_modern_target_authority_drift(drift: ModernTargetDrift) {
    let provider = StopAwareLoopbackProvider::spawn(drift.response_delay());
    let AuthorityFixture {
        root,
        core,
        conversation_id,
        branch_id,
        target,
        credential_authority,
        provider_profile_id: _,
    } = prepare_authority_fixture(&provider.origin);
    let caller_target = target.clone();

    if matches!(drift, ModernTargetDrift::Connection) {
        let mut connection = load_target_connection(&core);
        connection.timeout_seconds = 1;
        core.upsert_provider_connection(connection)
            .expect("seal one-second target connection timeout");
    }

    let old = begin_target_authority_attempt(
        &core,
        root.path(),
        &conversation_id,
        &branch_id,
        &target,
        &credential_authority,
        drift.old_nonce(),
    )
    .await;
    assert_only_old_authority_attempt(
        root.path(),
        &conversation_id,
        &branch_id,
        &old.generation_id,
    );
    provider.assert_no_request("before modern target drift");

    let mut core = Some(core);
    match drift {
        ModernTargetDrift::Route => {
            let current = core.as_ref().expect("Core before route drift");
            let mut route = load_target_route(current, &target);
            route.display_name = Some("Synthetic current route display".to_owned());
            current
                .upsert_model_route(route)
                .expect("change mutable target route field");
        }
        ModernTargetDrift::Connection => {
            let current = core.as_ref().expect("Core before connection drift");
            let mut connection = load_target_connection(current);
            connection.timeout_seconds = 3;
            current
                .upsert_provider_connection(connection)
                .expect("change target connection timeout");
        }
        ModernTargetDrift::Preset => {
            let current = core.as_ref().expect("Core before preset drift");
            let mut preset = load_target_preset(current, &target);
            preset.values = vec![ParameterValue {
                parameter_id: ParameterId::from("top_p"),
                state: ParameterValueState::Explicit(ParameterLiteral::Number(0.42)),
            }];
            preset.updated_at = Utc::now();
            current
                .upsert_generation_preset(preset)
                .expect("change target generation preset");
        }
        ModernTargetDrift::Capability => {
            let current = core.as_ref().expect("Core before capability drift");
            let observed_at = Utc::now();
            current
                .record_provider_api_capability_observations(vec![CapabilityObservation {
                    id: ObservationId::from("synthetic-authority-current-context-window"),
                    model_route_id: target.model_route_id.clone(),
                    key: CapabilityKey::ContextWindow,
                    value: CapabilityValue::Integer(65_536),
                    status: SupportStatus::Verified,
                    source: ObservationSource::ProviderApi,
                    confidence: Confidence::High,
                    observed_at,
                    expires_at: Some(observed_at + ChronoDuration::hours(24)),
                    evidence_ref: None,
                }])
                .expect("change target capability observation");
        }
        ModernTargetDrift::TemplateRebind => {
            drop(core.take().expect("Core before immutable-template rebind"));
            adversarially_rebind_target_template(root.path());
        }
    }
    if let Some(current) = core.take() {
        drop(current);
    }

    let core = Core::open(CoreConfig::new(root.path()))
        .expect("restart Core after modern provider-authority drift");
    assert_eq!(target, caller_target);
    match drift {
        ModernTargetDrift::Route => assert_eq!(
            load_target_route(&core, &target).display_name.as_deref(),
            Some("Synthetic current route display")
        ),
        ModernTargetDrift::Connection => {
            assert_eq!(load_target_connection(&core).timeout_seconds, 3);
        }
        ModernTargetDrift::Preset => {
            assert_eq!(
                load_target_preset(&core, &target).values,
                vec![ParameterValue {
                    parameter_id: ParameterId::from("top_p"),
                    state: ParameterValueState::Explicit(ParameterLiteral::Number(0.42)),
                }]
            );
        }
        ModernTargetDrift::Capability => {
            assert!(
                core.list_capability_observations(&target.model_route_id)
                    .expect("list current target capability observations")
                    .iter()
                    .any(|observation| {
                        observation.key == CapabilityKey::ContextWindow
                            && observation.value == CapabilityValue::Integer(65_536)
                    })
            );
        }
        ModernTargetDrift::TemplateRebind => {
            assert_eq!(
                load_target_connection(&core).template_id,
                ProviderTemplateId::from("synthetic-authority-template-rebind")
            );
        }
    }

    let rejected = ordinary_send(
        &core,
        &conversation_id,
        &branch_id,
        ConversationMode::Chat,
        &target,
        &credential_authority,
        GenerationOperationContext::Resume {
            generation_attempt_id: &old.generation_id,
        },
    )
    .await
    .expect_err("modern target drift must reject exact attempt resume");
    assert_provider_authority_drift_error(&rejected);
    assert_only_old_authority_attempt(
        root.path(),
        &conversation_id,
        &branch_id,
        &old.generation_id,
    );
    assert!(
        core.list_branch_messages(&branch_id)
            .expect("messages after rejected modern resume")
            .is_empty()
    );
    provider.assert_no_request("after rejected modern target resume");

    let credential_authority = if matches!(drift, ModernTargetDrift::TemplateRebind) {
        install_provider_credential_authority(&core)
    } else {
        credential_authority
    };

    let fresh = begin_target_authority_attempt(
        &core,
        root.path(),
        &conversation_id,
        &branch_id,
        &target,
        &credential_authority,
        drift.fresh_nonce(),
    )
    .await;
    assert_ne!(fresh.generation_id, old.generation_id);
    assert_ne!(fresh.operation_id, old.operation_id);
    assert_eq!(
        authority_runtime_counts(root.path(), &conversation_id, &branch_id),
        (2, 0, 0)
    );
    provider.assert_no_request("while fresh modern operation awaits dispatch");

    let dispatched = ordinary_send(
        &core,
        &conversation_id,
        &branch_id,
        ConversationMode::Chat,
        &target,
        &credential_authority,
        GenerationOperationContext::Resume {
            generation_attempt_id: &fresh.generation_id,
        },
    )
    .await
    .expect("dispatch fresh modern target operation");
    assert_eq!(dispatched, fresh.generation_id);
    wait_for_generation(&core, &branch_id, &fresh.generation_id).await;
    let request = provider.receive_request("with current modern target");
    let (_, body) = request_parts(&request);
    assert_eq!(
        body.get("model").and_then(serde_json::Value::as_str),
        Some("synthetic-ordinary-reasoning-model")
    );
    if matches!(drift, ModernTargetDrift::Preset) {
        assert_eq!(
            body.get("top_p").and_then(serde_json::Value::as_f64),
            Some(0.42)
        );
    }
    provider.assert_no_request("after sole modern dispatch was consumed");
    assert_fresh_authority_append(root.path(), &conversation_id, &branch_id);
    drop(core);
    assert_fresh_operation_resealed_current_authority(
        root.path(),
        &old,
        &fresh,
        ExpectedProviderAuthority::Target(&target),
    );
}

macro_rules! legacy_profile_authority_drift_test {
    ($name:ident, $drift:expr) => {
        #[tokio::test]
        async fn $name() {
            run_legacy_profile_authority_drift($drift).await;
        }
    };
}

legacy_profile_authority_drift_test!(
    legacy_profile_model_drift_rejects_resume_and_fresh_nonce_dispatches,
    LegacyProfileDrift::Model
);
legacy_profile_authority_drift_test!(
    legacy_profile_base_url_drift_rejects_resume_and_fresh_nonce_dispatches,
    LegacyProfileDrift::BaseUrl
);
legacy_profile_authority_drift_test!(
    legacy_profile_timeout_drift_rejects_resume_and_fresh_nonce_dispatches,
    LegacyProfileDrift::Timeout
);

macro_rules! modern_target_authority_drift_test {
    ($name:ident, $drift:expr) => {
        #[tokio::test]
        async fn $name() {
            run_modern_target_authority_drift($drift).await;
        }
    };
}

modern_target_authority_drift_test!(
    modern_route_drift_rejects_resume_and_fresh_nonce_dispatches,
    ModernTargetDrift::Route
);
modern_target_authority_drift_test!(
    modern_connection_drift_rejects_resume_and_fresh_nonce_dispatches,
    ModernTargetDrift::Connection
);
modern_target_authority_drift_test!(
    modern_preset_drift_rejects_resume_and_fresh_nonce_dispatches,
    ModernTargetDrift::Preset
);
modern_target_authority_drift_test!(
    modern_capability_drift_rejects_resume_and_fresh_nonce_dispatches,
    ModernTargetDrift::Capability
);
modern_target_authority_drift_test!(
    adversarial_template_rebind_rejects_resume_and_fresh_nonce_dispatches,
    ModernTargetDrift::TemplateRebind
);
