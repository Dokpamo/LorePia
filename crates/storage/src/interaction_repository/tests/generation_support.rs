use super::*;

pub(super) struct GenerationApprovalFixture {
    pub(super) _root: TempDir,
    pub(super) storage: Storage,
    pub(super) source_key: InteractionStateKey,
    pub(super) target_key: InteractionStateKey,
    pub(super) commit: GenerationAttemptBeforeReviewCommit,
    pub(super) policy: InteractionPolicySnapshot,
    approve_rule_id: InteractionRuleId,
    rule_set_revision_id: String,
}

pub(super) fn synthetic_prompt_selection_authority(
    storage: &Storage,
    conversation_id: &ConversationId,
) -> crate::GenerationPromptSelectionAuthority {
    let character = storage
        .connection()
        .expect("open prompt character authority connection")
        .query_row(
            "SELECT character.id, character.name, character.description,
                    character.source_hash, character.avatar_asset_hash,
                    character.created_at
             FROM conversations AS conversation
             JOIN characters AS character
               ON character.id = conversation.character_id
             WHERE conversation.id = ?1",
            [conversation_id.0.as_str()],
            |row| {
                Ok(Character {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    source_hash: row.get(3)?,
                    avatar_asset_hash: row.get(4)?,
                    created_at: DateTime::parse_from_rfc3339(&row.get::<_, String>(5)?)
                        .expect("parse prompt character authority time")
                        .with_timezone(&Utc),
                })
            },
        )
        .expect("load prompt character authority");
    crate::GenerationPromptSelectionAuthority {
        schema_version: 1,
        mode: ConversationMode::Chat,
        local_user_id_sha256: lorepia_domain::prompt_local_user_id_sha256(
            &storage
                .load_settings()
                .expect("load prompt local user authority")
                .local_user_id,
        ),
        character,
        character_content: None,
        character_knowledge_book: None,
        supported_capabilities: Vec::new(),
        quick_settings: crate::GenerationPromptQuickSettingsAuthority {
            response_length: crate::PromptResponseLength::Balanced,
            creativity: 50,
            reasoning_effort: None,
            memory_enabled: true,
            knowledge_enabled: true,
            supports_temperature: false,
            resolved_temperature: None,
            resolved_max_output_tokens: None,
        },
        provider_target_authority: Some(crate::GenerationProviderTargetAuthority::DirectModel {
            model_sha256: Sha256Digest::parse("e".repeat(64))
                .expect("synthetic direct model SHA-256"),
        }),
        explicit_preset_id: None,
        preset: crate::built_in_prompt_presets()
            .into_iter()
            .next()
            .expect("built-in chat preset"),
        preset_revision: 1,
        preset_revision_id: "synthetic-prompt-revision".to_owned(),
        binding: None,
        persona_selection: None,
    }
}

