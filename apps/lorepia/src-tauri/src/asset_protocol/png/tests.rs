use super::*;

#[test]
fn accepts_valid_png_and_rejects_the_native_decoder_corruption_fixture() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    let mut bytes = valid_test_image();
    assert!(has_valid_chunk_checksums(&bytes));
    // The malformed red fixture reports native load success but renders transparent.
    bytes[54..58].copy_from_slice(&[0x89, 0x99, 0x3d, 0x1d]);
    assert!(!has_valid_chunk_checksums(&bytes));
}

#[test]
fn every_truncation_and_each_chunk_corruption_fails_closed() {
    let bytes = valid_test_image();
    for end in 0..bytes.len() {
        assert!(
            !has_valid_chunk_checksums(&bytes[..end]),
            "truncated at {end}"
        );
    }
    for offset in [0, 20, 29, 41, 54, 66] {
        let mut corrupt = bytes.clone();
        corrupt[offset] ^= 1;
        assert!(!has_valid_chunk_checksums(&corrupt), "changed {offset}");
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(!has_valid_chunk_checksums(&trailing));
}

#[test]
fn malformed_chunk_lengths_and_missing_image_data_are_rejected() {
    for length in [0_u32, 12, u32::MAX] {
        let mut bytes = valid_test_image();
        bytes[8..12].copy_from_slice(&length.to_be_bytes());
        assert!(!has_valid_chunk_checksums(&bytes));
    }
    let mut bytes = valid_test_image();
    bytes.drain(33..58);
    assert!(!has_valid_chunk_checksums(&bytes));
    assert!(!has_valid_chunk_checksums(&vec![
        0;
        usize::try_from(
            super::super::MAX_RENDERABLE_IMAGE_BYTES
        )
        .expect("image bound")
            + 1
    ]));
}

#[test]
fn validates_ancillary_chunks_too() {
    let mut bytes = valid_test_image();
    let mut text = 3_u32.to_be_bytes().to_vec();
    text.extend_from_slice(b"tEXta\0b");
    text.extend_from_slice(&crc32(&text[4..]).to_be_bytes());
    bytes.splice(33..33, text);
    assert!(has_valid_chunk_checksums(&bytes));
    bytes[41] ^= 1;
    assert!(!has_valid_chunk_checksums(&bytes));
}
