//! Structural decoder for portable runtime documents embedded in card archives.
//!
//! The container is recognized from its two-byte header and bounded length
//! records. File names and source application labels are deliberately ignored.

use std::collections::BTreeMap;

use lorepia_domain::{
    CharacterRuntimeProfile, CoreError, CoreErrorCode, CoreResult, PortableRuntimeScript,
    PortableTextTransform, PortableTransformPhase,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

const HEADER_BYTES: usize = 6;
const MAGIC: u8 = 111;
const VERSION: u8 = 0;
const RECORD_ASSET: u8 = 1;
const RECORD_END: u8 = 0;
pub(crate) const MAX_RUNTIME_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;

// MIT-licensed byte substitution table, copyright (c) 2026 Kwaroran.
// The format has no algorithmic key negotiation; this fixed inverse table is
// part of the on-disk interoperability contract.
const DECODE_MAP_HEX: &str = concat!(
    "2cf7848bc965fbb69faeb3032d0169741fe4a3ecee5c3421934a0f6ae262029",
    "e229cfd3cfc71c7c6ad596705706d8a4412fa24865fafd17a47cefe5063dd510",
    "66f18e052a8099d56734cb8536cc3a00e19cf3e0d7e07326846ea48f9992eaba",
    "449205e5535380cbcd3b1581679280a1ae1f2cdc439dba2ba6072767d95ef7fc",
    "8c0de3794bfb51481922545ace7f566a72b365ac113e34b3ae88d831b7c27b09",
    "a42eb87aadc548e7826d25729d4b7f82f8f8975f04177c21effd81511e504971",
    "7f331d09b00d7cab44f2a3bd9b26bda5da13f3061bd913d4ee6dfbe4d828c1d",
    "23109864f485337b9043bba988f1d6a51cf6cc6eb95b0b96edd5e9c5cb08a68040"
);

pub(crate) struct DecodedRuntimeDocument {
    pub(crate) profile: CharacterRuntimeProfile,
    pub(crate) knowledge_entries: Option<Value>,
    pub(crate) embedded_assets: Vec<DecodedRuntimeAsset>,
}

pub(crate) struct DecodedRuntimeAsset {
    pub(crate) name: String,
    pub(crate) bytes: Vec<u8>,
}

pub(crate) fn has_runtime_header(bytes: &[u8]) -> bool {
    bytes.get(..2) == Some(&[MAGIC, VERSION])
}

pub(crate) fn decode_runtime_document(
    bytes: &[u8],
    source_sha256: &str,
) -> CoreResult<DecodedRuntimeDocument> {
    if bytes.len() < HEADER_BYTES || !has_runtime_header(bytes) {
        return Err(unsupported("runtime document has an invalid header"));
    }
    if bytes.len() > MAX_RUNTIME_DOCUMENT_BYTES {
        return Err(unsupported("runtime document exceeds the 16 MiB limit"));
    }
    let main_len = usize::try_from(u32::from_le_bytes(
        bytes[2..6].try_into().expect("four-byte length"),
    ))
    .map_err(|_| unsupported("runtime document length does not fit this device"))?;
    let main_end = HEADER_BYTES
        .checked_add(main_len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| unsupported("runtime document metadata is truncated"))?;
    let decoded = decode_substitution(&bytes[HEADER_BYTES..main_end])?;
    let root: Value = serde_json::from_slice(&decoded)
        .map_err(|error| unsupported(format!("runtime document metadata is invalid: {error}")))?;
    let module = root
        .get("module")
        .and_then(Value::as_object)
        .ok_or_else(|| unsupported("runtime document has no module object"))?;

    let embedded_asset_bytes = decode_runtime_records(bytes, main_end)?;

    let embedded_assets = parse_embedded_assets(module.get("assets"), embedded_asset_bytes)?;
    let source_id = format!("card-runtime:{source_sha256}");
    let transforms = parse_transforms(module.get("regex"), &source_id)?;
    let scripts = parse_scripts(module.get("trigger"), &source_id)?;
    let knowledge_entries = module.get("lorebook").cloned();
    let mut metadata = BTreeMap::new();
    for key in [
        "name",
        "description",
        "namespace",
        "customModuleToggle",
        "backgroundEmbedding",
        "lowLevelAccess",
        "hideIcon",
        "assets",
    ] {
        if let Some(value) = module.get(key) {
            metadata.insert(key.to_owned(), canonical_json(value)?);
        }
    }
    Ok(DecodedRuntimeDocument {
        profile: CharacterRuntimeProfile {
            source_id: Some(source_id),
            transform_set_id: None,
            transforms,
            scripts,
            background_markup: module
                .get("backgroundEmbedding")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            additional_text: String::new(),
            toggle_schema: module
                .get("customModuleToggle")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            initial_variables: BTreeMap::new(),
            metadata,
        },
        knowledge_entries,
        embedded_assets,
    })
}

fn decode_runtime_records(bytes: &[u8], mut cursor: usize) -> CoreResult<Vec<Vec<u8>>> {
    let mut assets = Vec::new();
    let mut terminated = false;
    while cursor < bytes.len() {
        let record_type = bytes[cursor];
        cursor += 1;
        match record_type {
            RECORD_END => {
                terminated = true;
                break;
            }
            RECORD_ASSET => {
                let length_end = cursor
                    .checked_add(4)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| unsupported("runtime asset length is truncated"))?;
                let asset_len = usize::try_from(u32::from_le_bytes(
                    bytes[cursor..length_end]
                        .try_into()
                        .expect("four-byte length"),
                ))
                .map_err(|_| unsupported("runtime asset length does not fit this device"))?;
                cursor = length_end;
                let asset_end = cursor
                    .checked_add(asset_len)
                    .filter(|end| *end <= bytes.len())
                    .ok_or_else(|| unsupported("runtime asset is truncated"))?;
                assets.push(decode_substitution(&bytes[cursor..asset_end])?);
                cursor = asset_end;
            }
            _ => return Err(unsupported("runtime document contains an unknown record")),
        }
    }
    if !terminated || cursor != bytes.len() {
        return Err(unsupported(
            "runtime document is missing its final record or has trailing bytes",
        ));
    }
    Ok(assets)
}

