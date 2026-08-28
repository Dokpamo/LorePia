use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use uuid::Uuid;

use crate::{
    content::{AssetDescriptor, Sha256Digest},
    orchestration::{
        ActivationRule, KnowledgeBook, KnowledgeBookId, KnowledgeEntry, KnowledgeEntryId,
        KnowledgePlacement, Provenance, SafeRegex, TokenBudget, TokenPolicy, TransformPhase,
        TransformRule, TransformRuleId, TransformSet, TransformSetId,
    },
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterKnowledgeBookRef {
    #[serde(default)]
    pub id: Option<KnowledgeBookId>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub source_sha256: Option<Sha256Digest>,
    /// Complete portable book data when the source carries an inline book.
    ///
    /// Older imports only stored the reference above. Keeping the normalized
    /// book here lets a committed character run without a separate creator
    /// document or a source-specific database migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded: Option<PortableKnowledgeBook>,
}

/// Provider-neutral knowledge book embedded in an imported character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableKnowledgeBook {
    pub id: KnowledgeBookId,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub entries: Vec<PortableKnowledgeEntry>,
    #[serde(default)]
    pub scan_depth: u32,
    #[serde(default)]
    pub token_budget: u32,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub max_recursion_depth: u32,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl PortableKnowledgeBook {
    /// Converts the lossless card representation into the native deterministic
    /// retrieval document used by prompt generation.
    #[must_use]
    pub fn materialize(&self, provenance: Provenance) -> KnowledgeBook {
        let named_positions = portable_named_positions(&self.entries);
        let entries = self
            .entries
            .iter()
            .filter(|entry| !entry.folder && !entry.content.trim().is_empty())
            .map(|entry| {
                materialize_portable_knowledge_entry(entry, &self.id, &named_positions, &provenance)
            })
            .collect();
        KnowledgeBook {
            id: self.id.clone(),
            name: if self.name.trim().is_empty() {
                "Embedded knowledge".to_owned()
            } else {
                self.name.clone()
            },
            schema_version: 1,
            entries,
            scan_depth: self.scan_depth,
            token_budget: TokenBudget {
                max_tokens: self.token_budget,
            },
            recursive: self.recursive,
            max_recursion_depth: self.max_recursion_depth,
            provenance,
        }
    }
}

fn materialize_portable_knowledge_entry(
    entry: &PortableKnowledgeEntry,
    book_id: &KnowledgeBookId,
    named_positions: &BTreeMap<String, Vec<String>>,
    provenance: &Provenance,
) -> KnowledgeEntry {
    let primary = normalized_knowledge_keys(&entry.primary_keys);
    let secondary = normalized_knowledge_keys(&entry.secondary_keys);
    let regex_patterns = if entry.use_regex {
        primary
            .iter()
            .filter_map(|pattern| portable_safe_regex(pattern, entry.case_sensitive))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let activation = if entry.constant {
        ActivationRule::Always
    } else if !regex_patterns.is_empty() {
        ActivationRule::Regex {
            patterns: regex_patterns,
        }
    } else if primary.is_empty() {
        ActivationRule::Manual
    } else {
        ActivationRule::Keyword {
            primary,
            secondary,
            selective: entry.selective,
            case_sensitive: entry.case_sensitive,
            whole_word: entry.whole_word,
        }
    };
    KnowledgeEntry {
        id: KnowledgeEntryId::from(entry.id.clone()),
        book_id: book_id.clone(),
        name: entry.name.clone(),
        content: resolve_named_knowledge_positions(
            &strip_knowledge_decorator(&entry.content),
            named_positions,
        ),
        enabled: entry.enabled,
        activation,
        priority: entry.priority,
        importance: 50,
        placement: match &entry.placement {
            PortableKnowledgePlacement::RetrievedContext => KnowledgePlacement::RetrievedContext,
            PortableKnowledgePlacement::BeforeOlderHistory => {
                KnowledgePlacement::BeforeOlderHistory
            }
            PortableKnowledgePlacement::BeforeRecentHistory => {
                KnowledgePlacement::BeforeRecentHistory
            }
            PortableKnowledgePlacement::PostHistory | PortableKnowledgePlacement::Named(_) => {
                KnowledgePlacement::PostHistory
            }
        },
        token_policy: TokenPolicy {
            priority: u16::try_from(entry.priority.clamp(0, i32::from(u16::MAX)))
                .unwrap_or(u16::MAX),
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        parent_id: None,
        activation_probability_basis_points: entry.probability_basis_points,
        provenance: provenance.clone(),
    }
}

fn normalized_knowledge_keys(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn portable_named_positions(entries: &[PortableKnowledgeEntry]) -> BTreeMap<String, Vec<String>> {
    let mut positions = BTreeMap::<String, Vec<String>>::new();
    for entry in entries {
        let PortableKnowledgePlacement::Named(name) = &entry.placement else {
            continue;
        };
        if !entry.enabled || entry.folder || entry.content.trim().is_empty() {
            continue;
        }
        let content = strip_knowledge_decorator(&entry.content);
        positions
            .entry(name.clone())
            .or_default()
            .push(content.clone());
        if let Some(alias) = name.strip_prefix("pt_") {
            positions.entry(alias.to_owned()).or_default().push(content);
        }
    }
    positions
}

fn resolve_named_knowledge_positions(
    source: &str,
    positions: &BTreeMap<String, Vec<String>>,
) -> String {
    const MAX_DEPTH: usize = 5;
    const MAX_OUTPUT_BYTES: usize = 256 * 1_024;

    let mut output = source.to_owned();
    for _ in 0..MAX_DEPTH {
        let mut cursor = 0;
        let mut replaced = false;
        while let Some(relative_start) = output[cursor..].find("{{position::") {
            let start = cursor + relative_start;
            let Some(relative_end) = output[start + 12..].find("}}") else {
                return output;
            };
            let end = start + 12 + relative_end + 2;
            let name = output[start + 12..end - 2].trim();
            let replacement = positions
                .get(name)
                .map(|values| values.join("\n"))
                .unwrap_or_default();
            let next_len = output
                .len()
                .saturating_sub(end.saturating_sub(start))
                .saturating_add(replacement.len());
            if next_len > MAX_OUTPUT_BYTES {
                return source.to_owned();
            }
            output.replace_range(start..end, &replacement);
            cursor = start.saturating_add(replacement.len());
            replaced = true;
        }
        if !replaced {
            return output;
        }
    }

    while let Some(start) = output.find("{{position::") {
        let Some(relative_end) = output[start + 12..].find("}}") else {
            break;
        };
        let end = start + 12 + relative_end + 2;
        output.replace_range(start..end, "");
    }
    output
}

fn portable_safe_regex(source: &str, case_sensitive: bool) -> Option<SafeRegex> {
    let mut pattern = source.trim();
    let mut case_insensitive = !case_sensitive;
    if let Some(body) = pattern.strip_prefix('/')
        && let Some(end) = body.rfind('/')
    {
        let flags = &body[end + 1..];
        pattern = &body[..end];
        case_insensitive |= flags.contains('i');
    }
    pattern = pattern.strip_prefix("(?!)|").unwrap_or(pattern);
    if pattern.is_empty()
        || ["(?=", "(?!", "(?<=", "(?<!", "\\1", "\\2", "\\k"]
            .iter()
            .any(|needle| pattern.contains(needle))
    {
        return None;
    }
    Some(SafeRegex {
        pattern: pattern.to_owned(),
        case_insensitive,
    })
}

fn strip_knowledge_decorator(content: &str) -> String {
    let Some((first, remaining)) = content.split_once('\n') else {
        return content.to_owned();
    };
    let first = first.trim();
    if first.starts_with("@@depth ") || first.starts_with("@@position ") {
        remaining.trim_start_matches(['\r', '\n']).to_owned()
    } else {
        content.to_owned()
    }
}

/// Portable activation and placement data for one embedded knowledge entry.
// These booleans mirror independent fields in the interchange schema; merging
// them into a state enum would lose valid combinations during round-tripping.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableKnowledgeEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub primary_keys: Vec<String>,
    #[serde(default)]
    pub secondary_keys: Vec<String>,
    #[serde(default)]
    pub constant: bool,
    #[serde(default)]
    pub selective: bool,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub use_regex: bool,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub placement: PortableKnowledgePlacement,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default = "full_probability_basis_points")]
    pub probability_basis_points: u16,
    #[serde(default)]
    pub folder: bool,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

const fn full_probability_basis_points() -> u16 {
    10_000
}

/// Placement preserved from card-level knowledge decorators.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PortableKnowledgePlacement {
    #[default]
    RetrievedContext,
    BeforeOlderHistory,
    BeforeRecentHistory,
    PostHistory,
    /// A named source slot that this runtime does not otherwise know about.
    Named(String),
}

/// A source-neutral transform phase carried by imported runtime data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortableTransformPhase {
    RequestContext,
    ProviderOutput,
    Display,
}

