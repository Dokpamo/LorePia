use std::{
    fs::{self, File},
    io::Write,
};

use lorepia_core::{Core, CoreConfig};
use lorepia_domain::CoreErrorCode;
use sha2::{Digest, Sha256};
use tempfile::{NamedTempFile, tempdir};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

#[test]
fn inspect_review_commit_and_restart_uses_the_reviewed_snapshot() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut source = NamedTempFile::new().expect("temporary source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"세구","description":"새 캐릭터"}}}}"#
    )
    .expect("write source");

    let review = core.inspect_import(source.path()).expect("inspect source");
    assert!(review.is_allowed());
    assert_eq!(review.display_name, "세구");
    assert_eq!(review.source_size, review.estimated_stored_size);

    fs::write(source.path(), b"untrusted mutation").expect("mutate picker source");
    let character = core
        .commit_import(&review.id)
        .expect("commit reviewed snapshot");
    assert_eq!(character.name, "세구");
    assert_eq!(core.list_characters().expect("library").len(), 1);

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    let restored = reopened
        .get_character(&character.id)
        .expect("restored character");
    assert_eq!(restored.source_hash, review.source_sha256);
}

#[test]
fn cancelled_review_cannot_be_committed() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut source = NamedTempFile::new().expect("temporary source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{"name":"Discard","description":""}}}}"#
    )
    .expect("write source");

    let review = core.inspect_import(source.path()).expect("inspect source");
    core.discard_import(&review.id).expect("discard review");
    assert!(core.commit_import(&review.id).is_err());
    assert!(core.list_characters().expect("library").is_empty());
}

#[test]
fn charx_assets_are_content_addressed_and_the_avatar_survives_restart() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let package = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/packages/with-avatar.charx");

    let review = core.inspect_import(&package).expect("inspect package");
    assert_eq!(review.asset_count, 1);
    let representative_image = review
        .representative_image
        .as_ref()
        .expect("representative image metadata");
    assert_eq!(representative_image.logical_asset_id, "assets/avatar.png");
    assert_eq!(representative_image.media_type, "image/png");
    assert_eq!(representative_image.size_bytes, 70);
    let character = core.commit_import(&review.id).expect("commit package");
    let avatar_hash = character.avatar_asset_hash.expect("avatar hash");
    assert_eq!(
        avatar_hash, "aa7bb0431aaeb198a77c26a14fe6dd714a75e4d7db94e3e1238a1fdcbfe1f8d4",
        "the committed avatar must be the image described by Import Review"
    );
    let avatar_path = root
        .path()
        .join("assets/sha256")
        .join(&avatar_hash[..2])
        .join(&avatar_hash[2..]);
    assert!(
        avatar_path.is_file(),
        "avatar must be stored in the asset CAS"
    );
    assert!(
        fs::read_dir(root.path().join("staging"))
            .expect("staging directory")
            .next()
            .is_none(),
        "source and extracted asset staging files must be removed"
    );

    drop(core);
    let reopened = Core::open(CoreConfig::new(root.path())).expect("reopen core");
    assert_eq!(
        reopened
            .get_character(&character.id)
            .expect("restored character")
            .avatar_asset_hash
            .as_deref(),
        Some(avatar_hash.as_str())
    );
    assert!(avatar_path.is_file());
}

#[test]
fn commit_uses_the_card_declared_main_icon_instead_of_archive_order() {
    let root = tempdir().expect("temporary data root");
    let source_dir = tempdir().expect("temporary source directory");
    let package = source_dir.path().join("declared-main.charx");
    let file = File::create(&package).expect("create package");
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    archive
        .start_file("card.json", options)
        .expect("start card");
    archive
        .write_all(
            br#"{
                "spec":"chara_card_v3",
                "data":{
                    "name":"Declared main",
                    "assets":[
                        {"type":"expression","name":"first","uri":"embeded://assets/expressions/first.png"},
                        {"type":"expression","name":"duplicate","uri":"embeded://assets/expressions/duplicate.png"},
                        {"type":"icon","name":"main","uri":"embeded://assets/icon/main.png"}
                    ]
                }
            }"#,
        )
        .expect("write card");
    let first_image = b"\x89PNG\r\n\x1a\nfirst";
    let main_image = b"\x89PNG\r\n\x1a\nmain";
    archive
        .start_file("assets/expressions/first.png", options)
        .expect("start first image");
    archive.write_all(first_image).expect("write first image");
    archive
        .start_file("assets/icon/main.png", options)
        .expect("start main image");
    archive.write_all(main_image).expect("write main image");
    archive
        .start_file("assets/expressions/duplicate.png", options)
        .expect("start duplicate image");
    archive
        .write_all(first_image)
        .expect("write duplicate image");
    archive.finish().expect("finish package");

    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let review = core.inspect_import(&package).expect("inspect package");
    assert_eq!(
        review
            .representative_image
            .as_ref()
            .map(|image| image.logical_asset_id.as_str()),
        Some("assets/icon/main.png")
    );

    let character = core.commit_import(&review.id).expect("commit package");
    let content = core
        .get_character_content(&character.id)
        .expect("load character content");
    assert_eq!(content.value.assets.len(), 3);
    let first_image_hash = format!("{:x}", Sha256::digest(first_image));
    let duplicate_hash_assets = content
        .value
        .assets
        .iter()
        .filter(|asset| asset.sha256.as_str() == first_image_hash.as_str())
        .collect::<Vec<_>>();
    assert_eq!(duplicate_hash_assets.len(), 2);
    assert_ne!(duplicate_hash_assets[0].id, duplicate_hash_assets[1].id);
    assert_eq!(
        character.avatar_asset_hash.as_deref(),
        Some(format!("{:x}", Sha256::digest(main_image)).as_str())
    );
}

