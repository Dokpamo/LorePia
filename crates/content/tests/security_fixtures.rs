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

fn rewrite_single_entry_as_zip64(path: &Path, central_record_copies: u16) {
    const EOCD_MAGIC: &[u8; 4] = b"PK\x05\x06";
    const ZIP64_EOCD_MAGIC: &[u8; 4] = b"PK\x06\x06";
    const ZIP64_LOCATOR_MAGIC: &[u8; 4] = b"PK\x06\x07";

    let bytes = fs::read(path).expect("read ZIP fixture");
    let eocd_offset = bytes
        .windows(EOCD_MAGIC.len())
        .rposition(|window| window == EOCD_MAGIC)
        .expect("ZIP32 EOCD");
    let central_size = u32::from_le_bytes(
        bytes[eocd_offset + 12..eocd_offset + 16]
            .try_into()
            .expect("central size"),
    ) as usize;
    let central_offset = u32::from_le_bytes(
        bytes[eocd_offset + 16..eocd_offset + 20]
            .try_into()
            .expect("central offset"),
    ) as usize;
    assert_eq!(central_offset + central_size, eocd_offset);
    let central_record = &bytes[central_offset..eocd_offset];

    let mut rewritten = bytes[..central_offset].to_vec();
    for _ in 0..central_record_copies {
        rewritten.extend_from_slice(central_record);
    }
    let rewritten_central_size = u64::try_from(central_record.len())
        .expect("central record length")
        * u64::from(central_record_copies);
    let zip64_eocd_offset = u64::try_from(rewritten.len()).expect("ZIP64 EOCD offset");
    rewritten.extend_from_slice(ZIP64_EOCD_MAGIC);
    rewritten.extend_from_slice(&44_u64.to_le_bytes());
    rewritten.extend_from_slice(&45_u16.to_le_bytes());
    rewritten.extend_from_slice(&45_u16.to_le_bytes());
    rewritten.extend_from_slice(&0_u32.to_le_bytes());
    rewritten.extend_from_slice(&0_u32.to_le_bytes());
    rewritten.extend_from_slice(&u64::from(central_record_copies).to_le_bytes());
    rewritten.extend_from_slice(&u64::from(central_record_copies).to_le_bytes());
    rewritten.extend_from_slice(&rewritten_central_size.to_le_bytes());
    rewritten.extend_from_slice(
        &u64::try_from(central_offset)
            .expect("central offset")
            .to_le_bytes(),
    );
    rewritten.extend_from_slice(ZIP64_LOCATOR_MAGIC);
    rewritten.extend_from_slice(&0_u32.to_le_bytes());
    rewritten.extend_from_slice(&zip64_eocd_offset.to_le_bytes());
    rewritten.extend_from_slice(&1_u32.to_le_bytes());

    let mut classic_eocd = bytes[eocd_offset..].to_vec();
    classic_eocd[8..10].copy_from_slice(&1_u16.to_le_bytes());
    classic_eocd[10..12].copy_from_slice(&1_u16.to_le_bytes());
    classic_eocd[12..16].copy_from_slice(
        &u32::try_from(rewritten_central_size)
            .expect("small central directory")
            .to_le_bytes(),
    );
    classic_eocd[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    rewritten.extend_from_slice(&classic_eocd);
    fs::write(path, rewritten).expect("write deterministic ZIP64 fixture");
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
fn bounds_raw_zip64_duplicate_records_before_archive_parsing() {
    let duplicate_records = synthetic_archive(vec![card_entry()]);
    rewrite_single_entry_as_zip64(duplicate_records.path(), 3);
    let limits = ImportLimits {
        max_entries: 2,
        ..ImportLimits::default()
    };
    let error = inspect_file(duplicate_records.path(), limits)
        .expect_err("raw ZIP64 central-directory records must be bounded");
    assert_eq!(error.code, CoreErrorCode::UnsafeArchive);

    let compatible = synthetic_archive(vec![card_entry()]);
    rewrite_single_entry_as_zip64(compatible.path(), 1);
    inspect_file(compatible.path(), ImportLimits::default())
        .expect("ordinary single-entry ZIP64 CHARX remains compatible");
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

    // A V2 card is promoted to the canonical V3 shape, and the reviewer is
    // told before anything is committed.
    let legacy_spec = synthetic_file(
        "legacy.json",
        br#"{"spec":"chara_card_v2","data":{"name":"Legacy"}}"#,
    );
    let inspection =
        inspect_file(legacy_spec.path(), ImportLimits::default()).expect("V2 card is promoted");
    assert_eq!(inspection.kind, ContentKind::CharacterCardV3);
    assert_eq!(inspection.display_name, "Legacy");
    assert_eq!(
        inspection
            .warnings
            .iter()
            .map(|warning| warning.code.as_str())
            .collect::<Vec<_>>(),
        vec!["character_card_v2_promoted"]
    );

    // Every other spec is still refused outright.
    let wrong_spec = synthetic_file(
        "wrong.json",
        br#"{"spec":"chara_card_v1","data":{"name":"Ancient"}}"#,
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
