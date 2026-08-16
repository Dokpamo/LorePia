//! Production memory-worker acceptance across provider fallback and restart.
//!
//! Every provider in this file is a process-local `127.0.0.1` fixture. The
//! tests use only synthetic conversations and credentials and make no external
//! network request.

use std::{
    future::Future,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use chrono::Utc;
use lorepia_core::{
    ApiFamily, CanonicalOrigin, ConnectionBoundCredential, ConnectionConfigEntry,
    ConnectionConfigValue, Core, CoreConfig, CoreResult, EndpointPath, EnqueueMemorySummaryRequest,
    GenerationOperationContext, GenerationPreset, GenerationPromptCacheSettings,
    GenerationReasoningSettings, GenerationTarget, MessageStatus, ModelAvailability,
    ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId, ProviderConnectionDraft,
    ProviderConnectionId, ProviderNetworkMode, TaskCredentialBroker,
};
use lorepia_domain::{
    AuxiliaryTaskKind, ConversationBranchId, ConversationId, ConversationMode, MemoryJobId,
    MemoryJobKind, MemoryJobStatus, MemoryKind, MemoryProfile, MemoryProfileId, MessageId,
    ModuleScope, PresetMetadata, PromptPreset, PromptPresetId, Provenance, RateLimit, SourceKind,
    SummarySchemaId, TaskProfile, TaskProfileId, TokenBudget, VariableMap,
};
use lorepia_orchestration::default_prompt_preset;
use lorepia_storage::{PromptPresetBinding, PromptResponseLength, Storage};
use rusqlite::Connection;
use tempfile::{NamedTempFile, TempDir, tempdir};

const SOURCE_CREDENTIAL: &str = "synthetic-source-credential-64d1";
const TASK_CREDENTIAL: &str = "synthetic-memory-task-credential-79af";

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

enum ProviderBehavior {
    Complete(String),
    StallAfterPartial(Duration),
}

struct TargetFixture {
    target: GenerationTarget,
    route: ModelRoute,
    connection_id: ProviderConnectionId,
}

#[derive(Default)]
struct BrokerActions {
    invalidate_primary_route: Option<ModelRoute>,
    cancel_on_primary: Option<tokio::sync::watch::Sender<bool>>,
}

struct RecordingBroker {
    core: Core,
    primary_connection_id: ProviderConnectionId,
    actions: Mutex<BrokerActions>,
    calls: Mutex<Vec<ProviderConnectionId>>,
}

impl RecordingBroker {
    fn new(core: &Core, primary_connection_id: ProviderConnectionId) -> Self {
        Self {
            core: core.clone(),
            primary_connection_id,
            actions: Mutex::new(BrokerActions::default()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn invalidating(
        core: &Core,
        primary_connection_id: ProviderConnectionId,
        primary_route: ModelRoute,
    ) -> Self {
        let broker = Self::new(core, primary_connection_id);
        broker
            .actions
            .lock()
            .expect("lock synthetic broker actions")
            .invalidate_primary_route = Some(primary_route);
        broker
    }

    fn cancelling(
        core: &Core,
        primary_connection_id: ProviderConnectionId,
        cancel_sender: tokio::sync::watch::Sender<bool>,
    ) -> Self {
        let broker = Self::new(core, primary_connection_id);
        broker
            .actions
            .lock()
            .expect("lock synthetic broker actions")
            .cancel_on_primary = Some(cancel_sender);
        broker
    }

    fn calls(&self) -> Vec<ProviderConnectionId> {
        self.calls
            .lock()
            .expect("lock synthetic broker calls")
            .clone()
    }
}

impl TaskCredentialBroker for RecordingBroker {
    fn credential_for<'a>(
        &'a self,
        connection_id: &'a ProviderConnectionId,
    ) -> Pin<Box<dyn Future<Output = CoreResult<ConnectionBoundCredential>> + Send + 'a>> {
        Box::pin(async move {
            self.calls
                .lock()
                .expect("record synthetic credential request")
                .push(connection_id.clone());
            if connection_id == &self.primary_connection_id {
                let mut actions = self.actions.lock().expect("lock synthetic broker actions");
                if let Some(mut route) = actions.invalidate_primary_route.take() {
                    route.status = ModelAvailability::MissingTemporarily;
                    self.core.upsert_model_route(route)?;
                }
                if let Some(sender) = actions.cancel_on_primary.take() {
                    let _ = sender.send(true);
                }
            }
            Ok(ConnectionBoundCredential::new(
                connection_id.clone(),
                Some(TASK_CREDENTIAL.to_owned()),
            ))
        })
    }
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set synthetic request timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = stream.read(&mut buffer).expect("read synthetic request");
        if read == 0 {
            break;
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
            break;
        }
    }
    request
}

fn write_completed_response(stream: &mut TcpStream, content: &str) {
    let delta = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": content}}]
    });
    let finish = serde_json::json!({
        "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]
    });
    let usage = serde_json::json!({
        "choices": [],
        "usage": {"prompt_tokens": 11, "completion_tokens": 7}
    });
    let body = format!("data: {delta}\n\ndata: {finish}\n\ndata: {usage}\n\ndata: [DONE]\n\n");
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .expect("write completed synthetic provider response");
}

