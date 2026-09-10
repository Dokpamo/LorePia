use std::collections::HashSet;

use lorepia_domain::{
    ControlId, ControlKind, ControlOption, ControlSpec, VariableId, VariableMap, VariableRef,
    VariableScope, VariableType, VariableValue,
};
use serde_json::Value;

use super::MAX_NAME_CHARS;

pub(super) fn prompt_controls(value: Option<&Value>) -> (Vec<ControlSpec>, VariableMap, usize) {
    let Some(schema) = value.and_then(Value::as_str) else {
        return (Vec::new(), VariableMap::default(), 0);
    };
    let mut controls = Vec::new();
    let mut defaults = VariableMap::default();
    let mut seen = HashSet::new();
    let mut skipped = 0_usize;
    let line_count = schema.lines().count();
    for (line_index, source_line) in schema.lines().enumerate() {
        let line = source_line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((control, variable, default_value)) = parse_prompt_control(line, &mut seen) else {
            skipped += 1;
            continue;
        };
        defaults.insert(variable.clone(), default_value.clone());
        controls.push(control);
        if controls.len() >= lorepia_domain::MAX_PROMPT_CONTROLS {
            skipped += line_count.saturating_sub(line_index + 1);
            break;
        }
    }
    (controls, defaults, skipped)
}

fn parse_prompt_control(
    line: &str,
    seen: &mut HashSet<String>,
) -> Option<(ControlSpec, VariableRef, VariableValue)> {
    let mut fields = line.splitn(4, '=');
    let key = fields.next().unwrap_or_default().trim();
    let label = fields.next().unwrap_or_default().trim();
    let raw_kind = fields
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let raw_choices = fields.next().unwrap_or_default();
    if key.is_empty()
        || label.is_empty()
        || key.chars().count() > 256
        || key.chars().any(char::is_control)
        || !seen.insert(key.to_owned())
    {
        return None;
    }
    let variable = VariableRef {
        scope: VariableScope::Conversation,
        namespace: None,
        id: VariableId::from(key),
    };
    let (kind, value_type, default_value, options) = prompt_control_kind(&raw_kind, raw_choices)?;
    Some((
        ControlSpec {
            id: ControlId::from(key),
            label: label.chars().take(MAX_NAME_CHARS).collect(),
            description: "가져온 프롬프트 호환 컨트롤".to_owned(),
            kind,
            value_type: Some(value_type),
            variable: Some(variable.clone()),
            default_value: Some(default_value.clone()),
            options,
            minimum: None,
            maximum: None,
            step: None,
            visible_when: None,
            scope: VariableScope::Conversation,
            sensitive: false,
            requires_regeneration: true,
        },
        variable,
        default_value,
    ))
}

fn prompt_control_kind(
    raw_kind: &str,
    raw_choices: &str,
) -> Option<(ControlKind, VariableType, VariableValue, Vec<ControlOption>)> {
    match raw_kind {
        "" | "toggle" | "checkbox" => Some((
            ControlKind::Toggle,
            VariableType::Bool,
            VariableValue::Bool(false),
            Vec::new(),
        )),
        "select" => {
            let options = raw_choices
                .split(',')
                .map(str::trim)
                .filter(|choice| !choice.is_empty())
                .take(128)
                .enumerate()
                .map(|(index, label)| ControlOption {
                    value: VariableValue::Enum(index.to_string()),
                    label: label.chars().take(MAX_NAME_CHARS).collect(),
                })
                .collect::<Vec<_>>();
            (!options.is_empty()).then_some((
                ControlKind::Select,
                VariableType::Enum,
                VariableValue::Enum("0".to_owned()),
                options,
            ))
        }
        "text" | "textarea" => Some((
            ControlKind::Text,
            VariableType::Text,
            VariableValue::Text(String::new()),
            Vec::new(),
        )),
        _ => None,
    }
}

pub(super) fn preserve_generation_hints(
    object: &serde_json::Map<String, Value>,
    values: &mut VariableMap,
) {
    insert_import_hint(
        values,
        "lorepia_risu_import_kind",
        VariableValue::Enum("generation".to_owned()),
    );
    for (source, target) in [
        ("apiType", "api_type"),
        ("aiModel", "model_hint"),
        ("subModel", "sub_model_hint"),
        ("maxContext", "max_context"),
        ("maxResponse", "max_response"),
        ("temperature", "temperature_raw"),
        ("top_p", "top_p_raw"),
        ("top_k", "top_k_raw"),
        ("top_a", "top_a_raw"),
        ("min_p", "min_p_raw"),
        ("frequencyPenalty", "frequency_penalty_raw"),
        ("PresensePenalty", "presence_penalty_raw"),
        ("repetition_penalty", "repetition_penalty_raw"),
        ("thinkingType", "thinking_type"),
        ("thinkingTokens", "thinking_tokens"),
        ("adaptiveThinkingEffort", "adaptive_thinking_effort"),
        ("deepseekThinkingType", "deepseek_thinking_type"),
        ("deepseekReasoningEffort", "deepseek_reasoning_effort"),
        ("reasonEffort", "reason_effort"),
        ("verbosity", "verbosity"),
        ("instructChatTemplate", "instruct_chat_template"),
        ("systemRoleReplacement", "system_role_replacement"),
        ("systemContentReplacement", "system_content_replacement"),
        ("currentPluginProvider", "plugin_provider"),
    ] {
        if let Some(value) = object.get(source).and_then(import_hint_value) {
            insert_import_hint(values, &format!("lorepia_risu_{target}"), value);
        }
    }
}

pub(super) fn import_hint_value(value: &Value) -> Option<VariableValue> {
    match value {
        Value::Bool(value) => Some(VariableValue::Bool(*value)),
        Value::Number(value) => value.as_i64().map(VariableValue::Integer).or_else(|| {
            value
                .as_f64()
                .filter(|value| value.is_finite())
                .map(VariableValue::Decimal)
        }),
        Value::String(value) if value.chars().count() <= 65_536 => {
            Some(VariableValue::Text(value.clone()))
        }
        _ => None,
    }
}

pub(super) fn insert_import_hint(values: &mut VariableMap, id: &str, value: VariableValue) {
    values.insert(
        VariableRef {
            scope: VariableScope::App,
            namespace: None,
            id: VariableId::from(id),
        },
        value,
    );
}
