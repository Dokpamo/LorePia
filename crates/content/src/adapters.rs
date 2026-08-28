use std::collections::BTreeMap;

use lorepia_domain::{
    CharacterContentV1, CharacterKnowledgeBookRef, CharacterRuntimeProfile, CoreError,
    CoreErrorCode, CoreResult, ExtensionQuarantine, ExtensionQuarantineKind, KnowledgeBookId,
    PortableKnowledgeBook, PortableKnowledgeEntry, PortableKnowledgePlacement,
    PortableRuntimeScript, Sha256Digest, UnknownExtensionEntry, UnknownExtensionIndex,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_CHARACTER_NAME_BYTES: usize = 1_024;
pub(crate) const MAX_CHARACTER_NAME_CHARS: usize = 256;
pub(crate) const MAX_CHARACTER_DESCRIPTION_BYTES: usize = 256 * 1024;
pub(crate) const MAX_CHARACTER_DESCRIPTION_CHARS: usize = 64 * 1024;
pub(crate) const MAX_UNSUPPORTED_OPTIONAL_FIELDS: usize = 128;
pub(crate) const MAX_CARD_ASSET_REFERENCES: usize = 8_192;
pub(crate) const MAX_OPTIONAL_FIELD_KEY_BYTES: usize = 256;
pub(crate) const MAX_OPTIONAL_FIELD_KEY_CHARS: usize = 128;
const MAX_ALTERNATE_GREETINGS: usize = 128;
const MAX_EXAMPLE_DIALOGS: usize = 128;
const MAX_KNOWLEDGE_ENTRIES: usize = 4_096;
const MAX_KNOWLEDGE_ENTRY_BYTES: usize = 256 * 1_024;
const MAX_KNOWLEDGE_ENTRY_CHARS: usize = 64 * 1_024;
const CHARACTER_CARD_V3_SPEC: &str = "chara_card_v3";
const CHARACTER_CARD_V2_SPEC: &str = "chara_card_v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NonPortableContentPolicy {
    PreserveForRoundTrip,
    Omit,
}

#[derive(Debug, Clone)]
pub(crate) struct CardMetadata {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) content: CharacterContentV1,
    pub(crate) unsupported_optional_fields: Vec<String>,
    pub(crate) preferred_image_path: Option<String>,
    pub(crate) len_bytes: u64,
    /// True when the source declared the V2 spec and was promoted to the
    /// canonical V3 shape. The reviewer is told before anything is committed.
    pub(crate) promoted_from_v2: bool,
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
    parse_card_json_with_source_and_policy(
        bytes,
        source_sha256,
        NonPortableContentPolicy::PreserveForRoundTrip,
    )
}