fn write_partial_then_stall(stream: &mut TcpStream, stall: Duration) {
    let event = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":",
        "{\"content\":\"synthetic partial\"}}]}\n\n"
    );
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:X}\r\n{}\r\n",
        event.len(),
        event
    )
    .expect("write partial synthetic provider response");
    stream.flush().expect("flush partial synthetic response");
    thread::sleep(stall);
}

fn spawn_provider(
    behaviors: Vec<ProviderBehavior>,
) -> (
    CanonicalOrigin,
    mpsc::Receiver<Vec<u8>>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind synthetic provider");
    let address = listener.local_addr().expect("synthetic provider address");
    let (request_sender, request_receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        for behavior in behaviors {
            let (mut stream, _) = listener
                .accept()
                .expect("accept synthetic provider request");
            request_sender
                .send(read_http_request(&mut stream))
                .expect("capture synthetic provider request");
            match behavior {
                ProviderBehavior::Complete(content) => {
                    write_completed_response(&mut stream, &content);
                }
                ProviderBehavior::StallAfterPartial(stall) => {
                    write_partial_then_stall(&mut stream, stall);
                }
            }
        }
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("parse synthetic provider origin"),
        request_receiver,
        handle,
    )
}

fn unused_loopback_origin() -> (TcpListener, CanonicalOrigin) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind unused synthetic provider");
    let address = listener
        .local_addr()
        .expect("unused synthetic provider address");
    let origin = CanonicalOrigin::parse(&format!("http://{address}"))
        .expect("parse unused synthetic provider origin");
    (listener, origin)
}

fn assert_listener_was_not_dispatched(listener: &TcpListener) {
    listener
        .set_nonblocking(true)
        .expect("make unused provider nonblocking");
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        Ok(_) => panic!("a provider request was dispatched to the forbidden fallback"),
        Err(error) => panic!("cannot inspect unused provider listener: {error}"),
    }
}

fn assert_fallback_summary_request(requests: &mpsc::Receiver<Vec<u8>>) {
    let request = requests
        .recv_timeout(Duration::from_secs(3))
        .expect("capture fallback summary request");
    assert!(String::from_utf8_lossy(&request).starts_with("POST /v1/chat/completions HTTP/1.1"));
}

fn import_synthetic_character(core: &Core) -> String {
    let mut source = NamedTempFile::new().expect("temporary synthetic character");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Mira","description":"Entirely synthetic memory worker fixture."}}}}"#
    )
    .expect("write synthetic character");
    let review = core
        .inspect_import(source.path())
        .expect("inspect character");
    core.commit_import(&review.id).expect("commit character").id
}

