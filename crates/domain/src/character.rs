use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use uuid::Uuid;

use crate::{
    content::{AssetDescriptor, Sha256Digest},
    orchestration::KnowledgeBookId,
};

/// A locally imported AI character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Character {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source_hash: String,
    pub avatar_asset_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Character {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        source_hash: impl Into<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            description: description.into(),
            source_hash: source_hash.into(),
            avatar_asset_hash: None,
            created_at: Utc::now(),
        }
    }
}

/// Wire and storage version for normalized character-card content.
pub const CHARACTER_CONTENT_SCHEMA_VERSION: u32 = 1;
pub const MAX_UNKNOWN_EXTENSION_ENTRIES: usize = 128;
pub const MAX_UNKNOWN_EXTENSION_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_UNKNOWN_EXTENSION_KEY_BYTES: usize = 256;
pub const MAX_UNKNOWN_EXTENSION_KEY_CHARS: usize = 128;
pub const MAX_UNKNOWN_EXTENSION_PATH_BYTES: usize = 1_024;
pub const MAX_UNKNOWN_EXTENSION_PATH_CHARS: usize = 512;
pub const MAX_EXTENSION_QUARANTINE_REASON_BYTES: usize = 1_024;
pub const MAX_EXTENSION_QUARANTINE_REASON_CHARS: usize = 512;

const fn character_content_schema_version() -> u32 {
    CHARACTER_CONTENT_SCHEMA_VERSION
}

fn deserialize_example_dialogs<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ExampleDialogsWire {
        One(String),
        Many(Vec<String>),
    }

    match ExampleDialogsWire::deserialize(deserializer)? {
        ExampleDialogsWire::One(value) if value.is_empty() => Ok(Vec::new()),
        ExampleDialogsWire::One(value) => Ok(vec![value]),
        ExampleDialogsWire::Many(values) => Ok(values),
    }
}

/// Reference to a normalized knowledge book associated with a character card.
///
/// `source_sha256` identifies the serialized source from which the book was
/// normalized. It does not contain arbitrary source JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterKnowledgeBookRef {
    #[serde(default)]
    pub id: Option<KnowledgeBookId>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub source_sha256: Option<Sha256Digest>,
}

/// Inert classification applied to an unknown extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionQuarantineKind {
    Code,
    Script,
    Html,
    ExternalUrl,
    UnknownActiveContent,
}

/// Quarantine record for extension data that must never execute on import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtensionQuarantine {
    pub kind: ExtensionQuarantineKind,
    pub reason: String,
    /// Quarantined content is always inactive. Deserialization rejects `true`.
    #[serde(default)]
    pub active: bool,
}

impl ExtensionQuarantine {
    pub fn inactive(kind: ExtensionQuarantineKind, reason: impl Into<String>) -> Self {
        Self {
            kind,
            reason: reason.into(),
            active: false,
        }
    }
}

#[derive(Deserialize)]
struct ExtensionQuarantineWire {
    kind: ExtensionQuarantineKind,
    reason: String,
    #[serde(default)]
    active: bool,
}

impl<'de> Deserialize<'de> for ExtensionQuarantine {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ExtensionQuarantineWire::deserialize(deserializer)?;
        if wire.active {
            return Err(D::Error::custom(
                "quarantined extension content must remain inactive",
            ));
        }
        validate_bounded_label(
            "extension quarantine reason",
            &wire.reason,
            MAX_EXTENSION_QUARANTINE_REASON_BYTES,
            MAX_EXTENSION_QUARANTINE_REASON_CHARS,
        )
        .map_err(D::Error::custom)?;
        Ok(Self {
            kind: wire.kind,
            reason: wire.reason,
            active: false,
        })
    }
}

/// Pointer to one unknown extension in an immutable imported source.
///
/// The value itself is deliberately absent: `source_path` and `sha256` index
/// bytes retained under the enclosing raw-source hash without making the
/// extension executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnknownExtensionEntry {
    pub key: String,
    pub source_path: String,
    pub sha256: Sha256Digest,
    pub size_bytes: u64,
    #[serde(default)]
    pub quarantine: Option<ExtensionQuarantine>,
}

