use super::*;
use lorepia_domain::{AssetId, AssetRole, AssetSource, AssetSourceKind, Sha256Digest};
use std::io::Write;
use tempfile::NamedTempFile;

fn descriptor(bytes: &[u8]) -> AssetDescriptor {
    AssetDescriptor {
        id: AssetId::from("image-policy"),
        sha256: Sha256Digest::parse("ab".repeat(32)).unwrap(),
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
    }
}

fn check(bytes: &[u8]) -> CoreResult<()> {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file.rewind().unwrap();
    validate(file.as_file_mut(), &descriptor(bytes)).map(|_| ())
}

#[test]
fn cold_policy_accepts_valid_png_and_rejects_corruption_outside_header_range() {
    let mut bytes = png::valid_test_image();
    check(&bytes).unwrap();
    bytes[54] ^= 1;
    assert_eq!(
        check(&bytes).unwrap_err().code,
        CoreErrorCode::UnsupportedContent
    );
}

#[test]
fn display_pixel_budget_rejects_large_dimensions_without_decoding_or_rewriting() {
    let mut bytes = png::valid_test_image();
    bytes[16..20].copy_from_slice(&8192u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&8192u32.to_be_bytes());
    // Correct the header checksum: the rejection is the pixel budget, not CRC.
    let checksum = crc_reference(&bytes[12..29]);
    bytes[29..33].copy_from_slice(&checksum.to_be_bytes());
    assert!(png::has_valid_chunk_checksums(&bytes));
    let error = check(&bytes).unwrap_err();
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    assert!(error.message.contains("single-frame"));
}

fn crc_reference(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
        }
    }
    !crc
}

#[test]
fn header_reader_rejects_outside_seeks_and_repeated_work() {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(&[0; 16]).unwrap();
    let mut reader = BoundedReader {
        file: file.as_file_mut(),
        length: 16,
        remaining_bytes: 2,
        remaining_ops: 10,
        failed: false,
    };
    assert!(reader.seek(SeekFrom::Start(17)).is_err());
    assert!(reader.seek(SeekFrom::End(-17)).is_err());
    reader.rewind().unwrap();
    assert_eq!(reader.read(&mut [0; 8]).unwrap(), 2);
    assert!(reader.read(&mut [0; 1]).is_err());
    reader.remaining_ops = 1;
    reader.rewind().unwrap();
    assert!(reader.rewind().is_err());
}

fn check_media(bytes: &[u8], media: &str) -> CoreResult<()> {
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file.rewind().unwrap();
    let mut descriptor = descriptor(bytes);
    descriptor.media_type = media.into();
    validate(file.as_file_mut(), &descriptor).map(|_| ())
}

#[test]
fn gif_jpeg_and_webp_headers_share_the_dimension_budget() {
    // These are dimension-header fixtures; this policy is not a full codec validator.
    let gif = b"GIF89a\x01\x00\x01\x00\x00\x00\x00";
    let jpeg = hex::decode("ffd8ffc00011080001000103012200021101031100").unwrap();
    let mut webp = vec![0; 30];
    webp[..4].copy_from_slice(b"RIFF");
    webp[8..16].copy_from_slice(b"WEBPVP8X");
    for (bytes, media) in [
        (gif.as_slice(), "image/gif"),
        (jpeg.as_slice(), "image/jpeg"),
        (webp.as_slice(), "image/webp"),
    ] {
        check_media(bytes, media).unwrap();
    }
    webp[24..27].copy_from_slice(&[0xff, 0x1f, 0]);
    webp[27..30].copy_from_slice(&[0xff, 0x1f, 0]);
    assert_eq!(
        check_media(&webp, "image/webp").unwrap_err().code,
        CoreErrorCode::UnsupportedContent
    );
}

#[cfg(target_pointer_width = "64")]
#[test]
fn heif_parser_cannot_swallow_a_reader_boundary_failure_after_valid_dimensions() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&20u32.to_be_bytes());
    bytes.extend_from_slice(b"ftypavif\0\0\0\0avif");
    bytes.extend_from_slice(&100_000u32.to_be_bytes());
    bytes.extend_from_slice(b"meta\0\0\0\0");
    bytes.extend_from_slice(&100_000u32.to_be_bytes());
    bytes.extend_from_slice(b"iprp");
    bytes.extend_from_slice(&100_000u32.to_be_bytes());
    bytes.extend_from_slice(b"ipco");
    bytes.extend_from_slice(&20u32.to_be_bytes());
    bytes.extend_from_slice(b"ispe\0\0\0\0");
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.extend_from_slice(&1u32.to_be_bytes());
    // Empty unknown boxes cause bounded seeks. After the valid ispe, exhaust
    // work during the next read_tag: that while-condition error is swallowed by
    // imagesize, which returns the earlier 1x1 size. The sticky flag must reject it.
    for _ in 0..4096 {
        bytes.extend_from_slice(&8u32.to_be_bytes());
        bytes.extend_from_slice(b"junk");
    }
    let error = check_media(&bytes, "image/avif").unwrap_err();
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    assert!(error.message.contains("bounded work"));
}