fn create_loopback_target(core: &Core, origin: &CanonicalOrigin, suffix: &str) -> TargetFixture {
    let template = core
        .list_provider_templates()
        .expect("list provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
        .expect("OpenAI-compatible provider template");
    assert_eq!(template.api_family, ApiFamily::OpenAiChatCompletions);
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(format!("synthetic-memory-{suffix}-connection")),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: format!("Synthetic memory {suffix}"),
            api_origin: origin.clone(),
            api_base_path: Some(EndpointPath::parse("/v1").expect("provider API base path")),
            network_mode: ProviderNetworkMode::LocalLoopback,
            local_network_approval: None,
            values: vec![ConnectionConfigEntry {
                key: "api_base_url".to_owned(),
                value: ConnectionConfigValue::Text(format!("{}/v1", origin.as_str())),
            }],
            approved_credential_origin: Some(origin.clone()),
            timeout_seconds: 5,
        })
        .expect("create synthetic provider connection");
    let now = Utc::now();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from(format!("synthetic-memory-{suffix}-route")),
            connection_id: connection.id.clone(),
            api_family: template.api_family,
            model_id: format!("synthetic-memory-{suffix}-model"),
            display_name: Some(format!("Synthetic memory {suffix}")),
            route_config: ModelRouteConfig::default(),
            status: ModelAvailability::Available,
            miss_count: 0,
            raw_metadata: None,
            metadata_source: ModelMetadataSource::UserOverride,
            metadata_observed_at: None,
            last_reconciled_sync_job_id: None,
            metadata_sync_job_id: None,
            first_seen_at: now,
            last_seen_at: Some(now),
        })
        .expect("save synthetic model route");
    let preset = core
        .upsert_generation_preset(GenerationPreset {
            id: format!("synthetic-memory-{suffix}-preset").into(),
            model_route_id: route.id.clone(),
            display_name: format!("Synthetic memory {suffix} preset"),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: now,
            updated_at: now,
        })
        .expect("save synthetic generation preset");
    TargetFixture {
        target: GenerationTarget {
            model_route_id: route.id.clone(),
            generation_preset_id: preset.id,
        },
        route,
        connection_id: connection.id,
    }
}

fn provenance(id: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some(id.to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic memory acceptance".to_owned()),
        license: Some("LicenseRef-Synthetic".to_owned()),
        imported_at: None,
    }
}

fn memory_prompt_preset(id: &str, memory_profile_id: MemoryProfileId) -> PromptPreset {
    let now = Utc::now();
    let mut preset = default_prompt_preset(
        PromptPresetId::from(id),
        "Synthetic memory runtime preset",
        PresetMetadata {
            description: "Synthetic local-only memory runtime acceptance".to_owned(),
            tags: vec!["synthetic".to_owned(), "memory".to_owned()],
            provenance: provenance(id),
            created_at: now,
            updated_at: now,
            local_override_of: None,
        },
    );
    preset.memory_profile_id = Some(memory_profile_id);
    for block in &mut preset.blocks {
        block.provenance = provenance(block.id.as_str());
    }
    preset
}

fn upsert_embedding_task(
    core: &Core,
    suffix: &str,
    embedding: Option<&TargetFixture>,
) -> Option<(TaskProfileId, String)> {
    embedding.map(|target| {
        let task_id = TaskProfileId::from(format!("synthetic-memory-{suffix}-embedding"));
        let stored = core
            .upsert_task_profile(
                &TaskProfile {
                    id: task_id,
                    kind: AuxiliaryTaskKind::MemoryEmbedding,
                    route_id: target.target.model_route_id.clone(),
                    generation_preset_id: target.target.generation_preset_id.clone(),
                    fallback_route_ids: Vec::new(),
                    embedding_dimensions: Some(3),
                    timeout_ms: 5_000,
                    rate_limit: RateLimit {
                        requests: 100,
                        per_seconds: 60,
                    },
                    concurrency_limit: 1,
                },
                None,
            )
            .expect("save synthetic memory embedding task");
        (
            stored.value.id.clone(),
            stored
                .revision_id
                .clone()
                .expect("embedding task immutable revision"),
        )
    })
}

