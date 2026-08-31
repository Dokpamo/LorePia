use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use lorepia_core::{
    AssetDescriptor, Character, CharacterContentV1, CharacterGreetingCatalog,
    CharacterGreetingKind, CharacterGreetingOption, ContentKind, ImportImagePreview,
    ImportInspection, ImportWarning,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterDto {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source_hash: String,
    /// Content-addressed logical identifier, never a host filesystem path.
    pub avatar_asset_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<Character> for CharacterDto {
    fn from(value: Character) -> Self {
        Self {
            id: value.id,
            name: value.name,
            description: value.description,
            source_hash: value.source_hash,
            avatar_asset_id: value.avatar_asset_hash,
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterRenderAssetDto {
    pub asset_id: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterDisplayTransformDto {
    pub pattern: String,
    pub replacement: String,
    pub flags: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterRuntimeScriptDto {
    pub id: String,
    pub name: String,
    pub event: String,
    pub language: String,
    pub source: String,
    pub elevated_access: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct CharacterRuntimeKnowledgeDto {
    pub id: String,
    pub name: String,
    pub content: String,
    pub enabled: bool,
    pub primary_keys: Vec<String>,
    pub secondary_keys: Vec<String>,
    pub constant: bool,
    pub selective: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub use_regex: bool,
    pub probability_basis_points: u16,
    pub folder: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterRenderProfileDto {
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub assets: Vec<CharacterRenderAssetDto>,
    pub background_markup: String,
    pub toggle_schema: String,
    pub initial_variables: BTreeMap<String, String>,
    pub output_transforms: Vec<CharacterDisplayTransformDto>,
    pub display_transforms: Vec<CharacterDisplayTransformDto>,
    pub runtime_scripts: Vec<CharacterRuntimeScriptDto>,
    pub required_runtime_capabilities: Vec<String>,
    pub runtime_capabilities_declared: bool,
    pub runtime_knowledge: Vec<CharacterRuntimeKnowledgeDto>,
    pub runtime_script_count: u32,
}

impl CharacterRenderProfileDto {
    pub(crate) fn from_content(
        character_id: String,
        character_content_revision_id: Option<String>,
        content: CharacterContentV1,
    ) -> Self {
        let assets = render_assets(&content);
        let output_transforms = display_transforms(
            &content,
            lorepia_core::PortableTransformPhase::ProviderOutput,
        );
        let display_transforms =
            display_transforms(&content, lorepia_core::PortableTransformPhase::Display);
        let runtime_scripts = runtime_scripts(&content);
        let runtime_capabilities_declared = content.runtime.required_capabilities.is_some();
        let required_runtime_capabilities = content
            .runtime
            .required_capabilities
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|capability| capability.as_str().to_owned())
            .collect();
        let runtime_knowledge = runtime_knowledge(&content);
        let runtime_script_count = u32::try_from(content.runtime.scripts.len()).unwrap_or(u32::MAX);
        Self {
            character_id,
            character_content_revision_id,
            assets,
            background_markup: content.runtime.background_markup,
            toggle_schema: content.runtime.toggle_schema,
            initial_variables: content.runtime.initial_variables,
            output_transforms,
            display_transforms,
            runtime_scripts,
            required_runtime_capabilities,
            runtime_capabilities_declared,
            runtime_knowledge,
            runtime_script_count,
        }
    }
}

fn render_assets(content: &CharacterContentV1) -> Vec<CharacterRenderAssetDto> {
    content
        .assets
        .iter()
        .map(|asset| CharacterRenderAssetDto {
            asset_id: asset.id.as_str().to_owned(),
            aliases: character_asset_aliases(asset),
        })
        .collect()
}

fn display_transforms(
    content: &CharacterContentV1,
    phase: lorepia_core::PortableTransformPhase,
) -> Vec<CharacterDisplayTransformDto> {
    content
        .runtime
        .transforms
        .iter()
        .filter(|transform| transform.enabled && transform.phase == phase)
        .map(|transform| CharacterDisplayTransformDto {
            pattern: transform.pattern.clone(),
            replacement: transform.replacement.clone(),
            flags: transform.flags.clone(),
        })
        .collect()
}

fn runtime_scripts(content: &CharacterContentV1) -> Vec<CharacterRuntimeScriptDto> {
    content
        .runtime
        .scripts
        .iter()
        .map(|script| CharacterRuntimeScriptDto {
            id: script.id.clone(),
            name: script.name.clone(),
            event: script.event.clone(),
            language: script.language.clone(),
            source: script.source.clone(),
            elevated_access: script.elevated_access,
        })
        .collect()
}

fn runtime_knowledge(content: &CharacterContentV1) -> Vec<CharacterRuntimeKnowledgeDto> {
    let mut remaining_lore_regex_rules = 128_usize;
    content
        .knowledge_book
        .as_ref()
        .and_then(|book| book.embedded.as_ref())
        .map(|book| {
            book.entries
                .iter()
                .filter(|entry| !entry.folder)
                .map(|entry| {
                    let mut enabled = entry.enabled;
                    let mut primary_keys = entry.primary_keys.clone();
                    let mut secondary_keys = entry.secondary_keys.clone();
                    let use_regex = entry.use_regex && !entry.constant;
                    if enabled && use_regex {
                        primary_keys = take_reviewed_lore_regex_keys(
                            &primary_keys,
                            &mut remaining_lore_regex_rules,
                        );
                        secondary_keys = if entry.selective {
                            take_reviewed_lore_regex_keys(
                                &secondary_keys,
                                &mut remaining_lore_regex_rules,
                            )
                        } else {
                            Vec::new()
                        };
                        if primary_keys.is_empty() {
                            enabled = false;
                            secondary_keys.clear();
                        }
                    }
                    CharacterRuntimeKnowledgeDto {
                        id: entry.id.clone(),
                        name: entry.name.clone(),
                        content: entry.content.clone(),
                        enabled,
                        primary_keys,
                        secondary_keys,
                        constant: entry.constant,
                        selective: entry.selective,
                        case_sensitive: entry.case_sensitive,
                        whole_word: entry.whole_word,
                        use_regex,
                        probability_basis_points: entry.probability_basis_points,
                        folder: false,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn take_reviewed_lore_regex_keys(keys: &[String], remaining: &mut usize) -> Vec<String> {
    let values = keys
        .iter()
        .filter(|key| !key.is_empty())
        .take(*remaining)
        .cloned()
        .collect::<Vec<_>>();
    *remaining = remaining.saturating_sub(values.len());
    values
}

fn character_asset_aliases(asset: &AssetDescriptor) -> Vec<String> {
    let mut aliases = BTreeSet::new();
    add_asset_aliases(&mut aliases, &asset.name);
    if let Some(path) = asset.source.logical_path.as_deref() {
        add_asset_aliases(&mut aliases, path);
        if let Some(name) = path.rsplit('/').next() {
            add_asset_aliases(&mut aliases, name);
        }
    }
    aliases.into_iter().collect()
}

fn add_asset_aliases(aliases: &mut BTreeSet<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() || value.len() > 1_024 || value.chars().any(char::is_control) {
        return;
    }
    aliases.insert(value.to_owned());
    let mut current = value;
    for _ in 0..2 {
        let Some((stem, extension)) = current.rsplit_once('.') else {
            break;
        };
        if stem.is_empty()
            || extension.is_empty()
            || extension.len() > 8
            || !extension
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
        {
            break;
        }
        aliases.insert(stem.to_owned());
        current = stem;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CharacterGreetingKindDto {
    Default,
    Alternate,
}

impl From<CharacterGreetingKind> for CharacterGreetingKindDto {
    fn from(value: CharacterGreetingKind) -> Self {
        match value {
            CharacterGreetingKind::Default => Self::Default,
            CharacterGreetingKind::Alternate => Self::Alternate,
        }
    }
}

/// Safe greeting selector metadata. Source greeting text is intentionally
/// absent and can only be resolved by Core during an exact conversation start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterGreetingOptionDto {
    pub id: String,
    pub kind: CharacterGreetingKindDto,
    pub enabled: bool,
}

impl From<CharacterGreetingOption> for CharacterGreetingOptionDto {
    fn from(value: CharacterGreetingOption) -> Self {
        Self {
            id: value.id,
            kind: value.kind.into(),
            enabled: value.enabled,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterGreetingCatalogDto {
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub greetings: Vec<CharacterGreetingOptionDto>,
}

impl From<CharacterGreetingCatalog> for CharacterGreetingCatalogDto {
    fn from(value: CharacterGreetingCatalog) -> Self {
        Self {
            character_id: value.character_id,
            character_content_revision_id: value.character_content_revision_id,
            greetings: value.greetings.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKindDto {
    CharacterCardV3,
    CharacterCardPng,
    CharxPackage,
}

impl From<ContentKind> for ContentKindDto {
    fn from(value: ContentKind) -> Self {
        match value {
            ContentKind::CharacterCardV3 => Self::CharacterCardV3,
            ContentKind::CharacterCardPng => Self::CharacterCardPng,
            ContentKind::CharxPackage => Self::CharxPackage,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportWarningDto {
    pub code: String,
    pub message: String,
}

impl From<ImportWarning> for ImportWarningDto {
    fn from(value: ImportWarning) -> Self {
        Self {
            code: value.code,
            message: value.message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportImagePreviewDto {
    pub logical_asset_id: String,
    pub media_type: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRegexRuleReviewDto {
    pub id: String,
    pub name: String,
    pub phase: lorepia_core::ImportRegexRulePhase,
    pub runtime_index: u32,
    pub pattern: String,
    pub flags: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportDynamicContentReviewDto {
    pub runtime_script_count: u32,
    pub elevated_runtime_script_count: u32,
    pub required_runtime_capabilities: Vec<String>,
    pub runtime_capabilities_declared: bool,
    pub regex_rule_count: u32,
    pub enabled_regex_rule_count: u32,
    pub model_calls_possible: bool,
    pub custom_markup_present: bool,
    pub regex_rules: Vec<ImportRegexRuleReviewDto>,
}

impl From<ImportImagePreview> for ImportImagePreviewDto {
    fn from(value: ImportImagePreview) -> Self {
        Self {
            logical_asset_id: value.logical_asset_id,
            media_type: value.media_type,
            size_bytes: value.size_bytes,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportInspectionDto {
    pub inspection_id: String,
    pub kind: ContentKindDto,
    pub display_name: String,
    pub description: String,
    pub representative_image: Option<ImportImagePreviewDto>,
    pub source_sha256: String,
    pub source_size: u64,
    pub estimated_stored_size: u64,
    pub asset_count: u32,
    pub dynamic_content: ImportDynamicContentReviewDto,
    pub warnings: Vec<ImportWarningDto>,
    pub blocked_reasons: Vec<String>,
    pub unsupported_optional_fields: Vec<String>,
    pub allowed: bool,
}

impl From<ImportInspection> for ImportInspectionDto {
    fn from(value: ImportInspection) -> Self {
        let allowed = value.is_allowed();
        Self {
            inspection_id: value.id.0,
            kind: value.kind.into(),
            display_name: value.display_name,
            description: value.description,
            representative_image: value.representative_image.map(Into::into),
            source_sha256: value.source_sha256,
            source_size: value.source_size,
            estimated_stored_size: value.estimated_stored_size,
            asset_count: value.asset_count,
            dynamic_content: ImportDynamicContentReviewDto {
                runtime_script_count: value.dynamic_content.runtime_script_count,
                elevated_runtime_script_count: value.dynamic_content.elevated_runtime_script_count,
                required_runtime_capabilities: value
                    .dynamic_content
                    .required_runtime_capabilities
                    .iter()
                    .map(|capability| capability.as_str().to_owned())
                    .collect(),
                runtime_capabilities_declared: value.dynamic_content.runtime_capabilities_declared,
                regex_rule_count: value.dynamic_content.regex_rule_count,
                enabled_regex_rule_count: value.dynamic_content.enabled_regex_rule_count,
                model_calls_possible: value.dynamic_content.model_calls_possible,
                custom_markup_present: value.dynamic_content.custom_markup_present,
                regex_rules: value
                    .dynamic_content
                    .regex_rules
                    .into_iter()
                    .map(|rule| ImportRegexRuleReviewDto {
                        id: rule.id,
                        name: rule.name,
                        phase: rule.phase,
                        runtime_index: rule.runtime_index,
                        pattern: rule.pattern,
                        flags: rule.flags,
                    })
                    .collect(),
            },
            warnings: value.warnings.into_iter().map(Into::into).collect(),
            blocked_reasons: value.blocked_reasons,
            unsupported_optional_fields: value.unsupported_optional_fields,
            allowed,
        }
    }
}