fn parse_embedded_assets(
    value: Option<&Value>,
    bytes: Vec<Vec<u8>>,
) -> CoreResult<Vec<DecodedRuntimeAsset>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let metadata = value
        .and_then(Value::as_array)
        .ok_or_else(|| unsupported("runtime assets have no metadata array"))?;
    if metadata.len() != bytes.len() {
        return Err(unsupported(
            "runtime asset metadata count does not match its binary records",
        ));
    }
    metadata
        .iter()
        .zip(bytes)
        .enumerate()
        .map(|(index, (metadata, bytes))| {
            let fields = metadata
                .as_array()
                .ok_or_else(|| unsupported("runtime asset metadata must be an array"))?;
            let name = fields
                .first()
                .and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty())
                .map_or_else(|| format!("embedded-asset-{index}"), str::to_owned);
            if name.len() > 1_024 || name.chars().any(char::is_control) {
                return Err(unsupported("runtime asset name is invalid"));
            }
            Ok(DecodedRuntimeAsset { name, bytes })
        })
        .collect()
}

fn parse_transforms(
    value: Option<&Value>,
    source_id: &str,
) -> CoreResult<Vec<PortableTextTransform>> {
    let Some(Value::Array(values)) = value else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let object = value
                .as_object()
                .ok_or_else(|| unsupported("runtime transform must be an object"))?;
            let kind = object
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let phase = match kind {
                "editprocess" => PortableTransformPhase::RequestContext,
                "editoutput" => PortableTransformPhase::ProviderOutput,
                "editdisplay" => PortableTransformPhase::Display,
                _ => {
                    return Err(unsupported(format!(
                        "unsupported runtime transform type: {kind}"
                    )));
                }
            };
            let pattern = object
                .get("in")
                .and_then(Value::as_str)
                .ok_or_else(|| unsupported("runtime transform has no string pattern"))?
                .to_owned();
            let replacement = object
                .get("out")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let name = object
                .get("comment")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let flags = object
                .get("flag")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let mut digest = Sha256::new();
            digest.update(b"portable-text-transform-v1\0");
            digest.update(source_id.as_bytes());
            digest.update([0]);
            digest.update(index.to_le_bytes());
            Ok(PortableTextTransform {
                id: format!("card-transform:{}", hex::encode(digest.finalize())),
                name,
                phase,
                enabled: !object
                    .get("disabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                pattern,
                replacement,
                flags,
                metadata: BTreeMap::from([("source_kind".to_owned(), kind.to_owned())]),
            })
        })
        .collect()
}

