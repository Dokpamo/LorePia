use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use lorepia_content::{PreparedImport, prepare_import};
use lorepia_core::{Character, Core, CoreConfig};
use lorepia_domain::{CoreErrorCode, ImportLimits};
use lorepia_storage::{StagedAssetImport, Storage};
use tempfile::tempdir;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

struct SyntheticCharx {
    path: PathBuf,
}

fn synthetic_charx(parent: &Path) -> SyntheticCharx {
    let path = parent.join("atomic-import.charx");
    let file = File::create(&path).expect("create CHARX fixture");
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
                    "name":"Atomic",
                    "description":"Synthetic",
                    "personality":"Careful",
                    "scenario":"A transaction test",
                    "first_mes":"Hello",
                    "alternate_greetings":["Welcome back."]
                }
            }"#,
        )
        .expect("write card");
    archive
        .start_file("assets/avatar.png", options)
        .expect("start asset");
    archive
        .write_all(b"\x89PNG\r\n\x1a\nsynthetic")
        .expect("write asset");
    archive.finish().expect("finish CHARX fixture");
    SyntheticCharx { path }
}

fn prepared_character(prepared: &PreparedImport) -> (Character, Vec<StagedAssetImport>) {
    let mut character = Character::new(
        &prepared.inspection.display_name,
        &prepared.inspection.description,
        &prepared.inspection.source_sha256,
    );
    character.avatar_asset_hash = prepared
        .staged_assets
        .iter()
        .find(|asset| asset.signature_valid && asset.media_type.starts_with("image/"))
        .map(|asset| asset.sha256.clone());
    let assets = prepared
        .staged_assets
        .iter()
        .map(|asset| StagedAssetImport {
            staged_path: asset.staged_path.clone(),
            sha256: asset.sha256.clone(),
            media_type: asset.media_type.clone(),
            size_bytes: asset.size_bytes,
        })
        .collect();
    (character, assets)
}

fn cas_path(root: &Path, family: &str, sha256: &str) -> PathBuf {
    root.join(family)
        .join("sha256")
        .join(&sha256[..2])
        .join(&sha256[2..])
}

fn staging_is_empty(root: &Path) -> bool {
    fs::read_dir(root.join("staging"))
        .expect("read staging")
        .next()
        .is_none()
}

#[test]
fn core_commits_ccv3_character_and_normalized_content_in_one_import() {
    let data_root = tempdir().expect("data root");
    let fixture_root = tempdir().expect("fixture root");
    let card_path = fixture_root.path().join("full-card.json");
    fs::write(
        &card_path,
        br#"{
            "spec":"chara_card_v3",
            "data":{
                "name":"Full card",
                "description":"Synthetic",
                "personality":"Careful",
                "scenario":"An atomic test",
                "first_mes":"Hello.",
                "mes_example":"User: Hi\nFull card: Hello.",
                "system_prompt":"Remain consistent.",
                "post_history_instructions":"Answer briefly.",
                "alternate_greetings":["Welcome back."],
                "character_book":{"id":"atomic-book","name":"Atomic lore"},
                "extensions":{"inert":{"value":7}}
            }
        }"#,
    )
    .expect("write card");
    let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");

    let inspection = core.inspect_import(&card_path).expect("inspect CCv3");
    let character = core.commit_import(&inspection.id).expect("commit CCv3");
    let core_content = core
        .get_character_content(&character.id)
        .expect("Core exposes committed normalized content");
    assert_eq!(core_content.revision, 1);
    assert_eq!(core_content.value.personality, "Careful");
    assert!(staging_is_empty(data_root.path()));
    drop(core);

    let storage = Storage::open(data_root.path()).expect("reopen storage");
    let stored_character = storage
        .get_character(&character.id)
        .expect("stored character");
    let stored_content = storage
        .get_character_content(&character.id)
        .expect("stored character content");
    assert_eq!(stored_character.name, "Full card");
    assert_eq!(stored_character.description, "Synthetic");
    assert_eq!(stored_character.source_hash, inspection.source_sha256);
    assert_eq!(stored_content.revision, 1);
    assert_eq!(stored_content.value.personality, "Careful");
    assert_eq!(stored_content.value.scenario, "An atomic test");
    assert_eq!(stored_content.value.first_message, "Hello.");
    assert_eq!(stored_content.value.example_dialogs.len(), 1);
    assert_eq!(
        stored_content.value.example_dialogs[0],
        "User: Hi\nFull card: Hello."
    );
    assert_eq!(
        stored_content.value.system_instruction,
        "Remain consistent."
    );
    assert_eq!(
        stored_content.value.post_history_instruction,
        "Answer briefly."
    );
    assert_eq!(stored_content.value.alternate_greetings, ["Welcome back."]);
    assert_eq!(
        stored_content
            .value
            .knowledge_book
            .as_ref()
            .and_then(|book| book.name.as_deref()),
        Some("Atomic lore")
    );
    assert_eq!(
        stored_content
            .value
            .unknown_extensions
            .raw_source_sha256
            .as_ref()
            .expect("source-bound extension index")
            .as_str(),
        inspection.source_sha256
    );
}