/// A text transform preserved from an imported character runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableTextTransform {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub phase: PortableTransformPhase,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub pattern: String,
    #[serde(default)]
    pub replacement: String,
    #[serde(default)]
    pub flags: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

const fn default_true() -> bool {
    true
}

/// An event-driven runtime script carried by a character package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRuntimeScript {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub elevated_access: bool,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

/// Runtime data that can be preserved independently of its source container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterRuntimeProfile {
    #[serde(default)]
    pub source_id: Option<String>,
    #[serde(default)]
    pub transform_set_id: Option<TransformSetId>,
    #[serde(default)]
    pub transforms: Vec<PortableTextTransform>,
    #[serde(default)]
    pub scripts: Vec<PortableRuntimeScript>,
    #[serde(default)]
    pub background_markup: String,
    #[serde(default)]
    pub additional_text: String,
    #[serde(default)]
    pub toggle_schema: String,
    #[serde(default)]
    pub initial_variables: BTreeMap<String, String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

impl CharacterRuntimeProfile {
    /// Builds every compatible native request/output/display rule. Rules requiring a
    /// regular-expression feature outside Rust's deterministic engine remain
    /// preserved in `transforms` for the portable client renderer.
    #[must_use]
    pub fn materialize_transform_set(&self, provenance: Provenance) -> Option<TransformSet> {
        let id = self.transform_set_id.clone()?;
        let rules = self
            .transforms
            .iter()
            .enumerate()
            .filter_map(|(order, transform)| {
                if !transform.enabled {
                    return None;
                }
                let (pattern, replacement) = portable_transform_parts(transform)?;
                Some(TransformRule {
                    id: TransformRuleId::from(transform.id.clone()),
                    name: if transform.name.trim().is_empty() {
                        format!("Character compatibility rule {}", order + 1)
                    } else {
                        transform.name.clone()
                    },
                    enabled: true,
                    imported_enabled: true,
                    imported_author_enabled: true,
                    phase: match transform.phase {
                        PortableTransformPhase::ProviderOutput => {
                            TransformPhase::ProviderOutputCanonical
                        }
                        PortableTransformPhase::Display => TransformPhase::DisplayOnly,
                        PortableTransformPhase::RequestContext => TransformPhase::ResolvedPrompt,
                    },
                    order: i32::try_from(order).unwrap_or(i32::MAX),
                    pattern: SafeRegex {
                        pattern,
                        case_insensitive: transform.flags.contains('i'),
                    },
                    replacement,
                    condition: None,
                    max_replacements: 10_000,
                    input_limit: 262_144,
                    output_limit: 262_144,
                    provenance: provenance.clone(),
                })
            })
            .collect::<Vec<_>>();
        (!rules.is_empty()).then(|| TransformSet {
            id,
            name: "Character output compatibility".to_owned(),
            schema_version: 1,
            enabled: true,
            imported_author_enabled: true,
            rules,
            max_rules_per_phase: 128,
            max_output_chars: 262_144,
            provenance,
        })
    }
}