#[test]
fn external_full_runtime_card_commits_executable_knowledge_and_output_rules() {
    let Ok(path) = std::env::var("LOREPIA_FULL_CARD_FIXTURE") else {
        return;
    };
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let review = core
        .inspect_import(&path)
        .expect("inspect full runtime card");
    assert!(
        review.is_allowed(),
        "review blockers: {:?}",
        review.blocked_reasons
    );
    let character = core
        .commit_import(&review.id)
        .expect("commit full runtime card");
    let content = core
        .get_character_content(&character.id)
        .expect("load full character content");
    assert_eq!(content.value.assets.len(), 1_411);
    assert_eq!(content.value.runtime.transforms.len(), 34);
    assert_eq!(content.value.runtime.scripts.len(), 1);
    assert!(content.value.runtime.background_markup.len() > 30_000);
    let book_id = content
        .value
        .knowledge_book
        .as_ref()
        .and_then(|reference| reference.id.as_ref())
        .expect("knowledge book id");
    let book = core
        .get_knowledge_book(book_id)
        .expect("load executable knowledge book");
    assert_eq!(book.value.entries.len(), 68);
    assert_eq!(book.value.scan_depth, 5);
    assert_eq!(book.value.token_budget.max_tokens, 80_000);
    let expanded_profile = book
        .value
        .entries
        .iter()
        .find(|entry| entry.name == "이도윤 프로필")
        .expect("profile with named knowledge slots");
    assert!(
        expanded_profile
            .content
            .contains("childhood friends and lovers")
    );
    assert!(!expanded_profile.content.contains("{{position::"));

    let transform_set_id = content
        .value
        .runtime
        .transform_set_id
        .as_ref()
        .expect("output transform set id");
    let transforms = core
        .get_transform_set(transform_set_id)
        .expect("load executable output transforms");
    assert_eq!(
        transforms
            .value
            .rules
            .iter()
            .filter(|rule| rule.phase == lorepia_domain::TransformPhase::ProviderOutputCanonical)
            .count(),
        17
    );
    assert!(
        transforms
            .value
            .rules
            .iter()
            .any(|rule| rule.phase == lorepia_domain::TransformPhase::DisplayOnly)
    );
    assert!(transforms.value.enabled);
}

#[test]
fn import_review_reports_only_unknown_fields_and_persists_normalized_content() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let mut source = NamedTempFile::new().expect("temporary source");
    write!(
        source,
        r#"{{"spec":"chara_card_v3","data":{{
            "name":"Optional",
            "personality":"Consumed fallback",
            "description":null,
            "scenario":"Persisted scenario",
            "creator":"Synthetic"
        }}}}"#
    )
    .expect("write source");

    let review = core.inspect_import(source.path()).expect("inspect source");
    assert_eq!(review.description, "Consumed fallback");
    assert_eq!(review.unsupported_optional_fields, ["creator"]);
    assert!(review.representative_image.is_none());
    let character = core.commit_import(&review.id).expect("commit");
    assert_eq!(character.description, "Consumed fallback");
    let content = core
        .get_character_content(&character.id)
        .expect("load normalized character content");
    assert_eq!(content.value.personality, "Consumed fallback");
    assert_eq!(content.value.scenario, "Persisted scenario");
}

#[test]
fn mime_mismatch_returns_a_blocked_review_and_never_reaches_the_library() {
    let root = tempdir().expect("temporary data root");
    let core = Core::open(CoreConfig::new(root.path())).expect("open core");
    let package = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/archives/mime-mismatch.zip");

    let review = core.inspect_import(&package).expect("inspect package");
    assert!(!review.is_allowed());
    assert_eq!(review.asset_count, 1);
    assert!(
        review
            .warnings
            .iter()
            .any(|warning| warning.code == "mime_mismatch")
    );
    assert!(
        review
            .blocked_reasons
            .iter()
            .any(|reason| reason.contains("file signature"))
    );
    assert_eq!(
        fs::read_dir(root.path().join("staging"))
            .expect("staging directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains("-asset-"))
            .count(),
        0,
        "blocked packages must not extract assets"
    );

    let error = core
        .commit_import(&review.id)
        .expect_err("blocked review must not commit");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
    assert!(core.list_characters().expect("library").is_empty());

    core.discard_import(&review.id)
        .expect("discard blocked review");
    assert!(
        fs::read_dir(root.path().join("staging"))
            .expect("staging directory")
            .next()
            .is_none(),
        "discard must remove the owned source snapshot"
    );
}
