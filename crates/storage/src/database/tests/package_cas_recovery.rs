fn staged_package_source(
    storage: &Storage,
    name: &str,
    bytes: &[u8],
    import_id: &str,
) -> (PathBuf, PackageCasPromotionIntent, PathBuf) {
    let staged = storage.staging_dir().join(name);
    fs::write(&staged, bytes).expect("write staged package source");
    let sha256 = hex::encode(Sha256::digest(bytes));
    let relative = content_relative_path(&sha256).expect("CAS relative path");
    let intent = PackageCasPromotionIntent {
        import_id: import_id.to_owned(),
        namespace: "source",
        sha256,
        size_bytes: u64::try_from(bytes.len()).expect("small package source"),
        media_type: None,
        relative_path: format!("sources/{relative}"),
    };
    let final_path = storage.data_root().join(&intent.relative_path);
    (staged, intent, final_path)
}

fn assert_abandoned_package_source_recovered(
    root: &Path,
    intent: &PackageCasPromotionIntent,
    final_path: &Path,
) {
    let reopened = Storage::open(root).expect("reopen storage for CAS recovery");
    assert!(!final_path.exists(), "abandoned CAS file must be removed");
    let connection = reopened.connection().expect("reopened database");
    let journal_count = connection
        .query_row(
            "SELECT COUNT(*) FROM package_cas_promotion_journal
                 WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
            params![intent.import_id, intent.namespace, intent.sha256],
            |row| row.get::<_, u64>(0),
        )
        .expect("journal count");
    assert_eq!(journal_count, 0);
    let source_count = connection
        .query_row(
            "SELECT COUNT(*) FROM content_sources WHERE sha256 = ?1",
            [intent.sha256.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .expect("source row count");
    assert_eq!(source_count, 0);
}

fn rollback_only_cas_promotion(
    namespace: &'static str,
    import_id: &str,
) -> (
    tempfile::TempDir,
    Storage,
    PackageCasPromotionIntent,
    PathBuf,
) {
    let root = tempdir().expect("temporary rollback-pinned CAS root");
    let database_path = root.path().join("db/lorepia.sqlite3");
    fs::create_dir_all(database_path.parent().expect("database parent"))
        .expect("create frozen database directory");
    let connection = Connection::open(&database_path).expect("create frozen database");
    connection
        .execute_batch(FROZEN_SCHEMA_ELEVEN_SQL)
        .expect("restore frozen schema-eleven fixture");

    for (directory, sha256, bytes) in [
        ("sources", FROZEN_SOURCE_SHA256, FROZEN_SOURCE_PACKAGE),
        ("assets", FROZEN_AVATAR_SHA256, FROZEN_AVATAR_ASSET),
    ] {
        let path = root
            .path()
            .join(directory)
            .join(content_relative_path(sha256).expect("frozen CAS path"));
        fs::create_dir_all(path.parent().expect("frozen CAS parent"))
            .expect("create frozen CAS directory");
        fs::write(path, bytes).expect("write frozen CAS object");
    }

    let bytes: &[u8] = match namespace {
        "source" => b"rollback-only package source",
        "asset" => b"\x89PNG\r\n\x1a\nrollback-only-asset",
        _ => panic!("unsupported rollback-only CAS namespace"),
    };
    let sha256 = hex::encode(Sha256::digest(bytes));
    let relative = content_relative_path(&sha256).expect("rollback-only CAS path");
    let (directory, media_type) = match namespace {
        "source" => ("sources", None),
        "asset" => ("assets", Some("image/png".to_owned())),
        _ => unreachable!("validated rollback-only CAS namespace"),
    };
    let relative_path = format!("{directory}/{relative}");
    let size_bytes = u64::try_from(bytes.len()).expect("small rollback-only object");
    match namespace {
        "source" => connection
            .execute(
                "INSERT INTO content_sources
                     (sha256, relative_path, size_bytes, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                params![
                    sha256,
                    relative_path,
                    u64_to_i64(size_bytes).expect("small source size"),
                    Utc::now().to_rfc3339(),
                ],
            )
            .expect("insert rollback-only source row"),
        "asset" => connection
            .execute(
                "INSERT INTO assets
                     (sha256, relative_path, media_type, size_bytes, created_at)
                     VALUES (?1, ?2, 'image/png', ?3, ?4)",
                params![
                    sha256,
                    relative_path,
                    u64_to_i64(size_bytes).expect("small asset size"),
                    Utc::now().to_rfc3339(),
                ],
            )
            .expect("insert rollback-only asset row"),
        _ => unreachable!("validated rollback-only CAS namespace"),
    };
    drop(connection);
    let final_path = root.path().join(&relative_path);
    fs::create_dir_all(final_path.parent().expect("rollback-only CAS parent"))
        .expect("create rollback-only CAS directory");
    fs::write(&final_path, bytes).expect("write rollback-only CAS object");
    let storage = Storage::open(root.path()).expect("cut over rollback-only CAS fixture");
    let intent = PackageCasPromotionIntent {
        import_id: import_id.to_owned(),
        namespace,
        sha256,
        size_bytes,
        media_type,
        relative_path,
    };
    (root, storage, intent, final_path)
}

struct DurableCommittingPackageFixture {
    root: tempfile::TempDir,
    import_id: String,
    database_path: PathBuf,
    promoted_asset_path: PathBuf,
    partial_document: Option<(String, String)>,
}

#[allow(clippy::too_many_lines)] // The fixture follows every guarded durable transition.
fn durable_committing_package_fixture(
    import_id: &str,
    with_partial_document: bool,
) -> DurableCommittingPackageFixture {
    let root = tempdir().expect("temp root");
    let source_sha256 = "ab".repeat(32);
    let snapshot_sha256 = "cd".repeat(32);
    let audit_payload = r#"{"schema_version":1,"value":{}}"#;
    let audit_payload_sha256 = hex::encode(Sha256::digest(audit_payload.as_bytes()));
    let created_at = "2026-08-16T00:00:00Z";
    let storage = Storage::open(root.path()).expect("open storage");
    let asset_bytes = b"\x89PNG\r\n\x1a\ninterrupted-package-asset";
    let asset_sha256 = hex::encode(Sha256::digest(asset_bytes));
    let staged_asset_path = storage.staging_dir().join("interrupted-asset.png");
    fs::write(&staged_asset_path, asset_bytes).expect("write staged package asset");
    let promoted_asset_path = storage
        .promote_package_assets(
            import_id,
            &[StagedAssetImport {
                staged_path: staged_asset_path,
                sha256: asset_sha256,
                media_type: "image/png".to_owned(),
                size_bytes: u64::try_from(asset_bytes.len()).expect("small package asset"),
            }],
        )
        .expect("promote package asset before commit")
        .into_iter()
        .next()
        .expect("one promoted package asset");
    let partial_document = with_partial_document.then(|| {
        let component_sha256 = "ef".repeat(32);
        let book: lorepia_domain::KnowledgeBook = serde_json::from_value(serde_json::json!({
            "id": format!("{import_id}-knowledge"),
            "name": "Interrupted package knowledge",
            "schema_version": 1,
            "entries": [],
            "scan_depth": 1,
            "token_budget": {"max_tokens": 1024},
            "recursive": false,
            "max_recursion_depth": 0,
            "provenance": {
                "source_kind": "imported_package",
                "source_id": "interrupted-package",
                "source_hash": source_sha256,
                "author": null,
                "license": null,
                "imported_at": null
            }
        }))
        .expect("valid interrupted knowledge document");
        let descriptor = lorepia_orchestration::PackageComponentDescriptor {
            id: format!("{import_id}-component"),
            kind: lorepia_orchestration::PackageComponentKind::KnowledgeBook,
            logical_path: "knowledge/interrupted.json".to_owned(),
            sha256: Sha256Digest::parse(component_sha256).expect("component digest"),
            dependencies: Vec::new(),
            conflicts_with: Vec::new(),
            required_capabilities: Vec::new(),
            asset_ids: Vec::new(),
            disposition: lorepia_orchestration::PackageComponentDisposition::Importable,
        };
        let document_json = serde_json::to_string(&book).expect("encode knowledge document");
        let document_sha256 = hex::encode(Sha256::digest(document_json.as_bytes()));
        let review_json = serde_json::to_string(&descriptor).expect("encode component review");
        let review_sha256 = hex::encode(Sha256::digest(review_json.as_bytes()));
        let revision_id = format!("{import_id}-knowledge-revision-1");
        (
            book,
            descriptor,
            document_json,
            document_sha256,
            review_json,
            review_sha256,
            revision_id,
        )
    });
    let database_path = {
        let connection = storage.connection().expect("database connection");
        let database_path = connection
            .query_row("PRAGMA database_list", [], |row| row.get::<_, String>(2))
            .map(PathBuf::from)
            .expect("active database path");
        connection
            .execute(
                "INSERT INTO content_sources (
                         sha256, relative_path, size_bytes, created_at
                     ) VALUES (?1, 'sources/synthetic-interrupted-package', 1, ?2)",
                params![source_sha256, created_at],
            )
            .expect("insert package source CAS row");
        connection
            .execute(
                "INSERT INTO package_sources (
                         id, source_hash, format, format_version, package_id, name,
                         version, author, manifest_json, manifest_sha256,
                         license_expression, license_status, redistribution_status,
                         required_app_version, signature_json, signature_status,
                         created_at
                     ) VALUES (
                         'interrupted-package-source', ?1, 'lorepia_content_package', 1,
                         'interrupted-package', 'Interrupted package', '1.0.0', NULL,
                         '{}', ?2, NULL, 'unknown', 'unknown', NULL, NULL, 'unsigned', ?3
                     )",
                params![source_sha256, snapshot_sha256, created_at],
            )
            .expect("insert package source");
        connection
            .execute(
                "INSERT INTO package_imports (
                         id, package_source_id, inspection_schema_version, state,
                         revision, inspection_json, inspection_sha256,
                         selection_json, selection_sha256,
                         capability_review_sha256, approved_selection_sha256,
                         approved_at, failure_json, created_at, updated_at, completed_at
                     ) VALUES (
                         ?1, 'interrupted-package-source', 1, 'inspected', 1,
                         '{}', ?2, NULL, NULL, ?2, NULL, NULL, NULL, ?3, ?3, NULL
                     )",
                params![import_id, snapshot_sha256, created_at],
            )
            .expect("insert inspected package import");
        if let Some((book, descriptor, _, document_sha256, review_json, review_sha256, _)) =
            partial_document.as_ref()
        {
            connection
                .execute(
                    "INSERT INTO package_import_components (
                             import_id, ordinal, source_component_key, component_kind,
                             disposition, selected, target_object_id, target_revision_id,
                             review_json, review_sha256
                         ) VALUES (?1, 0, ?2, 'knowledge_book', 'create', 1,
                                   NULL, NULL, ?3, ?4)",
                    params![import_id, descriptor.id, review_json, review_sha256],
                )
                .expect("insert interrupted package component");
            connection
                .execute(
                    "INSERT INTO package_import_document_target_reviews (
                             import_id, component_ordinal, document_ordinal,
                             document_index, document_kind, target_object_id,
                             disposition, expected_target_revision_id,
                             expected_target_state_revision, source_component_sha256,
                             document_sha256
                         ) VALUES (?1, 0, 0, 0, 'knowledge_book', ?2, 'create',
                                   NULL, NULL, ?3, ?4)",
                    params![
                        import_id,
                        book.id.as_str(),
                        descriptor.sha256.as_str(),
                        document_sha256,
                    ],
                )
                .expect("insert interrupted package target review");
        }
        connection
            .execute(
                "INSERT INTO package_import_audit_events (
                         import_id, sequence, import_revision, event_kind,
                         payload_json, payload_sha256, created_at
                     ) VALUES (?1, 1, 1, 'inspected', ?2, ?3, ?4)",
                params![import_id, audit_payload, audit_payload_sha256, created_at],
            )
            .expect("insert inspection audit");
        connection
            .execute(
                "INSERT INTO package_import_audit_events (
                         import_id, sequence, import_revision, event_kind,
                         payload_json, payload_sha256, created_at
                     ) VALUES (?1, 2, 2, 'selection_changed', ?2, ?3, ?4)",
                params![import_id, audit_payload, audit_payload_sha256, created_at],
            )
            .expect("insert selection audit");
        connection
            .execute(
                "UPDATE package_imports
                     SET state = 'awaiting_review', revision = 2,
                         selection_json = '{}', selection_sha256 = ?2, updated_at = ?3
                     WHERE id = ?1",
                params![import_id, snapshot_sha256, created_at],
            )
            .expect("select package import");
        connection
            .execute(
                "INSERT INTO package_import_approvals (
                         id, import_id, inspection_sha256, selection_sha256,
                         capability_review_sha256, approval_payload_json, approved_at
                     ) VALUES (
                         'interrupted-package-approval', ?1, ?2, ?2, ?2, '{}', ?3
                     )",
                params![import_id, snapshot_sha256, created_at],
            )
            .expect("insert package approval");
        connection
            .execute(
                "INSERT INTO package_import_audit_events (
                         import_id, sequence, import_revision, event_kind,
                         payload_json, payload_sha256, created_at
                     ) VALUES (?1, 3, 3, 'approved', ?2, ?3, ?4)",
                params![import_id, audit_payload, audit_payload_sha256, created_at],
            )
            .expect("insert approval audit");
        connection
            .execute(
                "UPDATE package_imports
                     SET state = 'approved', revision = 3,
                         approved_selection_sha256 = ?2, approved_at = ?3, updated_at = ?3
                     WHERE id = ?1",
                params![import_id, snapshot_sha256, created_at],
            )
            .expect("approve package import");
        connection
            .execute(
                "INSERT INTO package_import_audit_events (
                         import_id, sequence, import_revision, event_kind,
                         payload_json, payload_sha256, created_at
                     ) VALUES (?1, 4, 4, 'commit_started', ?2, ?3, ?4)",
                params![import_id, audit_payload, audit_payload_sha256, created_at],
            )
            .expect("insert commit-started audit");
        connection
            .execute(
                "UPDATE package_imports
                     SET state = 'committing', revision = 4, updated_at = ?2
                     WHERE id = ?1",
                params![import_id, created_at],
            )
            .expect("persist interrupted commit state");
        if let Some((book, descriptor, document_json, document_sha256, _, _, revision_id)) =
            partial_document.as_ref()
        {
            let provenance_json =
                serde_json::to_string(&book.provenance).expect("encode provenance");
            connection
                .execute(
                    "INSERT INTO content_objects
                         (id, object_kind, created_at, deleted_at)
                         VALUES (?1, 'knowledge_book', ?2, NULL)",
                    params![book.id.as_str(), created_at],
                )
                .expect("insert partially committed content object");
            connection
                .execute(
                    "INSERT INTO content_revisions (
                             id, object_id, object_kind, revision_no, parent_revision_id,
                             schema_version, document_json, document_sha256, source_kind,
                             source_hash, provenance_json, local_override_of_revision_id,
                             created_at
                         ) VALUES (
                             ?1, ?2, 'knowledge_book', 1, NULL, 1, ?3, ?4,
                             'imported_package', ?5, ?6, NULL, ?7
                         )",
                    params![
                        revision_id,
                        book.id.as_str(),
                        document_json,
                        document_sha256,
                        source_sha256,
                        provenance_json,
                        created_at,
                    ],
                )
                .expect("insert partially committed content revision");
            connection
                .execute(
                    "INSERT INTO content_object_state
                         (object_id, active_revision_id, state_version, updated_at)
                         VALUES (?1, ?2, 1, ?3)",
                    params![book.id.as_str(), revision_id, created_at],
                )
                .expect("publish partial content state inside the fixture");
            connection
                .execute(
                    "INSERT INTO content_revision_events (
                             id, object_id, event_kind, from_revision_id, to_revision_id,
                             diff_json, diff_sha256, plan_sha256, idempotency_key, created_at
                         ) VALUES (?1, ?2, 'import', NULL, ?3, NULL, NULL, NULL, ?4, ?5)",
                    params![
                        format!("{import_id}-content-event"),
                        book.id.as_str(),
                        revision_id,
                        format!("{import_id}-content-idempotency"),
                        created_at,
                    ],
                )
                .expect("insert partial content revision event");
            connection
                .execute(
                    "INSERT INTO knowledge_books (
                             id, name, schema_version, revision, scan_depth, token_budget,
                             recursive, max_recursion_depth, document_json, provenance_json,
                             source_kind, source_hash, created_at, updated_at, deleted_at
                         ) VALUES (?1, ?2, 1, 1, ?3, ?4, ?5, ?6, ?7, ?8,
                                   'imported_package', ?9, ?10, ?10, NULL)",
                    params![
                        book.id.as_str(),
                        book.name,
                        book.scan_depth,
                        book.token_budget.max_tokens,
                        book.recursive,
                        book.max_recursion_depth,
                        document_json,
                        provenance_json,
                        source_sha256,
                        created_at,
                    ],
                )
                .expect("insert partial knowledge projection");
            connection
                .execute(
                    "INSERT INTO knowledge_book_revisions (
                             revision_id, knowledge_book_id, revision_no, name,
                             description, token_budget, scan_depth, recursive,
                             max_recursion_depth, document_json
                         ) VALUES (?1, ?2, 1, ?3, '', ?4, ?5, ?6, ?7, ?8)",
                    params![
                        revision_id,
                        book.id.as_str(),
                        book.name,
                        book.token_budget.max_tokens,
                        book.scan_depth,
                        book.recursive,
                        book.max_recursion_depth,
                        document_json,
                    ],
                )
                .expect("insert partial knowledge revision projection");
            let result_json = serde_json::to_string(&serde_json::json!({
                "schema_version": 1,
                "value": {
                    "source_component_key": descriptor.id,
                    "component_document_ordinal": 0,
                    "source_component_sha256": descriptor.sha256.as_str(),
                    "document_sha256": document_sha256,
                    "target_object_id": book.id.as_str(),
                    "target_revision_id": revision_id,
                    "target_state_revision": 1,
                }
            }))
            .expect("encode interrupted component result");
            let result_sha256 = hex::encode(Sha256::digest(result_json.as_bytes()));
            connection
                .execute(
                    "INSERT INTO package_import_component_commits (
                             import_id, component_ordinal, document_ordinal,
                             target_object_id, target_revision_id, result_json,
                             result_sha256, committed_at
                         ) VALUES (?1, 0, 0, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        import_id,
                        book.id.as_str(),
                        revision_id,
                        result_json,
                        result_sha256,
                        created_at,
                    ],
                )
                .expect("insert partial package component result");
        }
        assert_eq!(
            connection
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .expect("fixture integrity check"),
            "ok"
        );
        assert!(
            connection
                .prepare("PRAGMA foreign_key_check")
                .expect("foreign key check")
                .query([])
                .expect("foreign key rows")
                .next()
                .expect("foreign key result")
                .is_none(),
            "fixture must satisfy every foreign key"
        );
        database_path
    };
    drop(storage);

    DurableCommittingPackageFixture {
        root,
        import_id: import_id.to_owned(),
        database_path,
        promoted_asset_path,
        partial_document: partial_document
            .map(|(book, _, _, _, _, _, revision_id)| (book.id.as_str().to_owned(), revision_id)),
    }
}