fn portable_transform_parts(transform: &PortableTextTransform) -> Option<(String, String)> {
    let (pattern, replacement) =
        rewrite_terminal_positive_lookahead(&transform.pattern, &transform.replacement)
            .unwrap_or_else(|| (transform.pattern.clone(), transform.replacement.clone()));
    (!pattern.is_empty()
        && pattern.chars().count() <= 4_096
        && !["(?=", "(?!", "(?<=", "(?<!", "\\1", "\\2", "\\k"]
            .iter()
            .any(|needle| pattern.contains(needle)))
    .then_some((pattern, replacement))
}

/// Converts a terminal positive look-ahead into a consumed-and-reinserted
/// capture. This preserves the common "stop before the next tagged block"
/// idiom without introducing a backtracking regex engine.
fn rewrite_terminal_positive_lookahead(
    pattern: &str,
    replacement: &str,
) -> Option<(String, String)> {
    let marker = pattern.rfind("(?=")?;
    if !pattern.ends_with(')') || pattern[..marker].contains("(?=") {
        return None;
    }
    let prefix = &pattern[..marker];
    if ["(?!", "(?<=", "(?<!", "\\1", "\\2", "\\k"]
        .iter()
        .any(|needle| prefix.contains(needle))
    {
        return None;
    }
    let lookahead = &pattern[marker + 3..pattern.len() - 1];
    if lookahead.contains("(?") || lookahead_capture_count(lookahead) != 0 {
        return None;
    }
    let capture = lookahead_capture_count(prefix).checked_add(1)?;
    Some((
        format!("{prefix}({lookahead})"),
        format!("{replacement}${capture}"),
    ))
}

fn lookahead_capture_count(pattern: &str) -> usize {
    let bytes = pattern.as_bytes();
    let mut escaped = false;
    let mut count = 0;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' {
            escaped = true;
        } else if byte == b'(' && bytes.get(index + 1) != Some(&b'?') {
            count += 1;
        }
    }
    count
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
    pub runtime: CharacterRuntimeProfile,
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
            runtime: CharacterRuntimeProfile::default(),
            unknown_extensions: UnknownExtensionIndex::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_lookahead_is_consumed_and_reinserted_for_native_regex() {
        assert_eq!(
            rewrite_terminal_positive_lookahead("(post)(?=\\nnext|\\nend)", "<$1>"),
            Some(("(post)(\\nnext|\\nend)".to_owned(), "<$1>$2".to_owned()))
        );
    }

    #[test]
    fn named_knowledge_positions_are_expanded_with_the_portable_alias() {
        let positions = BTreeMap::from([(
            "Do-yoon_option1".to_owned(),
            vec!["childhood friend".to_owned()],
        )]);
        assert_eq!(
            resolve_named_knowledge_positions(
                "profile\n{{position::Do-yoon_option1}}\nend",
                &positions,
            ),
            "profile\nchildhood friend\nend"
        );
        assert_eq!(
            resolve_named_knowledge_positions("{{position::missing}}", &positions),
            ""
        );
    }

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
