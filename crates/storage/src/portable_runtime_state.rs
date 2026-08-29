use chrono::{DateTime, Utc};
use lorepia_domain::{CoreError, CoreErrorCode, CoreResult};
use rusqlite::{OptionalExtension, Transaction, TransactionBehavior, params};
use serde_json::Value;

use crate::{Storage, database::storage_db_error};

pub const MAX_PORTABLE_RUNTIME_STATE_BYTES: usize = 4 * 1_024 * 1_024;
pub const MAX_PORTABLE_RUNTIME_STATE_TOTAL_BYTES: u64 = 64 * 1_024 * 1_024;
pub const MAX_PORTABLE_RUNTIME_STATE_ROWS: u64 = 1_024;
const PORTABLE_RUNTIME_STATE_SCHEMA_VERSION: u32 = 1;

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
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
const PORTABLE_RUNTIME_STATE_FIELDS: [&str; 6] = [
    "options",
    "chatVars",
    "state",
    "messageOverrides",
    "background",
    "auxiliarySelection",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableRuntimeStateScope {
    pub character_id: String,
    pub character_content_revision_id: Option<String>,
    pub conversation_id: String,
    pub branch_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortableRuntimeStatePayload {
    pub schema_version: u32,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortableRuntimeStateRecord {
    pub scope: PortableRuntimeStateScope,
    pub scope_epoch: u64,
    pub revision: u64,
    pub payload: PortableRuntimeStatePayload,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortableRuntimeStateSnapshot {
    pub scope_epoch: u64,
    pub record: Option<PortableRuntimeStateRecord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PortableRuntimeStateWrite {
    pub scope: PortableRuntimeStateScope,
    pub expected_scope_epoch: u64,
    pub expected_revision: Option<u64>,
    pub payload: PortableRuntimeStatePayload,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PortableRuntimeStateSaveResult {
    Saved {
        record: PortableRuntimeStateRecord,
        evicted_rows: u32,
        evicted_bytes: u64,
    },
    RevisionConflict {
        current: Option<PortableRuntimeStateRecord>,
    },
    ScopeInvalidated {
        current_scope_epoch: u64,
    },
}

#[derive(Debug)]
struct RawPortableRuntimeStateRecord {
    scope_epoch: i64,
    revision: i64,
    payload_schema_version: i64,
    payload_json: String,
    payload_bytes: i64,
    created_at: String,
    updated_at: String,
}

#[derive(Debug)]
struct EvictionCandidate {
    scope: PortableRuntimeStateScope,
    payload_bytes: u64,
}

impl Storage {
    pub fn get_portable_runtime_state(
        &self,
        scope: &PortableRuntimeStateScope,
    ) -> CoreResult<PortableRuntimeStateSnapshot> {
        validate_scope(scope)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        validate_scope_relationships(&transaction, scope)?;
        let scope_epoch = load_scope_epoch(&transaction, scope)?;
        let record = load_record(&transaction, scope, scope_epoch)?;
        if record.is_some() {
            let access_sequence = allocate_access_sequence(&transaction)?;
            let changed = transaction
                .execute(
                    "UPDATE portable_runtime_states
                     SET access_sequence = ?5
                     WHERE character_id = ?1
                       AND character_content_revision_id IS ?2
                       AND conversation_id = ?3
                       AND branch_id = ?4
                       AND branch_epoch = ?6",
                    params![
                        scope.character_id,
                        scope.character_content_revision_id,
                        scope.conversation_id,
                        scope.branch_id,
                        to_i64("portable runtime access sequence", access_sequence)?,
                        to_i64("portable runtime scope epoch", scope_epoch)?,
                    ],
                )
                .map_err(storage_db_error)?;
            if changed != 1 {
                return Err(storage_corrupted(
                    "portable runtime state disappeared while updating its access sequence",
                ));
            }
        }
        transaction.commit().map_err(storage_db_error)?;
        Ok(PortableRuntimeStateSnapshot {
            scope_epoch,
            record,
        })
    }

    pub fn put_portable_runtime_state(
        &self,
        write: PortableRuntimeStateWrite,
    ) -> CoreResult<PortableRuntimeStateSaveResult> {
        self.put_portable_runtime_state_with_limits(
            write,
            MAX_PORTABLE_RUNTIME_STATE_ROWS,
            MAX_PORTABLE_RUNTIME_STATE_TOTAL_BYTES,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn put_portable_runtime_state_with_limits(
        &self,
        write: PortableRuntimeStateWrite,
        maximum_rows: u64,
        maximum_bytes: u64,
    ) -> CoreResult<PortableRuntimeStateSaveResult> {
        validate_scope(&write.scope)?;
        validate_safe_integer(
            "portable runtime expected scope epoch",
            write.expected_scope_epoch,
        )?;
        if let Some(expected_revision) = write.expected_revision {
            validate_positive_safe_integer(
                "portable runtime expected revision",
                expected_revision,
            )?;
        }
        let payload_json = encode_payload(&write.payload)?;
        let payload_bytes = u64::try_from(payload_json.len())
            .map_err(|_| CoreError::invalid("portable runtime payload size overflowed"))?;

        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        validate_scope_relationships(&transaction, &write.scope)?;
        let scope_epoch = load_scope_epoch(&transaction, &write.scope)?;
        if scope_epoch != write.expected_scope_epoch {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(PortableRuntimeStateSaveResult::ScopeInvalidated {
                current_scope_epoch: scope_epoch,
            });
        }

        let current = load_record(&transaction, &write.scope, scope_epoch)?;
        let current_revision = current.as_ref().map(|record| record.revision);
        if current_revision != write.expected_revision {
            transaction.commit().map_err(storage_db_error)?;
            return Ok(PortableRuntimeStateSaveResult::RevisionConflict { current });
        }

        let now = Utc::now();
        let now_text = now.to_rfc3339();
        let access_sequence = allocate_access_sequence(&transaction)?;
        let revision = match current_revision {
            Some(revision) => revision
                .checked_add(1)
                .ok_or_else(|| CoreError::invalid("portable runtime state revision overflowed"))?,
            None => 1,
        };
        validate_positive_safe_integer("portable runtime state revision", revision)?;
        match &current {
            Some(current) => {
                let changed = transaction
                    .execute(
                        "UPDATE portable_runtime_states
                         SET revision = ?5,
                             payload_schema_version = ?6,
                             payload_json = ?7,
                             payload_bytes = ?8,
                             access_sequence = ?9,
                             updated_at = ?10
                         WHERE character_id = ?1
                           AND character_content_revision_id IS ?2
                           AND conversation_id = ?3
                           AND branch_id = ?4
                           AND branch_epoch = ?11
                           AND revision = ?12",
                        params![
                            write.scope.character_id,
                            write.scope.character_content_revision_id,
                            write.scope.conversation_id,
                            write.scope.branch_id,
                            to_i64("portable runtime state revision", revision)?,
                            i64::from(write.payload.schema_version),
                            payload_json,
                            to_i64("portable runtime payload bytes", payload_bytes)?,
                            to_i64("portable runtime access sequence", access_sequence)?,
                            now_text,
                            to_i64("portable runtime scope epoch", scope_epoch)?,
                            to_i64("portable runtime expected revision", current.revision)?,
                        ],
                    )
                    .map_err(storage_db_error)?;
                if changed != 1 {
                    return Err(storage_corrupted(
                        "portable runtime state CAS changed during an immediate transaction",
                    ));
                }
            }
            None => {
                transaction
                    .execute(
                        "INSERT INTO portable_runtime_states (
                             character_id, character_content_revision_id,
                             character_content_revision_key, conversation_id,
                             branch_id, branch_epoch, revision,
                             payload_schema_version, payload_json, payload_bytes,
                             access_sequence, created_at, updated_at
                         ) VALUES (?1, ?2, coalesce(?2, ''), ?3, ?4, ?5, 1,
                                   ?6, ?7, ?8, ?9, ?10, ?10)",
                        params![
                            write.scope.character_id,
                            write.scope.character_content_revision_id,
                            write.scope.conversation_id,
                            write.scope.branch_id,
                            to_i64("portable runtime scope epoch", scope_epoch)?,
                            i64::from(write.payload.schema_version),
                            payload_json,
                            to_i64("portable runtime payload bytes", payload_bytes)?,
                            to_i64("portable runtime access sequence", access_sequence)?,
                            now_text,
                        ],
                    )
                    .map_err(storage_db_error)?;
            }
        }

        let (evicted_rows, evicted_bytes) = enforce_quota_with_limits(
            &transaction,
            &write.scope,
            payload_bytes,
            maximum_rows,
            maximum_bytes,
        )?;
        let created_at = current.as_ref().map_or(now, |record| record.created_at);
        let record = PortableRuntimeStateRecord {
            scope: write.scope,
            scope_epoch,
            revision,
            payload: write.payload,
            created_at,
            updated_at: now,
        };
        transaction.commit().map_err(storage_db_error)?;
        Ok(PortableRuntimeStateSaveResult::Saved {
            record,
            evicted_rows,
            evicted_bytes,
        })
    }
}

pub(crate) fn invalidate_portable_runtime_state_for_branch_transaction(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    branch_id: &str,
    invalidated_at: DateTime<Utc>,
) -> CoreResult<u64> {
    transaction
        .execute(
            "DELETE FROM portable_runtime_states
             WHERE conversation_id = ?1 AND branch_id = ?2",
            params![conversation_id, branch_id],
        )
        .map_err(storage_db_error)?;
    let current_epoch = transaction
        .query_row(
            "SELECT epoch
             FROM portable_runtime_branch_epochs
             WHERE conversation_id = ?1 AND branch_id = ?2",
            params![conversation_id, branch_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)
        .and_then(|value| from_nonnegative_i64("portable runtime scope epoch", value))?;
    let next_epoch = current_epoch
        .checked_add(1)
        .ok_or_else(|| CoreError::internal("portable runtime scope epoch overflowed"))?;
    validate_safe_integer("portable runtime scope epoch", next_epoch)?;
    let changed = transaction
        .execute(
            "UPDATE portable_runtime_branch_epochs
             SET epoch = ?3, updated_at = ?4
             WHERE conversation_id = ?1 AND branch_id = ?2 AND epoch = ?5",
            params![
                conversation_id,
                branch_id,
                to_i64("portable runtime scope epoch", next_epoch)?,
                invalidated_at.to_rfc3339(),
                to_i64("portable runtime scope epoch", current_epoch)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "portable runtime branch epoch changed during an immediate transaction",
        ));
    }
    Ok(next_epoch)
}

fn validate_scope(scope: &PortableRuntimeStateScope) -> CoreResult<()> {
    validate_identifier("portable runtime character", &scope.character_id)?;
    if let Some(revision_id) = &scope.character_content_revision_id {
        validate_identifier("portable runtime character revision", revision_id)?;
    }
    validate_identifier("portable runtime conversation", &scope.conversation_id)?;
    validate_identifier("portable runtime branch", &scope.branch_id)
}

fn validate_identifier(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 512
        || value.chars().count() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(format!("{label} id is invalid")));
    }
    Ok(())
}

fn validate_scope_relationships(
    transaction: &Transaction<'_>,
    scope: &PortableRuntimeStateScope,
) -> CoreResult<()> {
    let route_matches = transaction
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM conversations AS conversation
                 JOIN conversation_branches AS branch
                   ON branch.conversation_id = conversation.id
                  AND branch.id = ?2
                 WHERE conversation.id = ?1
                   AND conversation.character_id = ?3
             )",
            params![scope.conversation_id, scope.branch_id, scope.character_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)?;
    if !route_matches {
        return Err(CoreError::invalid(
            "portable runtime character, conversation, and branch do not belong together",
        ));
    }
    if let Some(revision_id) = &scope.character_content_revision_id {
        let revision_matches = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1
                     FROM character_content AS content
                     JOIN character_content_revisions AS revision
                       ON revision.object_id = content.object_id
                     WHERE content.character_id = ?1
                       AND revision.revision_id = ?2
                 )",
                params![scope.character_id, revision_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_db_error)?;
        if !revision_matches {
            return Err(CoreError::invalid(
                "portable runtime character revision does not belong to the character",
            ));
        }
    }
    Ok(())
}

fn load_scope_epoch(
    transaction: &Transaction<'_>,
    scope: &PortableRuntimeStateScope,
) -> CoreResult<u64> {
    let epoch = transaction
        .query_row(
            "SELECT epoch
             FROM portable_runtime_branch_epochs
             WHERE conversation_id = ?1 AND branch_id = ?2",
            params![scope.conversation_id, scope.branch_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)
        .and_then(|value| from_nonnegative_i64("portable runtime scope epoch", value))?;
    validate_safe_integer("stored portable runtime scope epoch", epoch)
        .map_err(|error| storage_corrupted(error.message))?;
    Ok(epoch)
}

fn load_record(
    transaction: &Transaction<'_>,
    scope: &PortableRuntimeStateScope,
    scope_epoch: u64,
) -> CoreResult<Option<PortableRuntimeStateRecord>> {
    let raw = transaction
        .query_row(
            "SELECT branch_epoch, revision, payload_schema_version,
                    payload_json, payload_bytes, created_at, updated_at
             FROM portable_runtime_states
             WHERE character_id = ?1
               AND character_content_revision_id IS ?2
               AND conversation_id = ?3
               AND branch_id = ?4
               AND branch_epoch = ?5",
            params![
                scope.character_id,
                scope.character_content_revision_id,
                scope.conversation_id,
                scope.branch_id,
                to_i64("portable runtime scope epoch", scope_epoch)?,
            ],
            |row| {
                Ok(RawPortableRuntimeStateRecord {
                    scope_epoch: row.get(0)?,
                    revision: row.get(1)?,
                    payload_schema_version: row.get(2)?,
                    payload_json: row.get(3)?,
                    payload_bytes: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    raw.map(|raw| decode_record(scope, raw)).transpose()
}

fn decode_record(
    scope: &PortableRuntimeStateScope,
    raw: RawPortableRuntimeStateRecord,
) -> CoreResult<PortableRuntimeStateRecord> {
    let scope_epoch = from_nonnegative_i64("portable runtime scope epoch", raw.scope_epoch)?;
    validate_safe_integer("stored portable runtime scope epoch", scope_epoch)
        .map_err(|error| storage_corrupted(error.message))?;
    let revision = from_positive_i64("portable runtime state revision", raw.revision)?;
    validate_positive_safe_integer("stored portable runtime state revision", revision)
        .map_err(|error| storage_corrupted(error.message))?;
    let schema_version = u32::try_from(raw.payload_schema_version)
        .map_err(|_| storage_corrupted("portable runtime payload schema version is invalid"))?;
    if schema_version == 0 {
        return Err(storage_corrupted(
            "portable runtime payload schema version is zero",
        ));
    }
    let payload_bytes = from_nonnegative_i64("portable runtime payload bytes", raw.payload_bytes)?;
    if payload_bytes != u64::try_from(raw.payload_json.len()).unwrap_or(u64::MAX) {
        return Err(storage_corrupted(
            "portable runtime payload byte count is inconsistent",
        ));
    }
    let value: Value = serde_json::from_str(&raw.payload_json).map_err(|error| {
        storage_corrupted(format!("portable runtime payload is invalid: {error}"))
    })?;
    let payload = PortableRuntimeStatePayload {
        schema_version,
        value,
    };
    validate_payload(&payload).map_err(|error| storage_corrupted(error.message))?;
    Ok(PortableRuntimeStateRecord {
        scope: scope.clone(),
        scope_epoch,
        revision,
        payload,
        created_at: parse_time("portable runtime state created_at", &raw.created_at)?,
        updated_at: parse_time("portable runtime state updated_at", &raw.updated_at)?,
    })
}

fn encode_payload(payload: &PortableRuntimeStatePayload) -> CoreResult<String> {
    validate_payload(payload)?;
    serde_json::to_string(&payload.value).map_err(|error| {
        CoreError::invalid(format!("portable runtime payload is invalid: {error}"))
    })
}

fn validate_payload(payload: &PortableRuntimeStatePayload) -> CoreResult<()> {
    if payload.schema_version != PORTABLE_RUNTIME_STATE_SCHEMA_VERSION {
        return Err(CoreError::invalid(format!(
            "portable runtime payload schema version must be {PORTABLE_RUNTIME_STATE_SCHEMA_VERSION}",
        )));
    }
    let object = payload
        .value
        .as_object()
        .ok_or_else(|| CoreError::invalid("portable runtime payload must be a JSON object"))?;
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
            return Err(CoreError::invalid(
                "portable runtime payload exceeds JSON depth or node limits",
            ));
        }
        match value {
            Value::Object(object) => {
                pending.extend(
                    object
                        .values()
                        .map(|child| (child, depth.saturating_add(1))),
                );
            }
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
        CoreError::invalid(format!("portable runtime payload is invalid: {error}"))
    })?;
    if json.len() > MAX_PORTABLE_RUNTIME_STATE_BYTES {
        return Err(CoreError::invalid(format!(
            "portable runtime payload exceeds its {MAX_PORTABLE_RUNTIME_STATE_BYTES}-byte limit"
        )));
    }
    Ok(())
}

fn required_field<'a>(
    object: &'a serde_json::Map<String, Value>,
    field: &str,
) -> CoreResult<&'a Value> {
    object
        .get(field)
        .ok_or_else(|| CoreError::invalid(format!("portable runtime payload is missing {field}")))
}

fn validate_exact_object_fields(
    label: &str,
    object: &serde_json::Map<String, Value>,
    fields: &[&str],
) -> CoreResult<()> {
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err(CoreError::invalid(format!(
            "{label} does not have the exact supported fields",
        )));
    }
    Ok(())
}

fn validate_string_record(label: &str, value: &Value, maximum_chars: usize) -> CoreResult<()> {
    let object = value
        .as_object()
        .ok_or_else(|| CoreError::invalid(format!("{label} must be a JSON object")))?;
    validate_record_keys(label, object)?;
    for item in object.values() {
        validate_bounded_string(label, item, maximum_chars)?;
    }
    Ok(())
}

fn validate_state_record(label: &str, value: &Value) -> CoreResult<()> {
    let object = value
        .as_object()
        .ok_or_else(|| CoreError::invalid(format!("{label} must be a JSON object")))?;
    validate_record_keys(label, object)?;
    for item in object.values() {
        validate_json_value_budget(label, item)?;
    }
    Ok(())
}

fn validate_record_keys(label: &str, object: &serde_json::Map<String, Value>) -> CoreResult<()> {
    if object.len() > MAX_PORTABLE_RUNTIME_RECORD_KEYS {
        return Err(CoreError::invalid(format!(
            "{label} exceeds its {MAX_PORTABLE_RUNTIME_RECORD_KEYS}-key limit",
        )));
    }
    for key in object.keys() {
        if key.is_empty()
            || javascript_string_length(key) > MAX_PORTABLE_RUNTIME_KEY_CHARS
            || matches!(key.as_str(), "__proto__" | "constructor" | "prototype")
        {
            return Err(CoreError::invalid(format!(
                "{label} contains an invalid portable runtime key",
            )));
        }
    }
    Ok(())
}

fn validate_bounded_string(label: &str, value: &Value, maximum_chars: usize) -> CoreResult<()> {
    let value = value
        .as_str()
        .ok_or_else(|| CoreError::invalid(format!("{label} must contain only strings")))?;
    if javascript_string_length(value) > maximum_chars {
        return Err(CoreError::invalid(format!(
            "{label} exceeds its {maximum_chars}-character limit",
        )));
    }
    Ok(())
}

fn validate_json_value_budget(label: &str, value: &Value) -> CoreResult<()> {
    let json = serde_json::to_string(value)
        .map_err(|error| CoreError::invalid(format!("{label} is invalid: {error}")))?;
    if json.len() > MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES {
        return Err(CoreError::invalid(format!(
            "{label} value exceeds its {MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES}-byte limit",
        )));
    }
    let mut pending = vec![value];
    let mut nodes = 0_usize;
    while let Some(value) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_PORTABLE_RUNTIME_STATE_VALUE_NODES {
            return Err(CoreError::invalid(format!(
                "{label} value exceeds its {MAX_PORTABLE_RUNTIME_STATE_VALUE_NODES}-node limit",
            )));
        }
        match value {
            Value::Object(object) => pending.extend(object.values()),
            Value::Array(array) => pending.extend(array),
            _ => {}
        }
    }
    Ok(())
}

fn validate_auxiliary_selection(value: &Value) -> CoreResult<()> {
    if value.is_null() {
        return Ok(());
    }
    let selection = value.as_object().ok_or_else(|| {
        CoreError::invalid("portable runtime auxiliarySelection must be null or an object")
    })?;
    let kind = selection
        .get("kind")
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::invalid("portable runtime auxiliarySelection kind is invalid"))?;
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
                    CoreError::invalid("portable runtime auxiliarySelection target is invalid")
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
            return Err(CoreError::invalid(
                "portable runtime auxiliarySelection kind is unsupported",
            ));
        }
    }
    Ok(())
}

