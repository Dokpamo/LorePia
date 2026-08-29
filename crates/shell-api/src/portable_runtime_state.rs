//! Typed, bounded portable-runtime state projection for native webviews.

use chrono::{DateTime, Utc};
use lorepia_core::{
    PortableRuntimeStatePayload, PortableRuntimeStateRecord, PortableRuntimeStateSaveResult,
    PortableRuntimeStateScope, PortableRuntimeStateWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ShellApi, ShellError, ShellResult, api::validate_identifier};

const PORTABLE_RUNTIME_STATE_SCHEMA_VERSION: u32 = 1;
const MAX_PORTABLE_RUNTIME_STATE_BYTES: usize = 4 * 1_024 * 1_024;
const MAX_PORTABLE_RUNTIME_STATE_DEPTH: usize = 32;
const MAX_PORTABLE_RUNTIME_STATE_NODES: usize = 100_000;
// Keep these field limits aligned with portable-runtime-state.ts. "Chars"
// means JavaScript UTF-16 code units, not Unicode scalar values.
const MAX_PORTABLE_RUNTIME_RECORD_KEYS: usize = 256;
const MAX_PORTABLE_RUNTIME_KEY_CHARS: usize = 512;
const MAX_PORTABLE_RUNTIME_OPTION_VALUE_CHARS: usize = 16_384;
const MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES: usize = 64 * 1_024;
const MAX_PORTABLE_RUNTIME_STATE_VALUE_NODES: usize = 2_048;
const MAX_PORTABLE_RUNTIME_MESSAGE_OVERRIDE_CHARS: usize = 262_144;
const MAX_PORTABLE_RUNTIME_BACKGROUND_CHARS: usize = 1_024 * 1_024;
const PORTABLE_RUNTIME_STATE_FIELDS: [&str; 6] = [
    "options",
    "chatVars",
    "state",
    "messageOverrides",
    "background",
    "auxiliarySelection",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRuntimeStateScopeDto {
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub conversation_id: String,
    pub branch_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRuntimeStatePayloadDto {
    pub schema_version: u32,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRuntimeStateRecordDto {
    pub scope: PortableRuntimeStateScopeDto,
    pub scope_epoch: u64,
    pub revision: u64,
    pub payload: PortableRuntimeStatePayloadDto,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortableRuntimeStateSnapshotDto {
    pub scope_epoch: u64,
    pub record: Option<PortableRuntimeStateRecordDto>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetPortableRuntimeStateInput {
    pub scope: PortableRuntimeStateScopeDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutPortableRuntimeStateInput {
    pub scope: PortableRuntimeStateScopeDto,
    pub expected_scope_epoch: u64,
    pub expected_revision: Option<u64>,
    pub payload: PortableRuntimeStatePayloadDto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum PortableRuntimeStateSaveResultDto {
    Saved {
        record: PortableRuntimeStateRecordDto,
        evicted_rows: u32,
        evicted_bytes: u64,
    },
    RevisionConflict {
        current: Option<PortableRuntimeStateRecordDto>,
    },
    ScopeInvalidated {
        current_scope_epoch: u64,
    },
}

impl ShellApi {
    pub fn get_portable_runtime_state(
        &self,
        input: GetPortableRuntimeStateInput,
    ) -> ShellResult<PortableRuntimeStateSnapshotDto> {
        validate_scope(&input.scope)?;
        self.core
            .get_portable_runtime_state(&input.scope.into())
            .map(Into::into)
            .map_err(Into::into)
    }

    pub fn put_portable_runtime_state(
        &self,
        input: PutPortableRuntimeStateInput,
    ) -> ShellResult<PortableRuntimeStateSaveResultDto> {
        validate_scope(&input.scope)?;
        validate_payload(&input.payload)?;
        self.core
            .put_portable_runtime_state(PortableRuntimeStateWrite {
                scope: input.scope.into(),
                expected_scope_epoch: input.expected_scope_epoch,
                expected_revision: input.expected_revision,
                payload: input.payload.into(),
            })
            .map(Into::into)
            .map_err(Into::into)
    }
}

fn validate_scope(scope: &PortableRuntimeStateScopeDto) -> ShellResult<()> {
    validate_identifier("character_id", &scope.character_id)?;
    if let Some(revision_id) = &scope.character_content_revision_id {
        validate_identifier("character_content_revision_id", revision_id)?;
    }
    validate_identifier("conversation_id", &scope.conversation_id)?;
    validate_identifier("branch_id", &scope.branch_id)
}

fn validate_payload(payload: &PortableRuntimeStatePayloadDto) -> ShellResult<()> {
    if payload.schema_version != PORTABLE_RUNTIME_STATE_SCHEMA_VERSION {
        return invalid_payload(format!(
            "portable runtime payload schema version must be {PORTABLE_RUNTIME_STATE_SCHEMA_VERSION}",
        ));
    }
    let object = payload
        .value
        .as_object()
        .ok_or_else(|| invalid_payload_error("portable runtime payload must be a JSON object"))?;
    validate_exact_object_fields(
        "portable runtime payload",
        object,
        &PORTABLE_RUNTIME_STATE_FIELDS,
    )?;
    let mut pending = vec![(&payload.value, 0_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_PORTABLE_RUNTIME_STATE_NODES || depth > MAX_PORTABLE_RUNTIME_STATE_DEPTH {
            return invalid_payload("portable runtime payload exceeds JSON depth or node limits");
        }
        match value {
            Value::Object(object) => pending.extend(
                object
                    .values()
                    .map(|child| (child, depth.saturating_add(1))),
            ),
            Value::Array(array) => {
                pending.extend(array.iter().map(|child| (child, depth.saturating_add(1))));
            }
            _ => {}
        }
    }
    validate_string_record(
        "portable runtime options",
        required_field(object, "options")?,
        MAX_PORTABLE_RUNTIME_OPTION_VALUE_CHARS,
    )?;
    validate_state_record(
        "portable runtime chatVars",
        required_field(object, "chatVars")?,
    )?;
    validate_state_record("portable runtime state", required_field(object, "state")?)?;
    validate_string_record(
        "portable runtime messageOverrides",
        required_field(object, "messageOverrides")?,
        MAX_PORTABLE_RUNTIME_MESSAGE_OVERRIDE_CHARS,
    )?;
    validate_bounded_string(
        "portable runtime background",
        required_field(object, "background")?,
        MAX_PORTABLE_RUNTIME_BACKGROUND_CHARS,
    )?;
    validate_auxiliary_selection(required_field(object, "auxiliarySelection")?)?;

    let json = serde_json::to_string(&payload.value).map_err(|error| {
        invalid_payload_error(format!("portable runtime payload is invalid: {error}"))
    })?;
    if json.len() > MAX_PORTABLE_RUNTIME_STATE_BYTES {
        return invalid_payload(format!(
            "portable runtime payload exceeds its {MAX_PORTABLE_RUNTIME_STATE_BYTES}-byte limit",
        ));
    }
    Ok(())
}

fn required_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
) -> ShellResult<&'a Value> {
    object.get(field).ok_or_else(|| {
        invalid_payload_error(format!("portable runtime payload is missing {field}"))
    })
}

fn validate_exact_object_fields(
    label: &str,
    object: &serde_json::Map<String, Value>,
    fields: &[&str],
) -> ShellResult<()> {
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return invalid_payload(format!("{label} does not have the exact supported fields"));
    }
    Ok(())
}

fn validate_string_record(label: &str, value: &Value, maximum_chars: usize) -> ShellResult<()> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_payload_error(format!("{label} must be a JSON object")))?;
    validate_record_keys(label, object)?;
    for item in object.values() {
        validate_bounded_string(label, item, maximum_chars)?;
    }
    Ok(())
}

fn validate_state_record(label: &str, value: &Value) -> ShellResult<()> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid_payload_error(format!("{label} must be a JSON object")))?;
    validate_record_keys(label, object)?;
    for item in object.values() {
        validate_json_value_budget(label, item)?;
    }
    Ok(())
}

fn validate_record_keys(label: &str, object: &serde_json::Map<String, Value>) -> ShellResult<()> {
    if object.len() > MAX_PORTABLE_RUNTIME_RECORD_KEYS {
        return invalid_payload(format!(
            "{label} exceeds its {MAX_PORTABLE_RUNTIME_RECORD_KEYS}-key limit",
        ));
    }
    for key in object.keys() {
        if key.is_empty()
            || javascript_string_length(key) > MAX_PORTABLE_RUNTIME_KEY_CHARS
            || matches!(key.as_str(), "__proto__" | "constructor" | "prototype")
        {
            return invalid_payload(format!("{label} contains an invalid portable runtime key"));
        }
    }
    Ok(())
}

fn validate_bounded_string(label: &str, value: &Value, maximum_chars: usize) -> ShellResult<()> {
    let value = value
        .as_str()
        .ok_or_else(|| invalid_payload_error(format!("{label} must contain only strings")))?;
    if javascript_string_length(value) > maximum_chars {
        return invalid_payload(format!(
            "{label} exceeds its {maximum_chars}-character limit",
        ));
    }
    Ok(())
}

fn validate_json_value_budget(label: &str, value: &Value) -> ShellResult<()> {
    let json = serde_json::to_string(value)
        .map_err(|error| invalid_payload_error(format!("{label} is invalid: {error}")))?;
    if json.len() > MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES {
        return invalid_payload(format!(
            "{label} value exceeds its {MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES}-byte limit",
        ));
    }
    let mut pending = vec![value];
    let mut nodes = 0_usize;
    while let Some(value) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_PORTABLE_RUNTIME_STATE_VALUE_NODES {
            return invalid_payload(format!(
                "{label} value exceeds its {MAX_PORTABLE_RUNTIME_STATE_VALUE_NODES}-node limit",
            ));
        }
        match value {
            Value::Object(object) => pending.extend(object.values()),
            Value::Array(array) => pending.extend(array),
            _ => {}
        }
    }
    Ok(())
}

