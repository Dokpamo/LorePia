use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use chrono::{TimeZone, Utc};
use lorepia_core::{
    ApiFamily, CanonicalOrigin, CapabilityKey, CapabilityObservation, CapabilityValue, Confidence,
    ConnectionBoundCredential, ConnectionConfigEntry, ConnectionConfigValue,
    ContentModuleActivationRequest, ContentModuleBindingDraft, ContentModuleDeactivationRequest,
    ContentModuleRollbackApplyRequest, ContentModuleRollbackResolutionRequest,
    ContentModuleRuntimeTarget, Core, CoreConfig, CoreErrorCode, EndpointPath,
    GenerationOperationContext, GenerationPreset, GenerationPromptCacheSettings,
    GenerationReasoningSettings, GenerationTarget, KnowledgeSimulationRequest,
    KnowledgeTokenEstimate, MemoryRecordUserPatch, MessageStatus, ModelAvailability,
    ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId, ModuleActivationApproval,
    ModuleMergeResolutionSet, ObservationId, ObservationSource, ParameterId, ParameterLiteral,
    ParameterValue, ParameterValueState, PromptPlanRequest, ProviderConnectionDraft,
    ProviderConnectionId, ProviderCredentialObservedStatus, ProviderNetworkMode,
    ResolvedPromptPlan, RoomOrchestrationConfigPatch, SupportStatus, TransformPreviewRequest,
    VariableMap,
};
use lorepia_domain::{
    ActivationRule, AuxiliaryTaskKind, BlockSource, CharacterPromptContent, ContentCapability,
    ContentModule, ContentModuleId, ConversationMode, GenerationId, GenerationRecord,
    GenerationStatus, InstructionAuthority, InteractionAction, InteractionEffect, InteractionEvent,
    InteractionRule, InteractionRuleId, InteractionRuleSet, InteractionRuleSetId,
    KnowledgeActivationReason, KnowledgeBook, KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId,
    KnowledgePlacement, MemoryKind, MemoryProfile, MemoryProfileId, MemoryRecord, MemoryRecordId,
    MergePolicy, Message, ModuleBindingId, ModuleRevisionId, ModuleRevisionResolutionMode,
    ModuleScope, OverflowPolicy, PackageMetadata, PlacementZone, PresetMetadata, PromptBlock,
    PromptBlockId, PromptBlockKind, PromptConversationMessage, PromptMessageRole, PromptPreset,
    PromptPresetId, PromptResolutionContext, PromptResolveRequest, Provenance, ProviderMessageRole,
    ProviderPromptContract, RateLimit, RoleHint, SafeRegex, SafeTemplate, Sha256Digest, SourceKind,
    SummarySchemaId, TaskProfile, TaskProfileId, TemplatePart, TokenBudget, TokenPolicy,
    TransformPhase, TransformRule, TransformRuleId, TransformSet, TransformSetId,
    UnsupportedRolePolicy, VariableId, VariableRef, VariableScope, VersionedJson,
};
use lorepia_orchestration::{
    TransformRuleStatus, default_prompt_preset, resolve_prompt_plan, verify_resolved_prompt_plan,
};
use lorepia_storage::{
    GenerationPromptPlanRecord, KnowledgeActivationLog, PromptPresetBinding, PromptResponseLength,
    ProviderRequestSnapshotRecord, Storage,
};
use tempfile::{NamedTempFile, tempdir};

const CREDENTIAL_CANARY: &str = "sk-synthetic-orchestration-canary-6f71";
const USER_TEXT_CANARY: &str = "Synthetic prompt identity request 84e5";

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

fn reviewed_provider_credential(core: &Core) -> ConnectionBoundCredential {
    let connection_id = ProviderConnectionId::from("synthetic-orchestration-connection");
    let authority = core
        .ensure_provider_credential_access_settled(&connection_id)
        .expect("read synthetic credential access authority");
    ConnectionBoundCredential::new_with_access_authority(
        connection_id,
        Some(CREDENTIAL_CANARY.to_owned()),
        authority,
    )
}

fn install_provider_credential_authority(core: &Core, connection_id: &ProviderConnectionId) {
    let authority = core
        .propose_provider_credential_install_authority(connection_id)
        .expect("propose synthetic credential install authority");
    let install = core
        .prepare_provider_credential_install_operation(
            connection_id,
            &authority,
            ProviderCredentialObservedStatus::Missing,
        )
        .expect("prepare synthetic credential install");
    core.start_provider_credential_operation(&install.plan.operation_id, &install.plan_sha256)
        .expect("start synthetic credential install");
    core.finish_provider_credential_operation(
        &install.plan.operation_id,
        &install.plan_sha256,
        ProviderCredentialObservedStatus::Available,
    )
    .expect("finish synthetic credential install");
    let authority = core
        .ensure_provider_credential_access_settled(connection_id)
        .expect("read installed synthetic credential access authority");
    assert_eq!(authority.authority_id, install.plan.operation_id);
}

fn timestamp() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
        .single()
        .expect("valid synthetic timestamp")
}

fn provenance(source_kind: SourceKind, source_id: &str) -> Provenance {
    Provenance {
        source_kind,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic Test Author".to_owned()),
        license: Some("LicenseRef-Synthetic".to_owned()),
        imported_at: None,
    }
}

fn prompt_preset(id: &str) -> PromptPreset {
    let mut preset = default_prompt_preset(
        PromptPresetId::from(id),
        "Synthetic Core preset",
        PresetMetadata {
            description: "Synthetic prompt CRUD and generation fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: provenance(SourceKind::UserCreated, id),
            created_at: timestamp(),
            updated_at: timestamp(),
            local_override_of: None,
        },
    );
    // This fixture exercises the creator upsert path. The default factory is
    // also used by Core's internal built-in seeding path, so its blocks carry
    // application provenance until a creator-owned document replaces it.
    for block in &mut preset.blocks {
        block.provenance = provenance(SourceKind::UserCreated, block.id.as_str());
    }
    preset
}

fn knowledge_book() -> KnowledgeBook {
    let book_id = KnowledgeBookId::from("synthetic.core.knowledge");
    KnowledgeBook {
        id: book_id.clone(),
        name: "Synthetic Core knowledge".to_owned(),
        schema_version: 1,
        entries: vec![KnowledgeEntry {
            id: KnowledgeEntryId::from("synthetic.core.knowledge.moon"),
            book_id,
            name: "Synthetic Moon".to_owned(),
            content: "The synthetic moon is cobalt.".to_owned(),
            enabled: true,
            activation: ActivationRule::Keyword {
                primary: vec!["moon".to_owned()],
                secondary: Vec::new(),
                selective: false,
                case_sensitive: false,
                whole_word: true,
            },
            priority: 10,
            importance: 100,
            placement: KnowledgePlacement::RetrievedContext,
            token_policy: TokenPolicy {
                priority: 100,
                min_tokens: None,
                max_tokens: None,
                reserve_tokens: None,
            },
            parent_id: None,
            activation_probability_basis_points: 10_000,
            provenance: provenance(SourceKind::UserCreated, "synthetic.core.knowledge.moon"),
        }],
        scan_depth: 8,
        token_budget: TokenBudget { max_tokens: 32 },
        recursive: false,
        max_recursion_depth: 0,
        provenance: provenance(SourceKind::UserCreated, "synthetic.core.knowledge"),
    }
}

fn invalid_transform_set() -> TransformSet {
    TransformSet {
        id: TransformSetId::from("synthetic.core.transform"),
        name: "Synthetic Core transform".to_owned(),
        schema_version: 1,
        enabled: true,
        imported_author_enabled: false,
        rules: vec![TransformRule {
            id: TransformRuleId::from("synthetic.core.transform.invalid"),
            name: "Invalid regex stays inert".to_owned(),
            enabled: true,
            imported_enabled: false,
            imported_author_enabled: false,
            phase: TransformPhase::ProviderOutputCanonical,
            order: 0,
            pattern: SafeRegex {
                pattern: "(".to_owned(),
                case_insensitive: false,
            },
            replacement: "must-not-replace".to_owned(),
            condition: None,
            max_replacements: 8,
            input_limit: 1_024,
            output_limit: 1_024,
            provenance: provenance(SourceKind::UserCreated, "synthetic.core.transform.invalid"),
        }],
        max_rules_per_phase: 8,
        max_output_chars: 1_024,
        provenance: provenance(SourceKind::UserCreated, "synthetic.core.transform"),
    }
}

fn content_module() -> ContentModule {
    ContentModule {
        id: ContentModuleId::from("synthetic.core.module"),
        name: "Synthetic Core module".to_owned(),
        version: "1.0.0".to_owned(),
        schema_version: 1,
        prompt_fragments: Vec::new(),
        knowledge_book_ids: Vec::new(),
        control_specs: Vec::new(),
        transform_set_ids: Vec::new(),
        interaction_rule_set_ids: Vec::new(),
        asset_ids: Vec::new(),
        imported_components_enabled: false,
        required_capabilities: vec![ContentCapability::Knowledge],
        metadata: PackageMetadata {
            author: Some("Synthetic Test Author".to_owned()),
            license: "LicenseRef-Unknown".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: "Synthetic module for local-only acceptance testing".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: provenance(SourceKind::UserCreated, "synthetic.core.module"),
        },
    }
}

