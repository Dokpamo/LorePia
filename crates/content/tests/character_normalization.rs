use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
};

use lorepia_content::inspect_character_file;
use lorepia_domain::{ExtensionQuarantineKind, ImportLimits};
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
        ["creator", "extensions", "z_unknown"]
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
fn charx_preserves_code_html_and_url_entries_as_inactive_quarantine() {
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
    assert_eq!(entries.len(), 4);
    assert!(
        entries
            .iter()
            .all(|entry| entry.source_path.starts_with("/archive/"))
    );
    assert!(
        entries
            .iter()
            .filter_map(|entry| entry.quarantine.as_ref())
            .all(|quarantine| !quarantine.active)
    );
    assert!(entries.iter().any(|entry| entry.quarantine.is_none()));
    assert!(
        plan.inspection
            .warnings
            .iter()
            .any(|warning| warning.code == "quarantined_active_content")
    );
}
