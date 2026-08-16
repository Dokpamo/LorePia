use std::{
    fs,
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
    ConnectionConfigValue, Core, CoreConfig, CoreError, CoreErrorCode, EndpointPath,
    GenerationOperationContext, GenerationPreset, GenerationPromptCacheSettings,
    GenerationReasoningSettings, GenerationTarget, MessageStatus, ModelAvailability,
    ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId, PromptPlanRequest,
    ProviderConnection, ProviderConnectionDraft, ProviderConnectionId,
    ProviderCredentialObservedStatus, ProviderNetworkMode, ProviderTemplate, TaskCredentialBroker,
    VariableMap,
};
use lorepia_domain::{
    ActivationRule, AuxiliaryTaskKind, BlockSource, ConversationMode, InstructionAuthority,
    KnowledgeBook, KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId, KnowledgePlacement,
    MemoryProfile, MemoryProfileId, MergePolicy, ModuleScope, OverflowPolicy, PlacementZone,
    PresetMetadata, PromptBlock, PromptBlockId, PromptBlockKind, PromptPreset, Provenance,
    RateLimit, RoleHint, SourceKind, SummarySchemaId, TaskProfile, TaskProfileId, TokenBudget,
    TokenPolicy,
};
use lorepia_orchestration::default_prompt_preset;
use lorepia_providers::{AdapterRegistry, EmbeddingPurpose};
use lorepia_storage::{
    KnowledgeEmbeddingWrite, MemoryQueryEmbeddingStatus, PromptPresetBinding, PromptResponseLength,
    Storage,
};
use rusqlite::Connection;
use tempfile::{NamedTempFile, TempDir, tempdir};
use tokio::sync::watch;

const EMBEDDING_CREDENTIAL_CANARY: &str = "sk-synthetic-semantic-embedding-8a51";
const GENERATION_CREDENTIAL_CANARY: &str = "sk-synthetic-semantic-generation-9b62";
const KNOWLEDGE_MARKER: &str = "SYNTHETIC_PROVIDER_VECTOR_ONLY_31AD";

