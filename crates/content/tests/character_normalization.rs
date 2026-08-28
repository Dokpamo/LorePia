use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use lorepia_content::{inspect_character_file, prepare_import};
use lorepia_domain::{ContentKind, ExtensionQuarantineKind, ImportLimits};
use tempfile::{TempDir, tempdir};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn path(&self) -> &Path {
        &self.path
    }
}

fn json_fixture(bytes: &[u8]) -> Fixture {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("card.json");
    std::fs::write(&path, bytes).expect("write card");
    Fixture {
        _directory: directory,
        path,
    }
}

#[test]
fn normalizes_all_public_character_fields_and_indexes_unknown_extensions() {
    let fixture = json_fixture(
        br#"{
            "spec":"chara_card_v3",
            "data":{
                "name":"Segu",
                "description":"A careful guide",
                "personality":"Warm and precise",
                "scenario":"A local library",
                "first_mes":"Welcome.",
                "mes_example":"User: Hello\nSegu: Welcome.",
                "system_prompt":"Remain in character.",
                "post_history_instructions":"Answer concisely.",
                "alternate_greetings":["Good morning.","Ready when you are."],
                "character_book":{"id":"book-segu","name":"Segu lore","entries":[]},
                "assets":[
                    {"type":"icon","uri":"https://invalid.example/avatar.png","name":"avatar"}
                ],
                "creator":"LorePia synthetic fixture",
                "z_unknown":{"safe":true},
                "extensions":{
                    "script":"alert(1)",
                    "html":"<iframe src='x'></iframe>",
                    "remote":"https://invalid.example/extension.json",
                    "inert":{"value":7}
                }
            }
        }"#,
    );
    let first =
        inspect_character_file(fixture.path(), ImportLimits::default()).expect("inspection");
    let second =
        inspect_character_file(fixture.path(), ImportLimits::default()).expect("inspection");
    let content = &first.character_content;

    assert_eq!(first.plan_hash, second.plan_hash);
    assert_eq!(content.personality, "Warm and precise");
    assert_eq!(content.scenario, "A local library");
    assert_eq!(content.first_message, "Welcome.");
    assert_eq!(content.example_dialogs, ["User: Hello\nSegu: Welcome."]);
    assert_eq!(content.system_instruction, "Remain in character.");
    assert_eq!(content.post_history_instruction, "Answer concisely.");
    assert_eq!(content.alternate_greetings.len(), 2);
    assert_eq!(
        content
            .knowledge_book
            .as_ref()
            .and_then(|book| book.name.as_deref()),
        Some("Segu lore")
    );
    assert_eq!(
        content
            .unknown_extensions
            .raw_source_sha256
            .as_ref()
            .expect("source hash")
            .as_str(),
        first.inspection.source_sha256
    );
    assert_eq!(
        first.inspection.unsupported_optional_fields,
        ["creator", "z_unknown"]
    );

    let quarantines = content
        .unknown_extensions
        .entries
        .iter()
        .filter_map(|entry| entry.quarantine.as_ref())
        .collect::<Vec<_>>();
    assert!(
        quarantines
            .iter()
            .any(|value| value.kind == ExtensionQuarantineKind::Script)
    );
    assert!(
        quarantines
            .iter()
            .any(|value| value.kind == ExtensionQuarantineKind::Html)
    );
    assert!(
        quarantines
            .iter()
            .any(|value| value.kind == ExtensionQuarantineKind::ExternalUrl)
    );
    assert!(quarantines.iter().all(|value| !value.active));
}

#[test]
fn charx_streams_two_thousand_assets_into_bounded_descriptors() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("large.charx");
    let file = File::create(&path).expect("create archive");
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
                    "name":"Large synthetic",
                    "description":"Asset stress fixture",
                    "personality":"Stable",
                    "scenario":"Test",
                    "first_mes":"Hello"
                }
            }"#,
        )
        .expect("write card");
    for index in 0..2_000_u32 {
        archive
            .start_file(format!("assets/expressions/{index:04}.png"), options)
            .expect("start asset");
        archive
            .write_all(b"\x89PNG\r\n\x1a\n")
            .expect("write asset signature");
        archive
            .write_all(&index.to_le_bytes())
            .expect("write unique asset data");
    }
    archive.finish().expect("finish archive");
    let fixture = Fixture {
        _directory: directory,
        path,
    };

    let plan =
        inspect_character_file(fixture.path(), ImportLimits::default()).expect("large inspection");
    assert_eq!(plan.inspection.asset_count, 2_000);
    assert_eq!(plan.character_content.assets.len(), 2_000);
    assert!(plan.character_content.assets.iter().all(|asset| {
        asset
            .source
            .logical_path
            .as_deref()
            .is_some_and(|path| path.starts_with("assets/expressions/") && !path.contains(".."))
    }));
    assert!(plan.character_content.assets.iter().all(|asset| {
        asset
            .source
            .source_sha256
            .as_ref()
            .is_some_and(|hash| hash.as_str() == plan.inspection.source_sha256)
    }));
}