fn validate_auxiliary_selection(value: &Value) -> ShellResult<()> {
    if value.is_null() {
        return Ok(());
    }
    let selection = value.as_object().ok_or_else(|| {
        invalid_payload_error("portable runtime auxiliarySelection must be null or an object")
    })?;
    let kind = selection
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            invalid_payload_error("portable runtime auxiliarySelection kind is invalid")
        })?;
    match kind {
        "legacy_profile" => {
            validate_exact_object_fields(
                "portable runtime legacy auxiliarySelection",
                selection,
                &["kind", "provider_profile_id"],
            )?;
            require_string_field(selection, "provider_profile_id")?;
        }
        "target" => {
            validate_exact_object_fields(
                "portable runtime target auxiliarySelection",
                selection,
                &["kind", "target"],
            )?;
            let target = selection
                .get("target")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    invalid_payload_error("portable runtime auxiliarySelection target is invalid")
                })?;
            validate_exact_object_fields(
                "portable runtime auxiliarySelection target",
                target,
                &["model_route_id", "generation_preset_id"],
            )?;
            require_string_field(target, "model_route_id")?;
            require_string_field(target, "generation_preset_id")?;
        }
        _ => {
            return invalid_payload("portable runtime auxiliarySelection kind is unsupported");
        }
    }
    Ok(())
}

