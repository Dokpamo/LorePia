mod support;

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
use support::is_live_owner_lock_sharing_violation;
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

fn interaction_knowledge_rule_set(entry_id: &KnowledgeEntryId) -> InteractionRuleSet {
    InteractionRuleSet {
        id: InteractionRuleSetId::from("synthetic.core.interaction.knowledge-activation"),
        name: "Synthetic interaction knowledge activation".to_owned(),
        schema_version: 1,
        rules: vec![InteractionRule {
            id: InteractionRuleId::from("synthetic.core.interaction.knowledge-activation.commit"),
            name: "Activate knowledge after one committed message".to_owned(),
            enabled: true,
            imported_author_enabled: false,
            event: InteractionEvent::MessageCommitted,
            condition: None,
            actions: vec![InteractionAction::ActivateKnowledge {
                entry_id: entry_id.clone(),
            }],
            priority: 0,
            stop_after_match: false,
            provenance: provenance(
                SourceKind::UserCreated,
                "synthetic.core.interaction.knowledge-activation.commit",
            ),
        }],
        max_actions_per_event: 8,
        provenance: provenance(
            SourceKind::UserCreated,
            "synthetic.core.interaction.knowledge-activation",
        ),
    }
}

fn interaction_knowledge_module(
    book_id: &KnowledgeBookId,
    rule_set_id: &InteractionRuleSetId,
) -> ContentModule {
    let mut module = content_module();
    module.id = ContentModuleId::from("synthetic.core.module.interaction-knowledge");
    "Synthetic interaction knowledge module".clone_into(&mut module.name);
    module.knowledge_book_ids = vec![book_id.clone()];
    module.interaction_rule_set_ids = vec![rule_set_id.clone()];
    module.imported_components_enabled = true;
    module.required_capabilities = vec![
        ContentCapability::Knowledge,
        ContentCapability::DeclarativeInteractions,
    ];
    module.metadata.provenance = provenance(
        SourceKind::UserCreated,
        "synthetic.core.module.interaction-knowledge",
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

fn reactivate_app_module(
    core: &Core,
    module: &ContentModule,
    runtime_target: ContentModuleRuntimeTarget,
    binding_id: &ModuleBindingId,
) {
    let stored_binding = core
        .list_content_module_bindings(&module.id)
        .expect("list app-scope module bindings before reactivation")
        .into_iter()
        .find(|binding| binding.value.id == *binding_id && binding.deleted_at.is_none())
        .expect("active app-scope module binding before reactivation");
    let request = ContentModuleActivationRequest {
        runtime_target,
        expected_binding_revision: Some(stored_binding.revision),
        binding: ContentModuleBindingDraft {
            id: binding_id.clone(),
            module_id: module.id.clone(),
            scope: ModuleScope::App,
            target_id: None,
            conversation_id: None,
            priority: stored_binding.value.priority,
            resolution_mode: ModuleRevisionResolutionMode::Active,
            pinned_revision_id: None,
            package_import_approval_id: None,
            variable_overrides: stored_binding.value.variable_overrides,
        },
    };
    let review = core
        .review_content_module_activation(&request)
        .expect("review advanced app-scope module");
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let plan = core
        .resolve_content_module_activation(&request, &resolutions)
        .expect("resolve advanced app-scope module");
    let receipt = core
        .activate_content_module(
            &request,
            &resolutions,
            &ModuleActivationApproval {
                approval_id: format!("{}-advanced-approval", binding_id.as_str()),
                expected_review_sha256: review.review_sha256,
                expected_plan_sha256: plan.plan_sha256,
            },
        )
        .expect("activate advanced app-scope module");
    receipt.verify().expect("verify advanced module receipt");
}

fn active_interaction_knowledge_bindings(
    root: &Path,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
) -> Vec<(String, String)> {
    let connection = rusqlite::Connection::open(active_database_path(root))
        .expect("open interaction knowledge database");
    let mut statement = connection
        .prepare(
            "SELECT knowledge.book_revision_id, knowledge.entry_id
             FROM interaction_state_knowledge AS knowledge
             JOIN interaction_state AS state
               ON state.id = knowledge.interaction_state_id
             WHERE state.conversation_id = ?1
               AND state.branch_id = ?2
               AND knowledge.enabled = 1
             ORDER BY knowledge.book_revision_id, knowledge.entry_id",
        )
        .expect("prepare interaction knowledge query");
    statement
        .query_map(
            rusqlite::params![conversation_id.0.as_str(), branch_id.0.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query active interaction knowledge")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect active interaction knowledge")
}

fn wait_for_active_interaction_knowledge_bindings(
    core: &Core,
    root: &Path,
    conversation_id: &lorepia_core::ConversationId,
    branch_id: &lorepia_core::ConversationBranchId,
    expected: &[(String, String)],
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        core.list_interaction_effect_history(conversation_id, branch_id, None, 1_024)
            .expect("drain interaction lifecycle before reading knowledge bindings");
        let bindings = active_interaction_knowledge_bindings(root, conversation_id, branch_id);
        if bindings == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "interaction knowledge did not converge to {expected:?}; got {bindings:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
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
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(error) if is_live_owner_lock_sharing_violation(&path, &error) => continue,
                Err(error) => panic!("read Core data-root file {}: {error}", path.display()),
            };
            assert!(
                !bytes.windows(needle.len()).any(|window| window == needle),
                "sensitive canary was persisted in {}",
                path.display()
            );
        }
    }
}

include!("../src/orchestration/tests/documents.rs");
include!("../src/orchestration/tests/memory.rs");
include!("../src/orchestration/tests/generation.rs");
include!("../src/orchestration/tests/modules.rs");
