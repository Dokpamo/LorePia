use std::path::Path;

use chrono::{TimeZone, Utc};
use lorepia_domain::{
    BlockResolutionStatus, BlockSource, CacheBoundary, CacheBoundaryId, CacheDirectiveStatus,
    CacheMode, CacheRoleFilter, CacheTtl, CharacterField, CharacterPromptContent,
    ConversationBranchId, ConversationId, GenerationPresetId, HistorySelector,
    InstructionAuthority, KnowledgeActivationReason, KnowledgeEntryId, KnowledgePlacement,
    MemoryRecordId, MergePolicy, MessageId, OverflowPolicy, PlacementZone, PresetMetadata,
    PromptBlock, PromptBlockId, PromptBlockKind, PromptConversationMessage, PromptMessageRole,
    PromptPreset, PromptPresetId, PromptResolutionContext, PromptResolveRequest, Provenance,
    ProviderMessageRole, ProviderPromptContract, ResolvedPromptPlan, RoleHint, SafeTemplate,
    SelectedKnowledge, SelectedMemory, SourceKind, TemplatePart, TokenPolicy,
    UnsupportedRolePolicy, VariableMap, VariableRef, VariableScope, VariableValue,
};
use lorepia_orchestration::{resolve_prompt_plan, verify_resolved_prompt_plan};

const GOLDEN_JSON: &str = include_str!("fixtures/cross_platform_resolved_prompt_plan.json");
const GOLDEN_SHA256: &str = include_str!("fixtures/cross_platform_resolved_prompt_plan.sha256");
const UPDATE_GOLDEN_ENV: &str = "LOREPIA_UPDATE_CROSS_PLATFORM_GOLDEN";
const CREDENTIAL_CANARY: &str = "credential-canary-must-never-enter-a-prompt-plan";

#[derive(Debug, Clone, Copy)]
enum NativePlatform {
    Android,
    Ios,
    Macos,
    Windows,
}

fn fixed_provenance(source_kind: SourceKind, source_id: &str) -> Provenance {
    Provenance {
        source_kind,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("11".repeat(32)),
        author: Some("Synthetic Fixture".to_owned()),
        license: Some("MIT".to_owned()),
        imported_at: None,
    }
}

fn fixed_template(text: &str) -> SafeTemplate {
    SafeTemplate {
        parts: vec![TemplatePart::Text {
            value: text.to_owned(),
        }],
        max_output_chars: 4_096,
    }
}

#[allow(clippy::too_many_arguments)]
fn template_block(
    id: &str,
    zone: PlacementZone,
    kind: PromptBlockKind,
    role_hint: RoleHint,
    authority: InstructionAuthority,
    text: &str,
    priority: u16,
    maximum_tokens: Option<u32>,
    overflow_policy: OverflowPolicy,
) -> PromptBlock {
    PromptBlock {
        id: PromptBlockId::from(id),
        name: format!("Synthetic {id}"),
        kind,
        enabled: true,
        role_hint,
        authority,
        template: Some(fixed_template(text)),
        condition: None,
        source: BlockSource::Template,
        placement_zone: zone,
        history_selector: None,
        token_policy: TokenPolicy {
            priority,
            min_tokens: None,
            max_tokens: maximum_tokens,
            reserve_tokens: None,
        },
        overflow_policy,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: fixed_provenance(SourceKind::ApplicationBuiltIn, id),
    }
}

struct SourceBlockSpec<'a> {
    id: &'a str,
    zone: PlacementZone,
    kind: PromptBlockKind,
    role_hint: RoleHint,
    source: BlockSource,
    history_selector: Option<HistorySelector>,
    maximum_tokens: Option<u32>,
    overflow_policy: OverflowPolicy,
}

