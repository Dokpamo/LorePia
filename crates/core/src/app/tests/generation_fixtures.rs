fn imported_core() -> (tempfile::TempDir, Core, Character) {
    let root = tempdir().expect("temp root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut card = NamedTempFile::new_in(root.path()).expect("card");
    write!(
        card,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Segu","description":"Guide"}}}}"#
    )
    .expect("write card");
    let inspection = core.inspect_import(card.path()).expect("inspect");
    let character = core.commit_import(&inspection.id).expect("commit");
    (root, core, character)
}

const HARD_CRASH_GENERATION_ROOT_ENV: &str = "LOREPIA_TEST_HARD_CRASH_GENERATION_ROOT";
const HARD_CRASH_GENERATION_PRESERVE_ENV: &str = "LOREPIA_TEST_HARD_CRASH_GENERATION_PRESERVE";
const HARD_CRASH_GENERATION_REOPEN_PRESERVE_ENV: &str =
    "LOREPIA_TEST_HARD_CRASH_GENERATION_REOPEN_PRESERVE";
const HARD_CRASH_GENERATION_PARTIAL_ENV: &str = "LOREPIA_TEST_HARD_CRASH_GENERATION_PARTIAL";
const HARD_CRASH_GENERATION_EXIT_CODE: i32 = 86;

#[derive(Debug, Serialize, serde::Deserialize)]
struct HardCrashGenerationFixture {
    conversation_id: String,
    branch_id: String,
    user_message_id: String,
    assistant_message_id: String,
    generation_id: String,
    running_attempt_revision: u64,
    partial: String,
}

fn hard_crash_generation_fixture_path(root: &Path) -> PathBuf {
    root.join("hard-crash-generation-fixture.json")
}

fn run_hard_crash_generation_child(
    root: &Path,
    preserve_partial_generations: bool,
    reopen_preserve_partial_generations: bool,
    partial: &str,
) -> HardCrashGenerationFixture {
    let output = Command::new(std::env::current_exe().expect("current Core test executable"))
        .arg("--exact")
        .arg("app::tests::hard_crash_generation_fixture_child")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(HARD_CRASH_GENERATION_ROOT_ENV, root)
        .env(
            HARD_CRASH_GENERATION_PRESERVE_ENV,
            if preserve_partial_generations {
                "true"
            } else {
                "false"
            },
        )
        .env(
            HARD_CRASH_GENERATION_REOPEN_PRESERVE_ENV,
            if reopen_preserve_partial_generations {
                "true"
            } else {
                "false"
            },
        )
        .env(HARD_CRASH_GENERATION_PARTIAL_ENV, partial)
        .output()
        .expect("run hard-crash generation child");
    assert_eq!(
        output.status.code(),
        Some(HARD_CRASH_GENERATION_EXIT_CODE),
        "hard-crash child did not reach its deliberate process exit\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let fixture = fs::read(hard_crash_generation_fixture_path(root))
        .expect("read hard-crash generation fixture");
    serde_json::from_slice(&fixture).expect("decode hard-crash generation fixture")
}

#[derive(Debug, PartialEq, Eq)]
struct GenerationLifecycleRow {
    occurrence_id: String,
    event_kind: String,
    status: String,
    exact_head_message_id: Option<String>,
    owner_message_id: Option<String>,
}

fn hard_crash_database_path(root: &Path) -> PathBuf {
    fs::read_dir(root.join("db/schema-cutover"))
        .expect("read hard-crash database generations")
        .filter_map(|entry| {
            let entry = entry.expect("read hard-crash database generation");
            let manifest = fs::read(entry.path().join("generation-manifest.json")).ok()?;
            let manifest = serde_json::from_slice::<serde_json::Value>(&manifest)
                .expect("decode hard-crash database manifest");
            Some((
                manifest["activation_sequence"]
                    .as_u64()
                    .expect("hard-crash database activation sequence"),
                root.join(
                    manifest["active_database_relative_path"]
                        .as_str()
                        .expect("hard-crash active database path"),
                ),
            ))
        })
        .max_by_key(|(sequence, _)| *sequence)
        .map(|(_, path)| path)
        .expect("committed hard-crash database generation")
}

fn generation_lifecycle_rows(root: &Path, generation_id: &str) -> Vec<GenerationLifecycleRow> {
    let database = rusqlite::Connection::open(hard_crash_database_path(root))
        .expect("open lifecycle database");
    let mut statement = database
        .prepare(
            "SELECT occurrence_id, event_kind, status,
                        exact_head_message_id, owner_message_id
                 FROM core_lifecycle_outbox
                 WHERE generation_id = ?1
                   AND event_kind IN ('after_generation', 'message_committed')
                 ORDER BY occurrence_id",
        )
        .expect("prepare lifecycle query");
    statement
        .query_map([generation_id], |row| {
            Ok(GenerationLifecycleRow {
                occurrence_id: row.get(0)?,
                event_kind: row.get(1)?,
                status: row.get(2)?,
                exact_head_message_id: row.get(3)?,
                owner_message_id: row.get(4)?,
            })
        })
        .expect("query lifecycle rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect lifecycle rows")
}

fn hard_crash_assistant_content(root: &Path, assistant_message_id: &str) -> String {
    rusqlite::Connection::open(hard_crash_database_path(root))
        .expect("open hard-crash message database")
        .query_row(
            "SELECT content FROM messages WHERE id = ?1",
            [assistant_message_id],
            |row| row.get(0),
        )
        .expect("read durable hard-crash assistant content")
}

fn prompt_attempt_test_provenance(source_id: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::UserCreated,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("a5".repeat(32)),
        author: Some("Synthetic prompt-attempt test".to_owned()),
        license: Some("LicenseRef-Synthetic-Test".to_owned()),
        imported_at: None,
    }
}

fn install_generation_transform_fixture(
    core: &Core,
    conversation_id: &ConversationId,
    transform_set: &TransformSet,
    preset_id: &str,
    binding_id: &str,
) -> String {
    let transform_revision = core
        .upsert_transform_set(transform_set, None)
        .expect("save generation transform set")
        .revision_id
        .expect("generation transform set revision id");
    let now = Utc::now();
    let mut prompt_preset = lorepia_orchestration::default_prompt_preset(
        lorepia_domain::PromptPresetId::from(preset_id),
        "Synthetic generation transform preset",
        PresetMetadata {
            description: "Synthetic terminal transform fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: prompt_attempt_test_provenance(preset_id),
            created_at: now,
            updated_at: now,
            local_override_of: None,
        },
    );
    for block in &mut prompt_preset.blocks {
        block.provenance = prompt_attempt_test_provenance(block.id.as_str());
    }
    prompt_preset
        .transform_set_ids
        .push(transform_set.id.clone());
    core.upsert_prompt_preset(&prompt_preset, None)
        .expect("save generation transform prompt preset");
    core.bind_prompt_preset(
        &PromptPresetBinding {
            id: binding_id.to_owned(),
            prompt_preset_id: prompt_preset.id,
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
    .expect("bind generation transform prompt preset");
    transform_revision
}

fn fail_open_generation_transform_set() -> TransformSet {
    let set_id = TransformSetId::from("synthetic.fail-open.set");
    TransformSet {
        id: set_id.clone(),
        name: "Synthetic fail-open transforms".to_owned(),
        schema_version: 1,
        enabled: true,
        imported_author_enabled: false,
        rules: vec![
            TransformRule {
                id: TransformRuleId::from("synthetic.fail-open.invalid-regex"),
                name: "Invalid regex".to_owned(),
                enabled: true,
                imported_enabled: false,
                imported_author_enabled: false,
                phase: TransformPhase::ProviderOutputCanonical,
                order: 0,
                pattern: SafeRegex {
                    pattern: "(".to_owned(),
                    case_insensitive: false,
                },
                replacement: "must-not-appear".to_owned(),
                condition: None,
                max_replacements: 8,
                input_limit: 1_024,
                output_limit: 1_024,
                provenance: prompt_attempt_test_provenance("invalid-regex"),
            },
            TransformRule {
                id: TransformRuleId::from("synthetic.fail-open.output-limit"),
                name: "Output limit".to_owned(),
                enabled: true,
                imported_enabled: false,
                imported_author_enabled: false,
                phase: TransformPhase::ProviderOutputCanonical,
                order: 1,
                pattern: SafeRegex {
                    pattern: "Synthetic".to_owned(),
                    case_insensitive: false,
                },
                replacement: "X".repeat(64),
                condition: None,
                max_replacements: 8,
                input_limit: 1_024,
                output_limit: 32,
                provenance: prompt_attempt_test_provenance("output-limit"),
            },
        ],
        max_rules_per_phase: 8,
        max_output_chars: 1_024,
        provenance: prompt_attempt_test_provenance(set_id.as_str()),
    }
}

fn display_only_generation_transform_set() -> TransformSet {
    let set_id = TransformSetId::from("synthetic.display-only.set");
    TransformSet {
        id: set_id.clone(),
        name: "Synthetic DisplayOnly".to_owned(),
        schema_version: 1,
        enabled: true,
        imported_author_enabled: false,
        rules: vec![TransformRule {
            id: TransformRuleId::from("synthetic.display-only.rule"),
            name: "Render-only wording".to_owned(),
            enabled: true,
            imported_enabled: false,
            imported_author_enabled: false,
            phase: TransformPhase::DisplayOnly,
            order: 0,
            pattern: SafeRegex {
                pattern: "Synthetic".to_owned(),
                case_insensitive: false,
            },
            replacement: "Rendered".to_owned(),
            condition: None,
            max_replacements: 8,
            input_limit: 1_024,
            output_limit: 1_024,
            provenance: prompt_attempt_test_provenance("synthetic.display-only.rule"),
        }],
        max_rules_per_phase: 8,
        max_output_chars: 1_024,
        provenance: prompt_attempt_test_provenance(set_id.as_str()),
    }
}

fn assert_display_only_events(events: &[ChatEvent], generation_id: &GenerationId) -> String {
    let events = events
        .iter()
        .filter(|event| event.generation_id == *generation_id)
        .collect::<Vec<_>>();
    assert_eq!(events.first().map(|event| event.sequence), Some(1));
    assert!(
        events
            .windows(2)
            .all(|events| events[1].sequence > events[0].sequence)
    );
    let streamed_display = events
        .iter()
        .filter_map(|event| match &event.kind {
            ChatEventKind::TextDelta(delta) => Some(delta.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(streamed_display, "Rendered reply");
    let display_delta = events
        .iter()
        .position(|event| matches!(event.kind, ChatEventKind::TextDelta(_)))
        .expect("deferred DisplayOnly text delta");
    let committed = events
        .iter()
        .position(|event| matches!(event.kind, ChatEventKind::MessageCommitted { .. }))
        .expect("message committed event");
    let finished = events
        .iter()
        .position(|event| matches!(event.kind, ChatEventKind::GenerationFinished))
        .expect("generation finished event");
    assert!(display_delta < committed && committed < finished);
    streamed_display
}

fn assert_display_only_projection(
    presentation: &MessagePresentation,
    transform_revision_id: &str,
    streamed_display: &str,
) {
    assert_eq!(presentation.message.content, "Synthetic reply");
    assert_eq!(presentation.display_content, streamed_display);
    assert!(presentation.projection_diagnostics_sha256.is_some());
    assert_eq!(presentation.transform_diagnostics.len(), 1);
    let diagnostic = &presentation.transform_diagnostics[0];
    assert_eq!(
        diagnostic.set_revision_id.as_deref(),
        Some(transform_revision_id)
    );
    assert_eq!(
        diagnostic.rule_id.as_deref(),
        Some("synthetic.display-only.rule")
    );
    assert_eq!(diagnostic.stage, MessageTransformStage::DisplayOnly);
    assert_eq!(diagnostic.disposition, MessageTransformDisposition::Applied);
    assert!(diagnostic.code.is_none());
    assert_eq!(
        diagnostic.before_sha256,
        transform_content_sha256("Synthetic reply")
    );
    assert_eq!(
        diagnostic.after_sha256.as_ref(),
        Some(&transform_content_sha256("Rendered reply"))
    );
    let diagnostic_json =
        serde_json::to_string(diagnostic).expect("serialize public transform diagnostic");
    for forbidden in ["Synthetic reply", "Rendered reply", "Synthetic", "Rendered"] {
        assert!(!diagnostic_json.contains(forbidden));
    }
}

fn prompt_source_test_block(
    preset_id: &lorepia_domain::PromptPresetId,
    suffix: &str,
    name: &str,
    kind: PromptBlockKind,
    source: BlockSource,
    placement_zone: PlacementZone,
) -> PromptBlock {
    PromptBlock {
        id: PromptBlockId::from(format!("{}.{}", preset_id.as_str(), suffix)),
        name: name.to_owned(),
        kind,
        enabled: true,
        role_hint: RoleHint::System,
        authority: InstructionAuthority::Creator,
        template: None,
        condition: None,
        source,
        placement_zone,
        history_selector: None,
        token_policy: TokenPolicy {
            priority: 900,
            min_tokens: None,
            max_tokens: Some(1_024),
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::TrimTail,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: prompt_attempt_test_provenance(suffix),
    }
}

fn prompt_source_test_preset(summary_id: &MemoryRecordId) -> PromptPreset {
    let now = Utc::now();
    let preset_id = lorepia_domain::PromptPresetId::from("synthetic.prompt-source.preset");
    let mut preset = lorepia_orchestration::default_prompt_preset(
        preset_id.clone(),
        "Synthetic prompt sources",
        PresetMetadata {
            description: "Synthetic current-source materialization fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: prompt_attempt_test_provenance(preset_id.as_str()),
            created_at: now,
            updated_at: now,
            local_override_of: None,
        },
    );
    for block in &mut preset.blocks {
        block.provenance = prompt_attempt_test_provenance(block.id.as_str());
    }
    let mut character = preset.blocks.remove(0);
    let mut history = preset.blocks.remove(0);
    let latest = preset.blocks.remove(0);
    character.name = "Synthetic character source".to_owned();
    history.history_selector = Some(HistorySelector::SinceSummary {
        summary_id: summary_id.clone(),
    });
    let mut user_and_slot = prompt_source_test_block(
        &preset_id,
        "user-slot",
        "User and slot",
        PromptBlockKind::StaticInstruction,
        BlockSource::Template,
        PlacementZone::PresetInstruction,
    );
    user_and_slot.template = Some(SafeTemplate {
        parts: vec![
            TemplatePart::Text {
                value: "USER=".to_owned(),
            },
            TemplatePart::BuiltIn {
                value: BuiltInTemplateValue::UserName,
            },
            TemplatePart::Text {
                value: "; SLOT=".to_owned(),
            },
            TemplatePart::Slot {
                name: "tone".to_owned(),
            },
        ],
        max_output_chars: 1_024,
    });
    let group = prompt_source_test_block(
        &preset_id,
        "group",
        "Group context",
        PromptBlockKind::GroupContext,
        BlockSource::GroupContext,
        PlacementZone::CharacterContext,
    );
    let summary = prompt_source_test_block(
        &preset_id,
        "summary",
        "Conversation summary",
        PromptBlockKind::ConversationSummary,
        BlockSource::ConversationSummary,
        PlacementZone::RetrievedContext,
    );
    let author = prompt_source_test_block(
        &preset_id,
        "author",
        "Author note",
        PromptBlockKind::AuthorNote,
        BlockSource::AuthorNote,
        PlacementZone::PostHistory,
    );
    preset.blocks = vec![
        user_and_slot,
        character,
        group,
        summary,
        history,
        author,
        latest,
    ];
    preset
}

fn save_prompt_source_summary(
    core: &Core,
    branch_id: &ConversationBranchId,
    messages: &[Message],
) -> StoredRevision<MemoryRecord> {
    let [user, assistant] = messages else {
        panic!("prompt-source summary fixture requires one complete turn");
    };
    let now = Utc::now();
    let summary_id = MemoryRecordId::from("synthetic.prompt-source.summary");
    core.inner
        .storage
        .save_memory_record(
            &MemoryRecord {
                id: summary_id.clone(),
                conversation_id: user.conversation_id.clone(),
                branch_id: branch_id.clone(),
                source_start_message_id: user.id.clone(),
                source_end_message_id: assistant.id.clone(),
                kind: MemoryKind::ConversationSummary,
                title: "Synthetic exact prompt summary".to_owned(),
                summary: "SUMMARY_SOURCE_CANARY_7A31".to_owned(),
                structured_data: VersionedJson {
                    schema_version: 1,
                    value: serde_json::json!({"fixture": "prompt-source"}),
                },
                importance: 100,
                keywords: vec!["synthetic".to_owned()],
                embedding_ref: None,
                pinned: false,
                excluded_from_conversation: false,
                excluded_from_character: false,
                created_at: now,
                updated_at: now,
                invalidated_at: None,
                provenance: prompt_attempt_test_provenance(summary_id.as_str()),
            },
            None,
        )
        .expect("save prompt-source summary")
}

fn bind_prompt_source_test_preset(
    core: &Core,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    summary_id: &MemoryRecordId,
) -> Revisioned<PromptPresetBinding> {
    let preset = prompt_source_test_preset(summary_id);
    core.upsert_prompt_preset(&preset, None)
        .expect("save prompt-source preset");
    let now = Utc::now();
    core.bind_prompt_preset(
        &PromptPresetBinding {
            id: "synthetic.prompt-source.binding".to_owned(),
            prompt_preset_id: preset.id,
            scope: ModuleScope::Branch,
            target_id: Some(branch_id.0.clone()),
            conversation_id: Some(conversation_id.clone()),
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
            user_name_override: Some("USER_SOURCE_CANARY_2B64".to_owned()),
            author_note: Some("AUTHOR_SOURCE_CANARY_4C82".to_owned()),
            group_context: Some("GROUP_SOURCE_CANARY_1D53".to_owned()),
            template_slots: vec![TemplateSlot {
                name: "tone".to_owned(),
                value: "SLOT_SOURCE_CANARY_9E17".to_owned(),
            }],
            created_at: now,
            updated_at: now,
        },
        None,
    )
    .expect("bind prompt-source preset")
}

fn assert_prompt_source_preview(preview: &crate::ExpertPromptPreview) {
    let block_contents = |suffix: &str| {
        preview
            .effective_messages
            .iter()
            .filter(|message| message.block_id.as_str().ends_with(suffix))
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        block_contents(".user-slot"),
        ["USER=USER_SOURCE_CANARY_2B64; SLOT=SLOT_SOURCE_CANARY_9E17"]
    );
    assert_eq!(block_contents(".group"), ["GROUP_SOURCE_CANARY_1D53"]);
    assert_eq!(block_contents(".summary"), ["SUMMARY_SOURCE_CANARY_7A31"]);
    assert_eq!(block_contents(".author"), ["AUTHOR_SOURCE_CANARY_4C82"]);
    assert_eq!(
        block_contents(".history"),
        [
            "SINCE_SUMMARY_USER_CANARY_54C9",
            "SINCE_SUMMARY_ASSISTANT_CANARY_86E2"
        ]
    );
    assert_eq!(
        block_contents(".latest_user"),
        ["LATEST_USER_SOURCE_CANARY_03F8"]
    );
}

fn assert_prompt_source_snapshot(
    snapshot: &PromptContextSnapshotV1,
    conversation_id: &ConversationId,
    branch_id: &ConversationBranchId,
    messages: &[Message],
    summary: &StoredRevision<MemoryRecord>,
    binding: &Revisioned<PromptPresetBinding>,
) {
    assert_eq!(snapshot.conversation_id, *conversation_id);
    assert_eq!(snapshot.source_branch_id, *branch_id);
    assert_eq!(
        snapshot.context_head_message_id.as_ref(),
        Some(&messages[3].id)
    );
    assert_eq!(
        snapshot.conversation_summary_id.as_ref(),
        Some(&summary.value.id)
    );
    assert_eq!(snapshot.summaries.len(), 1);
    assert_eq!(
        snapshot.summaries[0].source_start_message_id,
        messages[0].id
    );
    assert_eq!(snapshot.summaries[0].source_end_message_id, messages[1].id);
    assert_eq!(snapshot.summaries[0].state_revision, summary.revision);
    assert_eq!(
        snapshot.summaries[0].active_revision_id.as_str(),
        summary
            .revision_id
            .as_deref()
            .expect("summary revision identity")
    );
    let summary_json =
        serde_json::to_string(&summary.value).expect("encode exact summary revision");
    assert_eq!(
        snapshot.summaries[0].active_revision_sha256,
        format!("{:x}", Sha256::digest(summary_json.as_bytes()))
    );
    let snapshot_binding = snapshot.binding.as_ref().expect("binding evidence");
    assert_eq!(snapshot_binding.binding_id, binding.value.id);
    assert_eq!(snapshot_binding.binding_revision, binding.revision);
    assert_eq!(
        snapshot_binding.document_sha256,
        binding
            .value
            .canonical_document_sha256()
            .expect("hash exact prompt binding")
    );
    assert_eq!(
        snapshot.snapshot_sha256,
        lorepia_domain::prompt_context_snapshot_sha256(snapshot)
            .expect("rehash prompt context snapshot")
    );
    let snapshot_json = serde_json::to_string(snapshot).expect("encode prompt source evidence");
    for source_text in [
        "USER_SOURCE_CANARY_2B64",
        "AUTHOR_SOURCE_CANARY_4C82",
        "GROUP_SOURCE_CANARY_1D53",
        "SLOT_SOURCE_CANARY_9E17",
        "SUMMARY_SOURCE_CANARY_7A31",
        "SINCE_SUMMARY_USER_CANARY_54C9",
    ] {
        assert!(!snapshot_json.contains(source_text));
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the fixture keeps one exact module revision self-contained for parity validation"
)]
fn install_prompt_attempt_parity_module(
    core: &Core,
    runtime_target: ContentModuleRuntimeTarget,
) -> (VariableRef, VariableRef, KnowledgeEntryId, String) {
    let module_id = ContentModuleId::from("synthetic.prompt-attempt.parity-module");
    let variable = VariableRef {
        scope: VariableScope::Module,
        namespace: Some(module_id.clone()),
        id: VariableId::from("synthetic.prompt-attempt.parity-module.approval-state"),
    };
    let temporal_roll = VariableRef {
        scope: VariableScope::Module,
        namespace: Some(module_id.clone()),
        id: VariableId::from("synthetic.prompt-attempt.parity-module.temporal-roll"),
    };
    let knowledge_book_id = KnowledgeBookId::from("synthetic.prompt-attempt.knowledge");
    let knowledge_entry_id = KnowledgeEntryId::from("synthetic.prompt-attempt.knowledge.manual");
    core.upsert_knowledge_book(
        &KnowledgeBook {
            id: knowledge_book_id.clone(),
            name: "Synthetic attempt-owned knowledge".to_owned(),
            schema_version: 1,
            entries: vec![KnowledgeEntry {
                id: knowledge_entry_id.clone(),
                book_id: knowledge_book_id.clone(),
                name: "Attempt-owned manual knowledge".to_owned(),
                content: "SYNTHETIC_ATTEMPT_MANUAL_KNOWLEDGE_6A91".to_owned(),
                enabled: true,
                activation: ActivationRule::Manual,
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
                provenance: prompt_attempt_test_provenance(
                    "synthetic.prompt-attempt.knowledge.manual",
                ),
            }],
            scan_depth: 8,
            token_budget: TokenBudget { max_tokens: 64 },
            recursive: false,
            max_recursion_depth: 0,
            provenance: prompt_attempt_test_provenance("synthetic.prompt-attempt.knowledge"),
        },
        None,
    )
    .expect("save attempt-owned knowledge book");

    let proposal_id = "synthetic.prompt-attempt.approval".to_owned();
    let rule_set = InteractionRuleSet {
        id: InteractionRuleSetId::from("synthetic.prompt-attempt.rules"),
        name: "Synthetic prompt-attempt rules".to_owned(),
        schema_version: 1,
        rules: vec![
            InteractionRule {
                id: InteractionRuleId::from("synthetic.prompt-attempt.rules.before"),
                name: "Prepare prompt state before generation".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::BeforeGeneration,
                condition: None,
                actions: vec![
                    InteractionAction::SetVariable {
                        target: variable.clone(),
                        value: ValueExpr::Literal {
                            value: VariableValue::Text("before-approval".to_owned()),
                        },
                    },
                    InteractionAction::ActivateKnowledge {
                        entry_id: knowledge_entry_id.clone(),
                    },
                    InteractionAction::RollDice {
                        expression: DiceExpression {
                            count: 1,
                            sides: 10_000,
                            modifier: 0,
                        },
                        target: Some(temporal_roll.clone()),
                    },
                    InteractionAction::AppendVisibleSystemEvent {
                        text: SafeTemplate {
                            parts: vec![TemplatePart::BuiltIn {
                                value: BuiltInTemplateValue::CurrentTime,
                            }],
                            max_output_chars: 64,
                        },
                    },
                    InteractionAction::RequestUserApproval {
                        proposal: ProposalSpec {
                            id: proposal_id.clone(),
                            title: "Approve synthetic prompt state".to_owned(),
                            body: SafeTemplate {
                                parts: vec![TemplatePart::BuiltIn {
                                    value: BuiltInTemplateValue::CurrentTime,
                                }],
                                max_output_chars: 64,
                            },
                            expires_after_seconds: None,
                        },
                    },
                ],
                priority: 0,
                stop_after_match: false,
                provenance: prompt_attempt_test_provenance("synthetic.prompt-attempt.rules.before"),
            },
            InteractionRule {
                id: InteractionRuleId::from("synthetic.prompt-attempt.rules.approved"),
                name: "Apply approved prompt state".to_owned(),
                enabled: true,
                imported_author_enabled: false,
                event: InteractionEvent::UserAction {
                    action_id: proposal_id.clone(),
                },
                condition: None,
                actions: vec![InteractionAction::SetVariable {
                    target: variable.clone(),
                    value: ValueExpr::Literal {
                        value: VariableValue::Text("approved-for-prompt".to_owned()),
                    },
                }],
                priority: 0,
                stop_after_match: false,
                provenance: prompt_attempt_test_provenance(
                    "synthetic.prompt-attempt.rules.approved",
                ),
            },
        ],
        max_actions_per_event: 8,
        provenance: prompt_attempt_test_provenance("synthetic.prompt-attempt.rules"),
    };
    core.upsert_interaction_rule_set(&rule_set, None)
        .expect("save prompt-attempt rule set");

    let module = ContentModule {
        id: module_id.clone(),
        name: "Synthetic prompt-attempt parity module".to_owned(),
        version: "1.0.0".to_owned(),
        schema_version: 1,
        prompt_fragments: vec![
            PromptBlock {
                id: PromptBlockId::from("synthetic.prompt-attempt.variable-block"),
                name: "Attempt-owned variable marker".to_owned(),
                kind: PromptBlockKind::StaticInstruction,
                enabled: true,
                role_hint: RoleHint::System,
                authority: InstructionAuthority::Creator,
                template: Some(SafeTemplate {
                    parts: vec![
                        TemplatePart::Text {
                            value: "SYNTHETIC_ATTEMPT_VARIABLE=".to_owned(),
                        },
                        TemplatePart::Variable {
                            variable: variable.clone(),
                        },
                        TemplatePart::Text {
                            value: ";SYNTHETIC_ATTEMPT_TIME_ROLL=".to_owned(),
                        },
                        TemplatePart::Variable {
                            variable: temporal_roll.clone(),
                        },
                        TemplatePart::Text {
                            value: ";SYNTHETIC_ATTEMPT_DATE=".to_owned(),
                        },
                        TemplatePart::BuiltIn {
                            value: BuiltInTemplateValue::CurrentDate,
                        },
                        TemplatePart::Text {
                            value: ";SYNTHETIC_ATTEMPT_TIME=".to_owned(),
                        },
                        TemplatePart::BuiltIn {
                            value: BuiltInTemplateValue::CurrentTime,
                        },
                    ],
                    max_output_chars: 256,
                }),
                condition: None,
                source: BlockSource::Template,
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
                provenance: prompt_attempt_test_provenance(
                    "synthetic.prompt-attempt.variable-block",
                ),
            },
            PromptBlock {
                id: PromptBlockId::from("synthetic.prompt-attempt.knowledge-block"),
                name: "Attempt-owned selected knowledge".to_owned(),
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
                    max_tokens: Some(64),
                    reserve_tokens: None,
                },
                overflow_policy: OverflowPolicy::Reject,
                merge_policy: MergePolicy::SeparateMessage,
                provenance: prompt_attempt_test_provenance(
                    "synthetic.prompt-attempt.knowledge-block",
                ),
            },
        ],
        knowledge_book_ids: vec![knowledge_book_id],
        control_specs: vec![
            ControlSpec {
                id: ControlId::from("synthetic.prompt-attempt.approval-state"),
                label: "Synthetic approval state".to_owned(),
                description: "Synthetic test-only variable".to_owned(),
                kind: ControlKind::Text,
                value_type: Some(VariableType::Text),
                variable: Some(variable.clone()),
                default_value: Some(VariableValue::Text("initial".to_owned())),
                options: Vec::new(),
                minimum: None,
                maximum: None,
                step: None,
                visible_when: None,
                scope: VariableScope::Module,
                sensitive: false,
                requires_regeneration: true,
            },
            ControlSpec {
                id: ControlId::from("synthetic.prompt-attempt.temporal-roll"),
                label: "Synthetic temporal roll".to_owned(),
                description: "Attempt-time seeded synthetic test variable".to_owned(),
                kind: ControlKind::Number,
                value_type: Some(VariableType::Integer),
                variable: Some(temporal_roll.clone()),
                default_value: Some(VariableValue::Integer(0)),
                options: Vec::new(),
                minimum: Some(0.0),
                maximum: Some(10_000.0),
                step: Some(1.0),
                visible_when: None,
                scope: VariableScope::Module,
                sensitive: false,
                requires_regeneration: true,
            },
        ],
        transform_set_ids: Vec::new(),
        interaction_rule_set_ids: vec![rule_set.id],
        asset_ids: Vec::new(),
        imported_components_enabled: true,
        required_capabilities: vec![
            ContentCapability::PromptFragments,
            ContentCapability::Knowledge,
            ContentCapability::Variables,
            ContentCapability::DeclarativeInteractions,
        ],
        metadata: PackageMetadata {
            author: Some("Synthetic prompt-attempt test".to_owned()),
            license: "LicenseRef-Synthetic-Test".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: "Synthetic prompt-attempt parity fixture".to_owned(),
            tags: vec!["synthetic".to_owned()],
            provenance: prompt_attempt_test_provenance("synthetic.prompt-attempt.parity-module"),
        },
    };
    core.upsert_content_module(&module, None)
        .expect("save prompt-attempt parity module");
    let mut initial_variables = VariableMap::default();
    initial_variables.insert(variable.clone(), VariableValue::Text("initial".to_owned()));
    initial_variables.insert(temporal_roll.clone(), VariableValue::Integer(0));
    let request = ContentModuleActivationRequest {
        runtime_target,
        expected_binding_revision: None,
        binding: ContentModuleBindingDraft {
            id: ModuleBindingId::from("synthetic.prompt-attempt.parity-binding"),
            module_id,
            scope: ModuleScope::App,
            target_id: None,
            conversation_id: None,
            priority: 0,
            resolution_mode: ModuleRevisionResolutionMode::Active,
            pinned_revision_id: None,
            package_import_approval_id: None,
            variable_overrides: initial_variables,
        },
    };
    let review = core
        .review_content_module_activation(&request)
        .expect("review prompt-attempt module activation");
    let resolutions = ModuleMergeResolutionSet {
        expected_review_sha256: review.review_sha256.clone(),
        resolutions: Vec::new(),
    };
    let plan = core
        .resolve_content_module_activation(&request, &resolutions)
        .expect("resolve prompt-attempt module activation");
    core.activate_content_module(
        &request,
        &resolutions,
        &ModuleActivationApproval {
            approval_id: "synthetic-prompt-attempt-activation".to_owned(),
            expected_review_sha256: review.review_sha256,
            expected_plan_sha256: plan.plan_sha256,
        },
    )
    .expect("activate prompt-attempt module")
    .verify()
    .expect("verify prompt-attempt activation receipt");
    (variable, temporal_roll, knowledge_entry_id, proposal_id)
}

fn poison_generation_registry(core: &Core) {
    let registry = Arc::clone(&core.inner.active_generations);
    let result = thread::spawn(move || {
        let _guard = registry.active.lock().expect("registry lock");
        panic!("synthetic generation registry failure");
    })
    .join();
    assert!(result.is_err(), "registry poison thread must panic");
}

fn wait_for_partial(core: &Core, conversation_id: &ConversationId, expected: &str) -> Message {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let messages = core.list_messages(conversation_id).expect("messages");
        if let Some(message) = messages.get(1)
            && message.content == expected
        {
            return message.clone();
        }
        assert!(
            Instant::now() < deadline,
            "partial checkpoint was not persisted"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_generation_status(
    core: &Core,
    generation_id: &GenerationId,
    expected: GenerationStatus,
) -> GenerationRecord {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let generation = core
            .inner
            .storage
            .get_generation(generation_id)
            .expect("generation snapshot");
        if generation.status == expected {
            return generation;
        }
        assert!(
            Instant::now() < deadline,
            "generation did not reach {expected:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_generation_registry_to_drain(core: &Core) {
    wait_for_active_generation_count(core, 0);
}

fn wait_for_active_generation_count(core: &Core, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while core.active_generation_count() != expected {
        assert!(
            Instant::now() < deadline,
            "generation registry did not reach {expected} active entries"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
