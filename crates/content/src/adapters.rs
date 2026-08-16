use lorepia_domain::{
    CharacterContentV1, CharacterKnowledgeBookRef, CoreError, CoreErrorCode, CoreResult,
    ExtensionQuarantine, ExtensionQuarantineKind, KnowledgeBookId, Sha256Digest,
    UnknownExtensionEntry, UnknownExtensionIndex,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_CHARACTER_NAME_BYTES: usize = 1_024;
pub(crate) const MAX_CHARACTER_NAME_CHARS: usize = 256;
pub(crate) const MAX_CHARACTER_DESCRIPTION_BYTES: usize = 256 * 1024;
pub(crate) const MAX_CHARACTER_DESCRIPTION_CHARS: usize = 64 * 1024;
pub(crate) const MAX_UNSUPPORTED_OPTIONAL_FIELDS: usize = 128;
pub(crate) const MAX_OPTIONAL_FIELD_KEY_BYTES: usize = 256;
pub(crate) const MAX_OPTIONAL_FIELD_KEY_CHARS: usize = 128;
const MAX_ALTERNATE_GREETINGS: usize = 128;
const MAX_EXAMPLE_DIALOGS: usize = 128;
const CHARACTER_CARD_V3_SPEC: &str = "chara_card_v3";

#[derive(Debug, Clone)]
pub(crate) struct CardMetadata {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) content: CharacterContentV1,
    pub(crate) unsupported_optional_fields: Vec<String>,
    pub(crate) len_bytes: u64,
}

#[cfg(test)]
pub(crate) fn parse_card_json(bytes: &[u8]) -> CoreResult<CardMetadata> {
    let source_sha256 = hex::encode(Sha256::digest(bytes));
    parse_card_json_with_source(bytes, &source_sha256)
}

pub(crate) fn parse_card_json_with_source(
    bytes: &[u8],
    source_sha256: &str,
) -> CoreResult<CardMetadata> {
    if bytes.is_empty() {
        return Err(unsupported("character metadata is empty"));
    }
    if bytes.len() > MAX_METADATA_BYTES {
        return Err(unsupported("character metadata exceeds 4 MiB"));
    }

    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| unsupported(format!("invalid character JSON: {error}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| unsupported("character metadata must be a JSON object"))?;
    let spec = object
        .get("spec")
        .and_then(Value::as_str)
        .ok_or_else(|| unsupported("character metadata must declare a string spec"))?;
    if spec != CHARACTER_CARD_V3_SPEC {
        return Err(unsupported(format!("unsupported character spec: {spec}")));
    }

    let data = object
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| unsupported("CCv3 metadata must contain a data object"))?;
    let name = data
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| unsupported("CCv3 metadata must contain a string data.name"))?
        .trim();
    if name.is_empty() {
        return Err(unsupported("CCv3 data.name must not be empty"));
    }
    validate_metadata_text(
        "data.name",
        name,
        MAX_CHARACTER_NAME_BYTES,
        MAX_CHARACTER_NAME_CHARS,
    )?;

    let (description_field, description) =
        if let Some(description) = data.get("description").and_then(Value::as_str) {
            (Some("description"), description.trim())
        } else if let Some(personality) = data.get("personality").and_then(Value::as_str) {
            (Some("personality"), personality.trim())
        } else {
            (None, "")
        };
    validate_metadata_text(
        "data.description",
        description,
        MAX_CHARACTER_DESCRIPTION_BYTES,
        MAX_CHARACTER_DESCRIPTION_CHARS,
    )?;
    let content = parse_character_content(data, source_sha256)?;
    let unsupported_optional_fields =
        collect_unsupported_optional_fields(data.keys(), description_field)?;

    Ok(CardMetadata {
        name: name.to_owned(),
        description: description.to_owned(),
        content,
        unsupported_optional_fields,
        len_bytes: bytes.len() as u64,
    })
}

