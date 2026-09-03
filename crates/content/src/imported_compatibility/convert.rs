use std::collections::BTreeSet;

use lorepia_domain::{
    CacheBoundary, CharacterContentV1, ContentCapability, ContentKind, ContentModule,
    ContentModuleId, ControlSpec, CoreError, CoreErrorCode, CoreResult, ImportDynamicContentReview,
    ImportWarning, KnowledgeBook, PackageMetadata, PresetMetadata, PromptBlock, PromptPreset,
    PromptPresetId, Provenance, RoleHint, SafeTemplate, SourceKind, TemplatePart, TransformSet,
    TransformSetId, ValidateOrchestration, VariableMap, VariableValue,
};
use serde::Serialize;
use serde_json::{Value, json};

use super::container::{ImportedAssetRecord, ImportedModuleSource};
use crate::{adapters, dynamic_content_review, runtime};

mod prompts;
mod variables;

use prompts::{append_latest_user_block, prompt_blocks, static_prompt_block};
use variables::{
    import_hint_value, insert_import_hint, preserve_generation_hints, prompt_controls,
};

const DOCUMENT_SCHEMA_VERSION: u32 = 1;
const MAX_NAME_CHARS: usize = 512;
const MAX_TEMPLATE_CHARS: usize = 262_144;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub(super) enum NormalizedDocument {
    PromptPreset(Box<PromptPreset>),
    KnowledgeBook(KnowledgeBook),
    TransformSet(TransformSet),
    ContentModule(Box<ContentModule>),
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
    pub(super) assets: Vec<ImportedAssetRecord>,
    pub(super) package_capabilities: Vec<&'static str>,
    pub(super) dynamic_content: ImportDynamicContentReview,
    pub(super) warnings: Vec<ImportWarning>,
    pub(super) unsupported_optional_fields: Vec<String>,
}

pub(super) fn convert_module(
    mut source: ImportedModuleSource,
    source_sha256: &str,
) -> CoreResult<NormalizedExternalContent> {
    let source_id = format!("risu-module:{source_sha256}");
    let provenance = imported_provenance(&source_id, source_sha256);
    let name = bounded_name(&source.metadata.name, "Imported module");
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
    let portable_runtime =
        runtime_profile_present(&source.metadata.profile).then(|| source.metadata.profile.clone());
    let warnings = module_warnings(
        source.metadata.warnings,
        original_script_count,
        original_transform_count,
        retained_transform_count,
        !source.metadata.profile.background_markup.trim().is_empty(),
        !source.metadata.profile.toggle_schema.trim().is_empty(),
        &source.assets,
    );
    let documents = module_documents(ModuleDocumentsInput {
        name: &name,
        description: &description,
        prefix,
        provenance,
        knowledge_book,
        transform_set,
        portable_runtime,
        assets: &source.assets,
    })?;
    let package_capabilities = module_package_capabilities(&documents, !source.assets.is_empty());
    Ok(NormalizedExternalContent {
        kind: ContentKind::RisuModule,
        name,
        description,
        documents,
        assets: source.assets,
        package_capabilities,
        dynamic_content,
        warnings,
        unsupported_optional_fields: Vec::new(),
    })
}

fn runtime_profile_present(profile: &lorepia_domain::CharacterRuntimeProfile) -> bool {
    !profile.transforms.is_empty()
        || !profile.scripts.is_empty()
        || !profile.background_markup.trim().is_empty()
        || !profile.toggle_schema.trim().is_empty()
        || !profile.initial_variables.is_empty()
}

struct ModuleDocumentsInput<'a> {
    name: &'a str,
    description: &'a str,
    prefix: &'a str,
    provenance: Provenance,
    knowledge_book: Option<KnowledgeBook>,
    transform_set: Option<TransformSet>,
    portable_runtime: Option<lorepia_domain::CharacterRuntimeProfile>,
    assets: &'a [ImportedAssetRecord],
}

