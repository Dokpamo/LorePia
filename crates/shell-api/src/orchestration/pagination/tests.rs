use super::{
    CreatorDocumentKind, ListCreatorDocumentsPageInput, ReadPageCursor, ShellApi, validate_document,
};
use crate::{DeleteKnowledgeBookInput, UpsertKnowledgeBookInput};
use lorepia_core::{
    ActivationRule, Core, CoreConfig, KnowledgeBook, KnowledgeBookId, KnowledgeEntry,
    KnowledgeEntryId, KnowledgePlacement, Provenance, SourceKind, TokenBudget, TokenPolicy,
};

fn book(id: &str, imported: bool, content_bytes: usize) -> KnowledgeBook {
    let provenance = Provenance {
        source_kind: if imported {
            SourceKind::ImportedStandard
        } else {
            SourceKind::UserCreated
        },
        source_id: None,
        source_hash: None,
        author: None,
        license: None,
        imported_at: None,
    };
    KnowledgeBook {
        id: KnowledgeBookId::from(id),
        name: id.into(),
        schema_version: 1,
        entries: vec![KnowledgeEntry {
            id: KnowledgeEntryId::from(format!("{id}.entry")),
            book_id: KnowledgeBookId::from(id),
            name: "Entry".into(),
            content: "x".repeat(content_bytes),
            enabled: true,
            activation: ActivationRule::Always,
            priority: 0,
            importance: 50,
            placement: KnowledgePlacement::BeforeRecentHistory,
            token_policy: TokenPolicy {
                priority: 0,
                min_tokens: None,
                max_tokens: None,
                reserve_tokens: None,
            },
            parent_id: None,
            activation_probability_basis_points: 10000,
            provenance: provenance.clone(),
        }],
        scan_depth: 4,
        token_budget: TokenBudget { max_tokens: 1024 },
        recursive: false,
        max_recursion_depth: 0,
        provenance,
    }
}
#[test]
fn creator_pages_ignore_large_imported_catalog_and_reach_all_editable_documents() {
    let root = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig::new(root.path())).unwrap();
    for index in 0..105 {
        core.upsert_knowledge_book(&book(&format!("user-{index:03}"), false, 32_000), None)
            .unwrap();
    }
    for index in 0..40 {
        core.upsert_knowledge_book(&book(&format!("imported-{index:03}"), true, 60_000), None)
            .unwrap();
    }
    let shell = ShellApi::from_core(core);
    let first = shell
        .list_creator_documents_page(ListCreatorDocumentsPageInput {
            kind: CreatorDocumentKind::KnowledgeBook,
            after: None,
            limit: 100,
        })
        .unwrap();
    assert!(
        first.documents.len() < 100,
        "byte budget produces a shorter, usable page"
    );
    let cursor = first.next_cursor.clone().unwrap();
    assert!(
        shell.list_knowledge_books().is_ok(),
        "legacy first page must also filter before aggregation"
    );
    let mut ids = Vec::new();
    let mut current = first;
    loop {
        validate_document(&current).unwrap();
        ids.extend(
            current
                .documents
                .iter()
                .map(|document| document.id().to_owned()),
        );
        let Some(after) = current.next_cursor else {
            break;
        };
        current = shell
            .list_creator_documents_page(ListCreatorDocumentsPageInput {
                kind: CreatorDocumentKind::KnowledgeBook,
                after: Some(after),
                limit: 100,
            })
            .unwrap();
    }
    assert_eq!(ids.len(), 105);
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    let mut edited = book("user-104", false, 32);
    edited.name = "Edited beyond the old cap".into();
    shell
        .upsert_knowledge_book(UpsertKnowledgeBookInput {
            value: edited.try_into().unwrap(),
            expected_revision: Some(1),
        })
        .unwrap();
    shell
        .delete_knowledge_book(DeleteKnowledgeBookInput {
            knowledge_book_id: cursor.after_id.clone(),
            expected_revision: 1,
        })
        .unwrap();
    assert!(
        shell
            .list_creator_documents_page(ListCreatorDocumentsPageInput {
                kind: CreatorDocumentKind::KnowledgeBook,
                after: Some(cursor.clone()),
                limit: 100
            })
            .is_ok(),
        "edits and deleted anchors do not invalidate a live keyset cursor"
    );
    assert!(
        shell
            .list_creator_documents_page(ListCreatorDocumentsPageInput {
                kind: CreatorDocumentKind::TransformSet,
                after: Some(cursor),
                limit: 100
            })
            .is_err()
    );
    assert!(
        shell
            .list_creator_documents_page(ListCreatorDocumentsPageInput {
                kind: CreatorDocumentKind::KnowledgeBook,
                after: Some(ReadPageCursor {
                    scope: "bad".into(),
                    after_id: "user-1".into()
                }),
                limit: 100
            })
            .is_err()
    );
}