fn bind_memory_policy(
    core: &Core,
    conversation_id: &ConversationId,
    suffix: &str,
    primary: &TargetFixture,
    fallback: &TargetFixture,
    timeout_ms: u64,
    embedding: Option<&TargetFixture>,
) -> Option<String> {
    let summary_task_id = TaskProfileId::from(format!("synthetic-memory-{suffix}-summary"));
    core.upsert_task_profile(
        &TaskProfile {
            id: summary_task_id.clone(),
            kind: AuxiliaryTaskKind::MemorySummary,
            route_id: primary.target.model_route_id.clone(),
            generation_preset_id: primary.target.generation_preset_id.clone(),
            fallback_route_ids: vec![fallback.target.model_route_id.clone()],
            embedding_dimensions: None,
            timeout_ms,
            rate_limit: RateLimit {
                requests: 100,
                per_seconds: 60,
            },
            concurrency_limit: 1,
        },
        None,
    )
    .expect("save synthetic memory summary task");

    let embedding_task = upsert_embedding_task(core, suffix, embedding);
    let memory_profile_id = MemoryProfileId::from(format!("synthetic-memory-{suffix}-profile"));
    core.upsert_memory_profile(
        &MemoryProfile {
            id: memory_profile_id.clone(),
            name: format!("Synthetic memory {suffix} profile"),
            schema_version: 1,
            summary_task: summary_task_id,
            embedding_task: embedding_task.as_ref().map(|(id, _)| id.clone()),
            turns_per_summary: 1,
            recent_raw_budget: TokenBudget { max_tokens: 1_024 },
            episodic_budget: TokenBudget { max_tokens: 1_024 },
            semantic_budget: TokenBudget { max_tokens: 1_024 },
            retrieval_count: 16,
            recency_weight: 1.0,
            similarity_weight: 1.0,
            importance_weight: 1.0,
            preserve_invalidated_records: true,
            summary_schema: SummarySchemaId::from(format!(
                "synthetic-memory-{suffix}-summary-schema"
            )),
            provenance: provenance(&format!("synthetic-memory-{suffix}-profile")),
        },
        None,
    )
    .expect("save synthetic memory profile");
    let preset = memory_prompt_preset(
        &format!("synthetic-memory-{suffix}-prompt"),
        memory_profile_id,
    );
    core.upsert_prompt_preset(&preset, None)
        .expect("save synthetic memory prompt preset");
    let now = Utc::now();
    core.bind_prompt_preset(
        &PromptPresetBinding {
            id: format!("synthetic-memory-{suffix}-binding"),
            prompt_preset_id: preset.id,
            scope: ModuleScope::Conversation,
            target_id: Some(conversation_id.0.clone()),
            conversation_id: None,
            pinned_revision_id: None,
            priority: 0,
            enabled: true,
            response_length: PromptResponseLength::Balanced,
            creativity: 50,
            reasoning_effort: None,
            memory_enabled: true,
            knowledge_enabled: true,
            variable_overrides: VariableMap::default(),
            generation_preset_override_id: None,
            user_name_override: None,
            author_note: None,
            group_context: None,
            template_slots: Vec::new(),
            created_at: now,
            updated_at: now,
        },
        None,
    )
    .expect("bind synthetic memory prompt preset");
    embedding_task.map(|(_, revision_id)| revision_id)
}

