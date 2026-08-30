
fn memory_head_fixture() -> MemoryHeadFixture {
    let root = tempfile::tempdir().expect("temporary storage root");
    let storage = Storage::open(root.path()).expect("open storage");
    let conversation_id = ConversationId("memory-head-conversation".to_owned());
    let branch_id = ConversationBranchId("memory-head-branch".to_owned());
    let head_id = MessageId("memory-head-message".to_owned());
    let source_sha256 =
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_owned();
    let now = Utc::now();
    {
        let connection = storage.connection().expect("storage connection");
        connection
            .execute(
                "INSERT INTO content_sources
                     (sha256, relative_path, size_bytes, created_at)
                     VALUES (?1, 'sources/memory-head', 1, ?2)",
                params![source_sha256.as_str(), now.to_rfc3339()],
            )
            .expect("insert source");
        connection
            .execute(
                "INSERT INTO characters
                     (id, name, description, source_hash, avatar_asset_hash, created_at)
                     VALUES ('memory-head-character', 'Memory', '', ?1, NULL, ?2)",
                params![source_sha256.as_str(), now.to_rfc3339()],
            )
            .expect("insert character");
        connection
            .execute(
                "INSERT INTO conversations
                     (id, character_id, title, created_at, updated_at)
                     VALUES (?1, 'memory-head-character', 'Memory', ?2, ?2)",
                params![conversation_id.0, now.to_rfc3339()],
            )
            .expect("insert conversation");
        connection
            .execute(
                "INSERT INTO messages
                     (id, conversation_id, parent_id, role, content, status,
                      generation_id, created_at)
                     VALUES (?1, ?2, NULL, 'user', 'first', 'complete', NULL, ?3)",
                params![head_id.0, conversation_id.0, now.to_rfc3339()],
            )
            .expect("insert first message");
        connection
            .execute(
                "INSERT INTO conversation_branches
                     (id, conversation_id, title, fork_message_id, head_message_id,
                      created_at, updated_at)
                     VALUES (?1, ?2, NULL, NULL, ?3, ?4, ?4)",
                params![branch_id.0, conversation_id.0, head_id.0, now.to_rfc3339()],
            )
            .expect("insert branch");
        connection
            .execute(
                "INSERT INTO conversation_state
                     (conversation_id, active_branch_id, selected_mode, updated_at)
                     VALUES (?1, ?2, 'chat', ?3)",
                params![conversation_id.0, branch_id.0, now.to_rfc3339()],
            )
            .expect("insert conversation state");
    }
    MemoryHeadFixture {
        _root: root,
        storage,
        conversation_id,
        branch_id,
        head_id,
        source_sha256,
        now,
    }
}

fn memory_head_record(fixture: &MemoryHeadFixture) -> MemoryRecord {
    MemoryRecord {
        id: MemoryRecordId::from("memory-head-record"),
        conversation_id: fixture.conversation_id.clone(),
        branch_id: fixture.branch_id.clone(),
        source_start_message_id: fixture.head_id.clone(),
        source_end_message_id: fixture.head_id.clone(),
        kind: lorepia_domain::MemoryKind::ConversationSummary,
        title: "Summary".to_owned(),
        summary: "Exact memory snapshot evidence.".to_owned(),
        structured_data: VersionedJson {
            schema_version: 1,
            value: serde_json::json!({"facts": []}),
        },
        importance: 50,
        keywords: vec!["snapshot".to_owned()],
        embedding_ref: None,
        pinned: false,
        excluded_from_conversation: false,
        excluded_from_character: false,
        created_at: fixture.now,
        updated_at: fixture.now,
        invalidated_at: None,
        provenance: Provenance {
            source_kind: SourceKind::Generated,
            source_id: Some("memory-head-fixture".to_owned()),
            source_hash: Some(fixture.source_sha256.clone()),
            author: None,
            license: None,
            imported_at: None,
        },
    }
}

fn assert_historical_root_snapshot(fixture: &MemoryHeadFixture) {
    let historical_root = fixture
        .storage
        .list_memory_records_at_head(&fixture.conversation_id, &fixture.branch_id, None, false)
        .expect("select the pre-first-message boundary");
    assert!(historical_root.records.is_empty());
    assert!(historical_root.snapshot.records.is_empty());
    assert_eq!(historical_root.snapshot.context_head_message_id, None);
    assert_eq!(
        memory_records_at_head_snapshot_sha256(&historical_root.snapshot)
            .expect("verify historical-root snapshot"),
        historical_root.snapshot.snapshot_sha256
    );
}

fn assert_exact_memory_revision_snapshot(
    fixture: &MemoryHeadFixture,
    stored: &StoredRevision<MemoryRecord>,
) {
    let at_head = fixture
        .storage
        .list_memory_records_at_head(
            &fixture.conversation_id,
            &fixture.branch_id,
            Some(&fixture.head_id),
            false,
        )
        .expect("select memory records at the first message");
    assert_eq!(at_head.records.len(), 1);
    assert_eq!(&at_head.records[0].value, &stored.value);
    let evidence = at_head
        .snapshot
        .records
        .first()
        .expect("memory snapshot evidence");
    let revision_id = stored.revision_id.as_deref().expect("memory revision id");
    let exact_revision_sha256 = fixture
        .storage
        .connection()
        .expect("storage connection")
        .query_row(
            "SELECT content_sha256 FROM memory_record_revisions WHERE id = ?1",
            [revision_id],
            |row| row.get::<_, String>(0),
        )
        .expect("exact memory revision SHA");
    assert_eq!(evidence.active_revision_sha256, exact_revision_sha256);
    assert_ne!(evidence.active_revision_sha256, fixture.head_id.0);
}

#[test]
fn memory_head_snapshot_accepts_the_historical_root_and_seals_the_exact_revision_sha() {
    let fixture = memory_head_fixture();
    let stored = fixture
        .storage
        .save_memory_record(&memory_head_record(&fixture), None)
        .expect("save memory record");
    assert_historical_root_snapshot(&fixture);
    assert_exact_memory_revision_snapshot(&fixture, &stored);
}
