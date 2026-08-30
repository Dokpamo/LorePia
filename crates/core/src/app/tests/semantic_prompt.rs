struct SemanticReplayFixture {
    root: tempfile::TempDir,
    core: Core,
    connection_id: ProviderConnectionId,
    book: KnowledgeBook,
    book_revision: u64,
    prompt_preset: PromptPreset,
    prompt_preset_revision: u64,
    request: crate::PromptPlanRequest,
}

fn semantic_replay_probability_sample(
    book_id: &KnowledgeBookId,
    entry_id: &KnowledgeEntryId,
    seed: u64,
) -> u16 {
    let seed_bytes = seed.to_be_bytes();
    let mut hasher = Sha256::new();
    for value in [
        b"lorepia-knowledge-probability-v1".as_slice(),
        book_id.as_str().as_bytes(),
        entry_id.as_str().as_bytes(),
        seed_bytes.as_slice(),
    ] {
        hasher.update(
            u64::try_from(value.len())
                .expect("field length fits u64")
                .to_be_bytes(),
        );
        hasher.update(value);
    }
    let digest = hasher.finalize();
    u16::from_be_bytes([digest[0], digest[1]]) % 10_000
}

fn semantic_replay_entry(
    book_id: &KnowledgeBookId,
    id: KnowledgeEntryId,
    name: &str,
    content: &str,
    activation: ActivationRule,
    probability: u16,
) -> KnowledgeEntry {
    KnowledgeEntry {
        id,
        book_id: book_id.clone(),
        name: name.to_owned(),
        content: content.to_owned(),
        enabled: true,
        activation,
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
        activation_probability_basis_points: probability,
        provenance: prompt_attempt_test_provenance("synthetic.semantic-replay.entry"),
    }
}

fn create_semantic_replay_book(core: &Core) -> (KnowledgeBook, u64) {
    let book_id = KnowledgeBookId::from("synthetic.semantic-replay.book");
    let semantic_entry_id = KnowledgeEntryId::from("synthetic.semantic-replay.cobalt-moon");
    let book = KnowledgeBook {
        id: book_id.clone(),
        name: "Synthetic semantic replay knowledge".to_owned(),
        schema_version: 1,
        entries: vec![semantic_replay_entry(
            &book_id,
            semantic_entry_id,
            "Cobalt moon",
            "SYNTHETIC_SEMANTIC_COBALT_MOON_41B7 cobalt moon",
            ActivationRule::Semantic {
                threshold: 0.1,
                top_k: 8,
            },
            10_000,
        )],
        scan_depth: 8,
        token_budget: TokenBudget { max_tokens: 512 },
        recursive: false,
        max_recursion_depth: 0,
        provenance: prompt_attempt_test_provenance("synthetic.semantic-replay.book"),
    };
    let stored = core
        .upsert_knowledge_book(&book, None)
        .expect("save initial semantic replay book");
    (book, stored.revision)
}

fn semantic_replay_knowledge_block() -> PromptBlock {
    PromptBlock {
        id: PromptBlockId::from("synthetic.semantic-replay.knowledge-block"),
        name: "Synthetic semantic knowledge".to_owned(),
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
            max_tokens: Some(512),
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::ReduceKnowledgeEntries,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: prompt_attempt_test_provenance("synthetic.semantic-replay.knowledge-block"),
    }
}

fn create_semantic_replay_prompt_preset(
    core: &Core,
    book_id: &KnowledgeBookId,
    now: DateTime<Utc>,
) -> (PromptPreset, u64) {
    let mut preset = lorepia_orchestration::default_prompt_preset(
        lorepia_domain::PromptPresetId::from("synthetic.semantic-replay.preset"),
        "Synthetic semantic replay preset",
        PresetMetadata {
            description: "Synthetic semantic/probability replay fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: prompt_attempt_test_provenance("synthetic.semantic-replay.preset"),
            created_at: now,
            updated_at: now,
            local_override_of: None,
        },
    );
    for block in &mut preset.blocks {
        block.provenance = prompt_attempt_test_provenance(block.id.as_str());
    }
    preset.blocks.push(semantic_replay_knowledge_block());
    preset.blocks.sort_by_key(|block| block.placement_zone);
    preset.knowledge_book_ids.push(book_id.clone());
    let stored = core
        .upsert_prompt_preset(&preset, None)
        .expect("save initial semantic replay preset");
    (preset, stored.revision)
}

