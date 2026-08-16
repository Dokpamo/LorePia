use std::{
    collections::VecDeque,
    fs,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use chrono::Utc;
use lorepia_core::{
    AssistantCallEstimate, AssistantHostAction, AssistantManifestDraft, AssistantToolCall,
    AssistantTurn, ConfidenceLevel, ConnectionConfigValue, Core, CoreConfig, CredentialRef,
    DiscoveryActionId, DiscoveryApprovalGrant, DiscoveryCandidateSummary,
    DiscoveryCompensationKind, DiscoveryCompensationStatus, DiscoveryOperationKind,
    DiscoveryOperationStatus, DiscoveryRecoveryOwner, DiscoverySessionSnapshot, DiscoveryState,
    DraftField, FieldConfidence, FieldEvidenceMapping, HttpUrl, ModelRouteId, ProviderConnection,
    ProviderConnectionId, ProviderDiscoveryAction, ProviderDiscoveryAdditionalEvidence,
    ProviderDiscoveryAssistantResumeAction, ProviderDiscoveryConnectionOptions,
    ProviderDiscoveryCredentialCommitConfirmation, ProviderDiscoveryCredentialInstallContext,
    ProviderDiscoveryCurlInput, ProviderNetworkMode, ProviderTemplateId, SanitizedDiscoveryInput,
    SecretCurlInput, UnresolvedQuestion, provider_discovery_action_envelope,
};
use lorepia_domain::{CanonicalOrigin, EndpointPath, ManifestSource, ManifestSourceKind};
use serde_json::json;
use tempfile::tempdir;

const SECRET_CANARY: &str = "sk-proj-discovery-e2e-canary-7a91";

struct SyntheticProvider {
    origin: String,
    api_base_path: String,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    assistant_responses: Arc<Mutex<VecDeque<Vec<u8>>>>,
    stop: Option<Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl SyntheticProvider {
    fn start() -> Self {
        Self::start_with_base("/v1")
    }

    fn start_with_base(api_base_path: &str) -> Self {
        assert!(api_base_path.starts_with('/'));
        assert!(!api_base_path.ends_with('/'));
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind synthetic provider");
        listener
            .set_nonblocking(true)
            .expect("set synthetic provider nonblocking");
        let origin = format!(
            "http://{}",
            listener.local_addr().expect("synthetic provider address")
        );
        let requests = Arc::new(Mutex::new(Vec::new()));
        let worker_requests = Arc::clone(&requests);
        let assistant_responses = Arc::new(Mutex::new(VecDeque::new()));
        let worker_assistant_responses = Arc::clone(&assistant_responses);
        let worker_origin = origin.clone();
        let worker_api_base_path = api_base_path.to_owned();
        let (stop, stopped) = mpsc::channel();
        let worker = thread::spawn(move || {
            loop {
                if stopped.try_recv().is_ok() {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("set synthetic provider connection blocking");
                        let request = read_request(&mut stream);
                        worker_requests
                            .lock()
                            .expect("synthetic request lock")
                            .push(request.clone());
                        respond(
                            &mut stream,
                            &worker_origin,
                            &worker_api_base_path,
                            &request,
                            &worker_assistant_responses,
                        );
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("accept synthetic provider request: {error}"),
                }
            }
        });
        Self {
            origin,
            api_base_path: api_base_path.to_owned(),
            requests,
            assistant_responses,
            stop: Some(stop),
            worker: Some(worker),
        }
    }

    fn openapi_url(&self) -> HttpUrl {
        HttpUrl::parse(&format!("{}/openapi.json", self.origin)).expect("synthetic OpenAPI URL")
    }

    fn generation_path(&self) -> String {
        format!("{}/chat/completions", self.api_base_path)
    }

    fn captured_requests(&self) -> Vec<Vec<u8>> {
        self.requests
            .lock()
            .expect("synthetic request lock")
            .clone()
    }

    fn queue_assistant_response(&self, response: Vec<u8>) {
        self.assistant_responses
            .lock()
            .expect("synthetic assistant response lock")
            .push_back(response);
    }
}

impl Drop for SyntheticProvider {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            worker.join().expect("join synthetic provider");
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set synthetic request timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("read synthetic request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        let Some(header_end) = find_bytes(&request, b"\r\n\r\n") else {
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
            .unwrap_or_default();
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}

fn respond(
    stream: &mut TcpStream,
    origin: &str,
    api_base_path: &str,
    request: &[u8],
    assistant_responses: &Mutex<VecDeque<Vec<u8>>>,
) {
    let request_line = String::from_utf8_lossy(request)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let models_path = format!("{api_base_path}/models");
    let generation_path = format!("{api_base_path}/chat/completions");
    let (status, content_type, body) = if request_line.contains(" /openapi.json ") {
        (
            "200 OK",
            "application/json",
            synthetic_openapi(origin, api_base_path).into_bytes(),
        )
    } else if request_line.contains(&format!(" {models_path} ")) {
        (
            "200 OK",
            "application/json",
            br#"{"data":[{"id":"synthetic-model","object":"model"}]}"#.to_vec(),
        )
    } else if request_line.contains(" /ambiguous.txt ") {
        (
            "200 OK",
            "text/plain",
            b"Synthetic API documentation. The generation endpoint is not specified.".to_vec(),
        )
    } else if request_line.contains(" /fresh.txt ") {
        (
            "200 OK",
            "text/plain",
            b"Fresh official evidence. Authentication uses the documented HTTP header.".to_vec(),
        )
    } else if request_line.contains(&format!(" {generation_path} "))
        && String::from_utf8_lossy(request).contains("provider setup assistant")
    {
        (
            "200 OK",
            "text/event-stream",
            assistant_responses
                .lock()
                .expect("synthetic assistant response lock")
                .pop_front()
                .unwrap_or_else(assistant_more_evidence_sse),
        )
    } else if request_line.contains(&format!(" {generation_path} ")) {
        (
            "200 OK",
            "text/event-stream",
            concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"reason\",",
                "\"content\":\"ok\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],",
                "\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5,",
                "\"prompt_tokens_details\":{\"cached_tokens\":2},",
                "\"completion_tokens_details\":{\"reasoning_tokens\":1}}}\n\n",
                "data: [DONE]\n\n"
            )
            .as_bytes()
            .to_vec(),
        )
    } else {
        ("404 Not Found", "text/plain", b"not found".to_vec())
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write synthetic response headers");
    stream
        .write_all(&body)
        .expect("write synthetic response body");
}

fn assistant_more_evidence_sse() -> Vec<u8> {
    let turn = json!({
        "turn": {
            "type": "need_more_evidence",
            "questions": [{
                "id": "need-current-endpoint",
                "field": null,
                "question": "Provide one more current official endpoint excerpt.",
                "required_evidence": "A bounded official document excerpt from the approved origin."
            }]
        }
    })
    .to_string();
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "content": turn
            }
        }]
    });
    let finished = json!({
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 32,
            "completion_tokens": 24,
            "total_tokens": 56
        }
    });
    format!("data: {delta}\n\ndata: {finished}\n\ndata: [DONE]\n\n").into_bytes()
}

fn assistant_turn_sse(turn: &AssistantTurn) -> Vec<u8> {
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "content": serde_json::to_string(&json!({"turn": turn}))
                    .expect("serialize assistant turn envelope")
            }
        }]
    });
    let finished = json!({
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 128,
            "completion_tokens": 256,
            "total_tokens": 384
        }
    });
    format!("data: {delta}\n\ndata: {finished}\n\ndata: [DONE]\n\n").into_bytes()
}

fn bare_assistant_turn_sse(turn: &AssistantTurn) -> Vec<u8> {
    let delta = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "content": serde_json::to_string(turn).expect("serialize legacy bare turn")
            }
        }]
    });
    let finished = json!({
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 32,
            "completion_tokens": 24,
            "total_tokens": 56
        }
    });
    format!("data: {delta}\n\ndata: {finished}\n\ndata: [DONE]\n\n").into_bytes()
}

fn assistant_credential_reflection_sse(credential: &str) -> Vec<u8> {
    let split = credential.len() / 2;
    let mut deltas = String::new();
    for content in [&credential[..split], &credential[split..]] {
        let event = json!({
            "choices": [{
                "index": 0,
                "delta": {"content": content}
            }]
        });
        deltas.push_str("data: ");
        deltas.push_str(&event.to_string());
        deltas.push_str("\n\n");
    }
    let finished = json!({
        "choices": [{
            "index": 0,
            "delta": {},
            "finish_reason": "stop"
        }]
    });
    format!("{deltas}data: {finished}\n\ndata: [DONE]\n\n").into_bytes()
}

