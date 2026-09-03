use std::collections::BTreeSet;

use lorepia_domain::CoreResult;
use serde_json::Value;

use super::unsupported;

const MAX_JSON_SCAN_NODES: usize = 100_000;

#[derive(Debug, Default)]
pub(super) struct HazardScan {
    pub(super) kinds: BTreeSet<HazardKind>,
    node_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum HazardKind {
    Code,
    Script,
    Html,
    ExternalUrl,
    UnsafeAction,
}

pub(super) fn inspect_json_component(
    bytes: &[u8],
    skip_typed_portable_runtime: bool,
) -> CoreResult<HazardScan> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| unsupported(format!("invalid component JSON: {error}")))?;
    if !value.is_object() && !value.is_array() {
        return Err(unsupported(
            "component JSON must contain an object or array",
        ));
    }
    let mut scan = HazardScan::default();
    scan_json_hazards(&value, None, skip_typed_portable_runtime, &mut scan)?;
    Ok(scan)
}

fn scan_json_hazards(
    value: &Value,
    key: Option<&str>,
    skip_typed_portable_runtime: bool,
    scan: &mut HazardScan,
) -> CoreResult<()> {
    scan.node_count = scan
        .node_count
        .checked_add(1)
        .ok_or_else(|| unsupported("component JSON node count overflow"))?;
    if scan.node_count > MAX_JSON_SCAN_NODES {
        return Err(unsupported(
            "component JSON exceeds the structural node limit",
        ));
    }
    match value {
        Value::Object(object) => {
            for (child_key, child) in object {
                let lower = child_key.to_ascii_lowercase();
                if skip_typed_portable_runtime && lower == "portable_runtime" {
                    // ContentModule is decoded with deny_unknown_fields and its
                    // CharacterRuntimeProfile is fully validated separately.
                    // Script source remains inert behind explicit package and
                    // renderer capability grants, so generic active-content
                    // heuristics must not quarantine that exact typed subtree.
                    continue;
                }
                if matches!(lower.as_str(), "script" | "scripts" | "javascript")
                    || lower.starts_with("script_")
                    || lower.ends_with("_script")
                    || lower.ends_with("_scripts")
                {
                    scan.kinds.insert(HazardKind::Script);
                }
                if lower == "html" || lower.ends_with("_html") {
                    scan.kinds.insert(HazardKind::Html);
                }
                if lower == "code" || lower.ends_with("_code") {
                    scan.kinds.insert(HazardKind::Code);
                }
                if matches!(
                    lower.as_str(),
                    "shell" | "command" | "exec" | "network_request" | "filesystem"
                ) {
                    scan.kinds.insert(HazardKind::UnsafeAction);
                }
                scan_json_hazards(child, Some(&lower), skip_typed_portable_runtime, scan)?;
            }
        }
        Value::Array(array) => {
            for child in array {
                scan_json_hazards(child, key, skip_typed_portable_runtime, scan)?;
            }
        }
        Value::String(text) => {
            let lower = text.to_ascii_lowercase();
            if lower.starts_with("http://")
                || lower.starts_with("https://")
                || lower.starts_with("//")
            {
                scan.kinds.insert(HazardKind::ExternalUrl);
            }
            if lower.contains("<script")
                || lower.contains("<iframe")
                || lower.contains("javascript:")
                || lower.contains("data:text/html")
            {
                scan.kinds.insert(HazardKind::Html);
            }
            if key.is_some_and(|key| {
                matches!(
                    key,
                    "action" | "action_type" | "kind" | "type" | "operation"
                )
            }) && matches!(
                lower.as_str(),
                "execute"
                    | "exec"
                    | "shell"
                    | "run_script"
                    | "network_request"
                    | "fetch"
                    | "open_url"
                    | "read_file"
                    | "write_file"
                    | "javascript"
                    | "html"
            ) {
                scan.kinds.insert(HazardKind::UnsafeAction);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_active_content_and_external_urls() {
        let value = serde_json::json!({
            "safe": "text",
            "script": "alert(1)",
            "asset_url": "https://invalid.example/image.png",
            "rules": [{"action": "network_request"}],
        });
        let mut scan = HazardScan::default();
        scan_json_hazards(&value, None, false, &mut scan).expect("scan");
        assert!(scan.kinds.contains(&HazardKind::Script));
        assert!(scan.kinds.contains(&HazardKind::ExternalUrl));
        assert!(scan.kinds.contains(&HazardKind::UnsafeAction));
    }

    #[test]
    fn does_not_treat_description_as_a_script_field() {
        let value = serde_json::json!({
            "description": "A normal inert content description.",
            "transcript": "A normal inert conversation transcript.",
        });
        let mut scan = HazardScan::default();
        scan_json_hazards(&value, None, false, &mut scan).expect("scan");
        assert!(!scan.kinds.contains(&HazardKind::Script));
    }

    #[test]
    fn skips_only_the_typed_portable_runtime_subtree() {
        let value = serde_json::json!({
            "portable_runtime": {
                "scripts": [{
                    "source": "const inert = '<script>'; const url = 'https://example.invalid';"
                }]
            },
            "description": "normal module metadata"
        });
        let mut scan = HazardScan::default();
        scan_json_hazards(&value, None, true, &mut scan).expect("scan typed runtime");
        assert!(scan.kinds.is_empty());

        let sibling_hazard = serde_json::json!({
            "portable_runtime": { "scripts": [] },
            "script": "outside the typed runtime"
        });
        let mut scan = HazardScan::default();
        scan_json_hazards(&sibling_hazard, None, true, &mut scan).expect("scan sibling field");
        assert!(scan.kinds.contains(&HazardKind::Script));
    }
}