fn module_documents(input: ModuleDocumentsInput<'_>) -> CoreResult<Vec<NormalizedDocumentEntry>> {
    let ModuleDocumentsInput {
        name,
        description,
        prefix,
        provenance,
        knowledge_book,
        transform_set,
        portable_runtime,
        assets,
    } = input;
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

    let asset_ids = module_asset_ids(assets, &mut dependency_ids);
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
    if portable_runtime.is_some() {
        required_capabilities.push(ContentCapability::PortableRuntime);
    }
    let module = ContentModule {
        id: ContentModuleId::from(format!("risu-module-{prefix}")),
        name: name.to_owned(),
        version: "1.0.0".to_owned(),
        schema_version: DOCUMENT_SCHEMA_VERSION,
        prompt_fragments: Vec::new(),
        knowledge_book_ids,
        control_specs: Vec::new(),
        transform_set_ids,
        interaction_rule_set_ids: Vec::new(),
        asset_ids,
        portable_runtime,
        imported_components_enabled: true,
        required_capabilities,
        metadata: PackageMetadata {
            author: None,
            license: "LicenseRef-Imported-User-Content".to_owned(),
            redistribution_allowed: false,
            homepage: None,
            description: description.to_owned(),
            tags: vec!["compatibility".to_owned(), "imported-module".to_owned()],
            provenance,
        },
    };
    module
        .validate()
        .map_err(|error| unsupported(format!("imported module cannot be normalized: {error}")))?;
    documents.push(NormalizedDocumentEntry {
        id: "module".to_owned(),
        path: "modules/risu-module.json".to_owned(),
        kind: "content_module",
        required_capabilities: vec!["content_modules"],
        depends_on: dependency_ids,
        document: NormalizedDocument::ContentModule(Box::new(module)),
    });
    Ok(documents)
}

fn module_asset_ids(
    assets: &[ImportedAssetRecord],
    dependency_ids: &mut Vec<String>,
) -> Vec<lorepia_domain::AssetId> {
    let mut asset_ids = Vec::new();
    let mut seen_assets = BTreeSet::new();
    for (index, asset) in assets.iter().enumerate() {
        if seen_assets.insert(asset.sha256.clone()) {
            asset_ids.push(lorepia_domain::AssetId::from(format!(
                "sha256:{}",
                asset.sha256
            )));
            dependency_ids.push(format!("asset-{index}"));
        }
    }
    asset_ids
}

fn module_package_capabilities(
    documents: &[NormalizedDocumentEntry],
    has_assets: bool,
) -> Vec<&'static str> {
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
    if documents.iter().any(|document| {
        matches!(
            &document.document,
            NormalizedDocument::ContentModule(module) if module.portable_runtime.is_some()
        )
    }) {
        package_capabilities.push("portable_runtime");
    }
    if has_assets {
        package_capabilities.push("image_assets");
    }
    package_capabilities
}

fn module_warnings(
    mut warnings: Vec<ImportWarning>,
    original_script_count: usize,
    original_transform_count: usize,
    retained_transform_count: usize,
    has_background_markup: bool,
    has_toggle_schema: bool,
    assets: &[ImportedAssetRecord],
) -> Vec<ImportWarning> {
    if original_script_count > 0 {
        warnings.push(ImportWarning {
            code: "imported_scripts_sandboxed".to_owned(),
            message: format!(
                "Preserved {original_script_count} imported Lua trigger script(s) for isolated, permission-gated Worker execution."
            ),
        });
    }
    if retained_transform_count < original_transform_count {
        warnings.push(ImportWarning {
            code: "imported_regex_rules_quarantined".to_owned(),
            message: format!(
                "Quarantined {} disabled or non-portable regex rule(s); {retained_transform_count} safe rule(s) remain available for explicit activation.",
                original_transform_count - retained_transform_count
            ),
        });
    }
    if has_background_markup {
        warnings.push(ImportWarning {
            code: "imported_markup_sandboxed".to_owned(),
            message: "Imported background markup will render only through LorePia's sanitized opaque-origin frame."
                .to_owned(),
        });
    }
    if has_toggle_schema {
        warnings.push(ImportWarning {
            code: "imported_toggle_schema_preserved".to_owned(),
            message: "Compatible imported toggle controls were preserved for the bound conversation runtime."
                .to_owned(),
        });
    }
    let corrected_asset_types = assets
        .iter()
        .filter(|asset| asset.declared_extension_mismatch)
        .count();
    if corrected_asset_types > 0 {
        warnings.push(ImportWarning {
            code: "imported_asset_media_type_corrected".to_owned(),
            message: format!(
                "Corrected {corrected_asset_types} asset extension(s) from their verified media signatures."
            ),
        });
    }
    warnings
}