fn source_block(spec: SourceBlockSpec<'_>) -> PromptBlock {
    PromptBlock {
        id: PromptBlockId::from(spec.id),
        name: format!("Synthetic {}", spec.id),
        kind: spec.kind,
        enabled: true,
        role_hint: spec.role_hint,
        authority: InstructionAuthority::Creator,
        template: None,
        condition: None,
        source: spec.source,
        placement_zone: spec.zone,
        history_selector: spec.history_selector,
        token_policy: TokenPolicy {
            priority: 50,
            min_tokens: None,
            max_tokens: spec.maximum_tokens,
            reserve_tokens: None,
        },
        overflow_policy: spec.overflow_policy,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: fixed_provenance(SourceKind::UserCreated, spec.id),
    }
}

#[allow(clippy::too_many_lines)]
fn cross_platform_request() -> PromptResolveRequest {
    let fixed_time = Utc
        .with_ymd_and_hms(2026, 8, 3, 12, 34, 56)
        .single()
        .expect("fixed fixture timestamp is valid");
    let application = template_block(
        "zone-01-application",
        PlacementZone::ApplicationPolicy,
        PromptBlockKind::StaticInstruction,
        RoleHint::System,
        InstructionAuthority::Application,
        "Fixed application policy.",
        1_000,
        None,
        OverflowPolicy::Reject,
    );
    let preset_developer = template_block(
        "zone-02-preset-b",
        PlacementZone::PresetInstruction,
        PromptBlockKind::StaticInstruction,
        RoleHint::Developer,
        InstructionAuthority::Creator,
        "Developer instruction maps to system.",
        90,
        None,
        OverflowPolicy::Reject,
    );
    let preset_provider_default = template_block(
        "zone-02-preset-a",
        PlacementZone::PresetInstruction,
        PromptBlockKind::StaticInstruction,
        RoleHint::ProviderDefault,
        InstructionAuthority::Creator,
        "Provider-default instruction.",
        90,
        None,
        OverflowPolicy::Reject,
    );
    let character = source_block(SourceBlockSpec {
        id: "zone-03-character",
        zone: PlacementZone::CharacterContext,
        kind: PromptBlockKind::CharacterDescription,
        role_hint: RoleHint::System,
        source: BlockSource::CharacterField {
            field: CharacterField::Description,
        },
        history_selector: None,
        maximum_tokens: None,
        overflow_policy: OverflowPolicy::Reject,
    });
    let knowledge = source_block(SourceBlockSpec {
        id: "zone-04-knowledge",
        zone: PlacementZone::RetrievedContext,
        kind: PromptBlockKind::WorldKnowledge,
        role_hint: RoleHint::Developer,
        source: BlockSource::SelectedKnowledge,
        history_selector: None,
        maximum_tokens: Some(15),
        overflow_policy: OverflowPolicy::ReduceKnowledgeEntries,
    });
    let memory = source_block(SourceBlockSpec {
        id: "zone-04-memory",
        zone: PlacementZone::RetrievedContext,
        kind: PromptBlockKind::RetrievedMemory,
        role_hint: RoleHint::System,
        source: BlockSource::SelectedMemory,
        history_selector: None,
        maximum_tokens: Some(17),
        overflow_policy: OverflowPolicy::KeepLatestItems,
    });
    let older_history = source_block(SourceBlockSpec {
        id: "zone-05-older-history",
        zone: PlacementZone::OlderHistory,
        kind: PromptBlockKind::HistorySlice,
        role_hint: RoleHint::ProviderDefault,
        source: BlockSource::History,
        history_selector: Some(HistorySelector::BeforeRecentTurns { recent_turns: 1 }),
        maximum_tokens: None,
        overflow_policy: OverflowPolicy::KeepLatestItems,
    });
    let recent_enhancement = source_block(SourceBlockSpec {
        id: "zone-06-recent-enhancement",
        zone: PlacementZone::RecentEnhancement,
        kind: PromptBlockKind::AuthorNote,
        role_hint: RoleHint::Developer,
        source: BlockSource::AuthorNote,
        history_selector: None,
        maximum_tokens: Some(14),
        overflow_policy: OverflowPolicy::TrimTail,
    });
    let recent_history = source_block(SourceBlockSpec {
        id: "zone-07-recent-history",
        zone: PlacementZone::RecentHistory,
        kind: PromptBlockKind::HistorySlice,
        role_hint: RoleHint::ProviderDefault,
        source: BlockSource::History,
        history_selector: Some(HistorySelector::RecentTurns { count: 1 }),
        maximum_tokens: None,
        overflow_policy: OverflowPolicy::KeepLatestItems,
    });
    let post_history = source_block(SourceBlockSpec {
        id: "zone-08-post-history",
        zone: PlacementZone::PostHistory,
        kind: PromptBlockKind::PostHistoryInstruction,
        role_hint: RoleHint::Developer,
        source: BlockSource::CharacterField {
            field: CharacterField::PostHistoryInstruction,
        },
        history_selector: None,
        maximum_tokens: None,
        overflow_policy: OverflowPolicy::Reject,
    });
    let latest_user = PromptBlock {
        id: PromptBlockId::from("zone-09-latest-user"),
        name: "Synthetic latest user".to_owned(),
        kind: PromptBlockKind::LatestUserTurn,
        enabled: true,
        role_hint: RoleHint::User,
        authority: InstructionAuthority::User,
        template: None,
        condition: None,
        source: BlockSource::LatestUser,
        placement_zone: PlacementZone::LatestUser,
        history_selector: None,
        token_policy: TokenPolicy {
            priority: u16::MAX,
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::Reject,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: fixed_provenance(SourceKind::UserCreated, "zone-09-latest-user"),
    };
    let prefill = template_block(
        "zone-10-assistant-prefill",
        PlacementZone::AssistantPrefill,
        PromptBlockKind::AssistantPrefill,
        RoleHint::Assistant,
        InstructionAuthority::Creator,
        "Fixed prefill",
        100,
        None,
        OverflowPolicy::Reject,
    );

    let preset = PromptPreset {
        id: PromptPresetId::from("cross-platform.synthetic.v1"),
        name: "Cross-platform synthetic fixture".to_owned(),
        schema_version: 1,
        blocks: vec![
            application,
            preset_developer,
            preset_provider_default,
            character,
            knowledge,
            memory,
            older_history,
            recent_enhancement,
            recent_history,
            post_history,
            latest_user,
            prefill,
        ],
        controls: Vec::new(),
        default_values: VariableMap::default(),
        default_generation_preset_id: Some(GenerationPresetId::from(
            "generation.provider-defaults.v1",
        )),
        memory_profile_id: None,
        knowledge_book_ids: Vec::new(),
        transform_set_ids: Vec::new(),
        module_ids: Vec::new(),
        cache_boundaries: vec![
            CacheBoundary {
                id: CacheBoundaryId::from("cache-applied-automatic"),
                after_block_id: PromptBlockId::from("zone-01-application"),
                role_filter: CacheRoleFilter::SystemLike,
                ttl: CacheTtl::ProviderDefault,
                mode: CacheMode::Automatic,
            },
            CacheBoundary {
                id: CacheBoundaryId::from("cache-unsupported-explicit"),
                after_block_id: PromptBlockId::from("zone-02-preset-b"),
                role_filter: CacheRoleFilter::ExactRole {
                    role: RoleHint::Developer,
                },
                ttl: CacheTtl::Long,
                mode: CacheMode::Explicit,
            },
        ],
        metadata: PresetMetadata {
            description: "Deterministic shared DTO fixture".to_owned(),
            tags: vec!["synthetic".to_owned(), "cross-platform".to_owned()],
            provenance: fixed_provenance(
                SourceKind::ApplicationBuiltIn,
                "cross-platform.synthetic.v1",
            ),
            created_at: fixed_time,
            updated_at: fixed_time,
            local_override_of: None,
        },
    };

    let branch_id = ConversationBranchId("branch-fixed-0001".to_owned());
    let latest_user_message_id = MessageId("message-latest-user-fixed".to_owned());
    let mut variables = VariableMap::default();
    variables.insert(
        VariableRef {
            scope: VariableScope::Session,
            namespace: None,
            id: lorepia_domain::VariableId::from("unused-credential-canary"),
        },
        VariableValue::Text(CREDENTIAL_CANARY.to_owned()),
    );
    PromptResolveRequest {
        preset,
        context: PromptResolutionContext {
            conversation_id: ConversationId("conversation-fixed-0001".to_owned()),
            branch_id: branch_id.clone(),
            character: CharacterPromptContent {
                character_id: "character-fixed-0001".to_owned(),
                name: "Synthetic Character".to_owned(),
                aliases: vec!["Fixture Alias".to_owned()],
                description: "Character context shared by every platform.".to_owned(),
                personality: "Deterministic".to_owned(),
                scenario: "Cross-platform contract test".to_owned(),
                first_message: "Hello.".to_owned(),
                dialogue_examples: Vec::new(),
                system_instruction: String::new(),
                post_history_instruction: "Fixed post-history instruction.".to_owned(),
                alternate_greetings: Vec::new(),
                knowledge_book_ids: Vec::new(),
                asset_ids: Vec::new(),
            },
            persona: None,
            user_name: "Synthetic User".to_owned(),
            messages: vec![
                PromptConversationMessage {
                    id: MessageId("message-turn-1-user".to_owned()),
                    branch_id: branch_id.clone(),
                    role: PromptMessageRole::User,
                    content: "Old user turn.".to_owned(),
                    turn_index: 1,
                },
                PromptConversationMessage {
                    id: MessageId("message-turn-1-assistant".to_owned()),
                    branch_id: branch_id.clone(),
                    role: PromptMessageRole::Assistant,
                    content: "Old assistant turn.".to_owned(),
                    turn_index: 1,
                },
                PromptConversationMessage {
                    id: MessageId("message-turn-2-user".to_owned()),
                    branch_id: branch_id.clone(),
                    role: PromptMessageRole::User,
                    content: "Recent user turn.".to_owned(),
                    turn_index: 2,
                },
                PromptConversationMessage {
                    id: MessageId("message-turn-2-assistant".to_owned()),
                    branch_id: branch_id.clone(),
                    role: PromptMessageRole::Assistant,
                    content: "Recent assistant turn.".to_owned(),
                    turn_index: 2,
                },
                PromptConversationMessage {
                    id: latest_user_message_id.clone(),
                    branch_id: branch_id.clone(),
                    role: PromptMessageRole::User,
                    content: "Fixed latest user turn.".to_owned(),
                    turn_index: 3,
                },
            ],
            latest_user_message_id,
            selected_knowledge: vec![
                SelectedKnowledge {
                    entry_id: KnowledgeEntryId::from("knowledge-tie-b"),
                    content: "Tie B selected fact.".to_owned(),
                    placement: KnowledgePlacement::RetrievedContext,
                    priority: 50,
                    evidence: vec![KnowledgeActivationReason::Always],
                    provenance: fixed_provenance(SourceKind::UserCreated, "knowledge-tie-b"),
                },
                SelectedKnowledge {
                    entry_id: KnowledgeEntryId::from("knowledge-tie-a"),
                    content: "Tie A selected fact.".to_owned(),
                    placement: KnowledgePlacement::RetrievedContext,
                    priority: 50,
                    evidence: vec![KnowledgeActivationReason::Keyword {
                        matched: "selected".to_owned(),
                    }],
                    provenance: fixed_provenance(SourceKind::UserCreated, "knowledge-tie-a"),
                },
                SelectedKnowledge {
                    entry_id: KnowledgeEntryId::from("knowledge-excluded-by-budget"),
                    content: "Lower priority fact excluded by budget.".to_owned(),
                    placement: KnowledgePlacement::RetrievedContext,
                    priority: 10,
                    evidence: vec![KnowledgeActivationReason::Manual],
                    provenance: fixed_provenance(
                        SourceKind::UserCreated,
                        "knowledge-excluded-by-budget",
                    ),
                },
                SelectedKnowledge {
                    entry_id: KnowledgeEntryId::from("knowledge-excluded-by-placement"),
                    content: "This placement has no matching block.".to_owned(),
                    placement: KnowledgePlacement::BeforeRecentHistory,
                    priority: 100,
                    evidence: vec![KnowledgeActivationReason::Always],
                    provenance: fixed_provenance(
                        SourceKind::UserCreated,
                        "knowledge-excluded-by-placement",
                    ),
                },
            ],
            selected_memory: vec![
                SelectedMemory {
                    record_id: MemoryRecordId::from("memory-tie-b"),
                    branch_id: branch_id.clone(),
                    content: "Memory B remains after reduction.".to_owned(),
                    score_millionths: 800_000,
                    reason: "fixed score tie".to_owned(),
                    provenance: fixed_provenance(SourceKind::Generated, "memory-tie-b"),
                },
                SelectedMemory {
                    record_id: MemoryRecordId::from("memory-tie-a"),
                    branch_id,
                    content: "Memory A is excluded by reduction.".to_owned(),
                    score_millionths: 800_000,
                    reason: "fixed score tie".to_owned(),
                    provenance: fixed_provenance(SourceKind::Generated, "memory-tie-a"),
                },
            ],
            summary_boundaries: Vec::new(),
            conversation_summary: Some("Fixed summary.".to_owned()),
            author_note: Some(
                "Optional author-note material that is intentionally reduced.".to_owned(),
            ),
            group_context: None,
            variables,
            slots: Vec::new(),
            current_date: "2026-08-03".to_owned(),
            current_time: "12:34:56".to_owned(),
            supported_capabilities: Vec::new(),
            session_seed: Some(0x5eed_cafe),
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
        generation_preset_id: Some(GenerationPresetId::from("generation.provider-defaults.v1")),
        max_context_tokens: 2_048,
        reserved_output_tokens: 256,
    }
}

fn shared_core_abi_json(
    _platform: NativePlatform,
    plan: &ResolvedPromptPlan,
) -> serde_json::Result<String> {
    serde_json::to_string_pretty(plan).map(|json| format!("{json}\n"))
}

fn maybe_update_golden(actual_json: &str, actual_sha256: &str) {
    if std::env::var_os(UPDATE_GOLDEN_ENV).is_none() {
        return;
    }
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    std::fs::write(
        manifest_dir.join("tests/fixtures/cross_platform_resolved_prompt_plan.json"),
        actual_json,
    )
    .expect("write opted-in cross-platform JSON golden");
    std::fs::write(
        manifest_dir.join("tests/fixtures/cross_platform_resolved_prompt_plan.sha256"),
        format!("{actual_sha256}\n"),
    )
    .expect("write opted-in cross-platform hash golden");
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the golden contract compares every native platform in one reviewed fixture"
)]
fn every_native_platform_consumes_the_same_canonical_plan_json_and_hash() {
    let plan = resolve_prompt_plan(&cross_platform_request()).expect("resolve shared prompt plan");
    verify_resolved_prompt_plan(&plan).expect("verify canonical prompt plan hash");
    let android = shared_core_abi_json(NativePlatform::Android, &plan).expect("serialize Android");
    let ios = shared_core_abi_json(NativePlatform::Ios, &plan).expect("serialize iOS");
    let macos = shared_core_abi_json(NativePlatform::Macos, &plan).expect("serialize macOS");
    let windows = shared_core_abi_json(NativePlatform::Windows, &plan).expect("serialize Windows");

    assert_eq!(android, ios);
    assert_eq!(android, macos);
    assert_eq!(android, windows);
    maybe_update_golden(&android, &plan.plan_hash);
    if std::env::var_os(UPDATE_GOLDEN_ENV).is_some() {
        return;
    }
    assert_eq!(android, GOLDEN_JSON);
    assert_eq!(plan.plan_hash, GOLDEN_SHA256.trim());
    assert_eq!(
        serde_json::from_str::<ResolvedPromptPlan>(GOLDEN_JSON)
            .expect("checked-in JSON is one shared ResolvedPromptPlan"),
        plan
    );

    let zones = [
        PlacementZone::ApplicationPolicy,
        PlacementZone::PresetInstruction,
        PlacementZone::CharacterContext,
        PlacementZone::RetrievedContext,
        PlacementZone::OlderHistory,
        PlacementZone::RecentEnhancement,
        PlacementZone::RecentHistory,
        PlacementZone::PostHistory,
        PlacementZone::LatestUser,
        PlacementZone::AssistantPrefill,
    ];
    for zone in zones {
        assert!(
            cross_platform_request()
                .preset
                .blocks
                .iter()
                .any(|block| block.placement_zone == zone),
            "fixture must exercise {zone:?}"
        );
    }
    assert!(plan.trace.role_mappings.iter().any(|mapping| {
        mapping.requested_role == RoleHint::Developer
            && mapping.effective_role == ProviderMessageRole::System
    }));
    assert!(plan.trace.role_mappings.iter().any(|mapping| {
        mapping.requested_role == RoleHint::ProviderDefault
            && mapping.effective_role == ProviderMessageRole::User
    }));
    assert!(plan.cache_directives.iter().any(|directive| {
        directive.boundary_id.as_str() == "cache-applied-automatic"
            && directive.status == CacheDirectiveStatus::Applied
    }));
    assert!(plan.cache_directives.iter().any(|directive| {
        directive.boundary_id.as_str() == "cache-unsupported-explicit"
            && directive.status == CacheDirectiveStatus::IgnoredUnsupported
    }));
    assert!(plan.trace.blocks.iter().any(|block| {
        block.block_id.as_str() == "zone-04-knowledge"
            && block.status == BlockResolutionStatus::ReducedItems
            && block
                .knowledge_evidence
                .iter()
                .any(|evidence| !evidence.selected)
    }));
    assert!(plan.trace.blocks.iter().any(|block| {
        block.block_id.as_str() == "zone-04-memory"
            && block.status == BlockResolutionStatus::ReducedItems
            && block
                .memory_record_ids
                .iter()
                .map(MemoryRecordId::as_str)
                .eq(["memory-tie-b"])
            && block.memory_evidence.len() == 2
            && block.memory_evidence.iter().any(|evidence| {
                evidence.record_id.as_str() == "memory-tie-a"
                    && !evidence.selected
                    && evidence.exclusion_reason.as_deref()
                        == Some("removed by the prompt token budget")
            })
            && block.memory_evidence.iter().any(|evidence| {
                evidence.record_id.as_str() == "memory-tie-b"
                    && evidence.selected
                    && evidence.exclusion_reason.is_none()
            })
    }));
    assert!(plan.trace.blocks.iter().all(|block| {
        block.source.source_id.as_deref() == Some(block.block_id.as_str())
            && block.source.source_hash.as_deref() == Some("11".repeat(32).as_str())
    }));
    assert!(plan.trace.blocks.iter().any(|block| {
        block.block_id.as_str() == "zone-06-recent-enhancement"
            && block.status == BlockResolutionStatus::TrimmedTail
    }));
    assert_eq!(plan.trace.session_seed, Some(0x5eed_cafe));
    assert_eq!(
        plan.generation_preset_id
            .as_ref()
            .map(GenerationPresetId::as_str),
        Some("generation.provider-defaults.v1")
    );
    assert!(!android.contains("knowledge-excluded-by-placement"));
    assert!(!android.contains(CREDENTIAL_CANARY));
    assert!(!android.contains("/Users/"));
    assert!(!android.contains("\\\\"));
}