impl UnknownExtensionEntry {
    pub fn try_new(
        key: impl Into<String>,
        source_path: impl Into<String>,
        sha256: Sha256Digest,
        size_bytes: u64,
        quarantine: Option<ExtensionQuarantine>,
    ) -> Result<Self, String> {
        let entry = Self {
            key: key.into(),
            source_path: source_path.into(),
            sha256,
            size_bytes,
            quarantine,
        };
        validate_unknown_extension_entry(&entry)?;
        Ok(entry)
    }
}

#[derive(Deserialize)]
struct UnknownExtensionEntryWire {
    key: String,
    source_path: String,
    sha256: Sha256Digest,
    size_bytes: u64,
    #[serde(default)]
    quarantine: Option<ExtensionQuarantine>,
}

impl<'de> Deserialize<'de> for UnknownExtensionEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = UnknownExtensionEntryWire::deserialize(deserializer)?;
        Self::try_new(
            wire.key,
            wire.source_path,
            wire.sha256,
            wire.size_bytes,
            wire.quarantine,
        )
        .map_err(D::Error::custom)
    }
}

/// Size-bounded index of unrecognized extension data.
///
/// Imported source bytes remain immutable in content-addressed storage. This
/// index records only bounded metadata needed for audit and lossless export.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UnknownExtensionIndex {
    pub raw_source_sha256: Option<Sha256Digest>,
    pub entries: Vec<UnknownExtensionEntry>,
    pub entry_count: u32,
    pub total_size_bytes: u64,
}

impl UnknownExtensionIndex {
    pub fn try_new(
        raw_source_sha256: Option<Sha256Digest>,
        mut entries: Vec<UnknownExtensionEntry>,
    ) -> Result<Self, String> {
        if entries.len() > MAX_UNKNOWN_EXTENSION_ENTRIES {
            return Err(format!(
                "unknown extension index exceeds the {MAX_UNKNOWN_EXTENSION_ENTRIES}-entry limit"
            ));
        }
        if !entries.is_empty() && raw_source_sha256.is_none() {
            return Err("unknown extension index requires a raw source SHA-256 digest".to_owned());
        }

        let mut total_size_bytes = 0_u64;
        for entry in &entries {
            validate_unknown_extension_entry(entry)?;
            total_size_bytes = total_size_bytes
                .checked_add(entry.size_bytes)
                .ok_or_else(|| "unknown extension size overflow".to_owned())?;
            if total_size_bytes > MAX_UNKNOWN_EXTENSION_TOTAL_BYTES {
                return Err(format!(
                    "unknown extensions exceed the {MAX_UNKNOWN_EXTENSION_TOTAL_BYTES}-byte limit"
                ));
            }
        }

        entries.sort_by(|left, right| {
            left.source_path
                .cmp(&right.source_path)
                .then_with(|| left.key.cmp(&right.key))
        });
        if entries
            .windows(2)
            .any(|pair| pair[0].source_path == pair[1].source_path)
        {
            return Err("unknown extension source paths must be unique".to_owned());
        }

        let entry_count = u32::try_from(entries.len())
            .map_err(|_| "unknown extension entry count overflow".to_owned())?;
        Ok(Self {
            raw_source_sha256,
            entries,
            entry_count,
            total_size_bytes,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[derive(Deserialize)]
struct UnknownExtensionIndexWire {
    #[serde(default)]
    raw_source_sha256: Option<Sha256Digest>,
    #[serde(default)]
    entries: Vec<UnknownExtensionEntry>,
    #[serde(default)]
    entry_count: Option<u32>,
    #[serde(default)]
    total_size_bytes: Option<u64>,
}

impl<'de> Deserialize<'de> for UnknownExtensionIndex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = UnknownExtensionIndexWire::deserialize(deserializer)?;
        let index =
            Self::try_new(wire.raw_source_sha256, wire.entries).map_err(D::Error::custom)?;
        if wire
            .entry_count
            .is_some_and(|count| count != index.entry_count)
        {
            return Err(D::Error::custom(
                "unknown extension entry_count does not match entries",
            ));
        }
        if wire
            .total_size_bytes
            .is_some_and(|size| size != index.total_size_bytes)
        {
            return Err(D::Error::custom(
                "unknown extension total_size_bytes does not match entries",
            ));
        }
        Ok(index)
    }
}

fn validate_unknown_extension_entry(entry: &UnknownExtensionEntry) -> Result<(), String> {
    validate_bounded_label(
        "unknown extension key",
        &entry.key,
        MAX_UNKNOWN_EXTENSION_KEY_BYTES,
        MAX_UNKNOWN_EXTENSION_KEY_CHARS,
    )?;
    validate_bounded_label(
        "unknown extension source path",
        &entry.source_path,
        MAX_UNKNOWN_EXTENSION_PATH_BYTES,
        MAX_UNKNOWN_EXTENSION_PATH_CHARS,
    )?;
    if !entry.source_path.starts_with('/') {
        return Err("unknown extension source path must be a JSON Pointer".to_owned());
    }
    validate_json_pointer(&entry.source_path)?;
    if entry.size_bytes > MAX_UNKNOWN_EXTENSION_TOTAL_BYTES {
        return Err(format!(
            "unknown extension exceeds the {MAX_UNKNOWN_EXTENSION_TOTAL_BYTES}-byte limit"
        ));
    }
    if let Some(quarantine) = &entry.quarantine {
        if quarantine.active {
            return Err("quarantined extension content must remain inactive".to_owned());
        }
        validate_bounded_label(
            "extension quarantine reason",
            &quarantine.reason,
            MAX_EXTENSION_QUARANTINE_REASON_BYTES,
            MAX_EXTENSION_QUARANTINE_REASON_CHARS,
        )?;
    }
    Ok(())
}

fn validate_json_pointer(value: &str) -> Result<(), String> {
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return Err(
                "unknown extension source path contains an invalid JSON Pointer escape".to_owned(),
            );
        }
    }
    Ok(())
}

