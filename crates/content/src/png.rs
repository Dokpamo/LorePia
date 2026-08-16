//! Bounded extraction of embedded character metadata from PNG cards.
//!
//! Character cards are distributed most often as PNG images whose textual
//! chunks carry the base64-encoded card JSON. Everything in this module treats
//! the file as hostile input: chunk walking is bounded on every axis, the
//! compressed `zTXt` path enforces an output ceiling so a decompression bomb
//! cannot exhaust memory, and no chunk is interpreted as an image.

use std::io::Read;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
const PNG_MAGIC_PREFIX: [u8; 4] = [0x89, b'P', b'N', b'G'];

/// Maximum chunks walked before the file is rejected as malformed.
const MAX_CHUNKS: usize = 4_096;
/// Maximum size of one chunk's payload.
const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;
/// Maximum size of a text chunk keyword, per the PNG specification.
const MAX_KEYWORD_BYTES: usize = 79;
/// Maximum decoded metadata size, matching the JSON card ceiling.
const MAX_DECODED_BYTES: usize = super::adapters::MAX_METADATA_BYTES;

const CHUNK_TYPE_LENGTH: usize = 4;
const CHUNK_CRC_LENGTH: usize = 4;
const TEXT_CHUNK: &[u8; 4] = b"tEXt";
const COMPRESSED_TEXT_CHUNK: &[u8; 4] = b"zTXt";
const END_CHUNK: &[u8; 4] = b"IEND";

/// Keyword holding V3 card JSON. Preferred whenever both are present.
const V3_KEYWORD: &[u8] = b"ccv3";
/// Legacy keyword holding V2 card JSON.
const V2_KEYWORD: &[u8] = b"chara";

/// Matches the leading bytes the shared four-byte magic read can cover.
///
/// This only routes the file to the PNG branch. `extract_card_metadata`
/// validates the complete eight-byte signature before reading any chunk.
pub(crate) fn has_png_magic(magic: &[u8]) -> bool {
    magic.len() >= PNG_MAGIC_PREFIX.len() && magic[..PNG_MAGIC_PREFIX.len()] == PNG_MAGIC_PREFIX
}

fn is_png_signature(bytes: &[u8]) -> bool {
    bytes.len() >= PNG_SIGNATURE.len() && bytes[..PNG_SIGNATURE.len()] == PNG_SIGNATURE
}

/// Extracts the embedded card JSON, preferring the V3 keyword.
///
/// Returns the decoded metadata bytes. The caller parses and validates them
/// with the ordinary card adapter, so a PNG card is held to exactly the same
/// content rules as a bare JSON card.
pub(crate) fn extract_card_metadata(bytes: &[u8]) -> CoreResult<Vec<u8>> {
    if !is_png_signature(bytes) {
        return Err(unsupported("the import source is not a PNG image"));
    }

    let mut cursor = PNG_SIGNATURE.len();
    let mut chunks = 0_usize;
    let mut legacy: Option<Vec<u8>> = None;

    while cursor < bytes.len() {
        chunks += 1;
        if chunks > MAX_CHUNKS {
            return Err(unsupported(format!(
                "PNG card exceeds the {MAX_CHUNKS}-chunk limit"
            )));
        }

        // length(4) + type(4) must be readable before the payload.
        let header_end = cursor
            .checked_add(CHUNK_TYPE_LENGTH + 4)
            .ok_or_else(|| unsupported("PNG chunk header overflows the file"))?;
        if header_end > bytes.len() {
            return Err(unsupported("PNG chunk header is truncated"));
        }
        let length = u32::from_be_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]) as usize;
        if length > MAX_CHUNK_BYTES {
            return Err(unsupported(format!(
                "PNG chunk is {length} bytes; maximum is {MAX_CHUNK_BYTES} bytes"
            )));
        }
        let chunk_type = &bytes[cursor + 4..header_end];
        let payload_end = header_end
            .checked_add(length)
            .ok_or_else(|| unsupported("PNG chunk payload overflows the file"))?;
        let chunk_end = payload_end
            .checked_add(CHUNK_CRC_LENGTH)
            .ok_or_else(|| unsupported("PNG chunk CRC overflows the file"))?;
        if chunk_end > bytes.len() {
            return Err(unsupported("PNG chunk payload is truncated"));
        }
        let payload = &bytes[header_end..payload_end];

        if chunk_type == END_CHUNK {
            break;
        }

        let decoded = if chunk_type == TEXT_CHUNK {
            read_text_chunk(payload)
        } else if chunk_type == COMPRESSED_TEXT_CHUNK {
            read_compressed_text_chunk(payload)?
        } else {
            None
        };

        if let Some((keyword, encoded)) = decoded {
            if keyword == V3_KEYWORD {
                return decode_base64(&encoded);
            }
            if keyword == V2_KEYWORD && legacy.is_none() {
                legacy = Some(encoded);
            }
        }

        cursor = chunk_end;
    }

    legacy.as_deref().map_or_else(
        || Err(unsupported("PNG card has no embedded character metadata")),
        decode_base64,
    )
}