fn bind_semantic_replay_prompt_preset(
    core: &Core,
    conversation_id: &ConversationId,
    prompt_preset_id: &lorepia_domain::PromptPresetId,
    now: DateTime<Utc>,
) {
    core.bind_prompt_preset(
        &PromptPresetBinding {
            id: "synthetic.semantic-replay.binding".to_owned(),
            prompt_preset_id: prompt_preset_id.clone(),
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
    .expect("bind semantic replay preset");
}

fn create_semantic_replay_fixture() -> SemanticReplayFixture {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "Semantic attempt replay",
            ConversationMode::Chat,
        )
        .expect("create semantic replay conversation");
    let branch_id = core
        .get_conversation_state(&conversation.id)
        .expect("semantic replay state")
        .active_branch_id;
    let (template, route) = create_built_in_public_route(
        &core,
        "openai-responses-v1",
        "/v1",
        "gpt-semantic-replay-fixture",
    );
    let generation_preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
        .expect("save semantic replay generation preset");
    let (book, book_revision) = create_semantic_replay_book(&core);
    let now = Utc::now();
    let (prompt_preset, prompt_preset_revision) =
        create_semantic_replay_prompt_preset(&core, &book.id, now);
    bind_semantic_replay_prompt_preset(&core, &conversation.id, &prompt_preset.id, now);
    let request = crate::PromptPlanRequest {
        conversation_id: conversation.id,
        branch_id,
        expected_head: None,
        user_text: "Tell me about the cobalt moon".to_owned(),
        generation_target: GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: generation_preset.id,
        },
        prompt_preset_id: Some(prompt_preset.id.clone()),
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };
    SemanticReplayFixture {
        root,
        core,
        connection_id: route.connection_id,
        book,
        book_revision,
        prompt_preset,
        prompt_preset_revision,
        request,
    }
}

fn add_semantic_replay_probability_entry(
    fixture: &mut SemanticReplayFixture,
    session_seed: u64,
) -> bool {
    let probabilistic_entry_id = (0_u32..100_000)
        .map(|index| KnowledgeEntryId::from(format!("synthetic.semantic-replay.roll-{index}")))
        .find(|entry_id| {
            (semantic_replay_probability_sample(&fixture.book.id, entry_id, session_seed) < 5_000)
                != (semantic_replay_probability_sample(&fixture.book.id, entry_id, 0) < 5_000)
        })
        .expect("find entry distinguished from the legacy zero seed");
    let expected_probability_selection =
        semantic_replay_probability_sample(&fixture.book.id, &probabilistic_entry_id, session_seed)
            < 5_000;
    assert_ne!(
        expected_probability_selection,
        semantic_replay_probability_sample(&fixture.book.id, &probabilistic_entry_id, 0) < 5_000,
    );
    let entry = semantic_replay_entry(
        &fixture.book.id,
        probabilistic_entry_id,
        "Attempt-owned probability",
        "SYNTHETIC_ATTEMPT_PROBABILITY_92CF",
        ActivationRule::Always,
        5_000,
    );
    fixture.book.entries.push(entry);
    let stored_book = fixture
        .core
        .upsert_knowledge_book(&fixture.book, Some(fixture.book_revision))
        .expect("save probabilistic semantic replay revision");
    fixture.book_revision = stored_book.revision;
    assert_eq!(fixture.book_revision, 2);
    fixture.prompt_preset.metadata.updated_at = Utc::now();
    let stored_preset = fixture
        .core
        .upsert_prompt_preset(&fixture.prompt_preset, Some(fixture.prompt_preset_revision))
        .expect("seal revised knowledge dependency");
    fixture.prompt_preset_revision = stored_preset.revision;
    assert_eq!(fixture.prompt_preset_revision, 2);
    expected_probability_selection
}