fn active_database_path(root: &Path) -> PathBuf {
    let cutover = root.join("db/schema-cutover");
    let (_, relative) = fs::read_dir(cutover)
        .expect("read committed database generations")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().join("generation-committed.json").is_file())
        .map(|entry| {
            let manifest = serde_json::from_slice::<serde_json::Value>(
                &fs::read(entry.path().join("generation-manifest.json"))
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

#[derive(Clone, Copy)]
enum FixtureResponse {
    Embedding,
    Generation,
}

struct SemanticFixture {
    core: Core,
    request: PromptPlanRequest,
    embedding_connection_id: ProviderConnectionId,
    generation_connection_id: ProviderConnectionId,
}

struct RecordingCredentialBroker {
    expected_connection_id: ProviderConnectionId,
    database_path: PathBuf,
    calls: Mutex<Vec<ProviderConnectionId>>,
    observed_query_states: Mutex<Vec<String>>,
}

impl RecordingCredentialBroker {
    fn new(expected_connection_id: ProviderConnectionId, root: &Path) -> Self {
        Self {
            expected_connection_id,
            database_path: active_database_path(root),
            calls: Mutex::new(Vec::new()),
            observed_query_states: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<ProviderConnectionId> {
        self.calls.lock().expect("credential call lock").clone()
    }

    fn observed_query_states(&self) -> Vec<String> {
        self.observed_query_states
            .lock()
            .expect("observed query state lock")
            .clone()
    }
}

fn assert_single_broker_call(
    broker: &RecordingCredentialBroker,
    expected_connection_id: &ProviderConnectionId,
) {
    let calls = broker.calls();
    assert_eq!(
        calls.as_slice(),
        std::slice::from_ref(expected_connection_id)
    );
}

impl TaskCredentialBroker for RecordingCredentialBroker {
    fn credential_for<'a>(
        &'a self,
        connection_id: &'a ProviderConnectionId,
    ) -> Pin<Box<dyn Future<Output = Result<ConnectionBoundCredential, CoreError>> + Send + 'a>>
    {
        self.calls
            .lock()
            .expect("credential call lock")
            .push(connection_id.clone());
        let query_state = Connection::open(&self.database_path)
            .expect("open durable query evidence database")
            .query_row(
                "SELECT state FROM memory_query_embeddings ORDER BY created_at DESC, id DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("credential access follows durable query claim");
        self.observed_query_states
            .lock()
            .expect("observed query state lock")
            .push(query_state);
        let result = if connection_id == &self.expected_connection_id {
            Ok(ConnectionBoundCredential::new(
                connection_id.clone(),
                Some(EMBEDDING_CREDENTIAL_CANARY.to_owned()),
            ))
        } else {
            Err(CoreError::invalid(
                "semantic embedding requested a credential for the wrong connection",
            ))
        };
        Box::pin(async move { result })
    }
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .expect("set synthetic provider read timeout");
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = stream.read(&mut buffer).expect("read provider request");
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

fn write_fixture_response(stream: &mut TcpStream, response: FixtureResponse) {
    let (content_type, body) = match response {
        FixtureResponse::Embedding => (
            "application/json",
            r#"{"data":[{"embedding":[1.0,0.0,0.0]}],"model":"synthetic-embedding-model"}"#,
        ),
        FixtureResponse::Generation => (
            "text/event-stream",
            concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Synthetic semantic reply\"}}]}\n\n",
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":11,\"completion_tokens\":3}}\n\n",
                "data: [DONE]\n\n"
            ),
        ),
    };
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("write synthetic provider response");
}

fn spawn_success_provider() -> (
    CanonicalOrigin,
    mpsc::Receiver<Vec<u8>>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind semantic fixture provider");
    let address = listener.local_addr().expect("semantic fixture address");
    let (request_sender, request_receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        for response in [FixtureResponse::Embedding, FixtureResponse::Generation] {
            let (mut stream, _) = listener.accept().expect("accept semantic provider request");
            request_sender
                .send(read_http_request(&mut stream))
                .expect("capture semantic provider request");
            write_fixture_response(&mut stream, response);
        }
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("canonical semantic provider origin"),
        request_receiver,
        handle,
    )
}

fn spawn_stalling_provider() -> (
    CanonicalOrigin,
    mpsc::Receiver<Vec<u8>>,
    mpsc::Sender<()>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stalling semantic provider");
    let address = listener.local_addr().expect("stalling semantic address");
    let (request_sender, request_receiver) = mpsc::channel();
    let (release_sender, release_receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept stalled embedding request");
        request_sender
            .send(read_http_request(&mut stream))
            .expect("capture stalled embedding request");
        release_receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("release stalled embedding request");
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("canonical stalling provider origin"),
        request_receiver,
        release_sender,
        handle,
    )
}

fn provenance(source_id: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("a5".repeat(32)),
        author: Some("Synthetic semantic provider test".to_owned()),
        license: Some("LicenseRef-Synthetic-Test".to_owned()),
        imported_at: None,
    }
}

fn import_character_with_greeting(core: &Core, root: &Path) -> String {
    let mut source = NamedTempFile::new_in(root).expect("synthetic character file");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Ari","description":"Synthetic semantic fixture.","first_mes":"Durable greeting anchor."}}}}"#
    )
    .expect("write synthetic character");
    let review = core
        .inspect_import(source.path())
        .expect("inspect character");
    core.commit_import(&review.id).expect("commit character").id
}

fn create_durable_greeting_conversation(
    core: &Core,
    root: &Path,
) -> lorepia_core::ConversationStart {
    let character_id = import_character_with_greeting(core, root);
    let greeting_catalog = core
        .get_character_greeting_catalog(&character_id)
        .expect("load synthetic greeting catalog");
    let greeting = greeting_catalog
        .greetings
        .first()
        .expect("synthetic default greeting");
    core.create_conversation_with_greeting(
        &character_id,
        "Provider semantic vertical",
        ConversationMode::Chat,
        greeting_catalog.character_content_revision_id.as_deref(),
        Some(&greeting.id),
    )
    .expect("create conversation with durable greeting")
}

fn create_connection_and_route(
    core: &Core,
    template: &ProviderTemplate,
    origin: &CanonicalOrigin,
    suffix: &str,
    purpose: &str,
) -> (ProviderConnection, ModelRoute, GenerationPreset) {
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from(format!("semantic-{suffix}-{purpose}-connection")),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: format!("Synthetic {purpose} connection"),
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
        .expect("create semantic provider connection");
    let now = Utc::now();
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from(format!("semantic-{suffix}-{purpose}-route")),
            connection_id: connection.id.clone(),
            api_family: template.api_family,
            model_id: format!("synthetic-{purpose}-model"),
            display_name: Some(format!("Synthetic {purpose} model")),
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
        .expect("save semantic provider route");
    let preset = core
        .upsert_generation_preset(GenerationPreset {
            id: format!("semantic-{suffix}-{purpose}-preset").into(),
            model_route_id: route.id.clone(),
            display_name: format!("Synthetic {purpose} preset"),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: now,
            updated_at: now,
        })
        .expect("save semantic generation preset");
    (connection, route, preset)
}

fn install_generation_credential_authority(core: &Core, connection_id: &ProviderConnectionId) {
    let authority = core
        .propose_provider_credential_install_authority(connection_id)
        .expect("propose synthetic generation credential authority");
    let install = core
        .prepare_provider_credential_install_operation(
            connection_id,
            &authority,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare synthetic generation credential install");
    core.start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
        .expect("start synthetic generation credential install");
    core.finish_provider_credential_operation(
        &install.plan.operation_id,
        &install.plan_sha256,
        ProviderCredentialObservedStatus::Available,
    )
    .expect("finish synthetic generation credential install");
    assert_eq!(
        core.ensure_provider_credential_access_settled(connection_id)
            .expect("read synthetic generation credential authority"),
        authority
    );
}

fn generation_credential(
    core: &Core,
    connection_id: ProviderConnectionId,
) -> ConnectionBoundCredential {
    let authority = core
        .ensure_provider_credential_access_settled(&connection_id)
        .expect("read current generation credential authority");
    ConnectionBoundCredential::new_with_access_authority(
        connection_id,
        Some(GENERATION_CREDENTIAL_CANARY.to_owned()),
        authority,
    )
}

fn create_tasks_and_memory_profile(
    core: &Core,
    suffix: &str,
    generation_route: &ModelRoute,
    generation_preset: &GenerationPreset,
    embedding_route: &ModelRoute,
    embedding_preset: &GenerationPreset,
    timeout_ms: u64,
) -> (MemoryProfile, String) {
    let summary_task_id = TaskProfileId::from(format!("semantic.{suffix}.summary-task"));
    core.upsert_task_profile(
        &TaskProfile {
            id: summary_task_id.clone(),
            kind: AuxiliaryTaskKind::MemorySummary,
            route_id: generation_route.id.clone(),
            generation_preset_id: generation_preset.id.clone(),
            fallback_route_ids: Vec::new(),
            embedding_dimensions: None,
            timeout_ms: 5_000,
            rate_limit: RateLimit {
                requests: 100,
                per_seconds: 60,
            },
            concurrency_limit: 1,
        },
        None,
    )
    .expect("save semantic summary task");
    let embedding_task_id = TaskProfileId::from(format!("semantic.{suffix}.embedding-task"));
    let embedding_task = core
        .upsert_task_profile(
            &TaskProfile {
                id: embedding_task_id.clone(),
                kind: AuxiliaryTaskKind::MemoryEmbedding,
                route_id: embedding_route.id.clone(),
                generation_preset_id: embedding_preset.id.clone(),
                fallback_route_ids: Vec::new(),
                embedding_dimensions: Some(3),
                timeout_ms,
                rate_limit: RateLimit {
                    requests: 100,
                    per_seconds: 60,
                },
                concurrency_limit: 1,
            },
            None,
        )
        .expect("save semantic embedding task");
    let memory_profile_id = MemoryProfileId::from(format!("semantic.{suffix}.memory-profile"));
    let memory_profile = core
        .upsert_memory_profile(
            &MemoryProfile {
                id: memory_profile_id,
                name: "Synthetic provider semantic memory".to_owned(),
                schema_version: 1,
                summary_task: summary_task_id,
                embedding_task: Some(embedding_task_id),
                turns_per_summary: 100,
                recent_raw_budget: TokenBudget { max_tokens: 1_024 },
                episodic_budget: TokenBudget { max_tokens: 1_024 },
                semantic_budget: TokenBudget { max_tokens: 1_024 },
                retrieval_count: 16,
                recency_weight: 1.0,
                similarity_weight: 1.0,
                importance_weight: 1.0,
                preserve_invalidated_records: true,
                summary_schema: SummarySchemaId::from(format!("semantic.{suffix}.summary-schema")),
                provenance: provenance(&format!("semantic.{suffix}.memory-profile")),
            },
            None,
        )
        .expect("save semantic memory profile");
    (
        memory_profile.value,
        embedding_task
            .revision_id
            .expect("semantic embedding task revision"),
    )
}

fn create_knowledge_book(core: &Core, suffix: &str) -> (KnowledgeBookId, KnowledgeEntryId, String) {
    let book_id = KnowledgeBookId::from(format!("semantic.{suffix}.book"));
    let entry_id = KnowledgeEntryId::from(format!("semantic.{suffix}.entry"));
    let book = KnowledgeBook {
        id: book_id.clone(),
        name: "Synthetic provider semantic knowledge".to_owned(),
        schema_version: 1,
        entries: vec![KnowledgeEntry {
            id: entry_id.clone(),
            book_id: book_id.clone(),
            name: "Provider vector only match".to_owned(),
            content: KNOWLEDGE_MARKER.to_owned(),
            enabled: true,
            activation: ActivationRule::Semantic {
                threshold: 0.9,
                top_k: 1,
            },
            priority: 100,
            importance: 100,
            placement: KnowledgePlacement::RetrievedContext,
            token_policy: TokenPolicy {
                priority: 100,
                min_tokens: None,
                max_tokens: Some(64),
                reserve_tokens: None,
            },
            parent_id: None,
            activation_probability_basis_points: 10_000,
            provenance: provenance(&format!("semantic.{suffix}.entry")),
        }],
        scan_depth: 8,
        token_budget: TokenBudget { max_tokens: 128 },
        recursive: false,
        max_recursion_depth: 0,
        provenance: provenance(&format!("semantic.{suffix}.book")),
    };
    let stored = core
        .upsert_knowledge_book(&book, None)
        .expect("save semantic knowledge book");
    (
        book_id,
        entry_id,
        stored
            .revision_id
            .expect("semantic knowledge book revision"),
    )
}

fn bind_semantic_prompt(
    core: &Core,
    suffix: &str,
    conversation_id: &lorepia_core::ConversationId,
    book_id: KnowledgeBookId,
    memory_profile_id: MemoryProfileId,
) -> lorepia_core::PromptPresetId {
    let now = Utc::now();
    let preset_id = lorepia_core::PromptPresetId::from(format!("semantic.{suffix}.prompt"));
    let mut preset: PromptPreset = default_prompt_preset(
        preset_id.clone(),
        "Synthetic provider semantic prompt",
        PresetMetadata {
            description: "Synthetic provider semantic vertical".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: provenance(&format!("semantic.{suffix}.prompt")),
            created_at: now,
            updated_at: now,
            local_override_of: None,
        },
    );
    for block in &mut preset.blocks {
        block.provenance = provenance(block.id.as_str());
    }
    preset.blocks.push(PromptBlock {
        id: PromptBlockId::from(format!("semantic.{suffix}.knowledge-block")),
        name: "Synthetic provider semantic knowledge".to_owned(),
        kind: PromptBlockKind::WorldKnowledge,
        enabled: true,
        role_hint: RoleHint::System,
        authority: InstructionAuthority::Creator,
        template: None,
        condition: None,
        source: BlockSource::SelectedKnowledge,
        placement_zone: PlacementZone::RetrievedContext,
        history_selector: None,
        token_policy: TokenPolicy {
            priority: 1_000,
            min_tokens: None,
            max_tokens: Some(128),
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::ReduceKnowledgeEntries,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance(&format!("semantic.{suffix}.knowledge-block")),
    });
    preset.blocks.sort_by_key(|block| block.placement_zone);
    preset.knowledge_book_ids.push(book_id);
    preset.memory_profile_id = Some(memory_profile_id);
    core.upsert_prompt_preset(&preset, None)
        .expect("save semantic prompt preset");
    core.bind_prompt_preset(
        &PromptPresetBinding {
            id: format!("semantic.{suffix}.binding"),
            prompt_preset_id: preset.id.clone(),
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
    .expect("bind semantic prompt preset");
    preset.id
}

fn create_semantic_fixture(
    root: &TempDir,
    origin: &CanonicalOrigin,
    suffix: &str,
    embedding_timeout_ms: u64,
) -> SemanticFixture {
    let core = Core::open(CoreConfig::new(root.path())).expect("open semantic Core");
    let start = create_durable_greeting_conversation(&core, root.path());
    let durable_head = start.initial_message.expect("durable greeting message").id;
    assert_eq!(start.branch.head_message_id.as_ref(), Some(&durable_head));

    let template = core
        .list_provider_templates()
        .expect("list provider templates")
        .into_iter()
        .find(|candidate| candidate.id.as_str() == "openai-chat-compatible-v1")
        .expect("OpenAI-compatible template");
    assert_eq!(template.api_family, ApiFamily::OpenAiChatCompletions);
    let (generation_connection, generation_route, generation_preset) =
        create_connection_and_route(&core, &template, origin, suffix, "generation");
    install_generation_credential_authority(&core, &generation_connection.id);
    let (embedding_connection, embedding_route, embedding_preset) =
        create_connection_and_route(&core, &template, origin, suffix, "embedding");
    let (memory_profile, task_profile_revision_id) = create_tasks_and_memory_profile(
        &core,
        suffix,
        &generation_route,
        &generation_preset,
        &embedding_route,
        &embedding_preset,
        embedding_timeout_ms,
    );
    let (book_id, entry_id, book_revision_id) = create_knowledge_book(&core, suffix);
    let prompt_preset_id = bind_semantic_prompt(
        &core,
        suffix,
        &start.conversation.id,
        book_id,
        memory_profile.id,
    );
    let embedding_provider = AdapterRegistry::new()
        .build_embedding_provider_for_route(&template, &embedding_connection, &embedding_route, 3)
        .expect("build exact semantic embedding provider");
    let vector_space_sha256 = embedding_provider.contract().vector_space_sha256();
    assert_eq!(
        embedding_provider
            .contract()
            .execution_sha256(EmbeddingPurpose::RetrievalQuery)
            .len(),
        64
    );
    let request = PromptPlanRequest {
        conversation_id: start.conversation.id,
        branch_id: start.branch.id,
        expected_head: Some(durable_head),
        user_text: "opaque vector retrieval query".to_owned(),
        generation_target: GenerationTarget {
            model_route_id: generation_route.id,
            generation_preset_id: generation_preset.id,
        },
        prompt_preset_id: Some(prompt_preset_id),
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };
    drop(core);

    let storage = Storage::open(root.path()).expect("open semantic fixture storage");
    storage
        .save_knowledge_embedding(&KnowledgeEmbeddingWrite {
            id: format!("semantic-{suffix}-knowledge-embedding"),
            book_revision_id,
            entry_id,
            task_profile_revision_id,
            model_route_id: embedding_route.id,
            dimensions: 3,
            vector_space_sha256,
            values: vec![1.0, 0.0, 0.0],
            created_at: Utc::now(),
        })
        .expect("save exact semantic knowledge embedding");
    drop(storage);

    SemanticFixture {
        core: Core::open(CoreConfig::new(root.path())).expect("reopen semantic Core"),
        request,
        embedding_connection_id: embedding_connection.id,
        generation_connection_id: generation_connection.id,
    }
}

fn wait_for_generation(
    core: &Core,
    branch_id: &lorepia_core::ConversationBranchId,
    generation_id: &lorepia_core::GenerationId,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let messages = core
            .list_branch_messages(branch_id)
            .expect("load semantic generation messages");
        if let Some(assistant) = messages
            .iter()
            .find(|message| message.generation_id.as_ref() == Some(generation_id))
            && assistant.status != MessageStatus::Pending
        {
            assert_eq!(assistant.status, MessageStatus::Complete);
            assert_eq!(assistant.content, "Synthetic semantic reply");
            return;
        }
        assert!(Instant::now() < deadline, "semantic generation timed out");
        thread::sleep(Duration::from_millis(10));
    }
}

fn request_text(request: &[u8]) -> String {
    String::from_utf8(request.to_vec()).expect("synthetic provider request is UTF-8")
}

fn request_json(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("synthetic provider request body");
    serde_json::from_str(body).expect("synthetic provider JSON request")
}

fn assert_embedding_wire_request(request: &str) {
    assert!(request.starts_with("POST /v1/embeddings HTTP/1.1\r\n"));
    assert!(request.to_ascii_lowercase().contains(&format!(
        "authorization: bearer {EMBEDDING_CREDENTIAL_CANARY}\r\n"
    )));
    assert!(!request.contains(GENERATION_CREDENTIAL_CANARY));
    let body = request_json(request);
    assert_eq!(body["model"], "synthetic-embedding-model");
    assert_eq!(body["dimensions"], 3);
    assert!(
        body["input"]
            .as_str()
            .expect("embedding query input")
            .contains("opaque vector retrieval query")
    );
    assert!(!body.to_string().contains(EMBEDDING_CREDENTIAL_CANARY));
    assert!(!body.to_string().contains(GENERATION_CREDENTIAL_CANARY));
}

fn assert_generation_wire_request(request: &str) {
    assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1\r\n"));
    assert!(request.to_ascii_lowercase().contains(&format!(
        "authorization: bearer {GENERATION_CREDENTIAL_CANARY}\r\n"
    )));
    assert!(!request.contains(EMBEDDING_CREDENTIAL_CANARY));
    let body = request_json(request);
    assert_eq!(body["model"], "synthetic-generation-model");
    assert!(body.to_string().contains(KNOWLEDGE_MARKER));
    assert!(!body.to_string().contains(EMBEDDING_CREDENTIAL_CANARY));
    assert!(!body.to_string().contains(GENERATION_CREDENTIAL_CANARY));
}

fn assert_success_provider_requests(requests: &mpsc::Receiver<Vec<u8>>) {
    let embedding_request = request_text(
        &requests
            .recv_timeout(Duration::from_secs(3))
            .expect("captured embedding request"),
    );
    let generation_request = request_text(
        &requests
            .recv_timeout(Duration::from_secs(3))
            .expect("captured generation request"),
    );
    assert_embedding_wire_request(&embedding_request);
    assert_generation_wire_request(&generation_request);
    assert!(
        requests.try_recv().is_err(),
        "provider query was dispatched twice"
    );
}

fn assert_completed_query(root: &Path, query_embedding_id: &str) {
    let storage = Storage::open(root).expect("open completed semantic storage");
    let durable_query = storage
        .get_memory_query_embedding(query_embedding_id)
        .expect("load durable completed semantic query");
    assert_eq!(durable_query.status, MemoryQueryEmbeddingStatus::Succeeded);
    assert_eq!(durable_query.revision, 3);
    assert_eq!(durable_query.attempts, 1);
}

fn assert_tree_excludes(root: &Path, canary: &str) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).expect("inspect Core data root");
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .expect("read Core data root")
                    .map(|entry| entry.expect("read Core data entry").path()),
            );
        } else if metadata.is_file() {
            let bytes = fs::read(&path).expect("read Core data file");
            assert!(
                !bytes
                    .windows(canary.len())
                    .any(|window| window == canary.as_bytes()),
                "credential canary persisted in {}",
                path.display()
            );
        }
    }
}

#[tokio::test]
async fn provider_semantic_query_executes_once_via_broker_and_reuses_durable_completion() {
    let root = tempdir().expect("semantic success root");
    let (origin, requests, server) = spawn_success_provider();
    let SemanticFixture {
        core,
        request,
        embedding_connection_id,
        generation_connection_id,
    } = create_semantic_fixture(&root, &origin, "success", 5_000);
    let broker = RecordingCredentialBroker::new(embedding_connection_id.clone(), root.path());
    let operation_nonce = "semantic-provider-success-preview-v1";

    let preview = core
        .resolve_prompt_preview_async(
            &request,
            GenerationOperationContext::New { operation_nonce },
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect("resolve provider semantic preview through broker");
    assert!(
        preview
            .effective_messages
            .iter()
            .any(|message| message.content.contains(KNOWLEDGE_MARKER))
    );
    assert_single_broker_call(&broker, &embedding_connection_id);
    assert_eq!(broker.observed_query_states(), ["running"]);
    let replay = core
        .resolve_prompt_preview_async(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview.generation_attempt_id,
            },
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect("reuse durable semantic preview");
    assert_eq!(replay, preview);
    assert_single_broker_call(&broker, &embedding_connection_id);
    assert_eq!(broker.observed_query_states(), ["running"]);
    drop(core);
    let core = Core::open(CoreConfig::new(root.path()))
        .expect("reopen Core with durable semantic completion");
    let restarted = core
        .resolve_prompt_preview_async(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview.generation_attempt_id,
            },
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect("reuse durable semantic preview after restart");
    assert_eq!(restarted, preview);
    assert_single_broker_call(&broker, &embedding_connection_id);
    assert_eq!(broker.observed_query_states(), ["running"]);

    let mut reviewed = request;
    reviewed.expected_plan_hash = Some(preview.plan.plan_hash.clone());
    let generation_id = core
        .send_message_with_prompt_plan_async(
            &reviewed,
            &preview.generation_attempt_id,
            generation_credential(&core, generation_connection_id),
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect("send exact provider semantic plan");
    wait_for_generation(&core, &reviewed.branch_id, &generation_id);
    assert_single_broker_call(&broker, &embedding_connection_id);

    let stored_plan = core
        .get_generation_prompt_plan(&generation_id)
        .expect("load provider semantic generation plan");
    let evidence = &stored_plan.provider_request.mapping_diagnostics.value["knowledge_semantic_evidence"]
        [0]["source"];
    assert_eq!(evidence["kind"], "provider_embedding_v1");
    assert_eq!(
        evidence["model_route_id"],
        "semantic-success-embedding-route"
    );
    assert_eq!(evidence["query_embedding_revision"], 3);
    let query_embedding_id = evidence["query_embedding_id"]
        .as_str()
        .expect("durable query embedding identity")
        .to_owned();
    let safe_surfaces = format!("{preview:?}{stored_plan:?}");
    assert!(!safe_surfaces.contains(EMBEDDING_CREDENTIAL_CANARY));
    assert!(!safe_surfaces.contains(GENERATION_CREDENTIAL_CANARY));

    assert_success_provider_requests(&requests);
    server.join().expect("join semantic success provider");

    drop(core);
    assert_completed_query(root.path(), &query_embedding_id);
    assert_tree_excludes(root.path(), EMBEDDING_CREDENTIAL_CANARY);
    assert_tree_excludes(root.path(), GENERATION_CREDENTIAL_CANARY);
}

#[tokio::test]
async fn provider_semantic_unknown_outcome_is_not_implicitly_retried() {
    let root = tempdir().expect("semantic unknown root");
    let (origin, requests, release_server, server) = spawn_stalling_provider();
    let SemanticFixture {
        core,
        request,
        embedding_connection_id,
        generation_connection_id: _,
    } = create_semantic_fixture(&root, &origin, "unknown", 100);
    let broker = RecordingCredentialBroker::new(embedding_connection_id.clone(), root.path());
    let operation_nonce = "semantic-provider-unknown-preview-v1";

    let first_error = core
        .resolve_prompt_preview_async(
            &request,
            GenerationOperationContext::New { operation_nonce },
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect_err("stalled embedding dispatch must have unknown outcome");
    assert_eq!(first_error.code, CoreErrorCode::ProviderUnavailable);
    assert!(!first_error.recoverable);
    assert!(first_error.message.contains("explicit retry is required"));
    assert_single_broker_call(&broker, &embedding_connection_id);
    assert_eq!(broker.observed_query_states(), ["running"]);
    let dispatched_request = request_text(
        &requests
            .recv_timeout(Duration::from_secs(3))
            .expect("captured stalled embedding request"),
    );
    assert_embedding_wire_request(&dispatched_request);

    drop(core);
    let core = Core::open(CoreConfig::new(root.path()))
        .expect("reopen Core after unknown semantic outcome");
    let replay_error = core
        .resolve_prompt_preview_async(
            &request,
            GenerationOperationContext::New { operation_nonce },
            &broker,
            watch::channel(false).1,
        )
        .await
        .expect_err("unknown durable outcome must block implicit redispatch");
    assert_eq!(replay_error.code, CoreErrorCode::ProviderUnavailable);
    assert!(!replay_error.recoverable);
    assert!(
        replay_error
            .message
            .contains("unknown prior provider outcome")
    );
    assert_single_broker_call(&broker, &embedding_connection_id);
    assert_eq!(broker.observed_query_states(), ["running"]);
    let retryable = core
        .list_retryable_memory_query_embeddings(&request.conversation_id, &request.branch_id, 8)
        .expect("list explicit semantic retries");
    assert_eq!(retryable.len(), 1);
    assert_eq!(retryable[0].status, MemoryQueryEmbeddingStatus::Interrupted);
    assert_eq!(retryable[0].revision, 3);
    assert!(retryable[0].requires_unknown_outcome_acknowledgement);
    let denied = core
        .retry_memory_query_embedding(
            &request.conversation_id,
            &request.branch_id,
            &retryable[0].id,
            retryable[0].revision,
            false,
        )
        .expect_err("unknown outcome retry requires positive acknowledgement");
    assert_eq!(denied.code, CoreErrorCode::PermissionDenied);
    let admitted = core
        .retry_memory_query_embedding(
            &request.conversation_id,
            &request.branch_id,
            &retryable[0].id,
            retryable[0].revision,
            true,
        )
        .expect("explicitly acknowledge unknown outcome");
    assert_eq!(admitted.status, MemoryQueryEmbeddingStatus::Queued);
    assert_eq!(admitted.revision, 4);
    assert_single_broker_call(&broker, &embedding_connection_id);

    release_server.send(()).expect("release stalled provider");
    server.join().expect("join stalled semantic provider");
    let safe_surfaces = format!("{first_error:?}{replay_error:?}{retryable:?}{admitted:?}");
    assert!(!safe_surfaces.contains(EMBEDDING_CREDENTIAL_CANARY));
    assert!(!safe_surfaces.contains(GENERATION_CREDENTIAL_CANARY));
    drop(core);
    let storage = Storage::open(root.path()).expect("open interrupted semantic storage");
    let durable_query = storage
        .get_memory_query_embedding(&retryable[0].id)
        .expect("load explicitly requeued semantic query");
    assert_eq!(durable_query.status, MemoryQueryEmbeddingStatus::Queued);
    assert_eq!(durable_query.revision, 4);
    assert_eq!(durable_query.attempts, 1);
    drop(storage);
    assert_tree_excludes(root.path(), EMBEDDING_CREDENTIAL_CANARY);
    assert_tree_excludes(root.path(), GENERATION_CREDENTIAL_CANARY);
}
