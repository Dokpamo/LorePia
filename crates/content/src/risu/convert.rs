use std::collections::BTreeSet;

use lorepia_domain::{
    BlockSource, CacheBoundary, CacheBoundaryId, CacheMode, CacheRoleFilter, CharacterContentV1,
    ContentCapability, ContentKind, ContentModule, ContentModuleId, CoreError, CoreErrorCode,
    CoreResult, HistorySelector, ImportDynamicContentReview, ImportWarning, InstructionAuthority,
    KnowledgeBook, MergePolicy, OverflowPolicy, PackageMetadata, PlacementZone, PresetMetadata,
    PromptBlock, PromptBlockId, PromptBlockKind, PromptPreset, PromptPresetId, Provenance,
    RoleHint, SafeTemplate, SourceKind, TemplatePart, TokenPolicy, TransformSet, TransformSetId,
    ValidateOrchestration, VariableMap,
};
use serde::Serialize;
use serde_json::{Value, json};

use super::container::{RisuAssetRecord, RisuModuleSource};
use crate::{adapters, dynamic_content_review, runtime};

const DOCUMENT_SCHEMA_VERSION: u32 = 1;
const MAX_NAME_CHARS: usize = 512;
const MAX_TEMPLATE_CHARS: usize = 262_144;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub(super) enum NormalizedDocument {
    PromptPreset(PromptPreset),
    KnowledgeBook(KnowledgeBook),
    TransformSet(TransformSet),
    ContentModule(ContentModule),
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct NormalizedDocumentEntry {
    pub(super) id: String,
    pub(super) path: String,
    pub(super) kind: &'static str,
    pub(super) required_capabilities: Vec<&'static str>,
    pub(super) depends_on: Vec<String>,
    pub(super) document: NormalizedDocument,
}

pub(super) struct NormalizedExternalContent {
    pub(super) kind: ContentKind,
    pub(super) name: String,
    pub(super) description: String,
    pub(super) documents: Vec<NormalizedDocumentEntry>,
    pub(super) assets: Vec<RisuAssetRecord>,
    pub(super) package_capabilities: Vec<&'static str>,
    pub(super) dynamic_content: ImportDynamicContentReview,
    pub(super) warnings: Vec<ImportWarning>,
    pub(super) unsupported_optional_fields: Vec<String>,
}

pub(super) fn convert_module(
    mut source: RisuModuleSource,
    source_sha256: &str,
) -> CoreResult<NormalizedExternalContent> {
    let source_id = format!("risu-module:{source_sha256}");
    let provenance = imported_provenance(&source_id, source_sha256);
    let name = bounded_name(&source.metadata.name, "Risu module");
    let description = source.metadata.description.clone();
    let original_transform_count = source.metadata.profile.transforms.len();
    let original_script_count = source.metadata.profile.scripts.len();
    let runtime_knowledge = source
        .metadata
        .knowledge_entries
        .as_ref()
        .map(|entries| adapters::parse_runtime_knowledge_book(entries, source_sha256, &name))
        .transpose()?
        .flatten();
    let dynamic_content = dynamic_content_review(&CharacterContentV1 {
        knowledge_book: runtime_knowledge.clone(),
        runtime: source.metadata.profile.clone(),
        ..CharacterContentV1::default()
    });

    let prefix = &source_sha256[..24];
    source.metadata.profile.transform_set_id =
        Some(TransformSetId::from(format!("risu-transform-{prefix}")));
    let transform_set = source
        .metadata
        .profile
        .materialize_transform_set(provenance.clone());
    let retained_transform_count = transform_set.as_ref().map_or(0, |set| set.rules.len());
    let knowledge_book = runtime_knowledge
        .and_then(|reference| reference.embedded)
        .map(|book| book.materialize(provenance.clone()));

    let mut warnings = source.metadata.warnings;
    if original_script_count > 0 {
        warnings.push(ImportWarning {
            code: "risu_scripts_quarantined".to_owned(),
            message: format!(
                "Quarantined {original_script_count} Risu trigger script(s). LorePia imports only declarative module behavior."
            ),
        });
    }
    if retained_transform_count < original_transform_count {
        warnings.push(ImportWarning {
            code: "risu_regex_rules_quarantined".to_owned(),
            message: format!(
                "Quarantined {} disabled or non-portable regex rule(s); {retained_transform_count} safe rule(s) remain available for explicit activation.",
                original_transform_count - retained_transform_count
            ),
        });
    }
    if !source.metadata.profile.background_markup.trim().is_empty() {
        warnings.push(ImportWarning {
            code: "risu_markup_quarantined".to_owned(),
            message: "Custom Risu HTML/CSS background markup was quarantined and will not run."
                .to_owned(),
        });
    }
    if !source.metadata.profile.toggle_schema.trim().is_empty() {
        warnings.push(ImportWarning {
            code: "risu_toggle_schema_preserved_inactive".to_owned(),
            message: "Risu's custom toggle schema is not executed; compatible declarative content was imported inactive."
                .to_owned(),
        });
    }
    let corrected_asset_types = source
        .assets
        .iter()
        .filter(|asset| asset.declared_extension_mismatch)
        .count();
    if corrected_asset_types > 0 {
        warnings.push(ImportWarning {
            code: "risu_asset_media_type_corrected".to_owned(),
            message: format!(
                "Corrected {corrected_asset_types} asset extension(s) from their verified media signatures."
            ),
        });
    }

    let mut documents = Vec::new();
    let mut dependency_ids = Vec::new();
    let mut knowledge_book_ids = Vec::new();
    let mut transform_set_ids = Vec::new();
    if let Some(book) = knowledge_book {
        knowledge_book_ids.push(book.id.clone());
        dependency_ids.push("knowledge".to_owned());
        documents.push(NormalizedDocumentEntry {
            id: "knowledge".to_owned(),
            path: "knowledge/risu-knowledge.json".to_owned(),
            kind: "knowledge",
            required_capabilities: vec!["knowledge_books"],
            depends_on: Vec::new(),
            document: NormalizedDocument::KnowledgeBook(book),
        });
    }
    if let Some(set) = transform_set {
        transform_set_ids.push(set.id.clone());
        dependency_ids.push("transform".to_owned());
        documents.push(NormalizedDocumentEntry {
            id: "transform".to_owned(),
            path: "transforms/risu-transform.json".to_owned(),
            kind: "transform",
            required_capabilities: vec!["safe_transforms"],
            depends_on: Vec::new(),
            document: NormalizedDocument::TransformSet(set),
        });
    }

    let mut asset_ids = Vec::new();
    let mut seen_assets = BTreeSet::new();
    for (index, asset) in source.assets.iter().enumerate() {
        if seen_assets.insert(asset.sha256.clone()) {
            asset_ids.push(lorepia_domain::AssetId::from(format!(
                "sha256:{}",
                asset.sha256
            )));
            dependency_ids.push(format!("asset-{index}"));
        }
    }
    let mut required_capabilities = Vec::new();
    if !knowledge_book_ids.is_empty() {
        required_capabilities.push(ContentCapability::Knowledge);
    }
    if !transform_set_ids.is_empty() {
        required_capabilities.push(ContentCapability::Transforms);
    }
    if !asset_ids.is_empty() {
        required_capabilities.push(ContentCapability::ImageAssets);
    }
    let module = ContentModule {
        id: ContentModuleId::from(format!("risu-module-{prefix}")),
        name: name.clone(),
        version: "1.0.0".to_owned(),
        schema_version: DOCUMENT_SCHEMA_VERSION,
        prompt_fragments: Vec::new(),
        knowledge_book_ids,
        control_specs: Vec::new(),
        transform_set_ids,
        interaction_rule_set_ids: Vec::new(),
        asset_ids,
        imported_components_enabled: false,
        required_capabilities,
        metadata: PackageMetadata {
            author: None,
            license: "LicenseRef-Risu-User-Content".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: description.clone(),
            tags: vec!["risu".to_owned(), "imported-module".to_owned()],
            provenance,
        },
    };
    module
        .validate()
        .map_err(|error| unsupported(format!("Risu module cannot be normalized: {error}")))?;
    documents.push(NormalizedDocumentEntry {
        id: "module".to_owned(),
        path: "modules/risu-module.json".to_owned(),
        kind: "content_module",
        required_capabilities: vec!["content_modules"],
        depends_on: dependency_ids,
        document: NormalizedDocument::ContentModule(module),
    });

    let mut package_capabilities = vec!["content_modules"];
    if documents
        .iter()
        .any(|document| document.kind == "knowledge")
    {
        package_capabilities.push("knowledge_books");
    }
    if documents
        .iter()
        .any(|document| document.kind == "transform")
    {
        package_capabilities.push("safe_transforms");
    }
    if !source.assets.is_empty() {
        package_capabilities.push("image_assets");
    }
    Ok(NormalizedExternalContent {
        kind: ContentKind::RisuModule,
        name,
        description,
        documents,
        assets: source.assets,
        package_capabilities,
        dynamic_content,
        warnings,
        unsupported_optional_fields: vec![
            "trigger scripts (quarantined)".to_owned(),
            "custom Risu toggle/background runtime".to_owned(),
        ],
    })
}

pub(super) fn convert_preset(
    value: &Value,
    source_sha256: &str,
) -> CoreResult<NormalizedExternalContent> {
    let object = value
        .as_object()
        .ok_or_else(|| unsupported("Risu preset root must be an object"))?;
    let name = bounded_name(
        object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "Risu preset",
    );
    let prefix = &source_sha256[..24];
    let provenance = imported_provenance(&format!("risu-preset:{source_sha256}"), source_sha256);
    let (mut blocks, cache_boundaries, skipped_prompt_items) =
        prompt_blocks(object.get("promptTemplate"), prefix, &provenance)?;
    if blocks.is_empty() {
        return Err(unsupported("Risu preset has no portable prompt blocks"));
    }
    append_latest_user_block(&mut blocks, prefix, &provenance);
    blocks.sort_by_key(|block| block.placement_zone);

    let runtime_root = json!({
        "type": "risuModule",
        "module": {
            "name": name,
            "regex": object.get("regex").cloned().unwrap_or(Value::Array(Vec::new())),
            "assets": [],
            "trigger": []
        }
    });
    let mut runtime = runtime::decode_runtime_metadata_value(&runtime_root, source_sha256)?;
    let original_transform_count = runtime.profile.transforms.len();
    runtime.profile.transform_set_id = Some(TransformSetId::from(format!(
        "risu-preset-transform-{prefix}"
    )));
    let transform_set = runtime
        .profile
        .materialize_transform_set(provenance.clone());
    let retained_transform_count = transform_set.as_ref().map_or(0, |set| set.rules.len());
    let dynamic_content = dynamic_content_review(&CharacterContentV1 {
        runtime: runtime.profile,
        ..CharacterContentV1::default()
    });

    let mut documents = Vec::new();
    let mut transform_set_ids = Vec::new();
    let mut prompt_dependencies = Vec::new();
    if let Some(set) = transform_set {
        transform_set_ids.push(set.id.clone());
        prompt_dependencies.push("transform".to_owned());
        documents.push(NormalizedDocumentEntry {
            id: "transform".to_owned(),
            path: "transforms/risu-preset-transform.json".to_owned(),
            kind: "transform",
            required_capabilities: vec!["safe_transforms"],
            depends_on: Vec::new(),
            document: NormalizedDocument::TransformSet(set),
        });
    }
    let api_type = object
        .get("apiType")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let model = object
        .get("aiModel")
        .and_then(Value::as_str)
        .unwrap_or("unbound");
    let preset = PromptPreset {
        id: PromptPresetId::from(format!("risu-preset-{prefix}")),
        name: name.clone(),
        schema_version: DOCUMENT_SCHEMA_VERSION,
        blocks,
        controls: Vec::new(),
        default_values: VariableMap::default(),
        default_generation_preset_id: None,
        memory_profile_id: None,
        knowledge_book_ids: Vec::new(),
        transform_set_ids,
        module_ids: Vec::new(),
        cache_boundaries,
        metadata: imported_preset_metadata(
            format!(
                "Imported Risu prompt preset. Original API type: {api_type}; model hint: {model}. Provider parameters remain unbound until a LorePia model route is selected."
            ),
            vec!["risu".to_owned(), "imported-preset".to_owned()],
            provenance,
        )?,
    };
    preset
        .validate()
        .map_err(|error| unsupported(format!("Risu preset cannot be normalized: {error}")))?;
    documents.push(NormalizedDocumentEntry {
        id: "prompt".to_owned(),
        path: "prompt/risu-preset.json".to_owned(),
        kind: "prompt",
        required_capabilities: vec!["prompt_presets"],
        depends_on: prompt_dependencies,
        document: NormalizedDocument::PromptPreset(preset),
    });

    let mut warnings = runtime.warnings;
    warnings.push(ImportWarning {
        code: "risu_provider_parameters_unbound".to_owned(),
        message: "Prompt blocks are imported, but provider/model sampling values remain unbound until you choose a LorePia model route."
            .to_owned(),
    });
    if skipped_prompt_items > 0 {
        warnings.push(ImportWarning {
            code: "risu_prompt_items_quarantined".to_owned(),
            message: format!(
                "Quarantined {skipped_prompt_items} empty or unsupported Risu prompt item(s)."
            ),
        });
    }
    if retained_transform_count < original_transform_count {
        warnings.push(ImportWarning {
            code: "risu_regex_rules_quarantined".to_owned(),
            message: format!(
                "Quarantined {} disabled or non-portable regex rule(s); {retained_transform_count} safe rule(s) remain available for explicit activation.",
                original_transform_count - retained_transform_count
            ),
        });
    }
    let mut package_capabilities = vec!["prompt_presets"];
    if retained_transform_count > 0 {
        package_capabilities.push("safe_transforms");
    }
    Ok(NormalizedExternalContent {
        kind: ContentKind::RisuPreset,
        name,
        description: format!("Risu prompt preset for {model}"),
        documents,
        assets: Vec::new(),
        package_capabilities,
        dynamic_content,
        warnings,
        unsupported_optional_fields: vec![
            "provider credentials and endpoint overrides".to_owned(),
            "provider-specific sampling values (unbound)".to_owned(),
        ],
    })
}

pub(super) fn convert_memory_preset(
    value: &Value,
    source_sha256: &str,
) -> CoreResult<NormalizedExternalContent> {
    let root = value
        .as_object()
        .ok_or_else(|| unsupported("Risu JSON root must be an object"))?;
    if root.get("type").and_then(Value::as_str) != Some("risu")
        || root.get("ver").and_then(Value::as_u64) != Some(1)
    {
        return Err(unsupported("Risu JSON type or version is unsupported"));
    }
    let data = root
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| unsupported("Risu JSON has no data object"))?;
    let settings = data
        .get("settings")
        .and_then(Value::as_object)
        .ok_or_else(|| unsupported("Risu JSON has no settings object"))?;
    let summary_prompt = settings
        .get("summarizationPrompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| unsupported("Risu memory preset has no summarization prompt"))?;
    let name = bounded_name(
        data.get("name").and_then(Value::as_str).unwrap_or_default(),
        "Risu memory preset",
    );
    let prefix = &source_sha256[..24];
    let provenance = imported_provenance(
        &format!("risu-memory-preset:{source_sha256}"),
        source_sha256,
    );
    let block = static_prompt_block(
        format!("risu-memory-summary-{prefix}"),
        "Memory summarization instruction".to_owned(),
        summary_prompt,
        RoleHint::System,
        0,
        &provenance,
    )?;
    let mut blocks = vec![block];
    append_latest_user_block(&mut blocks, prefix, &provenance);
    blocks.sort_by_key(|block| block.placement_zone);
    let preset = PromptPreset {
        id: PromptPresetId::from(format!("risu-memory-prompt-{prefix}")),
        name: format!("{name} · 요약 프롬프트"),
        schema_version: DOCUMENT_SCHEMA_VERSION,
        blocks,
        controls: Vec::new(),
        default_values: VariableMap::default(),
        default_generation_preset_id: None,
        memory_profile_id: None,
        knowledge_book_ids: Vec::new(),
        transform_set_ids: Vec::new(),
        module_ids: Vec::new(),
        cache_boundaries: Vec::new(),
        metadata: imported_preset_metadata(
            "Imported from a Risu Hypa memory preset. Bind this summarization prompt to a configured LorePia memory task before activation."
                .to_owned(),
            vec!["risu".to_owned(), "memory-template".to_owned()],
            provenance,
        )?,
    };
    preset.validate().map_err(|error| {
        unsupported(format!("Risu memory preset cannot be normalized: {error}"))
    })?;
    Ok(NormalizedExternalContent {
        kind: ContentKind::RisuMemoryPreset,
        name,
        description: "Risu Hypa memory summarization template".to_owned(),
        documents: vec![NormalizedDocumentEntry {
            id: "prompt".to_owned(),
            path: "prompt/risu-memory-prompt.json".to_owned(),
            kind: "prompt",
            required_capabilities: vec!["prompt_presets"],
            depends_on: Vec::new(),
            document: NormalizedDocument::PromptPreset(preset),
        }],
        assets: Vec::new(),
        package_capabilities: vec!["prompt_presets"],
        dynamic_content: ImportDynamicContentReview::default(),
        warnings: vec![ImportWarning {
            code: "risu_memory_task_binding_required".to_owned(),
            message: "The summarization prompt is imported safely; Risu scheduling/model fields need a configured LorePia memory task before activation."
                .to_owned(),
        }],
        unsupported_optional_fields: settings
            .keys()
            .filter(|key| key.as_str() != "summarizationPrompt")
            .take(128)
            .cloned()
            .collect(),
    })
}

fn prompt_blocks(
    value: Option<&Value>,
    prefix: &str,
    provenance: &Provenance,
) -> CoreResult<(Vec<PromptBlock>, Vec<CacheBoundary>, usize)> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| unsupported("Risu preset promptTemplate must be an array"))?;
    let mut blocks: Vec<PromptBlock> = Vec::new();
    let mut boundaries = Vec::new();
    let mut skipped = 0_usize;
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            skipped += 1;
            continue;
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("plain");
        if kind == "cache" {
            if let Some(previous) = blocks.last() {
                boundaries.push(CacheBoundary {
                    id: CacheBoundaryId::from(format!("risu-cache-{prefix}-{index}")),
                    after_block_id: previous.id.clone(),
                    role_filter: CacheRoleFilter::All,
                    ttl: lorepia_domain::CacheTtl::ProviderDefault,
                    mode: CacheMode::Explicit,
                });
            } else {
                skipped += 1;
            }
            continue;
        }
        let name = bounded_name(
            object
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            &format!("Risu prompt item {}", index + 1),
        );
        let text = object
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let role = role_hint(object.get("role").and_then(Value::as_str));
        let id = format!("risu-block-{prefix}-{index}");
        let block = match kind {
            "persona" => dynamic_prompt_block(
                id,
                name,
                PromptBlockKind::UserPersona,
                BlockSource::UserPersona,
                RoleHint::User,
                PlacementZone::CharacterContext,
                None,
                provenance,
            ),
            "description" => dynamic_prompt_block(
                id,
                name,
                PromptBlockKind::CharacterDescription,
                BlockSource::CharacterField {
                    field: lorepia_domain::CharacterField::Description,
                },
                role,
                PlacementZone::CharacterContext,
                None,
                provenance,
            ),
            "lorebook" => dynamic_prompt_block(
                id,
                name,
                PromptBlockKind::WorldKnowledge,
                BlockSource::SelectedKnowledge,
                role,
                PlacementZone::RetrievedContext,
                None,
                provenance,
            ),
            "memory" => dynamic_prompt_block(
                id,
                name,
                PromptBlockKind::RetrievedMemory,
                BlockSource::SelectedMemory,
                role,
                PlacementZone::RetrievedContext,
                None,
                provenance,
            ),
            "authornote" => dynamic_prompt_block(
                id,
                name,
                PromptBlockKind::AuthorNote,
                BlockSource::AuthorNote,
                role,
                PlacementZone::RecentEnhancement,
                None,
                provenance,
            ),
            "chat" | "chatML" => dynamic_prompt_block(
                id,
                name,
                PromptBlockKind::HistorySlice,
                BlockSource::History,
                role,
                PlacementZone::RecentHistory,
                Some(HistorySelector::All),
                provenance,
            ),
            "plain" | "postEverything" if !text.trim().is_empty() => {
                static_prompt_block(id, name, text, role, index, provenance)?
            }
            _ => {
                skipped += 1;
                continue;
            }
        };
        blocks.push(block);
    }
    Ok((blocks, boundaries, skipped))
}

