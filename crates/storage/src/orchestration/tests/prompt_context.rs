
fn prompt_context_test_preset(now: DateTime<Utc>) -> PromptPreset {
    let mut preset = built_in_prompt_presets()[0].clone();
    preset.id = PromptPresetId::from("prompt-context-append-preset");
    preset.name = "Prompt context append preset".to_owned();
    preset.metadata = PresetMetadata {
        description: "Synthetic prompt context append fixture".to_owned(),
        tags: Vec::new(),
        provenance: Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: Some("prompt-context-append-preset".to_owned()),
            source_hash: Some(sha256_hex(b"prompt-context-append-preset")),
            author: None,
            license: None,
            imported_at: None,
        },
        created_at: now,
        updated_at: now,
        local_override_of: None,
    };
    preset
}

fn prompt_context_append_fixture() -> PromptContextAppendFixture {
    let root = tempfile::tempdir().expect("temporary prompt context root");
    let storage = Storage::open(root.path()).expect("open prompt context storage");
    let now = Utc::now();
    let source_hash = sha256_hex(b"prompt-context-character-source");
    let conversation_id = ConversationId("prompt-context-conversation".to_owned());
    let branch_id = ConversationBranchId("prompt-context-branch".to_owned());
    storage
        .connection()
        .expect("prompt context database")
        .execute_batch(&format!(
            "INSERT INTO content_sources
                     (sha256, relative_path, size_bytes, created_at)
                 VALUES ('{source_hash}', 'sha256/source', 1, '{now}');
                 INSERT INTO characters
                     (id, name, description, source_hash, created_at)
                 VALUES ('prompt-context-character', 'Synthetic Character', '',
                         '{source_hash}', '{now}');
                 INSERT INTO conversations
                     (id, character_id, title, created_at, updated_at)
                 VALUES ('{conversation_id}', 'prompt-context-character',
                         'Prompt context append', '{now}', '{now}');
                 INSERT INTO conversation_branches
                     (id, conversation_id, title, fork_message_id,
                      head_message_id, created_at, updated_at)
                 VALUES ('{branch_id}', '{conversation_id}', NULL, NULL, NULL,
                         '{now}', '{now}');",
            conversation_id = conversation_id.0.as_str(),
            branch_id = branch_id.0.as_str(),
        ))
        .expect("create prompt context owner rows");
    let local_user_id = storage
        .load_settings()
        .expect("load local prompt identity")
        .local_user_id;
    let preset = prompt_context_test_preset(now);
    storage
        .save_prompt_preset(&preset, None)
        .expect("save prompt context preset");
    PromptContextAppendFixture {
        _root: root,
        storage,
        now,
        conversation_id,
        branch_id,
        preset,
        local_user_id,
    }
}

fn prompt_context_test_binding(fixture: &PromptContextAppendFixture) -> PromptPresetBinding {
    PromptPresetBinding {
        id: "prompt-context-binding".to_owned(),
        prompt_preset_id: fixture.preset.id.clone(),
        scope: ModuleScope::Branch,
        target_id: Some(fixture.branch_id.0.clone()),
        conversation_id: Some(fixture.conversation_id.clone()),
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
        user_name_override: Some("Synthetic room user".to_owned()),
        author_note: Some("Synthetic room author".to_owned()),
        group_context: Some("Synthetic room group".to_owned()),
        template_slots: vec![TemplateSlot {
            name: "tone".to_owned(),
            value: "Synthetic room tone".to_owned(),
        }],
        created_at: fixture.now,
        updated_at: fixture.now,
    }
}

fn require_prompt_context_test_record(
    fixture: &PromptContextAppendFixture,
    record: &GenerationPromptPlanRecord,
) -> CoreResult<()> {
    let mut connection = fixture.storage.connection()?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    require_generation_prompt_context_snapshot_transaction(
        &transaction,
        record,
        &fixture.branch_id,
        None,
        &fixture.local_user_id,
    )
}

fn prompt_context_test_snapshot(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    local_user_id: &LocalUserId,
    binding: Option<PromptContextBindingEvidence>,
) -> PromptContextSnapshotV1 {
    let mut context_snapshot = PromptContextSnapshotV1 {
        schema_version: 1,
        conversation_id: conversation_id.clone(),
        source_branch_id: branch_id.clone(),
        context_head_message_id: None,
        local_user_id_sha256: prompt_local_user_id_sha256(local_user_id),
        binding,
        persona: None,
        conversation_summary_id: None,
        summaries: Vec::new(),
        snapshot_sha256: String::new(),
    };
    context_snapshot.snapshot_sha256 =
        prompt_context_snapshot_sha256(&context_snapshot).expect("hash prompt context");
    context_snapshot
}

