//! PNG card extraction, V2 promotion, and hostile-input rejection.

use std::io::Write;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use lorepia_content::{inspect_file, prepare_import};
use lorepia_domain::{ContentKind, CoreErrorCode, ImportLimits};
use tempfile::{NamedTempFile, TempDir};

const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

const V3_CARD: &str = r#"{"spec":"chara_card_v3","data":{"name":"Aria","description":"V3 card","personality":"calm"}}"#;
const V2_CARD: &str = r#"{"spec":"chara_card_v2","data":{"name":"Segu","description":"V2 card","first_mes":"안녕","mes_example":"<START>"}}"#;

/// Builds a PNG whose only meaningful content is the requested text chunk.
///
/// The CRC is not validated by the extractor, so a fixed placeholder keeps the
/// fixtures readable while still exercising the exact chunk framing.
fn png_with_chunk(chunk_type: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut bytes = PNG_SIGNATURE.to_vec();
    push_chunk(&mut bytes, *b"IHDR", &[0; 13]);
    push_chunk(&mut bytes, chunk_type, payload);
    push_chunk(&mut bytes, *b"IEND", &[]);
    bytes
}

fn push_chunk(bytes: &mut Vec<u8>, chunk_type: [u8; 4], payload: &[u8]) {
    bytes.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("chunk fits")
            .to_be_bytes(),
    );
    bytes.extend_from_slice(&chunk_type);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&[0, 0, 0, 0]);
}

fn text_payload(keyword: &str, text: &str) -> Vec<u8> {
    let mut payload = keyword.as_bytes().to_vec();
    payload.push(0);
    payload.extend_from_slice(BASE64.encode(text).as_bytes());
    payload
}

fn compressed_payload(keyword: &str, text: &str) -> Vec<u8> {
    use flate2::{Compression, write::ZlibEncoder};

    let mut payload = keyword.as_bytes().to_vec();
    payload.push(0);
    payload.push(0); // zlib compression method
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(BASE64.encode(text).as_bytes())
        .expect("compress");
    payload.extend_from_slice(&encoder.finish().expect("finish"));
    payload
}

fn write_png(bytes: &[u8]) -> NamedTempFile {
    let mut file = tempfile::Builder::new()
        .suffix(".png")
        .tempfile()
        .expect("temporary png");
    file.write_all(bytes).expect("write png");
    file.flush().expect("flush png");
    file
}

#[test]
fn reads_a_v3_card_from_a_text_chunk() {
    let file = write_png(&png_with_chunk(*b"tEXt", &text_payload("ccv3", V3_CARD)));
    let inspection = inspect_file(file.path(), ImportLimits::default()).expect("inspect");

    assert_eq!(inspection.kind, ContentKind::CharacterCardPng);
    assert_eq!(inspection.display_name, "Aria");
    assert_eq!(inspection.description, "V3 card");
    assert_eq!(inspection.asset_count, 1);
    assert!(inspection.is_allowed());

    let preview = inspection
        .representative_image
        .expect("png card exposes its own image");
    assert_eq!(preview.media_type, "image/png");
    assert!(
        !inspection
            .warnings
            .iter()
            .any(|warning| warning.code == "character_card_v2_promoted")
    );
}

#[test]
fn reads_a_v3_card_from_a_compressed_chunk() {
    let file = write_png(&png_with_chunk(
        *b"zTXt",
        &compressed_payload("ccv3", V3_CARD),
    ));
    let inspection = inspect_file(file.path(), ImportLimits::default()).expect("inspect");

    assert_eq!(inspection.kind, ContentKind::CharacterCardPng);
    assert_eq!(inspection.display_name, "Aria");
}

#[test]
fn promotes_a_legacy_v2_card_and_reports_the_promotion() {
    let file = write_png(&png_with_chunk(*b"tEXt", &text_payload("chara", V2_CARD)));
    let inspection = inspect_file(file.path(), ImportLimits::default()).expect("inspect");

    assert_eq!(inspection.display_name, "Segu");
    assert_eq!(inspection.description, "V2 card");
    assert!(
        inspection
            .warnings
            .iter()
            .any(|warning| warning.code == "character_card_v2_promoted"),
        "reviewers must be told the card was promoted"
    );
}