pub(super) fn convert_preset(
    value: &Value,
    source_sha256: &str,
) -> CoreResult<NormalizedExternalContent> {
    let object = value
        .as_object()
        .ok_or_else(|| unsupported("imported preset root must be an object"))?;
    let name = bounded_name(
        object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        "Imported preset",
    );
    let prefix = &source_sha256[..24];
    let provenance = imported_provenance(&format!("risu-preset:{source_sha256}"), source_sha256);
    let (mut blocks, cache_boundaries, skipped_prompt_items) =
        prompt_blocks(object.get("promptTemplate"), prefix, &provenance)?;
    if blocks.is_empty() {
        return Err(unsupported("imported preset has no portable prompt blocks"));
    }
    append_latest_user_block(&mut blocks, prefix, &provenance);
    blocks.sort_by_key(|block| block.placement_zone);
    let (controls, mut default_values, skipped_toggle_lines) =
        prompt_controls(object.get("customPromptTemplateToggle"));
    let retained_toggle_count = controls.len();
    preserve_generation_hints(object, &mut default_values);

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

    let api_type = object
        .get("apiType")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let model = object
        .get("aiModel")
        .and_then(Value::as_str)
        .unwrap_or("unbound");
    let metadata_description = format!(
        "Imported prompt preset. Original API type: {api_type}; model hint: {model}. Provider parameters remain unbound until a LorePia model route is selected."
    );
    let documents = preset_documents(PresetDocumentsInput {
        name: &name,
        prefix,
        blocks,
        controls,
        default_values,
        cache_boundaries,
        transform_set,
        provenance,
        metadata_description,
    })?;
    let warnings = preset_warnings(
        runtime.warnings,
        skipped_prompt_items,
        skipped_toggle_lines,
        retained_toggle_count,
        original_transform_count,
        retained_transform_count,
    );
    let mut package_capabilities = vec!["prompt_presets"];
    if retained_transform_count > 0 {
        package_capabilities.push("safe_transforms");
    }
    Ok(NormalizedExternalContent {
        kind: ContentKind::RisuPreset,
        name,
        description: format!("Imported prompt preset for {model}"),
        documents,
        assets: Vec::new(),
        package_capabilities,
        dynamic_content,
        warnings,
        unsupported_optional_fields: vec!["provider credentials and endpoint overrides".to_owned()],
    })
}

fn preset_warnings(
    mut warnings: Vec<ImportWarning>,
    skipped_prompt_items: usize,
    skipped_toggle_lines: usize,
    retained_toggle_count: usize,
    original_transform_count: usize,
    retained_transform_count: usize,
) -> Vec<ImportWarning> {
    warnings.push(ImportWarning {
        code: "imported_provider_parameters_unbound".to_owned(),
        message: "Prompt blocks and portable provider hints are imported. Choose a LorePia model route in the compatibility panel to materialize supported sampling values."
            .to_owned(),
    });
    if retained_toggle_count > 0 {
        warnings.push(ImportWarning {
            code: "imported_prompt_toggles_preserved".to_owned(),
            message: format!(
                "Preserved {retained_toggle_count} imported prompt toggle(s) as room-scoped creator controls."
            ),
        });
    }
    if skipped_toggle_lines > 0 {
        warnings.push(ImportWarning {
            code: "imported_prompt_toggle_lines_skipped".to_owned(),
            message: format!(
                "Ignored {skipped_toggle_lines} imported toggle layout or unsupported control line(s); interactive values remain preserved for supported controls."
            ),
        });
    }
    if skipped_prompt_items > 0 {
        warnings.push(ImportWarning {
            code: "imported_prompt_items_quarantined".to_owned(),
            message: format!(
                "Quarantined {skipped_prompt_items} empty or unsupported imported prompt item(s)."
            ),
        });
    }
    if retained_transform_count < original_transform_count {
        warnings.push(ImportWarning {
            code: "imported_regex_rules_quarantined".to_owned(),
            message: format!(
                "Quarantined {} disabled or non-portable regex rule(s); {retained_transform_count} safe rule(s) remain available for explicit activation.",
                original_transform_count - retained_transform_count
            ),
        });
    }
    warnings
}

struct PresetDocumentsInput<'a> {
    name: &'a str,
    prefix: &'a str,
    blocks: Vec<PromptBlock>,
    controls: Vec<ControlSpec>,
    default_values: VariableMap,
    cache_boundaries: Vec<CacheBoundary>,
    transform_set: Option<TransformSet>,
    provenance: Provenance,
    metadata_description: String,
}