fn prompt_marker_module() -> ContentModule {
    let mut module = content_module();
    module.id = ContentModuleId::from("synthetic.core.module.prompt-marker");
    "Synthetic prompt marker module".clone_into(&mut module.name);
    module.required_capabilities = vec![ContentCapability::PromptFragments];
    module.metadata.provenance = provenance(
        SourceKind::UserCreated,
        "synthetic.core.module.prompt-marker",
    );
    module.prompt_fragments = vec![PromptBlock {
        id: PromptBlockId::from("synthetic.core.module.prompt-marker.block"),
        name: "Synthetic app-scope marker".to_owned(),
        kind: PromptBlockKind::StaticInstruction,
        enabled: true,
        role_hint: RoleHint::System,
        authority: InstructionAuthority::Creator,
        template: Some(SafeTemplate {
            parts: vec![TemplatePart::Text {
                value: "SYNTHETIC_APP_SCOPE_MODULE_MARKER_5D31".to_owned(),
            }],
            max_output_chars: 128,
        }),
        condition: None,
        source: BlockSource::Template,
        // Module fragments are appended after the base preset by Core. Keep the
        // marker in the final zone so this acceptance fixture isolates
        // cross-context materialization rather than block-order normalization.
        placement_zone: PlacementZone::AssistantPrefill,
        history_selector: None,
        token_policy: TokenPolicy {
            priority: 1_000,
            min_tokens: None,
            max_tokens: Some(64),
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::Reject,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance(
            SourceKind::UserCreated,
            "synthetic.core.module.prompt-marker.block",
        ),
    }];
    module
}

fn interaction_counter_rule_set() -> (InteractionRuleSet, VariableRef) {
    let counter = VariableRef {
        scope: VariableScope::Branch,
        namespace: None,
        id: VariableId::from("synthetic_branch_commit_count"),
    };
    let rule_set = InteractionRuleSet {
        id: InteractionRuleSetId::from("synthetic.core.interaction.fork-counter"),
        name: "Synthetic fork checkpoint counter".to_owned(),
        schema_version: 1,
        rules: vec![InteractionRule {
            id: InteractionRuleId::from("synthetic.core.interaction.fork-counter.commit"),
            name: "Count committed assistant turns".to_owned(),
            enabled: true,
            imported_author_enabled: false,
            event: InteractionEvent::MessageCommitted,
            condition: None,
            actions: vec![
                InteractionAction::IncrementVariable {
                    target: counter.clone(),
                    amount: 1,
                },
                InteractionAction::AppendVisibleSystemEvent {
                    text: SafeTemplate {
                        parts: vec![TemplatePart::Variable {
                            variable: counter.clone(),
                        }],
                        max_output_chars: 32,
                    },
                },
            ],
            priority: 0,
            stop_after_match: false,
            provenance: provenance(
                SourceKind::UserCreated,
                "synthetic.core.interaction.fork-counter.commit",
            ),
        }],
        max_actions_per_event: 8,
        provenance: provenance(
            SourceKind::UserCreated,
            "synthetic.core.interaction.fork-counter",
        ),
    };
    (rule_set, counter)
}

fn interaction_counter_module(rule_set_id: &InteractionRuleSetId) -> ContentModule {
    let mut module = content_module();
    module.id = ContentModuleId::from("synthetic.core.module.interaction-counter");
    "Synthetic interaction counter module".clone_into(&mut module.name);
    module.interaction_rule_set_ids = vec![rule_set_id.clone()];
    module.imported_components_enabled = true;
    module.required_capabilities = vec![ContentCapability::DeclarativeInteractions];
    module.metadata.provenance = provenance(
        SourceKind::UserCreated,
        "synthetic.core.module.interaction-counter",
    );
    module
}

fn activate_app_module(
    core: &Core,
    module: &ContentModule,
    runtime_target: ContentModuleRuntimeTarget,
    binding_id: &str,
) {
    core.upsert_content_module(module, None)
        .expect("save app-scope content module");
    let request = ContentModuleActivationRequest {
        runtime_target,
        expected_binding_revision: None,
        binding: ContentModuleBindingDraft {
            id: ModuleBindingId::from(binding_id),
            module_id: module.id.clone(),
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
        .expect("review app-scope content module");
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let plan = core
        .resolve_content_module_activation(&request, &resolutions)
        .expect("resolve app-scope content module");
    let receipt = core
        .activate_content_module(
            &request,
            &resolutions,
            &ModuleActivationApproval {
                approval_id: format!("{binding_id}-approval"),
                expected_review_sha256: review.review_sha256,
                expected_plan_sha256: plan.plan_sha256,
            },
        )
        .expect("activate app-scope content module");
    receipt.verify().expect("verify app-scope module receipt");
}

fn memory_record(
    id: &str,
    branch_id: &lorepia_core::ConversationBranchId,
    range: &[Message],
) -> MemoryRecord {
    assert_eq!(
        range.len(),
        2,
        "memory fixture range must contain one complete turn"
    );
    MemoryRecord {
        id: MemoryRecordId::from(id),
        conversation_id: range[0].conversation_id.clone(),
        branch_id: branch_id.clone(),
        source_start_message_id: range[0].id.clone(),
        source_end_message_id: range[1].id.clone(),
        kind: MemoryKind::EpisodicEvent,
        title: format!("Synthetic memory {id}"),
        summary: format!("Synthetic summary for {id}"),
        structured_data: VersionedJson {
            schema_version: 1,
            value: serde_json::json!({ "fixture": id }),
        },
        importance: 100,
        keywords: vec!["synthetic".to_owned()],
        embedding_ref: None,
        pinned: false,
        excluded_from_conversation: false,
        excluded_from_character: false,
        created_at: timestamp(),
        updated_at: timestamp(),
        invalidated_at: None,
        provenance: provenance(SourceKind::UserCreated, id),
    }
}

fn interaction_visible_system_texts(
    core: &Core,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
) -> Vec<String> {
    core.list_interaction_effect_history(conversation_id, branch_id, None, 1_024)
        .expect("list durable interaction effect history")
        .into_iter()
        .filter_map(|history| match history.stored.effect {
            InteractionEffect::VisibleSystemEvent { text } => Some(text),
            _ => None,
        })
        .collect()
}

fn wait_for_interaction_visible_system_texts(
    core: &Core,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
    expected_count: usize,
) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let texts = interaction_visible_system_texts(core, conversation_id, branch_id);
        if texts.len() >= expected_count {
            return texts;
        }
        assert!(
            Instant::now() < deadline,
            "interaction lifecycle did not publish {expected_count} visible effects; got {texts:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .expect("set provider request timeout");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    loop {
        let read = match stream.read(&mut buffer) {
            Ok(0) => panic!("provider closed before sending a complete HTTP request"),
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) && Instant::now() < deadline =>
            {
                continue;
            }
            Err(error) => panic!("read provider request: {error}"),
        };
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
                    .then_some(value.trim())
            })
            .expect("provider request content-length header")
            .parse::<usize>()
            .expect("provider request content-length value");
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    request
}

fn spawn_provider(
    request_count: usize,
) -> (
    CanonicalOrigin,
    mpsc::Receiver<Vec<u8>>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind synthetic provider");
    let address = listener.local_addr().expect("synthetic provider address");
    let (request_sender, request_receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        for _ in 0..request_count {
            let (mut stream, _) = listener
                .accept()
                .expect("accept synthetic provider request");
            request_sender
                .send(read_http_request(&mut stream))
                .expect("capture synthetic provider request");
            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Synthetic reply\"}}]}\n\n",
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
            .expect("write synthetic provider response");
        }
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("canonical synthetic provider origin"),
        request_receiver,
        handle,
    )
}

struct StoppableProvider {
    stop: mpsc::Sender<()>,
    handle: thread::JoinHandle<()>,
}

impl StoppableProvider {
    fn stop(self) {
        let _ = self.stop.send(());
        self.handle
            .join()
            .expect("join stoppable synthetic provider");
    }
}

fn spawn_stoppable_provider() -> (CanonicalOrigin, mpsc::Receiver<Vec<u8>>, StoppableProvider) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind stoppable synthetic provider");
    listener
        .set_nonblocking(true)
        .expect("make synthetic provider stop-aware");
    let address = listener.local_addr().expect("synthetic provider address");
    let (request_sender, request_receiver) = mpsc::channel();
    let (stop_sender, stop_receiver) = mpsc::channel();
    let handle = thread::spawn(move || {
        loop {
            if stop_receiver.try_recv().is_ok() {
                break;
            }
            let (mut stream, _) = match listener.accept() {
                Ok(accepted) => accepted,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("accept stoppable synthetic provider request: {error}"),
            };
            // Darwin may inherit O_NONBLOCK from the listener; only accept needs polling.
            stream
                .set_nonblocking(false)
                .expect("make accepted provider request blocking");
            let request = read_http_request(&mut stream);
            if request_sender.send(request).is_err() {
                break;
            }
            let body = concat!(
                "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Synthetic reply\"}}]}\n\n",
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
            .expect("write stoppable synthetic provider response");
        }
    });
    (
        CanonicalOrigin::parse(&format!("http://{address}"))
            .expect("canonical stoppable synthetic provider origin"),
        request_receiver,
        StoppableProvider {
            stop: stop_sender,
            handle,
        },
    )
}

fn generation_attempt_count(root: &Path) -> i64 {
    rusqlite::Connection::open(active_database_path(root))
        .expect("open generation-attempt count database")
        .query_row(
            "SELECT COUNT(*) FROM generation_attempt_intents",
            [],
            |row| row.get(0),
        )
        .expect("count generation attempts")
}

fn import_synthetic_character(core: &Core) -> String {
    let mut source = NamedTempFile::new().expect("temporary synthetic character");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Ari","description":"Entirely synthetic test character."}}}}"#
    )
    .expect("write synthetic character");
    let review = core
        .inspect_import(source.path())
        .expect("inspect character");
    core.commit_import(&review.id)
        .expect("commit synthetic character")
        .id
}