#[test]
fn prefers_the_v3_keyword_when_both_are_present() {
    let mut bytes = PNG_SIGNATURE.to_vec();
    push_chunk(&mut bytes, *b"IHDR", &[0; 13]);
    push_chunk(&mut bytes, *b"tEXt", &text_payload("chara", V2_CARD));
    push_chunk(&mut bytes, *b"tEXt", &text_payload("ccv3", V3_CARD));
    push_chunk(&mut bytes, *b"IEND", &[]);
    let file = write_png(&bytes);

    let inspection = inspect_file(file.path(), ImportLimits::default()).expect("inspect");
    assert_eq!(inspection.display_name, "Aria");
}

#[test]
fn stages_the_source_image_as_the_card_avatar() {
    let file = write_png(&png_with_chunk(*b"tEXt", &text_payload("ccv3", V3_CARD)));
    let staging = TempDir::new().expect("staging directory");

    let prepared = prepare_import(file.path(), ImportLimits::default(), staging.path())
        .expect("prepare import");

    assert_eq!(prepared.staged_assets.len(), 1);
    let asset = &prepared.staged_assets[0];
    assert_eq!(asset.media_type, "image/png");
    assert_eq!(asset.sha256, prepared.inspection.source_sha256);
    assert_eq!(asset.size_bytes, prepared.inspection.source_size);
    assert!(asset.staged_path.exists());
}

#[test]
fn rejects_a_bare_json_card_that_declares_an_unknown_spec() {
    let mut file = tempfile::Builder::new()
        .suffix(".json")
        .tempfile()
        .expect("temporary card");
    file.write_all(br#"{"spec":"chara_card_v1","data":{"name":"Old"}}"#)
        .expect("write");
    file.flush().expect("flush");

    let error = inspect_file(file.path(), ImportLimits::default()).expect_err("must reject");
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
}

#[test]
fn rejects_hostile_and_malformed_png_sources() {
    // No embedded metadata at all.
    let empty = write_png(&png_with_chunk(*b"tEXt", &text_payload("Comment", "hello")));
    assert_eq!(
        inspect_file(empty.path(), ImportLimits::default())
            .expect_err("no metadata")
            .code,
        CoreErrorCode::UnsupportedContent
    );

    // Chunk claims more bytes than the file holds.
    let mut truncated = PNG_SIGNATURE.to_vec();
    truncated.extend_from_slice(&1_000_u32.to_be_bytes());
    truncated.extend_from_slice(b"tEXt");
    truncated.extend_from_slice(b"short");
    let truncated = write_png(&truncated);
    assert_eq!(
        inspect_file(truncated.path(), ImportLimits::default())
            .expect_err("truncated chunk")
            .code,
        CoreErrorCode::UnsupportedContent
    );

    // Keyword present, payload is not base64.
    let mut payload = b"ccv3".to_vec();
    payload.push(0);
    payload.extend_from_slice(b"!!!not base64!!!");
    let corrupt = write_png(&png_with_chunk(*b"tEXt", &payload));
    assert_eq!(
        inspect_file(corrupt.path(), ImportLimits::default())
            .expect_err("corrupt base64")
            .code,
        CoreErrorCode::UnsupportedContent
    );

    // Valid base64 that does not decode to a character card.
    let not_a_card = write_png(&png_with_chunk(*b"tEXt", &text_payload("ccv3", "{}")));
    assert_eq!(
        inspect_file(not_a_card.path(), ImportLimits::default())
            .expect_err("not a card")
            .code,
        CoreErrorCode::UnsupportedContent
    );
}

#[test]
fn rejects_a_compressed_metadata_bomb() {
    use flate2::{Compression, write::ZlibEncoder};

    // ~64 MiB of zeros compresses to a few KiB; the extractor must refuse to
    // materialize it rather than inflating past the metadata ceiling.
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(&vec![b'A'; 64 * 1024 * 1024])
        .expect("compress");
    let stream = encoder.finish().expect("finish");

    let mut payload = b"ccv3".to_vec();
    payload.push(0);
    payload.push(0);
    payload.extend_from_slice(&stream);
    let bomb = write_png(&png_with_chunk(*b"zTXt", &payload));

    assert_eq!(
        inspect_file(bomb.path(), ImportLimits::default())
            .expect_err("decompression bomb")
            .code,
        CoreErrorCode::UnsupportedContent
    );
}