fn collect_unsupported_optional_fields<'a>(
    keys: impl Iterator<Item = &'a String>,
    description_field: Option<&str>,
) -> CoreResult<Vec<String>> {
    let mut fields = Vec::new();
    for key in keys {
        if is_supported_character_field(key) || description_field == Some(key.as_str()) {
            continue;
        }
        if key.is_empty()
            || key.chars().any(char::is_control)
            || key.len() > MAX_OPTIONAL_FIELD_KEY_BYTES
            || key.chars().count() > MAX_OPTIONAL_FIELD_KEY_CHARS
        {
            return Err(unsupported(format!(
                "CCv3 optional data field name must be non-empty, printable, and at most \
                 {MAX_OPTIONAL_FIELD_KEY_BYTES} bytes or {MAX_OPTIONAL_FIELD_KEY_CHARS} characters"
            )));
        }
        if fields.len() == MAX_UNSUPPORTED_OPTIONAL_FIELDS {
            return Err(unsupported(format!(
                "CCv3 metadata has more than {MAX_UNSUPPORTED_OPTIONAL_FIELDS} unsupported \
                 optional data fields"
            )));
        }
        fields.push(key.clone());
    }
    fields.sort();
    fields.dedup();
    Ok(fields)
}

fn parse_character_content(
    data: &serde_json::Map<String, Value>,
    source_sha256: &str,
) -> CoreResult<CharacterContentV1> {
    let personality = optional_text(data, "personality")?;
    let scenario = optional_text(data, "scenario")?;
    let first_message = optional_text(data, "first_mes")?;
    let system_instruction = optional_text(data, "system_prompt")?;
    let post_history_instruction = optional_text(data, "post_history_instructions")?;
    let example_dialogs = optional_text_list(data, "mes_example", MAX_EXAMPLE_DIALOGS)?;
    let alternate_greetings =
        optional_text_list(data, "alternate_greetings", MAX_ALTERNATE_GREETINGS)?;
    let knowledge_book = parse_knowledge_book(data.get("character_book"))?;
    let unknown_extensions = collect_unknown_extensions(data, source_sha256)?;

    Ok(CharacterContentV1 {
        personality,
        scenario,
        first_message,
        example_dialogs,
        system_instruction,
        post_history_instruction,
        alternate_greetings,
        knowledge_book,
        assets: Vec::new(),
        unknown_extensions,
        ..CharacterContentV1::default()
    })
}

fn optional_text(data: &serde_json::Map<String, Value>, key: &str) -> CoreResult<String> {
    match data.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => {
            validate_metadata_text(
                &format!("data.{key}"),
                value,
                MAX_CHARACTER_DESCRIPTION_BYTES,
                MAX_CHARACTER_DESCRIPTION_CHARS,
            )?;
            Ok(value.clone())
        }
        Some(_) => Err(unsupported(format!(
            "CCv3 data.{key} must be a string or null"
        ))),
    }
}

fn optional_text_list(
    data: &serde_json::Map<String, Value>,
    key: &str,
    max_items: usize,
) -> CoreResult<Vec<String>> {
    let Some(value) = data.get(key) else {
        return Ok(Vec::new());
    };
    let values = match value {
        Value::Null => Vec::new(),
        Value::String(value) if value.is_empty() => Vec::new(),
        Value::String(value) => vec![value.clone()],
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| unsupported(format!("CCv3 data.{key} entries must be strings")))
            })
            .collect::<CoreResult<Vec<_>>>()?,
        _ => {
            return Err(unsupported(format!(
                "CCv3 data.{key} must be a string, string array, or null"
            )));
        }
    };
    if values.len() > max_items {
        return Err(unsupported(format!(
            "CCv3 data.{key} exceeds the {max_items}-item limit"
        )));
    }
    for value in &values {
        validate_metadata_text(
            &format!("data.{key} entry"),
            value,
            MAX_CHARACTER_DESCRIPTION_BYTES,
            MAX_CHARACTER_DESCRIPTION_CHARS,
        )?;
    }
    Ok(values)
}

fn parse_knowledge_book(value: Option<&Value>) -> CoreResult<Option<CharacterKnowledgeBookRef>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .ok_or_else(|| unsupported("CCv3 data.character_book must be an object or null"))?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    if let Some(name) = &name {
        validate_metadata_text(
            "data.character_book.name",
            name,
            MAX_CHARACTER_NAME_BYTES,
            MAX_CHARACTER_NAME_CHARS,
        )?;
    }
    let bytes = serde_json::to_vec(value)
        .map_err(|error| unsupported(format!("cannot normalize character_book: {error}")))?;
    let source_sha256 =
        Sha256Digest::parse(hex::encode(Sha256::digest(&bytes))).map_err(unsupported)?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(KnowledgeBookId::from);
    Ok(Some(CharacterKnowledgeBookRef {
        id,
        name,
        source_sha256: Some(source_sha256),
    }))
}

