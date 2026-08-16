use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

use lorepia_content::inspect_file;
use lorepia_domain::{ContentKind, CoreErrorCode, ImportLimits};
use tempfile::{TempDir, tempdir};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const VALID_CARD: &[u8] =
    br#"{"spec":"chara_card_v3","data":{"name":"Synthetic","description":"Test"}}"#;

struct SyntheticFixture {
    _directory: TempDir,
    path: PathBuf,
}

impl SyntheticFixture {
    fn path(&self) -> &Path {
        &self.path
    }
}

fn fixture(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata")
        .join(relative)
}

fn synthetic_file(name: &str, bytes: &[u8]) -> SyntheticFixture {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join(name);
    fs::write(&path, bytes).expect("write synthetic fixture");
    SyntheticFixture {
        _directory: directory,
        path,
    }
}

fn synthetic_archive(entries: Vec<(String, Vec<u8>)>) -> SyntheticFixture {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("fixture.charx");
    let file = File::create(&path).expect("create synthetic archive");
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(0o644);
    for (name, bytes) in entries {
        archive.start_file(name, options).expect("start ZIP entry");
        archive.write_all(&bytes).expect("write ZIP entry");
    }
    archive.finish().expect("finish synthetic archive");
    SyntheticFixture {
        _directory: directory,
        path,
    }
}

fn card_entry() -> (String, Vec<u8>) {
    ("card.json".to_owned(), VALID_CARD.to_vec())
}

#[test]
fn accepts_project_owned_valid_fixtures() {
    let json = inspect_file(&fixture("cards/minimal-v3.json"), ImportLimits::default())
        .expect("minimal JSON");
    assert_eq!(json.kind, ContentKind::CharacterCardV3);
    assert!(json.representative_image.is_none());
    assert!(json.unsupported_optional_fields.is_empty());

    let charx = inspect_file(&fixture("packages/minimal.charx"), ImportLimits::default())
        .expect("minimal CHARX");
    assert_eq!(charx.kind, ContentKind::CharxPackage);
}

#[test]
fn reports_the_commit_avatar_candidate_without_exposing_staging() {
    let inspection = inspect_file(
        &fixture("packages/with-avatar.charx"),
        ImportLimits::default(),
    )
    .expect("avatar package");
    let image = inspection
        .representative_image
        .expect("representative image metadata");

    assert_eq!(image.logical_asset_id, "assets/avatar.png");
    assert_eq!(image.media_type, "image/png");
    assert_eq!(image.size_bytes, 70);
    assert!(!image.logical_asset_id.starts_with('/'));
    assert!(!image.logical_asset_id.contains(".."));
}

#[test]
fn reports_only_unconsumed_ccv3_data_fields_in_stable_order() {
    let card = synthetic_file(
        "optional-fields.json",
        br#"{
            "spec":"chara_card_v3",
            "data":{
                "name":"Synthetic",
                "description":"Consumed",
                "z_unknown":true,
                "personality":"Not selected",
                "creator":"Test",
                "alternate_greetings":[]
            }
        }"#,
    );
    let inspection =
        inspect_file(card.path(), ImportLimits::default()).expect("optional fields review");

    assert_eq!(
        inspection.unsupported_optional_fields,
        ["creator", "z_unknown"]
    );
    assert!(inspection.representative_image.is_none());
}

#[test]
fn blocks_unsafe_archive_paths_and_collisions() {
    for relative in [
        "archives/traversal.zip",
        "archives/absolute-path.zip",
        "archives/case-collision.zip",
        "archives/unicode-collision.zip",
        "archives/high-ratio.zip",
    ] {
        let error = inspect_file(&fixture(relative), ImportLimits::default())
            .expect_err("unsafe archive must be blocked");
        assert_eq!(error.code, CoreErrorCode::UnsafeArchive, "{relative}");
    }
}