fn validate_bounded_label(
    label: &str,
    value: &str,
    max_bytes: usize,
    max_chars: usize,
) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{label} must not contain control characters"));
    }
    if value.len() > max_bytes || value.chars().count() > max_chars {
        return Err(format!(
            "{label} exceeds the {max_bytes}-byte or {max_chars}-character limit"
        ));
    }
    Ok(())
}

/// Normalized, provider-neutral content stored alongside [`Character`].
///
/// Keeping this versioned object separate preserves the existing `Character`
/// storage and IPC contract while allowing complete character-card content to
/// round-trip. All fields default so older serialized companion records remain
/// readable as fields are introduced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterContentV1 {
    #[serde(default = "character_content_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub personality: String,
    #[serde(default)]
    pub scenario: String,
    #[serde(default, alias = "first_mes")]
    pub first_message: String,
    #[serde(
        default,
        alias = "mes_example",
        deserialize_with = "deserialize_example_dialogs"
    )]
    pub example_dialogs: Vec<String>,
    #[serde(default, alias = "system_prompt")]
    pub system_instruction: String,
    #[serde(default, alias = "post_history_instructions")]
    pub post_history_instruction: String,
    #[serde(default)]
    pub alternate_greetings: Vec<String>,
    #[serde(default)]
    pub knowledge_book: Option<CharacterKnowledgeBookRef>,
    #[serde(default)]
    pub assets: Vec<AssetDescriptor>,
    #[serde(default)]
    pub unknown_extensions: UnknownExtensionIndex,
}