fn claim_bound_assistant_draft(
    core: &Core,
    provider: &SyntheticProvider,
    session_id: &lorepia_core::DiscoverySessionId,
) -> AssistantTurn {
    let evidence = core
        .list_provider_discovery_evidence(session_id)
        .expect("list assistant draft evidence")
        .into_iter()
        .next()
        .expect("structural OpenAPI-derived evidence");
    let mut manifest = core
        .list_provider_templates()
        .expect("list provider templates")
        .into_iter()
        .find(|template| {
            template.api_family == lorepia_core::ApiFamily::OpenAiChatCompletions
                && template.id.as_str() == "openai-chat-compatible-v1"
        })
        .expect("compiled OpenAI chat adapter")
        .default_manifest;
    manifest.default_api_origin =
        Some(CanonicalOrigin::parse(&provider.origin).expect("target provider origin"));
    manifest.endpoints.generate.path =
        EndpointPath::parse(&provider.generation_path()).expect("target generation endpoint");
    manifest
        .endpoints
        .models
        .as_mut()
        .expect("target models endpoint")
        .path = EndpointPath::parse(&format!("{}/models", provider.api_base_path))
        .expect("target models endpoint path");
    manifest.endpoints.embeddings = None;
    manifest.sources = vec![ManifestSource {
        kind: ManifestSourceKind::OfficialDocumentation,
        url: evidence.source_url,
        content_sha256: Some(evidence.content_sha256),
    }];
    manifest.parameters.clear();

    let fields = [
        DraftField::ApiFamily,
        DraftField::DefaultApiOrigin,
        DraftField::Auth,
        DraftField::GenerateEndpoint,
        DraftField::ModelsEndpoint,
        DraftField::ResponseDecoder,
        DraftField::StreamingDecoder,
    ];
    AssistantTurn::SubmitDraft {
        draft: Box::new(AssistantManifestDraft {
            manifest,
            evidence_mappings: fields
                .iter()
                .cloned()
                .map(|field| FieldEvidenceMapping {
                    field,
                    evidence_ids: vec![evidence.id.clone()],
                    explanation:
                        "Exact deterministic extraction from the approved OpenAPI evidence."
                            .to_owned(),
                })
                .collect(),
            conflicts: Vec::new(),
            unresolved_questions: Vec::new(),
            confidence: fields
                .into_iter()
                .map(|field| FieldConfidence {
                    field,
                    level: ConfidenceLevel::High,
                    rationale:
                        "The value exactly matches the claim emitted by deterministic extraction."
                            .to_owned(),
                })
                .collect(),
            summary: "Claim-bound OpenAI-compatible manifest draft.".to_owned(),
        }),
    }
}

fn synthetic_openapi(origin: &str, api_base_path: &str) -> String {
    json!({
        "openapi": "3.1.0",
        "servers": [{"url": format!("{origin}{api_base_path}")}],
        "components": {
            "securitySchemes": {
                "BearerAuth": {
                    "type": "http",
                    "scheme": "bearer"
                }
            },
            "schemas": {
                "ChatRequest": {
                    "type": "object",
                    "properties": {
                        "model": {"type": "string"},
                        "messages": {"type": "array"},
                        "stream": {"type": "boolean"}
                    }
                }
            }
        },
        "security": [{"BearerAuth": []}],
        "paths": {
            "/models": {
                "get": {
                    "operationId": "listModels",
                    "responses": {
                        "200": {
                            "description": "synthetic model list",
                            "content": {
                                "application/json": {
                                    "schema": {"type": "object"}
                                }
                            }
                        }
                    }
                }
            },
            "/chat/completions": {
                "post": {
                    "operationId": "createChatCompletion",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {"$ref": "#/components/schemas/ChatRequest"}
                            }
                        }
                    },
                    "responses": {
                        "200": {
                            "description": "synthetic event stream",
                            "content": {
                                "text/event-stream": {
                                    "schema": {"type": "string"}
                                }
                            }
                        }
                    }
                }
            }
        }
    })
    .to_string()
}

fn discovery_input(provider: &SyntheticProvider, connection_id: &str) -> SanitizedDiscoveryInput {
    SanitizedDiscoveryInput {
        connection_id: ProviderConnectionId::from(connection_id),
        display_name: format!("Synthetic {connection_id}"),
        site_url: provider.openapi_url(),
        docs_url: Some(provider.openapi_url()),
        credential_ref: Some(CredentialRef(connection_id.to_owned())),
        preferred_assistant: None,
        connection_options: ProviderDiscoveryConnectionOptions {
            values: Vec::new(),
            api_base_path: None,
            timeout_seconds: 5,
            network_mode: ProviderNetworkMode::LocalLoopback,
            local_network_approval: None,
            local_network_approved_at: None,
        },
        supplied_evidence_ids: Vec::new(),
    }
}

fn curl_discovery_input(
    provider: &SyntheticProvider,
    connection_id: &str,
) -> ProviderDiscoveryCurlInput {
    let input = discovery_input(provider, connection_id);
    ProviderDiscoveryCurlInput {
        connection_id: input.connection_id,
        display_name: input.display_name,
        docs_url: input.docs_url,
        credential_ref: input.credential_ref,
        preferred_assistant: input.preferred_assistant,
        connection_options: input.connection_options,
        supplied_evidence_ids: input.supplied_evidence_ids,
    }
}

fn evidence_starved_input(
    provider: &SyntheticProvider,
    connection_id: &str,
) -> SanitizedDiscoveryInput {
    let mut input = discovery_input(provider, connection_id);
    input.site_url =
        HttpUrl::parse(&format!("{}/not-a-provider", provider.origin)).expect("empty fixture URL");
    input.docs_url = None;
    input
}

fn assistant_discovery_input(
    provider: &SyntheticProvider,
    connection_id: &str,
    assistant_route_id: ModelRouteId,
) -> SanitizedDiscoveryInput {
    let mut input = discovery_input(provider, connection_id);
    input.site_url =
        HttpUrl::parse(&format!("{}/ambiguous.txt", provider.origin)).expect("ambiguous docs URL");
    input.docs_url = None;
    input.preferred_assistant = Some(assistant_route_id);
    input
}

fn commit_synthetic_connection(
    core: &Core,
    provider: &SyntheticProvider,
    connection_id: &str,
) -> ProviderConnection {
    let discovered = core
        .begin_provider_discovery_site(discovery_input(provider, connection_id))
        .expect("begin assistant fixture connection discovery");
    let reviewed = approve_to_review(core, &discovered, provider, SECRET_CANARY, false);
    let committing = approve_review(core, &reviewed, provider);
    commit_credential_bound_discovery(core, &committing.session.id)
}

fn commit_credential_bound_discovery(
    core: &Core,
    session_id: &lorepia_core::DiscoverySessionId,
) -> ProviderConnection {
    let context = core
        .get_provider_discovery_credential_install_context(session_id)
        .expect("load exact credential install context");
    let started = reserve_and_start_credential_install(core, context);
    let confirmation = ProviderDiscoveryCredentialCommitConfirmation::try_from(&started)
        .expect("started credential install has exact physical authority");
    core.commit_provider_discovery(session_id, Some(&confirmation))
        .expect("commit exact credential-bound discovery")
}

fn reserve_and_start_credential_install(
    core: &Core,
    context: ProviderDiscoveryCredentialInstallContext,
) -> ProviderDiscoveryCredentialInstallContext {
    match context.operation_status {
        DiscoveryOperationStatus::Prepared => {
            let reserved = if context.native_execution_reservation_id.is_some() {
                context
            } else {
                core.reserve_provider_discovery_credential_install(
                    &context.session_id,
                    context.session_revision,
                    &context.operation_id,
                    &context.commit_attempt_id,
                    &context.commit_plan_sha256,
                )
                .expect("reserve exact credential install execution")
            };
            assert_eq!(
                reserved.operation_status,
                DiscoveryOperationStatus::Prepared
            );
            assert_eq!(reserved.native_execution_id, None);
            assert!(
                ProviderDiscoveryCredentialCommitConfirmation::try_from(&reserved).is_err(),
                "a reservation is not commit confirmation"
            );
            core.start_provider_discovery_credential_install(
                &reserved.session_id,
                reserved.session_revision,
                &reserved.operation_id,
                &reserved.commit_attempt_id,
                &reserved.commit_plan_sha256,
                reserved
                    .native_execution_reservation_id
                    .as_deref()
                    .expect("reserved native execution identifier"),
            )
            .expect("start exact reserved credential install operation")
        }
        DiscoveryOperationStatus::Started => context,
        status => panic!("unexpected credential install status: {status:?}"),
    }
}

