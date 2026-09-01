use std::collections::BTreeMap;

use lorepia_domain::CoreResult;
use serde_json::Value;

use super::canonical_json;

pub(super) fn parse_runtime_variables(
    value: Option<&Value>,
) -> CoreResult<BTreeMap<String, String>> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    match value {
        Value::Null => Ok(BTreeMap::new()),
        Value::String(value) if value.is_empty() => Ok(BTreeMap::new()),
        Value::String(value) => match parse_assignments(value) {
            Some(variables) => Ok(variables),
            None => source_variable(&Value::String(value.clone())),
        },
        Value::Object(values) => values
            .iter()
            .map(|(key, value)| Ok((key.clone(), variable_text(value)?)))
            .collect(),
        _ => source_variable(value),
    }
}

fn parse_assignments(value: &str) -> Option<BTreeMap<String, String>> {
    let mut variables = BTreeMap::new();
    for line in value.lines().filter(|line| !line.trim().is_empty()) {
        let (name, value) = line.split_once('=')?;
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        variables.insert(name.to_owned(), value.trim().to_owned());
    }
    (!variables.is_empty()).then_some(variables)
}

fn variable_text(value: &Value) -> CoreResult<String> {
    match value {
        Value::Null => Ok(String::new()),
        Value::Bool(value) => Ok(u8::from(*value).to_string()),
        Value::Number(value) => Ok(value.to_string()),
        Value::String(value) => Ok(value.clone()),
        Value::Array(_) | Value::Object(_) => canonical_json(value),
    }
}

fn source_variable(value: &Value) -> CoreResult<BTreeMap<String, String>> {
    Ok(BTreeMap::from([(
        "source".to_owned(),
        canonical_json(value)?,
    )]))
}
