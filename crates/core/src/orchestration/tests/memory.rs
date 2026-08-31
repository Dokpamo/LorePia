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
            &current_memory.conversation_id,
            &current_memory.branch_id,
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
            &current_memory.conversation_id,
            &current_memory.branch_id,
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
        .delete_memory_record(
            &current_memory.conversation_id,
            &current_memory.branch_id,
            &current_memory.id,
            edited.revision,
        )
        .expect("soft-delete edited memory");
    assert_eq!(deleted.revision, 3);
    assert!(deleted.deleted_at.is_some());
    assert_eq!(
        core.get_memory_record(
            &current_memory.conversation_id,
            &current_memory.branch_id,
            &current_memory.id,
        )
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

