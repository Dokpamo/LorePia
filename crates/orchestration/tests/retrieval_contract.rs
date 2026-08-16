use std::collections::{BTreeMap, BTreeSet};

use chrono::{TimeZone, Utc};
use lorepia_domain::{
    ActivationRule, ConversationBranchId, ConversationId, KnowledgeActivationReason, KnowledgeBook,
    KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId, KnowledgePlacement, MemoryKind,
    MemoryProfile, MemoryProfileId, MemoryRecord, MemoryRecordId, MessageId, Provenance,
    SourceKind, SummarySchemaId, TaskProfileId, TokenBudget, TokenPolicy, VariableMap,
    VersionedJson,
};
use lorepia_orchestration::{
    KnowledgeEngine, KnowledgeSelectionContext, MemoryEngine, MemorySelectionContext,
    MemorySelectionReason,
};

fn provenance(source_kind: SourceKind) -> Provenance {
    Provenance {
        source_kind,
        source_id: Some("synthetic.acceptance".to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("Synthetic Author".to_owned()),
        license: Some("MIT".to_owned()),
        imported_at: None,
    }
}

fn knowledge_entry(
    book_id: &KnowledgeBookId,
    id: &str,
    activation: ActivationRule,
    priority: i32,
) -> KnowledgeEntry {
    KnowledgeEntry {
        id: KnowledgeEntryId::from(id),
        book_id: book_id.clone(),
        name: format!("Synthetic {id}"),
        content: format!("Knowledge content for {id}"),
        enabled: true,
        activation,
        priority,
        importance: 100,
        placement: KnowledgePlacement::RetrievedContext,
        token_policy: TokenPolicy {
            priority: 100,
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        parent_id: None,
        activation_probability_basis_points: 10_000,
        provenance: provenance(SourceKind::UserCreated),
    }
}

fn knowledge_book(entries: Vec<KnowledgeEntry>) -> KnowledgeBook {
    KnowledgeBook {
        id: KnowledgeBookId::from("synthetic.book"),
        name: "Synthetic knowledge".to_owned(),
        schema_version: 1,
        entries,
        scan_depth: 8,
        token_budget: TokenBudget { max_tokens: 16 },
        recursive: false,
        max_recursion_depth: 0,
        provenance: provenance(SourceKind::UserCreated),
    }
}

#[test]
fn knowledge_selection_is_deterministic_and_explains_activation() {
    let book_id = KnowledgeBookId::from("synthetic.book");
    let always = knowledge_entry(&book_id, "always", ActivationRule::Always, 1);
    let keyword = knowledge_entry(
        &book_id,
        "keyword",
        ActivationRule::Keyword {
            primary: vec!["moon".to_owned()],
            secondary: Vec::new(),
            selective: false,
            case_sensitive: false,
            whole_word: true,
        },
        20,
    );
    let absent = knowledge_entry(
        &book_id,
        "absent",
        ActivationRule::Keyword {
            primary: vec!["sun".to_owned()],
            secondary: Vec::new(),
            selective: false,
            case_sensitive: false,
            whole_word: true,
        },
        100,
    );
    let estimates = BTreeMap::from([
        (KnowledgeEntryId::from("always"), 2),
        (KnowledgeEntryId::from("keyword"), 3),
        (KnowledgeEntryId::from("absent"), 1),
    ]);
    let scan = vec!["The synthetic MOON is visible.".to_owned()];
    let select = |book: &KnowledgeBook| {
        KnowledgeEngine::select(
            book,
            &KnowledgeSelectionContext {
                scan_texts: &scan,
                manual_entry_ids: &BTreeSet::new(),
                semantic_scores: &[],
                variables: &VariableMap::default(),
                supported_capabilities: &[],
                token_estimates: &estimates,
                activation_seed: 42,
            },
        )
        .expect("knowledge selection")
    };

    let first = select(&knowledge_book(vec![
        absent.clone(),
        keyword.clone(),
        always.clone(),
    ]));
    let second = select(&knowledge_book(vec![always, absent, keyword]));

    assert_eq!(
        first, second,
        "set-like input ordering must not affect output"
    );
    assert_eq!(
        first
            .selected
            .iter()
            .map(|entry| entry.entry_id.as_str())
            .collect::<Vec<_>>(),
        vec!["always", "keyword"]
    );
    let keyword_evidence = first
        .evidence
        .iter()
        .find(|item| item.entry_id.as_str() == "keyword")
        .expect("keyword evidence");
    assert!(keyword_evidence.selected);
    assert_eq!(keyword_evidence.estimated_tokens, 3);
    assert!(keyword_evidence.reasons.iter().any(|reason| {
        matches!(
            reason,
            KnowledgeActivationReason::Keyword { matched } if matched.eq_ignore_ascii_case("moon")
        )
    }));
    let absent_evidence = first
        .evidence
        .iter()
        .find(|item| item.entry_id.as_str() == "absent")
        .expect("excluded evidence");
    assert!(!absent_evidence.selected);
    assert!(
        absent_evidence
            .exclusion_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("activation"))
    );
}

fn memory_record(id: &str, branch: &str, start: &str, end: &str, importance: u8) -> MemoryRecord {
    let timestamp = Utc
        .with_ymd_and_hms(2026, 8, 3, 12, 0, 0)
        .single()
        .expect("valid synthetic timestamp");
    MemoryRecord {
        id: MemoryRecordId::from(id),
        conversation_id: ConversationId("synthetic.conversation".to_owned()),
        branch_id: ConversationBranchId(branch.to_owned()),
        source_start_message_id: MessageId(start.to_owned()),
        source_end_message_id: MessageId(end.to_owned()),
        kind: MemoryKind::EpisodicEvent,
        title: format!("Synthetic {id}"),
        summary: format!("Memory content for {id}"),
        structured_data: VersionedJson {
            schema_version: 1,
            value: serde_json::json!({ "synthetic": true }),
        },
        importance,
        keywords: Vec::new(),
        embedding_ref: None,
        pinned: false,
        excluded_from_conversation: false,
        excluded_from_character: false,
        created_at: timestamp,
        updated_at: timestamp,
        invalidated_at: None,
        provenance: provenance(SourceKind::Generated),
    }
}

fn memory_profile() -> MemoryProfile {
    MemoryProfile {
        id: MemoryProfileId::from("synthetic.memory-profile"),
        name: "Synthetic memory profile".to_owned(),
        schema_version: 1,
        summary_task: TaskProfileId::from("synthetic.summary-task"),
        embedding_task: None,
        turns_per_summary: 8,
        recent_raw_budget: TokenBudget { max_tokens: 64 },
        episodic_budget: TokenBudget { max_tokens: 64 },
        semantic_budget: TokenBudget { max_tokens: 64 },
        retrieval_count: 8,
        recency_weight: 1.0,
        similarity_weight: 1.0,
        importance_weight: 1.0,
        preserve_invalidated_records: true,
        summary_schema: SummarySchemaId::from("synthetic.summary-schema"),
        provenance: provenance(SourceKind::UserCreated),
    }
}

#[test]
fn memory_selection_keeps_common_ancestors_and_excludes_sibling_branch_state() {
    let records = vec![
        memory_record("root-memory", "root", "m1", "m2", 100),
        memory_record("current-memory", "current", "m2", "m3", 100),
        memory_record("sibling-memory", "sibling", "m2", "sibling-m3", 255),
    ];
    let conversation_id = ConversationId("synthetic.conversation".to_owned());
    let branch_id = ConversationBranchId("current".to_owned());
    let visible = vec![
        MessageId("m1".to_owned()),
        MessageId("m2".to_owned()),
        MessageId("m3".to_owned()),
    ];
    let estimates = BTreeMap::from([
        (MemoryRecordId::from("root-memory"), 3),
        (MemoryRecordId::from("current-memory"), 3),
        (MemoryRecordId::from("sibling-memory"), 1),
    ]);

    let selection = MemoryEngine::select(
        &records,
        &memory_profile(),
        &MemorySelectionContext {
            conversation_id: &conversation_id,
            branch_id: &branch_id,
            visible_message_ids: &visible,
            semantic_scores: &[],
            token_estimates: &estimates,
        },
    )
    .expect("branch-aware memory selection");

    let selected_ids = selection
        .selected
        .iter()
        .map(|record| record.record_id.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        selected_ids,
        BTreeSet::from(["current-memory", "root-memory"])
    );

    let root = selection
        .selected
        .iter()
        .find(|record| record.record_id.as_str() == "root-memory")
        .expect("shared ancestor memory");
    assert!(root.reasons.iter().any(|reason| {
        matches!(
            reason,
            MemorySelectionReason::SharedAncestor { source_branch_id }
                if source_branch_id.0 == "root"
        )
    }));

    let sibling = selection
        .evidence
        .iter()
        .find(|item| item.record_id.as_str() == "sibling-memory")
        .expect("sibling exclusion evidence");
    assert!(!sibling.selected);
    assert!(
        sibling
            .exclusion_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("active branch"))
    );
}