fn prompt_context_test_resolution_context(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    local_user_id: &LocalUserId,
    binding: Option<PromptContextBindingEvidence>,
) -> lorepia_domain::PromptResolutionContext {
    let hypothetical_user_id = MessageId("prompt-context-hypothetical-user".to_owned());
    lorepia_domain::PromptResolutionContext {
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
        character: lorepia_domain::CharacterPromptContent {
            character_id: "prompt-context-character".to_owned(),
            name: "Synthetic Character".to_owned(),
            aliases: Vec::new(),
            description: "Synthetic append-time prompt context character".to_owned(),
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
        user_name: "Local user".to_owned(),
        messages: vec![lorepia_domain::PromptConversationMessage {
            id: hypothetical_user_id.clone(),
            branch_id: branch_id.clone(),
            role: lorepia_domain::PromptMessageRole::User,
            content: "Synthetic append-time request".to_owned(),
            turn_index: 0,
        }],
        latest_user_message_id: hypothetical_user_id,
        selected_knowledge: Vec::new(),
        selected_memory: Vec::new(),
        summary_boundaries: Vec::new(),
        conversation_summary: None,
        author_note: None,
        group_context: None,
        variables: VariableMap::default(),
        slots: Vec::new(),
        current_date: "2026-08-09".to_owned(),
        current_time: "12:00".to_owned(),
        supported_capabilities: Vec::new(),
        session_seed: Some(1),
        context_snapshot: Some(prompt_context_test_snapshot(
            conversation_id,
            branch_id,
            local_user_id,
            binding,
        )),
    }
}

fn prompt_context_test_plan(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    preset: &PromptPreset,
    local_user_id: &LocalUserId,
    now: DateTime<Utc>,
    binding: Option<PromptContextBindingEvidence>,
) -> GenerationPromptPlanRecord {
    let hypothetical_user_id = MessageId("prompt-context-hypothetical-user".to_owned());
    let resolved =
        lorepia_orchestration::resolve_prompt_plan(&lorepia_domain::PromptResolveRequest {
            preset: preset.clone(),
            context: prompt_context_test_resolution_context(
                conversation_id,
                branch_id,
                local_user_id,
                binding,
            ),
            provider: lorepia_domain::ProviderPromptContract {
                supported_roles: vec![
                    ProviderMessageRole::System,
                    ProviderMessageRole::User,
                    ProviderMessageRole::Assistant,
                ],
                provider_default_role: ProviderMessageRole::User,
                unsupported_role_policy:
                    lorepia_domain::UnsupportedRolePolicy::MapDeveloperToSystem,
                supports_explicit_cache: false,
                max_cache_boundaries: 0,
            },
            generation_preset_id: None,
            max_context_tokens: 8_192,
            reserved_output_tokens: 1_024,
        })
        .expect("resolve prompt context test plan");
    let plan_sha256 = resolved.plan_hash.clone();
    GenerationPromptPlanRecord {
        id: "prompt-context-plan".to_owned(),
        generation_id: GenerationId("prompt-context-generation".to_owned()),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
        head_message_id: None,
        latest_user_message_id: hypothetical_user_id,
        prompt_preset_id: preset.id.clone(),
        prompt_preset_revision_id: "prompt-context-preset-revision".to_owned(),
        model_route_id: None,
        generation_preset_id: None,
        task_profile_revision_id: None,
        random_seed: Some(1),
        tokenizer_id: "synthetic-tokenizer".to_owned(),
        tokenizer_version: "1".to_owned(),
        plan: VersionedJson {
            schema_version: resolved.schema_version,
            value: serde_json::to_value(resolved).expect("encode prompt context test plan"),
        },
        plan_sha256: plan_sha256.clone(),
        input_fingerprint_sha256: plan_sha256,
        context_limit_tokens: 8_192,
        estimated_input_tokens: 1,
        reserved_output_tokens: 1_024,
        final_input_tokens: 1,
        cacheable_prefix_tokens: 0,
        provider_request: ProviderRequestSnapshotRecord {
            id: "prompt-context-provider-snapshot".to_owned(),
            api_family: ApiFamily::OpenAiChatCompletions,
            request_schema_version: 1,
            request: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({}),
            },
            mapping_diagnostics: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({}),
            },
            created_at: now,
        },
        created_at: now,
    }
}

#[test]
fn prompt_context_append_recheck_rejects_new_effective_binding() {
    let fixture = prompt_context_append_fixture();
    let record = prompt_context_test_plan(
        &fixture.conversation_id,
        &fixture.branch_id,
        &fixture.preset,
        &fixture.local_user_id,
        fixture.now,
        None,
    );
    require_prompt_context_test_record(&fixture, &record)
        .expect("unchanged prompt context must pass");
    let mut binding = prompt_context_test_binding(&fixture);
    binding.id = "prompt-context-late-binding".to_owned();
    fixture
        .storage
        .save_prompt_preset_binding(&binding, None)
        .expect("save late prompt binding");
    let error = require_prompt_context_test_record(&fixture, &record)
        .expect_err("late effective binding must invalidate prompt context");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
}

#[test]
fn prompt_context_append_recheck_rejects_existing_binding_source_change() {
    let fixture = prompt_context_append_fixture();
    let mut binding = prompt_context_test_binding(&fixture);
    let stored = fixture
        .storage
        .save_prompt_preset_binding(&binding, None)
        .expect("save initial prompt binding");
    let record = prompt_context_test_plan(
        &fixture.conversation_id,
        &fixture.branch_id,
        &fixture.preset,
        &fixture.local_user_id,
        fixture.now,
        Some(PromptContextBindingEvidence {
            binding_id: stored.value.id.clone(),
            binding_revision: stored.revision,
            document_sha256: stored
                .value
                .canonical_document_sha256()
                .expect("hash initial prompt binding"),
        }),
    );
    require_prompt_context_test_record(&fixture, &record)
        .expect("exact prompt binding must pass append recheck");

    binding.author_note = Some("Changed room author".to_owned());
    binding.updated_at += chrono::Duration::seconds(1);
    fixture
        .storage
        .save_prompt_preset_binding(&binding, Some(stored.revision))
        .expect("save changed prompt binding source");
    let error = require_prompt_context_test_record(&fixture, &record)
        .expect_err("binding source drift must invalidate the old attempt");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.recoverable);
}
