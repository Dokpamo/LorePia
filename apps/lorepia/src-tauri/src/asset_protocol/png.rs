//! Reject corrupt PNG chunks before handing bytes to a permissive native decoder.
//! This checks container integrity, not decompressed pixels or image semantics.

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
const CRC_TABLE: [[u32; 256]; 8] = crc_table();

const fn crc_table() -> [[u32; 256]; 8] {
    let mut table = [[0; 256]; 8];
    let mut index = 0;
    let mut seed = 0_u32;
    while index < 256 {
        let mut value = seed;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ if value & 1 == 1 { 0xedb8_8320 } else { 0 };
            bit += 1;
        }
        table[0][index] = value;
        index += 1;
        seed += 1;
    }
    index = 0;
    while index < 256 {
        let mut value = table[0][index];
        let mut slice = 1;
        while slice < 8 {
            value = (value >> 8) ^ table[0][(value & 0xff) as usize];
            table[slice][index] = value;
            slice += 1;
        }
        index += 1;
    }
    table
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    let mut blocks = bytes.chunks_exact(8);
    // Each table advances one byte by its remaining zero bytes. Explicit
    // little-endian words avoid alignment and host-endianness assumptions.
    for block in &mut blocks {
        let word = crc ^ u32::from_le_bytes(block[..4].try_into().expect("four CRC bytes"));
        let word = word.to_le_bytes();
        crc = CRC_TABLE[7][usize::from(word[0])]
            ^ CRC_TABLE[6][usize::from(word[1])]
            ^ CRC_TABLE[5][usize::from(word[2])]
            ^ CRC_TABLE[4][usize::from(word[3])]
            ^ CRC_TABLE[3][usize::from(block[4])]
            ^ CRC_TABLE[2][usize::from(block[5])]
            ^ CRC_TABLE[1][usize::from(block[6])]
            ^ CRC_TABLE[0][usize::from(block[7])];
    }
    for byte in blocks.remainder() {
        crc = (crc >> 8) ^ CRC_TABLE[0][usize::from(crc.to_le_bytes()[0] ^ byte)];
    }
    !crc
}

pub(super) fn has_valid_chunk_checksums(bytes: &[u8]) -> bool {
    if bytes.len() as u64 > super::MAX_RENDERABLE_IMAGE_BYTES || !bytes.starts_with(SIGNATURE) {
        return false;
    }
    let mut offset = SIGNATURE.len();
    let mut has_image_data = false;
    while let Some(header) = bytes.get(offset..).and_then(|tail| tail.get(..8)) {
        let length = u32::from_be_bytes(header[..4].try_into().expect("chunk length"));
        let Ok(length) = usize::try_from(length) else {
            return false;
        };
        let Some(end) = offset
            .checked_add(12)
            .and_then(|end| end.checked_add(length))
        else {
            return false;
        };
        let Some(chunk) = bytes.get(offset..end) else {
            return false;
        };
        let kind = &header[4..8];
        if offset == SIGNATURE.len() && (kind != b"IHDR" || length != 13) {
            return false;
        }
        let crc_offset = chunk.len() - 4;
        let expected = u32::from_be_bytes(chunk[crc_offset..].try_into().expect("chunk CRC"));
        if crc32(&chunk[4..crc_offset]) != expected {
            return false;
        }
        if kind == b"IDAT" {
            has_image_data = true;
        }
        if kind == b"IEND" {
            return length == 0 && has_image_data && end == bytes.len();
        }
        offset = end;
    }
    false
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) fn valid_test_image() -> Vec<u8> {
    // A 1x1 opaque red PNG with known valid chunk checksums.
    let hex = "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d4944415408d763f8cfc0f01f00050001ff729c52670000000049454e44ae426082";
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&hex[index..index + 2], 16).expect("fixture hex"))
        .collect()
}
