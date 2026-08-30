
fn runtime_evidence_module_snapshot(
    now: DateTime<Utc>,
) -> lorepia_orchestration::ModuleRevisionSnapshot {
    let module_id = ContentModuleId::from("runtime-evidence-module");
    let revision_id = ModuleRevisionId::from("runtime-evidence-revision");
    lorepia_orchestration::ModuleRevisionSnapshot {
        module: ContentModule {
            id: module_id.clone(),
            name: "Runtime evidence module".to_owned(),
            version: "1.0.0".to_owned(),
            schema_version: 1,
            prompt_fragments: Vec::new(),
            knowledge_book_ids: Vec::new(),
            control_specs: Vec::new(),
            transform_set_ids: Vec::new(),
            interaction_rule_set_ids: Vec::new(),
            asset_ids: Vec::new(),
            imported_components_enabled: false,
            required_capabilities: Vec::new(),
            metadata: lorepia_domain::PackageMetadata {
                author: Some("Synthetic Runtime Test".to_owned()),
                license: "LicenseRef-Synthetic".to_owned(),
                redistribution_allowed: false,
                homepage: None,
                description: "Synthetic applied runtime evidence".to_owned(),
                tags: Vec::new(),
                provenance: Provenance {
                    source_kind: SourceKind::UserCreated,
                    source_id: Some("runtime-evidence-module".to_owned()),
                    source_hash: Some(test_digest("runtime-evidence-source").into_inner()),
                    author: None,
                    license: None,
                    imported_at: None,
                },
            },
        },
        revision: ContentModuleRevision {
            id: revision_id.clone(),
            module_id: module_id.clone(),
            version: "1.0.0".to_owned(),
            source_hash: test_digest("runtime-evidence-revision-source"),
            previous_revision_id: None,
            component_hashes: Vec::new(),
            created_at: now,
        },
        import_approval: None,
    }
}

