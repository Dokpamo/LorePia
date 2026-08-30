mod support;

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
use support::is_live_owner_lock_sharing_violation;
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
                Err(error) if is_live_owner_lock_sharing_violation(&path, &error) => {}
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

include!("../src/provider_discovery/tests/integration/session_flow.rs");
include!("../src/provider_discovery/tests/integration/credential_recovery.rs");
include!("../src/provider_discovery/tests/integration/assistant.rs");