pub(crate) fn parse_card_json_with_source_and_policy(
    bytes: &[u8],
    source_sha256: &str,
    nonportable_policy: NonPortableContentPolicy,
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
    // V2 and V3 share the `data` field names this adapter consumes, so a V2
    // card is promoted by parsing it through the same canonical path. Fields
    // that only V3 defines are optional and simply stay empty; anything the
    // adapter does not consume still lands in the unknown-extension quarantine.
    let promoted_from_v2 = match spec {
        CHARACTER_CARD_V3_SPEC => false,
        CHARACTER_CARD_V2_SPEC => true,
        _ => return Err(unsupported(format!("unsupported character spec: {spec}"))),
    };

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
    let preferred_image_path = preferred_embedded_image_path(data.get("assets"))?;
    let content = parse_character_content(data, source_sha256, nonportable_policy)?;
    let unsupported_optional_fields =
        collect_unsupported_optional_fields(data.keys(), description_field)?;

    Ok(CardMetadata {
        name: name.to_owned(),
        description: description.to_owned(),
        content,
        unsupported_optional_fields,
        preferred_image_path,
        len_bytes: bytes.len() as u64,
        promoted_from_v2,
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
    nonportable_policy: NonPortableContentPolicy,
) -> CoreResult<CharacterContentV1> {
    let personality = optional_text(data, "personality")?;
    let scenario = optional_text(data, "scenario")?;
    let first_message = optional_text(data, "first_mes")?;
    let system_instruction = optional_text(data, "system_prompt")?;
    let post_history_instruction = optional_text(data, "post_history_instructions")?;
    let example_dialogs = optional_text_list(data, "mes_example", MAX_EXAMPLE_DIALOGS)?;
    let alternate_greetings =
        optional_text_list(data, "alternate_greetings", MAX_ALTERNATE_GREETINGS)?;
    let knowledge_book = parse_knowledge_book(data.get("character_book"), source_sha256)?;
    let runtime = parse_runtime_extensions(data.get("extensions"), source_sha256)?;
    let unknown_extensions = match nonportable_policy {
        NonPortableContentPolicy::PreserveForRoundTrip => {
            collect_unknown_extensions(data, source_sha256)?
        }
        NonPortableContentPolicy::Omit => UnknownExtensionIndex::default(),
    };

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
        runtime,
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

fn parse_knowledge_book(
    value: Option<&Value>,
    card_source_sha256: &str,
) -> CoreResult<Option<CharacterKnowledgeBookRef>> {
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
        .map_or_else(
            || KnowledgeBookId::from(format!("card-book:{}", source_sha256.as_str())),
            KnowledgeBookId::from,
        );
    let embedded =
        parse_portable_knowledge_book(object, id.clone(), name.clone(), card_source_sha256)?;
    Ok(Some(CharacterKnowledgeBookRef {
        id: Some(id),
        name,
        source_sha256: Some(source_sha256),
        embedded: Some(embedded),
    }))
}

pub(crate) fn parse_runtime_knowledge_book(
    entries: &Value,
    source_sha256: &str,
    name: &str,
) -> CoreResult<Option<CharacterKnowledgeBookRef>> {
    if !entries.is_array() {
        return Err(unsupported(
            "runtime knowledge entries must be an array when present",
        ));
    }
    let mut book = serde_json::Map::new();
    book.insert("name".to_owned(), Value::String(name.to_owned()));
    book.insert("entries".to_owned(), entries.clone());
    book.insert("scan_depth".to_owned(), Value::from(5));
    book.insert("token_budget".to_owned(), Value::from(80_000));
    book.insert("recursive_scanning".to_owned(), Value::Bool(false));
    parse_knowledge_book(Some(&Value::Object(book)), source_sha256)
}

fn parse_portable_knowledge_book(
    object: &serde_json::Map<String, Value>,
    id: KnowledgeBookId,
    name: Option<String>,
    card_source_sha256: &str,
) -> CoreResult<PortableKnowledgeBook> {
    let entries = match object.get("entries") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(entries)) => entries.clone(),
        Some(_) => {
            return Err(unsupported(
                "CCv3 data.character_book.entries must be an array or null",
            ));
        }
    };
    if entries.len() > MAX_KNOWLEDGE_ENTRIES {
        return Err(unsupported(format!(
            "CCv3 character book exceeds the {MAX_KNOWLEDGE_ENTRIES}-entry limit"
        )));
    }
    let book_name = name.unwrap_or_else(|| "Embedded knowledge".to_owned());
    let mut normalized_entries = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        normalized_entries.push(parse_portable_knowledge_entry(
            entry,
            index,
            card_source_sha256,
        )?);
    }
    let recursive = optional_bool(object, "recursive_scanning", false)?;
    let mut metadata = BTreeMap::new();
    for key in ["extensions"] {
        if let Some(value) = object.get(key) {
            metadata.insert(key.to_owned(), canonical_json(value)?);
        }
    }
    Ok(PortableKnowledgeBook {
        id,
        name: book_name,
        entries: normalized_entries,
        scan_depth: optional_u32(object, "scan_depth", 5)?,
        token_budget: optional_u32(object, "token_budget", 2_048)?,
        recursive,
        max_recursion_depth: if recursive { 4 } else { 0 },
        metadata,
    })
}