fn native_execution_id(context: &ProviderDiscoveryCredentialInstallContext) -> &str {
    context
        .native_execution_id
        .as_deref()
        .expect("started credential install has native execution authority")
}

fn configure_synthetic_assistant(core: &Core, provider: &SyntheticProvider) -> ModelRouteId {
    let assistant_connection =
        commit_synthetic_connection(core, provider, "assistant-provider-connection");
    let assistant_route = core
        .list_model_routes(&assistant_connection.id)
        .expect("list assistant model routes")
        .into_iter()
        .next()
        .expect("assistant model route");
    let assistant_preset = core
        .list_generation_presets(&assistant_route.id)
        .expect("list assistant presets")
        .into_iter()
        .next()
        .expect("assistant generation preset");
    let mut settings = core.get_settings().expect("load settings");
    settings.selected_model_route_id = Some(assistant_route.id.clone());
    settings.selected_generation_preset_id = Some(assistant_preset.id);
    core.update_settings(&settings)
        .expect("select assistant route and preset");
    assistant_route.id
}

fn continue_with(
    core: &Core,
    snapshot: &DiscoverySessionSnapshot,
    action: ProviderDiscoveryAction,
    credential: Option<&str>,
) -> DiscoverySessionSnapshot {
    let envelope = provider_discovery_action_envelope(
        DiscoveryActionId::new(),
        snapshot.session.revision,
        action,
    )
    .expect("build public discovery action");
    core.continue_provider_discovery(&snapshot.session.id, envelope, credential)
        .expect("continue public provider discovery")
}

fn select_known_template(
    core: &Core,
    snapshot: &DiscoverySessionSnapshot,
    template_id: &ProviderTemplateId,
) -> DiscoverySessionSnapshot {
    assert_eq!(
        snapshot.session.state,
        DiscoveryState::AwaitingTemplateSelection
    );
    let candidates = core
        .list_provider_discovery_candidates(&snapshot.session.id)
        .expect("list known-provider candidates");
    let candidate = candidates
        .into_iter()
        .find(|candidate| {
            matches!(
                &candidate.candidate.summary,
                DiscoveryCandidateSummary::ProviderTemplate {
                    template_id: candidate_template_id,
                    ..
                } if candidate_template_id == template_id
            )
        })
        .expect("known provider template candidate");
    continue_with(
        core,
        snapshot,
        ProviderDiscoveryAction::SelectTemplate {
            candidate_id: candidate.candidate.id,
        },
        None,
    )
}

fn select_only_template(
    core: &Core,
    snapshot: &DiscoverySessionSnapshot,
) -> DiscoverySessionSnapshot {
    assert_eq!(
        snapshot.session.state,
        DiscoveryState::AwaitingTemplateSelection
    );
    let candidates = core
        .list_provider_discovery_candidates(&snapshot.session.id)
        .expect("list provider template candidates");
    assert_eq!(
        candidates.len(),
        1,
        "the structural cURL fixture must infer exactly one provider family"
    );
    continue_with(
        core,
        snapshot,
        ProviderDiscoveryAction::SelectTemplate {
            candidate_id: candidates[0].candidate.id.clone(),
        },
        None,
    )
}