fn parse_scripts(value: Option<&Value>, source_id: &str) -> CoreResult<Vec<PortableRuntimeScript>> {
    let Some(Value::Array(triggers)) = value else {
        return Ok(Vec::new());
    };
    let mut scripts = Vec::new();
    for (trigger_index, trigger) in triggers.iter().enumerate() {
        let trigger = trigger
            .as_object()
            .ok_or_else(|| unsupported("runtime trigger must be an object"))?;
        let event = trigger
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("start")
            .to_owned();
        let elevated_access = trigger
            .get("lowLevelAccess")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let effects = trigger
            .get("effect")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for (effect_index, effect) in effects.iter().enumerate() {
            let effect = effect
                .as_object()
                .ok_or_else(|| unsupported("runtime trigger effect must be an object"))?;
            let Some(source) = effect.get("code").and_then(Value::as_str) else {
                continue;
            };
            let kind = effect
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let language = if kind.ends_with("lua") { "lua" } else { kind };
            let mut digest = Sha256::new();
            digest.update(b"portable-runtime-script-v1\0");
            digest.update(source_id.as_bytes());
            digest.update([0]);
            digest.update(trigger_index.to_le_bytes());
            digest.update(effect_index.to_le_bytes());
            scripts.push(PortableRuntimeScript {
                id: format!("card-script:{}", hex::encode(digest.finalize())),
                name: trigger
                    .get("comment")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                event: event.clone(),
                language: language.to_owned(),
                source: source.to_owned(),
                elevated_access,
                metadata: BTreeMap::from([("source_kind".to_owned(), kind.to_owned())]),
            });
        }
    }
    Ok(scripts)
}

fn decode_substitution(bytes: &[u8]) -> CoreResult<Vec<u8>> {
    let map = hex::decode(DECODE_MAP_HEX)
        .map_err(|error| unsupported(format!("runtime decode table is invalid: {error}")))?;
    if map.len() != 256 {
        return Err(unsupported("runtime decode table has an invalid length"));
    }
    Ok(bytes.iter().map(|byte| map[usize::from(*byte)]).collect())
}

fn canonical_json(value: &Value) -> CoreResult<String> {
    serde_json::to_string(value)
        .map_err(|error| unsupported(format!("cannot normalize runtime metadata: {error}")))
}

fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode_substitution(bytes: &[u8]) -> Vec<u8> {
        let map = hex::decode(DECODE_MAP_HEX).expect("decode table");
        let mut inverse = [0_u8; 256];
        for (encoded, decoded) in map.into_iter().enumerate() {
            inverse[usize::from(decoded)] = u8::try_from(encoded).expect("byte index");
        }
        bytes
            .iter()
            .map(|byte| inverse[usize::from(*byte)])
            .collect()
    }

    #[test]
    fn header_detection_is_structural() {
        assert!(has_runtime_header(&[MAGIC, VERSION, 0, 0, 0, 0]));
        assert!(!has_runtime_header(b"not a module"));
    }

    #[test]
    fn binary_asset_records_are_paired_with_their_declared_names() {
        let metadata = serde_json::to_vec(&serde_json::json!({
            "module": {
                "name": "Portable module",
                "assets": [["portrait.png", "stored-id", "image"]]
            }
        }))
        .expect("encode metadata");
        let asset = b"\x89PNG\r\n\x1a\nportable-payload";
        let mut document = vec![MAGIC, VERSION];
        document.extend_from_slice(
            &u32::try_from(metadata.len())
                .expect("metadata length")
                .to_le_bytes(),
        );
        document.extend(encode_substitution(&metadata));
        document.push(RECORD_ASSET);
        document.extend_from_slice(
            &u32::try_from(asset.len())
                .expect("asset length")
                .to_le_bytes(),
        );
        document.extend(encode_substitution(asset));
        document.push(RECORD_END);

        let decoded =
            decode_runtime_document(&document, &"a".repeat(64)).expect("decode runtime document");
        assert_eq!(decoded.embedded_assets.len(), 1);
        assert_eq!(decoded.embedded_assets[0].name, "portrait.png");
        assert_eq!(decoded.embedded_assets[0].bytes, asset);
    }
}
