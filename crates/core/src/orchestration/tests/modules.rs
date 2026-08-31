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

#[derive(Clone, Copy)]
enum StaleManualKnowledgePolicyChange {
    RemoveEntry,
    AdvanceBookRevision,
}

#[allow(
    clippy::too_many_lines,
    reason = "one fixture proves exact activation, policy drift, next-turn progress, and durable reconciliation"
)]
fn assert_stale_manual_knowledge_conversation_remains_operable(
    change: StaleManualKnowledgePolicyChange,
) {
    let root = tempdir().expect("temporary Core root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open Core");
    let character_id = import_synthetic_character(&core);
    let (origin, requests, provider) = spawn_provider(2);
    let target = provider_fixture(&core, &origin);
    let conversation = core
        .create_conversation(
            &character_id,
            "Synthetic stale manual knowledge room",
            lorepia_core::ConversationMode::Chat,
        )
        .expect("create stale manual knowledge conversation");
    let branch = core
        .list_conversation_branches(&conversation.id)
        .expect("list stale manual knowledge branches")
        .into_iter()
        .next()
        .expect("stale manual knowledge root branch");
    let book = knowledge_book();
    let first_book = core
        .upsert_knowledge_book(&book, None)
        .expect("save first interaction knowledge revision");
    let first_book_revision_id = first_book
        .revision_id
        .clone()
        .expect("first interaction knowledge immutable revision");
    let entry_id = book.entries[0].id.clone();
    let rule_set = interaction_knowledge_rule_set(&entry_id);
    core.upsert_interaction_rule_set(&rule_set, None)
        .expect("save interaction knowledge activation rule");
    let module = interaction_knowledge_module(&book.id, &rule_set.id);
    let runtime_target = ContentModuleRuntimeTarget {
        conversation_id: conversation.id.clone(),
        branch_id: branch.id.clone(),
    };
    let binding_id = ModuleBindingId::from("synthetic.core.module.interaction-knowledge.binding");
    activate_app_module(&core, &module, runtime_target.clone(), binding_id.as_str());

    let first_generation = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &branch.id,
            None,
            lorepia_core::ConversationMode::Chat,
            "Activate the synthetic interaction knowledge",
            GenerationOperationContext::New {
                operation_nonce: "stale-manual-knowledge-first-turn-v1",
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("send interaction knowledge activation turn");
    requests
        .recv_timeout(Duration::from_secs(2))
        .expect("capture interaction knowledge activation request");
    wait_for_generation(&core, &branch.id, &first_generation);
    wait_for_active_interaction_knowledge_bindings(
        &core,
        root.path(),
        &conversation.id,
        &branch.id,
        &[(first_book_revision_id.clone(), entry_id.as_str().to_owned())],
    );

    match change {
        StaleManualKnowledgePolicyChange::RemoveEntry => {
            let request = ContentModuleDeactivationRequest {
                runtime_target: runtime_target.clone(),
                binding_id: binding_id.clone(),
            };
            let review = core
                .review_content_module_deactivation(&request)
                .expect("review knowledge module deactivation");
            core.deactivate_content_module(&request, &review.review_sha256)
                .expect("deactivate knowledge module")
                .verify()
                .expect("verify knowledge module deactivation");
        }
        StaleManualKnowledgePolicyChange::AdvanceBookRevision => {
            let mut advanced_book = book.clone();
            "Synthetic Core knowledge revision two".clone_into(&mut advanced_book.name);
            let advanced_book = core
                .upsert_knowledge_book(&advanced_book, Some(first_book.revision))
                .expect("save advanced interaction knowledge revision");
            assert_ne!(
                advanced_book.revision_id.as_ref(),
                Some(&first_book_revision_id)
            );
            let current_module = core
                .get_content_module(&module.id)
                .expect("load first interaction knowledge module revision");
            let mut advanced_module = module.clone();
            "2.0.0".clone_into(&mut advanced_module.version);
            advanced_module.interaction_rule_set_ids.clear();
            advanced_module.required_capabilities = vec![ContentCapability::Knowledge];
            core.upsert_content_module(&advanced_module, Some(current_module.revision))
                .expect("save module with advanced knowledge revision");
            reactivate_app_module(&core, &advanced_module, runtime_target.clone(), &binding_id);
        }
    }

    let previous_revision = core
        .get_interaction_state_revision(&conversation.id, &branch.id)
        .expect("read interaction revision before stale reconciliation");
    let expected_head = core
        .list_branch_messages(&branch.id)
        .expect("list messages before stale reconciliation turn")
        .last()
        .map(|message| message.id.clone());
    let second_generation = core
        .send_message_to_branch_with_connection_credential(
            &conversation.id,
            &branch.id,
            expected_head.as_ref(),
            lorepia_core::ConversationMode::Chat,
            "Continue after the knowledge policy changed",
            GenerationOperationContext::New {
                operation_nonce: match change {
                    StaleManualKnowledgePolicyChange::RemoveEntry => {
                        "stale-manual-knowledge-removed-turn-v1"
                    }
                    StaleManualKnowledgePolicyChange::AdvanceBookRevision => {
                        "stale-manual-knowledge-advanced-turn-v1"
                    }
                },
            },
            &target,
            reviewed_provider_credential(&core),
        )
        .expect("stale manual knowledge must not block the next turn");
    requests
        .recv_timeout(Duration::from_secs(2))
        .expect("capture request after stale knowledge reconciliation");
    wait_for_generation(&core, &branch.id, &second_generation);
    wait_for_active_interaction_knowledge_bindings(
        &core,
        root.path(),
        &conversation.id,
        &branch.id,
        &[],
    );
    assert!(
        core.get_interaction_state_revision(&conversation.id, &branch.id)
            .expect("read reconciled interaction revision")
            > previous_revision,
        "the stale authority must reconcile through an auditable state transition"
    );
    provider.join().expect("join synthetic provider");
}

#[test]
fn removed_module_knowledge_does_not_block_an_existing_conversation() {
    assert_stale_manual_knowledge_conversation_remains_operable(
        StaleManualKnowledgePolicyChange::RemoveEntry,
    );
}

#[test]
fn advanced_module_knowledge_revision_does_not_rebind_existing_authority() {
    assert_stale_manual_knowledge_conversation_remains_operable(
        StaleManualKnowledgePolicyChange::AdvanceBookRevision,
    );
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