fn approve_to_review(
    core: &Core,
    snapshot: &DiscoverySessionSnapshot,
    provider: &SyntheticProvider,
    credential: &str,
    run_probes: bool,
) -> DiscoverySessionSnapshot {
    assert_eq!(
        snapshot.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval,
        "discovery stopped before credential approval: {:?}",
        snapshot.session.failure
    );
    let credential_proposal = core
        .get_provider_discovery_approval_proposal(&snapshot.session.id)
        .expect("load credential approval proposal")
        .expect("credential approval proposal");
    assert!(matches!(
        &credential_proposal.grant,
        DiscoveryApprovalGrant::CredentialOrigin { .. }
    ));
    let listed = continue_with(
        core,
        snapshot,
        ProviderDiscoveryAction::ApproveCredentialOrigin {
            approval_id: credential_proposal.id,
        },
        Some(credential),
    );
    assert_eq!(
        listed.session.state,
        DiscoveryState::AwaitingProbeConsent,
        "model listing failed: {:?}; requests: {:?}",
        listed.session.failure,
        provider
            .captured_requests()
            .iter()
            .map(|request| String::from_utf8_lossy(request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned())
            .collect::<Vec<_>>()
    );
    assert!(
        core.list_provider_discovery_candidates(&listed.session.id)
            .expect("list model candidates")
            .iter()
            .any(|candidate| matches!(
                &candidate.candidate.summary,
                DiscoveryCandidateSummary::ModelRoute { .. }
            )),
        "model listing must produce a durable model-route candidate"
    );

    let reviewed = if run_probes {
        let probe_proposal = core
            .get_provider_discovery_approval_proposal(&listed.session.id)
            .expect("load probe approval proposal")
            .expect("probe approval proposal");
        assert!(matches!(
            &probe_proposal.grant,
            DiscoveryApprovalGrant::CapabilityProbe { .. }
        ));
        continue_with(
            core,
            &listed,
            ProviderDiscoveryAction::ApproveProbes {
                approval_id: probe_proposal.id,
                approval_grant_sha256: probe_proposal.grant_sha256,
            },
            Some(credential),
        )
    } else {
        continue_with(core, &listed, ProviderDiscoveryAction::SkipProbes, None)
    };
    assert_eq!(
        reviewed.session.state,
        DiscoveryState::AwaitingReview,
        "discovery stopped after model listing/probes: {:?}; requests: {:?}",
        reviewed.session.failure,
        provider
            .captured_requests()
            .iter()
            .map(|request| String::from_utf8_lossy(request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned())
            .collect::<Vec<_>>()
    );
    assert!(
        core.get_provider_discovery_review(&reviewed.session.id)
            .expect("load review")
            .is_some()
    );
    reviewed
}

fn approve_review(
    core: &Core,
    snapshot: &DiscoverySessionSnapshot,
    provider: &SyntheticProvider,
) -> DiscoverySessionSnapshot {
    assert_eq!(snapshot.session.state, DiscoveryState::AwaitingReview);
    let proposal = core
        .get_provider_discovery_review_proposal(&snapshot.session.id)
        .expect("load review proposal")
        .expect("review proposal");
    let preview = proposal
        .request_preview
        .as_ref()
        .expect("review includes a structural request preview");
    assert_eq!(preview.path().as_str(), provider.generation_path());
    assert!(preview.query_parameter_names().is_empty());
    assert!(preview.body().is_some());
    continue_with(
        core,
        snapshot,
        ProviderDiscoveryAction::ApproveReview {
            approval_id: proposal.approval.id,
            commit_attempt_id: proposal.commit_attempt_id,
            commit_plan_sha256: proposal.commit_plan_sha256,
            graph_sha256: proposal.review.graph_sha256,
        },
        None,
    )
}

fn inspect_one_shot_curl(core: &Core, provider: &SyntheticProvider) -> String {
    let curl = format!(
        "curl -X POST '{}{}' \
         -H 'Authorization: Bearer {SECRET_CANARY}' \
         -H 'Content-Type: application/json' \
         -d '{{\"model\":\"synthetic-model\",\"messages\":[],\"stream\":true}}'",
        provider.origin,
        provider.generation_path()
    );
    let inspection = core
        .inspect_provider_curl(
            SecretCurlInput::new(curl),
            discovery_input(provider, "curl-inspection").connection_options,
        )
        .expect("inspect one-shot credential-bearing cURL");
    assert_eq!(
        inspection.extracted_credential(),
        Some(SECRET_CANARY.as_bytes())
    );
    assert_no_secret(
        &format!("{:?}", inspection.evidence()),
        "sanitized cURL evidence",
    );
    assert_no_secret(inspection.redacted_curl(), "redacted cURL");
    inspection.redacted_curl().to_owned()
}

fn assert_no_secret(value: &str, surface: &str) {
    assert!(
        !value
            .as_bytes()
            .windows(SECRET_CANARY.len())
            .any(|window| window == SECRET_CANARY.as_bytes()),
        "{surface} retained the secret canary"
    );
}

fn assert_public_surfaces_are_secret_free(core: &Core) {
    let discoveries = core
        .list_provider_discoveries(1_000)
        .expect("list provider discoveries");
    assert_no_secret(&format!("{discoveries:?}"), "discovery snapshots");
    for snapshot in discoveries {
        let session_id = &snapshot.session.id;
        assert_no_secret(
            &format!(
                "{:?}",
                core.list_provider_discovery_candidates(session_id)
                    .expect("list discovery candidates")
            ),
            "discovery candidates",
        );
        assert_no_secret(
            &format!(
                "{:?}",
                core.list_provider_discovery_evidence(session_id)
                    .expect("list discovery evidence")
            ),
            "discovery evidence",
        );
        assert_no_secret(
            &format!(
                "{:?}",
                core.list_provider_discovery_approvals(session_id)
                    .expect("list discovery approvals")
            ),
            "discovery approvals",
        );
        assert_no_secret(
            &format!(
                "{:?}",
                core.get_provider_discovery_review(session_id)
                    .expect("load discovery review")
            ),
            "discovery review",
        );
        assert_no_secret(
            &format!(
                "{:?}",
                core.get_provider_discovery_review_proposal(session_id)
                    .expect("load discovery review proposal")
            ),
            "discovery review proposal",
        );
    }
    let events = core
        .poll_provider_discovery_events(1_000, Utc::now() + chrono::Duration::days(1))
        .expect("poll discovery outbox");
    for event in events {
        assert_no_secret(
            &serde_json::to_string(&event.event).expect("serialize discovery event"),
            "discovery outbox event",
        );
    }
    assert_no_secret(
        &format!(
            "{:?}",
            core.list_provider_connections()
                .expect("list provider connections")
        ),
        "provider connections",
    );
}

fn assert_prompt_bodies_are_secret_free(provider: &SyntheticProvider) {
    let requests = provider.captured_requests();
    for request in requests {
        let body = find_bytes(&request, b"\r\n\r\n")
            .map(|header_end| &request[header_end + 4..])
            .unwrap_or_default();
        assert!(
            !body
                .windows(SECRET_CANARY.len())
                .any(|window| window == SECRET_CANARY.as_bytes()),
            "a provider prompt body retained the secret canary"
        );
    }
}

fn assert_probe_requests_borrow_credentials(provider: &SyntheticProvider) {
    let requests = provider.captured_requests();
    let authorized = requests
        .iter()
        .filter(|request| {
            String::from_utf8_lossy(request)
                .to_ascii_lowercase()
                .contains(&format!(
                    "authorization: bearer {}",
                    SECRET_CANARY.to_ascii_lowercase()
                ))
        })
        .count();
    assert!(
        authorized >= 2,
        "model listing and probes must borrow the credential"
    );
    let probe_requests = requests
        .iter()
        .filter(|request| {
            String::from_utf8_lossy(request)
                .lines()
                .next()
                .is_some_and(|line| {
                    line.starts_with(&format!("POST {} ", provider.generation_path()))
                })
        })
        .count();
    assert!(
        probe_requests >= 3,
        "the approved streaming, reasoning, and prompt-cache probes must reach the provider"
    );
}

fn assert_data_root_is_secret_free(root: &Path) {
    visit_files(root, &mut |path, bytes| {
        assert!(
            !bytes
                .windows(SECRET_CANARY.len())
                .any(|window| window == SECRET_CANARY.as_bytes()),
            "{} retained the secret canary",
            path.display()
        );
    });
}

fn visit_files(root: &Path, visit: &mut impl FnMut(&Path, &[u8])) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return,
        Err(error) => panic!("read {}: {error}", root.display()),
    };
    for entry in entries {
        let entry = entry.expect("read data-root entry");
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => panic!("inspect {}: {error}", path.display()),
        };
        if file_type.is_dir() {
            visit_files(&path, visit);
        } else if file_type.is_file() {
            match fs::read(&path) {
                Ok(bytes) => visit(&path, &bytes),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => panic!("read {}: {error}", path.display()),
            }
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[test]
fn session_scoped_outbox_poll_isolates_fifo_delivery_without_draining_foreign_events() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let begin_offline = |connection_id: &str, template_id: &str| {
        core.begin_provider_discovery_known(
            SanitizedDiscoveryInput {
                connection_id: ProviderConnectionId::from(connection_id),
                display_name: format!("Synthetic {connection_id}"),
                site_url: HttpUrl::parse("https://provider.example/").expect("site URL"),
                docs_url: None,
                credential_ref: None,
                preferred_assistant: None,
                connection_options: ProviderDiscoveryConnectionOptions::default(),
                supplied_evidence_ids: Vec::new(),
            },
            ProviderTemplateId::from(template_id),
        )
        .expect("begin offline discovery")
    };
    let selected = begin_offline("selected-connection", "missing-selected-template");
    let foreign = begin_offline("foreign-connection", "missing-foreign-template");
    let available_at = Utc::now() + chrono::Duration::days(1);

    let selected_first = core
        .poll_provider_discovery_events_for_session(&selected.session.id, 100, available_at)
        .expect("poll selected discovery");
    assert_eq!(selected_first.len(), 1);
    assert_eq!(selected_first[0].event.session_id, selected.session.id);
    assert_eq!(selected_first[0].event.sequence, 1);

    let global = core
        .poll_provider_discovery_events(100, available_at)
        .expect("poll global discovery outbox");
    assert_eq!(global.len(), 2);
    assert!(
        global
            .iter()
            .any(|event| event.event.session_id == foreign.session.id),
        "session-scoped polling must not drain a foreign event"
    );

    assert!(
        core.ack_provider_discovery_event(&selected_first[0].event.id, available_at)
            .expect("ack selected discovery event")
    );
    let selected_next = core
        .poll_provider_discovery_events_for_session(&selected.session.id, 100, available_at)
        .expect("poll selected discovery after acknowledgement");
    assert_eq!(selected_next.len(), 1);
    assert_eq!(selected_next[0].event.session_id, selected.session.id);
    assert_eq!(selected_next[0].event.sequence, 2);

    let foreign_still_pending = core
        .poll_provider_discovery_events_for_session(&foreign.session.id, 100, available_at)
        .expect("poll foreign discovery independently");
    assert_eq!(foreign_still_pending.len(), 1);
    assert_eq!(
        foreign_still_pending[0].event.session_id,
        foreign.session.id
    );
    assert_eq!(foreign_still_pending[0].event.sequence, 1);
}

#[test]
#[allow(clippy::too_many_lines)]
fn unknown_and_known_discovery_approve_probe_review_commit_and_reopen() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start_with_base("/api/v2");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let redacted_curl = inspect_one_shot_curl(&core, &provider);
    let mut curl_input = curl_discovery_input(&provider, "curl-discovery-connection");
    curl_input.connection_options.api_base_path =
        Some(EndpointPath::parse("/api/v2").expect("explicit custom base"));
    let curl_discovery = core
        .begin_provider_discovery_curl(curl_input, SecretCurlInput::new(redacted_curl))
        .expect("begin public cURL discovery");
    assert!(
        curl_discovery
            .session
            .input
            .connection_options
            .values
            .is_empty()
    );
    assert_eq!(
        curl_discovery
            .session
            .input
            .connection_options
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/api/v2")
    );
    let curl_discovery = select_only_template(&core, &curl_discovery);
    assert_eq!(
        curl_discovery.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval,
        "cURL discovery failed: {:?}",
        curl_discovery.session.failure
    );
    let curl_review = approve_to_review(&core, &curl_discovery, &provider, SECRET_CANARY, false);
    let curl_committing = approve_review(&core, &curl_review, &provider);
    let curl_connection = commit_credential_bound_discovery(&core, &curl_discovery.session.id);
    assert_eq!(curl_committing.session.state, DiscoveryState::Committing);
    assert_eq!(curl_connection.api_origin.as_str(), provider.origin);
    assert_eq!(
        curl_connection
            .config
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        None
    );
    assert!(curl_connection.config.values.iter().any(|entry| {
        entry.key == "api_base_url"
            && matches!(
                &entry.value,
                ConnectionConfigValue::Text(value)
                    if value == &provider.origin
            )
    }));

    let unknown = core
        .begin_provider_discovery_site(discovery_input(&provider, "unknown-discovery-connection"))
        .expect("begin unknown site discovery");
    let unknown = if unknown.session.state == DiscoveryState::AwaitingTemplateSelection {
        continue_with(
            &core,
            &unknown,
            ProviderDiscoveryAction::ContinueWithoutTemplate,
            None,
        )
    } else {
        unknown
    };
    assert!(unknown.session.input.connection_options.values.is_empty());
    assert!(
        unknown
            .session
            .input
            .connection_options
            .api_base_path
            .is_none()
    );
    assert_eq!(
        unknown.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval,
        "unknown discovery failed: {:?}; requests: {:?}",
        unknown.session.failure,
        provider
            .captured_requests()
            .iter()
            .map(|request| String::from_utf8_lossy(request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned())
            .collect::<Vec<_>>()
    );
    assert!(
        !core
            .list_provider_discovery_evidence(&unknown.session.id)
            .expect("unknown discovery evidence")
            .is_empty()
    );
    let unknown_review = approve_to_review(&core, &unknown, &provider, SECRET_CANARY, true);
    let unknown_committing = approve_review(&core, &unknown_review, &provider);
    assert_eq!(unknown_committing.session.state, DiscoveryState::Committing);
    let discovered_connection = commit_credential_bound_discovery(&core, &unknown.session.id);
    assert_eq!(
        discovered_connection
            .config
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        None
    );
    assert!(discovered_connection.config.values.iter().any(|entry| {
        entry.key == "api_base_url"
            && matches!(
                &entry.value,
                ConnectionConfigValue::Text(value)
                    if value == &provider.origin
            )
    }));
    assert_eq!(
        core.get_provider_discovery(&unknown.session.id)
            .expect("load committed unknown discovery")
            .session
            .state,
        DiscoveryState::Ready
    );
    assert_eq!(
        core.list_provider_discovery_approvals(&unknown.session.id)
            .expect("unknown discovery approvals")
            .len(),
        3
    );
    assert!(
        !core
            .list_model_routes(&discovered_connection.id)
            .expect("unknown discovery model routes")
            .is_empty()
    );
    assert_public_surfaces_are_secret_free(&core);
    assert_prompt_bodies_are_secret_free(&provider);
    assert_probe_requests_borrow_credentials(&provider);
    assert_data_root_is_secret_free(root.path());
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen after unknown commit");
    assert_eq!(
        reopened
            .get_provider_discovery(&unknown.session.id)
            .expect("reopen unknown discovery")
            .session
            .state,
        DiscoveryState::Ready
    );
    assert!(
        reopened
            .list_provider_templates()
            .expect("list templates after unknown commit")
            .iter()
            .any(|template| template.id == discovered_connection.template_id)
    );

    let known = reopened
        .begin_provider_discovery_known(
            discovery_input(&provider, "known-discovery-connection"),
            discovered_connection.template_id.clone(),
        )
        .expect("begin known provider discovery");
    let known = select_known_template(&reopened, &known, &discovered_connection.template_id);
    let known_review = approve_to_review(&reopened, &known, &provider, SECRET_CANARY, true);
    let known_committing = approve_review(&reopened, &known_review, &provider);
    assert_eq!(known_committing.session.state, DiscoveryState::Committing);
    let known_session_id = known.session.id.clone();
    drop(reopened);

    let recovered = Core::open(CoreConfig::new(root.path())).expect("reopen interrupted commit");
    let interrupted = recovered
        .get_provider_discovery(&known_session_id)
        .expect("load interrupted known discovery");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    assert_eq!(
        interrupted
            .session
            .recovery
            .as_ref()
            .map(|checkpoint| checkpoint.operation),
        Some(DiscoveryOperationKind::AtomicCommit)
    );
    let restarted = continue_with(
        &recovered,
        &interrupted,
        ProviderDiscoveryAction::RestartInterrupted,
        None,
    );
    assert_eq!(restarted.session.state, DiscoveryState::Committing);
    let known_connection = commit_credential_bound_discovery(&recovered, &known_session_id);
    assert_eq!(
        recovered
            .list_provider_discovery_approvals(&known_session_id)
            .expect("known discovery approvals")
            .len(),
        4
    );
    assert!(
        !recovered
            .list_model_routes(&known_connection.id)
            .expect("known discovery model routes")
            .is_empty()
    );
    assert_public_surfaces_are_secret_free(&recovered);
    assert_prompt_bodies_are_secret_free(&provider);
    assert_probe_requests_borrow_credentials(&provider);
    assert_data_root_is_secret_free(root.path());
    drop(recovered);

    let final_reopen = Core::open(CoreConfig::new(root.path())).expect("final Core reopen");
    assert_eq!(
        final_reopen
            .get_provider_discovery(&known_session_id)
            .expect("load final known discovery")
            .session
            .state,
        DiscoveryState::Ready
    );
    assert!(
        final_reopen
            .list_provider_connections()
            .expect("list final provider connections")
            .iter()
            .any(|connection| connection.id == known_connection.id)
    );
    assert_public_surfaces_are_secret_free(&final_reopen);
    assert_data_root_is_secret_free(root.path());
    drop(final_reopen);
    assert_data_root_is_secret_free(root.path());
}

#[test]
#[allow(clippy::too_many_lines)]
fn native_owned_startup_preserves_credential_wal_until_vault_reconciliation() {
    #[derive(Clone, Copy)]
    enum WalState {
        Prepared,
        Started,
    }

    #[derive(Clone, Copy)]
    enum VaultState {
        Missing,
        Available,
    }

    let provider = SyntheticProvider::start();
    let cases = [
        ("prepared-missing", WalState::Prepared, VaultState::Missing),
        (
            "prepared-available",
            WalState::Prepared,
            VaultState::Available,
        ),
        ("started-missing", WalState::Started, VaultState::Missing),
        (
            "started-available",
            WalState::Started,
            VaultState::Available,
        ),
    ];

    for (connection_id, wal_state, vault_state) in cases {
        let root = tempdir().expect("temporary Core root");
        let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
        let started = core
            .begin_provider_discovery_site(discovery_input(&provider, connection_id))
            .expect("begin credential recovery discovery");
        let reviewed = approve_to_review(&core, &started, &provider, SECRET_CANARY, false);
        let _committing = approve_review(&core, &reviewed, &provider);
        let prepared = core
            .get_provider_discovery_credential_install_context(&started.session.id)
            .expect("prepared credential install context");
        if matches!(wal_state, WalState::Started) {
            let durable_started = reserve_and_start_credential_install(&core, prepared.clone());
            assert_eq!(
                durable_started.operation_status,
                DiscoveryOperationStatus::Started
            );
        }
        drop(core);

        let reopened = Core::open_with_discovery_recovery_owner(
            CoreConfig::new(root.path()),
            DiscoveryRecoveryOwner::NativePlatform,
        )
        .expect("open Core for native recovery");
        let preserved = reopened
            .get_provider_discovery(&started.session.id)
            .expect("load preserved discovery");
        assert_eq!(
            preserved.session.state,
            DiscoveryState::Committing,
            "native-owned open must not prematurely classify {connection_id}"
        );
        let recovery_candidates = reopened
            .list_provider_discovery_credential_recovery_candidates()
            .expect("list complete credential recovery candidates");
        assert!(
            recovery_candidates
                .iter()
                .any(|candidate| candidate.session.id == started.session.id),
            "native-owned startup must preserve this credential WAL for vault reconciliation"
        );
        let context = reopened
            .get_provider_discovery_credential_install_context(&started.session.id)
            .expect("load preserved credential install context");
        assert_eq!(
            context.operation_status,
            match wal_state {
                WalState::Prepared => DiscoveryOperationStatus::Prepared,
                WalState::Started => DiscoveryOperationStatus::Started,
            }
        );

        let terminal = match (wal_state, vault_state) {
            (WalState::Started, VaultState::Available) => {
                let confirmation =
                    ProviderDiscoveryCredentialCommitConfirmation::try_from(&context)
                        .expect("started recovery context has physical authority");
                reopened
                    .commit_provider_discovery(&started.session.id, Some(&confirmation))
                    .expect("resume exact started credential commit");
                reopened
                    .get_provider_discovery(&started.session.id)
                    .expect("load resumed discovery")
            }
            (WalState::Started, VaultState::Missing) => reopened
                .attest_provider_discovery_credential_install_no_effect(
                    &started.session.id,
                    &context.operation_id,
                    &context.commit_attempt_id,
                    &context.commit_plan_sha256,
                    native_execution_id(&context),
                )
                .expect("attest exact missing credential slot"),
            (WalState::Prepared, VaultState::Missing | VaultState::Available) => {
                reopened
                    .recover_provider_discovery(Utc::now())
                    .expect("conservatively recover prepared operation");
                reopened
                    .get_provider_discovery(&started.session.id)
                    .expect("load interrupted prepared discovery")
            }
        };
        let expected_state = match (wal_state, vault_state) {
            (WalState::Started, VaultState::Available) => DiscoveryState::Ready,
            _ => DiscoveryState::Interrupted,
        };
        assert_eq!(terminal.session.state, expected_state);
        assert_ne!(terminal.session.state, DiscoveryState::UnknownOutcome);
        assert!(
            reopened
                .list_provider_discovery_credential_recovery_candidates()
                .expect("list reconciled credential recovery candidates")
                .iter()
                .all(|candidate| candidate.session.id != started.session.id)
        );
        if !matches!(expected_state, DiscoveryState::Ready) {
            assert!(
                reopened
                    .list_provider_connections()
                    .expect("list provider connections")
                    .iter()
                    .all(|connection| connection.id != started.session.input.connection_id)
            );
        }
        drop(reopened);

        let final_reopen =
            Core::open(CoreConfig::new(root.path())).expect("reopen reconciled Core");
        assert_eq!(
            final_reopen
                .get_provider_discovery(&started.session.id)
                .expect("load reconciled discovery")
                .session
                .state,
            expected_state
        );
    }
}

#[test]
fn core_owned_startup_conservatively_classifies_started_credential_wal() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let started = core
        .begin_provider_discovery_site(discovery_input(&provider, "core-owned-started-credential"))
        .expect("begin credential recovery discovery");
    let reviewed = approve_to_review(&core, &started, &provider, SECRET_CANARY, false);
    let _committing = approve_review(&core, &reviewed, &provider);
    let prepared = core
        .get_provider_discovery_credential_install_context(&started.session.id)
        .expect("prepared credential install context");
    reserve_and_start_credential_install(&core, prepared);
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen Core-owned recovery");
    let recovered = reopened
        .get_provider_discovery(&started.session.id)
        .expect("load recovered discovery");
    assert_eq!(recovered.session.state, DiscoveryState::UnknownOutcome);
    assert_eq!(
        recovered.session.unknown_operation,
        Some(DiscoveryOperationKind::AtomicCommit)
    );
    assert!(
        reopened
            .list_provider_connections()
            .expect("list provider connections")
            .iter()
            .all(|connection| connection.id != started.session.input.connection_id)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn cancelled_commit_reopens_and_completes_explicit_compensation_restart() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let started = core
        .begin_provider_discovery_site(discovery_input(&provider, "cancelled-discovery-connection"))
        .expect("begin cancellable discovery");
    let review = approve_to_review(&core, &started, &provider, SECRET_CANARY, true);
    let committing = approve_review(&core, &review, &provider);
    assert_eq!(committing.session.state, DiscoveryState::Committing);

    let interrupted = core
        .cancel_provider_discovery(&started.session.id, committing.session.revision)
        .expect("cancel prepared commit");
    assert_eq!(interrupted.session.state, DiscoveryState::Interrupted);
    assert!(interrupted.session.cancellation_pending);
    assert_eq!(
        interrupted
            .session
            .recovery
            .as_ref()
            .map(|checkpoint| checkpoint.operation),
        Some(DiscoveryOperationKind::Compensation)
    );
    assert!(
        core.list_provider_connections()
            .expect("list connections before compensation")
            .iter()
            .all(|connection| connection.id != started.session.input.connection_id)
    );
    assert_data_root_is_secret_free(root.path());
    drop(core);

    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen cancelled commit");
    let persisted = reopened
        .get_provider_discovery(&started.session.id)
        .expect("load cancelled commit");
    assert_eq!(persisted.session.state, DiscoveryState::Interrupted);
    let compensating = continue_with(
        &reopened,
        &persisted,
        ProviderDiscoveryAction::RestartInterrupted,
        None,
    );
    assert_eq!(compensating.session.state, DiscoveryState::Compensating);
    let awaiting_native = reopened
        .continue_provider_discovery_compensation(&started.session.id)
        .expect("run Core-owned compensation steps");
    assert_eq!(awaiting_native.session.state, DiscoveryState::Compensating);
    let attempt_id = awaiting_native
        .session
        .commit_attempt_id
        .as_ref()
        .expect("compensation commit attempt");
    let steps = reopened
        .list_provider_discovery_compensation_steps(attempt_id)
        .expect("list compensation steps");
    let credential_step = steps
        .iter()
        .find(|step| step.kind == DiscoveryCompensationKind::RemoveCredentialSlot)
        .expect("native credential compensation step");
    let started_step = reopened
        .start_provider_discovery_credential_compensation(&started.session.id, &credential_step.id)
        .expect("start native credential compensation");
    assert_eq!(started_step.status, DiscoveryCompensationStatus::InProgress);
    let cancelled = reopened
        .complete_provider_discovery_credential_compensation(
            &started.session.id,
            &credential_step.id,
        )
        .expect("complete native credential compensation");
    assert_eq!(cancelled.session.state, DiscoveryState::Cancelled);
    assert!(!cancelled.session.cancellation_pending);
    assert!(
        reopened
            .list_provider_discovery_compensation_steps(attempt_id)
            .expect("list completed compensation steps")
            .iter()
            .all(|step| step.status == DiscoveryCompensationStatus::Completed)
    );
    assert!(
        reopened
            .list_provider_connections()
            .expect("list connections after compensation")
            .iter()
            .all(|connection| connection.id != started.session.input.connection_id)
    );
    assert_public_surfaces_are_secret_free(&reopened);
    assert_prompt_bodies_are_secret_free(&provider);
    assert_probe_requests_borrow_credentials(&provider);
    assert_data_root_is_secret_free(root.path());
    drop(reopened);

    let final_reopen = Core::open(CoreConfig::new(root.path())).expect("reopen compensated Core");
    assert_eq!(
        final_reopen
            .get_provider_discovery(&started.session.id)
            .expect("load compensated discovery")
            .session
            .state,
        DiscoveryState::Cancelled
    );
    drop(final_reopen);
    assert_data_root_is_secret_free(root.path());
}

#[test]
fn credential_bearing_curl_requires_inspection_before_fresh_evidence_submission() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let awaiting = core
        .begin_provider_discovery_site(evidence_starved_input(
            &provider,
            "fresh-evidence-connection",
        ))
        .expect("begin evidence-starved discovery");
    assert_eq!(awaiting.session.state, DiscoveryState::AwaitingMoreEvidence);
    let evidence_before = core
        .list_provider_discovery_evidence(&awaiting.session.id)
        .expect("list initial evidence");
    let raw_curl = || {
        SecretCurlInput::new(format!(
            "curl -X POST '{}/v1/chat/completions' \
             -H 'Authorization: Bearer {SECRET_CANARY}' \
             -H 'Content-Type: application/json' \
             --data-raw '{{\"model\":\"synthetic-model\",\"messages\":[]}}'",
            provider.origin
        ))
    };

    let begin_error = core
        .begin_provider_discovery_curl(
            curl_discovery_input(&provider, "uninspected-curl-connection"),
            raw_curl(),
        )
        .expect_err("initial raw credential-bearing cURL must fail closed");
    assert_eq!(begin_error.code, lorepia_core::CoreErrorCode::InvalidInput);
    assert!(begin_error.message.contains("inspected first"));
    assert_no_secret(&begin_error.message, "initial credential handoff error");

    let error = core
        .supply_provider_discovery_evidence(
            &awaiting.session.id,
            awaiting.session.revision,
            ProviderDiscoveryAdditionalEvidence::curl(raw_curl()),
        )
        .expect_err("raw credential-bearing cURL must fail closed");
    assert_eq!(error.code, lorepia_core::CoreErrorCode::InvalidInput);
    assert!(error.message.contains("inspected first"));
    assert_no_secret(&error.message, "credential handoff error");
    let unchanged = core
        .get_provider_discovery(&awaiting.session.id)
        .expect("reload unchanged discovery");
    assert_eq!(
        unchanged.session.state,
        DiscoveryState::AwaitingMoreEvidence
    );
    assert_eq!(unchanged.session.revision, awaiting.session.revision);
    assert_eq!(
        core.list_provider_discovery_evidence(&awaiting.session.id)
            .expect("list unchanged evidence"),
        evidence_before
    );

    let inspection = core
        .inspect_provider_curl(
            raw_curl(),
            awaiting.session.input.connection_options.clone(),
        )
        .expect("inspect credential-bearing cURL");
    assert_eq!(
        inspection.extracted_credential(),
        Some(SECRET_CANARY.as_bytes())
    );
    let redacted = inspection.redacted_curl().to_owned();
    drop(inspection);
    let supplied = core
        .supply_provider_discovery_evidence(
            &awaiting.session.id,
            awaiting.session.revision,
            ProviderDiscoveryAdditionalEvidence::curl(SecretCurlInput::new(redacted)),
        )
        .expect("submit inspected redacted cURL");
    assert_eq!(
        supplied.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval
    );
    assert_public_surfaces_are_secret_free(&core);
    assert_data_root_is_secret_free(root.path());
}