fn static_prompt_block(
    id: String,
    name: String,
    text: &str,
    role_hint: RoleHint,
    order: usize,
    provenance: &Provenance,
) -> CoreResult<PromptBlock> {
    if text.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(unsupported(format!(
            "Risu prompt block exceeds {MAX_TEMPLATE_CHARS} characters: {name}"
        )));
    }
    Ok(PromptBlock {
        id: PromptBlockId::from(id),
        name,
        kind: PromptBlockKind::StaticInstruction,
        enabled: true,
        role_hint,
        authority: InstructionAuthority::ImportedContent,
        template: Some(SafeTemplate {
            parts: vec![TemplatePart::Text {
                value: text.to_owned(),
            }],
            max_output_chars: u32::try_from(text.chars().count().max(1))
                .unwrap_or(262_144)
                .min(262_144),
        }),
        condition: None,
        source: BlockSource::Template,
        placement_zone: PlacementZone::PresetInstruction,
        history_selector: None,
        token_policy: TokenPolicy {
            priority: u16::try_from(1_000_usize.saturating_sub(order)).unwrap_or(0),
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::TrimTail,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn dynamic_prompt_block(
    id: String,
    name: String,
    kind: PromptBlockKind,
    source: BlockSource,
    role_hint: RoleHint,
    placement_zone: PlacementZone,
    history_selector: Option<HistorySelector>,
    provenance: &Provenance,
) -> PromptBlock {
    PromptBlock {
        id: PromptBlockId::from(id),
        name,
        kind,
        enabled: true,
        role_hint,
        authority: InstructionAuthority::ImportedContent,
        template: None,
        condition: None,
        source,
        placement_zone,
        history_selector,
        token_policy: TokenPolicy {
            priority: 500,
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::TrimHead,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance.clone(),
    }
}

fn append_latest_user_block(blocks: &mut Vec<PromptBlock>, prefix: &str, provenance: &Provenance) {
    blocks.push(PromptBlock {
        id: PromptBlockId::from(format!("risu-latest-user-{prefix}")),
        name: "Latest user message".to_owned(),
        kind: PromptBlockKind::LatestUserTurn,
        enabled: true,
        role_hint: RoleHint::User,
        authority: InstructionAuthority::ImportedContent,
        template: None,
        condition: None,
        source: BlockSource::LatestUser,
        placement_zone: PlacementZone::LatestUser,
        history_selector: None,
        token_policy: TokenPolicy {
            priority: u16::MAX,
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::Reject,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance.clone(),
    });
}

fn role_hint(value: Option<&str>) -> RoleHint {
    match value {
        Some("system") => RoleHint::System,
        Some("user") => RoleHint::User,
        Some("bot" | "assistant") => RoleHint::Assistant,
        _ => RoleHint::ProviderDefault,
    }
}

fn imported_provenance(source_id: &str, source_sha256: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::ImportedStandard,
        source_id: Some(source_id.to_owned()),
        source_hash: Some(source_sha256.to_owned()),
        author: None,
        license: Some("LicenseRef-Risu-User-Content".to_owned()),
        imported_at: None,
    }
}

fn imported_preset_metadata(
    description: String,
    tags: Vec<String>,
    provenance: Provenance,
) -> CoreResult<PresetMetadata> {
    serde_json::from_value(json!({
        "description": description,
        "tags": tags,
        "provenance": provenance,
        "created_at": "1970-01-01T00:00:00Z",
        "updated_at": "1970-01-01T00:00:00Z",
        "local_override_of": null
    }))
    .map_err(|error| unsupported(format!("Risu preset metadata is invalid: {error}")))
}

fn bounded_name(value: &str, fallback: &str) -> String {
    let value = value.trim();
    let source = if value.is_empty() { fallback } else { value };
    source.chars().take(MAX_NAME_CHARS).collect()
}

pub(super) fn looks_like_memory_preset(value: &Value) -> bool {
    value.get("type").and_then(Value::as_str) == Some("risu")
        && value
            .pointer("/data/settings/summarizationPrompt")
            .and_then(Value::as_str)
            .is_some()
}

fn unsupported(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::UnsupportedContent, message, false)
}