#[test]
fn reports_asset_mime_mismatch() {
    let inspection = inspect_file(
        &fixture("archives/mime-mismatch.zip"),
        ImportLimits::default(),
    )
    .expect("package remains available for a blocked review");
    assert!(
        inspection
            .warnings
            .iter()
            .any(|warning| warning.code == "mime_mismatch")
    );
    assert!(!inspection.is_allowed());
    assert!(
        inspection
            .blocked_reasons
            .iter()
            .any(|reason| reason.contains("file signature"))
    );
}

#[test]
fn blocks_symbolic_links_duplicate_entries_and_too_many_entries() {
    let symlink = synthetic_archive(vec![("link".to_owned(), b"card.json".to_vec())]);
    mark_first_entry_as_symlink(symlink.path());
    let error = inspect_file(symlink.path(), ImportLimits::default())
        .expect_err("symbolic link entry must be rejected");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);

    let duplicate = synthetic_archive(vec![
        card_entry(),
        ("copy.json".to_owned(), VALID_CARD.to_vec()),
    ]);
    replace_archive_name(duplicate.path(), b"copy.json", b"card.json");
    let error = inspect_file(duplicate.path(), ImportLimits::default())
        .expect_err("duplicate entry must be rejected");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);

    let too_many = synthetic_archive(vec![
        card_entry(),
        ("assets/a.png".to_owned(), b"\x89PNG\r\n\x1a\n".to_vec()),
        ("assets/b.png".to_owned(), b"\x89PNG\r\n\x1a\n".to_vec()),
    ]);
    let limits = ImportLimits {
        max_entries: 2,
        ..ImportLimits::default()
    };
    let error = inspect_file(too_many.path(), limits).expect_err("entry count must be bounded");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
}

#[test]
fn rejects_corrupt_metadata_missing_canonical_metadata_and_empty_inputs() {
    let corrupt_json = synthetic_file("corrupt.json", br#"{"spec":"chara_card_v3","data":"#);
    let error = inspect_file(corrupt_json.path(), ImportLimits::default())
        .expect_err("corrupt JSON must be rejected");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);

    let corrupt_charx = synthetic_archive(vec![("card.json".to_owned(), b"{not-json".to_vec())]);
    let error = inspect_file(corrupt_charx.path(), ImportLimits::default())
        .expect_err("corrupt CHARX metadata must be rejected");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);

    let misplaced_metadata =
        synthetic_archive(vec![("metadata.json".to_owned(), VALID_CARD.to_vec())]);
    let error = inspect_file(misplaced_metadata.path(), ImportLimits::default())
        .expect_err("CHARX requires root card.json");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);

    let empty_file = synthetic_file("empty.json", b"");
    let error = inspect_file(empty_file.path(), ImportLimits::default())
        .expect_err("empty source must be rejected");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);

    let empty_archive = synthetic_archive(Vec::new());
    let error = inspect_file(empty_archive.path(), ImportLimits::default())
        .expect_err("empty CHARX must be rejected");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
}

#[test]
fn accepts_exact_source_entry_total_and_count_boundaries() {
    let mut boundary_card = VALID_CARD.to_vec();
    boundary_card.extend(std::iter::repeat_n(b' ', 64));
    let json = synthetic_file("boundary.json", &boundary_card);
    let source_limits = ImportLimits {
        max_source_bytes: boundary_card.len() as u64,
        ..ImportLimits::default()
    };
    let inspection = inspect_file(json.path(), source_limits).expect("exact source boundary");
    assert_eq!(inspection.source_size, boundary_card.len() as u64);

    let archive = synthetic_archive(vec![
        ("card.json".to_owned(), boundary_card.clone()),
        ("assets/a.png".to_owned(), b"\x89PNG\r\n\x1a\n".to_vec()),
    ]);
    let total_size = boundary_card.len() as u64 + 8;
    let archive_limits = ImportLimits {
        max_entries: 2,
        max_entry_bytes: boundary_card.len() as u64,
        max_total_uncompressed_bytes: total_size,
        ..ImportLimits::default()
    };
    let inspection = inspect_file(archive.path(), archive_limits)
        .expect("entry, total, and count boundaries are inclusive");
    assert_eq!(inspection.estimated_stored_size, total_size);
    assert_eq!(inspection.asset_count, 1);
}