#[test]
fn core_commits_charx_character_content_asset_metadata_and_asset_cas_together() {
    let data_root = tempdir().expect("data root");
    let fixture_root = tempdir().expect("fixture root");
    let fixture = synthetic_charx(fixture_root.path());
    let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");

    let inspection = core.inspect_import(&fixture.path).expect("inspect CHARX");
    let character = core.commit_import(&inspection.id).expect("commit CHARX");
    let core_content = core
        .get_character_content(&character.id)
        .expect("Core exposes committed CHARX content");
    assert_eq!(core_content.value.assets.len(), 1);
    let avatar_hash = character
        .avatar_asset_hash
        .clone()
        .expect("avatar asset hash");
    let avatar_cas = cas_path(data_root.path(), "assets", &avatar_hash);
    assert!(avatar_cas.is_file());
    assert!(staging_is_empty(data_root.path()));
    drop(core);

    let storage = Storage::open(data_root.path()).expect("reopen storage");
    let stored_content = storage
        .get_character_content(&character.id)
        .expect("stored character content");
    assert_eq!(stored_content.value.assets.len(), 1);
    assert_eq!(stored_content.value.assets[0].sha256.as_str(), avatar_hash);
    assert_eq!(
        stored_content.value.assets[0]
            .source
            .logical_path
            .as_deref(),
        Some("assets/avatar.png")
    );
    assert_eq!(
        stored_content.value.assets[0]
            .source
            .source_sha256
            .as_ref()
            .expect("asset source hash")
            .as_str(),
        inspection.source_sha256
    );
    assert!(avatar_cas.is_file());
}

#[test]
fn core_rejects_a_tampered_owned_snapshot_without_partial_character_rows() {
    let data_root = tempdir().expect("data root");
    let fixture_root = tempdir().expect("fixture root");
    let card_path = fixture_root.path().join("tamper-card.json");
    fs::write(
        &card_path,
        br#"{
            "spec":"chara_card_v3",
            "data":{"name":"Tamper","personality":"Original"}
        }"#,
    )
    .expect("write card");
    let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
    let inspection = core.inspect_import(&card_path).expect("inspect card");
    let staged_paths = fs::read_dir(data_root.path().join("staging"))
        .expect("read staging")
        .map(|entry| entry.expect("staging entry").path())
        .collect::<Vec<_>>();
    assert_eq!(
        staged_paths.len(),
        1,
        "JSON import owns one source snapshot"
    );
    fs::write(&staged_paths[0], b"tampered owned snapshot").expect("tamper snapshot");

    let error = core
        .commit_import(&inspection.id)
        .expect_err("tampered snapshot must fail");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
    assert!(core.list_characters().expect("characters").is_empty());
    core.discard_import(&inspection.id)
        .expect("discard restored failed claim");
    assert!(staging_is_empty(data_root.path()));
    drop(core);

    let recovered = Storage::open(data_root.path()).expect("reopen storage");
    let stats = recovered.stats().expect("stats");
    assert_eq!(stats.characters, 0);
    assert_eq!(stats.pending_imports, 0);
}

