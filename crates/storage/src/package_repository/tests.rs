use lorepia_domain::{
    AssetDescriptor, AssetId, AssetRole, AssetSource, AssetSourceKind, KnowledgeBook,
    MemoryProfile, PackageManifest, PromptPresetId, Provenance, Sha256Digest,
};
use lorepia_orchestration::RedistributionStatus;
use tempfile::tempdir;

use super::*;

fn write_staged(storage: &Storage, name: &str, bytes: &[u8]) -> PathBuf {
    let path = storage.staging_dir().join(name);
    fs::write(&path, bytes).expect("write owned staged fixture");
    path
}

fn source_record(hash: &str, size: u64) -> PackageSourceRecord {
    PackageSourceRecord {
        id: format!("package-source-{hash}"),
        package_id: PackageId::from("cleanup-package"),
        format: "lorepia_content_package".to_owned(),
        format_version: 1,
        name: "Cleanup package".to_owned(),
        version: "1.0.0".to_owned(),
        source_sha256: hash.to_owned(),
        source_size_bytes: size,
        author: Some("LorePia tests".to_owned()),
        license: "MIT".to_owned(),
        redistribution_allowed: true,
        manifest: VersionedJson {
            schema_version: 1,
            value: json!({}),
        },
        created_at: Utc::now(),
    }
}

fn invalid_review(hash: &str) -> PackageReview {
    let digest = Sha256Digest::parse(hash).expect("fixture digest");
    PackageReview {
        review_sha256: Sha256Digest::parse("00".repeat(32)).expect("review digest"),
        source_sha256: digest.clone(),
        manifest: PackageManifest {
            format: "lorepia_content_package".to_owned(),
            format_version: 1,
            package_id: PackageId::from("cleanup-package"),
            name: "Cleanup package".to_owned(),
            version: "1.0.0".to_owned(),
            author: Some("LorePia tests".to_owned()),
            license: "MIT".to_owned(),
            redistribution_allowed: true,
            required_app_version: None,
            required_capabilities: Vec::new(),
            content_hashes: Vec::new(),
            signature: None,
            provenance: Provenance {
                source_kind: SourceKind::ImportedPackage,
                source_id: Some("cleanup-package".to_owned()),
                source_hash: Some(hash.to_owned()),
                author: Some("LorePia tests".to_owned()),
                license: Some("MIT".to_owned()),
                imported_at: Some(Utc::now()),
            },
        },
        components: Vec::new(),
        assets: Vec::new(),
        issues: Vec::new(),
        local_import_allowed: true,
        redistribution_status: RedistributionStatus::Allowed,
    }
}

fn promote_missing_approved_source(storage: &Storage) -> (String, u64) {
    let source_bytes = b"synthetic commit source";
    let source_hash = sha256_hex(source_bytes);
    let source_size = u64::try_from(source_bytes.len()).expect("small source fixture");
    let source_staged = write_staged(storage, "commit.snapshot", source_bytes);
    storage
        .promote_package_source(
            "missing-approved-import",
            &source_staged,
            &source_hash,
            source_size,
        )
        .expect("promote source");
    (source_hash, source_size)
}

fn imported_document_provenance() -> Provenance {
    Provenance {
        source_kind: SourceKind::ImportedPackage,
        source_id: Some("dev.lorepia.storage-validation-test".to_owned()),
        source_hash: Some("ab".repeat(32)),
        author: Some("LorePia tests".to_owned()),
        license: Some("MIT".to_owned()),
        imported_at: None,
    }
}

#[test]
fn storage_rejects_noncanonical_imported_knowledge_before_persistence() {
    let book: KnowledgeBook = serde_json::from_value(json!({
        "id": "storage.package.invalid-knowledge",
        "name": "Invalid imported knowledge",
        "schema_version": 1,
        "entries": [],
        "scan_depth": 1025,
        "token_budget": {"max_tokens": 1024},
        "recursive": false,
        "max_recursion_depth": 0,
        "provenance": imported_document_provenance()
    }))
    .expect("typed invalid knowledge fixture");
    let error =
        validate_normalized_package_documents(&[PackageCommitDocument::KnowledgeBook(book)])
            .expect_err("storage must reject invalid knowledge before persistence");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
}

