use std::collections::{BTreeMap, BTreeSet};

use lorepia_domain::{CoreResult, ImportWarning, PortableRuntimeCapability, PortableRuntimeScript};
use serde_json::Value;

use super::canonical_json;

pub(super) fn sandbox_legacy_low_level_scripts(
    scripts: &mut Vec<PortableRuntimeScript>,
    required_capabilities: &mut Option<Vec<PortableRuntimeCapability>>,
) -> Vec<ImportWarning> {
    let mut sandboxed_count = 0;
    let mut quarantined_count = 0;
    scripts.retain_mut(|script| {
        if !script.elevated_access {
            return true;
        }
        if !script.language.trim().eq_ignore_ascii_case("lua") {
            quarantined_count += 1;
            return false;
        }
        script.elevated_access = false;
        script
            .metadata
            .insert("legacy_low_level_sandboxed".to_owned(), "true".to_owned());
        sandboxed_count += 1;
        true
    });
    let mut warnings = Vec::new();
    if sandboxed_count > 0 {
        let inferred = infer_legacy_lua_capabilities(scripts);
        let required = required_capabilities.get_or_insert_default();
        for capability in inferred {
            if !required.contains(&capability) {
                required.push(capability);
            }
        }
        required.sort_unstable();
        warnings.push(ImportWarning {
            code: "legacy_low_level_scripts_sandboxed".to_owned(),
            message: format!(
                "Preserved {sandboxed_count} legacy low-level Lua runtime script(s) inside LorePia's isolated worker and inferred explicit reviewable capabilities; unrestricted host access remains disabled."
            ),
        });
    }
    if quarantined_count > 0 {
        warnings.push(ImportWarning {
            code: "legacy_non_lua_scripts_quarantined".to_owned(),
            message: format!(
                "Quarantined {quarantined_count} legacy low-level non-Lua runtime script(s); only bounded Lua worker execution is supported."
            ),
        });
    }
    warnings
}

pub(super) fn runtime_profile_metadata(
    module: &serde_json::Map<String, Value>,
) -> CoreResult<BTreeMap<String, String>> {
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
    Ok(metadata)
}

pub(super) fn runtime_toggle_defaults(schema: &str) -> BTreeMap<String, String> {
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
            let kind = fields
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
                return None;
            }
            let default = match kind.as_str() {
                "" | "select" | "toggle" | "checkbox" => "0",
                "text" | "textarea" => "",
                _ => return None,
            };
            Some((name.to_owned(), default.to_owned()))
        })
        .collect()
}

fn infer_legacy_lua_capabilities(
    scripts: &[PortableRuntimeScript],
) -> Vec<PortableRuntimeCapability> {
    use PortableRuntimeCapability as Capability;

    let mut identifiers = BTreeSet::new();
    for source in scripts
        .iter()
        .filter(|script| script.language.trim().eq_ignore_ascii_case("lua"))
        .map(|script| script.source.as_str())
    {
        for identifier in
            source.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        {
            if !identifier.is_empty() {
                identifiers.insert(identifier.to_owned());
            }
        }
    }
    let has_any = |names: &[&str]| names.iter().any(|name| identifiers.contains(*name));
    let mut capabilities = vec![Capability::RuntimeCallbacks];
    if has_any(&["getChatLength", "getChat", "getFullChat", "editRequest"]) {
        capabilities.push(Capability::ChatRead);
    }
    if has_any(&[
        "setChat",
        "removeChat",
        "reloadChat",
        "stopChat",
        "addChat",
        "editRequest",
    ]) {
        capabilities.push(Capability::ChatWrite);
    }
    if has_any(&["getChatVar", "setChatVar", "getState", "setState"]) {
        capabilities.push(Capability::StateReadWrite);
    }
    if has_any(&[
        "getGlobalVar",
        "getPersonaName",
        "getPersonaDescription",
        "getDescription",
        "cbs",
    ]) {
        capabilities.push(Capability::ProfileRead);
    }
    if has_any(&["getLoreBooks", "loadLoreBooks"]) {
        capabilities.push(Capability::LoreRead);
    }
    if has_any(&[
        "getBackgroundEmbedding",
        "setBackgroundEmbedding",
        "alertNormal",
        "alertError",
        "editDisplay",
        "cbs",
    ]) {
        capabilities.push(Capability::UiWrite);
    }
    if identifiers.contains("LLM") {
        capabilities.push(Capability::ModelPrimary);
    }
    if identifiers.contains("axLLM") {
        capabilities.push(Capability::ModelAuxiliary);
    }
    capabilities.sort_unstable();
    capabilities.dedup();
    capabilities
}
