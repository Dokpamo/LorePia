use std::{
    fs::File,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
};

use flate2::read::GzDecoder;
use lorepia_domain::{CoreError, CoreErrorCode, CoreResult, ImportLimits};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::messagepack::{self, MessagePackValue};
use crate::runtime::{
    self, DecodedRuntimeAssetMetadata, DecodedRuntimeMetadata, decode_substitution_in_place,
};

const RISU_MAGIC: [u8; 2] = [111, 0];
const RISU_HEADER_BYTES: usize = 6;
const MAX_MODULE_METADATA_BYTES: usize = 4 * 1024 * 1024;
const MAX_PRESET_CONTAINER_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PRESET_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
const BUFFER_BYTES: usize = 64 * 1024;
const MAX_MESSAGEPACK_DEPTH: usize = 32;
const MAX_MESSAGEPACK_NODES: usize = 200_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RisuAssetRecord {
    pub(super) offset: u64,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
    pub(super) extension: String,
    pub(super) media_type: String,
    pub(super) declared_extension_mismatch: bool,
}

pub(super) struct RisuModuleSource {
    pub(super) metadata: DecodedRuntimeMetadata,
    pub(super) assets: Vec<RisuAssetRecord>,
}

pub(super) fn is_risu_module(path: &Path) -> CoreResult<bool> {
    let mut file = File::open(path).map_err(storage_error)?;
    let mut header = [0_u8; 2];
    let read = file.read(&mut header).map_err(storage_error)?;
    Ok(read == header.len() && header == RISU_MAGIC)
}

pub(super) fn read_risu_module(
    path: &Path,
    limits: ImportLimits,
    source_sha256: &str,
) -> CoreResult<RisuModuleSource> {
    let mut reader = BufReader::new(File::open(path).map_err(storage_error)?);
    let mut header = [0_u8; RISU_HEADER_BYTES];
    reader
        .read_exact(&mut header)
        .map_err(|_| unsupported("Risu module header is truncated"))?;
    if header[..2] != RISU_MAGIC {
        return Err(unsupported("Risu module header is invalid"));
    }
    let metadata_len = usize::try_from(u32::from_le_bytes(
        header[2..6].try_into().expect("four-byte length"),
    ))
    .map_err(|_| unsupported("Risu module metadata does not fit this device"))?;
    if metadata_len == 0 || metadata_len > MAX_MODULE_METADATA_BYTES {
        return Err(unsupported("Risu module metadata exceeds the 4 MiB limit"));
    }
    let mut encoded = vec![0_u8; metadata_len];
    reader
        .read_exact(&mut encoded)
        .map_err(|_| unsupported("Risu module metadata is truncated"))?;
    let metadata = runtime::decode_runtime_metadata(&encoded, source_sha256)?;
    let assets = scan_asset_records(&mut reader, &metadata.asset_metadata, limits)?;
    Ok(RisuModuleSource { metadata, assets })
}

fn scan_asset_records(
    reader: &mut BufReader<File>,
    metadata: &[DecodedRuntimeAssetMetadata],
    limits: ImportLimits,
) -> CoreResult<Vec<RisuAssetRecord>> {
    if metadata.len() > limits.max_entries {
        return Err(unsafe_archive("Risu module exceeds the entry-count limit"));
    }
    let mut records = Vec::with_capacity(metadata.len());
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    for asset in metadata {
        let mut record_type = [0_u8; 1];
        reader
            .read_exact(&mut record_type)
            .map_err(|_| unsafe_archive("Risu asset record is truncated"))?;
        if record_type[0] != 1 {
            return Err(unsafe_archive("Risu asset record type is invalid"));
        }
        let mut length = [0_u8; 4];
        reader
            .read_exact(&mut length)
            .map_err(|_| unsafe_archive("Risu asset length is truncated"))?;
        let size_bytes = u64::from(u32::from_le_bytes(length));
        total = total
            .checked_add(size_bytes)
            .ok_or_else(|| unsafe_archive("Risu asset size overflow"))?;
        if size_bytes == 0
            || size_bytes > limits.max_entry_bytes
            || total > limits.max_total_uncompressed_bytes
        {
            return Err(unsafe_archive("Risu assets exceed configured size limits"));
        }
        let offset = reader.stream_position().map_err(storage_error)?;
        let mut digest = Sha256::new();
        let mut remaining = size_bytes;
        let mut signature = Vec::with_capacity(16);
        while remaining > 0 {
            let wanted =
                usize::try_from(remaining.min(BUFFER_BYTES as u64)).expect("bounded buffer size");
            reader
                .read_exact(&mut buffer[..wanted])
                .map_err(|_| unsafe_archive("Risu asset payload is truncated"))?;
            decode_substitution_in_place(&mut buffer[..wanted])?;
            if signature.len() < 16 {
                let copy = (16 - signature.len()).min(wanted);
                signature.extend_from_slice(&buffer[..copy]);
            }
            digest.update(&buffer[..wanted]);
            remaining -= wanted as u64;
        }
        let (extension, media_type, declared_extension_mismatch) =
            validate_asset_signature(asset, &signature)?;
        records.push(RisuAssetRecord {
            offset,
            size_bytes,
            sha256: hex::encode(digest.finalize()),
            extension,
            media_type,
            declared_extension_mismatch,
        });
    }
    let mut end = [0_u8; 1];
    reader
        .read_exact(&mut end)
        .map_err(|_| unsafe_archive("Risu module is missing its final record"))?;
    if end[0] != 0 {
        return Err(unsafe_archive("Risu module has an invalid final record"));
    }
    let mut trailing = [0_u8; 1];
    if reader.read(&mut trailing).map_err(storage_error)? != 0 {
        return Err(unsafe_archive("Risu module has trailing bytes"));
    }
    Ok(records)
}