fn require_string_field(object: &serde_json::Map<String, Value>, field: &str) -> CoreResult<()> {
    if object.get(field).is_none_or(|value| !value.is_string()) {
        return Err(CoreError::invalid(format!(
            "portable runtime auxiliarySelection {field} must be a string",
        )));
    }
    Ok(())
}

fn javascript_string_length(value: &str) -> usize {
    value.encode_utf16().count()
}

fn allocate_access_sequence(transaction: &Transaction<'_>) -> CoreResult<u64> {
    let current = transaction
        .query_row(
            "SELECT next_access_sequence
             FROM portable_runtime_state_sequence
             WHERE singleton = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage_db_error)
        .and_then(|value| from_positive_i64("portable runtime access sequence", value))?;
    let next = current
        .checked_add(1)
        .ok_or_else(|| CoreError::internal("portable runtime access sequence overflowed"))?;
    let changed = transaction
        .execute(
            "UPDATE portable_runtime_state_sequence
             SET next_access_sequence = ?1
             WHERE singleton = 1 AND next_access_sequence = ?2",
            params![
                to_i64("portable runtime access sequence", next)?,
                to_i64("portable runtime access sequence", current)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "portable runtime access sequence changed during an immediate transaction",
        ));
    }
    Ok(current)
}

fn enforce_quota_with_limits(
    transaction: &Transaction<'_>,
    protected_scope: &PortableRuntimeStateScope,
    protected_payload_bytes: u64,
    maximum_rows: u64,
    maximum_bytes: u64,
) -> CoreResult<(u32, u64)> {
    let (raw_rows, raw_bytes) = transaction
        .query_row(
            "SELECT COUNT(*), coalesce(SUM(payload_bytes), 0)
             FROM portable_runtime_states",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(storage_db_error)?;
    let mut rows = from_nonnegative_i64("portable runtime state row count", raw_rows)?;
    let mut bytes = from_nonnegative_i64("portable runtime state total bytes", raw_bytes)?;
    if rows <= maximum_rows && bytes <= maximum_bytes {
        return Ok((0, 0));
    }

    let candidates = load_eviction_candidates(transaction, protected_scope)?;

    let mut evicted_rows = 0_u32;
    let mut evicted_bytes = 0_u64;
    for candidate in candidates {
        if rows <= maximum_rows && bytes <= maximum_bytes {
            break;
        }
        let changed = transaction
            .execute(
                "DELETE FROM portable_runtime_states
                 WHERE character_id = ?1
                   AND character_content_revision_id IS ?2
                   AND conversation_id = ?3
                   AND branch_id = ?4",
                params![
                    candidate.scope.character_id,
                    candidate.scope.character_content_revision_id,
                    candidate.scope.conversation_id,
                    candidate.scope.branch_id,
                ],
            )
            .map_err(storage_db_error)?;
        if changed != 1 {
            return Err(storage_corrupted(
                "portable runtime LRU candidate disappeared during quota enforcement",
            ));
        }
        rows = rows
            .checked_sub(1)
            .ok_or_else(|| storage_corrupted("portable runtime eviction row count underflowed"))?;
        bytes = bytes
            .checked_sub(candidate.payload_bytes)
            .ok_or_else(|| storage_corrupted("portable runtime eviction byte count underflowed"))?;
        evicted_rows = evicted_rows
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("portable runtime eviction count overflowed"))?;
        evicted_bytes = evicted_bytes
            .checked_add(candidate.payload_bytes)
            .ok_or_else(|| CoreError::internal("portable runtime eviction bytes overflowed"))?;
    }
    if rows > maximum_rows || bytes > maximum_bytes || protected_payload_bytes > maximum_bytes {
        return Err(CoreError::internal(
            "portable runtime state quota cannot retain the current write",
        ));
    }
    Ok((evicted_rows, evicted_bytes))
}