impl Default for CharacterContentV1 {
    fn default() -> Self {
        Self {
            schema_version: CHARACTER_CONTENT_SCHEMA_VERSION,
            personality: String::new(),
            scenario: String::new(),
            first_message: String::new(),
            example_dialogs: Vec::new(),
            system_instruction: String::new(),
            post_history_instruction: String::new(),
            alternate_greetings: Vec::new(),
            knowledge_book: None,
            assets: Vec::new(),
            unknown_extensions: UnknownExtensionIndex::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extension_entry(path: &str, size_bytes: u64) -> UnknownExtensionEntry {
        UnknownExtensionEntry {
            key: "synthetic".into(),
            source_path: path.into(),
            sha256: Sha256Digest::parse("ab".repeat(32)).expect("digest"),
            size_bytes,
            quarantine: None,
        }
    }

    #[test]
    fn legacy_character_wire_shape_remains_unchanged() {
        let character = Character {
            id: "character".into(),
            name: "Segu".into(),
            description: "Guide".into(),
            source_hash: "12".repeat(32),
            avatar_asset_hash: None,
            created_at: DateTime::parse_from_rfc3339("2026-08-03T00:00:00Z")
                .expect("timestamp")
                .with_timezone(&Utc),
        };

        let value = serde_json::to_value(character).expect("serialize character");
        assert_eq!(
            value.as_object().expect("object").keys().count(),
            6,
            "the existing Character contract must not gain companion fields"
        );
        assert!(value.get("content").is_none());
    }

    #[test]
    fn character_content_defaults_and_accepts_standard_card_aliases() {
        let empty: CharacterContentV1 = serde_json::from_str("{}").expect("default content");
        assert_eq!(empty, CharacterContentV1::default());

        let content: CharacterContentV1 = serde_json::from_str(
            r#"{
                "first_mes":"Hello",
                "mes_example":"User: Hi\nSegu: Welcome",
                "system_prompt":"Stay in character",
                "post_history_instructions":"Answer briefly"
            }"#,
        )
        .expect("standard aliases");
        assert_eq!(content.first_message, "Hello");
        assert_eq!(
            content.example_dialogs,
            ["User: Hi\nSegu: Welcome"],
            "the standard single string must remain intact"
        );
        assert_eq!(content.system_instruction, "Stay in character");
        assert_eq!(content.post_history_instruction, "Answer briefly");
    }

    #[test]
    fn unknown_extension_index_is_sorted_bounded_and_source_backed() {
        let source = Sha256Digest::parse("cd".repeat(32)).expect("source digest");
        let index = UnknownExtensionIndex::try_new(
            Some(source),
            vec![
                extension_entry("/data/extensions/zeta", 7),
                extension_entry("/data/extensions/alpha", 5),
            ],
        )
        .expect("valid index");

        assert_eq!(index.entry_count, 2);
        assert_eq!(index.total_size_bytes, 12);
        assert_eq!(index.entries[0].source_path, "/data/extensions/alpha");
        assert!(
            UnknownExtensionIndex::try_new(
                None,
                vec![extension_entry("/data/extensions/value", 1)]
            )
            .is_err()
        );
        assert!(
            UnknownExtensionIndex::try_new(
                Some(Sha256Digest::parse("ef".repeat(32)).expect("digest")),
                vec![
                    extension_entry("/data/extensions/duplicate", 1),
                    extension_entry("/data/extensions/duplicate", 1),
                ],
            )
            .is_err()
        );
    }

    #[test]
    fn unknown_extension_wire_data_cannot_bypass_bounds_or_quarantine() {
        let oversized_entries = (0..=MAX_UNKNOWN_EXTENSION_ENTRIES)
            .map(|index| extension_entry(&format!("/data/extensions/{index}"), 1))
            .collect::<Vec<_>>();
        let oversized_json = serde_json::json!({
            "raw_source_sha256": "12".repeat(32),
            "entries": oversized_entries,
        });
        assert!(
            serde_json::from_value::<UnknownExtensionIndex>(oversized_json).is_err(),
            "deserialization must enforce the entry limit"
        );

        let mut active = extension_entry("/data/extensions/script", 1);
        active.quarantine = Some(ExtensionQuarantine {
            kind: ExtensionQuarantineKind::Script,
            reason: "scripts are inert on import".into(),
            active: true,
        });
        let active_json = serde_json::json!({
            "raw_source_sha256": "34".repeat(32),
            "entries": [active],
        });
        assert!(
            serde_json::from_value::<UnknownExtensionIndex>(active_json).is_err(),
            "quarantined extensions cannot become active through serialized input"
        );

        let mismatched_totals = serde_json::json!({
            "raw_source_sha256": "56".repeat(32),
            "entries": [extension_entry("/data/extensions/value", 3)],
            "entry_count": 1,
            "total_size_bytes": 4,
        });
        assert!(serde_json::from_value::<UnknownExtensionIndex>(mismatched_totals).is_err());
    }
}