fn prepare_final_semantic_replay_preview(
    fixture: &mut SemanticReplayFixture,
) -> (crate::ExpertPromptPreview, u64) {
    let operation_target = GenerationActionTargetIdentity::GenerationTarget {
        model_route_id: fixture.request.generation_target.model_route_id.clone(),
        generation_preset_id: fixture
            .request
            .generation_target
            .generation_preset_id
            .clone(),
    };
    let base_request_fingerprint_sha256 =
        same_branch_generation_semantic_fingerprint(&SameBranchGenerationAttemptIdentity {
            conversation_id: &fixture.request.conversation_id,
            branch_id: &fixture.request.branch_id,
            expected_head: fixture.request.expected_head.as_ref(),
            text: &fixture.request.user_text,
            operation_context: GenerationOperationContext::New {
                operation_nonce: "semantic-final-replay-v1",
            },
            target: &operation_target,
            temperature: None,
            max_output_tokens: None,
            prompt_preset_id: fixture.request.prompt_preset_id.as_ref(),
            variable_overrides: &fixture.request.variable_overrides,
        })
        .expect("derive semantic replay base request fingerprint");
    let session_seed = reviewed_prompt_session_seed(&base_request_fingerprint_sha256);
    assert_ne!(
        session_seed, 0,
        "attempt seed cannot be the legacy constant"
    );
    let expected_probability_selection =
        add_semantic_replay_probability_entry(fixture, session_seed);
    let preview = fixture
        .core
        .resolve_prompt_preview(
            &fixture.request,
            GenerationOperationContext::New {
                operation_nonce: "semantic-final-replay-v1",
            },
        )
        .expect("resolve final semantic replay preview");
    let attempt = fixture
        .core
        .inner
        .storage
        .get_generation_attempt(&preview.generation_attempt_id)
        .expect("load final semantic replay attempt");
    assert_eq!(
        attempt.input.base_request_fingerprint_sha256, base_request_fingerprint_sha256,
        "attempt must persist the nonce-free semantic seed authority"
    );
    let preview_text = preview
        .effective_messages
        .iter()
        .map(|message| message.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(preview_text.contains("SYNTHETIC_SEMANTIC_COBALT_MOON_41B7"));
    assert_eq!(
        preview_text.contains("SYNTHETIC_ATTEMPT_PROBABILITY_92CF"),
        expected_probability_selection,
    );
    (preview, session_seed)
}

fn assert_semantic_replay_restart_and_send(
    root: &Path,
    request: &crate::PromptPlanRequest,
    connection_id: &ProviderConnectionId,
    credential_authority: &ProviderCredentialAccessAuthority,
    preview: &crate::ExpertPromptPreview,
    session_seed: u64,
) {
    let reopened = Core::open(CoreConfig::new(root)).expect("reopen semantic replay core");
    let reopened_preview = reopened
        .resolve_prompt_preview(
            request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview.generation_attempt_id,
            },
        )
        .expect("resolve semantic replay preview after restart");
    assert_eq!(&reopened_preview, preview);

    let mut reviewed = request.clone();
    reviewed.expected_plan_hash = Some(reopened_preview.plan.plan_hash.clone());
    let generation_id = reopened
        .send_message_with_prompt_plan(
            &reviewed,
            &reopened_preview.generation_attempt_id,
            ConnectionBoundCredential::new_with_access_authority(
                connection_id.clone(),
                Some("synthetic-semantic-replay-credential".to_owned()),
                credential_authority.clone(),
            ),
        )
        .expect("send exact restarted semantic replay preview");
    let stored_plan = reopened
        .get_generation_prompt_plan(&generation_id)
        .expect("load sent semantic replay plan");
    assert_eq!(stored_plan.id, reopened_preview.plan.plan_id);
    assert_eq!(stored_plan.random_seed, Some(session_seed));
    assert_eq!(
        stored_plan.provider_request.mapping_diagnostics.value["knowledge_semantic_evidence"][0]["source"]
            ["kind"],
        "lexical_v1",
    );
    let sent_plan: ResolvedPromptPlan =
        serde_json::from_value(stored_plan.plan.value).expect("decode sent semantic replay plan");
    assert_eq!(
        sent_plan
            .effective_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
        reopened_preview
            .effective_messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>(),
    );
}