fn load_eviction_candidates(
    transaction: &Transaction<'_>,
    protected_scope: &PortableRuntimeStateScope,
) -> CoreResult<Vec<EvictionCandidate>> {
    let mut statement = transaction
        .prepare(
            "SELECT character_id, character_content_revision_id,
                    conversation_id, branch_id, payload_bytes
             FROM portable_runtime_states
             WHERE NOT (
                 character_id = ?1
                 AND character_content_revision_id IS ?2
                 AND conversation_id = ?3
                 AND branch_id = ?4
             )
             ORDER BY access_sequence,
                      character_id,
                      character_content_revision_key,
                      conversation_id,
                      branch_id",
        )
        .map_err(storage_db_error)?;
    statement
        .query_map(
            params![
                protected_scope.character_id,
                protected_scope.character_content_revision_id,
                protected_scope.conversation_id,
                protected_scope.branch_id,
            ],
            |row| {
                Ok((
                    PortableRuntimeStateScope {
                        character_id: row.get(0)?,
                        character_content_revision_id: row.get(1)?,
                        conversation_id: row.get(2)?,
                        branch_id: row.get(3)?,
                    },
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?
        .into_iter()
        .map(|(scope, payload_bytes)| {
            Ok(EvictionCandidate {
                scope,
                payload_bytes: from_nonnegative_i64(
                    "portable runtime eviction payload bytes",
                    payload_bytes,
                )?,
            })
        })
        .collect::<CoreResult<Vec<_>>>()
}

fn validate_safe_integer(label: &str, value: u64) -> CoreResult<()> {
    if value > MAX_SAFE_INTEGER {
        return Err(CoreError::invalid(format!(
            "{label} exceeds the portable integer range"
        )));
    }
    Ok(())
}

fn validate_positive_safe_integer(label: &str, value: u64) -> CoreResult<()> {
    if value == 0 {
        return Err(CoreError::invalid(format!("{label} must be positive")));
    }
    validate_safe_integer(label, value)
}

fn to_i64(label: &str, value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid(format!("{label} exceeds SQLite range")))
}

fn from_nonnegative_i64(label: &str, value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| storage_corrupted(format!("{label} is negative")))
}

fn from_positive_i64(label: &str, value: i64) -> CoreResult<u64> {
    let value = from_nonnegative_i64(label, value)?;
    if value == 0 {
        return Err(storage_corrupted(format!("{label} is zero")));
    }
    Ok(value)
}

fn parse_time(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| storage_corrupted(format!("{label} is invalid: {error}")))
}

fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use lorepia_domain::{ConversationBranchId, ConversationId, CoreErrorCode, MessageId};
    use rusqlite::TransactionBehavior;
    use serde_json::json;
    use tempfile::TempDir;

    use super::*;

    const NOW: &str = "2026-08-29T00:00:00Z";
    const SOURCE_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    struct Fixture {
        root: TempDir,
        storage: Storage,
    }

    impl Fixture {
        fn open() -> Self {
            let root = tempfile::tempdir().expect("temporary portable runtime state root");
            let storage = Storage::open(root.path()).expect("open portable runtime state storage");
            seed_character(&storage, "character-a");
            Self { root, storage }
        }

        fn scope(&self, suffix: &str) -> PortableRuntimeStateScope {
            seed_scope(&self.storage, "character-a", suffix)
        }
    }

    #[test]
    fn state_round_trips_reopens_and_revision_conflicts_are_typed() {
        let fixture = Fixture::open();
        let scope = fixture.scope("round-trip");
        let missing = fixture
            .storage
            .get_portable_runtime_state(&scope)
            .expect("load missing state");
        assert_eq!(missing.scope_epoch, 0);
        assert!(missing.record.is_none());

        let first = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, None, json!({"visited": true})))
            .expect("save first state");
        let PortableRuntimeStateSaveResult::Saved { record, .. } = first else {
            panic!("first state write must be saved");
        };
        assert_eq!(record.revision, 1);

        let conflict = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, None, json!({})))
            .expect("return revision conflict");
        let PortableRuntimeStateSaveResult::RevisionConflict { current } = conflict else {
            panic!("stale create must return a revision conflict");
        };
        assert_eq!(current.expect("current state").revision, 1);

        let second = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, Some(1), json!({"visited": false})))
            .expect("save second state");
        let PortableRuntimeStateSaveResult::Saved { record, .. } = second else {
            panic!("second state write must be saved");
        };
        assert_eq!(record.revision, 2);
        assert_eq!(record.payload.value["state"]["visited"], false);

        let root = fixture.root.path().to_path_buf();
        drop(fixture.storage);
        let reopened = Storage::open(root).expect("reopen portable runtime state storage");
        let record = reopened
            .get_portable_runtime_state(&scope)
            .expect("load reopened state")
            .record
            .expect("reopened record");
        assert_eq!(record.revision, 2);
        assert_eq!(record.payload.value["state"]["visited"], false);
    }

    #[test]
    fn concurrent_writes_with_the_same_revision_save_exactly_once() {
        let Fixture {
            root: _root,
            storage,
        } = Fixture::open();
        let scope = seed_scope(&storage, "character-a", "concurrent-cas");
        storage
            .put_portable_runtime_state(write(&scope, 0, None, json!({"winner": "seed"})))
            .expect("seed concurrent state");

        let storage = Arc::new(storage);
        let barrier = Arc::new(Barrier::new(3));
        let handles = ["first", "second"].map(|candidate| {
            let storage = Arc::clone(&storage);
            let barrier = Arc::clone(&barrier);
            let scope = scope.clone();
            std::thread::spawn(move || {
                barrier.wait();
                storage.put_portable_runtime_state(write(
                    &scope,
                    0,
                    Some(1),
                    json!({"winner": candidate}),
                ))
            })
        });
        barrier.wait();

        let outcomes = handles.map(|handle| {
            handle
                .join()
                .expect("concurrent runtime writer must not panic")
                .expect("concurrent runtime write outcome")
        });
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| {
                    matches!(outcome, PortableRuntimeStateSaveResult::Saved { .. })
                })
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| {
                    matches!(
                        outcome,
                        PortableRuntimeStateSaveResult::RevisionConflict { .. }
                    )
                })
                .count(),
            1
        );
        let current = storage
            .get_portable_runtime_state(&scope)
            .expect("load concurrent winner")
            .record
            .expect("concurrent winner record");
        assert_eq!(current.revision, 2);
        assert!(matches!(
            current.payload.value["state"]["winner"].as_str(),
            Some("first" | "second")
        ));
    }

    #[test]
    fn payload_bounds_reject_oversized_and_deep_updates_without_losing_current_state() {
        let fixture = Fixture::open();
        let scope = fixture.scope("bounds");
        fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, None, json!({"ok": true})))
            .expect("save bounded state");

        let oversized = json!({"value": "x".repeat(MAX_PORTABLE_RUNTIME_STATE_BYTES)});
        let error = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, Some(1), oversized))
            .expect_err("oversized state must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        let mut deep = json!(true);
        for _ in 0..=MAX_PORTABLE_RUNTIME_STATE_DEPTH {
            deep = json!({"nested": deep});
        }
        let error = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, Some(1), deep))
            .expect_err("deep state must fail");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        let many_nodes = json!({
            "nodes": vec![Value::Null; MAX_PORTABLE_RUNTIME_STATE_VALUE_NODES]
        });
        assert!(
            serde_json::to_vec(&many_nodes)
                .expect("encode node-bound payload")
                .len()
                < MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES
        );
        let error = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, Some(1), many_nodes))
            .expect_err("wide state value must fail its node bound");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        let exact_node_value = Value::Array(vec![
            Value::Null;
            MAX_PORTABLE_RUNTIME_STATE_VALUE_NODES - 1
        ]);
        validate_json_value_budget("portable runtime state", &exact_node_value)
            .expect("exact per-value node budget must be accepted");
        let mut globally_wide_state = serde_json::Map::new();
        for index in 0..50 {
            globally_wide_state.insert(format!("wide-{index}"), exact_node_value.clone());
        }
        let globally_wide_state = Value::Object(globally_wide_state);
        assert!(
            serde_json::to_vec(&portable_runtime_state_value(globally_wide_state.clone()))
                .expect("encode globally node-bound payload")
                .len()
                < MAX_PORTABLE_RUNTIME_STATE_BYTES
        );
        let error = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, Some(1), globally_wide_state))
            .expect_err("globally wide state must fail the payload node bound");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);

        let current = fixture
            .storage
            .get_portable_runtime_state(&scope)
            .expect("reload bounded state")
            .record
            .expect("bounded record");
        assert_eq!(current.revision, 1);
        assert_eq!(current.payload.value["state"], json!({"ok": true}));

        let exact_value =
            Value::String("x".repeat(MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES - "\"\"".len()));
        assert_eq!(
            serde_json::to_vec(&exact_value)
                .expect("encode exact-boundary state value")
                .len(),
            MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES
        );
        let exact = json!({"value": exact_value});
        let saved = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, Some(1), exact))
            .expect("save exact 64 KiB state value");
        assert!(matches!(
            saved,
            PortableRuntimeStateSaveResult::Saved { .. }
        ));
    }

    #[test]
    fn payload_contract_accepts_only_the_exact_renderer_shape() {
        let valid = PortableRuntimeStatePayload {
            schema_version: PORTABLE_RUNTIME_STATE_SCHEMA_VERSION,
            value: json!({
                "options": {"tone": "warm"},
                "chatVars": {"turn": 3},
                "state": {"visited": ["intro"]},
                "messageOverrides": {"message": "override"},
                "background": "<section>safe</section>",
                "auxiliarySelection": {
                    "kind": "target",
                    "target": {
                        "model_route_id": "route",
                        "generation_preset_id": "preset"
                    }
                }
            }),
        };
        validate_payload(&valid).expect("exact renderer payload must be accepted");

        let mut future = valid.clone();
        future.schema_version = PORTABLE_RUNTIME_STATE_SCHEMA_VERSION + 1;
        assert_eq!(
            validate_payload(&future)
                .expect_err("future schema must fail closed")
                .code,
            CoreErrorCode::InvalidInput
        );

        for malformed in [
            json!({
                "options": {},
                "chatVars": {},
                "state": {},
                "messageOverrides": {},
                "background": ""
            }),
            json!({
                "options": {},
                "chatVars": {},
                "state": {},
                "messageOverrides": {},
                "background": "",
                "auxiliarySelection": null,
                "future": true
            }),
            json!({
                "options": {},
                "chatVars": [],
                "state": {},
                "messageOverrides": {},
                "background": "",
                "auxiliarySelection": null
            }),
            json!({
                "options": {},
                "chatVars": {},
                "state": {},
                "messageOverrides": {},
                "background": "",
                "auxiliarySelection": {
                    "kind": "legacy_profile",
                    "provider_profile_id": "profile",
                    "future": true
                }
            }),
        ] {
            let payload = PortableRuntimeStatePayload {
                schema_version: PORTABLE_RUNTIME_STATE_SCHEMA_VERSION,
                value: malformed,
            };
            assert_eq!(
                validate_payload(&payload)
                    .expect_err("malformed renderer payload must fail closed")
                    .code,
                CoreErrorCode::InvalidInput
            );
        }
    }

    #[test]
    fn invalid_payloads_do_not_consume_or_block_the_current_cas_revision() {
        let fixture = Fixture::open();
        let scope = fixture.scope("invalid-payload-cas");
        fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, None, json!({"step": 1})))
            .expect("seed valid state");

        let mut future = write(&scope, 0, Some(1), json!({"step": 2}));
        future.payload.schema_version = PORTABLE_RUNTIME_STATE_SCHEMA_VERSION + 1;
        assert_eq!(
            fixture
                .storage
                .put_portable_runtime_state(future)
                .expect_err("future schema must fail before CAS")
                .code,
            CoreErrorCode::InvalidInput
        );

        let mut malformed = write(&scope, 0, Some(1), json!({"step": 2}));
        malformed
            .payload
            .value
            .as_object_mut()
            .expect("test payload object")
            .insert("future".to_owned(), Value::Bool(true));
        assert_eq!(
            fixture
                .storage
                .put_portable_runtime_state(malformed)
                .expect_err("malformed payload must fail before CAS")
                .code,
            CoreErrorCode::InvalidInput
        );

        let current = fixture
            .storage
            .get_portable_runtime_state(&scope)
            .expect("load state after rejected writes")
            .record
            .expect("valid state must remain");
        assert_eq!(current.revision, 1);
        assert_eq!(current.payload.value["state"]["step"], 1);

        let saved = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, Some(1), json!({"step": 2})))
            .expect("valid retry must still use the original CAS revision");
        let PortableRuntimeStateSaveResult::Saved { record, .. } = saved else {
            panic!("valid retry must save after rejected payloads");
        };
        assert_eq!(record.revision, 2);
        assert_eq!(record.payload.value["state"]["step"], 2);
    }

    #[test]
    fn payload_contract_enforces_renderer_record_and_string_bounds() {
        let mut dangerous_key = portable_runtime_state_value(json!({}));
        dangerous_key["options"] = json!({"__proto__": "blocked"});

        let mut long_key = portable_runtime_state_value(json!({}));
        long_key["chatVars"] = Value::Object(serde_json::Map::from_iter([(
            "k".repeat(MAX_PORTABLE_RUNTIME_KEY_CHARS + 1),
            Value::Bool(true),
        )]));

        let mut long_option = portable_runtime_state_value(json!({}));
        long_option["options"] = json!({
            "emoji": "🙂".repeat((MAX_PORTABLE_RUNTIME_OPTION_VALUE_CHARS / 2) + 1)
        });

        let mut too_many_state_keys = serde_json::Map::new();
        for index in 0..=MAX_PORTABLE_RUNTIME_RECORD_KEYS {
            too_many_state_keys.insert(format!("key-{index}"), Value::Null);
        }
        let mut too_many_keys = portable_runtime_state_value(json!({}));
        too_many_keys["state"] = Value::Object(too_many_state_keys);

        let mut long_override = portable_runtime_state_value(json!({}));
        long_override["messageOverrides"] = json!({
            "message": "x".repeat(MAX_PORTABLE_RUNTIME_MESSAGE_OVERRIDE_CHARS + 1)
        });

        let mut long_background = portable_runtime_state_value(json!({}));
        long_background["background"] =
            Value::String("x".repeat(MAX_PORTABLE_RUNTIME_BACKGROUND_CHARS + 1));

        let mut oversized_state_value = portable_runtime_state_value(json!({}));
        oversized_state_value["state"] = json!({
            "large": "x".repeat(MAX_PORTABLE_RUNTIME_STATE_VALUE_BYTES)
        });

        for invalid in [
            dangerous_key,
            long_key,
            long_option,
            too_many_keys,
            long_override,
            long_background,
            oversized_state_value,
        ] {
            let error = validate_payload(&PortableRuntimeStatePayload {
                schema_version: PORTABLE_RUNTIME_STATE_SCHEMA_VERSION,
                value: invalid,
            })
            .expect_err("renderer state bound must fail closed");
            assert_eq!(error.code, CoreErrorCode::InvalidInput);
        }
    }

    #[test]
    fn scope_relationships_reject_wrong_character_revision_conversation_and_branch() {
        let fixture = Fixture::open();
        seed_character(&fixture.storage, "character-b");
        seed_character_revision(&fixture.storage, "character-b", "revision-b");
        let scope = fixture.scope("ownership");

        let wrong_character = PortableRuntimeStateScope {
            character_id: "character-b".to_owned(),
            ..scope.clone()
        };
        assert_eq!(
            fixture
                .storage
                .get_portable_runtime_state(&wrong_character)
                .expect_err("conversation character mismatch must fail")
                .code,
            CoreErrorCode::InvalidInput
        );

        let wrong_revision = PortableRuntimeStateScope {
            character_content_revision_id: Some("revision-b".to_owned()),
            ..scope
        };
        assert_eq!(
            fixture
                .storage
                .get_portable_runtime_state(&wrong_revision)
                .expect_err("character revision mismatch must fail")
                .code,
            CoreErrorCode::InvalidInput
        );
    }

    #[test]
    fn deterministic_lru_touch_and_byte_quota_evict_the_oldest_unprotected_scope() {
        assert_eq!(MAX_PORTABLE_RUNTIME_STATE_ROWS, 1_024);
        assert_eq!(MAX_PORTABLE_RUNTIME_STATE_TOTAL_BYTES, 64 * 1_024 * 1_024);
        let fixture = Fixture::open();
        let first = fixture.scope("lru-a");
        let second = fixture.scope("lru-b");
        let protected = fixture.scope("lru-c");
        for (scope, value) in [
            (&first, json!({"value": "first"})),
            (&second, json!({"value": "second"})),
            (&protected, json!({"value": "protected"})),
        ] {
            fixture
                .storage
                .put_portable_runtime_state(write(scope, 0, None, value))
                .expect("seed LRU state");
        }
        fixture
            .storage
            .get_portable_runtime_state(&first)
            .expect("touch first state");

        let mut connection = fixture.storage.connection().expect("quota connection");
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("quota transaction");
        let protected_bytes = payload_bytes(&transaction, &protected);
        let first_bytes = payload_bytes(&transaction, &first);
        let (evicted_rows, evicted_bytes) = enforce_quota_with_limits(
            &transaction,
            &protected,
            protected_bytes,
            3,
            protected_bytes + first_bytes,
        )
        .expect("enforce test byte quota");
        transaction.commit().expect("commit quota eviction");
        drop(connection);

        assert_eq!(evicted_rows, 1);
        assert!(evicted_bytes > 0);
        assert!(
            fixture
                .storage
                .get_portable_runtime_state(&first)
                .expect("load touched state")
                .record
                .is_some()
        );
        assert!(
            fixture
                .storage
                .get_portable_runtime_state(&second)
                .expect("load evicted state")
                .record
                .is_none()
        );
        assert!(
            fixture
                .storage
                .get_portable_runtime_state(&protected)
                .expect("load protected state")
                .record
                .is_some()
        );
    }

    #[test]
    fn quota_failure_rolls_back_the_current_write_and_candidate_evictions() {
        let fixture = Fixture::open();
        let candidate = fixture.scope("quota-rollback-candidate");
        let protected = fixture.scope("quota-rollback-protected");
        fixture
            .storage
            .put_portable_runtime_state(write(
                &candidate,
                0,
                None,
                json!({"state": "must survive"}),
            ))
            .expect("seed quota rollback candidate");

        let error = fixture
            .storage
            .put_portable_runtime_state_with_limits(
                write(&protected, 0, None, json!({"state": "must roll back"})),
                1,
                1,
            )
            .expect_err("impossible quota must fail after considering eviction");
        assert_eq!(error.code, CoreErrorCode::Internal);

        let candidate_record = fixture
            .storage
            .get_portable_runtime_state(&candidate)
            .expect("load rolled-back eviction candidate")
            .record
            .expect("candidate eviction must have rolled back");
        assert_eq!(candidate_record.revision, 1);
        assert_eq!(
            candidate_record.payload.value["state"]["state"],
            "must survive"
        );
        assert!(
            fixture
                .storage
                .get_portable_runtime_state(&protected)
                .expect("load rolled-back protected write")
                .record
                .is_none(),
            "the current write must roll back with quota enforcement"
        );
    }

    #[test]
    fn explicit_branch_rewind_deletes_state_and_rejects_stale_epoch_writes() {
        let fixture = Fixture::open();
        let scope = fixture.scope("rewind");
        seed_branch_head(&fixture.storage, &scope, "rewind-message");
        fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, None, json!({"step": 1})))
            .expect("save pre-rewind state");

        fixture
            .storage
            .remove_message_from_branch(
                &ConversationId(scope.conversation_id.clone()),
                &ConversationBranchId(scope.branch_id.clone()),
                Some(&MessageId("rewind-message".to_owned())),
                &MessageId("rewind-message".to_owned()),
            )
            .expect("rewind branch");

        let snapshot = fixture
            .storage
            .get_portable_runtime_state(&scope)
            .expect("load post-rewind state");
        assert_eq!(snapshot.scope_epoch, 1);
        assert!(snapshot.record.is_none());
        let stale = fixture
            .storage
            .put_portable_runtime_state(write(&scope, 0, Some(1), json!({"step": 2})))
            .expect("return stale epoch outcome");
        assert_eq!(
            stale,
            PortableRuntimeStateSaveResult::ScopeInvalidated {
                current_scope_epoch: 1
            }
        );
    }

    fn write(
        scope: &PortableRuntimeStateScope,
        expected_scope_epoch: u64,
        expected_revision: Option<u64>,
        value: Value,
    ) -> PortableRuntimeStateWrite {
        PortableRuntimeStateWrite {
            scope: scope.clone(),
            expected_scope_epoch,
            expected_revision,
            payload: PortableRuntimeStatePayload {
                schema_version: PORTABLE_RUNTIME_STATE_SCHEMA_VERSION,
                value: portable_runtime_state_value(value),
            },
        }
    }

    fn portable_runtime_state_value(state: Value) -> Value {
        json!({
            "options": {},
            "chatVars": {},
            "state": state,
            "messageOverrides": {},
            "background": "",
            "auxiliarySelection": null
        })
    }

    fn seed_character(storage: &Storage, character_id: &str) {
        let connection = storage.connection().expect("seed character connection");
        connection
            .execute(
                "INSERT OR IGNORE INTO content_sources
                 (sha256, relative_path, size_bytes, created_at)
                 VALUES (?1, 'source.bin', 1, ?2)",
                params![SOURCE_HASH, NOW],
            )
            .expect("seed content source");
        connection
            .execute(
                "INSERT OR IGNORE INTO characters
                 (id, name, description, source_hash, avatar_asset_hash, created_at)
                 VALUES (?1, ?1, '', ?2, NULL, ?3)",
                params![character_id, SOURCE_HASH, NOW],
            )
            .expect("seed character");
    }

    fn seed_scope(
        storage: &Storage,
        character_id: &str,
        suffix: &str,
    ) -> PortableRuntimeStateScope {
        let conversation_id = format!("conversation-{suffix}");
        let branch_id = format!("branch-{suffix}");
        let connection = storage.connection().expect("seed scope connection");
        connection
            .execute(
                "INSERT INTO conversations
                 (id, character_id, title, created_at, updated_at)
                 VALUES (?1, ?2, ?1, ?3, ?3)",
                params![conversation_id, character_id, NOW],
            )
            .expect("seed conversation");
        connection
            .execute(
                "INSERT INTO conversation_branches
                 (id, conversation_id, title, fork_message_id, head_message_id,
                  created_at, updated_at)
                 VALUES (?1, ?2, NULL, NULL, NULL, ?3, ?3)",
                params![branch_id, conversation_id, NOW],
            )
            .expect("seed branch");
        connection
            .execute(
                "INSERT INTO conversation_state
                 (conversation_id, active_branch_id, selected_mode, updated_at)
                 VALUES (?1, ?2, 'chat', ?3)",
                params![conversation_id, branch_id, NOW],
            )
            .expect("seed conversation state");
        drop(connection);
        PortableRuntimeStateScope {
            character_id: character_id.to_owned(),
            character_content_revision_id: None,
            conversation_id,
            branch_id,
        }
    }

    fn seed_character_revision(storage: &Storage, character_id: &str, revision_id: &str) {
        let object_id = format!("content-{character_id}");
        let connection = storage.connection().expect("seed revision connection");
        connection
            .execute(
                "INSERT INTO content_objects (id, object_kind, created_at, deleted_at)
                 VALUES (?1, 'character_content', ?2, NULL)",
                params![object_id, NOW],
            )
            .expect("seed character content object");
        connection
            .execute(
                "INSERT INTO content_revisions (
                     id, object_id, object_kind, revision_no, parent_revision_id,
                     schema_version, document_json, document_sha256, source_kind,
                     source_hash, provenance_json, local_override_of_revision_id,
                     created_at
                 ) VALUES (?1, ?2, 'character_content', 1, NULL, 1, '{}', ?3,
                           'migrated', NULL, '{}', NULL, ?4)",
                params![revision_id, object_id, SOURCE_HASH, NOW],
            )
            .expect("seed content revision");
        connection
            .execute(
                "INSERT INTO character_content (object_id, character_id)
                 VALUES (?1, ?2)",
                params![object_id, character_id],
            )
            .expect("seed character content identity");
        connection
            .execute(
                "INSERT INTO character_content_revisions (
                     revision_id, object_id, unknown_extensions_json,
                     metadata_json, payload_json
                 ) VALUES (?1, ?2, '{}', '{}', '{}')",
                params![revision_id, object_id],
            )
            .expect("seed character content revision");
    }

    fn seed_branch_head(storage: &Storage, scope: &PortableRuntimeStateScope, message_id: &str) {
        let connection = storage.connection().expect("seed branch head connection");
        connection
            .execute(
                "INSERT INTO messages
                 (id, conversation_id, parent_id, role, content, status,
                  generation_id, created_at)
                 VALUES (?1, ?2, NULL, 'user', 'message', 'complete', NULL, ?3)",
                params![message_id, scope.conversation_id, NOW],
            )
            .expect("seed branch message");
        connection
            .execute(
                "UPDATE conversation_branches
                 SET head_message_id = ?3
                 WHERE id = ?1 AND conversation_id = ?2",
                params![scope.branch_id, scope.conversation_id, message_id],
            )
            .expect("seed branch head");
    }

    fn payload_bytes(transaction: &Transaction<'_>, scope: &PortableRuntimeStateScope) -> u64 {
        let value = transaction
            .query_row(
                "SELECT payload_bytes
                 FROM portable_runtime_states
                 WHERE character_id = ?1
                   AND character_content_revision_id IS ?2
                   AND conversation_id = ?3
                   AND branch_id = ?4",
                params![
                    scope.character_id,
                    scope.character_content_revision_id,
                    scope.conversation_id,
                    scope.branch_id,
                ],
                |row| row.get::<_, i64>(0),
            )
            .expect("load payload bytes");
        u64::try_from(value).expect("nonnegative payload bytes")
    }
}