fn runtime_evidence_context(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> lorepia_orchestration::ModuleResolutionContext {
    lorepia_orchestration::ModuleResolutionContext {
        local_user_id: lorepia_domain::LocalUserId::from("runtime-local-user"),
        persona_id: None,
        character_id: Some("runtime-character".to_owned()),
        conversation_id: Some(conversation_id.0.clone()),
        branch_id: Some(branch_id.0.clone()),
        supported_capabilities: Vec::new(),
    }
}

fn runtime_evidence_binding(conversation_id: &ConversationId, now: DateTime<Utc>) -> ModuleBinding {
    ModuleBinding {
        id: ModuleBindingId::from("runtime-evidence-binding"),
        module_id: ContentModuleId::from("runtime-evidence-module"),
        scope: ModuleScope::Conversation,
        target_id: Some(conversation_id.0.clone()),
        conversation_id: None,
        priority: 0,
        resolution_mode: lorepia_domain::ModuleRevisionResolutionMode::Active,
        pinned_revision_id: None,
        enabled: false,
        approved: false,
        package_import_approval_id: None,
        activation_approval_id: None,
        activation_review_sha256: None,
        activation_plan_sha256: None,
        variable_overrides: VariableMap::default(),
        revision_id: ModuleRevisionId::from("runtime-evidence-revision"),
        created_at: now,
    }
}

fn applied_runtime_authority(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
) -> (
    lorepia_orchestration::ModuleMergeReview,
    lorepia_orchestration::AppliedModuleRuntimePlan,
) {
    let now = Utc::now();
    let snapshot = runtime_evidence_module_snapshot(now);
    let context = runtime_evidence_context(conversation_id, branch_id);
    let mut proposed = runtime_evidence_binding(conversation_id, now);
    let activation_review = lorepia_orchestration::review_module_activation(
        None,
        &context,
        &[],
        &proposed,
        std::slice::from_ref(&snapshot),
    )
    .expect("activation review");
    let activation_plan = lorepia_orchestration::resolve_module_merge(
        &activation_review,
        &lorepia_orchestration::ModuleMergeResolutionSet {
            expected_review_sha256: activation_review.review_sha256.clone(),
            resolutions: Vec::new(),
        },
    )
    .expect("activation plan");
    let approval = lorepia_orchestration::approve_module_activation_plan(
        &activation_plan,
        &lorepia_orchestration::ModuleActivationApproval {
            approval_id: "runtime-evidence-approval".to_owned(),
            expected_review_sha256: activation_review.review_sha256.clone(),
            expected_plan_sha256: activation_plan.plan_sha256.clone(),
        },
    )
    .expect("activation approval");
    proposed.enabled = true;
    proposed.approved = true;
    proposed.activation_approval_id = Some(approval.approval_id.clone());
    proposed.activation_review_sha256 = Some(activation_review.review_sha256.clone());
    proposed.activation_plan_sha256 = Some(activation_plan.plan_sha256);
    let runtime_review = lorepia_orchestration::review_module_merge(
        1,
        &context,
        &[proposed],
        std::slice::from_ref(&snapshot),
    )
    .expect("runtime review");
    let runtime =
        lorepia_orchestration::materialize_approved_module_runtime_plan(&approval, &runtime_review)
            .expect("runtime plan");
    (activation_review, runtime)
}

fn runtime_evidence_generation_record(
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    runtime: &lorepia_orchestration::AppliedModuleRuntimePlan,
) -> GenerationPromptPlanRecord {
    GenerationPromptPlanRecord {
        id: "runtime-evidence-prompt-plan".to_owned(),
        generation_id: GenerationId("runtime-evidence-generation".to_owned()),
        conversation_id: conversation_id.clone(),
        branch_id: branch_id.clone(),
        head_message_id: None,
        latest_user_message_id: MessageId("runtime-evidence-user-message".to_owned()),
        prompt_preset_id: PromptPresetId::from("runtime-evidence-preset"),
        prompt_preset_revision_id: "runtime-evidence-preset-revision".to_owned(),
        model_route_id: None,
        generation_preset_id: None,
        task_profile_revision_id: None,
        random_seed: None,
        tokenizer_id: "runtime-evidence-tokenizer".to_owned(),
        tokenizer_version: "1".to_owned(),
        plan: VersionedJson {
            schema_version: 1,
            value: serde_json::json!({}),
        },
        plan_sha256: test_digest("runtime-evidence-prompt-plan").into_inner(),
        input_fingerprint_sha256: test_digest("runtime-evidence-input").into_inner(),
        context_limit_tokens: 1,
        estimated_input_tokens: 0,
        reserved_output_tokens: 0,
        final_input_tokens: 0,
        cacheable_prefix_tokens: 0,
        provider_request: ProviderRequestSnapshotRecord {
            id: "runtime-evidence-provider-request".to_owned(),
            api_family: ApiFamily::OpenAiResponses,
            request_schema_version: 1,
            request: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({}),
            },
            mapping_diagnostics: VersionedJson {
                schema_version: 1,
                value: serde_json::json!({
                    "module_plan_sha256": runtime.applied_plan_sha256,
                }),
            },
            created_at: Utc::now(),
        },
        created_at: Utc::now(),
    }
}

fn seed_runtime_evidence_conversation(
    transaction: &Transaction<'_>,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    now: &str,
) {
    let source_sha256 = test_digest("runtime-evidence-character-source");
    transaction
        .execute(
            "INSERT INTO content_sources
                     (sha256, relative_path, size_bytes, created_at)
                     VALUES (?1, 'sources/runtime-evidence', 1, ?2)",
            params![source_sha256.as_str(), now],
        )
        .expect("insert runtime evidence source");
    transaction
        .execute(
            "INSERT INTO characters
                     (id, name, description, source_hash, avatar_asset_hash, created_at)
                     VALUES ('runtime-character', 'Runtime', '', ?1, NULL, ?2)",
            params![source_sha256.as_str(), now],
        )
        .expect("insert runtime evidence character");
    transaction
        .execute(
            "INSERT INTO conversations
                     (id, character_id, title, created_at, updated_at)
                     VALUES (?1, 'runtime-character', 'Runtime', ?2, ?2)",
            params![conversation_id.0, now],
        )
        .expect("insert runtime evidence conversation");
    transaction
        .execute(
            "INSERT INTO conversation_branches
                     (id, conversation_id, title, fork_message_id, head_message_id,
                      created_at, updated_at)
                     VALUES (?1, ?2, NULL, NULL, NULL, ?3, ?3)",
            params![branch_id.0, conversation_id.0, now],
        )
        .expect("insert runtime evidence branch");
}