#[test]
#[allow(clippy::too_many_lines)]
fn assistant_question_reopens_accepts_fresh_evidence_and_resumes_high_level_turn() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let assistant_route_id = configure_synthetic_assistant(&core, &provider);

    let started = core
        .begin_provider_discovery_site(assistant_discovery_input(
            &provider,
            "assistant-discovery-connection",
            assistant_route_id,
        ))
        .expect("begin assistant discovery");
    let awaiting_consent = if started.session.state == DiscoveryState::AwaitingTemplateSelection {
        continue_with(
            &core,
            &started,
            ProviderDiscoveryAction::ContinueWithoutTemplate,
            None,
        )
    } else {
        started
    };
    assert_eq!(
        awaiting_consent.session.state,
        DiscoveryState::AwaitingAssistantConsent,
        "assistant discovery failed: {:?}; requests: {:?}",
        awaiting_consent.session.failure,
        provider
            .captured_requests()
            .iter()
            .map(|request| String::from_utf8_lossy(request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned())
            .collect::<Vec<_>>()
    );
    let approval = core
        .get_provider_discovery_approval_proposal(&awaiting_consent.session.id)
        .expect("load assistant approval")
        .expect("assistant approval proposal");
    assert!(matches!(
        approval.grant,
        DiscoveryApprovalGrant::AssistantConsent { .. }
    ));
    let ready = continue_with(
        &core,
        &awaiting_consent,
        ProviderDiscoveryAction::ApproveAssistant {
            approval_id: approval.id,
            approval_grant_sha256: approval.grant_sha256,
        },
        None,
    );
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&ready.session.id)
            .expect("load ready assistant boundary")
            .expect("ready assistant boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );

    let session_id = ready.session.id.clone();
    let persisted_options = ready.session.input.connection_options.clone();
    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen ready assistant");
    let reopened_ready = reopened
        .get_provider_discovery(&session_id)
        .expect("reload ready assistant");
    assert_eq!(
        reopened_ready.session.state,
        DiscoveryState::BuildingAssistantManifestDraft
    );
    assert_eq!(
        reopened_ready.session.input.connection_options,
        persisted_options
    );
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load reopened ready boundary")
            .expect("reopened ready boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );

    let estimate = AssistantCallEstimate {
        input_tokens: 128,
        maximum_output_tokens: 256,
        maximum_cost_micro_units: 1_000,
    };
    let first_action = reopened
        .run_provider_discovery_assistant_turn(&session_id, estimate, Some(SECRET_CANARY))
        .expect("run first high-level assistant turn");
    let AssistantHostAction::RequestMoreEvidence { questions, .. } = first_action else {
        panic!("assistant must request fresh evidence");
    };
    assert_eq!(questions.len(), 1);
    let awaiting_evidence = reopened
        .get_provider_discovery(&session_id)
        .expect("load assistant question boundary");
    assert_eq!(
        awaiting_evidence.session.state,
        DiscoveryState::AwaitingMoreEvidence
    );
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load question boundary")
            .expect("question boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence
    );

    drop(reopened);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen assistant question");
    let question_boundary = reopened
        .get_provider_discovery_assistant_resume_boundary(&session_id)
        .expect("reload persisted question boundary")
        .expect("persisted question boundary");
    assert_eq!(
        question_boundary.action,
        ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence
    );
    assert_eq!(question_boundary.questions.len(), 1);
    let awaiting_evidence = reopened
        .get_provider_discovery(&session_id)
        .expect("reload awaiting evidence");
    let resumed = reopened
        .supply_provider_discovery_evidence(
            &session_id,
            awaiting_evidence.session.revision,
            ProviderDiscoveryAdditionalEvidence::document_url(
                HttpUrl::parse(&format!("{}/fresh.txt", provider.origin))
                    .expect("fresh evidence URL"),
            ),
        )
        .expect("supply fresh assistant evidence");
    assert_eq!(
        resumed.session.state,
        DiscoveryState::BuildingAssistantManifestDraft
    );
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load resumed assistant boundary")
            .expect("resumed assistant boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );

    drop(reopened);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen resumed assistant");
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load second ready boundary")
            .expect("second ready boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::RunAssistant
    );
    provider.queue_assistant_response(assistant_turn_sse(&AssistantTurn::CallTool {
        call: AssistantToolCall::ShowUnresolvedQuestions,
    }));
    provider.queue_assistant_response(assistant_turn_sse(&AssistantTurn::NeedMoreEvidence {
        questions: vec![UnresolvedQuestion {
            id: "still-unresolved-after-show".to_owned(),
            field: None,
            question: "Provide one final current official endpoint excerpt.".to_owned(),
            required_evidence: "A different bounded excerpt from the approved origin.".to_owned(),
        }],
    }));
    let second_action = reopened
        .run_provider_discovery_assistant_turn(&session_id, estimate, Some(SECRET_CANARY))
        .expect("run second high-level assistant turn");
    let AssistantHostAction::RequestMoreEvidence { questions, .. } = second_action else {
        panic!("assistant must return the next unresolved evidence boundary");
    };
    assert_eq!(
        questions
            .iter()
            .map(|question| question.id.as_str())
            .collect::<Vec<_>>(),
        vec!["still-unresolved-after-show"]
    );
    let follow_up_boundary = reopened
        .get_provider_discovery_assistant_resume_boundary(&session_id)
        .expect("load follow-up unresolved boundary")
        .expect("follow-up unresolved boundary");
    assert_eq!(
        follow_up_boundary.action,
        ProviderDiscoveryAssistantResumeAction::SupplyMoreEvidence
    );
    assert_eq!(
        follow_up_boundary
            .questions
            .iter()
            .map(|question| question.id.as_str())
            .collect::<Vec<_>>(),
        vec!["still-unresolved-after-show"],
        "the consumed question set must be replaced by the exact follow-up set"
    );
    let assistant_prompt_bodies = provider
        .captured_requests()
        .into_iter()
        .filter_map(|request| {
            let request_line = String::from_utf8_lossy(&request)
                .lines()
                .next()
                .unwrap_or_default()
                .to_owned();
            if !request_line.contains(&format!(" {} ", provider.generation_path())) {
                return None;
            }
            let header_end = find_bytes(&request, b"\r\n\r\n")?;
            Some(String::from_utf8_lossy(&request[header_end + 4..]).into_owned())
        })
        .collect::<Vec<_>>();
    assert!(
        assistant_prompt_bodies.len() >= 3,
        "need-more-evidence, ShowUnresolvedQuestions, and follow-up turns must all run"
    );
    for raw_body in &assistant_prompt_bodies {
        let body: serde_json::Value =
            serde_json::from_str(raw_body).expect("parse captured assistant request body");
        let format = &body["response_format"];
        assert_eq!(format["type"], "json_schema");
        assert_eq!(
            format["json_schema"]["name"],
            "lorepia_setup_assistant_turn_v1"
        );
        assert_eq!(format["json_schema"]["strict"], true);
        let schema = &format["json_schema"]["schema"];
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["turn"]["$ref"],
            "#/$defs/assistant_turn"
        );
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["role"], "user");
        let untrusted_payload = body["messages"][1]["content"]
            .as_str()
            .expect("assistant data-channel payload");
        assert!(untrusted_payload.contains("\"unresolved_questions\""));
        assert!(untrusted_payload.contains("lorepia_setup_assistant_turn_v1"));
    }
    let post_evidence_prompt = &assistant_prompt_bodies[assistant_prompt_bodies.len() - 2];
    assert!(
        [
            "unresolved_questions",
            "need-current-endpoint",
            "Provide one more current official endpoint excerpt.",
            "A bounded official document excerpt from the approved origin.",
        ]
        .iter()
        .all(|expected| post_evidence_prompt.contains(expected)),
        "the post-evidence prompt must preserve the full typed durable unresolved question"
    );
    assert!(
        assistant_prompt_bodies.last().is_some_and(|body| {
            body.contains("question_ids") && body.contains("need-current-endpoint")
        }),
        "the follow-up prompt must contain the typed ShowUnresolvedQuestions result"
    );
    assert_public_surfaces_are_secret_free(&reopened);
    assert_prompt_bodies_are_secret_free(&provider);
    assert_data_root_is_secret_free(root.path());
}