fn parse_portable_knowledge_entry(
    value: &Value,
    index: usize,
    card_source_sha256: &str,
) -> CoreResult<PortableKnowledgeEntry> {
    let object = value
        .as_object()
        .ok_or_else(|| unsupported("CCv3 data.character_book.entries items must be objects"))?;
    let content = object
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    validate_metadata_text(
        "data.character_book.entries.content",
        &content,
        MAX_KNOWLEDGE_ENTRY_BYTES,
        MAX_KNOWLEDGE_ENTRY_CHARS,
    )?;
    let name = object
        .get("name")
        .or_else(|| object.get("comment"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    validate_metadata_text(
        "data.character_book.entries.name",
        &name,
        MAX_CHARACTER_NAME_BYTES,
        MAX_CHARACTER_NAME_CHARS,
    )?;
    let primary_keys = if let Some(value) = object.get("keys") {
        value_string_array(value)?
    } else if let Some(value) = object.get("key") {
        comma_separated_keys(value)?
    } else {
        Vec::new()
    };
    let secondary_keys = if let Some(value) = object
        .get("secondary_keys")
        .or_else(|| object.get("secondaryKeys"))
    {
        value_string_array(value)?
    } else if let Some(value) = object.get("secondkey") {
        comma_separated_keys(value)?
    } else {
        Vec::new()
    };
    let raw_id = object
        .get("id")
        .or_else(|| object.get("uid"))
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            _ => None,
        });
    let id = raw_id.unwrap_or_else(|| {
        let mut digest = Sha256::new();
        digest.update(b"portable-knowledge-entry-v1\0");
        digest.update(card_source_sha256.as_bytes());
        digest.update([0]);
        digest.update(index.to_le_bytes());
        format!("card-entry:{}", hex::encode(digest.finalize()))
    });
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("normal");
    let folder = mode.eq_ignore_ascii_case("folder");
    let placement = knowledge_placement(&content);
    let parent_id = object
        .get("folder")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let mut metadata = BTreeMap::new();
    for key in ["extensions", "mode", "folder"] {
        if let Some(value) = object.get(key) {
            metadata.insert(key.to_owned(), canonical_json(value)?);
        }
    }
    Ok(PortableKnowledgeEntry {
        id,
        name,
        content,
        enabled: optional_bool(object, "enabled", true)?,
        primary_keys,
        secondary_keys,
        constant: optional_bool_alias(object, &["constant", "alwaysActive"], false)?,
        selective: optional_bool(object, "selective", false)?,
        case_sensitive: optional_bool(object, "case_sensitive", false)?,
        whole_word: optional_bool(object, "match_whole_words", false)?,
        use_regex: optional_bool_alias(object, &["use_regex", "useRegex"], false)?,
        priority: optional_i32_alias(object, &["insertion_order", "insertorder"], 0)?,
        placement,
        parent_id,
        probability_basis_points: parse_probability_basis_points(object)?,
        folder,
        metadata,
    })
}

fn knowledge_placement(content: &str) -> PortableKnowledgePlacement {
    let first_line = content.lines().next().unwrap_or_default().trim();
    if let Some(depth) = first_line.strip_prefix("@@depth ") {
        return if depth.trim() == "0" {
            PortableKnowledgePlacement::PostHistory
        } else {
            PortableKnowledgePlacement::BeforeRecentHistory
        };
    }
    if let Some(name) = first_line.strip_prefix("@@position ") {
        return PortableKnowledgePlacement::Named(name.trim().to_owned());
    }
    PortableKnowledgePlacement::RetrievedContext
}