fn seed_runtime_activation_authority(
    transaction: &Transaction<'_>,
    activation_review: &lorepia_orchestration::ModuleMergeReview,
    runtime: &lorepia_orchestration::AppliedModuleRuntimePlan,
    now: &str,
) {
    let source_approval = &runtime.source_approval;
    let activation_plan_id = "runtime-evidence-activation-row";
    let activation_binding_id = source_approval
        .plan
        .activation_binding_ids
        .first()
        .expect("activation binding id");
    let review_json = serde_json::to_string(activation_review).expect("activation review JSON");
    let approval_json = serde_json::to_string(source_approval).expect("activation approval JSON");
    transaction
        .execute(
            "INSERT INTO module_activation_plans
                     (id, scope_kind, expected_bindings_revision_sha256,
                      input_module_revisions_json, conflicts_json, resolutions_json,
                      merge_sha256, plan_sha256, activation_binding_id, review_json,
                      approved_plan_json, approval_id, approval_sha256, state,
                     revision, prepared_at, approved_at, applied_at)
                     VALUES (?1, 'conversation', ?2, '[]', '[]', '[]', ?3, ?4,
                             ?5, ?6, ?7, ?8, ?9, 'prepared', 1, ?10, NULL, NULL)",
            params![
                activation_plan_id,
                activation_review.review_sha256.as_str(),
                sha256_hex(b"[]"),
                source_approval.plan.plan_sha256.as_str(),
                activation_binding_id.as_str(),
                review_json,
                approval_json,
                source_approval.approval_id,
                source_approval.approval_sha256.as_str(),
                now,
            ],
        )
        .expect("insert prepared activation authority");
    assert_eq!(
        transaction
            .execute(
                "UPDATE module_activation_plans
                     SET state = 'approved', revision = 2, approved_at = ?2
                     WHERE id = ?1 AND state = 'prepared' AND revision = 1",
                params![activation_plan_id, now],
            )
            .expect("approve activation authority"),
        1
    );
    assert_eq!(
        transaction
            .execute(
                "UPDATE module_activation_plans
                     SET state = 'applied', revision = 3, applied_at = ?2
                     WHERE id = ?1 AND state = 'approved' AND revision = 2",
                params![activation_plan_id, now],
            )
            .expect("apply activation authority"),
        1
    );
    persist_applied_module_runtime_plan_transaction(transaction, runtime, Utc::now())
        .expect("persist applied runtime plan");
}

fn applied_runtime_generation_fixture() -> AppliedRuntimeGenerationFixture {
    let root = tempfile::tempdir().expect("temporary storage root");
    let storage = Storage::open(root.path()).expect("open storage");
    let conversation_id = ConversationId("runtime-evidence-conversation".to_owned());
    let branch_id = ConversationBranchId("runtime-evidence-branch".to_owned());
    let (activation_review, runtime) = applied_runtime_authority(&conversation_id, &branch_id);
    let generation = runtime_evidence_generation_record(&conversation_id, &branch_id, &runtime);
    let mut connection = storage.connection().expect("storage connection");
    let transaction = connection.transaction().expect("fixture transaction");
    let now = Utc::now().to_rfc3339();
    seed_runtime_evidence_conversation(&transaction, &conversation_id, &branch_id, &now);
    seed_runtime_activation_authority(&transaction, &activation_review, &runtime, &now);
    transaction.commit().expect("commit runtime fixture");
    drop(connection);

    AppliedRuntimeGenerationFixture {
        root,
        storage,
        activation_review,
        runtime,
        generation,
    }
}