fn collect_unknown_extensions(
    data: &serde_json::Map<String, Value>,
    source_sha256: &str,
) -> CoreResult<UnknownExtensionIndex> {
    let raw_source_sha256 = Sha256Digest::parse(source_sha256.to_owned()).map_err(unsupported)?;
    let mut entries = Vec::new();
    for (key, value) in data {
        if is_supported_character_field(key) {
            if key == "assets" {
                collect_external_asset_references(value, &mut entries)?;
            }
            continue;
        }
        if key == "extensions" {
            let extensions = value.as_object().ok_or_else(|| {
                unsupported("CCv3 data.extensions must be an object when present")
            })?;
            for (extension_key, extension_value) in extensions {
                push_unknown_extension(
                    extension_key,
                    &format!("/data/extensions/{}", escape_json_pointer(extension_key)),
                    extension_value,
                    &mut entries,
                )?;
            }
        } else {
            push_unknown_extension(
                key,
                &format!("/data/{}", escape_json_pointer(key)),
                value,
                &mut entries,
            )?;
        }
    }
    UnknownExtensionIndex::try_new(Some(raw_source_sha256), entries).map_err(unsupported)
}

fn collect_external_asset_references(
    value: &Value,
    entries: &mut Vec<UnknownExtensionEntry>,
) -> CoreResult<()> {
    let Some(assets) = value.as_array() else {
        if value.is_null() {
            return Ok(());
        }
        return Err(unsupported("CCv3 data.assets must be an array or null"));
    };
    if assets.len() > MAX_UNSUPPORTED_OPTIONAL_FIELDS {
        return Err(unsupported(format!(
            "CCv3 data.assets exceeds the {MAX_UNSUPPORTED_OPTIONAL_FIELDS}-item limit"
        )));
    }
    for (index, asset) in assets.iter().enumerate() {
        let Some(uri) = asset
            .as_object()
            .and_then(|object| object.get("uri"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if is_external_url(uri) {
            push_unknown_extension(
                "external_asset_url",
                &format!("/data/assets/{index}/uri"),
                &Value::String(uri.to_owned()),
                entries,
            )?;
        }
    }
    Ok(())
}

fn push_unknown_extension(
    key: &str,
    source_path: &str,
    value: &Value,
    entries: &mut Vec<UnknownExtensionEntry>,
) -> CoreResult<()> {
    validate_optional_key(key)?;
    let bytes = serde_json::to_vec(value)
        .map_err(|error| unsupported(format!("cannot index CCv3 extension: {error}")))?;
    let sha256 = Sha256Digest::parse(hex::encode(Sha256::digest(&bytes))).map_err(unsupported)?;
    let quarantine = classify_extension(key, value)
        .map(|(kind, reason)| ExtensionQuarantine::inactive(kind, reason));
    entries.push(UnknownExtensionEntry {
        key: key.to_owned(),
        source_path: source_path.to_owned(),
        sha256,
        size_bytes: bytes.len() as u64,
        quarantine,
    });
    Ok(())
}

fn classify_extension(key: &str, value: &Value) -> Option<(ExtensionQuarantineKind, &'static str)> {
    let lower_key = key.to_ascii_lowercase();
    if lower_key.contains("script") || lower_key == "javascript" {
        return Some((
            ExtensionQuarantineKind::Script,
            "script extension is preserved but inactive",
        ));
    }
    if lower_key == "html" || lower_key.ends_with("_html") {
        return Some((
            ExtensionQuarantineKind::Html,
            "HTML extension is preserved but inactive",
        ));
    }
    if lower_key == "code" || lower_key.ends_with("_code") {
        return Some((
            ExtensionQuarantineKind::Code,
            "code extension is preserved but inactive",
        ));
    }
    if value_contains_external_url(value) {
        return Some((
            ExtensionQuarantineKind::ExternalUrl,
            "external URL is preserved but never fetched automatically",
        ));
    }
    if value_contains_active_markup(value) {
        return Some((
            ExtensionQuarantineKind::Html,
            "active markup is preserved but inactive",
        ));
    }
    None
}

fn value_contains_external_url(value: &Value) -> bool {
    match value {
        Value::String(value) => is_external_url(value),
        Value::Array(values) => values.iter().any(value_contains_external_url),
        Value::Object(values) => values.values().any(value_contains_external_url),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn value_contains_active_markup(value: &Value) -> bool {
    match value {
        Value::String(value) => {
            let lower = value.to_ascii_lowercase();
            lower.contains("<script")
                || lower.contains("<iframe")
                || lower.contains("javascript:")
                || lower.contains("data:text/html")
        }
        Value::Array(values) => values.iter().any(value_contains_active_markup),
        Value::Object(values) => values.values().any(value_contains_active_markup),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn is_external_url(value: &str) -> bool {
    let lower = value.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("//")
}

fn is_supported_character_field(key: &str) -> bool {
    matches!(
        key,
        "name"
            | "description"
            | "personality"
            | "scenario"
            | "first_mes"
            | "mes_example"
            | "system_prompt"
            | "post_history_instructions"
            | "alternate_greetings"
            | "character_book"
            | "assets"
    )
}

fn validate_optional_key(key: &str) -> CoreResult<()> {
    if key.is_empty()
        || key.chars().any(char::is_control)
        || key.len() > MAX_OPTIONAL_FIELD_KEY_BYTES
        || key.chars().count() > MAX_OPTIONAL_FIELD_KEY_CHARS
    {
        return Err(unsupported(format!(
            "CCv3 optional data field name must be non-empty, printable, and at most \
             {MAX_OPTIONAL_FIELD_KEY_BYTES} bytes or {MAX_OPTIONAL_FIELD_KEY_CHARS} characters"
        )));
    }
    Ok(())
}

fn escape_json_pointer(value: &str) -> String {
    value.replace('~', "~0").replace('/', "~1")
}

fn validate_metadata_text(
    field: &str,
    value: &str,
    max_bytes: usize,
    max_chars: usize,
) -> CoreResult<()> {
    if value.len() > max_bytes || value.chars().count() > max_chars {
        return Err(unsupported(format!(
            "CCv3 {field} exceeds the {max_bytes}-byte or {max_chars}-character limit"
        )));
    }
    Ok(())
}

fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_unsupported(bytes: &[u8]) {
        let error = parse_card_json(bytes).expect_err("metadata must be rejected");
        assert_eq!(error.code, CoreErrorCode::UnsupportedContent);
        assert!(!error.recoverable);
    }

    #[test]
    fn requires_an_object_with_v3_spec_data_and_name() {
        for bytes in [
            b"".as_slice(),
            b"null".as_slice(),
            br"[]".as_slice(),
            br"{}".as_slice(),
            br#"{"spec":3,"data":{"name":"Segu"}}"#.as_slice(),
            br#"{"spec":"chara_card_v2","data":{"name":"Segu"}}"#.as_slice(),
            br#"{"spec":"chara_card_v3"}"#.as_slice(),
            br#"{"spec":"chara_card_v3","data":[]}"#.as_slice(),
            br#"{"spec":"chara_card_v3","data":{}}"#.as_slice(),
            br#"{"spec":"chara_card_v3","data":{"name":3}}"#.as_slice(),
            br#"{"spec":"chara_card_v3","data":{"name":"  "}}"#.as_slice(),
        ] {
            assert_unsupported(bytes);
        }
    }

    #[test]
    fn parses_required_and_optional_fields() {
        let metadata = parse_card_json(
            br#"{"spec":"chara_card_v3","data":{"name":" Segu ","description":" Guide "}}"#,
        )
        .expect("valid CCv3 metadata");

        assert_eq!(metadata.name, "Segu");
        assert_eq!(metadata.description, "Guide");
        assert!(metadata.unsupported_optional_fields.is_empty());
    }

    #[test]
    fn reports_sorted_unsupported_fields_and_excludes_only_consumed_text() {
        let description = parse_card_json(
            br#"{
                "spec":"chara_card_v3",
                "data":{
                    "name":"Segu",
                    "description":"Guide",
                    "personality":"Unused fallback",
                    "z_unknown":true,
                    "alternate_greetings":[],
                    "creator":"Synthetic"
                }
            }"#,
        )
        .expect("valid CCv3 metadata");
        assert_eq!(
            description.unsupported_optional_fields,
            ["creator", "z_unknown"]
        );

        let fallback = parse_card_json(
            br#"{
                "spec":"chara_card_v3",
                "data":{
                    "name":"Segu",
                    "description":null,
                    "personality":"Fallback",
                    "scenario":"Synthetic"
                }
            }"#,
        )
        .expect("valid fallback metadata");
        assert_eq!(fallback.description, "Fallback");
        assert!(fallback.unsupported_optional_fields.is_empty());

        let duplicate = parse_card_json(
            br#"{
                "spec":"chara_card_v3",
                "data":{
                    "name":"Segu",
                    "creator":"First",
                    "creator":"Last"
                }
            }"#,
        )
        .expect("duplicate optional keys remain bounded");
        assert_eq!(duplicate.unsupported_optional_fields, ["creator"]);
    }

    #[test]
    fn bounds_optional_field_names_and_count() {
        let oversized_key = "k".repeat(MAX_OPTIONAL_FIELD_KEY_BYTES + 1);
        let oversized = serde_json::json!({
            "spec": CHARACTER_CARD_V3_SPEC,
            "data": {
                "name": "Segu",
                oversized_key: true,
            }
        });
        assert_unsupported(&serde_json::to_vec(&oversized).expect("encode"));

        let mut data = serde_json::Map::new();
        data.insert("name".to_owned(), Value::String("Segu".to_owned()));
        for index in 0..=MAX_UNSUPPORTED_OPTIONAL_FIELDS {
            data.insert(format!("optional_{index:03}"), Value::Bool(true));
        }
        let too_many = serde_json::json!({
            "spec": CHARACTER_CARD_V3_SPEC,
            "data": data,
        });
        assert_unsupported(&serde_json::to_vec(&too_many).expect("encode"));

        let control_key = serde_json::json!({
            "spec": CHARACTER_CARD_V3_SPEC,
            "data": {
                "name": "Segu",
                "unsafe\nlabel": true,
            }
        });
        assert_unsupported(&serde_json::to_vec(&control_key).expect("encode"));
    }

    #[test]
    fn metadata_limits_are_inclusive_at_multibyte_utf8_boundaries() {
        let name = "😀".repeat(MAX_CHARACTER_NAME_CHARS);
        assert_eq!(name.len(), MAX_CHARACTER_NAME_BYTES);
        let description = "😀".repeat(MAX_CHARACTER_DESCRIPTION_CHARS);
        assert_eq!(description.len(), MAX_CHARACTER_DESCRIPTION_BYTES);
        let json = serde_json::json!({
            "spec": CHARACTER_CARD_V3_SPEC,
            "data": {
                "name": name,
                "description": description,
            }
        });

        let metadata =
            parse_card_json(&serde_json::to_vec(&json).expect("encode")).expect("exact limits");
        assert_eq!(metadata.name.chars().count(), MAX_CHARACTER_NAME_CHARS);
        assert_eq!(
            metadata.description.chars().count(),
            MAX_CHARACTER_DESCRIPTION_CHARS
        );
    }

    #[test]
    fn metadata_limits_reject_one_complete_multibyte_scalar_over_the_boundary() {
        let oversized_name = "😀".repeat(MAX_CHARACTER_NAME_CHARS + 1);
        let name_json = serde_json::json!({
            "spec": CHARACTER_CARD_V3_SPEC,
            "data": {"name": oversized_name}
        });
        let name_error =
            parse_card_json(&serde_json::to_vec(&name_json).expect("encode")).expect_err("name");
        assert_eq!(name_error.code, CoreErrorCode::UnsupportedContent);
        assert_eq!(
            name_error.message,
            "CCv3 data.name exceeds the 1024-byte or 256-character limit"
        );

        let oversized_description = "😀".repeat(MAX_CHARACTER_DESCRIPTION_CHARS + 1);
        let description_json = serde_json::json!({
            "spec": CHARACTER_CARD_V3_SPEC,
            "data": {
                "name": "Segu",
                "description": oversized_description,
            }
        });
        let description_error =
            parse_card_json(&serde_json::to_vec(&description_json).expect("encode"))
                .expect_err("description");
        assert_eq!(description_error.code, CoreErrorCode::UnsupportedContent);
        assert_eq!(
            description_error.message,
            "CCv3 data.description exceeds the 262144-byte or 65536-character limit"
        );
    }
}