fn wait_for_generation(
    core: &Core,
    branch_id: &ConversationBranchId,
    generation_id: &lorepia_core::GenerationId,
) -> MessageId {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let messages = core
            .list_branch_messages(branch_id)
            .expect("load synthetic source messages");
        if let Some(message) = messages
            .iter()
            .find(|message| message.generation_id.as_ref() == Some(generation_id))
            && message.status != MessageStatus::Pending
        {
            assert_eq!(message.status, MessageStatus::Complete);
            return message.id.clone();
        }
        assert!(
            Instant::now() < deadline,
            "synthetic source generation did not finish"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn create_source_turn(
    core: &Core,
    character_id: &str,
    suffix: &str,
    target: &GenerationTarget,
) -> (ConversationId, ConversationBranchId, MessageId) {
    let conversation = core
        .create_conversation(
            character_id,
            format!("Synthetic memory {suffix}"),
            ConversationMode::Chat,
        )
        .expect("create synthetic memory conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list synthetic memory branches")
        .into_iter()
        .next()
        .expect("synthetic root branch");
    let operation_nonce = format!("memory-source-turn-{suffix}");
    let generation = core
        .send_message_to_branch_with_target(
            &conversation.id,
            &branch.id,
            None,
            ConversationMode::Chat,
            "Synthetic user memory source turn",
            GenerationOperationContext::New {
                operation_nonce: &operation_nonce,
            },
            target,
            Some(SOURCE_CREDENTIAL.to_owned()),
        )
        .expect("send synthetic source turn");
    let head = wait_for_generation(core, &branch.id, &generation);
    (conversation.id, branch.id, head)
}

fn enqueue_summary(
    core: &Core,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    head: &MessageId,
) -> MemoryJobId {
    core.enqueue_memory_summary(&EnqueueMemorySummaryRequest {
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
        expected_head: Some(head.clone()),
    })
    .expect("enqueue synthetic memory summary")
    .job
    .value
    .id
}

fn queued_embedding_job_id(root: &TempDir, conversation_id: &ConversationId) -> MemoryJobId {
    let connection = Connection::open(active_database_path(root.path()))
        .expect("open synthetic Core database read-only seam");
    let mut statement = connection
        .prepare(
            "SELECT id FROM memory_jobs
             WHERE conversation_id = ?1 AND job_kind = 'embedding'
             ORDER BY created_at, id",
        )
        .expect("prepare embedding job lookup");
    let ids = statement
        .query_map([&conversation_id.0], |row| row.get::<_, String>(0))
        .expect("query embedding jobs")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect embedding jobs");
    assert_eq!(
        ids.len(),
        1,
        "summary completion must enqueue one embedding"
    );
    MemoryJobId::from(ids[0].clone())
}

fn assert_queued_embedding_job(
    root: &TempDir,
    core: &Core,
    conversation_id: &ConversationId,
    head: &MessageId,
    expected_revision_id: &str,
) -> MemoryJobId {
    let embedding_job_id = queued_embedding_job_id(root, conversation_id);
    let embedding_job = core
        .get_memory_job(&embedding_job_id)
        .expect("load atomically enqueued embedding job");
    assert_eq!(embedding_job.value.kind, MemoryJobKind::Embedding);
    assert_eq!(embedding_job.value.status, MemoryJobStatus::Queued);
    assert_eq!(&embedding_job.value.source_end_message_id, head);
    let queued_revision_id = Connection::open(active_database_path(root.path()))
        .expect("open embedding revision evidence")
        .query_row(
            "SELECT task_profile_revision_id FROM memory_jobs WHERE id = ?1",
            [&embedding_job_id.0],
            |row| row.get::<_, String>(0),
        )
        .expect("load exact embedding task revision");
    assert_eq!(queued_revision_id, expected_revision_id);
    embedding_job_id
}

fn assert_database_excludes(root: &TempDir, needle: &str) {
    let database =
        std::fs::read(active_database_path(root.path())).expect("read synthetic Core database");
    assert!(
        !database
            .windows(needle.len())
            .any(|window| window == needle.as_bytes()),
        "synthetic credential must not be persisted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn before_dispatch_primary_revalidation_falls_back_and_commits_summary_with_embedding() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let summary_json = serde_json::json!({
        "title": "Synthetic durable summary",
        "summary": "The synthetic user completed one local memory source turn.",
        "structured_data": {"turns": 1, "fixture": true},
        "importance": 72,
        "keywords": ["synthetic", "memory"]
    })
    .to_string();
    let (fallback_origin, fallback_requests, fallback_server) = spawn_provider(vec![
        ProviderBehavior::Complete("Synthetic source assistant reply".to_owned()),
        ProviderBehavior::Complete(summary_json),
    ]);
    let (primary_listener, primary_origin) = unused_loopback_origin();
    let fallback = create_loopback_target(&core, &fallback_origin, "fallback-success");
    let primary = create_loopback_target(&core, &primary_origin, "primary-race");
    let (conversation_id, branch_id, head) =
        create_source_turn(&core, &character_id, "fallback-success", &fallback.target);
    fallback_requests
        .recv_timeout(Duration::from_secs(3))
        .expect("capture source provider request");
    let embedding_revision_id = bind_memory_policy(
        &core,
        &conversation_id,
        "fallback-success",
        &primary,
        &fallback,
        5_000,
        Some(&fallback),
    )
    .expect("configured embedding task revision");
    let summary_job_id = enqueue_summary(&core, &conversation_id, &branch_id, &head);
    let broker =
        RecordingBroker::invalidating(&core, primary.connection_id.clone(), primary.route.clone());
    let (_cancel_sender, cancel_receiver) = tokio::sync::watch::channel(false);

    let execution = core
        .execute_next_memory_job(&broker, cancel_receiver)
        .await
        .expect("execute synthetic memory summary")
        .expect("one synthetic memory job");

    assert_eq!(execution.job.value.id, summary_job_id);
    assert_eq!(execution.job.value.kind, MemoryJobKind::Summary);
    assert_eq!(execution.job.value.status, MemoryJobStatus::Succeeded);
    let record = execution.record.expect("durable generated memory record");
    assert_eq!(record.value.kind, MemoryKind::ConversationSummary);
    assert_eq!(record.value.title, "Synthetic durable summary");
    assert_ne!(record.value.source_start_message_id, head);
    assert_eq!(record.value.source_end_message_id, head);
    assert_eq!(
        core.get_memory_record(
            &record.value.conversation_id,
            &record.value.branch_id,
            &record.value.id,
        )
        .expect("reload generated memory record"),
        record
    );
    assert_eq!(
        broker.calls(),
        vec![
            primary.connection_id.clone(),
            fallback.connection_id.clone()
        ],
        "the primary must fail before dispatch and the reviewed fallback must run once"
    );
    assert_fallback_summary_request(&fallback_requests);
    assert_listener_was_not_dispatched(&primary_listener);

    let embedding_job_id = assert_queued_embedding_job(
        &root,
        &core,
        &conversation_id,
        &head,
        &embedding_revision_id,
    );

    drop(broker);
    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen completed memory Core");
    assert_eq!(
        reopened
            .get_memory_record(
                &record.value.conversation_id,
                &record.value.branch_id,
                &record.value.id,
            )
            .expect("reopen durable generated memory"),
        record
    );
    assert_eq!(
        reopened
            .get_memory_job(&embedding_job_id)
            .expect("reopen queued embedding")
            .value
            .status,
        MemoryJobStatus::Queued
    );
    assert_database_excludes(&root, SOURCE_CREDENTIAL);
    assert_database_excludes(&root, TASK_CREDENTIAL);
    fallback_server.join().expect("join fallback provider");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_outcome_never_falls_back_or_requeues_after_restart() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (source_origin, source_requests, source_server) =
        spawn_provider(vec![ProviderBehavior::Complete(
            "Synthetic source assistant reply".to_owned(),
        )]);
    let (primary_origin, primary_requests, primary_server) =
        spawn_provider(vec![ProviderBehavior::StallAfterPartial(
            Duration::from_millis(800),
        )]);
    let (fallback_listener, fallback_origin) = unused_loopback_origin();
    let source = create_loopback_target(&core, &source_origin, "unknown-source");
    let primary = create_loopback_target(&core, &primary_origin, "unknown-primary");
    let fallback = create_loopback_target(&core, &fallback_origin, "unknown-fallback");
    let (conversation_id, branch_id, head) =
        create_source_turn(&core, &character_id, "unknown", &source.target);
    source_requests
        .recv_timeout(Duration::from_secs(3))
        .expect("capture unknown fixture source request");
    assert!(
        bind_memory_policy(
            &core,
            &conversation_id,
            "unknown",
            &primary,
            &fallback,
            300,
            None,
        )
        .is_none()
    );
    let summary_job_id = enqueue_summary(&core, &conversation_id, &branch_id, &head);
    let broker = RecordingBroker::new(&core, primary.connection_id.clone());
    let (_cancel_sender, cancel_receiver) = tokio::sync::watch::channel(false);

    let execution = core
        .execute_next_memory_job(&broker, cancel_receiver)
        .await
        .expect("execute unknown-outcome memory summary")
        .expect("one unknown-outcome memory job");

    primary_requests
        .recv_timeout(Duration::from_secs(3))
        .expect("provider dispatch must reach the synthetic primary");
    assert_eq!(execution.job.value.id, summary_job_id);
    assert_eq!(execution.job.value.status, MemoryJobStatus::Interrupted);
    assert!(execution.job.value.error_code.is_none());
    assert!(execution.record.is_none());
    assert_eq!(broker.calls(), vec![primary.connection_id.clone()]);
    assert_listener_was_not_dispatched(&fallback_listener);
    let interrupted_revision = execution.job.revision;
    drop(broker);
    drop(core);
    let storage = Storage::open(root.path()).expect("open interrupted queue evidence");
    let interrupted_queue = storage
        .get_memory_job_queue_entry(&summary_job_id)
        .expect("load interrupted queue evidence");
    assert_eq!(interrupted_queue.revision, interrupted_revision);
    assert_eq!(interrupted_queue.interruptions.len(), 1);
    assert_eq!(
        interrupted_queue.interruptions[0].error_code.as_deref(),
        Some("provider_unknown_outcome")
    );
    drop(storage);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen interrupted Core");
    let stored = reopened
        .get_memory_job(&summary_job_id)
        .expect("reopen interrupted memory job");
    assert_eq!(stored.value.status, MemoryJobStatus::Interrupted);
    assert!(stored.value.error_code.is_none());
    assert_eq!(stored.revision, interrupted_revision);
    assert!(
        reopened
            .recover_running_memory_jobs()
            .expect("conservative startup recovery")
            .is_empty()
    );
    let restart_broker = RecordingBroker::new(&reopened, primary.connection_id.clone());
    let (_restart_cancel_sender, restart_cancel_receiver) = tokio::sync::watch::channel(false);
    assert!(
        reopened
            .execute_next_memory_job(&restart_broker, restart_cancel_receiver)
            .await
            .expect("poll memory queue after restart")
            .is_none(),
        "interrupted unknown work must require an explicit CAS retry"
    );
    assert!(restart_broker.calls().is_empty());
    assert_listener_was_not_dispatched(&fallback_listener);
    assert_database_excludes(&root, TASK_CREDENTIAL);
    source_server.join().expect("join source provider");
    primary_server.join().expect("join stalling provider");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn known_no_side_effect_cancellation_is_terminal_and_does_not_fall_back() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (source_origin, source_requests, source_server) =
        spawn_provider(vec![ProviderBehavior::Complete(
            "Synthetic source assistant reply".to_owned(),
        )]);
    let (primary_listener, primary_origin) = unused_loopback_origin();
    let (fallback_listener, fallback_origin) = unused_loopback_origin();
    let source = create_loopback_target(&core, &source_origin, "cancel-source");
    let primary = create_loopback_target(&core, &primary_origin, "cancel-primary");
    let fallback = create_loopback_target(&core, &fallback_origin, "cancel-fallback");
    let (conversation_id, branch_id, head) =
        create_source_turn(&core, &character_id, "cancel", &source.target);
    source_requests
        .recv_timeout(Duration::from_secs(3))
        .expect("capture cancellation fixture source request");
    assert!(
        bind_memory_policy(
            &core,
            &conversation_id,
            "cancel",
            &primary,
            &fallback,
            5_000,
            None,
        )
        .is_none()
    );
    let summary_job_id = enqueue_summary(&core, &conversation_id, &branch_id, &head);
    let (cancel_sender, cancel_receiver) = tokio::sync::watch::channel(false);
    let broker = RecordingBroker::cancelling(&core, primary.connection_id.clone(), cancel_sender);

    let execution = core
        .execute_next_memory_job(&broker, cancel_receiver)
        .await
        .expect("cancel memory summary before provider dispatch")
        .expect("one cancelled memory job");

    assert_eq!(execution.job.value.id, summary_job_id);
    assert_eq!(execution.job.value.status, MemoryJobStatus::Cancelled);
    assert!(execution.job.value.error_code.is_none());
    assert!(execution.record.is_none());
    assert_eq!(broker.calls(), vec![primary.connection_id.clone()]);
    assert_listener_was_not_dispatched(&primary_listener);
    assert_listener_was_not_dispatched(&fallback_listener);
    assert_database_excludes(&root, TASK_CREDENTIAL);
    source_server
        .join()
        .expect("join cancellation source provider");
}