#[test]
fn charx_omits_nonportable_archive_entries_from_normalized_content() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("quarantine.charx");
    let file = File::create(&path).expect("create archive");
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, bytes) in [
        (
            "card.json",
            br#"{"spec":"chara_card_v3","data":{"name":"Quarantine"}}"#.as_slice(),
        ),
        ("extensions/run.js", b"alert(1)".as_slice()),
        ("extensions/panel.html", b"<script>x()</script>".as_slice()),
        (
            "extensions/homepage.url",
            b"https://invalid.example".as_slice(),
        ),
        ("extensions/inert.json", br#"{"value":7}"#.as_slice()),
    ] {
        archive.start_file(name, options).expect("start entry");
        archive.write_all(bytes).expect("write entry");
    }
    archive.finish().expect("finish archive");
    let fixture = Fixture {
        _directory: directory,
        path,
    };

    let plan = inspect_character_file(fixture.path(), ImportLimits::default()).expect("inspection");
    let entries = &plan.character_content.unknown_extensions.entries;
    assert!(entries.is_empty());
    assert!(
        plan.inspection
            .warnings
            .iter()
            .any(|warning| warning.code == "nonportable_content_omitted")
    );
    let normalized = serde_json::to_string(&plan).expect("serialize normalized plan");
    assert!(!normalized.contains("extensions/run.js"));
    assert!(!normalized.contains("extensions/panel.html"));
    assert!(!normalized.contains("invalid.example"));
}

const WRAPPED_ASSET_COUNT: u32 = 1_411;
const WRAPPED_AUXILIARY_ENTRY_COUNT: u32 = 1_411;
const PRIVATE_MARKER: &str = "nonportable-private-marker";

fn large_card_fixture(wrap_in_image: bool) -> Fixture {
    let directory = tempdir().expect("temp directory");
    let archive_path = directory.path().join("payload.zip");
    let file = File::create(&archive_path).expect("create archive");
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);

    let mut declared_assets = Vec::with_capacity(WRAPPED_ASSET_COUNT as usize);
    declared_assets.push(serde_json::json!({
        "type": "icon",
        "name": "main",
        "uri": "embeded://assets/icon/main.png"
    }));
    for index in 0..WRAPPED_ASSET_COUNT - 1 {
        declared_assets.push(serde_json::json!({
            "type": "expression",
            "name": format!("expression-{index}"),
            "uri": format!("embeded://assets/expressions/{index:04}.png")
        }));
    }
    let card = serde_json::to_vec(&serde_json::json!({
        "spec": "chara_card_v3",
        "data": {
            "name": "Wrapped synthetic",
            "description": "Portable card fields",
            "first_mes": "Hello",
            "assets": declared_assets,
            "extensions": {
                "private_runtime": {
                    "marker": PRIVATE_MARKER,
                    "html": "<script>never()</script>"
                }
            }
        }
    }))
    .expect("serialize card");
    archive
        .start_file("card.json", options)
        .expect("start card");
    archive.write_all(&card).expect("write card");

    // Put another image before the declared main icon to prove that the card
    // descriptor, rather than archive order, selects the representative image.
    for index in 0..WRAPPED_ASSET_COUNT - 1 {
        archive
            .start_file(format!("assets/expressions/{index:04}.png"), options)
            .expect("start asset");
        archive
            .write_all(b"\x89PNG\r\n\x1a\n")
            .expect("write asset signature");
        archive
            .write_all(&index.to_le_bytes())
            .expect("write unique asset bytes");
    }
    archive
        .start_file("assets/icon/main.png", options)
        .expect("start main icon");
    archive
        .write_all(b"RIFF\x04\0\0\0WEBP")
        .expect("write main icon");

    for index in 0..WRAPPED_AUXILIARY_ENTRY_COUNT {
        archive
            .start_file(format!("metadata/{index:04}.json"), options)
            .expect("start auxiliary entry");
        archive
            .write_all(br#"{"ignored":true}"#)
            .expect("write auxiliary entry");
    }
    archive
        .start_file("private-runtime.bundle", options)
        .expect("start private runtime");
    archive
        .write_all(PRIVATE_MARKER.as_bytes())
        .expect("write private runtime");
    archive.finish().expect("finish archive");

    let final_path = if wrap_in_image {
        // Native pickers copy unknown/image extensions to an app-owned
        // `.pending` path, so detection must rely on bytes rather than suffix.
        let wrapped_path = directory.path().join("selected.pending");
        let archive_bytes = std::fs::read(&archive_path).expect("read archive");
        let mut wrapped = b"\xff\xd8\xff\xe0preview\xff\xd9".to_vec();
        wrapped.extend_from_slice(&archive_bytes);
        std::fs::write(&wrapped_path, wrapped).expect("write wrapped card");
        wrapped_path
    } else {
        archive_path
    };
    Fixture {
        _directory: directory,
        path: final_path,
    }
}

