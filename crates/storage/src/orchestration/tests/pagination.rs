use super::{MemoryRecordId, ReadPageCursor, memory_head_fixture, memory_head_record};

#[test]
fn creator_pages_keep_recency_and_the_observed_anchor_after_an_edit() {
    use super::{
        CreatorDocumentKind, KnowledgeBook, KnowledgeBookId, Provenance, SourceKind, TokenBudget,
    };
    let fixture = memory_head_fixture();
    for id in ["a", "c", "b"] {
        let book = KnowledgeBook {
            id: KnowledgeBookId::from(id),
            name: id.to_owned(),
            schema_version: 1,
            entries: Vec::new(),
            scan_depth: 4,
            token_budget: TokenBudget { max_tokens: 1024 },
            recursive: false,
            max_recursion_depth: 0,
            provenance: Provenance {
                source_kind: SourceKind::UserCreated,
                source_id: None,
                source_hash: None,
                author: None,
                license: None,
                imported_at: None,
            },
        };
        fixture.storage.save_knowledge_book(&book, None).unwrap();
    }
    let first = fixture
        .storage
        .list_creator_documents_page::<KnowledgeBook>(CreatorDocumentKind::KnowledgeBook, None, 1)
        .unwrap();
    assert_eq!(first.items[0].value.id.as_str(), "b");
    let cursor = ReadPageCursor {
        scope: first.scope,
        after_id: "b".to_owned(),
        after_updated_at: Some(first.items[0].updated_at),
    };
    let mut changed = first.items[0].value.clone();
    changed.name = "Updated anchor".to_owned();
    fixture
        .storage
        .save_knowledge_book(&changed, Some(first.items[0].revision))
        .unwrap();
    let next = fixture
        .storage
        .list_creator_documents_page::<KnowledgeBook>(
            CreatorDocumentKind::KnowledgeBook,
            Some(&cursor),
            10,
        )
        .unwrap();
    assert_eq!(
        next.items
            .iter()
            .map(|item| item.value.id.as_str())
            .collect::<Vec<_>>(),
        ["c", "a"]
    );
    assert!(!next.has_more);
}

#[test]
fn memory_pages_reach_and_edit_beyond_250_without_invalidating_live_continuation() {
    let fixture = memory_head_fixture();
    let mut record = memory_head_record(&fixture);
    for index in 0..275 {
        record.id = MemoryRecordId::from(format!("memory-{index:04}"));
        fixture
            .storage
            .save_memory_record(&record, None)
            .expect("save memory");
    }
    let first = fixture
        .storage
        .list_memory_records_page(
            &fixture.conversation_id,
            &fixture.branch_id,
            false,
            None,
            100,
        )
        .expect("first memory page");
    assert_eq!(first.items.len(), 100);
    let cursor = ReadPageCursor {
        scope: first.scope,
        after_id: first.items.last().unwrap().value.id.0.clone(),
        after_updated_at: None,
    };
    let mut changed = first.items[0].value.clone();
    changed.title = "Edited while browsing".into();
    fixture
        .storage
        .save_memory_record(&changed, Some(first.items[0].revision))
        .expect("edit unrelated memory");
    record.id = MemoryRecordId::from("memory-0300");
    fixture
        .storage
        .save_memory_record(&record, None)
        .expect("background new memory");
    let mut after = Some(cursor.clone());
    let mut ids = first
        .items
        .into_iter()
        .map(|v| v.value.id.0)
        .collect::<Vec<_>>();
    let mut last = None;
    while let Some(cursor) = after {
        let page = fixture
            .storage
            .list_memory_records_page(
                &fixture.conversation_id,
                &fixture.branch_id,
                false,
                Some(&cursor),
                100,
            )
            .expect("next live memory page");
        let tail = page.items.last().unwrap();
        after = page.has_more.then(|| ReadPageCursor {
            scope: page.scope,
            after_id: tail.value.id.0.clone(),
            after_updated_at: None,
        });
        last = Some(tail.clone());
        ids.extend(page.items.into_iter().map(|v| v.value.id.0));
    }
    assert_eq!(ids.len(), 276);
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    let mut tail = last.unwrap();
    tail.value.title = "Editable after item 250".into();
    let saved = fixture
        .storage
        .save_memory_record(&tail.value, Some(tail.revision))
        .expect("edit last page");
    assert_eq!(saved.revision, tail.revision + 1);
    assert!(
        fixture
            .storage
            .list_memory_records_page(
                &fixture.conversation_id,
                &fixture.branch_id,
                true,
                Some(&cursor),
                100
            )
            .is_err(),
        "visibility scope cannot be changed with a cursor"
    );
}