#[test]
fn storage_rejects_noncanonical_imported_memory_before_persistence() {
    let profile: MemoryProfile = serde_json::from_value(json!({
        "id": "storage.package.invalid-memory",
        "name": "Invalid imported memory",
        "schema_version": 1,
        "summary_task": "memory-summary",
        "embedding_task": null,
        "turns_per_summary": 0,
        "recent_raw_budget": {"max_tokens": 1024},
        "episodic_budget": {"max_tokens": 1024},
        "semantic_budget": {"max_tokens": 1024},
        "retrieval_count": 8,
        "recency_weight": 1.0,
        "similarity_weight": 1.0,
        "importance_weight": 1.0,
        "preserve_invalidated_records": false,
        "summary_schema": "memory-summary-v1",
        "provenance": imported_document_provenance()
    }))
    .expect("typed invalid memory fixture");
    let error =
        validate_normalized_package_documents(&[PackageCommitDocument::MemoryProfile(profile)])
            .expect_err("storage must reject invalid memory before persistence");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
}

#[test]
fn storage_rejects_imported_prompt_with_elevated_package_block_authority() {
    let imported_provenance = imported_document_provenance();
    let mut preset = crate::orchestration::built_in_prompt_presets()[0].clone();
    preset.id = PromptPresetId::from("storage.package.prompt-authority-boundary");
    preset.metadata.provenance = imported_provenance.clone();
    for block in preset.blocks.iter_mut().skip(1) {
        block.authority = InstructionAuthority::ImportedContent;
        block.provenance = imported_provenance.clone();
    }
    preset.blocks[1].authority = InstructionAuthority::Creator;

    let error =
        validate_normalized_package_documents(&[PackageCommitDocument::PromptPreset(preset)])
            .expect_err(
                "storage must reject elevated imported prompt authority before persistence",
            );
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
}

fn promote_missing_approved_asset(
    storage: &Storage,
    source_hash: &str,
) -> (StagedAssetImport, PathBuf, AssetDescriptor) {
    let asset_bytes = b"\x89PNG\r\n\x1a\nsynthetic-package-image";
    let asset_hash = sha256_hex(asset_bytes);
    let asset_size = u64::try_from(asset_bytes.len()).expect("small asset fixture");
    let staged_asset = StagedAssetImport {
        staged_path: write_staged(storage, "asset.partial", asset_bytes),
        sha256: asset_hash.clone(),
        media_type: "image/png".to_owned(),
        size_bytes: asset_size,
    };
    let durable_assets = storage
        .promote_package_assets(
            "missing-approved-import",
            std::slice::from_ref(&staged_asset),
        )
        .expect("promote asset");
    assert_eq!(durable_assets.len(), 1);
    let durable_asset = durable_assets
        .into_iter()
        .next()
        .expect("one promoted asset");
    assert!(durable_asset.is_file());
    let descriptor = AssetDescriptor {
        id: AssetId::from("cleanup-asset"),
        sha256: Sha256Digest::parse(&asset_hash).expect("asset digest"),
        media_type: "image/png".to_owned(),
        role: AssetRole::Illustration,
        name: "asset.png".to_owned(),
        size_bytes: asset_size,
        width: None,
        height: None,
        duration_ms: None,
        source: AssetSource {
            kind: AssetSourceKind::LorepiaPackage,
            source_sha256: Some(Sha256Digest::parse(source_hash).expect("source digest")),
            logical_path: Some("assets/asset.png".to_owned()),
        },
    };
    (staged_asset, durable_asset, descriptor)
}

fn missing_approved_package_commit_input(
    source_hash: &str,
    source_size: u64,
    asset: AssetDescriptor,
) -> PackageCommitInput {
    let now = Utc::now();
    PackageCommitInput {
        source: source_record(source_hash, source_size),
        import: PackageImportRecord {
            id: "missing-approved-import".to_owned(),
            package_id: PackageId::from("cleanup-package"),
            status: PackageImportStatus::Approved,
            revision: 3,
            inspection: VersionedJson {
                schema_version: 1,
                value: json!({}),
            },
            selection: Some(VersionedJson {
                schema_version: 1,
                value: json!({}),
            }),
            selected_component_ids: Vec::new(),
            failure_code: None,
            created_at: now,
            updated_at: now,
        },
        documents: Vec::new(),
        assets: vec![asset],
    }
}