fn require_string_field(object: &serde_json::Map<String, Value>, field: &str) -> ShellResult<()> {
    if object.get(field).is_none_or(|value| !value.is_string()) {
        return invalid_payload(format!(
            "portable runtime auxiliarySelection {field} must be a string",
        ));
    }
    Ok(())
}

fn javascript_string_length(value: &str) -> usize {
    value.encode_utf16().count()
}

fn invalid_payload(message: impl Into<String>) -> ShellResult<()> {
    Err(invalid_payload_error(message))
}

fn invalid_payload_error(message: impl Into<String>) -> ShellError {
    ShellError::from(lorepia_core::CoreError::invalid(message))
}

impl From<PortableRuntimeStateScopeDto> for PortableRuntimeStateScope {
    fn from(value: PortableRuntimeStateScopeDto) -> Self {
        Self {
            character_id: value.character_id,
            character_content_revision_id: value.character_content_revision_id,
            conversation_id: value.conversation_id,
            branch_id: value.branch_id,
        }
    }
}

impl From<PortableRuntimeStateScope> for PortableRuntimeStateScopeDto {
    fn from(value: PortableRuntimeStateScope) -> Self {
        Self {
            character_id: value.character_id,
            character_content_revision_id: value.character_content_revision_id,
            conversation_id: value.conversation_id,
            branch_id: value.branch_id,
        }
    }
}

impl From<PortableRuntimeStatePayloadDto> for PortableRuntimeStatePayload {
    fn from(value: PortableRuntimeStatePayloadDto) -> Self {
        Self {
            schema_version: value.schema_version,
            value: value.value,
        }
    }
}

