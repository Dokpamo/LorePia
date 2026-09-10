use std::collections::BTreeMap;

use lorepia_domain::{
    CharacterRuntimeProfile, HistorySelector, PortableTextTransform, PortableTransformPhase,
    TemplatePart, VariableValue,
};
use serde_json::json;

use super::*;

#[test]
fn converted_module_enables_only_its_reviewed_declarative_components() {
    let source_sha256 = "a".repeat(64);
    let source = ImportedModuleSource {
        metadata: runtime::DecodedRuntimeMetadata {
            profile: CharacterRuntimeProfile {
                transforms: vec![PortableTextTransform {
                    id: "safe-transform".to_owned(),
                    name: "Safe transform".to_owned(),
                    phase: PortableTransformPhase::ProviderOutput,
                    enabled: true,
                    pattern: "a".to_owned(),
                    replacement: "aa".to_owned(),
                    flags: String::new(),
                    metadata: BTreeMap::new(),
                }],
                ..CharacterRuntimeProfile::default()
            },
            knowledge_entries: None,
            asset_metadata: Vec::new(),
            name: "Synthetic external module".to_owned(),
            description: String::new(),
            warnings: Vec::new(),
        },
        assets: Vec::new(),
    };

    let converted = convert_module(source, &source_sha256).expect("convert external module");
    let module = converted
        .documents
        .iter()
        .find_map(|entry| match &entry.document {
            NormalizedDocument::ContentModule(module) => Some(module),
            _ => None,
        })
        .expect("converted content module");

    assert!(module.imported_components_enabled);
    assert!(module.portable_runtime.is_none());
    assert!(
        !module
            .required_capabilities
            .contains(&lorepia_domain::ContentCapability::PortableRuntime)
    );
    assert_eq!(module.transform_set_ids.len(), 1);
    assert!(module.prompt_fragments.is_empty());
    assert!(module.interaction_rule_set_ids.is_empty());
    let set = converted
        .documents
        .iter()
        .find_map(|entry| match &entry.document {
            NormalizedDocument::TransformSet(set) => Some(set),
            _ => None,
        })
        .expect("native transform set");
    assert_eq!(set.rules[0].pattern.pattern, "a");
    assert_eq!(set.rules[0].replacement, "aa");
}

#[test]
fn converted_module_keeps_rules_that_only_the_portable_renderer_can_apply() {
    let profile = CharacterRuntimeProfile {
        transforms: [("native", "a"), ("portable", "(?<=a)b")]
            .into_iter()
            .map(|(id, pattern)| PortableTextTransform {
                id: id.to_owned(),
                name: id.to_owned(),
                phase: PortableTransformPhase::Display,
                enabled: true,
                pattern: pattern.to_owned(),
                replacement: "expanded".to_owned(),
                flags: String::new(),
                metadata: BTreeMap::new(),
            })
            .collect(),
        ..CharacterRuntimeProfile::default()
    };
    let converted = convert_module(
        ImportedModuleSource {
            metadata: runtime::DecodedRuntimeMetadata {
                profile,
                knowledge_entries: None,
                asset_metadata: Vec::new(),
                name: "Mixed rules".to_owned(),
                description: String::new(),
                warnings: Vec::new(),
            },
            assets: Vec::new(),
        },
        &"d".repeat(64),
    )
    .unwrap();
    let module = converted
        .documents
        .iter()
        .find_map(|entry| match &entry.document {
            NormalizedDocument::ContentModule(module) => Some(module),
            _ => None,
        })
        .unwrap();
    let transforms = &module.portable_runtime.as_ref().unwrap().transforms;
    assert_eq!(transforms.len(), 1);
    assert_eq!(transforms[0].id, "portable");
    assert_eq!(transforms[0].pattern, "(?<=a)b");
}

#[test]
fn converted_generation_preset_preserves_ranges_wrappers_controls_and_hints() {
    let source_sha256 = "b".repeat(64);
    let converted = convert_preset(
        &json!({
            "name": "Synthetic preset",
            "aiModel": "gemini-test",
            "apiType": "gemini",
            "temperature": 100,
            "maxResponse": 8192,
            "customPromptTemplateToggle": "status=상태창\nmode=표현=select=간결,상세\nnote=메모=text",
            "promptTemplate": [
                {"type":"description", "name":"Description", "innerFormat":"<description>{{slot}}</description>"},
                {"type":"chat", "name":"Older", "rangeStart":-12, "rangeEnd":-2},
                {"type":"chat", "name":"Latest", "rangeStart":-1, "rangeEnd":"end"}
            ],
            "regex": []
        }),
        &source_sha256,
    )
    .expect("convert generation preset");
    let preset = converted
        .documents
        .iter()
        .find_map(|entry| match &entry.document {
            NormalizedDocument::PromptPreset(preset) => Some(preset),
            _ => None,
        })
        .expect("prompt preset");

    assert_eq!(preset.controls.len(), 3);
    assert_eq!(preset.controls[1].options.len(), 2);
    assert!(preset.blocks.iter().any(|block| {
        matches!(
            block.history_selector,
            Some(HistorySelector::RelativeMessageRange {
                start: -12,
                end: Some(-2)
            })
        )
    }));
    assert!(preset.blocks.iter().any(|block| {
        block.template.as_ref().is_some_and(|template| {
            template
                .parts
                .iter()
                .any(|part| matches!(part, TemplatePart::Slot { name } if name == "block_content"))
        })
    }));
    assert!(preset.default_values.values.iter().any(|binding| {
        binding.variable.id.as_str() == "lorepia_risu_model_hint"
            && binding.value == VariableValue::Text("gemini-test".to_owned())
    }));
}

#[test]
fn converted_memory_preset_preserves_bounded_setup_hints_for_materialization() {
    let source_sha256 = "c".repeat(64);
    let converted = convert_memory_preset(
        &json!({
            "type": "risu",
            "ver": 1,
            "data": {
                "name": "Synthetic Hypa",
                "settings": {
                    "summarizationPrompt": "Extract relationships.\n{{slot}}\nFinish.",
                    "maxChatsPerSummary": 7,
                    "queryChatCount": 3,
                    "recentMemoryRatio": 0.6,
                    "similarMemoryRatio": 0.4,
                    "summarizationRequestsPerMinute": 20,
                    "summarizationMaxConcurrent": 2,
                    "preserveOrphanedMemory": true
                }
            }
        }),
        &source_sha256,
    )
    .expect("convert memory preset");
    let setup = converted
        .documents
        .iter()
        .find_map(|entry| match &entry.document {
            NormalizedDocument::PromptPreset(preset) => Some(preset),
            _ => None,
        })
        .expect("setup prompt");

    assert!(
        setup
            .default_values
            .values
            .iter()
            .any(|binding| { binding.variable.id.as_str() == "lorepia_risu_memory_profile_id" })
    );
    let hint = |id: &str| {
        setup
            .default_values
            .values
            .iter()
            .find(|binding| binding.variable.id.as_str() == id)
            .map(|binding| &binding.value)
    };
    assert_eq!(
        hint("lorepia_risu_memory_max_chats_per_summary"),
        Some(&VariableValue::Integer(7))
    );
    assert_eq!(
        hint("lorepia_risu_memory_query_chat_count"),
        Some(&VariableValue::Integer(3))
    );
    assert_eq!(converted.package_capabilities, ["prompt_presets"]);
}
