use std::{fs, io::Write};

use lorepia_core::{Core, CoreConfig};
use lorepia_domain::CoreErrorCode;
use tempfile::{NamedTempFile, tempdir};

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