impl From<PortableRuntimeStatePayload> for PortableRuntimeStatePayloadDto {
    fn from(value: PortableRuntimeStatePayload) -> Self {
        Self {
            schema_version: value.schema_version,
            value: value.value,
        }
    }
}

impl From<PortableRuntimeStateRecord> for PortableRuntimeStateRecordDto {
    fn from(value: PortableRuntimeStateRecord) -> Self {
        Self {
            scope: value.scope.into(),
            scope_epoch: value.scope_epoch,
            revision: value.revision,
            payload: value.payload.into(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

impl From<lorepia_core::PortableRuntimeStateSnapshot> for PortableRuntimeStateSnapshotDto {
    fn from(value: lorepia_core::PortableRuntimeStateSnapshot) -> Self {
        Self {
            scope_epoch: value.scope_epoch,
            record: value.record.map(Into::into),
        }
    }
}

impl From<PortableRuntimeStateSaveResult> for PortableRuntimeStateSaveResultDto {
    fn from(value: PortableRuntimeStateSaveResult) -> Self {
        match value {
            PortableRuntimeStateSaveResult::Saved {
                record,
                evicted_rows,
                evicted_bytes,
            } => Self::Saved {
                record: record.into(),
                evicted_rows,
                evicted_bytes,
            },
            PortableRuntimeStateSaveResult::RevisionConflict { current } => {
                Self::RevisionConflict {
                    current: current.map(Into::into),
                }
            }
            PortableRuntimeStateSaveResult::ScopeInvalidated {
                current_scope_epoch,
            } => Self::ScopeInvalidated {
                current_scope_epoch,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ShellErrorCode;

    #[test]
    fn save_outcomes_use_stable_tagged_wire_shapes() {
        let invalidated = PortableRuntimeStateSaveResultDto::ScopeInvalidated {
            current_scope_epoch: 4,
        };
        assert_eq!(
            serde_json::to_value(invalidated).expect("serialize invalidated result"),
            json!({"status": "scope_invalidated", "current_scope_epoch": 4})
        );
        let conflict = PortableRuntimeStateSaveResultDto::RevisionConflict { current: None };
        assert_eq!(
            serde_json::to_value(conflict).expect("serialize conflict result"),
            json!({"status": "revision_conflict", "current": null})
        );
    }

    #[test]
    fn state_inputs_reject_unknown_fields() {
        let error = serde_json::from_value::<GetPortableRuntimeStateInput>(json!({
            "scope": {
                "character_id": "character",
                "character_content_revision_id": null,
                "conversation_id": "conversation",
                "branch_id": "branch",
                "unexpected": true
            }
        }))
        .expect_err("unknown scope field must fail");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn payload_validation_rejects_future_and_malformed_state_before_core() {
        let valid = PortableRuntimeStatePayloadDto {
            schema_version: PORTABLE_RUNTIME_STATE_SCHEMA_VERSION,
            value: valid_payload_value(),
        };
        validate_payload(&valid).expect("exact schema-one payload must pass");

        let mut future = valid.clone();
        future.schema_version += 1;
        assert_eq!(
            validate_payload(&future)
                .expect_err("future payload must fail closed")
                .code,
            ShellErrorCode::InvalidInput
        );

        let mut extra_field = valid.clone();
        extra_field
            .value
            .as_object_mut()
            .expect("test payload object")
            .insert("future".to_owned(), Value::Bool(true));
        assert_eq!(
            validate_payload(&extra_field)
                .expect_err("shape drift must fail closed")
                .code,
            ShellErrorCode::InvalidInput
        );

        let mut invalid_key = valid.clone();
        invalid_key.value["state"] = json!({"constructor": true});
        assert_eq!(
            validate_payload(&invalid_key)
                .expect_err("prototype-bearing state key must fail closed")
                .code,
            ShellErrorCode::InvalidInput
        );

        let mut long_background = valid;
        long_background.value["background"] =
            Value::String("x".repeat(MAX_PORTABLE_RUNTIME_BACKGROUND_CHARS + 1));
        assert_eq!(
            validate_payload(&long_background)
                .expect_err("oversized background must fail closed")
                .code,
            ShellErrorCode::InvalidInput
        );
    }

    fn valid_payload_value() -> Value {
        json!({
            "options": {},
            "chatVars": {},
            "state": {},
            "messageOverrides": {},
            "background": "",
            "auxiliarySelection": null
        })
    }
}