#[test]
fn semantic_knowledge_and_probability_replay_across_restart_preview_and_send() {
    let mut fixture = create_semantic_replay_fixture();
    let (preview, session_seed) = prepare_final_semantic_replay_preview(&mut fixture);
    let credential_authority =
        install_provider_credential_authority(&fixture.core, &fixture.connection_id);
    drop(fixture.core);
    assert_semantic_replay_restart_and_send(
        fixture.root.path(),
        &fixture.request,
        &fixture.connection_id,
        &credential_authority,
        &preview,
        session_seed,
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn provider_semantic_knowledge_reuses_one_durable_query_for_preview_and_send() {
    let (root, core, character) = imported_core();
    let conversation = core
        .create_conversation(
            &character.id,
            "Provider semantic replay",
            ConversationMode::Chat,
        )
        .expect("create provider semantic conversation");
    let first_generation = core
        .send_message_with_provider(
            &conversation.id,
            "durable first turn",
            "static-provider".to_owned(),
            None,
            Arc::new(StaticProvider::new("durable assistant anchor")),
        )
        .expect("create durable semantic anchor");
    wait_for_generation_status(&core, &first_generation, GenerationStatus::Complete);
    wait_for_generation_registry_to_drain(&core);
    let branch_id = core
        .get_conversation_state(&conversation.id)
        .expect("provider semantic state")
        .active_branch_id;
    let durable_messages = core
        .list_branch_messages(&branch_id)
        .expect("provider semantic anchor messages");
    let durable_head = durable_messages
        .last()
        .expect("durable assistant anchor")
        .id
        .clone();

    let (template, route) = create_built_in_public_route(
        &core,
        "openai-responses-v1",
        "/v1",
        "text-embedding-provider-semantic-fixture",
    );
    let generation_preset = core
        .upsert_generation_preset(initial_generation_preset(&route.id, &template, Utc::now()))
        .expect("save provider semantic generation preset");
    let summary_task_id = TaskProfileId::from("synthetic.provider-semantic.summary-task");
    core.upsert_task_profile(
        &TaskProfile {
            id: summary_task_id.clone(),
            kind: AuxiliaryTaskKind::MemorySummary,
            route_id: route.id.clone(),
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
    .expect("save provider semantic summary task");
    let embedding_task_id = TaskProfileId::from("synthetic.provider-semantic.embedding-task");
    let embedding_task = core
        .upsert_task_profile(
            &TaskProfile {
                id: embedding_task_id.clone(),
                kind: AuxiliaryTaskKind::MemoryEmbedding,
                route_id: route.id.clone(),
                generation_preset_id: generation_preset.id.clone(),
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
        .expect("save provider semantic embedding task");
    let memory_profile_id = MemoryProfileId::from("synthetic.provider-semantic.memory-profile");
    let memory_profile = core
        .upsert_memory_profile(
            &MemoryProfile {
                id: memory_profile_id.clone(),
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
                summary_schema: SummarySchemaId::from("synthetic.provider-semantic.summary-schema"),
                provenance: prompt_attempt_test_provenance(
                    "synthetic.provider-semantic.memory-profile",
                ),
            },
            None,
        )
        .expect("save provider semantic memory profile");

    let book_id = KnowledgeBookId::from("synthetic.provider-semantic.book");
    let entry_id = KnowledgeEntryId::from("synthetic.provider-semantic.entry");
    let book = KnowledgeBook {
        id: book_id.clone(),
        name: "Synthetic provider semantic knowledge".to_owned(),
        schema_version: 1,
        entries: vec![KnowledgeEntry {
            id: entry_id.clone(),
            book_id: book_id.clone(),
            name: "Provider-only vector match".to_owned(),
            content: "SYNTHETIC_PROVIDER_SEMANTIC_VECTOR_31AD".to_owned(),
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
            provenance: prompt_attempt_test_provenance("synthetic.provider-semantic.entry"),
        }],
        scan_depth: 8,
        token_budget: TokenBudget { max_tokens: 128 },
        recursive: false,
        max_recursion_depth: 0,
        provenance: prompt_attempt_test_provenance("synthetic.provider-semantic.book"),
    };
    let stored_book = core
        .upsert_knowledge_book(&book, None)
        .expect("save provider semantic book");
    let book_revision_id = stored_book
        .revision_id
        .clone()
        .expect("provider semantic book revision id");

    let now = Utc::now();
    let mut prompt_preset = lorepia_orchestration::default_prompt_preset(
        lorepia_domain::PromptPresetId::from("synthetic.provider-semantic.preset"),
        "Synthetic provider semantic preset",
        PresetMetadata {
            description: "Synthetic provider semantic fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: prompt_attempt_test_provenance("synthetic.provider-semantic.preset"),
            created_at: now,
            updated_at: now,
            local_override_of: None,
        },
    );
    for block in &mut prompt_preset.blocks {
        block.provenance = prompt_attempt_test_provenance(block.id.as_str());
    }
    prompt_preset.blocks.push(PromptBlock {
        id: PromptBlockId::from("synthetic.provider-semantic.knowledge-block"),
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
        provenance: prompt_attempt_test_provenance("synthetic.provider-semantic.knowledge-block"),
    });
    prompt_preset
        .blocks
        .sort_by_key(|block| block.placement_zone);
    prompt_preset.knowledge_book_ids.push(book_id);
    prompt_preset.memory_profile_id = Some(memory_profile_id.clone());
    core.upsert_prompt_preset(&prompt_preset, None)
        .expect("save provider semantic prompt preset");
    core.bind_prompt_preset(
        &PromptPresetBinding {
            id: "synthetic.provider-semantic.binding".to_owned(),
            prompt_preset_id: prompt_preset.id.clone(),
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
    .expect("bind provider semantic prompt preset");

    let connection = core
        .inner
        .storage
        .get_provider_connection(&route.connection_id)
        .expect("load provider semantic connection");
    let credential_authority = install_provider_credential_authority(&core, &connection.id);
    let embedding_provider = AdapterRegistry::new()
        .build_embedding_provider_for_route(&template, &connection, &route, 3)
        .expect("build provider semantic embedding contract");
    let embedding_contract = embedding_provider.contract();
    let vector_space_sha256 = embedding_contract.vector_space_sha256();
    assert_eq!(
        embedding_contract
            .execution_sha256(EmbeddingPurpose::RetrievalQuery)
            .len(),
        64
    );
    let task_profile_revision_id = embedding_task
        .revision_id
        .expect("provider semantic task revision id");
    let memory_profile_revision_id = memory_profile
        .revision_id
        .expect("provider semantic memory revision id");
    let query_text = {
        let mut texts = durable_messages
            .iter()
            .map(|message| message.content.clone())
            .collect::<Vec<_>>();
        texts.push("opaque provider vector query".to_owned());
        texts
            .iter()
            .rev()
            .filter(|text| !text.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    let query_sha256 = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&("lorepia.memory-query.v1", &query_text))
                .expect("encode provider semantic query")
        )
    );
    let intent_digest = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                "lorepia.memory-query-embedding-intent.v1",
                memory_profile_id.as_str(),
                memory_profile_revision_id.as_str(),
                task_profile_revision_id.as_str(),
                conversation.id.0.as_str(),
                branch_id.0.as_str(),
                durable_head.0.as_str(),
                durable_head.0.as_str(),
                query_sha256.as_str(),
                vector_space_sha256.as_str(),
                route.id.as_str(),
                3_u32,
            ))
            .expect("encode provider semantic intent")
        )
    );
    let intent = MemoryQueryEmbeddingIntent {
        id: format!("memory-query-embedding-{intent_digest}"),
        idempotency_key: format!("memory-query-embedding:v1:{intent_digest}"),
        memory_profile_id,
        memory_profile_revision_id,
        task_profile_revision_id: task_profile_revision_id.clone(),
        conversation_id: conversation.id.clone(),
        branch_id: branch_id.clone(),
        source_start_message_id: durable_head.clone(),
        source_end_message_id: durable_head.clone(),
        query_sha256,
        vector_space_sha256: vector_space_sha256.clone(),
        model_route_id: route.id.clone(),
        dimensions: 3,
        created_at: now,
    };
    let request = crate::PromptPlanRequest {
        conversation_id: conversation.id.clone(),
        branch_id: branch_id.clone(),
        expected_head: Some(durable_head),
        user_text: "opaque provider vector query".to_owned(),
        generation_target: GenerationTarget {
            model_route_id: route.id.clone(),
            generation_preset_id: generation_preset.id.clone(),
        },
        prompt_preset_id: Some(prompt_preset.id.clone()),
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };
    let lexical_fallback_preview = core
        .resolve_prompt_preview_async(
            &request,
            new_test_generation_operation("provider-semantic-preview-v1"),
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("fall back lexically before exact knowledge vectors exist");
    assert!(
        lexical_fallback_preview
            .effective_messages
            .iter()
            .all(|message| {
                !message
                    .content
                    .contains("SYNTHETIC_PROVIDER_SEMANTIC_VECTOR_31AD")
            })
    );
    assert_eq!(
        core.inner
            .storage
            .get_memory_query_embedding(&intent.id)
            .expect_err("lexical fallback must not enqueue a provider query")
            .code,
        CoreErrorCode::NotFound,
    );
    let queued = core
        .inner
        .storage
        .enqueue_memory_query_embedding(&intent)
        .expect("enqueue provider semantic query");
    let running = core
        .inner
        .storage
        .claim_memory_query_embedding(&intent.id, queued.entry.revision, now)
        .expect("claim provider semantic query");
    let completed = core
        .inner
        .storage
        .complete_memory_query_embedding(&intent.id, running.revision, &[1.0, 0.0, 0.0], now)
        .expect("complete provider semantic query");
    assert_eq!(completed.revision, 3);
    core.inner
        .storage
        .save_knowledge_embedding(&KnowledgeEmbeddingWrite {
            id: "synthetic-provider-semantic-embedding".to_owned(),
            book_revision_id,
            entry_id,
            task_profile_revision_id,
            model_route_id: route.id.clone(),
            dimensions: 3,
            vector_space_sha256,
            values: vec![1.0, 0.0, 0.0],
            created_at: now,
        })
        .expect("save provider semantic knowledge embedding");

    let preview = core
        .resolve_prompt_preview_async(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &lexical_fallback_preview.generation_attempt_id,
            },
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("resolve provider semantic preview from durable query");
    assert_eq!(
        preview.generation_attempt_id,
        lexical_fallback_preview.generation_attempt_id,
    );
    assert!(preview.effective_messages.iter().any(|message| {
        message
            .content
            .contains("SYNTHETIC_PROVIDER_SEMANTIC_VECTOR_31AD")
    }));
    drop(core);
    let core = Core::open(CoreConfig::new(root.path()))
        .expect("reopen provider semantic core before reviewed send");
    assert_eq!(
        core.resolve_prompt_preview_async(
            &request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &preview.generation_attempt_id,
            },
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("repeat provider semantic preview"),
        preview,
    );

    let mut reviewed = request;
    reviewed.expected_plan_hash = Some(preview.plan.plan_hash.clone());
    let generation_id = core
        .send_message_with_prompt_plan_async(
            &reviewed,
            &preview.generation_attempt_id,
            ConnectionBoundCredential::new_with_access_authority(
                connection.id.clone(),
                Some("synthetic-provider-semantic-credential".to_owned()),
                credential_authority.clone(),
            ),
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("send provider semantic reviewed plan");
    let stored_plan = core
        .get_generation_prompt_plan(&generation_id)
        .expect("load provider semantic sent plan");
    assert_eq!(stored_plan.id, preview.plan.plan_id);
    assert_eq!(
        stored_plan.provider_request.mapping_diagnostics.value["knowledge_semantic_evidence"][0]["source"]
            ["kind"],
        "provider_embedding_v1",
    );
    let reused_query = core
        .inner
        .storage
        .get_memory_query_embedding(&intent.id)
        .expect("load reused provider semantic query");
    assert_eq!(reused_query.revision, 3);
    assert_eq!(reused_query.attempts, 1);

    let root_conversation = core
        .create_conversation(
            &character.id,
            "Provider semantic lexical root fallback",
            ConversationMode::Chat,
        )
        .expect("create provider semantic root conversation");
    let root_branch_id = core
        .get_conversation_state(&root_conversation.id)
        .expect("provider semantic root state")
        .active_branch_id;
    core.bind_prompt_preset(
        &PromptPresetBinding {
            id: "synthetic.provider-semantic.root-binding".to_owned(),
            prompt_preset_id: prompt_preset.id.clone(),
            scope: ModuleScope::Conversation,
            target_id: Some(root_conversation.id.0.clone()),
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
    .expect("bind provider semantic root prompt preset");
    let root_request = crate::PromptPlanRequest {
        conversation_id: root_conversation.id,
        branch_id: root_branch_id,
        expected_head: None,
        user_text: "lexically unrelated first turn".to_owned(),
        generation_target: GenerationTarget {
            model_route_id: route.id,
            generation_preset_id: generation_preset.id,
        },
        prompt_preset_id: Some(prompt_preset.id),
        variable_overrides: VariableMap::default(),
        expected_plan_hash: None,
    };
    let root_preview = core
        .resolve_prompt_preview_async(
            &root_request,
            new_test_generation_operation("provider-semantic-root-preview-v1"),
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("resolve provider semantic root preview with lexical fallback");
    assert!(root_preview.effective_messages.iter().all(|message| {
        !message
            .content
            .contains("SYNTHETIC_PROVIDER_SEMANTIC_VECTOR_31AD")
    }));
    assert_eq!(
        core.resolve_prompt_preview_async(
            &root_request,
            GenerationOperationContext::Resume {
                generation_attempt_id: &root_preview.generation_attempt_id,
            },
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("repeat provider semantic root preview"),
        root_preview,
    );
    let mut reviewed_root = root_request;
    reviewed_root.expected_plan_hash = Some(root_preview.plan.plan_hash.clone());
    let root_generation_id = core
        .send_message_with_prompt_plan_async(
            &reviewed_root,
            &root_preview.generation_attempt_id,
            ConnectionBoundCredential::new_with_access_authority(
                connection.id,
                Some("synthetic-provider-semantic-root-credential".to_owned()),
                credential_authority,
            ),
            &RejectingTaskCredentialBroker,
            watch::channel(false).1,
        )
        .await
        .expect("send exact provider semantic root lexical preview");
    let root_plan = core
        .get_generation_prompt_plan(&root_generation_id)
        .expect("load provider semantic root plan");
    assert_eq!(root_plan.id, root_preview.plan.plan_id);
    assert_eq!(
        root_plan.provider_request.mapping_diagnostics.value["knowledge_semantic_evidence"][0]["source"]
            ["kind"],
        "lexical_v1",
    );
}