/// Splits a `tEXt` payload into its keyword and Latin-1 text.
fn read_text_chunk(payload: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let separator = payload.iter().position(|byte| *byte == 0)?;
    if separator == 0 || separator > MAX_KEYWORD_BYTES {
        return None;
    }
    let text = &payload[separator + 1..];
    if text.len() > MAX_CHUNK_BYTES {
        return None;
    }
    Some((payload[..separator].to_vec(), text.to_vec()))
}

/// Splits a `zTXt` payload and inflates it under a hard output ceiling.
fn read_compressed_text_chunk(payload: &[u8]) -> CoreResult<Option<(Vec<u8>, Vec<u8>)>> {
    let Some(separator) = payload.iter().position(|byte| *byte == 0) else {
        return Ok(None);
    };
    if separator == 0 || separator > MAX_KEYWORD_BYTES {
        return Ok(None);
    }
    let keyword = payload[..separator].to_vec();
    // Only the keyword-bearing chunks are worth inflating at all.
    if keyword != V3_KEYWORD && keyword != V2_KEYWORD {
        return Ok(None);
    }
    // separator, then a one-byte compression method, then the zlib stream.
    let Some(method_index) = separator.checked_add(1) else {
        return Ok(None);
    };
    if method_index >= payload.len() {
        return Ok(None);
    }
    if payload[method_index] != 0 {
        return Err(unsupported(
            "PNG card uses an unsupported zTXt compression method",
        ));
    }
    let stream = &payload[method_index + 1..];

    // `take` bounds the inflated output, so a decompression bomb cannot grow
    // past the ceiling regardless of the compressed size.
    let mut inflated = Vec::new();
    let limit = u64::try_from(MAX_DECODED_BYTES)
        .map_err(|_| unsupported("PNG card metadata limit is unrepresentable"))?;
    flate2::read::ZlibDecoder::new(stream)
        .take(limit.saturating_add(1))
        .read_to_end(&mut inflated)
        .map_err(|_| unsupported("PNG card zTXt metadata is not a valid zlib stream"))?;
    if inflated.len() > MAX_DECODED_BYTES {
        return Err(unsupported(format!(
            "PNG card metadata exceeds the {MAX_DECODED_BYTES}-byte limit"
        )));
    }
    Ok(Some((keyword, inflated)))
}

fn decode_base64(encoded: &[u8]) -> CoreResult<Vec<u8>> {
    // Cards in the wild wrap the base64 payload across lines.
    let compact = encoded
        .iter()
        .copied()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect::<Vec<_>>();
    if compact.is_empty() {
        return Err(unsupported("PNG card metadata is empty"));
    }
    // Bound the encoded form before allocating the decoded form.
    if compact.len() / 4 * 3 > MAX_DECODED_BYTES {
        return Err(unsupported(format!(
            "PNG card metadata exceeds the {MAX_DECODED_BYTES}-byte limit"
        )));
    }
    let decoded = BASE64
        .decode(&compact)
        .map_err(|_| unsupported("PNG card metadata is not valid base64"))?;
    if decoded.len() > MAX_DECODED_BYTES {
        return Err(unsupported(format!(
            "PNG card metadata exceeds the {MAX_DECODED_BYTES}-byte limit"
        )));
    }
    Ok(decoded)
}

fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}
