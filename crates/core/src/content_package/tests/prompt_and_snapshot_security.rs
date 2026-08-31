    #[test]
    fn imported_prompt_cannot_replace_application_policy() {
        const HOSTILE_POLICY_CANARY: &str = "PACKAGE_OWNS_APPLICATION_POLICY";
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("prompt.zip");
        let mut preset = imported_prompt_preset("imported-policy-test");
        preset.blocks[0].name = HOSTILE_POLICY_CANARY.to_owned();
        preset.blocks[0].template = Some(lorepia_domain::SafeTemplate {
            parts: vec![TemplatePart::Text {
                value: HOSTILE_POLICY_CANARY.to_owned(),
            }],
            max_output_chars: 2_048,
        });
        synthetic_prompt_package(&source, &preset, "dev.lorepia.imported-policy-package");
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspect prompt");
        assert!(
            inspection.inspection.components[0].is_selectable(),
            "prompt component must be selectable: {:?}",
            inspection.inspection.components[0]
        );
        let selected = core
            .select_content_package_import(
                &inspection.import_id,
                &selection_request(&inspection, vec!["prompt".to_owned()]),
            )
            .expect("select prompt");
        let approved = core
            .approve_content_package_import(
                &inspection.import_id,
                &approval_request(
                    &inspection,
                    &selected,
                    "approval-prompt-policy",
                    vec!["prompt".to_owned()],
                    Vec::new(),
                ),
            )
            .expect("approve prompt");
        core.commit_content_package_import(
            &inspection.import_id,
            &commit_request(&inspection, &selected, &approved),
        )
        .expect("commit prompt");

        let stored = core
            .get_prompt_preset(&PromptPresetId::from("imported-policy-test"))
            .expect("stored prompt");
        let canonical_policy = &built_in_prompt_presets()[0].blocks[0];
        assert_eq!(stored.value.blocks.first(), Some(canonical_policy));
        assert_eq!(
            stored
                .value
                .blocks
                .iter()
                .filter(|block| *block == canonical_policy)
                .count(),
            1
        );
        assert!(stored.value.blocks.iter().skip(1).all(|block| {
            block.authority != InstructionAuthority::Application
                && block.placement_zone != PlacementZone::ApplicationPolicy
        }));
        assert!(
            !serde_json::to_string(&stored.value)
                .expect("encode stored prompt")
                .contains(HOSTILE_POLICY_CANARY)
        );
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the real package review, persistence, resolver, and provider boundaries form one security regression"
    )]
    fn imported_prompt_authority_is_downgraded_through_provider_compilation() {
        const PACKAGE_DEVELOPER_CANARY: &str = "PACKAGE_DEVELOPER_AUTHORITY_CANARY";
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("prompt-authority.zip");
        let mut preset = imported_prompt_preset("imported-authority-test");
        let elevated = &mut preset.blocks[1];
        elevated.name = "Package developer instruction".to_owned();
        elevated.kind = PromptBlockKind::StaticInstruction;
        elevated.role_hint = RoleHint::Developer;
        elevated.authority = InstructionAuthority::Creator;
        elevated.template = Some(SafeTemplate {
            parts: vec![TemplatePart::Text {
                value: PACKAGE_DEVELOPER_CANARY.to_owned(),
            }],
            max_output_chars: 2_048,
        });
        elevated.source = BlockSource::Template;
        elevated.placement_zone = PlacementZone::PresetInstruction;
        elevated.history_selector = None;
        synthetic_prompt_package(&source, &preset, "dev.lorepia.imported-authority-package");

        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspect prompt authority package");
        let selected = core
            .select_content_package_import(
                &inspection.import_id,
                &selection_request(&inspection, vec!["prompt".to_owned()]),
            )
            .expect("select prompt authority package");
        let approved = core
            .approve_content_package_import(
                &inspection.import_id,
                &approval_request(
                    &inspection,
                    &selected,
                    "approval-prompt-authority",
                    vec!["prompt".to_owned()],
                    Vec::new(),
                ),
            )
            .expect("approve prompt authority package");
        core.commit_content_package_import(
            &inspection.import_id,
            &commit_request(&inspection, &selected, &approved),
        )
        .expect("commit prompt authority package");

        let stored = core
            .get_prompt_preset(&PromptPresetId::from("imported-authority-test"))
            .expect("stored prompt authority preset")
            .value;
        assert_eq!(
            stored.blocks[0].authority,
            InstructionAuthority::Application,
            "the canonical application policy remains trusted"
        );
        assert!(
            stored
                .blocks
                .iter()
                .skip(1)
                .all(|block| block.authority == InstructionAuthority::ImportedContent),
            "every package-supplied block must persist as imported content"
        );

        let adapter = ProviderPromptAdapterContract::for_family(ApiFamily::OpenAiResponses);
        let branch_id = ConversationBranchId("prompt-authority-branch".to_owned());
        let latest_message_id = MessageId("prompt-authority-latest".to_owned());
        let resolved = lorepia_orchestration::resolve_prompt_plan(&PromptResolveRequest {
            preset: stored,
            context: PromptResolutionContext {
                conversation_id: ConversationId("prompt-authority-conversation".to_owned()),
                branch_id: branch_id.clone(),
                character: CharacterPromptContent {
                    character_id: "prompt-authority-character".to_owned(),
                    name: "Synthetic Character".to_owned(),
                    aliases: Vec::new(),
                    description: "Synthetic description".to_owned(),
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
                    id: latest_message_id.clone(),
                    branch_id,
                    role: PromptMessageRole::User,
                    content: "Synthetic latest user message".to_owned(),
                    turn_index: 1,
                }],
                latest_user_message_id: latest_message_id,
                selected_knowledge: Vec::new(),
                selected_memory: Vec::new(),
                summary_boundaries: Vec::new(),
                conversation_summary: None,
                author_note: None,
                group_context: None,
                variables: VariableMap::default(),
                slots: Vec::new(),
                current_date: "2026-08-16".to_owned(),
                current_time: "12:00".to_owned(),
                supported_capabilities: Vec::new(),
                session_seed: Some(7),
                context_snapshot: None,
            },
            provider: adapter.resolution_contract(DeveloperRoleCapability::Supported),
            generation_preset_id: None,
            max_context_tokens: 8_192,
            reserved_output_tokens: 512,
        })
        .expect("resolve imported prompt authority preset");
        let resolved_canary = resolved
            .effective_messages
            .iter()
            .find(|message| message.content == PACKAGE_DEVELOPER_CANARY)
            .expect("resolved package developer canary");
        assert_eq!(
            resolved_canary.authority,
            InstructionAuthority::ImportedContent
        );
        assert_eq!(resolved_canary.requested_role, RoleHint::Developer);
        assert_eq!(resolved_canary.effective_role, ProviderMessageRole::User);

        let compiled = adapter
            .compile_resolved_plan(
                &resolved,
                DeveloperRoleCapability::Supported,
                PromptCacheWireDialect::Unsupported,
            )
            .expect("compile imported prompt for provider");
        let provider_canary = compiled
            .messages()
            .iter()
            .find(|message| message.content() == PACKAGE_DEVELOPER_CANARY)
            .expect("provider package developer canary");
        assert_eq!(provider_canary.effective_role(), ProviderMessageRole::User);
    }

    #[test]
    fn invalid_typed_prompt_is_rejected_before_selection_is_persisted() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("invalid-prompt.zip");
        let mut preset = imported_prompt_preset("invalid-imported-prompt");
        preset
            .blocks
            .retain(|block| block.kind != PromptBlockKind::LatestUserTurn);
        synthetic_prompt_package(&source, &preset, "dev.lorepia.invalid-prompt-package");
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspection accepts typed payload for review");
        core.select_content_package_import(
            &inspection.import_id,
            &selection_request(&inspection, vec!["prompt".to_owned()]),
        )
        .expect_err("invalid prompt preset must fail before review transition");
        let import = core
            .get_content_package_import(&inspection.import_id)
            .expect("import remains inspectable");
        assert_eq!(import.status, PackageImportStatus::Inspected);
        assert!(import.selection.is_none());
        assert!(
            core.get_prompt_preset(&PromptPresetId::from("invalid-imported-prompt"))
                .is_err()
        );
    }

    #[test]
    fn one_shot_source_becomes_an_opaque_core_owned_snapshot() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("untrusted.zip");
        synthetic_transform_package(&source);

        let owned = stage_content_package(
            &source,
            &data_root.path().join("staging"),
            ImportLimits::default(),
        )
        .expect("stage and inspect");
        let view = owned
            .public_inspection(1, package_capability_review(owned.review()))
            .expect("public inspection");
        let capability_review_sha256 = view.capability_review_sha256.clone();
        Uuid::parse_str(&view.import_id).expect("opaque canonical import id");
        assert_eq!(view.inspection.id.0, view.import_id);
        view.review.verify().expect("verify orchestration review");
        assert!(view.review.local_import_allowed);
        let serialized = serde_json::to_string(&view).expect("serialize view");
        assert!(!serialized.contains(&source.display().to_string()));
        assert!(!serialized.contains(&data_root.path().display().to_string()));
        fs::write(&source, b"external source changed").expect("mutate external source");

        let selection = owned
            .select(&ContentPackageSelectionRequest {
                expected_revision: view.revision,
                expected_package_plan_hash: view.inspection.plan_hash.clone(),
                expected_review_sha256: view.review.review_sha256.clone(),
                expected_capability_review_sha256: capability_review_sha256,
                selected_component_ids: vec!["transform".to_owned()],
            })
            .expect("select private snapshot");
        let prepared = owned
            .prepare(
                &selection,
                &view.inspection.plan_hash,
                &selection.selection_plan_hash,
                ImportLimits::default(),
            )
            .expect("prepare private snapshot");
        let transform = match &prepared.documents[0].document {
            lorepia_content::PreparedContentDocument::TransformSet(set) => set,
            other => panic!("unexpected document: {other:?}"),
        };
        assert!(!transform.enabled, "imported transforms stay inactive");
        assert_eq!(prepared.transformations.len(), 1);
        owned
            .discard(&data_root.path().join("staging"))
            .expect("discard snapshot");
        assert!(
            fs::read_dir(data_root.path().join("staging"))
                .expect("read staging")
                .next()
                .is_none()
        );
    }

    #[test]
    fn stale_hash_tamper_and_invalid_ticket_never_prepare_content() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let staging = data_root.path().join("staging");
        let source = source_root.path().join("untrusted.zip");
        synthetic_transform_package(&source);
        let owned =
            stage_content_package(&source, &staging, ImportLimits::default()).expect("inspect");
        let view = owned
            .public_inspection(1, package_capability_review(owned.review()))
            .expect("public inspection");
        let capability_review_sha256 = view.capability_review_sha256.clone();

        let stale = owned
            .select(&ContentPackageSelectionRequest {
                expected_revision: view.revision,
                expected_package_plan_hash: "00".repeat(32),
                expected_review_sha256: view.review.review_sha256.clone(),
                expected_capability_review_sha256: capability_review_sha256.clone(),
                selected_component_ids: vec!["transform".to_owned()],
            })
            .expect_err("stale inspection hash must fail");
        assert_eq!(stale.code, CoreErrorCode::InvalidInput);
        let selection = owned
            .select(&ContentPackageSelectionRequest {
                expected_revision: view.revision,
                expected_package_plan_hash: view.inspection.plan_hash.clone(),
                expected_review_sha256: view.review.review_sha256.clone(),
                expected_capability_review_sha256: capability_review_sha256,
                selected_component_ids: vec!["transform".to_owned()],
            })
            .expect("selection");
        let wrong_approval = owned
            .prepare(
                &selection,
                &view.inspection.plan_hash,
                &"ff".repeat(32),
                ImportLimits::default(),
            )
            .expect_err("wrong selection approval hash must fail");
        assert_eq!(wrong_approval.code, CoreErrorCode::InvalidInput);

        fs::write(&owned.path, b"tampered private snapshot").expect("tamper snapshot");
        let tampered = owned
            .prepare(
                &selection,
                &view.inspection.plan_hash,
                &selection.selection_plan_hash,
                ImportLimits::default(),
            )
            .expect_err("tampered private snapshot must fail");
        assert!(matches!(
            tampered.code,
            CoreErrorCode::UnsafeArchive | CoreErrorCode::UnsupportedContent
        ));
        assert!(
            discard_content_package_snapshot("../escape", &staging).is_err(),
            "opaque ticket validation must happen before path construction"
        );
        owned.discard(&staging).expect("discard tampered snapshot");
    }