fn preset_documents(input: PresetDocumentsInput<'_>) -> CoreResult<Vec<NormalizedDocumentEntry>> {
    let PresetDocumentsInput {
        name,
        prefix,
        blocks,
        controls,
        default_values,
        cache_boundaries,
        transform_set,
        provenance,
        metadata_description,
    } = input;
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
    let preset = PromptPreset {
        id: PromptPresetId::from(format!("risu-preset-{prefix}")),
        name: name.to_owned(),
        schema_version: DOCUMENT_SCHEMA_VERSION,
        blocks,
        controls,
        default_values,
        default_generation_preset_id: None,
        memory_profile_id: None,
        knowledge_book_ids: Vec::new(),
        transform_set_ids,
        module_ids: Vec::new(),
        cache_boundaries,
        metadata: imported_preset_metadata(
            metadata_description,
            vec!["compatibility".to_owned(), "imported-preset".to_owned()],
            provenance,
        )?,
    };
    preset
        .validate()
        .map_err(|error| unsupported(format!("imported preset cannot be normalized: {error}")))?;
    documents.push(NormalizedDocumentEntry {
        id: "prompt".to_owned(),
        path: "prompt/risu-preset.json".to_owned(),
        kind: "prompt",
        required_capabilities: vec!["prompt_presets"],
        depends_on: prompt_dependencies,
        document: NormalizedDocument::PromptPreset(Box::new(preset)),
    });
    Ok(documents)
}

pub(super) fn convert_memory_preset(
    value: &Value,
    source_sha256: &str,
) -> CoreResult<NormalizedExternalContent> {
    let root = value
        .as_object()
        .ok_or_else(|| unsupported("compatibility JSON root must be an object"))?;
    if root.get("type").and_then(Value::as_str) != Some("risu")
        || root.get("ver").and_then(Value::as_u64) != Some(1)
    {
        return Err(unsupported(
            "compatibility JSON type or version is unsupported",
        ));
    }
    let data = root
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| unsupported("compatibility JSON has no data object"))?;
    let settings = data
        .get("settings")
        .and_then(Value::as_object)
        .ok_or_else(|| unsupported("compatibility JSON has no settings object"))?;
    let summary_prompt = settings
        .get("summarizationPrompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| unsupported("imported memory preset has no summarization prompt"))?;
    let name = bounded_name(
        data.get("name").and_then(Value::as_str).unwrap_or_default(),
        "Imported memory preset",
    );
    let prefix = &source_sha256[..24];
    let provenance = imported_provenance(
        &format!("risu-memory-preset:{source_sha256}"),
        source_sha256,
    );
    imported_memory_summary_template(summary_prompt)?;
    let preset = memory_prompt_preset(MemoryPromptPresetInput {
        name: &name,
        prefix,
        summary_prompt,
        settings,
        provenance,
    })?;
    Ok(NormalizedExternalContent {
        kind: ContentKind::RisuMemoryPreset,
        name,
        description: "Imported memory summarization template".to_owned(),
        documents: vec![NormalizedDocumentEntry {
            id: "prompt".to_owned(),
            path: "prompt/risu-memory-prompt.json".to_owned(),
            kind: "prompt",
            required_capabilities: vec!["prompt_presets"],
            depends_on: Vec::new(),
            document: NormalizedDocument::PromptPreset(Box::new(preset)),
        }],
        assets: Vec::new(),
        package_capabilities: vec!["prompt_presets"],
        dynamic_content: ImportDynamicContentReview::default(),
        warnings: vec![ImportWarning {
            code: "imported_memory_task_binding_required".to_owned(),
            message: "The summary template, schedule, retrieval weights, and preservation policy were imported as setup data. Choose a LorePia model route in the compatibility panel to create the linked task and memory profile before activation."
                .to_owned(),
        }],
        unsupported_optional_fields: unsupported_memory_settings(settings),
    })
}

struct MemoryPromptPresetInput<'a> {
    name: &'a str,
    prefix: &'a str,
    summary_prompt: &'a str,
    settings: &'a serde_json::Map<String, Value>,
    provenance: Provenance,
}