fn validate_asset_signature(
    metadata: &DecodedRuntimeAssetMetadata,
    signature: &[u8],
) -> CoreResult<(String, String, bool)> {
    let (extension, media_type) = if signature.starts_with(b"\x89PNG\r\n\x1a\n") {
        ("png", "image/png")
    } else if signature.starts_with(&[0xff, 0xd8, 0xff]) {
        ("jpg", "image/jpeg")
    } else if signature.starts_with(b"GIF87a") || signature.starts_with(b"GIF89a") {
        ("gif", "image/gif")
    } else if signature.starts_with(b"RIFF") && signature.get(8..12) == Some(b"WEBP".as_slice()) {
        ("webp", "image/webp")
    } else {
        return Err(unsupported(format!(
            "Risu asset has an unsupported media signature: {} ({})",
            metadata.name, metadata.extension
        )));
    };
    let declared = match metadata.extension.as_str() {
        "jpeg" => "jpg",
        value => value,
    };
    Ok((
        extension.to_owned(),
        media_type.to_owned(),
        declared != extension,
    ))
}

pub(super) fn copy_decoded_asset<W: std::io::Write>(
    source: &mut File,
    record: &RisuAssetRecord,
    output: &mut W,
) -> CoreResult<()> {
    source
        .seek(SeekFrom::Start(record.offset))
        .map_err(storage_error)?;
    let mut remaining = record.size_bytes;
    let mut buffer = vec![0_u8; BUFFER_BYTES];
    while remaining > 0 {
        let wanted =
            usize::try_from(remaining.min(BUFFER_BYTES as u64)).expect("bounded buffer size");
        source
            .read_exact(&mut buffer[..wanted])
            .map_err(|_| unsafe_archive("Risu asset changed while preparing import"))?;
        decode_substitution_in_place(&mut buffer[..wanted])?;
        output.write_all(&buffer[..wanted]).map_err(storage_error)?;
        remaining -= wanted as u64;
    }
    Ok(())
}

pub(super) fn decode_risu_preset(path: &Path) -> CoreResult<Value> {
    let mut encoded = Vec::new();
    File::open(path)
        .map_err(storage_error)?
        .take(MAX_PRESET_CONTAINER_BYTES + 1)
        .read_to_end(&mut encoded)
        .map_err(storage_error)?;
    if encoded.is_empty() || encoded.len() as u64 > MAX_PRESET_CONTAINER_BYTES {
        return Err(unsupported(
            "Risu preset exceeds the 16 MiB container limit",
        ));
    }
    decode_substitution_in_place(&mut encoded)?;
    let mut decompressed = Vec::new();
    GzDecoder::new(encoded.as_slice())
        .take(MAX_PRESET_CONTAINER_BYTES + 1)
        .read_to_end(&mut decompressed)
        .map_err(|error| unsupported(format!("Risu preset compression is invalid: {error}")))?;
    if decompressed.len() as u64 > MAX_PRESET_CONTAINER_BYTES {
        return Err(unsupported("Risu preset container expands beyond 16 MiB"));
    }
    let outer = read_messagepack(&decompressed, "Risu preset container")?;
    if map_text(&outer, "type") != Some("preset") || map_u64(&outer, "presetVersion") != Some(2) {
        return Err(unsupported("Risu preset type or version is unsupported"));
    }
    let encrypted = map_value(&outer, "preset")
        .or_else(|| map_value(&outer, "pres"))
        .and_then(MessagePackValue::as_slice)
        .ok_or_else(|| unsupported("Risu preset has no encrypted payload"))?;
    if encrypted.len() > MAX_PRESET_DOCUMENT_BYTES {
        return Err(unsupported("Risu preset payload exceeds 16 MiB"));
    }
    let key = Sha256::digest(b"risupreset");
    let key = UnboundKey::new(&AES_256_GCM, key.as_slice())
        .map_err(|_| unsupported("Risu preset key could not be initialized"))?;
    let cipher = LessSafeKey::new(key);
    let mut plaintext = encrypted.to_vec();
    let plaintext = cipher
        .open_in_place(
            Nonce::assume_unique_for_key([0_u8; 12]),
            Aad::empty(),
            &mut plaintext,
        )
        .map_err(|_| unsupported("Risu preset authentication failed"))?;
    if plaintext.len() > MAX_PRESET_DOCUMENT_BYTES {
        return Err(unsupported("Risu preset document exceeds 16 MiB"));
    }
    let value = read_messagepack(plaintext, "Risu preset document")?;
    messagepack_to_json(&value, 0, &mut 0)
}