#[test]
fn rejects_one_byte_beyond_each_size_boundary() {
    let json = synthetic_file("source.json", VALID_CARD);
    let source_limits = ImportLimits {
        max_source_bytes: VALID_CARD.len() as u64 - 1,
        ..ImportLimits::default()
    };
    let error = inspect_file(json.path(), source_limits).expect_err("source is too large");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);

    let archive = synthetic_archive(vec![card_entry()]);
    let entry_limits = ImportLimits {
        max_entry_bytes: VALID_CARD.len() as u64 - 1,
        ..ImportLimits::default()
    };
    let error = inspect_file(archive.path(), entry_limits).expect_err("entry is too large");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);

    let total_limits = ImportLimits {
        max_total_uncompressed_bytes: VALID_CARD.len() as u64 - 1,
        ..ImportLimits::default()
    };
    let error = inspect_file(archive.path(), total_limits).expect_err("total is too large");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);
}

#[test]
fn returns_stable_kind_warning_and_error_semantics() {
    let renamed_json = synthetic_file("card.charx", VALID_CARD);
    let inspection =
        inspect_file(renamed_json.path(), ImportLimits::default()).expect("valid JSON content");
    assert_eq!(inspection.kind, ContentKind::CharacterCardV3);
    assert_eq!(
        inspection
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect::<Vec<_>>(),
        vec!["extension_mismatch"]
    );

    let wrong_spec = synthetic_file(
        "wrong.json",
        br#"{"spec":"chara_card_v2","data":{"name":"Legacy"}}"#,
    );
    let error = inspect_file(wrong_spec.path(), ImportLimits::default())
        .expect_err("unsupported spec must fail");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    assert!(!error.recoverable);
}

fn mark_first_entry_as_symlink(path: &Path) {
    const CENTRAL_DIRECTORY_MAGIC: &[u8; 4] = b"PK\x01\x02";
    const CREATOR_SYSTEM_OFFSET: usize = 5;
    const EXTERNAL_ATTRIBUTES_OFFSET: usize = 38;
    const UNIX_CREATOR_SYSTEM: u8 = 3;
    const SYMLINK_MODE: u32 = 0o120_777;

    let mut bytes = fs::read(path).expect("read archive");
    let position = bytes
        .windows(CENTRAL_DIRECTORY_MAGIC.len())
        .position(|window| window == CENTRAL_DIRECTORY_MAGIC)
        .expect("central directory entry");
    bytes[position + CREATOR_SYSTEM_OFFSET] = UNIX_CREATOR_SYSTEM;
    bytes[position + EXTERNAL_ATTRIBUTES_OFFSET..position + EXTERNAL_ATTRIBUTES_OFFSET + 4]
        .copy_from_slice(&(SYMLINK_MODE << 16).to_le_bytes());
    fs::write(path, bytes).expect("patch synthetic symlink metadata");
}

fn replace_archive_name(path: &Path, from: &[u8], to: &[u8]) {
    assert_eq!(from.len(), to.len(), "ZIP names must retain their length");
    let mut bytes = fs::read(path).expect("read archive");
    let positions = bytes
        .windows(from.len())
        .enumerate()
        .filter_map(|(position, window)| (window == from).then_some(position))
        .collect::<Vec<_>>();
    assert_eq!(
        positions.len(),
        2,
        "local and central names must be patched"
    );
    for position in positions {
        bytes[position..position + to.len()].copy_from_slice(to);
    }
    fs::write(path, bytes).expect("patch duplicate names");
}