fn provider_fixture(core: &Core, origin: &CanonicalOrigin) -> GenerationTarget {
    let template = core
        .list_provider_templates()
        .expect("list provider templates")
        .into_iter()
        .find(|template| template.id.as_str() == "openai-chat-compatible-v1")
        .expect("OpenAI-compatible provider template");
    let connection = core
        .create_provider_connection(ProviderConnectionDraft {
            id: ProviderConnectionId::from("synthetic-orchestration-connection"),
            template_id: template.id.clone(),
            template_version: template.manifest_version,
            display_name: "Synthetic orchestration provider".to_owned(),
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
        .expect("create provider connection");
    install_provider_credential_authority(core, &connection.id);
    let now = Utc::now();
    assert_eq!(template.api_family, ApiFamily::OpenAiChatCompletions);
    let route = core
        .upsert_model_route(ModelRoute {
            id: ModelRouteId::from("synthetic-orchestration-route"),
            connection_id: connection.id,
            api_family: template.api_family,
            model_id: "synthetic-model".to_owned(),
            display_name: Some("Synthetic model".to_owned()),
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
    let generation_preset = core
        .upsert_generation_preset(GenerationPreset {
            id: "synthetic-orchestration-generation-preset".into(),
            model_route_id: route.id.clone(),
            display_name: "Synthetic generation preset".to_owned(),
            values: Vec::new(),
            reasoning: GenerationReasoningSettings::default(),
            prompt_cache: GenerationPromptCacheSettings::default(),
            created_at: now,
            updated_at: now,
        })
        .expect("save synthetic generation preset");
    GenerationTarget {
        model_route_id: route.id,
        generation_preset_id: generation_preset.id,
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
            .expect("load messages while waiting");
        if let Some(assistant) = messages
            .iter()
            .find(|message| message.generation_id.as_ref() == Some(generation_id))
            && assistant.status != MessageStatus::Pending
        {
            assert_eq!(assistant.status, MessageStatus::Complete);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "synthetic generation did not finish"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn request_body(request: &[u8]) -> serde_json::Value {
    let header_end = request
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .expect("provider request header terminator");
    serde_json::from_slice(&request[header_end + 4..]).expect("provider request JSON")
}

fn assert_tree_excludes(root: &Path, needle: &[u8]) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path).expect("inspect Core data-root entry");
        if metadata.is_dir() {
            pending.extend(
                fs::read_dir(&path)
                    .expect("read Core data-root")
                    .map(|entry| entry.expect("read Core data-root entry").path()),
            );
        } else if metadata.is_file() {
            let bytes = fs::read(&path).expect("read Core data-root file");
            assert!(
                !bytes.windows(needle.len()).any(|window| window == needle),
                "sensitive canary was persisted in {}",
                path.display()
            );
        }
    }
}

#[test]
fn prompt_crud_uses_revision_cas_and_soft_delete() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let mut preset = prompt_preset("synthetic.core.prompt-crud");

    let created = core
        .upsert_prompt_preset(&preset, None)
        .expect("create prompt preset");
    assert_eq!(created.revision, 1);
    let duplicate = core
        .upsert_prompt_preset(&preset, None)
        .expect_err("new-only insert must reject an existing preset");
    assert_eq!(duplicate.code, CoreErrorCode::InvalidInput);
    assert!(duplicate.recoverable);

    "Synthetic Core preset v2".clone_into(&mut preset.name);
    preset.metadata.updated_at = timestamp() + chrono::Duration::seconds(1);
    let updated = core
        .upsert_prompt_preset(&preset, Some(created.revision))
        .expect("update exact prompt revision");
    assert_eq!(updated.revision, 2);
    assert_eq!(
        core.get_prompt_preset(&preset.id)
            .expect("load updated preset"),
        updated
    );
    let stale = core
        .upsert_prompt_preset(&preset, Some(created.revision))
        .expect_err("stale prompt update must fail");
    assert_eq!(stale.code, CoreErrorCode::InvalidInput);
    assert!(stale.recoverable);

    let deleted = core
        .delete_prompt_preset(&preset.id, updated.revision)
        .expect("soft-delete exact prompt revision");
    assert_eq!(deleted.revision, 3);
    assert!(deleted.deleted_at.is_some());
    assert_eq!(
        core.get_prompt_preset(&preset.id)
            .expect_err("soft-deleted prompt must be hidden")
            .code,
        CoreErrorCode::NotFound
    );
    assert!(
        core.list_prompt_presets()
            .expect("list prompt presets")
            .iter()
            .all(|item| item.value.id != preset.id)
    );
}

#[test]
fn knowledge_and_transform_previews_are_deterministic_and_fail_open() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let book = knowledge_book();
    let transform_set = invalid_transform_set();
    core.upsert_knowledge_book(&book, None)
        .expect("save knowledge book");
    core.upsert_transform_set(&transform_set, None)
        .expect("save transform set");

    let simulation = KnowledgeSimulationRequest {
        book_id: book.id.clone(),
        sample_texts: vec!["The synthetic MOON appeared.".to_owned()],
        manual_entry_ids: Vec::new(),
        semantic_scores: Vec::new(),
        variables: VariableMap::default(),
        supported_capabilities: Vec::new(),
        token_estimates: vec![KnowledgeTokenEstimate {
            entry_id: book.entries[0].id.clone(),
            tokens: 5,
        }],
        activation_seed: 42,
    };
    let first = core
        .simulate_knowledge_activation(&simulation)
        .expect("simulate knowledge activation");
    let second = core
        .simulate_knowledge_activation(&simulation)
        .expect("repeat knowledge activation");
    assert_eq!(first, second);
    assert_eq!(first.selected.len(), 1);
    assert_eq!(first.selected[0].entry_id, book.entries[0].id);
    assert!(first.evidence[0].reasons.iter().any(|reason| matches!(
        reason,
        lorepia_core::KnowledgeActivationReason::Keyword { matched }
            if matched.eq_ignore_ascii_case("moon")
    )));

    let original = "<b>literal synthetic text</b>";
    let transformed = core
        .preview_transform(&TransformPreviewRequest {
            transform_set_id: transform_set.id,
            rule_id: transform_set.rules[0].id.clone(),
            input: original.to_owned(),
            variables: VariableMap::default(),
            supported_capabilities: Vec::new(),
            approved_import_source_ids: Vec::new(),
            allow_resolved_prompt: false,
        })
        .expect("preview invalid transform");
    assert_eq!(transformed.original, original);
    assert_eq!(transformed.output, original);
    assert!(!transformed.changed);
    assert_eq!(transformed.reports.len(), 1);
    assert_eq!(transformed.reports[0].status, TransformRuleStatus::Failed);
}

#[test]
fn room_generation_preset_resolves_its_own_route_over_the_global_target() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, _requests, provider) = spawn_provider(0);
    let global_target = provider_fixture(&core, &origin);
    let mut room_route = core
        .list_model_routes(&ProviderConnectionId::from(
            "synthetic-orchestration-connection",
        ))
        .expect("list synthetic model routes")
        .into_iter()
        .find(|route| route.id == global_target.model_route_id)
        .expect("global synthetic route");
    room_route.id = ModelRouteId::from("synthetic-orchestration-room-route");
    room_route.model_id = "synthetic-room-model".to_owned();
    room_route.display_name = Some("Synthetic room model".to_owned());
    let room_route = core
        .upsert_model_route(room_route)
        .expect("save room model route");
    let mut room_preset = core
        .list_generation_presets(&global_target.model_route_id)
        .expect("list global generation presets")
        .into_iter()
        .find(|preset| preset.id == global_target.generation_preset_id)
        .expect("global synthetic generation preset");
    room_preset.id = "synthetic-orchestration-room-preset".into();
    room_preset.model_route_id = room_route.id.clone();
    room_preset.display_name = "Synthetic room generation preset".to_owned();
    let room_preset = core
        .upsert_generation_preset(room_preset)
        .expect("save room generation preset");
    let room_target = GenerationTarget {
        model_route_id: room_route.id,
        generation_preset_id: room_preset.id,
    };
    core.select_generation_target(Some(global_target.clone()))
        .expect("select global generation target");
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic room target",
            ConversationMode::Chat,
        )
        .expect("create room target conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list room target branches")
        .into_iter()
        .next()
        .expect("root room target branch");
    let inherited = core
        .get_room_orchestration_config(&conversation.id, &branch.id)
        .expect("resolve inherited room target");
    assert_eq!(inherited.generation_target, Some(global_target.clone()));

    let saved = core
        .save_room_orchestration_config(
            &conversation.id,
            &branch.id,
            inherited.binding_revision,
            &RoomOrchestrationConfigPatch {
                prompt_preset_id: Some(inherited.prompt_preset_id),
                generation_preset_id: Some(room_target.generation_preset_id.clone()),
                creator_values: BTreeMap::default(),
                response_length: inherited.response_length,
                creativity: inherited.creativity,
                reasoning_effort: inherited.reasoning_effort,
                memory_enabled: inherited.memory_enabled,
                knowledge_enabled: inherited.knowledge_enabled,
                user_name_override: inherited.user_name_override,
                author_note: inherited.author_note,
                group_context: inherited.group_context,
                template_slots: inherited.template_slots,
            },
        )
        .expect("save room generation target");

    assert_eq!(
        saved.generation_preset_id,
        Some(room_target.generation_preset_id.clone())
    );
    assert_eq!(saved.generation_target, Some(room_target));
    let settings = core.get_settings().expect("load unchanged global settings");
    assert_eq!(
        settings.selected_model_route_id,
        Some(global_target.model_route_id)
    );
    assert_eq!(
        settings.selected_generation_preset_id,
        Some(global_target.generation_preset_id)
    );
    provider.join().expect("join idle synthetic provider");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one fixture proves edit, delete, and lineage isolation against the same branch graph"
)]
fn memory_user_edits_deletes_and_branch_lineage_are_durable() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_provider(3);
    let target = provider_fixture(&core, &origin);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic memory branches",
            lorepia_core::ConversationMode::Chat,
        )
        .expect("create memory conversation");
    let root_branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list root branch")
        .into_iter()
        .next()
        .expect("root branch");

    let root_generation = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &root_branch.id,
            None,
            lorepia_core::ConversationMode::Chat,
            "Synthetic root turn",
            GenerationOperationContext::New {
                operation_nonce: "memory-root-turn-v1",
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("send root turn");
    wait_for_generation(&core, &root_branch.id, &root_generation);
    let root_messages = core
        .list_branch_messages(&root_branch.id)
        .expect("root branch messages");
    assert_eq!(root_messages.len(), 2);
    let root_head = root_messages[1].id.clone();

    let current_branch = core
        .create_conversation_branch(
            &conversation.id,
            Some(&root_head),
            Some("Current branch".to_owned()),
        )
        .expect("create current branch");
    let sibling_branch = core
        .create_conversation_branch(
            &conversation.id,
            Some(&root_head),
            Some("Sibling branch".to_owned()),
        )
        .expect("create sibling branch");
    let current_generation = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &current_branch.id,
            Some(&root_head),
            lorepia_core::ConversationMode::Chat,
            "Synthetic current-branch turn",
            GenerationOperationContext::New {
                operation_nonce: "memory-current-branch-turn-v1",
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("send current-branch turn");
    wait_for_generation(&core, &current_branch.id, &current_generation);
    let sibling_generation = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &sibling_branch.id,
            Some(&root_head),
            lorepia_core::ConversationMode::Chat,
            "Synthetic sibling-branch turn",
            GenerationOperationContext::New {
                operation_nonce: "memory-sibling-branch-turn-v1",
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("send sibling-branch turn");
    wait_for_generation(&core, &sibling_branch.id, &sibling_generation);

    let current_messages = core
        .list_branch_messages(&current_branch.id)
        .expect("current branch messages");
    let sibling_messages = core
        .list_branch_messages(&sibling_branch.id)
        .expect("sibling branch messages");
    assert_eq!(current_messages.len(), 4);
    assert_eq!(sibling_messages.len(), 4);
    let root_memory = memory_record("synthetic.memory.root", &root_branch.id, &root_messages);
    let current_memory = memory_record(
        "synthetic.memory.current",
        &current_branch.id,
        &current_messages[2..],
    );
    let sibling_memory = memory_record(
        "synthetic.memory.sibling",
        &sibling_branch.id,
        &sibling_messages[2..],
    );
    drop(core);
    let storage = Storage::open(root.path()).expect("open exclusive storage fixture seam");
    let root_stored = storage
        .save_memory_record(&root_memory, None)
        .expect("create root memory");
    let current_stored = storage
        .save_memory_record(&current_memory, None)
        .expect("create current memory");
    storage
        .save_memory_record(&sibling_memory, None)
        .expect("create sibling memory");
    drop(storage);
    let core = Core::open(CoreConfig::new(root.path())).expect("reopen Core after fixture seeding");
    assert_eq!(root_stored.revision, 1);
    assert_eq!(current_stored.revision, 1);

    let edited = core
        .patch_memory_record_user_fields(
            &current_memory.id,
            current_stored.revision,
            &MemoryRecordUserPatch {
                summary: Some("User-edited current memory".to_owned()),
                ..MemoryRecordUserPatch::default()
            },
        )
        .expect("edit memory at exact revision");
    assert_eq!(edited.revision, 2);
    assert_eq!(edited.value.summary, "User-edited current memory");
    let stale = core
        .patch_memory_record_user_fields(
            &current_memory.id,
            current_stored.revision,
            &MemoryRecordUserPatch {
                summary: Some("Stale memory edit".to_owned()),
                ..MemoryRecordUserPatch::default()
            },
        )
        .expect_err("stale memory edit must fail");
    assert_eq!(stale.code, CoreErrorCode::InvalidInput);
    assert!(stale.recoverable);

    let visible = core
        .list_memory_records(&conversation.id, &current_branch.id, false)
        .expect("list current branch memory lineage");
    let visible_ids = visible
        .iter()
        .map(|stored| stored.value.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        visible_ids,
        std::collections::BTreeSet::from(["synthetic.memory.current", "synthetic.memory.root",])
    );

    let deleted = core
        .delete_memory_record(&current_memory.id, edited.revision)
        .expect("soft-delete edited memory");
    assert_eq!(deleted.revision, 3);
    assert!(deleted.deleted_at.is_some());
    assert_eq!(
        core.get_memory_record(&current_memory.id)
            .expect_err("deleted memory must be hidden")
            .code,
        CoreErrorCode::NotFound
    );
    let after_delete = core
        .list_memory_records(&conversation.id, &current_branch.id, false)
        .expect("list memory lineage after delete");
    assert_eq!(after_delete.len(), 1);
    assert_eq!(after_delete[0].value.id, root_memory.id);

    for _ in 0..3 {
        requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured branch provider request");
    }
    provider.join().expect("join synthetic provider");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the exact historical fork contract needs source turns, pre/post memories, and provider evidence"
)]
fn historical_edit_fork_includes_only_memory_whose_complete_range_precedes_the_fork() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_provider(4);
    let target = provider_fixture(&core, &origin);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic historical memory fork",
            lorepia_core::ConversationMode::Chat,
        )
        .expect("create historical memory conversation");
    let source_branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list source branches")
        .into_iter()
        .next()
        .expect("source branch");

    let summary_task_id = TaskProfileId::from("synthetic.core.memory-fork.summary-task");
    core.upsert_task_profile(
        &TaskProfile {
            id: summary_task_id.clone(),
            kind: AuxiliaryTaskKind::MemorySummary,
            route_id: target.model_route_id.clone(),
            generation_preset_id: target.generation_preset_id.clone(),
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
    .expect("save memory summary task");
    let memory_profile_id = MemoryProfileId::from("synthetic.core.memory-fork.profile");
    core.upsert_memory_profile(
        &MemoryProfile {
            id: memory_profile_id.clone(),
            name: "Synthetic historical fork memory".to_owned(),
            schema_version: 1,
            summary_task: summary_task_id,
            embedding_task: None,
            turns_per_summary: 100,
            recent_raw_budget: TokenBudget { max_tokens: 1_024 },
            episodic_budget: TokenBudget { max_tokens: 1_024 },
            semantic_budget: TokenBudget { max_tokens: 1_024 },
            retrieval_count: 16,
            recency_weight: 1.0,
            similarity_weight: 1.0,
            importance_weight: 1.0,
            preserve_invalidated_records: true,
            summary_schema: SummarySchemaId::from("synthetic.core.memory-fork.schema"),
            provenance: provenance(
                SourceKind::UserCreated,
                "synthetic.core.memory-fork.profile",
            ),
        },
        None,
    )
    .expect("save memory profile");
    let mut preset = prompt_preset("synthetic.core.memory-fork.preset");
    preset.memory_profile_id = Some(memory_profile_id);
    preset.blocks.insert(
        1,
        PromptBlock {
            id: PromptBlockId::from("synthetic.core.memory-fork.block"),
            name: "Synthetic selected memory".to_owned(),
            kind: PromptBlockKind::RetrievedMemory,
            enabled: true,
            role_hint: RoleHint::System,
            authority: InstructionAuthority::Creator,
            template: None,
            condition: None,
            source: BlockSource::SelectedMemory,
            placement_zone: PlacementZone::RetrievedContext,
            history_selector: None,
            token_policy: TokenPolicy {
                priority: 900,
                min_tokens: None,
                max_tokens: Some(1_024),
                reserve_tokens: None,
            },
            overflow_policy: OverflowPolicy::TrimTail,
            merge_policy: MergePolicy::SeparateMessage,
            provenance: provenance(SourceKind::UserCreated, "synthetic.core.memory-fork.block"),
        },
    );
    core.upsert_prompt_preset(&preset, None)
        .expect("save memory-aware prompt preset");
    let now = Utc::now();
    core.bind_prompt_preset(
        &PromptPresetBinding {
            id: "synthetic.core.memory-fork.binding".to_owned(),
            prompt_preset_id: preset.id,
            scope: ModuleScope::Conversation,
            target_id: Some(conversation.id.0.clone()),
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
    .expect("bind memory preset at conversation scope");

    let mut expected_head = None;
    for (text, operation_nonce) in [
        ("Synthetic source turn one", "memory-source-turn-one-v1"),
        ("Synthetic source turn two", "memory-source-turn-two-v1"),
        ("Synthetic source turn three", "memory-source-turn-three-v1"),
    ] {
        let generation = core
            .send_message_to_branch_with_connection_credential(
                &conversation.id,
                &source_branch.id,
                expected_head.as_ref(),
                lorepia_core::ConversationMode::Chat,
                text,
                GenerationOperationContext::New { operation_nonce },
                &target,
                reviewed_provider_credential(&core),
            )
            .expect("send source turn");
        wait_for_generation(&core, &source_branch.id, &generation);
        expected_head = core
            .list_branch_messages(&source_branch.id)
            .expect("source messages after turn")
            .last()
            .map(|message| message.id.clone());
    }
    for _ in 0..3 {
        requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured source provider request");
    }
    let source_messages = core
        .list_branch_messages(&source_branch.id)
        .expect("complete source lineage");
    assert_eq!(source_messages.len(), 6);
    drop(core);
    let storage = Storage::open(root.path()).expect("open exclusive storage fixture seam");
    storage
        .save_memory_record(
            &memory_record(
                "synthetic.memory.before-fork",
                &source_branch.id,
                &source_messages[0..2],
            ),
            None,
        )
        .expect("save pre-fork memory");
    storage
        .save_memory_record(
            &memory_record(
                "synthetic.memory.after-fork",
                &source_branch.id,
                &source_messages[4..6],
            ),
            None,
        )
        .expect("save post-fork memory");
    drop(storage);
    let core = Core::open(CoreConfig::new(root.path())).expect("reopen Core after fixture seeding");

    let edited = core
        .edit_user_message_with_connection_credential(
            &conversation.id,
            &source_branch.id,
            source_messages.last().map(|message| &message.id),
            &source_messages[2].id,
            "Synthetic replacement for turn two",
            GenerationOperationContext::New {
                operation_nonce: "memory-historical-edit-turn-two-v1",
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("edit an old user message into a historical fork");
    let edited_request = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured historical-fork provider request");
    wait_for_generation(&core, &edited.branch.id, &edited.generation_id);
    let edited_request_json = serde_json::to_string(&request_body(&edited_request))
        .expect("encode captured provider request");
    assert!(
        edited_request_json.contains("Synthetic summary for synthetic.memory.before-fork"),
        "the prompt must include memory whose complete source range precedes the fork"
    );
    assert!(
        !edited_request_json.contains("Synthetic summary for synthetic.memory.after-fork"),
        "the prompt must exclude memory created from post-fork source messages"
    );

    let child_visible = core
        .list_memory_records(&conversation.id, &edited.branch.id, false)
        .expect("list exact child memory lineage");
    assert_eq!(
        child_visible
            .iter()
            .map(|record| record.value.id.as_str())
            .collect::<Vec<_>>(),
        vec!["synthetic.memory.before-fork"]
    );
    assert_eq!(
        core.list_memory_records(&conversation.id, &source_branch.id, false)
            .expect("source memory remains unchanged")
            .len(),
        2
    );
    provider.join().expect("join synthetic provider");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end contract proves nonce isolation across preview, resume, storage, and dispatch"
)]
fn reviewed_operation_nonce_changes_only_the_durable_operation_identity() {
    const NONCE_A: &str = "reviewed-nonce-isolation-A-7d4f";
    const NONCE_B: &str = "reviewed-nonce-isolation-B-9a21";

    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_stoppable_provider();
    let target = provider_fixture(&core, &origin);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic reviewed nonce isolation",
            ConversationMode::Chat,
        )
        .expect("create reviewed nonce conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list reviewed nonce branch")
        .into_iter()
        .next()
        .expect("root branch");
    let preset = prompt_preset("synthetic.core.reviewed-nonce-isolation");
    core.upsert_prompt_preset(&preset, None)
        .expect("save reviewed nonce prompt preset");
    let request = PromptPlanRequest {
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        expected_head: None,
        user_text: USER_TEXT_CANARY.to_owned(),
        generation_target: target.clone(),
        prompt_preset_id: Some(preset.id.clone()),
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };

    let preview_a = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: NONCE_A,
            },
        )
        .expect("resolve reviewed preview with nonce A");
    let preview_b = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: NONCE_B,
            },
        )
        .expect("resolve reviewed preview with nonce B");
    assert_ne!(
        preview_a.generation_attempt_id, preview_b.generation_attempt_id,
        "rotating only the caller nonce must allocate a new durable attempt"
    );
    assert_eq!(preview_a.plan, preview_b.plan);
    assert_eq!(preview_a.effective_messages, preview_b.effective_messages);
    assert_eq!(preview_a.provider_request, preview_b.provider_request);
    assert_eq!(preview_a.applied_parameters, preview_b.applied_parameters);
    assert_eq!(preview_a.prompt_diff, preview_b.prompt_diff);

    let trace_a = core
        .explain_prompt_plan(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview_a.generation_attempt_id,
            },
            &preview_a.plan.plan_hash,
        )
        .expect("explain nonce A preview");
    let trace_b = core
        .explain_prompt_plan(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview_b.generation_attempt_id,
            },
            &preview_b.plan.plan_hash,
        )
        .expect("explain nonce B preview");
    assert_eq!(trace_a, trace_b);
    assert!(trace_a.session_seed.is_some());

    let other_conversation = core
        .create_conversation(
            &character_id,
            "Synthetic reviewed nonce cross-room rejection",
            ConversationMode::Chat,
        )
        .expect("create cross-room resume target");
    let other_branch = core
        .list_conversation_branches(&other_conversation.id)
        .expect("list cross-room branch")
        .into_iter()
        .next()
        .expect("cross-room root branch");
    let mut cross_room = request.clone();
    cross_room.conversation_id = other_conversation.id;
    cross_room.branch_id = other_branch.id;
    let mut changed_text = request.clone();
    changed_text.user_text = "A different caller-owned reviewed message".to_owned();
    let mut changed_target = request.clone();
    changed_target.generation_target = GenerationTarget {
        model_route_id: ModelRouteId::from("synthetic-reviewed-hijack-route"),
        generation_preset_id: "synthetic-reviewed-hijack-preset".into(),
    };
    for (case, mismatched_request) in [
        ("cross-room", cross_room),
        ("changed text", changed_text),
        ("changed target", changed_target),
    ] {
        let error = core
            .resolve_prompt_preview(
                &mismatched_request,
                GenerationOperationContext::Resume {
                    generation_attempt_id: &preview_a.generation_attempt_id,
                },
            )
            .expect_err("a reviewed resume cannot hijack another caller-owned request");
        assert_eq!(error.code, CoreErrorCode::InvalidInput, "{case}");
        assert!(error.recoverable, "{case}");
        assert!(
            error.message.contains("start a new generation operation"),
            "{case}: {}",
            error.message
        );
    }
    assert_eq!(generation_attempt_count(root.path()), 2);
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    for (label, encoded) in [
        (
            "prompt plan A",
            serde_json::to_string(&preview_a.plan).expect("serialize prompt plan A"),
        ),
        (
            "prompt trace A",
            serde_json::to_string(&trace_a).expect("serialize prompt trace A"),
        ),
        (
            "provider request A",
            serde_json::to_string(&preview_a.provider_request)
                .expect("serialize provider request A"),
        ),
        (
            "prompt plan B",
            serde_json::to_string(&preview_b.plan).expect("serialize prompt plan B"),
        ),
        (
            "prompt trace B",
            serde_json::to_string(&trace_b).expect("serialize prompt trace B"),
        ),
        (
            "provider request B",
            serde_json::to_string(&preview_b.provider_request)
                .expect("serialize provider request B"),
        ),
    ] {
        assert!(!encoded.contains(NONCE_A), "{label} leaked nonce A");
        assert!(!encoded.contains(NONCE_B), "{label} leaked nonce B");
    }

    let mut approved = request.clone();
    approved.expected_plan_hash = Some(preview_b.plan.plan_hash.clone());
    let generation_id = core
        .send_message_with_prompt_plan(
            &approved,
            &preview_b.generation_attempt_id,
            reviewed_provider_credential(&core),
        )
        .expect("send reviewed nonce B attempt");
    wait_for_generation(&core, &branch.id, &generation_id);
    let captured = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("capture reviewed nonce provider request");
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    provider.stop();
    let captured_body = request_body(&captured);
    let captured_json = serde_json::to_string(&captured_body).expect("serialize wire request");
    assert!(!captured_json.contains(NONCE_A));
    assert!(!captured_json.contains(NONCE_B));

    drop(core);
    let storage = Storage::open(root.path()).expect("open typed nonce-isolation storage");
    let attempt_a = storage
        .get_generation_attempt(&preview_a.generation_attempt_id)
        .expect("load nonce A attempt");
    let attempt_b = storage
        .get_generation_attempt(&preview_b.generation_attempt_id)
        .expect("load nonce B attempt");
    assert_ne!(attempt_a.input.operation_id, attempt_b.input.operation_id);
    assert_eq!(
        attempt_a.input.base_request_fingerprint_sha256,
        attempt_b.input.base_request_fingerprint_sha256
    );
    let mut nonce_free_input_a = attempt_a.input.clone();
    let mut nonce_free_input_b = attempt_b.input.clone();
    nonce_free_input_a.operation_id.clear();
    nonce_free_input_b.operation_id.clear();
    assert_eq!(
        nonce_free_input_a, nonce_free_input_b,
        "only the derived operation id may differ between nonce variants"
    );
    let generation = storage
        .get_generation(&generation_id)
        .expect("load reviewed nonce generation payload");
    let stored_plan = storage
        .get_generation_prompt_plan_by_generation(&generation_id)
        .expect("load reviewed nonce prompt snapshot");
    assert_eq!(stored_plan.random_seed, trace_a.session_seed);
    assert_eq!(stored_plan.random_seed, trace_b.session_seed);
    assert_eq!(stored_plan.provider_request.request.value, captured_body);

    for (label, encoded) in [
        (
            "stored attempt A input",
            serde_json::to_string(&attempt_a.input).expect("serialize attempt A input"),
        ),
        (
            "stored attempt B input",
            serde_json::to_string(&attempt_b.input).expect("serialize attempt B input"),
        ),
        (
            "stored prompt plan",
            serde_json::to_string(&stored_plan).expect("serialize stored prompt plan"),
        ),
        (
            "generation payload",
            serde_json::to_string(&generation).expect("serialize generation payload"),
        ),
    ] {
        assert!(!encoded.contains(NONCE_A), "{label} leaked nonce A");
        assert!(!encoded.contains(NONCE_B), "{label} leaked nonce B");
    }
    drop(storage);
    assert_tree_excludes(root.path(), NONCE_A.as_bytes());
    assert_tree_excludes(root.path(), NONCE_B.as_bytes());
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one end-to-end contract compares preview, durable snapshot, and provider payload"
)]
fn preview_send_provider_and_snapshot_share_one_hash_bound_plan() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_stoppable_provider();
    let target = provider_fixture(&core, &origin);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic prompt identity",
            lorepia_core::ConversationMode::Chat,
        )
        .expect("create prompt identity conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list prompt identity branch")
        .into_iter()
        .next()
        .expect("root branch");
    let preset = prompt_preset("synthetic.core.prompt-identity");
    let stored_preset = core
        .upsert_prompt_preset(&preset, None)
        .expect("save identity prompt preset");
    let request = PromptPlanRequest {
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        expected_head: None,
        user_text: USER_TEXT_CANARY.to_owned(),
        generation_target: target.clone(),
        prompt_preset_id: Some(preset.id.clone()),
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };

    let expert_preview = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: "prompt-identity-preview-v1",
            },
        )
        .expect("resolve expert prompt preview");
    let preview = core
        .render_prompt_preview(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &expert_preview.generation_attempt_id,
            },
        )
        .expect("render prompt preview");
    assert_eq!(expert_preview.plan, preview);
    assert_eq!(
        preview,
        core.render_prompt_preview(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &expert_preview.generation_attempt_id,
            },
        )
        .expect("repeat prompt preview")
    );
    let redacted_preview = serde_json::to_string(&preview).expect("serialize redacted preview");
    assert!(!redacted_preview.contains(USER_TEXT_CANARY));
    assert!(!redacted_preview.contains(CREDENTIAL_CANARY));

    let mut tampered = request.clone();
    tampered.expected_plan_hash = Some("00".repeat(32));
    let mismatch = core
        .send_message_with_prompt_plan(
            &tampered,
            &expert_preview.generation_attempt_id,
            reviewed_provider_credential(&core),
        )
        .expect_err("tampered reviewed hash must fail before send");
    assert_eq!(mismatch.code, CoreErrorCode::InvalidInput);
    assert!(
        core.list_branch_messages(&branch.id)
            .expect("messages after rejected send")
            .is_empty()
    );
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        generation_attempt_count(root.path()),
        1,
        "review and hash rejection must retain exactly one original attempt"
    );

    let connection_id = ProviderConnectionId::from("synthetic-orchestration-connection");
    let mut changed_connection = core
        .list_provider_connections()
        .expect("list provider connections before drift")
        .into_iter()
        .find(|candidate| candidate.id == connection_id)
        .expect("reviewed connection used by preview");
    changed_connection.timeout_seconds = 7;
    core.upsert_provider_connection(changed_connection)
        .expect("change connection timeout under the same connection id");

    let mut changed_route = core
        .list_model_routes(&connection_id)
        .expect("list model routes before drift")
        .into_iter()
        .find(|candidate| candidate.id == target.model_route_id)
        .expect("model route used by preview");
    changed_route.display_name = Some("Synthetic model after reviewed drift".to_owned());
    core.upsert_model_route(changed_route)
        .expect("change route metadata under the same route id");

    core.upsert_user_capability_override(CapabilityObservation {
        id: ObservationId::from("synthetic-reviewed-context-window-drift"),
        model_route_id: target.model_route_id.clone(),
        key: CapabilityKey::ContextWindow,
        value: CapabilityValue::Integer(16_384),
        status: SupportStatus::Verified,
        source: ObservationSource::UserOverride,
        confidence: Confidence::Low,
        observed_at: Utc::now(),
        expires_at: None,
        evidence_ref: None,
    })
    .expect("change capability input under the same route id");

    let mut changed_generation_preset = core
        .list_generation_presets(&target.model_route_id)
        .expect("list generation presets")
        .into_iter()
        .find(|candidate| candidate.id == target.generation_preset_id)
        .expect("generation preset used by preview");
    changed_generation_preset.values = vec![ParameterValue {
        parameter_id: ParameterId::from("temperature"),
        state: ParameterValueState::Explicit(ParameterLiteral::Number(0.25)),
    }];
    changed_generation_preset.updated_at = Utc::now();
    core.upsert_generation_preset(changed_generation_preset)
        .expect("change exact request-plan input under the same preset id");

    let mut stale_review = request.clone();
    stale_review.expected_plan_hash = Some(preview.plan_hash.clone());
    let drift = core
        .send_message_with_prompt_plan(
            &stale_review,
            &expert_preview.generation_attempt_id,
            reviewed_provider_credential(&core),
        )
        .expect_err("provider mapping drift must fail before reviewed send");
    assert_eq!(drift.code, CoreErrorCode::InvalidInput);
    assert!(drift.recoverable);
    assert!(drift.message.contains("new generation operation"));
    assert!(
        core.list_branch_messages(&branch.id)
            .expect("messages after rejected provider mapping drift")
            .is_empty()
    );
    assert!(matches!(
        requests.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        generation_attempt_count(root.path()),
        1,
        "drift rejection must not synthesize a replacement attempt"
    );

    let fresh_expert_preview = core
        .resolve_prompt_preview(
            &request,
            GenerationOperationContext::New {
                operation_nonce: "prompt-identity-preview-v2",
            },
        )
        .expect("render fresh expert execution preview");
    let fresh_preview = &fresh_expert_preview.plan;
    assert_ne!(
        fresh_preview.plan_hash, preview.plan_hash,
        "request-plan input changes must alter the composite execution hash"
    );
    assert_ne!(
        fresh_expert_preview.generation_attempt_id, expert_preview.generation_attempt_id,
        "an explicit new nonce must create a newly sealed attempt"
    );
    assert_eq!(
        generation_attempt_count(root.path()),
        2,
        "only an explicit new nonce may add the fresh attempt"
    );
    assert_eq!(
        fresh_expert_preview.effective_messages, expert_preview.effective_messages,
        "provider parameter changes must not rewrite effective prompt messages"
    );
    let mut approved = request;
    approved.expected_plan_hash = Some(fresh_preview.plan_hash.clone());
    let generation_id = core
        .send_message_with_prompt_plan(
            &approved,
            &fresh_expert_preview.generation_attempt_id,
            reviewed_provider_credential(&core),
        )
        .expect("send reviewed prompt plan");
    wait_for_generation(&core, &branch.id, &generation_id);
    let captured = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("capture exact provider request");
    provider.stop();
    let captured_text = String::from_utf8_lossy(&captured);
    assert!(captured_text.to_ascii_lowercase().contains(&format!(
        "authorization: bearer {}",
        CREDENTIAL_CANARY.to_ascii_lowercase()
    )));
    let wire_body = request_body(&captured);
    let wire_messages = wire_body
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .expect("OpenAI-compatible messages array");
    assert_eq!(fresh_preview.provider_messages.len(), wire_messages.len());
    assert_eq!(
        wire_body
            .get("temperature")
            .and_then(serde_json::Value::as_f64),
        Some(0.25)
    );
    assert!(wire_messages.iter().any(|message| {
        message.get("role").and_then(serde_json::Value::as_str) == Some("user")
            && message.get("content").and_then(serde_json::Value::as_str) == Some(USER_TEXT_CANARY)
    }));

    let snapshot = core
        .get_generation_prompt_plan(&generation_id)
        .expect("load immutable generation prompt plan");
    assert_eq!(snapshot.id, fresh_preview.plan_id);
    assert_eq!(snapshot.generation_id, generation_id);
    assert_eq!(snapshot.prompt_preset_id, preset.id);
    assert_eq!(fresh_preview.prompt_preset_revision, stored_preset.revision);
    assert!(!snapshot.prompt_preset_revision_id.is_empty());
    assert_eq!(
        snapshot.prompt_preset_revision_id,
        fresh_preview.prompt_preset_revision_id
    );
    assert_eq!(
        snapshot.model_route_id.as_ref(),
        Some(&target.model_route_id)
    );
    assert_eq!(
        snapshot.generation_preset_id.as_ref(),
        Some(&target.generation_preset_id)
    );
    assert_eq!(
        snapshot
            .plan
            .value
            .get("plan_hash")
            .and_then(serde_json::Value::as_str),
        Some(snapshot.plan_sha256.as_str())
    );
    let resolved: ResolvedPromptPlan =
        serde_json::from_value(snapshot.plan.value.clone()).expect("decode stored resolved plan");
    verify_resolved_prompt_plan(&resolved).expect("verify stored resolved plan");
    assert_eq!(snapshot.plan_sha256, fresh_preview.neutral_plan_hash);
    assert_eq!(snapshot.plan_sha256, resolved.plan_hash);
    assert_eq!(snapshot.input_fingerprint_sha256, fresh_preview.plan_hash);
    assert_eq!(resolved.effective_messages.len(), wire_messages.len());
    assert_eq!(snapshot.provider_request.request.value, wire_body);
    let serialized_snapshot =
        serde_json::to_string(&snapshot).expect("serialize generation prompt plan");
    assert!(!serialized_snapshot.contains(CREDENTIAL_CANARY));

    let mut preset_v2 = preset;
    "Synthetic prompt identity v2".clone_into(&mut preset_v2.name);
    preset_v2.metadata.updated_at = timestamp() + chrono::Duration::seconds(1);
    core.upsert_prompt_preset(&preset_v2, Some(stored_preset.revision))
        .expect("update prompt preset after generation");
    assert_eq!(
        core.get_generation_prompt_plan(&generation_id)
            .expect("snapshot survives preset update"),
        snapshot
    );
    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen Core");
    assert_eq!(
        reopened
            .get_generation_prompt_plan(&generation_id)
            .expect("snapshot survives Core reopen"),
        snapshot
    );
    drop(reopened);
    assert_tree_excludes(root.path(), CREDENTIAL_CANARY.as_bytes());
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the stale-revision fixture must prove every generation-side table rolls back together"
)]
fn stale_knowledge_revision_rejects_the_atomic_generation_append() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic stale knowledge",
            ConversationMode::Chat,
        )
        .expect("create stale-knowledge conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list stale-knowledge branch")
        .into_iter()
        .next()
        .expect("root branch");
    let preset = prompt_preset("synthetic.core.stale-knowledge-preset");
    let stored_preset = core
        .upsert_prompt_preset(&preset, None)
        .expect("save stale-knowledge prompt preset");
    let prompt_preset_revision_id = stored_preset
        .revision_id
        .clone()
        .expect("prompt preset immutable revision id");
    let mut book = knowledge_book();
    let first_book = core
        .upsert_knowledge_book(&book, None)
        .expect("save first knowledge revision");
    let stale_book_revision_id = first_book
        .revision_id
        .clone()
        .expect("first knowledge immutable revision id");

    let generation_id = GenerationId("generation-stale-knowledge".to_owned());
    let mut user = Message::user_after(
        conversation.id.clone(),
        None,
        "Synthetic stale knowledge request",
    );
    user.id = lorepia_domain::MessageId("message-stale-knowledge-user".to_owned());
    let assistant = Message::pending_assistant(
        conversation.id.clone(),
        user.id.clone(),
        generation_id.clone(),
    );
    let resolved = resolve_prompt_plan(&PromptResolveRequest {
        preset: preset.clone(),
        context: PromptResolutionContext {
            conversation_id: conversation.id.clone(),
            branch_id: branch.id.clone(),
            character: CharacterPromptContent {
                character_id: character_id.clone(),
                name: "Ari".to_owned(),
                aliases: Vec::new(),
                description: "Entirely synthetic test character.".to_owned(),
                personality: String::new(),
                scenario: String::new(),
                first_message: String::new(),
                dialogue_examples: Vec::new(),
                system_instruction: String::new(),
                post_history_instruction: String::new(),
                alternate_greetings: Vec::new(),
                knowledge_book_ids: Vec::new(),
                asset_ids: Vec::new(),
            },
            persona: None,
            user_name: "Synthetic User".to_owned(),
            messages: vec![PromptConversationMessage {
                id: user.id.clone(),
                branch_id: branch.id.clone(),
                role: PromptMessageRole::User,
                content: user.content.clone(),
                turn_index: 1,
            }],
            latest_user_message_id: user.id.clone(),
            selected_knowledge: Vec::new(),
            selected_memory: Vec::new(),
            summary_boundaries: Vec::new(),
            conversation_summary: None,
            author_note: None,
            group_context: None,
            variables: VariableMap::default(),
            slots: Vec::new(),
            current_date: "2026-08-03".to_owned(),
            current_time: "12:00".to_owned(),
            supported_capabilities: Vec::new(),
            session_seed: Some(7),
            context_snapshot: None,
        },
        provider: ProviderPromptContract {
            supported_roles: vec![
                ProviderMessageRole::System,
                ProviderMessageRole::User,
                ProviderMessageRole::Assistant,
            ],
            provider_default_role: ProviderMessageRole::User,
            unsupported_role_policy: UnsupportedRolePolicy::MapDeveloperToSystem,
            supports_explicit_cache: false,
            max_cache_boundaries: 0,
        },
        generation_preset_id: None,
        max_context_tokens: 2_048,
        reserved_output_tokens: 256,
    })
    .expect("resolve stale-knowledge prompt");
    verify_resolved_prompt_plan(&resolved).expect("verify stale-knowledge prompt");
    let generation = GenerationRecord {
        id: generation_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        user_message_id: user.id.clone(),
        assistant_message_id: Some(assistant.id.clone()),
        mode: ConversationMode::Chat,
        model: "synthetic-storage-provider".to_owned(),
        model_route_id: None,
        generation_preset_id: None,
        provider_family: Some(ApiFamily::OpenAiChatCompletions),
        status: GenerationStatus::Running,
        input_tokens: None,
        cached_read_tokens: None,
        cached_write_tokens: None,
        output_tokens: None,
        reasoning_tokens: None,
        tool_tokens: None,
        provider_raw_summary: None,
        opaque_reasoning_state: Vec::new(),
        error_code: None,
        started_at: assistant.created_at,
        finished_at: None,
    };
    let plan_value = serde_json::to_value(&resolved).expect("encode resolved prompt");
    let prompt_plan = GenerationPromptPlanRecord {
        id: "plan-stale-knowledge".to_owned(),
        generation_id: generation_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        head_message_id: None,
        latest_user_message_id: user.id.clone(),
        prompt_preset_id: preset.id,
        prompt_preset_revision_id,
        model_route_id: None,
        generation_preset_id: None,
        task_profile_revision_id: None,
        random_seed: resolved.trace.session_seed,
        tokenizer_id: "utf8_bytes_div_4_v1".to_owned(),
        tokenizer_version: "1".to_owned(),
        plan: VersionedJson {
            schema_version: resolved.schema_version,
            value: plan_value,
        },
        plan_sha256: resolved.plan_hash.clone(),
        input_fingerprint_sha256: "55".repeat(32),
        context_limit_tokens: resolved.trace.max_context_tokens,
        estimated_input_tokens: resolved.trace.estimated_input_tokens,
        reserved_output_tokens: resolved.trace.reserved_output_tokens,
        final_input_tokens: resolved.trace.estimated_input_tokens,
        cacheable_prefix_tokens: 0,
        provider_request: ProviderRequestSnapshotRecord {
            id: "provider-request-stale-knowledge".to_owned(),
            api_family: ApiFamily::OpenAiChatCompletions,
            request_schema_version: 1,
            request: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({
                    "model": "synthetic-storage-provider",
                    "messages": [{"role": "user", "content": user.content.clone()}]
                }),
            },
            mapping_diagnostics: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({"fixture": "stale-knowledge"}),
            },
            created_at: assistant.created_at,
        },
        created_at: assistant.created_at,
    };
    let stale_log = KnowledgeActivationLog {
        id: "knowledge-log-stale-revision".to_owned(),
        book_id: book.id.clone(),
        book_revision_id: stale_book_revision_id,
        entry_id: book.entries[0].id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
        selected: true,
        reasons: vec![KnowledgeActivationReason::Always],
        estimated_tokens: 5,
        exclusion_reason: None,
        created_at: timestamp(),
    };

    "Synthetic Core knowledge v2".clone_into(&mut book.name);
    let second_book = core
        .upsert_knowledge_book(&book, Some(first_book.revision))
        .expect("advance knowledge book after prompt resolution");
    assert_ne!(
        second_book.revision_id.as_ref(),
        Some(&stale_log.book_revision_id)
    );
    drop(core);

    let storage = Storage::open(root.path()).expect("open storage for atomic append");
    let error = storage
        .append_generation_with_prompt_plan(
            &branch.id,
            None,
            &user,
            &assistant,
            &generation,
            &prompt_plan,
            &[stale_log],
        )
        .expect_err("stale knowledge revision must reject the whole append");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        storage
            .list_branch_messages(&branch.id)
            .expect("branch after stale append")
            .is_empty()
    );
    assert_eq!(
        storage
            .get_generation(&generation_id)
            .expect_err("generation row must roll back")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        storage
            .get_generation_prompt_plan_by_generation(&generation_id)
            .expect_err("prompt plan row must roll back")
            .code,
        CoreErrorCode::NotFound
    );
    let stats = storage
        .orchestration_stats()
        .expect("orchestration row counts after stale append");
    assert_eq!(stats.generations, 0);
    assert_eq!(stats.generation_prompt_plans, 0);
    assert_eq!(stats.knowledge_activation_logs, 0);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one app-scope activation must be materialized for both another room and a manual branch"
)]
fn app_scope_module_applies_in_a_second_room_and_on_a_manual_branch_first_send() {
    const MARKER: &str = "SYNTHETIC_APP_SCOPE_MODULE_MARKER_5D31";

    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_provider(2);
    let target = provider_fixture(&core, &origin);
    let activation_room = core
        .create_conversation(
            &character_id,
            "Synthetic module activation room",
            lorepia_core::ConversationMode::Chat,
        )
        .expect("create activation room");
    let activation_branch = core
        .list_conversation_branches(&activation_room.id)
        .expect("list activation-room branches")
        .into_iter()
        .next()
        .expect("activation-room root branch");
    let second_room = core
        .create_conversation(
            &character_id,
            "Synthetic second module room",
            lorepia_core::ConversationMode::Chat,
        )
        .expect("create second room before app activation");
    let second_branch = core
        .list_conversation_branches(&second_room.id)
        .expect("list second-room branches")
        .into_iter()
        .next()
        .expect("second-room root branch");

    let module = prompt_marker_module();
    activate_app_module(
        &core,
        &module,
        ContentModuleRuntimeTarget {
            conversation_id: activation_room.id.clone(),
            branch_id: activation_branch.id.clone(),
        },
        "synthetic.core.module.prompt-marker.binding",
    );
    let manual_branch = core
        .create_conversation_branch(
            &activation_room.id,
            None,
            Some("Synthetic manual branch".to_owned()),
        )
        .expect("create manual branch after app activation");

    let manual_generation = core
        .send_message_to_branch_with_connection_credential(
            &activation_room.id,
            &manual_branch.id,
            None,
            lorepia_core::ConversationMode::Chat,
            "Synthetic first message on manual branch",
            GenerationOperationContext::New {
                operation_nonce: "module-manual-branch-first-turn-v1",
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("first send on manual branch must derive the app module plan");
    let manual_request = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured manual-branch provider request");
    wait_for_generation(&core, &manual_branch.id, &manual_generation);

    let second_generation = core
        .send_message_to_branch_with_connection_credential(
            &second_room.id,
            &second_branch.id,
            None,
            lorepia_core::ConversationMode::Chat,
            "Synthetic first message in second room",
            GenerationOperationContext::New {
                operation_nonce: "module-second-room-first-turn-v1",
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("second room must derive the app module plan");
    let second_request = requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured second-room provider request");
    wait_for_generation(&core, &second_branch.id, &second_generation);

    for (label, request) in [
        ("manual branch", manual_request),
        ("second room", second_request),
    ] {
        let request_json = serde_json::to_string(&request_body(&request))
            .expect("encode captured provider request");
        assert!(
            request_json.contains(MARKER),
            "{label} did not apply the exact app-scope prompt component"
        );
    }
    provider.join().expect("join synthetic provider");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the interaction checkpoint contract needs several source commits and one historical edit"
)]
fn historical_edit_fork_starts_from_the_interaction_checkpoint_at_the_fork_message() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_provider(4);
    let target = provider_fixture(&core, &origin);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic interaction checkpoint fork",
            lorepia_core::ConversationMode::Chat,
        )
        .expect("create interaction checkpoint conversation");
    let source_branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list interaction source branches")
        .into_iter()
        .next()
        .expect("interaction source branch");
    let (rule_set, _counter) = interaction_counter_rule_set();
    core.upsert_interaction_rule_set(&rule_set, None)
        .expect("save interaction checkpoint rules");
    let module = interaction_counter_module(&rule_set.id);
    activate_app_module(
        &core,
        &module,
        ContentModuleRuntimeTarget {
            conversation_id: conversation.id.clone(),
            branch_id: source_branch.id.clone(),
        },
        "synthetic.core.module.interaction-counter.binding",
    );

    let mut expected_head = None;
    for (text, operation_nonce) in [
        (
            "Synthetic interaction source turn one",
            "interaction-source-turn-one-v1",
        ),
        (
            "Synthetic interaction source turn two",
            "interaction-source-turn-two-v1",
        ),
        (
            "Synthetic interaction source turn three",
            "interaction-source-turn-three-v1",
        ),
    ] {
        let generation = core
            .send_message_to_branch_with_connection_credential(
                &conversation.id,
                &source_branch.id,
                expected_head.as_ref(),
                lorepia_core::ConversationMode::Chat,
                text,
                GenerationOperationContext::New { operation_nonce },
                &target,
                reviewed_provider_credential(&core),
            )
            .expect("send interaction source turn");
        wait_for_generation(&core, &source_branch.id, &generation);
        expected_head = core
            .list_branch_messages(&source_branch.id)
            .expect("source interaction messages")
            .last()
            .map(|message| message.id.clone());
    }
    for _ in 0..3 {
        requests
            .recv_timeout(Duration::from_secs(2))
            .expect("captured interaction source provider request");
    }
    assert_eq!(
        wait_for_interaction_visible_system_texts(&core, &conversation.id, &source_branch.id, 3,),
        vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
        "the source branch counter proves each committed turn's durable state"
    );
    let source_messages = core
        .list_branch_messages(&source_branch.id)
        .expect("complete interaction source lineage");
    assert_eq!(source_messages.len(), 6);

    let edited = core
        .edit_user_message_with_connection_credential(
            &conversation.id,
            &source_branch.id,
            source_messages.last().map(|message| &message.id),
            &source_messages[2].id,
            "Synthetic interaction replacement for turn two",
            GenerationOperationContext::New {
                operation_nonce: "interaction-historical-edit-turn-two-v1",
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("edit old turn using the historical interaction checkpoint");
    requests
        .recv_timeout(Duration::from_secs(2))
        .expect("captured interaction-fork provider request");
    wait_for_generation(&core, &edited.branch.id, &edited.generation_id);
    assert_eq!(
        wait_for_interaction_visible_system_texts(&core, &conversation.id, &edited.branch.id, 1,),
        vec!["2".to_owned()],
        "the child commit must increment pre-fork state 1, not source-head state 3 or default state 0"
    );
    assert_eq!(
        interaction_visible_system_texts(&core, &conversation.id, &source_branch.id),
        vec!["1".to_owned(), "2".to_owned(), "3".to_owned()],
        "creating the historical child must not mutate source interaction history"
    );
    provider.join().expect("join synthetic provider");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "one fixture proves diff, review hash, stale-state rejection, rollback, and share policy"
)]
fn module_revisions_diff_and_share_gate_are_durable() {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let conversation = core
        .open_conversation(&character_id)
        .expect("open module test conversation");
    let conversation_state = core
        .get_conversation_state(&conversation.id)
        .expect("module test conversation state");
    let runtime_target = ContentModuleRuntimeTarget {
        conversation_id: conversation.id,
        branch_id: conversation_state.active_branch_id,
    };
    let mut module = content_module();
    let first = core
        .upsert_content_module(&module, None)
        .expect("create content module");
    assert_eq!(first.revision, 1);

    "2.0.0".clone_into(&mut module.version);
    "Synthetic Core module v2".clone_into(&mut module.name);
    module.metadata.provenance.source_hash = Some("cd".repeat(32));
    let second = core
        .upsert_content_module(&module, Some(first.revision))
        .expect("update content module");
    assert_eq!(second.revision, 2);
    let diff = core
        .diff_content_module_revisions(&module.id, 1, 2)
        .expect("diff content module revisions");
    assert_eq!(diff.module_id, module.id);
    assert_eq!(diff.from_revision, 1);
    assert_eq!(diff.to_revision, 2);
    assert_ne!(diff.from_sha256, diff.to_sha256);
    assert!(diff.changed_paths.iter().any(|path| path == "/version"));

    let target_revision_id = ModuleRevisionId::from(
        first
            .revision_id
            .clone()
            .expect("first immutable module revision id"),
    );
    let current_revision_id = ModuleRevisionId::from(
        second
            .revision_id
            .clone()
            .expect("second immutable module revision id"),
    );
    let binding_id = ModuleBindingId::from("synthetic.core.module-binding");
    let activation_request = ContentModuleActivationRequest {
        runtime_target: runtime_target.clone(),
        expected_binding_revision: None,
        binding: ContentModuleBindingDraft {
            id: binding_id.clone(),
            module_id: module.id.clone(),
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
    let activation_review = core
        .review_content_module_activation(&activation_request)
        .expect("review content module activation");
    activation_review
        .verify()
        .expect("verify activation review");
    let activation_resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: activation_review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let activation_plan = core
        .resolve_content_module_activation(&activation_request, &activation_resolutions)
        .expect("resolve content module activation");
    activation_plan.verify().expect("verify activation plan");
    let activation_approval = ModuleActivationApproval {
        approval_id: "synthetic-core-module-activation".to_owned(),
        expected_review_sha256: activation_review.review_sha256,
        expected_plan_sha256: activation_plan.plan_sha256.clone(),
    };
    let activation_preflight = core
        .preflight_content_module_activation(
            &activation_request,
            &activation_resolutions,
            &activation_approval,
        )
        .expect("preflight reviewed content module activation");
    activation_preflight
        .verify()
        .expect("verify activation receipt preflight");
    assert_eq!(activation_preflight.resulting_state_revision, 1);
    assert!(
        core.list_content_module_bindings(&module.id)
            .expect("bindings after read-only activation preflight")
            .is_empty(),
        "receipt preflight must not create the binding"
    );
    let activation_receipt = core
        .activate_content_module(
            &activation_request,
            &activation_resolutions,
            &activation_approval,
        )
        .expect("activate reviewed content module");
    activation_receipt
        .verify()
        .expect("verify activation receipt");
    let recovered_activation_receipt = core
        .activate_content_module(
            &activation_request,
            &activation_resolutions,
            &activation_approval,
        )
        .expect("recover exact activation receipt after response loss");
    assert_eq!(
        recovered_activation_receipt, activation_receipt,
        "an exact retry must return the first durable receipt without a second mutation"
    );
    let conflicting_reuse = core
        .activate_content_module(
            &activation_request,
            &activation_resolutions,
            &ModuleActivationApproval {
                approval_id: "synthetic-core-module-conflicting-retry".to_owned(),
                expected_review_sha256: activation_approval.expected_review_sha256.clone(),
                expected_plan_sha256: activation_approval.expected_plan_sha256.clone(),
            },
        )
        .expect_err("an applied plan cannot be rebound to another approval id");
    assert_eq!(conflicting_reuse.code, CoreErrorCode::InvalidInput);
    let stored_binding = activation_receipt.binding;
    assert_eq!(stored_binding.value.id, binding_id);
    assert_eq!(stored_binding.value.revision_id, current_revision_id);
    assert!(stored_binding.value.enabled);
    assert!(stored_binding.value.approved);

    let review = core
        .review_content_module_rollback(&binding_id, &target_revision_id, None, &runtime_target)
        .expect("review content module rollback");
    review
        .rollback
        .verify()
        .expect("verify rollback review hash");
    review
        .activation
        .verify()
        .expect("verify rollback activation review");
    assert!(review.rollback.eligible);
    assert_eq!(
        review.rollback.expected_state_revision,
        stored_binding.revision
    );
    assert_eq!(review.rollback.current_revision_id, current_revision_id);
    assert_eq!(review.rollback.target_revision_id, target_revision_id);
    assert_ne!(
        review.rollback.current_source_sha256, review.rollback.target_source_sha256,
        "review must bind the exact current and target source hashes"
    );

    let wrong_hash = Sha256Digest::parse("ff".repeat(32)).expect("synthetic wrong review hash");
    let tampered = core
        .resolve_content_module_rollback(&ContentModuleRollbackResolutionRequest {
            runtime_target: runtime_target.clone(),
            binding_id: binding_id.clone(),
            target_revision_id: target_revision_id.clone(),
            target_package_import_approval_id: None,
            expected_state_revision: review.rollback.expected_state_revision,
            expected_rollback_review_sha256: wrong_hash,
            resolutions: ModuleMergeResolutionSet {
                expected_review_sha256: review.activation.review_sha256.clone(),
                resolutions: Vec::new(),
            },
        })
        .expect_err("wrong rollback review hash must fail");
    assert_eq!(tampered.code, CoreErrorCode::InvalidInput);
    let after_tamper = core
        .list_content_module_bindings(&module.id)
        .expect("binding after tampered rollback")
        .into_iter()
        .find(|candidate| candidate.value.id == binding_id)
        .expect("module binding remains");
    assert_eq!(after_tamper, stored_binding);

    "3.0.0".clone_into(&mut module.version);
    "Synthetic Core module v3".clone_into(&mut module.name);
    module.metadata.provenance.source_hash = Some("ef".repeat(32));
    let third = core
        .upsert_content_module(&module, Some(second.revision))
        .expect("advance the active module after rollback review");
    let stale_resolution = ContentModuleRollbackResolutionRequest {
        runtime_target: runtime_target.clone(),
        binding_id: binding_id.clone(),
        target_revision_id: target_revision_id.clone(),
        target_package_import_approval_id: None,
        expected_state_revision: review.rollback.expected_state_revision,
        expected_rollback_review_sha256: review.rollback.review_sha256.clone(),
        resolutions: ModuleMergeResolutionSet {
            expected_review_sha256: review.activation.review_sha256,
            resolutions: Vec::new(),
        },
    };
    let stale = core
        .resolve_content_module_rollback(&stale_resolution)
        .expect_err("stale rollback review must fail");
    assert_eq!(stale.code, CoreErrorCode::InvalidInput);
    let after_stale = core
        .list_content_module_bindings(&module.id)
        .expect("binding after stale rollback")
        .into_iter()
        .find(|candidate| candidate.value.id == binding_id)
        .expect("module binding remains after active revision drift");
    assert_eq!(after_stale, stored_binding);

    let drifted_review = core
        .review_content_module_rollback(&binding_id, &target_revision_id, None, &runtime_target)
        .expect("refresh content module rollback review");
    assert_ne!(
        drifted_review.rollback.review_sha256,
        review.rollback.review_sha256
    );
    assert_eq!(
        drifted_review.rollback.expected_state_revision,
        stored_binding.revision
    );
    assert!(!drifted_review.rollback.eligible);
    assert!(
        drifted_review
            .rollback
            .blockers
            .contains(&lorepia_core::ModuleRollbackBlocker::StaleBinding),
        "an active module edit must not silently change the approved binding revision"
    );
    let drifted_workspace = core
        .review_content_module_runtime_workspace(&runtime_target)
        .expect("review runtime workspace after active revision drift");
    let drifted_binding = drifted_workspace
        .bindings
        .iter()
        .find(|candidate| candidate.binding.id == binding_id)
        .expect("drifted binding remains visible for explicit reapproval");
    assert_eq!(
        drifted_binding.disposition,
        lorepia_core::ContentModuleRuntimeBindingDisposition::NeedsReapproval
    );
    assert_eq!(drifted_binding.approved_revision_id, current_revision_id);
    assert_eq!(
        drifted_binding.binding.revision_id,
        ModuleRevisionId::from(
            third
                .revision_id
                .clone()
                .expect("third immutable module revision id")
        ),
        "the workspace must distinguish the newly resolved revision from the last approved revision"
    );

    let reactivation_request = ContentModuleActivationRequest {
        runtime_target: runtime_target.clone(),
        expected_binding_revision: Some(stored_binding.revision),
        binding: ContentModuleBindingDraft {
            id: binding_id.clone(),
            module_id: module.id.clone(),
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
    let reactivation_review = core
        .review_content_module_activation(&reactivation_request)
        .expect("review explicit activation of the advanced module");
    let reactivation_resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: reactivation_review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let reactivation_plan = core
        .resolve_content_module_activation(&reactivation_request, &reactivation_resolutions)
        .expect("resolve explicit activation of the advanced module");
    let reactivated = core
        .activate_content_module(
            &reactivation_request,
            &reactivation_resolutions,
            &ModuleActivationApproval {
                approval_id: "synthetic-core-module-reactivation".to_owned(),
                expected_review_sha256: reactivation_review.review_sha256,
                expected_plan_sha256: reactivation_plan.plan_sha256,
            },
        )
        .expect("activate the advanced module with a fresh approval");
    assert_eq!(reactivated.binding.revision, stored_binding.revision + 1);
    assert_eq!(
        reactivated.binding.value.revision_id,
        ModuleRevisionId::from(
            third
                .revision_id
                .clone()
                .expect("third immutable module revision id")
        )
    );

    let fresh_review = core
        .review_content_module_rollback(&binding_id, &target_revision_id, None, &runtime_target)
        .expect("review rollback from the freshly approved advanced revision");
    assert!(fresh_review.rollback.eligible);
    assert_eq!(
        fresh_review.rollback.expected_state_revision,
        reactivated.binding.revision
    );
    let fresh_resolution = ContentModuleRollbackResolutionRequest {
        runtime_target: runtime_target.clone(),
        binding_id: binding_id.clone(),
        target_revision_id: target_revision_id.clone(),
        target_package_import_approval_id: None,
        expected_state_revision: fresh_review.rollback.expected_state_revision,
        expected_rollback_review_sha256: fresh_review.rollback.review_sha256.clone(),
        resolutions: ModuleMergeResolutionSet {
            expected_review_sha256: fresh_review.activation.review_sha256.clone(),
            resolutions: Vec::new(),
        },
    };
    let fresh_plan = core
        .resolve_content_module_rollback(&fresh_resolution)
        .expect("resolve fresh rollback");
    fresh_plan.verify().expect("verify combined rollback plan");
    let rollback_approval = ModuleActivationApproval {
        approval_id: "synthetic-core-module-rollback".to_owned(),
        expected_review_sha256: fresh_review.activation.review_sha256,
        expected_plan_sha256: fresh_plan.activation.plan_sha256.clone(),
    };
    let rollback_apply_request = ContentModuleRollbackApplyRequest {
        resolution: fresh_resolution,
        expected_rollback_plan_sha256: fresh_plan.rollback.plan_sha256,
        activation_approval: rollback_approval,
    };
    let rollback_preflight = core
        .preflight_content_module_rollback(&rollback_apply_request)
        .expect("preflight exact reviewed rollback");
    rollback_preflight
        .verify()
        .expect("verify rollback receipt preflight");
    assert_eq!(
        rollback_preflight.resulting_state_revision,
        reactivated.binding.revision + 1
    );
    let binding_after_preflight = core
        .list_content_module_bindings(&module.id)
        .expect("binding after read-only rollback preflight")
        .into_iter()
        .find(|candidate| candidate.value.id == binding_id)
        .expect("binding remains after rollback preflight");
    assert_eq!(binding_after_preflight, reactivated.binding);
    let rolled_back = core
        .apply_content_module_rollback(&rollback_apply_request)
        .expect("apply exact reviewed rollback");
    rolled_back.verify().expect("verify rollback receipt");
    assert_eq!(
        rolled_back.binding.revision,
        fresh_review.rollback.expected_state_revision + 1
    );
    assert_eq!(rolled_back.binding.value.revision_id, target_revision_id);

    drop(core);
    let core = Core::open(CoreConfig::new(root.path()))
        .expect("reopen Core after losing the rollback response");
    let recovered_rollback = core
        .apply_content_module_rollback(&rollback_apply_request)
        .expect("recover exact rollback receipt after restart and response loss");
    assert_eq!(
        recovered_rollback, rolled_back,
        "an exact rollback retry must return the first durable receipt without a second mutation"
    );

    let retry_wrong_hash =
        Sha256Digest::parse("ee".repeat(32)).expect("synthetic wrong rollback retry hash");
    let mut wrong_rollback_plan = rollback_apply_request.clone();
    wrong_rollback_plan.expected_rollback_plan_sha256 = retry_wrong_hash.clone();
    let rejected_plan = core
        .apply_content_module_rollback(&wrong_rollback_plan)
        .expect_err("a different rollback plan hash must not recover the receipt");
    assert_eq!(rejected_plan.code, CoreErrorCode::InvalidInput);

    let mut wrong_rollback_review = rollback_apply_request.clone();
    wrong_rollback_review
        .resolution
        .expected_rollback_review_sha256 = retry_wrong_hash;
    let rejected_review = core
        .apply_content_module_rollback(&wrong_rollback_review)
        .expect_err("a different rollback review hash must not recover the receipt");
    assert_eq!(rejected_review.code, CoreErrorCode::InvalidInput);

    let mut wrong_approval_id = rollback_apply_request.clone();
    wrong_approval_id.activation_approval.approval_id =
        "synthetic-core-module-rollback-conflicting-retry".to_owned();
    let rejected_approval = core
        .apply_content_module_rollback(&wrong_approval_id)
        .expect_err("a different approval id must not recover the receipt");
    assert_eq!(rejected_approval.code, CoreErrorCode::InvalidInput);

    let deactivation_request = ContentModuleDeactivationRequest {
        runtime_target: runtime_target.clone(),
        binding_id: binding_id.clone(),
    };
    let stale_deactivation_review = core
        .review_content_module_deactivation(&deactivation_request)
        .expect("review exact module deactivation");
    stale_deactivation_review
        .verify()
        .expect("verify deactivation review hash");

    let deactivation_drift_request = ContentModuleActivationRequest {
        runtime_target: runtime_target.clone(),
        expected_binding_revision: Some(rolled_back.binding.revision),
        binding: ContentModuleBindingDraft {
            id: binding_id.clone(),
            module_id: module.id.clone(),
            scope: ModuleScope::App,
            target_id: None,
            conversation_id: None,
            priority: 1,
            resolution_mode: ModuleRevisionResolutionMode::Pinned,
            pinned_revision_id: Some(target_revision_id.clone()),
            package_import_approval_id: None,
            variable_overrides: VariableMap::default(),
        },
    };
    let drift_review = core
        .review_content_module_activation(&deactivation_drift_request)
        .expect("review binding mutation after deactivation review");
    let drift_resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: drift_review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let drift_plan = core
        .resolve_content_module_activation(&deactivation_drift_request, &drift_resolutions)
        .expect("resolve binding mutation after deactivation review");
    let drifted_binding = core
        .activate_content_module(
            &deactivation_drift_request,
            &drift_resolutions,
            &ModuleActivationApproval {
                approval_id: "synthetic-core-module-before-deactivation".to_owned(),
                expected_review_sha256: drift_review.review_sha256,
                expected_plan_sha256: drift_plan.plan_sha256,
            },
        )
        .expect("mutate binding before stale deactivation apply");
    let stale_deactivation = core
        .deactivate_content_module(
            &deactivation_request,
            &stale_deactivation_review.review_sha256,
        )
        .expect_err("stale deactivation review must not delete the changed binding");
    assert_eq!(stale_deactivation.code, CoreErrorCode::InvalidInput);
    let fresh_deactivation_review = core
        .review_content_module_deactivation(&deactivation_request)
        .expect("refresh exact deactivation review");
    assert_eq!(
        fresh_deactivation_review.expected_binding_revision,
        drifted_binding.binding.revision
    );
    let deactivated = core
        .deactivate_content_module(
            &deactivation_request,
            &fresh_deactivation_review.review_sha256,
        )
        .expect("deactivate the freshly reviewed binding");
    deactivated.verify().expect("verify deactivation receipt");
    assert_eq!(
        deactivated.binding.revision,
        fresh_deactivation_review.expected_binding_revision + 1
    );

    let gate = core
        .evaluate_content_module_share_gate(&module.id)
        .expect("evaluate local share gate");
    assert!(gate.local_use_allowed);
    assert!(!gate.sharing_allowed);
    assert!(gate.reasons.iter().any(|reason| reason.contains("license")));
    assert!(
        gate.reasons
            .iter()
            .any(|reason| reason.contains("redistribution"))
    );
    drop(core);
    let reopened =
        Core::open(CoreConfig::new(root.path())).expect("reopen Core after deactivation");
    let restarted_workspace = reopened
        .review_content_module_runtime_workspace(&runtime_target)
        .expect("review module workspace after restart");
    assert!(
        restarted_workspace
            .bindings
            .iter()
            .all(|binding| binding.binding.id != binding_id),
        "a deactivated binding must remain absent after restart"
    );
}