#[test]
#[allow(clippy::too_many_lines)]
fn structural_assistant_draft_is_claim_bound_reviewed_committed_and_reopened() {
    let root = tempdir().expect("temporary Core root");
    let assistant_provider = SyntheticProvider::start();
    let target_provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let assistant_route_id = configure_synthetic_assistant(&core, &assistant_provider);

    let mut input = discovery_input(&target_provider, "assistant-committed-connection");
    input.preferred_assistant = Some(assistant_route_id);
    let awaiting_consent = core
        .begin_provider_discovery_site(input)
        .expect("begin structural assistant discovery");
    assert_eq!(
        awaiting_consent.session.state,
        DiscoveryState::AwaitingAssistantConsent,
        "structural evidence with an explicitly selected assistant must reach consent: {:?}",
        awaiting_consent.session.failure
    );
    assistant_provider.queue_assistant_response(assistant_turn_sse(&claim_bound_assistant_draft(
        &core,
        &target_provider,
        &awaiting_consent.session.id,
    )));

    let approval = core
        .get_provider_discovery_approval_proposal(&awaiting_consent.session.id)
        .expect("load assistant approval")
        .expect("assistant approval proposal");
    let ready = continue_with(
        &core,
        &awaiting_consent,
        ProviderDiscoveryAction::ApproveAssistant {
            approval_id: approval.id,
            approval_grant_sha256: approval.grant_sha256,
        },
        None,
    );
    let session_id = ready.session.id.clone();
    let action = core
        .run_provider_discovery_assistant_turn(
            &session_id,
            AssistantCallEstimate {
                input_tokens: 512,
                maximum_output_tokens: 2_048,
                maximum_cost_micro_units: 10_000,
            },
            Some(SECRET_CANARY),
        )
        .expect("run claim-bound assistant draft");
    let AssistantHostAction::ReviewDraft(review) = action else {
        panic!("assistant must return a claim-bound draft");
    };
    assert!(review.draft.manifest.endpoints.models.is_some());
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("load draft-ready boundary")
            .expect("draft-ready boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::ReviewDraft
    );

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen draft-ready assistant");
    assert_eq!(
        reopened
            .get_provider_discovery_assistant_resume_boundary(&session_id)
            .expect("reload draft-ready boundary")
            .expect("persisted draft-ready boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::ReviewDraft
    );
    let accepted = reopened
        .accept_provider_discovery_assistant_draft(&session_id)
        .expect("accept claim-bound assistant draft");
    assert_eq!(
        accepted.session.state,
        DiscoveryState::AwaitingCredentialOriginApproval
    );
    let reviewed = approve_to_review(&reopened, &accepted, &target_provider, SECRET_CANARY, false);
    let committing = approve_review(&reopened, &reviewed, &target_provider);
    let committed = commit_credential_bound_discovery(&reopened, &session_id);
    assert_eq!(
        committed.id,
        ProviderConnectionId::from("assistant-committed-connection")
    );
    assert_eq!(committed.api_origin.as_str(), target_provider.origin);
    assert!(
        committed.config.api_base_path.is_none(),
        "the assistant-discovered template must own its API base path"
    );
    let committed_template = reopened
        .list_provider_templates()
        .expect("list assistant-discovered templates")
        .into_iter()
        .find(|template| template.id == committed.template_id)
        .expect("assistant-discovered template");
    assert_eq!(
        committed_template
            .default_manifest
            .endpoints
            .generate
            .path
            .as_str(),
        target_provider.generation_path()
    );
    assert_eq!(committing.session.state, DiscoveryState::Committing);

    drop(reopened);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen committed provider");
    let persisted = reopened
        .list_provider_connections()
        .expect("list reopened connections")
        .into_iter()
        .find(|connection| connection.id == committed.id)
        .expect("reopened assistant-discovered connection");
    assert_eq!(persisted, committed);
    assert_public_surfaces_are_secret_free(&reopened);
    assert_prompt_bodies_are_secret_free(&assistant_provider);
    assert_data_root_is_secret_free(root.path());
}

#[test]
fn legacy_bare_assistant_turn_is_rejected_with_explicit_retry_and_no_fallback_request() {
    let root = tempdir().expect("temporary Core root");
    let provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let assistant_route_id = configure_synthetic_assistant(&core, &provider);
    let started = core
        .begin_provider_discovery_site(assistant_discovery_input(
            &provider,
            "assistant-bare-envelope-connection",
            assistant_route_id,
        ))
        .expect("begin assistant bare-envelope discovery");
    let awaiting_consent = if started.session.state == DiscoveryState::AwaitingTemplateSelection {
        continue_with(
            &core,
            &started,
            ProviderDiscoveryAction::ContinueWithoutTemplate,
            None,
        )
    } else {
        started
    };
    assert_eq!(
        awaiting_consent.session.state,
        DiscoveryState::AwaitingAssistantConsent
    );
    let approval = core
        .get_provider_discovery_approval_proposal(&awaiting_consent.session.id)
        .expect("load assistant approval")
        .expect("assistant approval proposal");
    let ready = continue_with(
        &core,
        &awaiting_consent,
        ProviderDiscoveryAction::ApproveAssistant {
            approval_id: approval.id,
            approval_grant_sha256: approval.grant_sha256,
        },
        None,
    );
    provider.queue_assistant_response(bare_assistant_turn_sse(&AssistantTurn::NeedMoreEvidence {
        questions: vec![UnresolvedQuestion {
            id: "legacy-bare-question".to_owned(),
            field: None,
            question: "Which endpoint is current?".to_owned(),
            required_evidence: "A current official endpoint table.".to_owned(),
        }],
    }));
    let generation_path = provider.generation_path();
    let generation_count = || {
        provider
            .captured_requests()
            .iter()
            .filter(|request| {
                String::from_utf8_lossy(request)
                    .lines()
                    .next()
                    .is_some_and(|line| line.contains(&format!(" {generation_path} ")))
            })
            .count()
    };
    let before = generation_count();
    let error = core
        .run_provider_discovery_assistant_turn(
            &ready.session.id,
            AssistantCallEstimate {
                input_tokens: 128,
                maximum_output_tokens: 256,
                maximum_cost_micro_units: 1_000,
            },
            Some(SECRET_CANARY),
        )
        .expect_err("legacy bare turn must not bypass the response envelope");
    assert_eq!(error.code, lorepia_core::CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
    assert_eq!(generation_count(), before + 1);
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&ready.session.id)
            .expect("load invalid-envelope boundary")
            .expect("invalid-envelope retry boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::ApproveRetry
    );
    assert_public_surfaces_are_secret_free(&core);
    assert_prompt_bodies_are_secret_free(&provider);
    drop(core);
    assert_data_root_is_secret_free(root.path());
}

#[test]
fn assistant_response_reflecting_split_credential_fails_closed_without_persistence() {
    let root = tempdir().expect("temporary Core root");
    let assistant_provider = SyntheticProvider::start();
    let target_provider = SyntheticProvider::start();
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let assistant_route_id = configure_synthetic_assistant(&core, &assistant_provider);

    let mut input = discovery_input(&target_provider, "assistant-reflection-connection");
    input.preferred_assistant = Some(assistant_route_id);
    let awaiting_consent = core
        .begin_provider_discovery_site(input)
        .expect("begin assistant reflection discovery");
    assert_eq!(
        awaiting_consent.session.state,
        DiscoveryState::AwaitingAssistantConsent
    );
    let approval = core
        .get_provider_discovery_approval_proposal(&awaiting_consent.session.id)
        .expect("load assistant approval")
        .expect("assistant approval proposal");
    let ready = continue_with(
        &core,
        &awaiting_consent,
        ProviderDiscoveryAction::ApproveAssistant {
            approval_id: approval.id,
            approval_grant_sha256: approval.grant_sha256,
        },
        None,
    );
    assistant_provider.queue_assistant_response(assistant_credential_reflection_sse(SECRET_CANARY));
    let error = core
        .run_provider_discovery_assistant_turn(
            &ready.session.id,
            AssistantCallEstimate {
                input_tokens: 128,
                maximum_output_tokens: 256,
                maximum_cost_micro_units: 1_000,
            },
            Some(SECRET_CANARY),
        )
        .expect_err("credential-reflecting assistant response must fail closed");
    assert_eq!(error.code, lorepia_core::CoreErrorCode::ProviderUnavailable);
    assert!(!format!("{error:?}").contains(SECRET_CANARY));
    let recovered = core
        .get_provider_discovery(&ready.session.id)
        .expect("load assistant after rejected reflection");
    assert_eq!(
        core.get_provider_discovery_assistant_resume_boundary(&ready.session.id)
            .expect("load rejected reflection boundary")
            .expect("rejected reflection boundary")
            .action,
        ProviderDiscoveryAssistantResumeAction::ApproveRetry
    );
    assert!(recovered.review.is_none());
    assert_public_surfaces_are_secret_free(&core);
    assert_prompt_bodies_are_secret_free(&assistant_provider);
    drop(core);
    assert_data_root_is_secret_free(root.path());
}
