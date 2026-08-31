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