fn memory_prompt_preset(input: MemoryPromptPresetInput<'_>) -> CoreResult<PromptPreset> {
    let MemoryPromptPresetInput {
        name,
        prefix,
        summary_prompt,
        settings,
        provenance,
    } = input;
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
    let mut default_values = VariableMap::default();
    insert_import_hint(
        &mut default_values,
        "lorepia_risu_import_kind",
        VariableValue::Enum("memory".to_owned()),
    );
    for (id, value) in [
        (
            "lorepia_risu_memory_profile_id",
            VariableValue::Text(format!("risu-memory-profile-{prefix}")),
        ),
        (
            "lorepia_risu_summary_task_id",
            VariableValue::Text(format!("risu-memory-summary-task-{prefix}")),
        ),
        (
            "lorepia_risu_requested_similarity_weight",
            VariableValue::Decimal(f64::from(bounded_setting_f32(
                settings,
                "similarMemoryRatio",
                0.4,
            ))),
        ),
    ] {
        insert_import_hint(&mut default_values, id, value);
    }
    preserve_memory_hints(settings, &mut default_values);
    let preset = PromptPreset {
        id: PromptPresetId::from(format!("risu-memory-prompt-{prefix}")),
        name: format!("{name} · 요약 프롬프트"),
        schema_version: DOCUMENT_SCHEMA_VERSION,
        blocks,
        controls: Vec::new(),
        default_values,
        default_generation_preset_id: None,
        memory_profile_id: None,
        knowledge_book_ids: Vec::new(),
        transform_set_ids: Vec::new(),
        module_ids: Vec::new(),
        cache_boundaries: Vec::new(),
        metadata: imported_preset_metadata(
            "Imported memory preset. Bind this summarization prompt to a configured LorePia memory task before activation."
                .to_owned(),
            vec!["compatibility".to_owned(), "memory-template".to_owned()],
            provenance,
        )?,
    };
    preset.validate().map_err(|error| {
        unsupported(format!(
            "imported memory preset cannot be normalized: {error}"
        ))
    })?;
    Ok(preset)
}

fn unsupported_memory_settings(settings: &serde_json::Map<String, Value>) -> Vec<String> {
    settings
        .keys()
        .filter(|key| {
            matches!(
                key.as_str(),
                "reSummarizationPrompt" | "processRegexScript" | "doNotSummarizeUserMessage"
            ) && settings.get(*key).is_some_and(setting_is_enabled)
        })
        .take(128)
        .cloned()
        .collect()
}

fn imported_memory_summary_template(source: &str) -> CoreResult<SafeTemplate> {
    if source.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(unsupported(
            "imported memory summarization prompt exceeds the safe template limit",
        ));
    }
    let occurrences = source.matches("{{slot}}").count();
    if occurrences != 1 {
        return Err(unsupported(
            "imported memory summarization prompt must contain exactly one {{slot}} marker",
        ));
    }
    let (before, after) = source
        .split_once("{{slot}}")
        .ok_or_else(|| unsupported("imported memory summarization slot is missing"))?;
    Ok(SafeTemplate {
        parts: vec![
            TemplatePart::Text {
                value: before.to_owned(),
            },
            TemplatePart::Slot {
                name: "memory_source".to_owned(),
            },
            TemplatePart::Text {
                value: after.to_owned(),
            },
        ],
        max_output_chars: lorepia_domain::MAX_TEMPLATE_OUTPUT_CHARS,
    })
}

fn bounded_setting_f32(settings: &serde_json::Map<String, Value>, key: &str, fallback: f32) -> f32 {
    settings
        .get(key)
        .and_then(Value::as_f64)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .and_then(|value| value.to_string().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(fallback)
}

fn preserve_memory_hints(settings: &serde_json::Map<String, Value>, values: &mut VariableMap) {
    for key in [
        "summarizationModel",
        "memoryTokensRatio",
        "extraSummarizationRatio",
        "maxChatsPerSummary",
        "recentMemoryRatio",
        "similarMemoryRatio",
        "enableSimilarityCorrection",
        "preserveOrphanedMemory",
        "processRegexScript",
        "doNotSummarizeUserMessage",
        "useExperimentalImpl",
        "summarizationRequestsPerMinute",
        "summarizationMaxConcurrent",
        "embeddingRequestsPerMinute",
        "embeddingMaxConcurrent",
        "alwaysToggleOn",
        "queryChatCount",
    ] {
        if let Some(value) = settings.get(key).and_then(import_hint_value) {
            insert_import_hint(
                values,
                &format!("lorepia_risu_memory_{}", camel_to_snake(key)),
                value,
            );
        }
    }
}

fn camel_to_snake(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_ascii_uppercase() {
            output.push('_');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

fn setting_is_enabled(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::String(value) => !value.trim().is_empty(),
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
        Value::Null => false,
    }
}

fn imported_provenance(source_id: &str, source_sha256: &str) -> Provenance {
    Provenance {
        source_kind: SourceKind::ImportedStandard,
        source_id: Some(source_id.to_owned()),
        source_hash: Some(source_sha256.to_owned()),
        author: None,
        license: Some("LicenseRef-Imported-User-Content".to_owned()),
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
    .map_err(|error| unsupported(format!("imported preset metadata is invalid: {error}")))
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

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
