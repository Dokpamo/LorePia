#[test]
fn import_commit_observer_proves_cas_durability_precedes_sqlite_commit() {
    let root = tempdir().expect("temp root");
    let mut source = NamedTempFile::new_in(root.path()).expect("source staging");
    source.write_all(b"character").expect("source");
    let mut asset = NamedTempFile::new_in(root.path()).expect("asset staging");
    asset.write_all(b"avatar").expect("asset");
    let source_hash = hex::encode(Sha256::digest(b"character"));
    let asset_hash = hex::encode(Sha256::digest(b"avatar"));
    let mut character = Character::new("Segu", "Guide", &source_hash);
    character.avatar_asset_hash = Some(asset_hash.clone());
    let staged_assets = vec![StagedAssetImport {
        staged_path: asset.path().to_path_buf(),
        sha256: asset_hash.clone(),
        media_type: "image/png".to_owned(),
        size_bytes: 6,
    }];
    let storage = Storage::open(root.path()).expect("open storage");
    let source_cas = root
        .path()
        .join("sources")
        .join(content_relative_path(&source_hash).expect("source path"));
    let asset_cas = root
        .path()
        .join("assets")
        .join(content_relative_path(&asset_hash).expect("asset path"));
    let mut phases = Vec::new();

    storage
        .commit_character_import_observed(
            source.path(),
            &character,
            9,
            "observed-import",
            &staged_assets,
            |phase| {
                let stats = storage.stats().expect("stats at phase");
                match phase {
                    ImportCommitPhase::JournalCreated
                    | ImportCommitPhase::JournalMarkedFileStored => {
                        assert_eq!(stats.pending_imports, 1);
                        assert_eq!(stats.characters, 0);
                    }
                    ImportCommitPhase::CasFilesDurable => {
                        assert!(source_cas.is_file());
                        assert!(asset_cas.is_file());
                        assert_eq!(stats.pending_imports, 1);
                        assert_eq!(stats.characters, 0);
                    }
                    ImportCommitPhase::RecordsCommitted => {
                        assert_eq!(stats.pending_imports, 0);
                        assert_eq!(stats.characters, 1);
                    }
                }
                phases.push(phase);
            },
        )
        .expect("observed import");

    assert_eq!(
        phases,
        vec![
            ImportCommitPhase::JournalCreated,
            ImportCommitPhase::CasFilesDurable,
            ImportCommitPhase::JournalMarkedFileStored,
            ImportCommitPhase::RecordsCommitted,
        ]
    );
}