fn read_messagepack(bytes: &[u8], label: &str) -> CoreResult<MessagePackValue> {
    messagepack::decode(bytes, label, MAX_MESSAGEPACK_DEPTH, MAX_MESSAGEPACK_NODES)
}

fn messagepack_to_json(
    value: &MessagePackValue,
    depth: usize,
    nodes: &mut usize,
) -> CoreResult<Value> {
    if depth > MAX_MESSAGEPACK_DEPTH {
        return Err(unsupported("Risu preset nesting exceeds the limit"));
    }
    *nodes = nodes.saturating_add(1);
    if *nodes > MAX_MESSAGEPACK_NODES {
        return Err(unsupported("Risu preset structure exceeds the node limit"));
    }
    match value {
        MessagePackValue::Null | MessagePackValue::Binary(_) => Ok(Value::Null),
        MessagePackValue::Bool(value) => Ok(Value::Bool(*value)),
        MessagePackValue::I64(value) => Ok(Value::from(*value)),
        MessagePackValue::U64(value) => Ok(Value::from(*value)),
        MessagePackValue::F64(value) => Ok(Value::from(*value)),
        MessagePackValue::String(value) => Ok(Value::String(value.clone())),
        MessagePackValue::Array(values) => values
            .iter()
            .map(|value| messagepack_to_json(value, depth + 1, nodes))
            .collect::<CoreResult<Vec<_>>>()
            .map(Value::Array),
        MessagePackValue::Map(values) => {
            let mut map = Map::new();
            for (key, value) in values {
                let key = key
                    .as_str()
                    .ok_or_else(|| unsupported("Risu preset map key is not text"))?;
                if map.contains_key(key) {
                    return Err(unsupported("Risu preset contains duplicate map keys"));
                }
                map.insert(
                    key.to_owned(),
                    messagepack_to_json(value, depth + 1, nodes)?,
                );
            }
            Ok(Value::Object(map))
        }
    }
}

fn map_value<'a>(value: &'a MessagePackValue, key: &str) -> Option<&'a MessagePackValue> {
    value
        .as_map()?
        .iter()
        .find_map(|(candidate, value)| (candidate.as_str() == Some(key)).then_some(value))
}

fn map_text<'a>(value: &'a MessagePackValue, key: &str) -> Option<&'a str> {
    map_value(value, key)?.as_str()
}

fn map_u64(value: &MessagePackValue, key: &str) -> Option<u64> {
    map_value(value, key)?.as_u64()
}

fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}

fn unsafe_archive(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsafeArchive, message, false)
}

