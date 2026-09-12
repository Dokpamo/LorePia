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

fn scalar_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
        }
    }
    crc
}

#[test]
fn crc_matches_scalar_across_lengths_alignments_and_split_updates() {
    let mut seed = 0x92a5_734b_u32;
    let bytes: Vec<_> = (0..65_552)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed.to_le_bytes()[0]
        })
        .collect();
    for offset in 0..16 {
        for length in (0..=257).chain([4095, 4096, 16_384, 65_536]) {
            let input = &bytes[offset..offset + length];
            let split = length / 3;
            let intermediate = scalar_update(u32::MAX, &input[..split]);
            let expected = !scalar_update(intermediate, &input[split..]);
            assert_eq!(crc32(input), expected, "offset {offset}, length {length}");
        }
    }
    assert_eq!(crc32(&[]), 0);
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
}

#[test]
fn arbitrary_chunk_boundaries_preserve_all_payload_corruption_checks() {
    let valid = valid_test_image();
    let mut image = valid[..33].to_vec();
    let mut payload_offsets = Vec::new();
    for length in [1_usize, 7, 8, 9, 15, 16, 17, 255, 4096] {
        image.extend_from_slice(&u32::try_from(length).unwrap().to_be_bytes());
        let checksum_start = image.len();
        image.extend_from_slice(b"IDAT");
        payload_offsets.push(image.len());
        image.extend((0..length).map(|index| index.to_le_bytes()[0]));
        let checksum = !scalar_update(u32::MAX, &image[checksum_start..]);
        image.extend_from_slice(&checksum.to_be_bytes());
    }
    image.extend_from_slice(&valid[valid.len() - 12..]);
    // This validator checks container CRC integrity, not pixel decoding.
    assert!(has_valid_chunk_checksums(&image));
    for offset in payload_offsets {
        image[offset] ^= 1;
        assert!(
            !has_valid_chunk_checksums(&image),
            "payload offset {offset}"
        );
        image[offset] ^= 1;
    }
}