fn load_runtime_generation_evidence(
    storage: &Storage,
    generation: &GenerationPromptPlanRecord,
) -> CoreResult<Option<lorepia_orchestration::AppliedModuleRuntimePlan>> {
    let mut connection = storage.connection()?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let result = load_generation_module_plan_evidence(&transaction, generation);
    transaction.commit().map_err(storage_db_error)?;
    result
}

#[test]
fn persisted_runtime_generation_evidence_survives_restart() {
    let fixture = applied_runtime_generation_fixture();
    assert_eq!(
        fixture.activation_review.review_sha256,
        fixture.runtime.source_approval.plan.review_sha256
    );
    assert_eq!(
        load_runtime_generation_evidence(&fixture.storage, &fixture.generation)
            .expect("load persisted runtime generation evidence"),
        Some(fixture.runtime.clone())
    );

    let AppliedRuntimeGenerationFixture {
        root,
        storage,
        activation_review: _,
        runtime,
        generation,
    } = fixture;
    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen runtime evidence storage");
    assert_eq!(
        load_runtime_generation_evidence(&reopened, &generation)
            .expect("load runtime generation evidence after restart"),
        Some(runtime)
    );
}

#[test]
fn persisted_runtime_generation_evidence_rejects_wrong_source_authority() {
    let fixture = applied_runtime_generation_fixture();
    let source = &fixture.runtime.source_approval;
    let replacement = lorepia_orchestration::approve_module_activation_plan(
        &source.plan,
        &lorepia_orchestration::ModuleActivationApproval {
            approval_id: "runtime-evidence-replacement-approval".to_owned(),
            expected_review_sha256: source.plan.review_sha256.clone(),
            expected_plan_sha256: source.plan.plan_sha256.clone(),
        },
    )
    .expect("replacement activation approval");
    {
        let connection = fixture.storage.connection().expect("storage connection");
        connection
            .execute_batch("DROP TRIGGER module_activation_plans_transition_guard;")
            .expect("disable immutable activation guard in synthetic corruption fixture");
        connection
            .execute(
                "UPDATE module_activation_plans
                     SET approved_plan_json = ?1
                     WHERE plan_sha256 = ?2",
                params![
                    serde_json::to_string(&replacement).expect("replacement approval JSON"),
                    source.plan.plan_sha256.as_str(),
                ],
            )
            .expect("tamper source activation authority");
    }

    let error = load_runtime_generation_evidence(&fixture.storage, &fixture.generation)
        .expect_err("wrong runtime source authority must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn persisted_runtime_generation_evidence_rejects_tampered_runtime() {
    let fixture = applied_runtime_generation_fixture();
    let mut tampered = serde_json::to_value(&fixture.runtime).expect("applied runtime plan JSON");
    tampered["applied_plan_sha256"] =
        Value::String(test_digest("tampered-applied-runtime-plan").into_inner());
    {
        let connection = fixture.storage.connection().expect("storage connection");
        connection
            .execute_batch("DROP TRIGGER applied_module_runtime_plans_identity_guard;")
            .expect("disable immutable runtime guard in synthetic corruption fixture");
        connection
            .execute(
                "UPDATE applied_module_runtime_plans
                     SET runtime_plan_json = ?1
                     WHERE applied_plan_sha256 = ?2",
                params![
                    serde_json::to_string(&tampered).expect("tampered runtime JSON"),
                    fixture.runtime.applied_plan_sha256.as_str(),
                ],
            )
            .expect("tamper applied runtime payload");
    }

    let error = load_runtime_generation_evidence(&fixture.storage, &fixture.generation)
        .expect_err("tampered applied runtime must fail closed");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}
