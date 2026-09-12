use std::io::Write;

use super::*;

fn append_chunk(png: &mut Vec<u8>, kind: [u8; 4], payload: &[u8]) {
    png.extend_from_slice(
        &u32::try_from(payload.len())
            .expect("bounded payload")
            .to_be_bytes(),
    );
    png.extend_from_slice(&kind);
    png.extend_from_slice(payload);
    png.extend_from_slice(&[0; 4]);
}

fn text_payload(keyword: &[u8], decoded: &[u8]) -> Vec<u8> {
    let mut payload = keyword.to_vec();
    payload.push(0);
    payload.extend_from_slice(BASE64.encode(decoded).as_bytes());
    payload
}

#[test]
fn text_chunks_borrow_large_irrelevant_payloads_and_non_utf8_keywords() {
    let mut payload = vec![0xff, 0];
    payload.resize(1024 * 1024, b'A');
    let (keyword, text) = read_text_chunk(&payload).expect("valid Latin-1 keyword");
    assert_eq!(keyword, &[0xff]);
    let Cow::Borrowed(text) = text else {
        panic!("uncompressed text must not allocate a second payload");
    };
    assert_eq!(text.as_ptr(), payload[2..].as_ptr());
    assert_eq!(text.len(), payload.len() - 2);
}

#[test]
fn first_legacy_metadata_survives_later_chunks_and_compressed_v3_takes_priority() {
    let mut png = PNG_SIGNATURE.to_vec();
    append_chunk(
        &mut png,
        *TEXT_CHUNK,
        &text_payload(V2_KEYWORD, b"first legacy"),
    );
    append_chunk(
        &mut png,
        *TEXT_CHUNK,
        &text_payload(V2_KEYWORD, b"second legacy"),
    );
    append_chunk(
        &mut png,
        *TEXT_CHUNK,
        &text_payload(b"Comment", b"unrelated"),
    );
    let mut legacy_png = png.clone();
    append_chunk(&mut legacy_png, *END_CHUNK, &[]);
    assert_eq!(
        extract_card_metadata(&mut legacy_png.as_slice()).unwrap(),
        b"first legacy"
    );

    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(BASE64.encode(b"preferred v3").as_bytes())
        .unwrap();
    let mut compressed = b"ccv3\0\0".to_vec();
    compressed.extend_from_slice(&encoder.finish().unwrap());
    let (_, inflated) = read_compressed_text_chunk(&compressed).unwrap().unwrap();
    assert!(matches!(inflated, Cow::Owned(_)));
    append_chunk(&mut png, *COMPRESSED_TEXT_CHUNK, &compressed);
    append_chunk(&mut png, *END_CHUNK, &[]);
    assert_eq!(
        extract_card_metadata(&mut png.as_slice()).unwrap(),
        b"preferred v3"
    );
}

#[test]
fn base64_accepts_wrapped_and_compact_bytes_without_interpreting_utf8() {
    let decoded = b"card\xff\0bytes";
    let encoded = BASE64.encode(decoded);
    let mut wrapped = Vec::new();
    for byte in encoded.bytes() {
        wrapped.extend_from_slice(b" \t\r\n\x0c");
        wrapped.push(byte);
    }
    assert_eq!(decode_base64(encoded.as_bytes()).unwrap(), decoded);
    assert_eq!(decode_base64(&wrapped).unwrap(), decoded);
}

#[test]
fn base64_preserves_empty_invalid_and_size_errors() {
    for input in [b"".as_slice(), b" \t\r\n\x0c"] {
        let error = decode_base64(input).unwrap_err();
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
        assert_eq!(error.message, "PNG card metadata is empty");
    }
    for input in [b"\xff".as_slice(), b" \xff ", b"A===", b" A=== "] {
        let error = decode_base64(input).unwrap_err();
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
        assert_eq!(error.message, "PNG card metadata is not valid base64");
    }
    let oversized = vec![b'A'; (MAX_DECODED_BYTES / 3 + 1) * 4];
    let error = decode_base64(&oversized).unwrap_err();
    assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
    assert!(error.message.contains("metadata exceeds"));
}