#[test]
fn approved_asset_delivery_revalidates_descriptor_hash_signature_and_range() {
    let root = tempdir().expect("temp root");
    let source_bytes = b"synthetic character";
    let image_bytes = b"\x89PNG\r\n\x1a\nsynthetic-image";
    let mut source = NamedTempFile::new_in(root.path()).expect("source staging");
    source.write_all(source_bytes).expect("source");
    let mut asset = NamedTempFile::new_in(root.path()).expect("asset staging");
    asset.write_all(image_bytes).expect("asset");
    let source_hash = hex::encode(Sha256::digest(source_bytes));
    let asset_hash = hex::encode(Sha256::digest(image_bytes));
    let asset_digest = Sha256Digest::parse(&asset_hash).expect("asset digest");
    let descriptor = AssetDescriptor {
        id: AssetId::from("avatar"),
        sha256: asset_digest.clone(),
        media_type: "image/png".to_owned(),
        role: lorepia_domain::AssetRole::Avatar,
        name: "avatar.png".to_owned(),
        size_bytes: u64::try_from(image_bytes.len()).expect("small image"),
        width: Some(1),
        height: Some(1),
        duration_ms: None,
        source: lorepia_domain::AssetSource {
            kind: lorepia_domain::AssetSourceKind::CharxPackage,
            source_sha256: Some(Sha256Digest::parse(&source_hash).expect("source digest")),
            logical_path: Some("assets/avatar.png".to_owned()),
        },
    };
    let mut content = CharacterContentV1::default();
    content.assets.push(descriptor.clone());
    let mut character = Character::new("Segu", "Guide", &source_hash);
    character.avatar_asset_hash = Some(asset_hash.clone());
    let staged_assets = [StagedAssetImport {
        staged_path: asset.path().to_path_buf(),
        sha256: asset_hash.clone(),
        media_type: "image/png".to_owned(),
        size_bytes: descriptor.size_bytes,
    }];
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .commit_character_import_with_content(
            source.path(),
            &character,
            &content,
            &"ab".repeat(32),
            u64::try_from(source_bytes.len()).expect("small source"),
            "approved-asset-import",
            &staged_assets,
        )
        .expect("commit approved asset");

    assert_eq!(
        storage
            .resolve_approved_asset_by_id(&descriptor.id)
            .expect("resolve by id"),
        descriptor
    );
    let hash_verifications = storage.approved_asset_hash_verification_count();
    assert_eq!(hash_verifications, 1);
    assert_eq!(
        storage
            .resolve_approved_asset_by_sha256(&asset_digest)
            .expect("resolve by digest"),
        descriptor
    );
    let range = storage
        .read_approved_asset_range(&asset_digest, 1, 4)
        .expect("read exact range");
    assert_eq!(range.start, 1);
    assert_eq!(range.bytes, image_bytes[1..5]);
    let second_range = storage
        .read_approved_asset_range(&asset_digest, 8, 5)
        .expect("read second range");
    assert_eq!(second_range.start, 8);
    assert_eq!(second_range.bytes, image_bytes[8..13]);
    assert_eq!(
        storage.approved_asset_hash_verification_count(),
        hash_verifications,
        "repeated ranges must reuse the verified handle"
    );

    let cas_path = root
        .path()
        .join("assets")
        .join(content_relative_path(&asset_hash).expect("asset path"));
    #[cfg(windows)]
    {
        // Windows deliberately opens verified assets without write sharing,
        // so the live cache lease must be closed before an external mutation.
        drop(storage);
        fs::write(&cas_path, b"\x89PNG\r\n\x1a\nchanged").expect("tamper CAS");
        let reopened = Storage::open(root.path()).expect("reopen tampered storage");
        let error = reopened
            .resolve_approved_asset_by_sha256(&asset_digest)
            .expect_err("tampered CAS must fail closed after reopen");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }
    #[cfg(not(windows))]
    {
        fs::write(&cas_path, b"\x89PNG\r\n\x1a\nchanged").expect("tamper CAS");
        let error = storage
            .resolve_approved_asset_by_sha256(&asset_digest)
            .expect_err("tampered CAS must fail closed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }
}

#[test]
fn approved_asset_delivery_rejects_non_renderer_media() {
    let root = tempdir().expect("temp root");
    let source_bytes = b"synthetic package";
    let attachment_bytes = b"%PDF-synthetic";
    let mut source = NamedTempFile::new_in(root.path()).expect("source staging");
    source.write_all(source_bytes).expect("source");
    let mut asset = NamedTempFile::new_in(root.path()).expect("asset staging");
    asset.write_all(attachment_bytes).expect("asset");
    let source_hash = hex::encode(Sha256::digest(source_bytes));
    let asset_hash = hex::encode(Sha256::digest(attachment_bytes));
    let descriptor = AssetDescriptor {
        id: AssetId::from("attachment"),
        sha256: Sha256Digest::parse(&asset_hash).expect("asset digest"),
        media_type: "application/pdf".to_owned(),
        role: lorepia_domain::AssetRole::Attachment,
        name: "attachment.pdf".to_owned(),
        size_bytes: u64::try_from(attachment_bytes.len()).expect("small attachment"),
        width: None,
        height: None,
        duration_ms: None,
        source: lorepia_domain::AssetSource {
            kind: lorepia_domain::AssetSourceKind::CharxPackage,
            source_sha256: Some(Sha256Digest::parse(&source_hash).expect("source digest")),
            logical_path: Some("assets/attachment.pdf".to_owned()),
        },
    };
    let mut content = CharacterContentV1::default();
    content.assets.push(descriptor.clone());
    let character = Character::new("Segu", "Guide", &source_hash);
    let staged_assets = [StagedAssetImport {
        staged_path: asset.path().to_path_buf(),
        sha256: asset_hash,
        media_type: descriptor.media_type.clone(),
        size_bytes: descriptor.size_bytes,
    }];
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .commit_character_import_with_content(
            source.path(),
            &character,
            &content,
            &"cd".repeat(32),
            u64::try_from(source_bytes.len()).expect("small source"),
            "attachment-import",
            &staged_assets,
        )
        .expect("commit inert attachment");

    let error = storage
        .resolve_approved_asset_by_id(&descriptor.id)
        .expect_err("attachments must not reach the renderer protocol");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
}
