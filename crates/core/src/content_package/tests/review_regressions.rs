#[test]
fn approved_large_asset_survives_restart_selection_approval_and_commit() {
    let root = tempdir().unwrap();
    let source = root.path().join("large.zip");
    let data = root.path().join("library");
    let mut bytes = b"ID3synthetic".to_vec();
    bytes.resize(65 * 1024 * 1024, 0);
    let digest = format!("{:x}", Sha256::digest(&bytes));
    let path = format!("assets/sha256/{digest}.mp3");
    let manifest = json!({
        "format": "lorepia_content_package", "format_version": 1,
        "package_id": "dev.lorepia.large-review", "name": "Large reviewed asset",
        "version": "1.0.0", "author": "Tests", "license": "MIT",
        "redistribution_allowed": true, "required_app_version": "0.1.0",
        "required_capabilities": ["media_assets"], "dependencies": [], "conflicts": [],
        "content_hashes": { &path: &digest }, "content_types": { &path: "audio/mpeg" },
        "components": [{ "id": "audio", "path": &path, "kind": "asset",
            "required_capabilities": ["media_assets"] }], "signature": null
    });
    let mut zip = ZipWriter::new(File::create(&source).unwrap());
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("manifest.json", options).unwrap();
    zip.write_all(&serde_json::to_vec(&manifest).unwrap()).unwrap();
    zip.start_file(path, options).unwrap();
    zip.write_all(&bytes).unwrap();
    zip.finish().unwrap();
    drop(bytes);
    let core = Core::open(CoreConfig::new(&data)).unwrap();
    assert!(core.inspect_content_package_import(&source).is_err());
    let inspection = core.inspect_content_package_import_with_limits(&source, ImportLimits {
        max_entry_bytes: 65 * 1024 * 1024,
        ..ImportLimits::default()
    }).unwrap();
    assert!(inspection.review.local_import_allowed);
    drop(core);
    fs::remove_file(&source).unwrap();

    let core = Core::open(CoreConfig::new(&data)).unwrap();
    let ids = vec!["audio".to_owned()];
    let selected = core.select_content_package_import(&inspection.import_id,
        &selection_request(&inspection, ids.clone())).unwrap();
    drop(core);
    let core = Core::open(CoreConfig::new(&data)).unwrap();
    let approval = core.approve_content_package_import(&inspection.import_id,
        &approval_request(&inspection, &selected, "large-approval", ids, Vec::new())).unwrap();
    drop(core);
    let core = Core::open(CoreConfig::new(&data)).unwrap();
    let result = core.commit_content_package_import(&inspection.import_id,
        &commit_request(&inspection, &selected, &approval)).unwrap();
    assert_eq!(result.import.status, PackageImportStatus::Completed);
    assert_eq!(result.asset_ids.len(), 1);
    assert!(core.list_pending_content_package_import_reviews(10).unwrap().is_empty());
}

#[test]
fn compatible_import_commit_failure_discards_the_inner_package_before_retry() {
    let root = tempdir().unwrap();
    let source = root.path().join("memory.json");
    let data = root.path().join("library");
    fs::write(&source, serde_json::to_vec(&json!({
        "type": "risu", "ver": 1, "data": { "name": "Memory", "settings": {
            "summarizationPrompt": "Extract facts. {{slot}}", "maxChatsPerSummary": 7
        }}
    })).unwrap()).unwrap();
    let core = Core::open(CoreConfig::new(&data)).unwrap();
    let inspection = core.inspect_import(&source).unwrap();
    let connection = Connection::open(active_test_database_path(&data)).unwrap();
    connection.execute_batch("CREATE TRIGGER package_commit_abort
        BEFORE INSERT ON package_import_audit_events WHEN NEW.event_kind = 'commit_completed'
        BEGIN SELECT RAISE(ABORT, 'synthetic commit failure'); END;").unwrap();
    core.commit_compatible_import(&inspection.id).expect_err("injected commit failure");
    assert!(core.list_pending_content_package_import_reviews(10).unwrap().is_empty());
    let imports = core.list_content_package_imports(None).unwrap();
    assert_eq!(imports.len(), 1);
    assert_eq!(imports[0].status, PackageImportStatus::Discarded);
    connection.execute_batch("DROP TRIGGER package_commit_abort;").unwrap();
    assert!(matches!(core.commit_compatible_import(&inspection.id).unwrap(),
        crate::ImportCommitResult::Content(_)));
    let imports = core.list_content_package_imports(None).unwrap();
    assert_eq!(imports.iter().filter(|i| i.status == PackageImportStatus::Completed).count(), 1);
}