fn parse_probability_basis_points(object: &serde_json::Map<String, Value>) -> CoreResult<u16> {
    let value = object
        .get("probability")
        .or_else(|| object.get("activation_probability"));
    let Some(value) = value else {
        return Ok(10_000);
    };
    let probability = value
        .as_f64()
        .ok_or_else(|| unsupported("knowledge entry probability must be a number when present"))?;
    if !probability.is_finite() || probability < 0.0 {
        return Err(unsupported(
            "knowledge entry probability must be a finite non-negative number",
        ));
    }
    let basis_points = if probability <= 1.0 {
        probability * 10_000.0
    } else if probability <= 100.0 {
        probability * 100.0
    } else {
        probability
    };
    Ok(clamped_probability_basis_points(basis_points))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamped_probability_basis_points(value: f64) -> u16 {
    // The caller rejects non-finite and negative values. Rounding and clamping
    // establish the complete u16-safe interval before this numeric conversion.
    value.round().clamp(0.0, 10_000.0) as u16
}

fn parse_runtime_extensions(
    value: Option<&Value>,
    source_sha256: &str,
) -> CoreResult<CharacterRuntimeProfile> {
    let Some(Value::Object(extensions)) = value else {
        return Ok(CharacterRuntimeProfile::default());
    };
    for candidate in extensions.values() {
        let Some(object) = portable_runtime_extension_object(candidate) else {
            continue;
        };
        let background_markup = optional_runtime_string(object, "backgroundHTML")?;
        let additional_text = optional_runtime_string(object, "additionalText")?;
        let toggle_schema = optional_runtime_string(object, "toggles")?;
        let elevated_access = optional_bool(object, "lowLevelAccess", false)?;
        let virtual_script = optional_runtime_string(object, "virtualscript")?;
        let scripts = (!virtual_script.is_empty())
            .then(|| PortableRuntimeScript {
                id: format!("card-script:{source_sha256}:0"),
                name: "Embedded runtime".to_owned(),
                event: "load".to_owned(),
                language: "javascript".to_owned(),
                source: virtual_script,
                elevated_access,
                metadata: BTreeMap::new(),
            })
            .into_iter()
            .collect();
        let mut initial_variables = parse_runtime_variables(object.get("defaultVariables"))?;
        for (name, value) in parse_toggle_defaults(&toggle_schema) {
            initial_variables.entry(name).or_insert(value);
        }
        let mut metadata = BTreeMap::new();
        for (key, value) in object {
            if matches!(
                key.as_str(),
                "backgroundHTML"
                    | "virtualscript"
                    | "additionalText"
                    | "toggles"
                    | "defaultVariables"
            ) {
                continue;
            }
            metadata.insert(key.clone(), canonical_json(value)?);
        }
        return Ok(CharacterRuntimeProfile {
            source_id: Some(format!("card-runtime:{source_sha256}")),
            transform_set_id: None,
            transforms: Vec::new(),
            scripts,
            background_markup,
            additional_text,
            toggle_schema,
            initial_variables,
            metadata,
        });
    }
    Ok(CharacterRuntimeProfile::default())
}

fn parse_runtime_variables(value: Option<&Value>) -> CoreResult<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::String(value) if value.is_empty() => Ok(BTreeMap::new()),
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), runtime_variable_text(value)?)))
            .collect(),
        _ => Ok(BTreeMap::from([(
            "source".to_owned(),
            canonical_json(value)?,
        )])),
    }
}

fn runtime_variable_text(value: &Value) -> CoreResult<String> {
    match value {
        Value::Null => Ok(String::new()),
        Value::Bool(value) => Ok(u8::from(*value).to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(value.clone()),
        Value::Array(_) | Value::Object(_) => canonical_json(value),
    }
}

fn parse_toggle_defaults(schema: &str) -> BTreeMap<String, String> {
    schema
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('=') {
                return None;
            }
            let mut fields = line.split('=');
            let name = fields.next()?.trim();
            let _label = fields.next()?;
            let kind = fields.next()?.trim().to_ascii_lowercase();
            if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
                return None;
            }
            let default = match kind.as_str() {
                "select" | "toggle" | "checkbox" => "0",
                "text" | "textarea" => "",
                _ => return None,
            };
            Some((name.to_owned(), default.to_owned()))
        })
        .collect()
}

fn optional_runtime_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> CoreResult<String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(unsupported(format!(
            "runtime field {key} must be a string or null"
        ))),
    }
}

fn value_string_array(value: &Value) -> CoreResult<Vec<String>> {
    match value {
        Value::Null => Ok(Vec::new()),
        Value::String(value) => Ok(vec![value.clone()]),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| unsupported("knowledge keys must be strings"))
            })
            .collect(),
        _ => Err(unsupported("knowledge keys must be a string array")),
    }
}

fn comma_separated_keys(value: &Value) -> CoreResult<Vec<String>> {
    let value = value
        .as_str()
        .ok_or_else(|| unsupported("knowledge keys must be strings"))?;
    Ok(value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect())
}

fn optional_bool(
    object: &serde_json::Map<String, Value>,
    key: &str,
    default: bool,
) -> CoreResult<bool> {
    object.get(key).map_or(Ok(default), |value| {
        value
            .as_bool()
            .ok_or_else(|| unsupported(format!("{key} must be a boolean")))
    })
}

fn optional_bool_alias(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    default: bool,
) -> CoreResult<bool> {
    for key in keys {
        if object.contains_key(*key) {
            return optional_bool(object, key, default);
        }
    }
    Ok(default)
}