pub(super) fn synthetic_evaluation_seal(
    policy: &InteractionPolicySnapshot,
) -> InteractionEvaluationSeal {
    let limits = lorepia_orchestration::InteractionLimits::default();
    InteractionEvaluationSeal {
        schema_version: 1,
        engine_contract_version: 1,
        policy_sha256: Sha256Digest::parse(
            interaction_policy_sha256(policy).expect("synthetic policy digest"),
        )
        .expect("synthetic policy SHA-256"),
        executable_rule_sets_sha256: Sha256Digest::parse(sha256_hex(
            b"synthetic-executable-interaction-policy",
        ))
        .expect("synthetic executable policy SHA-256"),
        knowledge_revisions: Vec::new(),
        asset_action_diagnostics: Vec::new(),
        approved_import_source_ids: Vec::new(),
        policy_variables: lorepia_domain::VariableMap::default(),
        supported_capabilities: Vec::new(),
        template_values: crate::InteractionEvaluationTemplateValues {
            character_name: Some("Synthetic Character".to_owned()),
            user_name: Some("Synthetic User".to_owned()),
            persona_name: None,
            persona_description: None,
            current_date: Some("2026-08-09".to_owned()),
            current_time: Some("00:00:00+00:00".to_owned()),
        },
        event_epoch_seconds: 0,
        limits: crate::InteractionEvaluationLimits {
            max_rule_sets: limits.max_rule_sets,
            max_rules: limits.max_rules,
            max_actions_per_event: limits.max_actions_per_event,
            max_actions_per_rule: limits.max_actions_per_rule,
            max_condition_depth: limits.max_condition_depth,
            max_condition_nodes: limits.max_condition_nodes,
            max_template_depth: limits.max_template_depth,
            max_template_parts: limits.max_template_parts,
            max_variables: limits.max_variables,
            max_proposals: limits.max_proposals,
            max_pending_proposals: limits.max_pending_proposals,
            max_effects: limits.max_effects,
            max_choices: limits.max_choices,
            max_dice_count: limits.max_dice_count,
            max_dice_sides: limits.max_dice_sides,
            max_text_chars: limits.max_text_chars,
            max_identifier_bytes: limits.max_identifier_bytes,
        },
        seed_contract_version: 1,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn synthetic_closure(
    generation_id: &GenerationId,
    event_id: &str,
    event: InteractionEvent,
    policy: &InteractionPolicySnapshot,
    evaluation_seal: &InteractionEvaluationSeal,
    previous_state: &InteractionState,
    next_state: &InteractionState,
    knowledge: &[InteractionKnowledgeBinding],
    action_results: &[InteractionActionResultWrite],
    effects: &[InteractionEffect],
    derived_events: &[InteractionDerivedEventWrite],
    proposals: &[InteractionProposalWrite],
) -> GenerationAttemptDerivedClosure {
    let transition = crate::GenerationAttemptDerivedTransition {
        ordinal: 0,
        parent_ordinal: None,
        depth: 0,
        event_id: event_id.to_owned(),
        event_sha256: crate::generation_attempt_derived_event_sha256(&event)
            .expect("synthetic event digest"),
        event,
        deterministic_seed: 0,
        expected_state_revision: previous_state.revision,
        resulting_state_revision: next_state.revision,
        policy: policy.clone(),
        evaluation_seal: evaluation_seal.clone(),
        next_state: next_state.clone(),
        knowledge: knowledge.to_vec(),
        action_results: action_results.to_vec(),
        effects: effects.to_vec(),
        derived_events: derived_events.to_vec(),
        proposals: proposals.to_vec(),
        commit_sha256: Sha256Digest::parse(sha256_hex(b"synthetic-transition-commit"))
            .expect("synthetic transition commit digest"),
    };
    let mut closure = GenerationAttemptDerivedClosure {
        schema_version: 1,
        transitions: vec![transition],
        guard_audits: Vec::new(),
        final_state: next_state.clone(),
        final_knowledge: knowledge.to_vec(),
        event_count: 1,
        guard_count: 0,
        chain_sha256: Sha256Digest::parse(sha256_hex(b"placeholder-derived-chain"))
            .expect("placeholder derived chain digest"),
    };
    closure.transitions[0].commit_sha256 =
        crate::generation_attempt_derived_transition_commit_sha256(
            generation_id,
            &closure.transitions[0],
        )
        .expect("synthetic transition commit digest");
    closure.chain_sha256 = crate::generation_attempt_derived_chain_sha256(&closure)
        .expect("synthetic derived chain digest");
    closure
}

fn install_generation_approval_rule(
    storage: &Storage,
) -> (
    InteractionRuleSetId,
    InteractionRuleId,
    InteractionRuleId,
    String,
) {
    let provenance = Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: None,
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    };
    let rule_set_id = InteractionRuleSetId::from("generation-approval-rules");
    let rule_id = InteractionRuleId::from("generation-request-rule");
    let approve_rule_id = InteractionRuleId::from("generation-approve-rule");
    let rule_set = InteractionRuleSet {
        id: rule_set_id.clone(),
        name: "Generation approval rules".to_owned(),
        schema_version: 1,
        rules: vec![
            InteractionRule {
                id: rule_id.clone(),
                name: "Review generation change".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::BeforeGeneration,
                condition: None,
                actions: vec![InteractionAction::RequestUserApproval {
                    proposal: ProposalSpec {
                        id: "approve-generation-change".to_owned(),
                        title: "Approve generation change".to_owned(),
                        body: text_template("Allow this generation-scoped change?"),
                        expires_after_seconds: Some(60),
                    },
                }],
                priority: 0,
                stop_after_match: false,
                provenance: provenance.clone(),
            },
            InteractionRule {
                id: approve_rule_id.clone(),
                name: "Apply generation approval".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::UserAction {
                    action_id: "approve-generation-change".to_owned(),
                },
                condition: None,
                actions: vec![InteractionAction::AppendVisibleSystemEvent {
                    text: text_template("Generation change approved"),
                }],
                priority: 1,
                stop_after_match: false,
                provenance: provenance.clone(),
            },
        ],
        max_actions_per_event: 8,
        provenance,
    };
    let revision_id = storage
        .save_interaction_rule_set(&rule_set, None)
        .expect("save generation approval rules")
        .revision_id
        .expect("generation approval rule-set revision");
    (rule_set_id, rule_id, approve_rule_id, revision_id)
}

pub(super) fn generation_approval_fixture(fork: bool) -> GenerationApprovalFixture {
    let (root, storage, conversation_id, source_branch_id) = interaction_storage();
    let source_key = InteractionStateKey {
        state_id: "generation-attempt-source-state".to_owned(),
        conversation_id: conversation_id.clone(),
        branch_id: source_branch_id.clone(),
    };
    let operation_id = "generation-approval-operation";
    let proposed_branch_id = if fork {
        crate::deterministic_proposed_branch_id(
            operation_id,
            &conversation_id,
            &source_branch_id,
            None,
        )
        .expect("derive deterministic generation target branch")
    } else {
        source_branch_id.clone()
    };
    let target_key = InteractionStateKey {
        state_id: if fork {
            "generation-attempt-target-state".to_owned()
        } else {
            source_key.state_id.clone()
        },
        conversation_id: conversation_id.clone(),
        branch_id: proposed_branch_id.clone(),
    };
    let occurred_at = Utc::now();
    let previous_state = empty_state(0);
    storage
        .get_or_init_interaction_state(&source_key, &previous_state, &[], occurred_at)
        .expect("initialize generation attempt boundary");

    let (rule_set_id, rule_id, approve_rule_id, rule_set_revision_id) =
        install_generation_approval_rule(&storage);
    let policy = policy_for_rule_set(&storage, &rule_set_id, &rule_set_revision_id);
    let proposal = InteractionProposalRecord {
        id: interaction_proposal_record_id(&rule_set_id, &rule_id, "approve-generation-change", 0)
            .expect("derive generation proposal record id"),
        rule_set_id: rule_set_id.clone(),
        rule_id: rule_id.clone(),
        proposal_id: "approve-generation-change".to_owned(),
        title: "Approve generation change".to_owned(),
        body: "Allow this generation-scoped change?".to_owned(),
        status: InteractionProposalStatus::Pending,
        source_interaction_state_revision: 0,
        requested_at_epoch_seconds: occurred_at.timestamp(),
        expires_at_epoch_seconds: Some(occurred_at.timestamp() + 60),
        decided_at_epoch_seconds: None,
    };
    let mut next_state = empty_state(1);
    next_state.proposals.push(proposal.clone());

    let settings = storage.load_settings().expect("load local user authority");
    let character_id = storage
        .connection()
        .expect("open fixture metadata connection")
        .query_row(
            "SELECT character_id FROM conversations WHERE id = ?1",
            [conversation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load fixture character id");
    let module_context = lorepia_orchestration::ModuleResolutionContext {
        local_user_id: settings.local_user_id,
        persona_id: None,
        character_id: Some(character_id),
        conversation_id: Some(conversation_id.0.clone()),
        branch_id: Some(proposed_branch_id.0.clone()),
        supported_capabilities: Vec::new(),
    };
    let module_runtime_review =
        lorepia_orchestration::review_module_merge(0, &module_context, &[], &[])
            .expect("review no-module generation context");
    let module_plan_sha256 = no_applied_module_runtime_plan_sha256();
    let attempt = storage
        .prepare_generation_attempt(
            &GenerationAttemptInput {
                operation_id: operation_id.to_owned(),
                conversation_id: conversation_id.clone(),
                source_branch_id: source_branch_id.clone(),
                proposed_branch_id,
                expected_head_message_id: None,
                context_head_message_id: None,
                module_plan_sha256,
                base_request_fingerprint_sha256: Sha256Digest::parse(sha256_hex(
                    b"generation-approval-base-input",
                ))
                .expect("base input digest"),
                prompt_selection_authority: Some(synthetic_prompt_selection_authority(
                    &storage,
                    &conversation_id,
                )),
                module_runtime_review_authority: Some(module_runtime_review.clone()),
                applied_runtime_plan_authority: None,
            },
            occurred_at,
        )
        .expect("prepare generation attempt");
    let memory_head_snapshot = storage
        .list_memory_records_at_head(&conversation_id, &source_branch_id, None, false)
        .expect("capture empty memory authority")
        .snapshot;
    let event_id = "generation-attempt-before-review".to_owned();
    let action_results = vec![InteractionActionResultWrite {
        set_revision_id: rule_set_revision_id.clone(),
        rule_id: rule_id.clone(),
        action_ordinal: 0,
        status: InteractionActionResultStatus::Proposed,
        result: VersionedJson {
            schema_version: 1,
            value: json!({"status": "proposal_requested"}),
        },
    }];
    let effects = vec![InteractionEffect::ApprovalRequested {
        rule_set_id: rule_set_id.clone(),
        rule_id: rule_id.clone(),
        proposal_id: "approve-generation-change".to_owned(),
        title: "Approve generation change".to_owned(),
        body: "Allow this generation-scoped change?".to_owned(),
        expires_after_seconds: Some(60),
    }];
    let proposals = vec![InteractionProposalWrite {
        review_payload_sha256: interaction_proposal_review_sha256(&proposal)
            .expect("generation proposal review digest"),
        record: proposal,
        rule_set_revision_id: rule_set_revision_id.clone(),
        action_ordinal: 0,
    }];
    let evaluation_seal = synthetic_evaluation_seal(&policy);
    let derived_closure = synthetic_closure(
        &attempt.generation_id,
        &event_id,
        InteractionEvent::BeforeGeneration,
        &policy,
        &evaluation_seal,
        &previous_state,
        &next_state,
        &[],
        &action_results,
        &effects,
        &[],
        &proposals,
    );
    let commit = GenerationAttemptBeforeReviewCommit {
        generation_id: attempt.generation_id,
        expected_attempt_revision: attempt.revision,
        event_id,
        occurred_at,
        context_head_message_id: None,
        context_checkpoint_sha256: interaction_state_snapshot_sha256(&previous_state, &[])
            .expect("interaction boundary digest"),
        previous_state,
        previous_knowledge: Vec::new(),
        module_runtime_review,
        memory_head_snapshot,
        applied_runtime_plan: None,
        policy: policy.clone(),
        evaluation_seal,
        derived_closure,
        next_state,
        knowledge: Vec::new(),
        action_results,
        effects,
        derived_events: Vec::new(),
        proposals,
        review_sha256: sha256_hex(b"generation-attempt-before-review-authority"),
    };
    GenerationApprovalFixture {
        _root: root,
        storage,
        source_key,
        target_key,
        commit,
        policy,
        approve_rule_id,
        rule_set_revision_id,
    }
}
fn generation_materialization_prompt_plan(
    fixture: &GenerationApprovalFixture,
    generation_id: &GenerationId,
) -> GenerationPromptPlanRecord {
    let created_at = fixture.commit.occurred_at + Duration::seconds(2);
    let provenance = Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: None,
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    };
    let metadata = PresetMetadata {
        description: "materialization test preset".to_owned(),
        tags: Vec::new(),
        provenance,
        created_at,
        updated_at: created_at,
        local_override_of: None,
    };
    let preset = lorepia_orchestration::default_prompt_preset(
        PromptPresetId::from("materialization-test-preset"),
        "Materialization test",
        metadata,
    );
    let latest_user_message_id = MessageId("materialization-latest-user".to_owned());
    let resolved = lorepia_orchestration::resolve_prompt_plan(&PromptResolveRequest {
        preset: preset.clone(),
        context: PromptResolutionContext {
            conversation_id: fixture.target_key.conversation_id.clone(),
            branch_id: fixture.target_key.branch_id.clone(),
            character: CharacterPromptContent {
                character_id: "materialization-character".to_owned(),
                name: "Synthetic character".to_owned(),
                aliases: Vec::new(),
                description: "Synthetic materialization fixture".to_owned(),
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
            user_name: "Synthetic user".to_owned(),
            messages: vec![PromptConversationMessage {
                id: latest_user_message_id.clone(),
                branch_id: fixture.target_key.branch_id.clone(),
                role: PromptMessageRole::User,
                content: "Materialize this reviewed generation.".to_owned(),
                turn_index: 1,
            }],
            latest_user_message_id: latest_user_message_id.clone(),
            selected_knowledge: Vec::new(),
            selected_memory: Vec::new(),
            summary_boundaries: Vec::new(),
            conversation_summary: None,
            author_note: None,
            group_context: None,
            variables: lorepia_domain::VariableMap::default(),
            slots: Vec::new(),
            current_date: "2026-08-09".to_owned(),
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
        max_context_tokens: 512,
        reserved_output_tokens: 32,
    })
    .expect("resolve materialization prompt plan");
    GenerationPromptPlanRecord {
        id: "materialization-prompt-plan".to_owned(),
        generation_id: generation_id.clone(),
        conversation_id: fixture.target_key.conversation_id.clone(),
        branch_id: fixture.target_key.branch_id.clone(),
        head_message_id: None,
        latest_user_message_id,
        prompt_preset_id: preset.id,
        prompt_preset_revision_id: "materialization-prompt-preset-revision".to_owned(),
        model_route_id: None,
        generation_preset_id: None,
        task_profile_revision_id: None,
        random_seed: Some(7),
        tokenizer_id: "utf8-bytes".to_owned(),
        tokenizer_version: "1".to_owned(),
        plan: VersionedJson {
            schema_version: resolved.schema_version,
            value: serde_json::to_value(&resolved)
                .expect("encode materialization resolved prompt plan"),
        },
        plan_sha256: resolved.plan_hash,
        input_fingerprint_sha256: sha256_hex(b"materialization-prompt-input"),
        context_limit_tokens: resolved.trace.max_context_tokens,
        estimated_input_tokens: resolved.trace.estimated_input_tokens,
        reserved_output_tokens: resolved.trace.reserved_output_tokens,
        final_input_tokens: resolved.trace.estimated_input_tokens,
        cacheable_prefix_tokens: 0,
        provider_request: ProviderRequestSnapshotRecord {
            id: "materialization-provider-request".to_owned(),
            api_family: ApiFamily::OpenAiChatCompletions,
            request_schema_version: 1,
            request: VersionedJson {
                schema_version: 1,
                value: json!({"messages": []}),
            },
            mapping_diagnostics: VersionedJson {
                schema_version: 1,
                value: json!({"module_plan_sha256": null}),
            },
            created_at,
        },
        created_at,
    }
}

pub(super) fn seal_approved_generation_fixture(
    fixture: &GenerationApprovalFixture,
) -> (
    StoredGenerationAttempt,
    GenerationPromptPlanRecord,
    GenerationAttemptProposalDecisionReceipt,
) {
    let before = fixture
        .storage
        .commit_generation_attempt_before_review(&fixture.commit)
        .expect("stage generation BeforeGeneration review");
    assert!(before.evidence.awaiting_approval);
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
    let proposal = fixture
        .storage
        .list_generation_attempt_proposals(
            &fixture.commit.generation_id,
            InteractionProposalStatus::Pending,
            8,
        )
        .expect("list staged generation proposal")
        .pop()
        .expect("one staged generation proposal");
    let aggregate = fixture
        .storage
        .get_generation_attempt_interaction_aggregate(&fixture.commit.generation_id)
        .expect("load staged generation aggregate");
    let decided_at = fixture.commit.occurred_at + Duration::seconds(1);
    let domain_state = remap_generation_attempt_test_state(
        &fixture.storage,
        &fixture.commit.generation_id,
        &aggregate.state,
        true,
    );
    let domain_decision_state = approve_pending(
        &domain_state,
        &proposal.record.proposal_id,
        domain_state.revision,
        decided_at.timestamp(),
    )
    .expect("derive exact staged approval state")
    .state;
    let decision_state = remap_generation_attempt_test_state(
        &fixture.storage,
        &fixture.commit.generation_id,
        &domain_decision_state,
        false,
    );
    let mut derived_state = decision_state.clone();
    derived_state.revision = derived_state
        .revision
        .checked_add(1)
        .expect("derived state revision");
    let action_results = vec![InteractionActionResultWrite {
        set_revision_id: fixture.rule_set_revision_id.clone(),
        rule_id: fixture.approve_rule_id.clone(),
        action_ordinal: 0,
        status: InteractionActionResultStatus::Applied,
        result: VersionedJson {
            schema_version: 1,
            value: json!({"status": "visible_event_created"}),
        },
    }];
    let effects = vec![InteractionEffect::VisibleSystemEvent {
        text: "Generation change approved".to_owned(),
    }];
    let evaluation_seal = proposal.origin_evaluation_seal.clone();
    let decision_event_id = "generation-materialization-user-action";
    let derived_closure = synthetic_closure(
        &fixture.commit.generation_id,
        decision_event_id,
        InteractionEvent::UserAction {
            action_id: proposal.record.proposal_id.clone(),
        },
        &fixture.policy,
        &evaluation_seal,
        &decision_state,
        &derived_state,
        &[],
        &action_results,
        &effects,
        &[],
        &[],
    );
    let decision = GenerationAttemptProposalDecisionCommit {
        proposal_record_id: proposal.record.id.clone(),
        expected_proposal_revision: proposal.proposal_revision,
        expected_aggregate_revision: aggregate.aggregate_revision,
        decision: GenerationAttemptProposalDecision::Approve,
        decision_idempotency_key: "generation-materialization-approval".to_owned(),
        decided_at_epoch_seconds: decided_at.timestamp(),
        decision_state,
        current_policy: Some(fixture.policy.clone()),
        evaluation_seal: Some(evaluation_seal.clone()),
        derived_closure: Some(derived_closure),
        derived: Some(InteractionDerivedEventCommit {
            event_id: decision_event_id.to_owned(),
            idempotency_key: "generation-materialization-user-action-key".to_owned(),
            policy: fixture.policy.clone(),
            evaluation_seal: Some(evaluation_seal),
            deterministic_seed: Some(0),
            next_state: derived_state,
            knowledge: Vec::new(),
            action_results,
            effects,
            derived_events: Vec::new(),
            proposals: Vec::new(),
            created_at: decided_at,
        }),
        updated_at: decided_at,
    };
    let receipt = fixture
        .storage
        .decide_generation_attempt_proposal(&decision)
        .expect("decide staged generation proposal");
    assert!(!receipt.exact_replay);
    let replay = fixture
        .storage
        .decide_generation_attempt_proposal(&decision)
        .expect("replay staged generation proposal decision");
    assert!(replay.exact_replay);
    assert_eq!(replay.aggregate, receipt.aggregate);
    assert_eq!(
        replay.approval_evidence_sha256,
        receipt.approval_evidence_sha256
    );

    let current = fixture
        .storage
        .get_generation_attempt(&fixture.commit.generation_id)
        .expect("load approved generation attempt");
    assert_eq!(
        current.status,
        GenerationAttemptStatus::BeforeGenerationApplied
    );
    let prompt_plan = generation_materialization_prompt_plan(fixture, &current.generation_id);
    let seal = crate::GenerationDispatchSeal {
        final_prompt_plan_sha256: Sha256Digest::parse(prompt_plan.plan_sha256.clone())
            .expect("final prompt plan hash"),
        final_prompt_input_fingerprint_sha256: Sha256Digest::parse(
            prompt_plan.input_fingerprint_sha256.clone(),
        )
        .expect("final prompt input hash"),
        final_interaction_state_revision: receipt.aggregate.state.revision,
        final_interaction_state_sha256: receipt.aggregate.state_snapshot_sha256.clone(),
        applied_module_plan_sha256: no_applied_module_runtime_plan_sha256(),
        before_generation_evidence_sha256: before.evidence_sha256,
        approval_evidence_sha256: receipt.approval_evidence_sha256.clone(),
        derived_chain_sha256: Some(receipt.aggregate.derived_chain_sha256.clone()),
        derived_event_count: Some(receipt.aggregate.derived_event_count),
        derived_guard_count: Some(receipt.aggregate.derived_guard_count),
    };
    let sealed = fixture
        .storage
        .seal_generation_attempt_dispatch_ready(
            &current.generation_id,
            current.revision,
            &seal,
            decided_at + Duration::seconds(1),
        )
        .expect("seal generation materialization attempt");
    (sealed, prompt_plan, receipt)
}

pub(super) fn assert_generation_attempt_has_no_live_mutation(
    storage: &Storage,
    key: &InteractionStateKey,
) {
    assert_eq!(
        storage
            .get_interaction_state_snapshot(&key.conversation_id, &key.branch_id)
            .expect("load live interaction state")
            .state,
        empty_state(0),
        "attempt staging and decisions must remain isolated until append"
    );
    let connection = storage.connection().expect("open live-state assertion");
    for table in [
        "interaction_events",
        "interaction_proposals",
        "interaction_effect_outbox",
    ] {
        let count = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, u64>(0)
            })
            .expect("count live interaction rows");
        assert_eq!(count, 0, "{table} must remain empty before append");
    }
}

pub(super) fn remap_generation_attempt_test_state(
    storage: &Storage,
    generation_id: &GenerationId,
    state: &InteractionState,
    to_domain: bool,
) -> InteractionState {
    let connection = storage
        .connection()
        .expect("open proposal identity mapping");
    remap_generation_attempt_state_proposal_ids(&connection, generation_id, state, to_domain)
        .expect("remap generation proposal identities")
}

pub(super) fn generation_decision_handshake_counts(
    storage: &Storage,
    generation_id: &GenerationId,
) -> (u64, u64) {
    storage
        .connection()
        .expect("open generation decision handshake database")
        .query_row(
            "SELECT
                 (SELECT COUNT(*)
                  FROM generation_attempt_proposal_decision_commits
                  WHERE generation_id = ?1),
                 (SELECT COUNT(*)
                  FROM generation_attempt_aggregate_decision_bindings
                  WHERE generation_id = ?1)",
            [generation_id.0.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("count generation decision handshake rows")
}

pub(super) fn direct_terminalize_generation_proposal(
    connection: &Connection,
    proposal_record_id: &InteractionProposalRecordId,
    resulting_aggregate_revision: u64,
    resulting_state_revision: u64,
    resulting_state_snapshot_sha256: &str,
    updated_at: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE generation_attempt_proposals
         SET status = 'expired', proposal_revision = 2,
             decision_kind = 'expired',
             decision_idempotency_key = 'direct-generation-decision',
             decision_evidence_json = '{}',
             decision_evidence_sha256 = ?4,
             resulting_aggregate_revision = ?2,
             resulting_state_revision = ?3,
             resulting_state_json = '{}',
             resulting_state_snapshot_sha256 = ?4,
             resulting_derived_chain_sha256 = (
                 SELECT aggregate.derived_chain_sha256
                 FROM generation_attempt_interaction_aggregates AS aggregate
                 WHERE aggregate.generation_id = generation_attempt_proposals.generation_id
             ),
             resulting_derived_event_count = (
                 SELECT aggregate.derived_event_count
                 FROM generation_attempt_interaction_aggregates AS aggregate
                 WHERE aggregate.generation_id = generation_attempt_proposals.generation_id
             ),
             resulting_derived_guard_count = (
                 SELECT aggregate.derived_guard_count
                 FROM generation_attempt_interaction_aggregates AS aggregate
                 WHERE aggregate.generation_id = generation_attempt_proposals.generation_id
             ),
             resulting_pending_proposal_count = (
                 SELECT aggregate.pending_proposal_count - 1
                 FROM generation_attempt_interaction_aggregates AS aggregate
                 WHERE aggregate.generation_id = generation_attempt_proposals.generation_id
             ),
             materialization_json = '{}', materialization_sha256 = ?4,
             decided_at_epoch_seconds = expires_at_epoch_seconds,
             updated_at = ?5
         WHERE proposal_record_id = ?1",
        params![
            proposal_record_id.as_str(),
            i64::try_from(resulting_aggregate_revision)
                .expect("direct resulting aggregate revision fits i64"),
            i64::try_from(resulting_state_revision)
                .expect("direct resulting state revision fits i64"),
            resulting_state_snapshot_sha256,
            updated_at,
        ],
    )
}

pub(super) fn direct_advance_generation_aggregate(
    connection: &Connection,
    generation_id: &GenerationId,
    aggregate_revision: u64,
    interaction_state_revision: u64,
    state_snapshot_sha256: &str,
    updated_at: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE generation_attempt_interaction_aggregates
         SET aggregate_revision = ?2,
             interaction_state_revision = ?3,
             state_json = '{}', state_document_sha256 = ?4,
             state_snapshot_sha256 = ?4,
             knowledge_json = '[]', knowledge_sha256 = ?4,
             pending_proposal_count = 0, terminal_decision_count = 1,
             updated_at = ?5
         WHERE generation_id = ?1 AND aggregate_revision = 1",
        params![
            generation_id.0.as_str(),
            i64::try_from(aggregate_revision).expect("direct aggregate revision fits i64"),
            i64::try_from(interaction_state_revision)
                .expect("direct aggregate state revision fits i64"),
            state_snapshot_sha256,
            updated_at,
        ],
    )
}

pub(super) fn assert_pending_generation_handshake_unchanged(
    fixture: &GenerationApprovalFixture,
    proposal: &StoredGenerationAttemptProposal,
    aggregate: &StoredGenerationAttemptInteractionAggregate,
) {
    assert_eq!(
        fixture
            .storage
            .get_generation_attempt_proposal(&proposal.record.id)
            .expect("reload pending generation proposal"),
        *proposal
    );
    assert_eq!(
        fixture
            .storage
            .get_generation_attempt_interaction_aggregate(&fixture.commit.generation_id)
            .expect("reload pending generation aggregate"),
        *aggregate
    );
    assert_eq!(
        generation_decision_handshake_counts(&fixture.storage, &fixture.commit.generation_id),
        (0, 0)
    );
    assert_generation_attempt_has_no_live_mutation(&fixture.storage, &fixture.source_key);
}

pub(super) fn parallel_generation_commit(
    fixture: &GenerationApprovalFixture,
    operation_id: &str,
    event_id: &str,
    key: &InteractionStateKey,
) -> GenerationAttemptBeforeReviewCommit {
    let occurred_at = fixture.commit.occurred_at;
    fixture
        .storage
        .get_or_init_interaction_state(key, &empty_state(0), &[], occurred_at)
        .expect("initialize parallel generation boundary");
    let settings = fixture
        .storage
        .load_settings()
        .expect("load parallel local user authority");
    let character_id = fixture
        .storage
        .connection()
        .expect("open parallel fixture metadata")
        .query_row(
            "SELECT character_id FROM conversations WHERE id = ?1",
            [key.conversation_id.0.as_str()],
            |row| row.get::<_, String>(0),
        )
        .expect("load parallel fixture character");
    let module_context = lorepia_orchestration::ModuleResolutionContext {
        local_user_id: settings.local_user_id,
        persona_id: None,
        character_id: Some(character_id),
        conversation_id: Some(key.conversation_id.0.clone()),
        branch_id: Some(key.branch_id.0.clone()),
        supported_capabilities: Vec::new(),
    };
    let module_runtime_review =
        lorepia_orchestration::review_module_merge(0, &module_context, &[], &[])
            .expect("review parallel no-module context");
    let attempt = fixture
        .storage
        .prepare_generation_attempt(
            &GenerationAttemptInput {
                operation_id: operation_id.to_owned(),
                conversation_id: key.conversation_id.clone(),
                source_branch_id: key.branch_id.clone(),
                proposed_branch_id: key.branch_id.clone(),
                expected_head_message_id: None,
                context_head_message_id: None,
                module_plan_sha256: no_applied_module_runtime_plan_sha256(),
                base_request_fingerprint_sha256: Sha256Digest::parse(sha256_hex(
                    b"generation-approval-base-input",
                ))
                .expect("parallel base input digest"),
                prompt_selection_authority: Some(synthetic_prompt_selection_authority(
                    &fixture.storage,
                    &key.conversation_id,
                )),
                module_runtime_review_authority: Some(module_runtime_review.clone()),
                applied_runtime_plan_authority: None,
            },
            occurred_at,
        )
        .expect("prepare parallel generation attempt");
    let memory_head_snapshot = fixture
        .storage
        .list_memory_records_at_head(&key.conversation_id, &key.branch_id, None, false)
        .expect("capture parallel memory authority")
        .snapshot;
    let mut commit = GenerationAttemptBeforeReviewCommit {
        generation_id: attempt.generation_id,
        expected_attempt_revision: attempt.revision,
        event_id: event_id.to_owned(),
        module_runtime_review,
        memory_head_snapshot,
        ..fixture.commit.clone()
    };
    let root = commit
        .derived_closure
        .transitions
        .first_mut()
        .expect("parallel generation closure root");
    root.event_id.clone_from(&commit.event_id);
    root.commit_sha256 =
        crate::generation_attempt_derived_transition_commit_sha256(&commit.generation_id, root)
            .expect("rehash parallel generation closure root");
    commit.derived_closure.chain_sha256 =
        crate::generation_attempt_derived_chain_sha256(&commit.derived_closure)
            .expect("rehash parallel generation closure");
    commit
}