#[test]
fn failed_inspection_creation_removes_unclaimed_source_row_and_cas_bytes() {
    let root = tempdir().expect("data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let bytes = b"synthetic package source";
    let hash = sha256_hex(bytes);
    let source_size = u64::try_from(bytes.len()).expect("small source fixture");
    let staged = write_staged(&storage, "source.snapshot", bytes);
    let durable = storage
        .promote_package_source("cleanup-invalid-inspection", &staged, &hash, source_size)
        .expect("promote source");
    assert!(durable.is_file());

    let source = source_record(&hash, source_size);
    let now = Utc::now();
    let import = PackageImportRecord {
        id: "cleanup-invalid-inspection".to_owned(),
        package_id: source.package_id.clone(),
        status: PackageImportStatus::Inspected,
        revision: 1,
        inspection: VersionedJson {
            schema_version: 1,
            value: json!({}),
        },
        selection: None,
        selected_component_ids: Vec::new(),
        failure_code: None,
        created_at: now,
        updated_at: now,
    };
    let error = storage
        .create_inspected_package_import(
            &source,
            &import,
            &invalid_review(&hash),
            &PackageCapabilityReview {
                schema_version: 1,
                decisions: Vec::new(),
            },
        )
        .expect_err("invalid inspection must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        storage
            .discard_unclaimed_package_source("cleanup-invalid-inspection", &hash, source_size,)
            .expect("compensate source")
    );
    assert!(!durable.exists());
    assert_eq!(
        storage
            .package_source_path(&hash, source_size)
            .expect_err("source row must be removed")
            .code,
        CoreErrorCode::NotFound
    );
}

#[test]
fn failed_package_commit_removes_unclaimed_asset_row_and_cas_bytes() {
    let root = tempdir().expect("data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let (source_hash, source_size) = promote_missing_approved_source(&storage);
    let (staged_asset, durable_asset, asset) =
        promote_missing_approved_asset(&storage, &source_hash);
    let input = missing_approved_package_commit_input(&source_hash, source_size, asset);
    let expectation = PackageImportExpectation {
        revision: 3,
        inspection_sha256: "11".repeat(32),
        selection_sha256: "22".repeat(32),
        capability_review_sha256: "33".repeat(32),
    };
    let error = storage
        .commit_package_import(&input, &expectation, &[])
        .expect_err("missing approved import must fail after CAS verification");
    assert_eq!(error.code, CoreErrorCode::NotFound);
    assert_eq!(
        storage
            .discard_unclaimed_package_assets(
                "missing-approved-import",
                std::slice::from_ref(&staged_asset),
            )
            .expect("compensate asset"),
        1
    );
    assert!(!durable_asset.exists());
    assert!(
        storage
            .discard_unclaimed_package_source("missing-approved-import", &source_hash, source_size,)
            .expect("compensate source")
    );
}

struct ModuleAssetAuthorityFixture {
    asset_id: AssetId,
    asset_content_sha256: Sha256Digest,
    descriptor: AssetDescriptor,
    descriptor_sha256: String,
    module_component: CompletedPackageComponentAuthority,
    component: lorepia_domain::ComponentHash,
    authority: CompletedPackageAuthority,
}

fn module_asset_authority_fixture() -> ModuleAssetAuthorityFixture {
    let asset_id = AssetId::from("module-asset");
    let asset_content_sha256 = Sha256Digest::parse("11".repeat(32)).expect("asset content digest");
    let descriptor = AssetDescriptor {
        id: asset_id.clone(),
        sha256: asset_content_sha256.clone(),
        media_type: "image/png".to_owned(),
        role: AssetRole::Illustration,
        name: "module-asset.png".to_owned(),
        size_bytes: 123,
        width: Some(16),
        height: Some(16),
        duration_ms: None,
        source: AssetSource {
            kind: AssetSourceKind::LorepiaPackage,
            source_sha256: Some(
                Sha256Digest::parse("22".repeat(32)).expect("package source digest"),
            ),
            logical_path: Some("assets/module-asset.png".to_owned()),
        },
    };
    let descriptor_json =
        encode_json("module asset descriptor fixture", &descriptor).expect("encode descriptor");
    let descriptor_sha256 = sha256_hex(descriptor_json.as_bytes());
    let module_component = CompletedPackageComponentAuthority {
        component_id: "content-module-component".to_owned(),
        kind: PackageComponentKind::ContentModule,
        sha256: "33".repeat(32),
        committed_documents: Vec::new(),
    };
    let component = lorepia_domain::ComponentHash {
        component: ModuleComponentRef::Asset {
            id: asset_id.clone(),
        },
        sha256: Sha256Digest::parse(&descriptor_sha256).expect("descriptor digest"),
    };
    let authority = CompletedPackageAuthority {
        approval_id: "package-approval".to_owned(),
        import_id: "package-import".to_owned(),
        package_id: PackageId::from("module-package"),
        status: PackageImportStatus::Completed,
        import_revision: 5,
        source_sha256: "22".repeat(32),
        inspection_sha256: "44".repeat(32),
        selection_sha256: "55".repeat(32),
        capability_review_sha256: "66".repeat(32),
        approval_sha256: "77".repeat(32),
        required_capabilities: vec![ContentCapability::ImageAssets],
        approved_capabilities: Vec::new(),
        enabled_components: vec![
            module_component.clone(),
            CompletedPackageComponentAuthority {
                component_id: "asset-index-component".to_owned(),
                kind: PackageComponentKind::AssetIndex,
                sha256: "88".repeat(32),
                committed_documents: Vec::new(),
            },
        ],
        committed_assets: vec![CompletedPackageAssetAuthority {
            asset_id: asset_id.clone(),
            descriptor: descriptor.clone(),
            descriptor_sha256: descriptor_sha256.clone(),
            cas_sha256: asset_content_sha256.as_str().to_owned(),
            source_components: vec![
                CompletedPackageAssetSourceAuthority {
                    component_id: module_component.component_id.clone(),
                    component_sha256: module_component.sha256.clone(),
                },
                CompletedPackageAssetSourceAuthority {
                    component_id: "asset-index-component".to_owned(),
                    component_sha256: "88".repeat(32),
                },
            ],
        }],
    };
    ModuleAssetAuthorityFixture {
        asset_id,
        asset_content_sha256,
        descriptor,
        descriptor_sha256,
        module_component,
        component,
        authority,
    }
}

#[test]
fn module_asset_authority_is_bound_to_the_content_module_component() {
    let fixture = module_asset_authority_fixture();
    let evidence = asset_module_component_authority(
        &fixture.component,
        &fixture.asset_id,
        &fixture.descriptor,
        &fixture.module_component,
        &fixture.authority,
    )
    .expect("module asset authority");
    assert_eq!(
        evidence.package_component_id,
        fixture.module_component.component_id
    );
    assert_eq!(
        evidence.package_component_sha256.as_str(),
        fixture.module_component.sha256
    );
    assert_eq!(
        evidence.committed_target_object_id,
        fixture.asset_id.as_str()
    );
    assert_eq!(
        evidence.committed_target_revision_id,
        fixture.descriptor_sha256
    );
    assert_eq!(
        evidence.committed_result_sha256.as_str(),
        fixture.descriptor_sha256
    );
    assert_eq!(
        evidence.committed_content_sha256.as_ref(),
        Some(&fixture.asset_content_sha256)
    );

    let unrelated_module_component = CompletedPackageComponentAuthority {
        component_id: "other-content-module".to_owned(),
        kind: PackageComponentKind::ContentModule,
        sha256: "99".repeat(32),
        committed_documents: Vec::new(),
    };
    assert_eq!(
        asset_module_component_authority(
            &fixture.component,
            &fixture.asset_id,
            &fixture.descriptor,
            &unrelated_module_component,
            &fixture.authority,
        )
        .expect_err("unrelated module component must not authorize the asset")
        .code,
        CoreErrorCode::PermissionDenied
    );
}

#[test]
fn linked_module_authority_keeps_package_and_inner_document_hashes_distinct() {
    let target_object_id = "module-knowledge";
    let target_revision_id = "module-knowledge-revision";
    let inner_document_sha256 = Sha256Digest::parse("11".repeat(32)).expect("inner digest");
    let package_component_sha256 = "22".repeat(32);
    let package_document_sha256 = "33".repeat(32);
    let commit_result_sha256 = "44".repeat(32);
    let component = lorepia_domain::ComponentHash {
        component: ModuleComponentRef::KnowledgeBook {
            id: lorepia_domain::KnowledgeBookId::from(target_object_id),
        },
        sha256: inner_document_sha256.clone(),
    };
    let package_component = CompletedPackageComponentAuthority {
        component_id: "knowledge-component".to_owned(),
        kind: PackageComponentKind::KnowledgeBook,
        sha256: package_component_sha256.clone(),
        committed_documents: vec![CompletedPackageDocumentAuthority {
            document_ordinal: 0,
            target_object_id: target_object_id.to_owned(),
            target_revision_id: target_revision_id.to_owned(),
            source_component_sha256: package_component_sha256.clone(),
            document_sha256: package_document_sha256.clone(),
            result_sha256: commit_result_sha256.clone(),
        }],
    };
    let authority = CompletedPackageAuthority {
        approval_id: "package-approval".to_owned(),
        import_id: "package-import".to_owned(),
        package_id: PackageId::from("module-package"),
        status: PackageImportStatus::Completed,
        import_revision: 5,
        source_sha256: "55".repeat(32),
        inspection_sha256: "66".repeat(32),
        selection_sha256: "77".repeat(32),
        capability_review_sha256: "88".repeat(32),
        approval_sha256: "99".repeat(32),
        required_capabilities: vec![ContentCapability::Knowledge],
        approved_capabilities: Vec::new(),
        enabled_components: vec![package_component.clone()],
        committed_assets: Vec::new(),
    };

    assert_ne!(
        package_document_sha256,
        inner_document_sha256.as_str(),
        "the package binding hashes a tagged commit envelope, not the inner revision"
    );
    let evidence = document_module_component_authority(
        &component,
        PackageComponentKind::KnowledgeBook,
        target_object_id,
        target_revision_id,
        &authority,
    )
    .expect("exact linked document authority");
    assert_eq!(evidence.component_sha256, inner_document_sha256);
    assert_eq!(
        evidence.package_component_id,
        package_component.component_id
    );
    assert_eq!(
        evidence.package_component_sha256.as_str(),
        package_component_sha256
    );
    assert_eq!(evidence.committed_target_object_id, target_object_id);
    assert_eq!(evidence.committed_target_revision_id, target_revision_id);
    assert_eq!(
        evidence.committed_result_sha256.as_str(),
        commit_result_sha256
    );

    assert_eq!(
        document_module_component_authority(
            &component,
            PackageComponentKind::KnowledgeBook,
            target_object_id,
            "different-revision",
            &authority,
        )
        .expect_err("a different immutable revision must not be authorized")
        .code,
        CoreErrorCode::PermissionDenied
    );
    let mut wrong_source = authority;
    wrong_source.enabled_components[0].committed_documents[0].source_component_sha256 =
        "aa".repeat(32);
    assert_eq!(
        document_module_component_authority(
            &component,
            PackageComponentKind::KnowledgeBook,
            target_object_id,
            target_revision_id,
            &wrong_source,
        )
        .expect_err("a different reviewed package component must not authorize the child")
        .code,
        CoreErrorCode::PermissionDenied
    );
}

fn target_review_document(index: u32) -> PackageDocumentTargetReview {
    PackageDocumentTargetReview {
        source_component_id: format!("component-{index}"),
        component_document_ordinal: 0,
        document_index: index,
        document_kind: "knowledge_book".to_owned(),
        target_object_id: format!("target-{index}"),
        disposition: PackageDocumentTargetDisposition::Create,
        expected_target_revision_id: None,
        expected_target_state_revision: None,
        source_component_sha256: "11".repeat(32),
        document_sha256: "22".repeat(32),
    }
}

#[test]
fn target_review_document_limit_is_enforced_by_the_canonical_digest_boundary() {
    let allowed = (0..u32::try_from(MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS)
        .expect("target-review limit fits u32"))
        .map(target_review_document)
        .collect::<Vec<_>>();
    package_import_target_review_sha256(&allowed).expect("bounded target review");

    let mut excessive = allowed;
    excessive.push(target_review_document(
        u32::try_from(MAX_PACKAGE_TARGET_REVIEW_DOCUMENTS).expect("target-review limit fits u32"),
    ));
    let error = package_import_target_review_sha256(&excessive)
        .expect_err("oversized target review must fail before persistence");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(error.message.contains("200-document limit"));
}

#[test]
fn update_confirmations_are_canonical_and_exact_while_create_needs_none() {
    let mut documents = vec![target_review_document(0), target_review_document(1)];
    documents[0].source_component_id = "mixed-component".to_owned();
    documents[0].component_document_ordinal = 0;
    documents[0].target_object_id = "existing-target".to_owned();
    documents[0].disposition = PackageDocumentTargetDisposition::Update;
    documents[0].expected_target_revision_id = Some("existing-revision".to_owned());
    documents[0].expected_target_state_revision = Some(7);
    documents[1].source_component_id = "mixed-component".to_owned();
    documents[1].component_document_ordinal = 1;
    let confirmation = PackageUpdateTargetConfirmation {
        source_component_id: "mixed-component".to_owned(),
        component_document_ordinal: 0,
        target_object_id: "existing-target".to_owned(),
        expected_target_revision_id: "existing-revision".to_owned(),
        expected_target_state_revision: 7,
    };

    validate_document_target_reviews(&documents).expect("mixed target review");
    validate_exact_update_target_confirmations(&documents, std::slice::from_ref(&confirmation))
        .expect("exact update confirmation");
    validate_exact_update_target_confirmations(&documents, &[])
        .expect_err("missing update confirmation");
    let mut stale = confirmation;
    stale.expected_target_state_revision += 1;
    validate_exact_update_target_confirmations(&documents, &[stale])
        .expect_err("stale update confirmation");
}

#[test]
fn completed_package_export_listing_is_bounded_ordered_and_status_filtered() {
    let root = tempdir().expect("data root");
    let storage = Storage::open(root.path()).expect("open storage");
    let source_sha256 = "ab".repeat(32);
    let connection = storage.connection().expect("open fixture connection");
    connection
        .execute("DROP TRIGGER package_imports_initial_state_guard", [])
        .expect("allow terminal-state list fixtures");
    connection
        .execute(
            "DROP TRIGGER package_imports_require_inspected_initial_state_v19",
            [],
        )
        .expect("allow terminal-state recovery fixtures");
    connection
        .execute(
            "INSERT INTO content_sources (
                    sha256, relative_path, size_bytes, created_at
                 ) VALUES (?1, 'sources/synthetic', 1, '2026-08-09T00:00:00Z')",
            [source_sha256.as_str()],
        )
        .expect("content source fixture");
    connection
        .execute(
            "INSERT INTO package_sources (
                    id, source_hash, format, format_version, package_id, name,
                    version, author, manifest_json, manifest_sha256,
                    license_expression, license_status, redistribution_status,
                    required_app_version, signature_json, signature_status,
                    created_at
                 ) VALUES (
                    'completed-export-source', ?1, 'lorepia_content_package', 1,
                    'completed-export-package', 'Completed export package',
                    '1.0.0', NULL, '{}', ?2, NULL, 'unknown', 'unknown',
                    NULL, NULL, 'unsigned', '2026-08-09T00:00:00Z'
                 )",
            params![source_sha256, "cd".repeat(32)],
        )
        .expect("package source fixture");
    for (id, state, updated_at) in [
        ("completed-old", "completed", "2026-08-09T01:00:00Z"),
        ("completed-b", "completed", "2026-08-09T03:00:00Z"),
        ("completed-a", "completed", "2026-08-09T03:00:00Z"),
        ("rolled-back-newer", "rolled_back", "2026-08-09T04:00:00Z"),
    ] {
        connection
            .execute(
                "INSERT INTO package_imports (
                        id, package_source_id, inspection_schema_version,
                        state, revision, inspection_json, inspection_sha256,
                        selection_json, selection_sha256,
                        capability_review_sha256, approved_selection_sha256,
                        approved_at, failure_json, created_at, updated_at,
                        completed_at
                     ) VALUES (
                        ?1, 'completed-export-source', 1, ?2, 4, '{}', ?3,
                        '{}', ?3, ?3, ?3, '2026-08-09T00:30:00Z', NULL,
                        '2026-08-09T00:00:00Z', ?4, ?4
                     )",
                params![id, state, "ef".repeat(32), updated_at],
            )
            .expect("terminal package import fixture");
    }
    drop(connection);

    assert_eq!(
        storage
            .list_completed_package_import_ids(2)
            .expect("bounded completed export identities"),
        ["completed-a", "completed-b"]
    );
    assert_eq!(
        storage
            .list_completed_package_import_ids(MAX_COMPLETED_PACKAGE_EXPORTS)
            .expect("all completed export identities"),
        ["completed-a", "completed-b", "completed-old"]
    );
    for invalid_limit in [0, MAX_COMPLETED_PACKAGE_EXPORTS + 1] {
        let error = storage
            .list_completed_package_import_ids(invalid_limit)
            .expect_err("completed export list limit must fail closed");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
    }
}