fn storage_error(error: std::io::Error) -> CoreError {
    CoreError::new(
        CoreErrorCode::StorageUnavailable,
        format!("cannot read or write Risu import data: {error}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use flate2::{Compression, write::GzEncoder};
    use serde_json::json;
    use tempfile::NamedTempFile;

    use super::*;

    fn encode_substitution(bytes: &[u8]) -> Vec<u8> {
        let mut decode_map = (0_u16..=255)
            .map(|value| u8::try_from(value).expect("byte"))
            .collect::<Vec<_>>();
        decode_substitution_in_place(&mut decode_map).expect("decode map");
        let mut inverse = [0_u8; 256];
        for (encoded, decoded) in decode_map.into_iter().enumerate() {
            inverse[usize::from(decoded)] = u8::try_from(encoded).expect("encoded byte");
        }
        bytes
            .iter()
            .map(|byte| inverse[usize::from(*byte)])
            .collect()
    }

    fn messagepack_bytes(value: &Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_messagepack(value, &mut bytes);
        bytes
    }

    fn write_messagepack(value: &Value, bytes: &mut Vec<u8>) {
        match value {
            Value::Null => bytes.push(0xc0),
            Value::Bool(false) => bytes.push(0xc2),
            Value::Bool(true) => bytes.push(0xc3),
            Value::Number(value) => {
                let value = value.as_u64().expect("fixture unsigned integer");
                if let Ok(value) = u8::try_from(value)
                    && value <= 0x7f
                {
                    bytes.push(value);
                } else {
                    bytes.push(0xcf);
                    bytes.extend_from_slice(&value.to_be_bytes());
                }
            }
            Value::String(value) => write_string(value, bytes),
            Value::Array(values) => {
                assert!(values.len() <= 15, "fixture array length");
                bytes.push(0x90 | u8::try_from(values.len()).expect("array length"));
                for value in values {
                    write_messagepack(value, bytes);
                }
            }
            Value::Object(values) => {
                assert!(values.len() <= 15, "fixture map length");
                bytes.push(0x80 | u8::try_from(values.len()).expect("map length"));
                for (key, value) in values {
                    write_string(key, bytes);
                    write_messagepack(value, bytes);
                }
            }
        }
    }

    fn write_string(value: &str, bytes: &mut Vec<u8>) {
        assert!(value.len() <= 31, "fixture string length");
        bytes.push(0xa0 | u8::try_from(value.len()).expect("string length"));
        bytes.extend_from_slice(value.as_bytes());
    }

    fn write_binary(value: &[u8], bytes: &mut Vec<u8>) {
        if let Ok(len) = u8::try_from(value.len()) {
            bytes.extend_from_slice(&[0xc4, len]);
        } else {
            bytes.push(0xc5);
            bytes.extend_from_slice(
                &u16::try_from(value.len())
                    .expect("fixture binary length")
                    .to_be_bytes(),
            );
        }
        bytes.extend_from_slice(value);
    }

    #[test]
    fn streams_module_assets_and_corrects_declared_media_type() {
        let metadata = serde_json::to_vec(&json!({
            "type": "risuModule",
            "module": {
                "name": "Synthetic Risu module",
                "assets": [["portrait.png", "asset-id", "png"]],
                "regex": [
                    {"type": "disabled", "in": "x", "out": "y"},
                    {"type": "editinput", "in": "a", "out": "b"}
                ],
                "trigger": [{
                    "lowLevelAccess": true,
                    "effect": [{"type": "risuaiLua", "code": "return 1"}]
                }]
            }
        }))
        .expect("metadata");
        let asset = b"RIFF\x04\x00\x00\x00WEBPsynthetic";
        let mut bytes = vec![111, 0];
        bytes.extend_from_slice(
            &u32::try_from(metadata.len())
                .expect("metadata length")
                .to_le_bytes(),
        );
        bytes.extend(encode_substitution(&metadata));
        bytes.push(1);
        bytes.extend_from_slice(
            &u32::try_from(asset.len())
                .expect("asset length")
                .to_le_bytes(),
        );
        bytes.extend(encode_substitution(asset));
        bytes.push(0);
        let mut source = NamedTempFile::new().expect("module source");
        source.write_all(&bytes).expect("write module");

        let module = read_risu_module(source.path(), ImportLimits::default(), &"a".repeat(64))
            .expect("read module");
        assert_eq!(module.metadata.name, "Synthetic Risu module");
        assert_eq!(module.metadata.profile.transforms.len(), 2);
        assert!(!module.metadata.profile.transforms[0].enabled);
        assert!(module.metadata.profile.scripts.is_empty());
        assert_eq!(module.assets.len(), 1);
        assert_eq!(module.assets[0].extension, "webp");
        assert_eq!(module.assets[0].media_type, "image/webp");
        assert!(module.assets[0].declared_extension_mismatch);
    }

    #[test]
    fn authenticates_and_decodes_a_version_two_preset() {
        let preset = json!({
            "name": "Synthetic preset",
            "promptTemplate": [{"type": "plain", "name": "System", "text": "Be helpful"}]
        });
        let mut plaintext = messagepack_bytes(&preset);
        let key = Sha256::digest(b"risupreset");
        let key = UnboundKey::new(&AES_256_GCM, key.as_slice()).expect("preset key");
        LessSafeKey::new(key)
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key([0_u8; 12]),
                Aad::empty(),
                &mut plaintext,
            )
            .expect("encrypt preset");
        let mut outer = vec![0x83];
        write_string("presetVersion", &mut outer);
        outer.push(2);
        write_string("type", &mut outer);
        write_string("preset", &mut outer);
        write_string("preset", &mut outer);
        write_binary(&plaintext, &mut outer);
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(&outer).expect("gzip preset");
        let encoded = encode_substitution(&gzip.finish().expect("finish gzip"));
        let mut source = NamedTempFile::new().expect("preset source");
        source.write_all(&encoded).expect("write preset");

        let decoded = decode_risu_preset(source.path()).expect("decode preset");
        assert_eq!(
            decoded.get("name").and_then(Value::as_str),
            Some("Synthetic preset")
        );
    }
}
