use super::*;
use lorepia_domain::{AssetRole, AssetSource, AssetSourceKind, Character, CharacterContentV1};
use sha2::{Digest, Sha256};
use std::{io::Write, sync::Barrier};
use tempfile::{NamedTempFile, TempDir};

fn approved(bytes: &[u8]) -> (TempDir, Storage, Sha256Digest) {
    let root = tempfile::tempdir().unwrap();
    let mut source = NamedTempFile::new_in(root.path()).unwrap();
    source.write_all(b"synthetic character").unwrap();
    let source_hash = hex::encode(Sha256::digest(b"synthetic character"));
    let digest = Sha256Digest::parse(hex::encode(Sha256::digest(bytes))).unwrap();
    let mut asset = NamedTempFile::new_in(root.path()).unwrap();
    asset.write_all(bytes).unwrap();
    let descriptor = AssetDescriptor {
        id: AssetId::from("policy-image"),
        sha256: digest.clone(),
        media_type: "image/png".into(),
        role: AssetRole::Avatar,
        name: "image.png".into(),
        size_bytes: bytes.len() as u64,
        width: None,
        height: None,
        duration_ms: None,
        source: AssetSource {
            kind: AssetSourceKind::CharxPackage,
            source_sha256: None,
            logical_path: None,
        },
    };
    let mut content = CharacterContentV1::default();
    content.assets.push(descriptor);
    let storage = Storage::open(root.path()).unwrap();
    storage
        .commit_character_import_with_content(
            source.path(),
            &Character::new("Synthetic", "Guide", &source_hash),
            &content,
            &"ab".repeat(32),
            19,
            "policy-image-import",
            &[crate::StagedAssetImport {
                staged_path: asset.path().into(),
                sha256: digest.as_str().into(),
                media_type: "image/png".into(),
                size_bytes: bytes.len() as u64,
            }],
        )
        .unwrap();
    (root, storage, digest)
}

fn png() -> Vec<u8> {
    hex::decode("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415408d763f8cfc0f01f00050001ff729c52670000000049454e44ae426082").unwrap()
}

#[test]
fn concurrent_cold_png_ranges_share_hash_and_policy_then_detect_warm_mutation() {
    let bytes = png();
    let (root, storage, digest) = approved(&bytes);
    let barrier = Barrier::new(8);
    std::thread::scope(|scope| {
        for start in 0..8 {
            let (storage, digest, barrier, bytes) = (&storage, &digest, &barrier, &bytes);
            scope.spawn(move || {
                barrier.wait();
                let range = storage.read_approved_asset_range(digest, start, 3).unwrap();
                let index = usize::try_from(start).unwrap();
                assert_eq!(range.bytes, bytes[index..index + 3]);
                assert_eq!(
                    range.image_validation_policy,
                    ApprovedAssetRange::IMAGE_VALIDATION_POLICY
                );
            });
        }
    });
    assert_eq!(storage.approved_asset_hash_verification_count(), 1);
    let path = root
        .path()
        .join("assets")
        .join(content_relative_path(digest.as_str()).unwrap());
    let mut changed = bytes;
    changed[54] ^= 1;
    #[cfg(windows)]
    let storage = {
        // A live verified lease intentionally denies write/delete sharing.
        // Test that protection first, then validate a fresh open after tampering.
        let error = std::fs::write(&path, &changed).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(32)); // ERROR_SHARING_VIOLATION
        assert_eq!(
            storage
                .read_approved_asset_range(&digest, 0, 8)
                .unwrap()
                .bytes,
            png()[..8]
        );
        drop(storage);
        std::fs::write(&path, &changed).unwrap();
        Storage::open(root.path()).unwrap()
    };
    #[cfg(not(windows))]
    std::fs::write(path, changed).unwrap();
    assert_eq!(
        storage
            .read_approved_asset_range(&digest, 0, 8)
            .unwrap_err()
            .code,
        CoreErrorCode::StorageCorrupted
    );
}

#[test]
fn cold_crc_damage_outside_requested_range_is_rejected_after_valid_digest() {
    let mut bytes = png();
    bytes[54] ^= 1;
    // CAS and approved digest intentionally describe the corrupt container.
    let (_root, storage, digest) = approved(&bytes);
    assert_eq!(
        storage
            .read_approved_asset_range(&digest, 0, 8)
            .unwrap_err()
            .code,
        CoreErrorCode::UnsupportedContent
    );
}

#[test]
fn cold_png_full_range_reuses_scratch_and_partial_range_is_right_sized() {
    let bytes = png();
    let pointer = bytes.as_ptr();
    let length = bytes.len();
    let full = verified_png_range(bytes, 0, length as u64).unwrap();
    assert_eq!(full.as_ptr(), pointer);
    let partial = verified_png_range(full, 2, 3).unwrap();
    assert_eq!(partial, png()[2..5]);
    assert_eq!(partial.capacity(), 3);
    assert!(verified_png_range(vec![0; 3], u64::MAX, 1).is_err());
}