#[test]
fn image_wrapped_card_is_detected_and_only_portable_content_is_kept() {
    let fixture = large_card_fixture(true);

    let plan = inspect_character_file(fixture.path(), ImportLimits::default())
        .expect("wrapped card inspection");

    assert_eq!(plan.inspection.kind, ContentKind::CharxPackage);
    assert_eq!(plan.inspection.asset_count, WRAPPED_ASSET_COUNT);
    assert_eq!(
        plan.character_content.assets.len(),
        WRAPPED_ASSET_COUNT as usize
    );
    assert_eq!(
        plan.inspection
            .representative_image
            .as_ref()
            .map(|image| image.logical_asset_id.as_str()),
        Some("assets/icon/main.png")
    );
    assert_eq!(
        plan.inspection
            .representative_image
            .as_ref()
            .map(|image| image.media_type.as_str()),
        Some("image/webp")
    );
    assert!(plan.character_content.unknown_extensions.is_empty());
    assert!(
        plan.inspection
            .warnings
            .iter()
            .any(|warning| warning.code == "embedded_character_card")
    );
    assert!(
        plan.inspection
            .warnings
            .iter()
            .any(|warning| warning.code == "media_type_reclassified")
    );

    let normalized = serde_json::to_string(&plan).expect("serialize normalized plan");
    assert!(!normalized.contains(PRIVATE_MARKER));
    assert!(!normalized.contains("private_runtime"));
    assert!(!normalized.contains("private-runtime.bundle"));
    assert!(!normalized.contains("metadata/0000.json"));
}

#[test]
fn zip_card_uses_the_same_portable_import_policy() {
    let fixture = large_card_fixture(false);
    let plan = inspect_character_file(fixture.path(), ImportLimits::default())
        .expect("ZIP card inspection");

    assert_eq!(plan.inspection.kind, ContentKind::CharxPackage);
    assert_eq!(plan.inspection.asset_count, WRAPPED_ASSET_COUNT);
    assert!(plan.inspection.is_allowed());
    assert!(plan.character_content.unknown_extensions.is_empty());
    let normalized = serde_json::to_string(&plan).expect("serialize normalized plan");
    assert!(!normalized.contains(PRIVATE_MARKER));
    assert!(!normalized.contains("private-runtime.bundle"));
}

#[test]
fn image_wrapped_card_stages_only_verified_media() {
    let fixture = large_card_fixture(true);
    let staging = tempdir().expect("asset staging");
    let prepared = prepare_import(fixture.path(), ImportLimits::default(), staging.path())
        .expect("wrapped card preparation");

    assert!(prepared.inspection.is_allowed());
    assert_eq!(prepared.staged_assets.len(), WRAPPED_ASSET_COUNT as usize);
    assert!(
        prepared
            .staged_assets
            .iter()
            .all(|asset| asset.signature_valid)
    );
    assert!(prepared.staged_assets.iter().any(|asset| {
        asset.original_path == "assets/icon/main.png" && asset.media_type == "image/webp"
    }));
    assert!(
        prepared
            .staged_assets
            .iter()
            .all(|asset| !asset.original_path.starts_with("metadata/"))
    );
}

#[test]
fn external_full_runtime_card_is_normalized_when_fixture_is_available() {
    let Ok(path) = std::env::var("LOREPIA_FULL_CARD_FIXTURE") else {
        return;
    };
    let plan = inspect_character_file(Path::new(&path), ImportLimits::default())
        .expect("full runtime card inspection");
    let book = plan
        .character_content
        .knowledge_book
        .as_ref()
        .and_then(|reference| reference.embedded.as_ref())
        .expect("embedded knowledge book");
    assert_eq!(book.entries.len(), 76);
    assert_eq!(book.scan_depth, 5);
    assert_eq!(book.token_budget, 80_000);
    assert_eq!(plan.character_content.assets.len(), 1_411);
    assert_eq!(plan.character_content.runtime.transforms.len(), 34);
    assert_eq!(plan.character_content.runtime.scripts.len(), 1);
    assert!(plan.character_content.runtime.scripts[0].source.len() > 90_000);
    assert!(plan.character_content.runtime.background_markup.len() > 30_000);
    assert!(plan.character_content.runtime.transform_set_id.is_some());
    assert_eq!(
        plan.character_content
            .runtime
            .initial_variables
            .get("seoulzombie.optionmode")
            .map(String::as_str),
        Some("0")
    );
    assert_eq!(
        plan.character_content
            .runtime
            .initial_variables
            .get("seoulzombie.assetmax")
            .map(String::as_str),
        Some("")
    );
}