fn assert_durable_committing_fixture_unchanged(fixture: &DurableCommittingPackageFixture) {
    let connection = Connection::open(&fixture.database_path).expect("inspect rejected database");
    assert_eq!(
        connection
            .query_row(
                "SELECT state, revision, failure_json, completed_at
                     FROM package_imports WHERE id = ?1",
                [fixture.import_id.as_str()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, u64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .expect("load rejected package import"),
        ("committing".to_owned(), 4, None, None),
        "fail-closed open must not reinterpret or mutate impossible durable state"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM package_import_audit_events WHERE import_id = ?1",
                [fixture.import_id.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .expect("load unchanged package audit count"),
        4
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT event_kind FROM package_import_audit_events
                     WHERE import_id = ?1 ORDER BY sequence DESC LIMIT 1",
                [fixture.import_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .expect("load unchanged final package audit"),
        "commit_started"
    );
    assert!(
        fixture.promoted_asset_path.exists(),
        "fail-closed open must preserve forensic CAS evidence"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM package_cas_promotion_journal
                     WHERE import_id = ?1 AND namespace = 'asset'",
                [fixture.import_id.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .expect("load unchanged package CAS journal"),
        1
    );
}

#[test]
fn startup_rejects_impossible_durable_package_committing_state_without_mutation() {
    let fixture = durable_committing_package_fixture("package-impossible-committing", false);

    let Err(error) = Storage::open(fixture.root.path()) else {
        panic!("durable committing state must fail closed");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    assert_eq!(
        error.message,
        "durable package import remained in the impossible committing state"
    );
    assert_durable_committing_fixture_unchanged(&fixture);
}

#[test]
fn startup_rejects_partial_package_commit_effects_without_publishing_or_mutating_them() {
    let fixture = durable_committing_package_fixture("package-partial-committing", true);
    let (document_id, revision_id) = fixture
        .partial_document
        .as_ref()
        .expect("partial document fixture");

    let Err(error) = Storage::open(fixture.root.path()) else {
        panic!("schema-valid partial package commit must fail closed");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    assert_eq!(
        error.message,
        "durable package import remained in the impossible committing state"
    );
    assert_durable_committing_fixture_unchanged(&fixture);

    let connection = Connection::open(&fixture.database_path).expect("inspect partial effects");
    assert_eq!(
        connection
            .query_row(
                "SELECT state.active_revision_id, revision.source_kind
                     FROM content_objects AS object
                     JOIN content_object_state AS state ON state.object_id = object.id
                     JOIN content_revisions AS revision
                       ON revision.object_id = object.id
                      AND revision.id = state.active_revision_id
                     WHERE object.id = ?1 AND object.deleted_at IS NULL",
                [document_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("load preserved partial content"),
        (revision_id.clone(), "imported_package".to_owned())
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM package_import_component_commits
                     WHERE import_id = ?1 AND target_object_id = ?2
                       AND target_revision_id = ?3",
                params![fixture.import_id, document_id, revision_id],
                |row| row.get::<_, u64>(0),
            )
            .expect("load preserved component effect"),
        1
    );
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .expect("post-rejection integrity check"),
        "ok"
    );
}

#[test]
fn package_cas_recovery_removes_intent_before_file_publish() {
    let root = tempdir().expect("temp root");
    let (intent, final_path) = {
        let storage = Storage::open(root.path()).expect("open storage");
        let (_, intent, final_path) = staged_package_source(
            &storage,
            "intent.snapshot",
            b"package intent crash",
            "package-crash-intent",
        );
        ensure_package_cas_promotion_intents(
            &mut storage.connection().expect("database"),
            std::slice::from_ref(&intent),
        )
        .expect("persist intent");
        (intent, final_path)
    };
    assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
}

#[test]
fn package_cas_recovery_removes_file_published_before_phase_update() {
    let root = tempdir().expect("temp root");
    let (intent, final_path) = {
        let storage = Storage::open(root.path()).expect("open storage");
        let (staged, intent, final_path) = staged_package_source(
            &storage,
            "published.snapshot",
            b"package published crash",
            "package-crash-published",
        );
        ensure_package_cas_promotion_intents(
            &mut storage.connection().expect("database"),
            std::slice::from_ref(&intent),
        )
        .expect("persist intent");
        store_verified_source(
            &staged,
            &final_path,
            &storage.data_root().join("sources/sha256"),
            &intent.sha256,
            intent.size_bytes,
        )
        .expect("publish CAS file");
        (intent, final_path)
    };
    assert!(final_path.is_file());
    assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
}

#[test]
fn package_cas_recovery_removes_file_durable_before_row_registration() {
    let root = tempdir().expect("temp root");
    let (intent, final_path) = {
        let storage = Storage::open(root.path()).expect("open storage");
        let (staged, intent, final_path) = staged_package_source(
            &storage,
            "durable.snapshot",
            b"package durable crash",
            "package-crash-file-durable",
        );
        ensure_package_cas_promotion_intents(
            &mut storage.connection().expect("database"),
            std::slice::from_ref(&intent),
        )
        .expect("persist intent");
        store_verified_source(
            &staged,
            &final_path,
            &storage.data_root().join("sources/sha256"),
            &intent.sha256,
            intent.size_bytes,
        )
        .expect("publish CAS file");
        mark_package_cas_file_durable(&storage.connection().expect("database"), &intent)
            .expect("mark file durable");
        (intent, final_path)
    };
    assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
}

#[test]
fn package_cas_recovery_removes_row_registered_before_product_claim() {
    let root = tempdir().expect("temp root");
    let (intent, final_path) = {
        let storage = Storage::open(root.path()).expect("open storage");
        let (staged, intent, final_path) = staged_package_source(
            &storage,
            "registered.snapshot",
            b"package row registered crash",
            "package-crash-row-registered",
        );
        storage
            .promote_package_source(
                &intent.import_id,
                &staged,
                &intent.sha256,
                intent.size_bytes,
            )
            .expect("register source CAS");
        (intent, final_path)
    };
    assert!(final_path.is_file());
    assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
}

#[test]
fn package_cas_recovery_finishes_cleanup_pending_after_row_delete() {
    let root = tempdir().expect("temp root");
    let (intent, final_path) = {
        let storage = Storage::open(root.path()).expect("open storage");
        let (staged, intent, final_path) = staged_package_source(
            &storage,
            "cleanup.snapshot",
            b"package cleanup crash",
            "package-crash-cleanup-pending",
        );
        storage
            .promote_package_source(
                &intent.import_id,
                &staged,
                &intent.sha256,
                intent.size_bytes,
            )
            .expect("register source CAS");
        {
            let mut connection = storage.connection().expect("database");
            let transaction = connection.transaction().expect("cleanup transaction");
            transaction
                .execute(
                    "UPDATE package_cas_promotion_journal
                         SET phase = 'cleanup_pending', updated_at = ?4
                         WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
                    params![
                        intent.import_id,
                        intent.namespace,
                        intent.sha256,
                        Utc::now().to_rfc3339(),
                    ],
                )
                .expect("mark cleanup pending");
            transaction
                .execute(
                    "DELETE FROM content_sources WHERE sha256 = ?1",
                    [intent.sha256.as_str()],
                )
                .expect("delete CAS row");
            transaction.commit().expect("commit cleanup phase");
        }
        (intent, final_path)
    };
    assert!(final_path.is_file());
    assert_abandoned_package_source_recovered(root.path(), &intent, &final_path);
}

#[test]
fn package_cleanup_preserves_same_digest_pinned_by_canonical_generation() {
    for namespace in ["source", "asset"] {
        let (_root, storage, intent, final_path) =
            rollback_only_cas_promotion(namespace, &format!("rollback-cleanup-{namespace}"));
        ensure_package_cas_promotion_intents(
            &mut storage.connection().expect("active database"),
            std::slice::from_ref(&intent),
        )
        .expect("persist same-digest cleanup intent");
        let cas_mutation = storage.cas_mutation().expect("CAS mutation guard");

        assert!(
            !cleanup_package_cas_promotion(&storage, &cas_mutation, &intent)
                .expect("clean up active-only package promotion"),
            "a canonical rollback pin is retained rather than physically removed"
        );
        assert!(
            final_path.is_file(),
            "{namespace} bytes pinned by the canonical generation must survive normal cleanup"
        );
    }
}

#[test]
fn startup_recovery_preserves_same_digest_pinned_by_canonical_generation() {
    for namespace in ["source", "asset"] {
        let (root, storage, intent, final_path) =
            rollback_only_cas_promotion(namespace, &format!("rollback-recovery-{namespace}"));
        ensure_package_cas_promotion_intents(
            &mut storage.connection().expect("active database"),
            std::slice::from_ref(&intent),
        )
        .expect("persist abandoned same-digest promotion");
        drop(storage);

        let reopened = Storage::open(root.path()).expect("recover abandoned package promotion");
        assert!(
            final_path.is_file(),
            "{namespace} bytes pinned by the canonical generation must survive startup recovery"
        );
        assert_eq!(
            reopened
                .connection()
                .expect("recovered database")
                .query_row(
                    "SELECT COUNT(*) FROM package_cas_promotion_journal
                         WHERE import_id = ?1 AND namespace = ?2 AND sha256 = ?3",
                    params![intent.import_id, intent.namespace, intent.sha256],
                    |row| row.get::<_, u64>(0),
                )
                .expect("recovered promotion journal count"),
            0
        );
    }
}

#[test]
fn package_asset_cas_recovery_removes_registered_orphan() {
    let root = tempdir().expect("temp root");
    let (sha256, size_bytes, final_path) = {
        let storage = Storage::open(root.path()).expect("open storage");
        let bytes = b"\x89PNG\r\n\x1a\npackage-asset-crash";
        let sha256 = hex::encode(Sha256::digest(bytes));
        let staged_path = storage.staging_dir().join("asset-crash.png");
        fs::write(&staged_path, bytes).expect("write staged asset");
        let staged = StagedAssetImport {
            staged_path,
            sha256: sha256.clone(),
            media_type: "image/png".to_owned(),
            size_bytes: u64::try_from(bytes.len()).expect("small asset"),
        };
        let paths = storage
            .promote_package_assets("package-asset-crash", &[staged])
            .expect("promote package asset");
        (
            sha256,
            u64::try_from(bytes.len()).expect("small asset"),
            paths[0].clone(),
        )
    };
    assert!(final_path.is_file());
    let reopened = Storage::open(root.path()).expect("reopen asset CAS storage");
    assert!(!final_path.exists());
    assert_eq!(
        reopened
            .connection()
            .expect("database")
            .query_row(
                "SELECT COUNT(*) FROM assets WHERE sha256 = ?1",
                [sha256.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .expect("asset row count"),
        0
    );
    assert_eq!(size_bytes, 27);
}

#[test]
fn package_cas_shared_digest_survives_other_operation_cleanup_and_restart() {
    let root = tempdir().expect("temp root");
    let (second_intent, final_path) = {
        let storage = Storage::open(root.path()).expect("open storage");
        let bytes = b"shared package CAS digest";
        let (first_staged, first_intent, final_path) =
            staged_package_source(&storage, "shared-a.snapshot", bytes, "package-shared-a");
        let (second_staged, second_intent, _) =
            staged_package_source(&storage, "shared-b.snapshot", bytes, "package-shared-b");
        storage
            .promote_package_source(
                &first_intent.import_id,
                &first_staged,
                &first_intent.sha256,
                first_intent.size_bytes,
            )
            .expect("promote first shared source");
        storage
            .promote_package_source(
                &second_intent.import_id,
                &second_staged,
                &second_intent.sha256,
                second_intent.size_bytes,
            )
            .expect("promote second shared source");
        assert!(
            !storage
                .cleanup_package_source_promotion(
                    &first_intent.import_id,
                    &first_intent.sha256,
                    first_intent.size_bytes,
                )
                .expect("cleanup first shared promotion")
        );
        assert!(final_path.is_file());
        storage
            .connection()
            .expect("database")
            .execute(
                "INSERT INTO characters (
                        id, name, description, source_hash,
                        avatar_asset_hash, created_at
                     ) VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
                params![
                    "shared-cas-character",
                    "Shared CAS",
                    "Synthetic reference",
                    second_intent.sha256,
                    Utc::now().to_rfc3339(),
                ],
            )
            .expect("claim shared source from product data");
        (second_intent, final_path)
    };
    let reopened = Storage::open(root.path()).expect("reopen shared CAS storage");
    assert!(final_path.is_file(), "referenced shared CAS must survive");
    reopened
        .package_source_path(&second_intent.sha256, second_intent.size_bytes)
        .expect("shared source remains registered");
    assert_eq!(
        reopened
            .connection()
            .expect("database")
            .query_row(
                "SELECT COUNT(*) FROM package_cas_promotion_journal
                     WHERE sha256 = ?1",
                [second_intent.sha256.as_str()],
                |row| row.get::<_, u64>(0),
            )
            .expect("journal count"),
        0
    );
}

#[test]
fn atomic_cas_publish_never_replaces_an_existing_destination() {
    let root = tempdir().expect("temp root");
    let temp_path = root.path().join("new.partial");
    let final_path = root.path().join("final");
    fs::write(&temp_path, b"new").expect("temporary content");
    fs::write(&final_path, b"existing").expect("existing content");

    let error = publish_temp_noclobber(&temp_path, &final_path).expect_err("must not overwrite");
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&final_path).expect("final content"), b"existing");
    assert_eq!(fs::read(&temp_path).expect("temporary content"), b"new");
}

#[cfg(unix)]
#[test]
fn recovery_rejects_a_symlinked_cas_hash_prefix() {
    use std::os::unix::fs::symlink;

    let root = tempdir().expect("temp root");
    let source_hash = format!("aa{}", "0".repeat(62));
    let staging_path = root.path().join("staging/inspection-symlink.json");
    {
        let storage = Storage::open(root.path()).expect("open storage");
        fs::write(&staging_path, b"staging").expect("staging");
        storage
            .connection()
            .expect("connection")
            .execute(
                "INSERT INTO import_jobs
                     (id, source_hash, staging_path, state, updated_at, asset_hashes_json)
                     VALUES (?1, ?2, ?3, 'file_stored', ?4, '[]')",
                params![
                    "symlink-job",
                    source_hash,
                    staging_path.to_string_lossy(),
                    Utc::now().to_rfc3339()
                ],
            )
            .expect("journal");
    }
    let outside = root.path().join("outside");
    fs::create_dir(&outside).expect("outside");
    symlink(&outside, root.path().join("sources/sha256/aa")).expect("prefix symlink");

    let Err(error) = Storage::open(root.path()) else {
        panic!("symlinked CAS prefix must be rejected");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    assert_eq!(
        error.message,
        "CAS recovery hash-prefix path is not a real directory"
    );
}
