use std::collections::BTreeMap;

use chrono::{TimeZone, Utc};
use lorepia_domain::{
    BlockSource, CacheBoundary, CacheBoundaryId, CacheDirectiveStatus, CacheMode, CacheRoleFilter,
    CacheTtl, CharacterPromptContent, ConversationBranchId, ConversationId, GenerationPresetId,
    InstructionAuthority, KnowledgeActivationReason, KnowledgeEntryId, KnowledgePlacement,
    MemoryRecordId, OverflowPolicy, PlacementZone, PresetMetadata, PromptBlockId, PromptBlockKind,
    PromptConversationMessage, PromptMemorySelectionEvidence, PromptMemorySelectionLane,
    PromptMemorySelectionReason, PromptMessageRole, PromptPresetId, PromptResolutionContext,
    PromptResolveRequest, Provenance, ProviderMessageRole, ProviderPromptContract, RoleHint,
    SelectedKnowledge, SelectedMemory, SourceKind, UnsupportedRolePolicy, VariableMap,
};
use lorepia_orchestration::{
    OrchestrationError, default_prompt_preset, render_prompt_preview,
    reseal_prompt_resolution_evidence, resolve_prompt_plan, verify_resolved_prompt_plan,
};

fn provenance(source_kind: SourceKind, source_id: &str) -> Provenance {
    Provenance {
        source_kind,
        source_id: Some(source_id.to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic Author".to_owned()),
        license: Some("MIT".to_owned()),
        imported_at: None,
    }
}

#[allow(clippy::too_many_lines)]
fn request() -> PromptResolveRequest {
    let timestamp = Utc
        .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
        .single()
        .expect("valid synthetic timestamp");
    let metadata = PresetMetadata {
        description: "Synthetic acceptance preset".to_owned(),
        tags: vec!["synthetic".to_owned()],
        provenance: provenance(SourceKind::UserCreated, "synthetic.preset"),
        created_at: timestamp,
        updated_at: timestamp,
        local_override_of: None,
    };
    let mut preset = default_prompt_preset(
        PromptPresetId::from("synthetic.preset"),
        "Synthetic preset",
        metadata,
    );
    let character_block_id = preset.blocks[0].id.clone();

    let mut knowledge_block = preset.blocks[0].clone();
    knowledge_block.id = PromptBlockId::from("synthetic.knowledge");
    "Selected knowledge".clone_into(&mut knowledge_block.name);
    knowledge_block.kind = PromptBlockKind::WorldKnowledge;
    knowledge_block.role_hint = RoleHint::Developer;
    knowledge_block.authority = InstructionAuthority::Creator;
    knowledge_block.source = BlockSource::SelectedKnowledge;
    knowledge_block.placement_zone = PlacementZone::RetrievedContext;
    knowledge_block.overflow_policy = OverflowPolicy::ReduceKnowledgeEntries;
    knowledge_block.provenance = provenance(SourceKind::UserCreated, "synthetic.knowledge-block");

    let mut memory_block = preset.blocks[0].clone();
    memory_block.id = PromptBlockId::from("synthetic.memory");
    "Selected memory".clone_into(&mut memory_block.name);
    memory_block.kind = PromptBlockKind::RetrievedMemory;
    memory_block.role_hint = RoleHint::System;
    memory_block.authority = InstructionAuthority::Conversation;
    memory_block.source = BlockSource::SelectedMemory;
    memory_block.placement_zone = PlacementZone::RetrievedContext;
    memory_block.overflow_policy = OverflowPolicy::DropBlock;
    memory_block.provenance = provenance(SourceKind::Generated, "synthetic.memory-block");

    preset.blocks.insert(1, knowledge_block);
    preset.blocks.insert(2, memory_block);
    preset.cache_boundaries.push(CacheBoundary {
        id: CacheBoundaryId::from("synthetic.cache"),
        after_block_id: character_block_id,
        role_filter: CacheRoleFilter::SystemLike,
        ttl: CacheTtl::Short,
        mode: CacheMode::Explicit,
    });

    let branch_id = ConversationBranchId("synthetic.branch".to_owned());
    let latest_id = lorepia_domain::MessageId("synthetic.latest".to_owned());
    PromptResolveRequest {
        preset,
        context: PromptResolutionContext {
            conversation_id: ConversationId("synthetic.conversation".to_owned()),
            branch_id: branch_id.clone(),
            character: CharacterPromptContent {
                character_id: "synthetic.character".to_owned(),
                name: "Ari".to_owned(),
                aliases: Vec::new(),
                description: "A fully synthetic character used for tests.".to_owned(),
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
            messages: vec![
                PromptConversationMessage {
                    id: lorepia_domain::MessageId("synthetic.old-user".to_owned()),
                    branch_id: branch_id.clone(),
                    role: PromptMessageRole::User,
                    content: "Earlier synthetic question.".to_owned(),
                    turn_index: 1,
                },
                PromptConversationMessage {
                    id: lorepia_domain::MessageId("synthetic.old-assistant".to_owned()),
                    branch_id: branch_id.clone(),
                    role: PromptMessageRole::Assistant,
                    content: "Earlier synthetic answer.".to_owned(),
                    turn_index: 1,
                },
                PromptConversationMessage {
                    id: latest_id.clone(),
                    branch_id: branch_id.clone(),
                    role: PromptMessageRole::User,
                    content: "Continue from this latest turn.".to_owned(),
                    turn_index: 2,
                },
            ],
            latest_user_message_id: latest_id,
            selected_knowledge: vec![
                SelectedKnowledge {
                    entry_id: KnowledgeEntryId::from("synthetic.knowledge-b"),
                    content: "Second selected fact.".to_owned(),
                    placement: KnowledgePlacement::RetrievedContext,
                    priority: 10,
                    evidence: vec![KnowledgeActivationReason::Always],
                    provenance: provenance(SourceKind::UserCreated, "synthetic.knowledge-b"),
                },
                SelectedKnowledge {
                    entry_id: KnowledgeEntryId::from("synthetic.knowledge-a"),
                    content: "First selected fact.".to_owned(),
                    placement: KnowledgePlacement::RetrievedContext,
                    priority: 20,
                    evidence: vec![KnowledgeActivationReason::Keyword {
                        matched: "fact".to_owned(),
                    }],
                    provenance: provenance(SourceKind::UserCreated, "synthetic.knowledge-a"),
                },
            ],
            selected_memory: vec![
                SelectedMemory {
                    record_id: MemoryRecordId::from("synthetic.memory-b"),
                    branch_id: branch_id.clone(),
                    content: "Lower-ranked memory.".to_owned(),
                    score_millionths: 500_000,
                    reason: "synthetic recency".to_owned(),
                    provenance: provenance(SourceKind::Generated, "synthetic.memory-b"),
                },
                SelectedMemory {
                    record_id: MemoryRecordId::from("synthetic.memory-a"),
                    branch_id,
                    content: "Higher-ranked memory.".to_owned(),
                    score_millionths: 900_000,
                    reason: "synthetic similarity".to_owned(),
                    provenance: provenance(SourceKind::Generated, "synthetic.memory-a"),
                },
            ],
            summary_boundaries: Vec::new(),
            conversation_summary: None,
            author_note: None,
            group_context: None,
            variables: VariableMap::default(),
            slots: Vec::new(),
            current_date: "2026-08-03".to_owned(),
            current_time: "12:00".to_owned(),
            supported_capabilities: Vec::new(),
            session_seed: Some(42),
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
        generation_preset_id: Some(GenerationPresetId::from("synthetic.generation")),
        max_context_tokens: 1_024,
        reserved_output_tokens: 128,
    }
}

#[test]
fn canonical_plan_is_deterministic_explainable_and_preview_identical() {
    let first_request = request();
    let mut permuted_request = first_request.clone();
    permuted_request.context.messages.reverse();
    permuted_request.context.selected_knowledge.reverse();
    permuted_request.context.selected_memory.reverse();

    let first = resolve_prompt_plan(&first_request).expect("first plan");
    let second = resolve_prompt_plan(&permuted_request).expect("permuted plan");

    assert_eq!(
        first, second,
        "unordered context collections must canonicalize before hashing"
    );
    assert_eq!(first.plan_hash.len(), 64);
    assert!(first.plan_hash.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        first.plan_hash,
        "d25ff21949d80cbb89907137bf8b50a1291d28543f4728f5d80e8c9e8fbc8857"
    );
    assert_eq!(first.preview.effective_messages, first.effective_messages);
    assert_eq!(first.preview.cache_directives, first.cache_directives);
    assert_eq!(
        render_prompt_preview(&first_request).expect("preview"),
        first.preview,
        "preview must use the exact send-plan resolver"
    );
    assert_eq!(
        first.trace.estimated_input_tokens,
        first
            .effective_messages
            .iter()
            .map(|message| message.estimated_tokens)
            .sum::<u32>()
    );
    assert!(first.effective_messages.iter().all(|message| {
        message.estimated_tokens > 0
            && message.provenance.source_id.is_some()
            && !message.content.is_empty()
    }));
    for trace in &first.trace.blocks {
        assert!(trace.source.source_id.is_some());
        let expected_tokens = first
            .effective_messages
            .iter()
            .filter(|message| message.block_id == trace.block_id)
            .map(|message| message.estimated_tokens)
            .sum::<u32>();
        assert_eq!(
            trace.final_estimated_tokens,
            expected_tokens,
            "block {} token evidence",
            trace.block_id.as_str()
        );
    }

    let knowledge_trace = first
        .trace
        .blocks
        .iter()
        .find(|trace| trace.block_id.as_str() == "synthetic.knowledge")
        .expect("knowledge trace");
    assert_eq!(knowledge_trace.knowledge_evidence.len(), 2);
    assert!(
        knowledge_trace
            .knowledge_evidence
            .iter()
            .all(|item| { item.selected && item.estimated_tokens > 0 && !item.reasons.is_empty() })
    );

    let memory_trace = first
        .trace
        .blocks
        .iter()
        .find(|trace| trace.block_id.as_str() == "synthetic.memory")
        .expect("memory trace");
    assert_eq!(
        memory_trace
            .memory_record_ids
            .iter()
            .map(MemoryRecordId::as_str)
            .collect::<Vec<_>>(),
        vec!["synthetic.memory-a", "synthetic.memory-b"]
    );
    assert_eq!(memory_trace.memory_evidence.len(), 2);
    assert!(memory_trace.memory_evidence.iter().all(|evidence| {
        evidence.selected
            && evidence.lane.is_some()
            && evidence.rank_millionths.is_some()
            && evidence.estimated_tokens > 0
    }));

    assert!(first.trace.role_mappings.iter().any(|mapping| {
        mapping.block_id.as_str() == "synthetic.knowledge"
            && mapping.requested_role == RoleHint::Developer
            && mapping.effective_role == ProviderMessageRole::System
            && mapping.explanation.contains("mapped")
    }));
    assert_eq!(
        first.cache_directives[0].status,
        CacheDirectiveStatus::IgnoredUnsupported
    );
    assert!(first.trace.warnings.iter().any(|warning| {
        warning.contains("synthetic.cache") && warning.contains("lacks explicit caching")
    }));
}

#[test]
fn source_revisions_and_complete_memory_evidence_are_hash_bound() {
    let original = resolve_prompt_plan(&request()).expect("original plan");
    let source_revisions = BTreeMap::from([(
        PromptBlockId::from("synthetic.memory"),
        "synthetic-preset-revision-7".to_owned(),
    )]);
    let evidence = vec![
        PromptMemorySelectionEvidence {
            record_id: MemoryRecordId::from("synthetic.memory-a"),
            selected: true,
            lane: Some(PromptMemorySelectionLane::Semantic),
            rank_millionths: Some(900_000),
            estimated_tokens: 6,
            reasons: vec![
                PromptMemorySelectionReason::CurrentBranch,
                PromptMemorySelectionReason::Similarity {
                    score_millionths: 800_000,
                },
            ],
            exclusion_reason: None,
        },
        PromptMemorySelectionEvidence {
            record_id: MemoryRecordId::from("synthetic.memory-b"),
            selected: true,
            lane: Some(PromptMemorySelectionLane::Episodic),
            rank_millionths: Some(500_000),
            estimated_tokens: 5,
            reasons: vec![
                PromptMemorySelectionReason::CurrentBranch,
                PromptMemorySelectionReason::Recency {
                    score_millionths: 500_000,
                },
                PromptMemorySelectionReason::Importance {
                    score_millionths: 400_000,
                },
            ],
            exclusion_reason: None,
        },
        PromptMemorySelectionEvidence {
            record_id: MemoryRecordId::from("synthetic.memory-excluded"),
            selected: false,
            lane: None,
            rank_millionths: Some(100_000),
            estimated_tokens: 99,
            reasons: vec![PromptMemorySelectionReason::SharedAncestor {
                source_branch_id: ConversationBranchId("synthetic.ancestor".to_owned()),
            }],
            exclusion_reason: Some("memory retrieval count limit reached".to_owned()),
        },
    ];
    let sealed =
        reseal_prompt_resolution_evidence(&original, &source_revisions, &evidence).expect("seal");
    assert_ne!(sealed.plan_hash, original.plan_hash);
    let memory_trace = sealed
        .trace
        .blocks
        .iter()
        .find(|trace| trace.block_id.as_str() == "synthetic.memory")
        .expect("memory trace");
    assert_eq!(
        memory_trace.source.source_revision.as_deref(),
        Some("synthetic-preset-revision-7")
    );
    assert_eq!(
        memory_trace
            .memory_evidence
            .iter()
            .map(|item| (
                &item.record_id,
                item.selected,
                item.lane,
                item.estimated_tokens
            ))
            .collect::<Vec<_>>(),
        evidence
            .iter()
            .map(|item| (
                &item.record_id,
                item.selected,
                item.lane,
                item.estimated_tokens
            ))
            .collect::<Vec<_>>()
    );
    verify_resolved_prompt_plan(&sealed).expect("sealed plan verifies");

    let mut tampered = sealed;
    tampered
        .trace
        .blocks
        .iter_mut()
        .find(|trace| trace.block_id.as_str() == "synthetic.memory")
        .expect("memory trace")
        .memory_evidence[0]
        .rank_millionths = Some(1);
    assert!(matches!(
        verify_resolved_prompt_plan(&tampered),
        Err(OrchestrationError::PlanHashMismatch)
    ));
}

#[test]
fn latest_user_is_never_silently_removed_and_unsupported_roles_can_reject() {
    let mut bounded = request();
    bounded.max_context_tokens = 96;
    bounded.reserved_output_tokens = 16;
    bounded.context.messages[2].content = "latest survives".to_owned();
    let plan = resolve_prompt_plan(&bounded).expect("bounded prompt");
    assert!(plan.effective_messages.iter().any(|message| {
        message.block_kind == PromptBlockKind::LatestUserTurn
            && message.content == "latest survives"
            && message.effective_role == ProviderMessageRole::User
    }));

    let mut impossible = request();
    impossible.max_context_tokens = 32;
    impossible.reserved_output_tokens = 16;
    impossible.context.messages[2].content = "x".repeat(512);
    assert!(matches!(
        resolve_prompt_plan(&impossible),
        Err(OrchestrationError::LatestUserMessageExceedsBudget { .. })
    ));

    let mut strict_role = request();
    strict_role.provider.unsupported_role_policy = UnsupportedRolePolicy::Reject;
    assert!(matches!(
        resolve_prompt_plan(&strict_role),
        Err(OrchestrationError::UnsupportedRole {
            role: RoleHint::Developer,
            ..
        })
    ));
}