fn optional_u32(
    object: &serde_json::Map<String, Value>,
    key: &str,
    default: u32,
) -> CoreResult<u32> {
    object.get(key).map_or(Ok(default), |value| {
        value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .ok_or_else(|| unsupported(format!("{key} must be a non-negative 32-bit integer")))
    })
}

fn optional_i32(
    object: &serde_json::Map<String, Value>,
    key: &str,
    default: i32,
) -> CoreResult<i32> {
    object.get(key).map_or(Ok(default), |value| {
        value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| unsupported(format!("{key} must be a 32-bit integer")))
    })
}

fn optional_i32_alias(
    object: &serde_json::Map<String, Value>,
    keys: &[&str],
    default: i32,
) -> CoreResult<i32> {
    for key in keys {
        if object.contains_key(*key) {
            return optional_i32(object, key, default);
        }
    }
    Ok(default)
}

fn canonical_json(value: &Value) -> CoreResult<String> {
    serde_json::to_string(value)
        .map_err(|error| unsupported(format!("cannot normalize runtime metadata: {error}")))
}

fn collect_unknown_extensions(
    data: &serde_json::Map<String, Value>,
    source_sha256: &str,
) -> CoreResult<UnknownExtensionIndex> {
    let raw_source_sha256 = Sha256Digest::parse(source_sha256.to_owned()).map_err(unsupported)?;
    let mut entries = Vec::new();
    for (key, value) in data {
        if key == "extensions" {
            let extensions = value.as_object().ok_or_else(|| {
                unsupported("CCv3 data.extensions must be an object when present")
            })?;
            for (extension_key, extension_value) in extensions {
                if portable_runtime_extension_object(extension_value).is_some() {
                    continue;
                }
                push_unknown_extension(
                    extension_key,
                    &format!("/data/extensions/{}", escape_json_pointer(extension_key)),
                    extension_value,
                    &mut entries,
                )?;
            }
            continue;
        }
        if is_supported_character_field(key) {
            if key == "assets" {
                collect_external_asset_references(value, &mut entries)?;
            }
            continue;
        }
        push_unknown_extension(
            key,
            &format!("/data/{}", escape_json_pointer(key)),
            value,
            &mut entries,
        )?;
    }
    UnknownExtensionIndex::try_new(Some(raw_source_sha256), entries).map_err(unsupported)
}

fn portable_runtime_extension_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    let object = value.as_object()?;
    [
        "backgroundHTML",
        "virtualscript",
        "additionalText",
        "toggles",
        "defaultVariables",
        "lowLevelAccess",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
    .then_some(object)
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
    if assets.len() > MAX_CARD_ASSET_REFERENCES {
        return Err(unsupported(format!(
            "CCv3 data.assets exceeds the {MAX_CARD_ASSET_REFERENCES}-item limit"
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

fn preferred_embedded_image_path(value: Option<&Value>) -> CoreResult<Option<String>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let assets = match value {
        Value::Null => return Ok(None),
        Value::Array(assets) => assets,
        _ => return Err(unsupported("CCv3 data.assets must be an array or null")),
    };
    if assets.len() > MAX_CARD_ASSET_REFERENCES {
        return Err(unsupported(format!(
            "CCv3 data.assets exceeds the {MAX_CARD_ASSET_REFERENCES}-item limit"
        )));
    }

    let mut first_icon = None;
    for asset in assets {
        let Some(asset) = asset.as_object() else {
            continue;
        };
        let Some(uri) = asset.get("uri").and_then(Value::as_str) else {
            continue;
        };
        let Some(path) = embedded_asset_path(uri) else {
            continue;
        };
        let is_icon = asset
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("icon"));
        if !is_icon {
            continue;
        }
        let is_main = asset
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("main"));
        if is_main {
            return Ok(Some(path));
        }
        first_icon.get_or_insert(path);
    }
    Ok(first_icon)
}

fn embedded_asset_path(uri: &str) -> Option<String> {
    const PREFIXES: [&str; 2] = ["embeded://", "embedded://"];
    let lower = uri.to_ascii_lowercase();
    let prefix = PREFIXES.iter().find(|prefix| lower.starts_with(**prefix))?;
    let path = uri.get(prefix.len()..)?;
    crate::path::validate_archive_path(path).ok()
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
            | "extensions"
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