#[test]
fn tampered_source_is_rejected_and_recovery_removes_every_partial_import_artifact() {
    let data_root = tempdir().expect("data root");
    let storage = Storage::open(data_root.path()).expect("open storage");
    let fixture = synthetic_charx(&storage.staging_dir());
    let prepared = prepare_import(
        &fixture.path,
        ImportLimits::default(),
        &storage.staging_dir(),
    )
    .expect("prepare import");
    let (character, assets) = prepared_character(&prepared);
    let source_cas = cas_path(
        data_root.path(),
        "sources",
        &prepared.inspection.source_sha256,
    );
    let asset_cas = cas_path(data_root.path(), "assets", &assets[0].sha256);

    fs::write(&fixture.path, b"tampered after review").expect("tamper staged source");
    let error = storage
        .commit_character_import_with_content(
            &fixture.path,
            &character,
            &prepared.character_content,
            &prepared.plan_hash,
            prepared.inspection.source_size,
            &prepared.inspection.id.0,
            &assets,
        )
        .expect_err("tampered source must fail");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
    assert_eq!(storage.stats().expect("stats").characters, 0);
    assert_eq!(storage.stats().expect("stats").pending_imports, 1);
    assert_eq!(
        storage
            .get_character(&character.id)
            .expect_err("character transaction must roll back")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        storage
            .get_character_content(&character.id)
            .expect_err("content transaction must roll back")
            .code,
        CoreErrorCode::NotFound
    );

    drop(storage);
    let recovered = Storage::open(data_root.path()).expect("recover storage");
    let stats = recovered.stats().expect("recovered stats");
    assert_eq!(stats.characters, 0);
    assert_eq!(stats.pending_imports, 0);
    assert!(!source_cas.exists());
    assert!(!asset_cas.exists());
    assert!(staging_is_empty(data_root.path()));
}

#[test]
fn invalid_plan_hash_rolls_back_character_content_and_recovery_removes_durable_cas_orphans() {
    let data_root = tempdir().expect("data root");
    let storage = Storage::open(data_root.path()).expect("open storage");
    let fixture = synthetic_charx(&storage.staging_dir());
    let prepared = prepare_import(
        &fixture.path,
        ImportLimits::default(),
        &storage.staging_dir(),
    )
    .expect("prepare import");
    let (character, assets) = prepared_character(&prepared);
    let source_cas = cas_path(
        data_root.path(),
        "sources",
        &prepared.inspection.source_sha256,
    );
    let asset_cas = cas_path(data_root.path(), "assets", &assets[0].sha256);

    let error = storage
        .commit_character_import_with_content(
            &fixture.path,
            &character,
            &prepared.character_content,
            "tampered-plan-hash",
            prepared.inspection.source_size,
            &prepared.inspection.id.0,
            &assets,
        )
        .expect_err("invalid plan hash must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        source_cas.is_file(),
        "source CAS is durable before DB commit"
    );
    assert!(asset_cas.is_file(), "asset CAS is durable before DB commit");
    let stats = storage.stats().expect("stats after rollback");
    assert_eq!(stats.characters, 0);
    assert_eq!(stats.pending_imports, 1);
    assert_eq!(
        storage
            .get_character(&character.id)
            .expect_err("rolled-back character must not be visible")
            .code,
        CoreErrorCode::NotFound
    );
    assert_eq!(
        storage
            .get_character_content(&character.id)
            .expect_err("rolled-back content must not be visible")
            .code,
        CoreErrorCode::NotFound
    );

    drop(storage);
    let recovered = Storage::open(data_root.path()).expect("recover storage");
    let stats = recovered.stats().expect("recovered stats");
    assert_eq!(stats.characters, 0);
    assert_eq!(stats.pending_imports, 0);
    assert!(!source_cas.exists());
    assert!(!asset_cas.exists());
    assert!(staging_is_empty(data_root.path()));
}
