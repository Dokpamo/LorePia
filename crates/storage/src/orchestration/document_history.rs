//! Immutable revision history, deterministic diffs, and generic rollback.

use super::{
    BTreeSet, Connection, ContentModuleId, ContentModuleRevisionDiff, CoreError, CoreErrorCode,
    CoreResult, DateTime, DeserializeOwned, Digest, MAX_CHARACTER_CONTENT_JSON_BYTES,
    MAX_CHARACTER_CONTENT_JSON_CHARS, MAX_CHARACTER_CONTENT_JSON_NODES,
    MAX_ORCHESTRATION_JSON_BYTES, MAX_ORCHESTRATION_JSON_CHARS, MAX_ORCHESTRATION_JSON_DEPTH,
    MAX_ORCHESTRATION_JSON_NODES, ObjectRevision, OptionalExtension, PromptPresetId,
    PromptPresetRevisionDiff, Provenance, Serialize, Sha256, SourceKind, Storage, StoredRevision,
    Transaction, TransactionBehavior, Utc, Uuid, Value, params, storage_db_error,
};

#[derive(Debug, Clone, Copy)]
pub(super) enum DocumentTable {
    Personas,
    PromptPresets,
    TaskProfiles,
    KnowledgeBooks,
    MemoryProfiles,
    MemorySummarySchemas,
    TransformSets,
    InteractionRuleSets,
    ContentModules,
    CharacterContent,
}

impl DocumentTable {
    pub(super) const fn object_kind(self) -> &'static str {
        match self {
            Self::Personas => "persona",
            Self::PromptPresets => "prompt_preset",
            Self::TaskProfiles => "task_profile",
            Self::KnowledgeBooks => "knowledge_book",
            Self::MemoryProfiles => "memory_profile",
            Self::MemorySummarySchemas => "memory_summary_schema",
            Self::TransformSets => "transform_set",
            Self::InteractionRuleSets => "interaction_rule_set",
            Self::ContentModules => "content_module",
            Self::CharacterContent => "character_content",
        }
    }

    pub(super) const fn current_table(self) -> Option<&'static str> {
        match self {
            Self::Personas | Self::CharacterContent => None,
            Self::PromptPresets => Some("prompt_presets"),
            Self::TaskProfiles => Some("task_profiles"),
            Self::KnowledgeBooks => Some("knowledge_books"),
            Self::MemoryProfiles => Some("memory_profiles"),
            Self::MemorySummarySchemas => Some("memory_summary_schemas"),
            Self::TransformSets => Some("transform_sets"),
            Self::InteractionRuleSets => Some("interaction_rule_sets"),
            Self::ContentModules => Some("content_modules"),
        }
    }
}

pub(super) fn encode_document<T>(label: &str, value: &T) -> CoreResult<(String, String)>
where
    T: Serialize + DeserializeOwned,
{
    let json = serde_json::to_string(value)
        .map_err(|error| CoreError::invalid(format!("{label} cannot be serialized: {error}")))?;
    validate_json_bounds(label, &json)?;
    // This is deliberately a typed round trip, rather than checking only that
    // SQLite accepts syntactically valid JSON. It catches non-finite numbers,
    // custom-deserializer invariants, and wire-shape drift before mutation.
    let _: T = serde_json::from_str(&json)
        .map_err(|error| CoreError::invalid(format!("{label} cannot round-trip: {error}")))?;
    let sha256 = sha256_hex(json.as_bytes());
    Ok((json, sha256))
}

pub(super) fn decode_document<T>(label: &str, json: &str) -> CoreResult<T>
where
    T: DeserializeOwned,
{
    validate_json_bounds(label, json).map_err(|error| {
        storage_corrupted(format!(
            "{label} violates storage bounds: {}",
            error.message
        ))
    })?;
    serde_json::from_str(json)
        .map_err(|error| storage_corrupted(format!("stored {label} is invalid: {error}")))
}

pub(super) fn validate_json_bounds(label: &str, json: &str) -> CoreResult<()> {
    let character_content = matches!(label, "character_content" | "character content");
    let max_bytes = if character_content {
        MAX_CHARACTER_CONTENT_JSON_BYTES
    } else {
        MAX_ORCHESTRATION_JSON_BYTES
    };
    let max_chars = if character_content {
        MAX_CHARACTER_CONTENT_JSON_CHARS
    } else {
        MAX_ORCHESTRATION_JSON_CHARS
    };
    let max_nodes = if character_content {
        MAX_CHARACTER_CONTENT_JSON_NODES
    } else {
        MAX_ORCHESTRATION_JSON_NODES
    };
    if json.len() > max_bytes || json.chars().count() > max_chars {
        return Err(CoreError::invalid(format!(
            "{label} exceeds its JSON storage limit"
        )));
    }
    let value = serde_json::from_str::<Value>(json)
        .map_err(|error| CoreError::invalid(format!("{label} is invalid JSON: {error}")))?;
    let mut pending = vec![(&value, 0_usize)];
    let mut visited = 0_usize;
    while let Some((node, depth)) = pending.pop() {
        visited = visited.saturating_add(1);
        if visited > max_nodes || depth > MAX_ORCHESTRATION_JSON_DEPTH {
            return Err(CoreError::invalid(format!(
                "{label} exceeds JSON nesting or node limits"
            )));
        }
        match node {
            Value::Object(object) => {
                for (key, child) in object {
                    if is_forbidden_secret_key(key) {
                        return Err(CoreError::invalid(format!(
                            "{label} contains a raw credential field"
                        )));
                    }
                    pending.push((child, depth.saturating_add(1)));
                }
            }
            Value::Array(array) => {
                pending.extend(array.iter().map(|child| (child, depth.saturating_add(1))));
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_forbidden_secret_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "api_key"
            | "authorization"
            | "password"
            | "private_key"
            | "client_secret"
            | "access_token"
            | "refresh_token"
            | "credential"
    )
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub(super) fn storage_corrupted(message: impl Into<String>) -> CoreError {
    CoreError::new(CoreErrorCode::StorageCorrupted, message, false)
}

pub(super) fn not_found(kind: &str) -> CoreError {
    CoreError::new(
        CoreErrorCode::NotFound,
        format!("{kind} was not found"),
        false,
    )
}

pub(super) fn revision_conflict(
    kind: &str,
    id: &str,
    expected: Option<u64>,
    actual: Option<u64>,
) -> CoreError {
    CoreError::new(
        CoreErrorCode::InvalidInput,
        format!(
            "{kind} revision conflict for {id}: expected {}, current {}",
            expected.map_or_else(|| "new".to_owned(), |value| value.to_string()),
            actual.map_or_else(|| "missing".to_owned(), |value| value.to_string())
        ),
        true,
    )
}

pub(super) fn parse_datetime(label: &str, value: &str) -> CoreResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| storage_corrupted(format!("stored {label} is invalid: {error}")))
}

pub(super) fn i64_revision(value: u64) -> CoreResult<i64> {
    i64::try_from(value).map_err(|_| CoreError::invalid("revision exceeds SQLite integer range"))
}

pub(super) fn u64_revision(value: i64) -> CoreResult<u64> {
    u64::try_from(value).map_err(|_| storage_corrupted("stored revision is negative"))
}

pub(super) type RawStoredDocument = (String, i64, Option<String>, String, String, Option<String>);

pub(super) struct RevisionWrite {
    pub(super) state_version: u64,
    pub(super) revision_id: String,
    pub(super) created_at: DateTime<Utc>,
    pub(super) updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RevisionEventKind {
    Create,
    Update,
    Import,
    Rollback,
    SoftDelete,
}

impl RevisionEventKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Import => "import",
            Self::Rollback => "rollback",
            Self::SoftDelete => "soft_delete",
        }
    }
}

pub(super) fn decode_stored_document<T>(
    label: &str,
    row: RawStoredDocument,
) -> CoreResult<StoredRevision<T>>
where
    T: DeserializeOwned,
{
    Ok(StoredRevision {
        value: decode_document(label, &row.0)?,
        revision: u64_revision(row.1)?,
        revision_id: row.2,
        created_at: parse_datetime("created_at", &row.3)?,
        updated_at: parse_datetime("updated_at", &row.4)?,
        deleted_at: row
            .5
            .as_deref()
            .map(|value| parse_datetime("deleted_at", value))
            .transpose()?,
    })
}

pub(super) fn validate_identifier(label: &str, value: &str) -> CoreResult<()> {
    if value.is_empty()
        || value.len() > 256
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(CoreError::invalid(format!(
            "{label} id is empty, oversized, untrimmed, or contains control characters"
        )));
    }
    Ok(())
}

pub(super) fn validate_optional_sha256(label: &str, value: Option<&str>) -> CoreResult<()> {
    if let Some(value) = value
        && (value.len() != 64
            || value
                .bytes()
                .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase()))
    {
        return Err(CoreError::invalid(format!(
            "{label} must be a lowercase SHA-256 digest"
        )));
    }
    Ok(())
}

pub(super) fn source_kind_str(kind: &SourceKind) -> &'static str {
    match kind {
        SourceKind::ApplicationBuiltIn => "application_built_in",
        SourceKind::UserCreated => "user_created",
        SourceKind::ImportedStandard => "imported_standard",
        SourceKind::ImportedPackage => "imported_package",
        SourceKind::Generated => "generated",
    }
}

pub(super) fn revision_diff_json(before: Option<&str>, after: &str) -> CoreResult<String> {
    let before = before
        .map(serde_json::from_str::<Value>)
        .transpose()
        .map_err(|error| storage_corrupted(format!("stored revision JSON is invalid: {error}")))?;
    let after = serde_json::from_str::<Value>(after)
        .map_err(|error| CoreError::invalid(format!("revision JSON is invalid: {error}")))?;
    let mut changed_paths = BTreeSet::new();
    collect_changed_paths(before.as_ref(), Some(&after), "", &mut changed_paths);
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "before_sha256": before.as_ref().map(|value| {
            serde_json::to_vec(value).map_or_else(|_| String::new(), |bytes| sha256_hex(&bytes))
        }),
        "after_sha256": sha256_hex(after.to_string().as_bytes()),
        "changed_paths": changed_paths,
    }))
    .map_err(|error| CoreError::internal(format!("cannot encode revision diff: {error}")))
}

fn collect_changed_paths(
    before: Option<&Value>,
    after: Option<&Value>,
    path: &str,
    changed: &mut BTreeSet<String>,
) {
    if before == after {
        return;
    }
    match (before, after) {
        (Some(Value::Object(before)), Some(Value::Object(after))) => {
            let keys = before.keys().chain(after.keys()).collect::<BTreeSet<_>>();
            for key in keys {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                collect_changed_paths(
                    before.get(key),
                    after.get(key),
                    &format!("{path}/{escaped}"),
                    changed,
                );
            }
        }
        _ => {
            changed.insert(if path.is_empty() {
                "/".to_owned()
            } else {
                path.to_owned()
            });
        }
    }
}

pub(super) fn document_schema_version<T>(table: DocumentTable, value: &T) -> CoreResult<u32>
where
    T: Serialize,
{
    if matches!(table, DocumentTable::TaskProfiles) {
        return Ok(1);
    }
    let value = serde_json::to_value(value)
        .map_err(|error| CoreError::invalid(format!("cannot inspect schema version: {error}")))?;
    value
        .get("schema_version")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .ok_or_else(|| CoreError::invalid("content object requires a positive schema_version"))
}

pub(super) fn document_provenance<T>(table: DocumentTable, value: &T) -> CoreResult<Provenance>
where
    T: Serialize,
{
    let value = serde_json::to_value(value)
        .map_err(|error| CoreError::invalid(format!("cannot inspect provenance: {error}")))?;
    let provenance = if matches!(
        table,
        DocumentTable::PromptPresets | DocumentTable::ContentModules
    ) {
        value
            .get("metadata")
            .and_then(|metadata| metadata.get("provenance"))
    } else {
        value.get("provenance")
    };
    let parsed = provenance
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| CoreError::invalid(format!("content provenance is invalid: {error}")))?;
    if let Some(parsed) = parsed {
        Ok(parsed)
    } else if matches!(
        table,
        DocumentTable::TaskProfiles | DocumentTable::CharacterContent
    ) {
        Ok(Provenance {
            source_kind: SourceKind::UserCreated,
            source_id: None,
            source_hash: None,
            author: None,
            license: None,
            imported_at: None,
        })
    } else {
        Err(CoreError::invalid("content object requires provenance"))
    }
}

pub(super) fn list_object_revisions<T>(
    storage: &Storage,
    table: DocumentTable,
    id: &str,
) -> CoreResult<Vec<ObjectRevision<T>>>
where
    T: DeserializeOwned,
{
    let connection = storage.connection()?;
    let mut statement = connection
        .prepare(
            "SELECT id, revision_no, document_json, document_sha256, created_at
             FROM content_revisions
             WHERE object_id = ?1 AND object_kind = ?2
             ORDER BY revision_no, id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map(params![id, table.object_kind()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter()
        .map(
            |(revision_id, revision, document_json, sha256, created_at)| {
                Ok(ObjectRevision {
                    revision_id,
                    object_kind: table.object_kind().to_owned(),
                    object_id: id.to_owned(),
                    revision: u64_revision(revision)?,
                    value: decode_document(table.object_kind(), &document_json)?,
                    sha256,
                    created_at: parse_datetime("content revision created_at", &created_at)?,
                })
            },
        )
        .collect()
}

pub(super) fn get_object_revision<T>(
    transaction: &Transaction<'_>,
    table: DocumentTable,
    id: &str,
    revision: u64,
) -> CoreResult<ObjectRevision<T>>
where
    T: DeserializeOwned,
{
    let row = transaction
        .query_row(
            "SELECT id, document_json, document_sha256, created_at
             FROM content_revisions
             WHERE object_id = ?1 AND object_kind = ?2 AND revision_no = ?3",
            params![id, table.object_kind(), i64_revision(revision)?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("content revision"))?;
    Ok(ObjectRevision {
        revision_id: row.0,
        object_kind: table.object_kind().to_owned(),
        object_id: id.to_owned(),
        revision,
        value: decode_document(table.object_kind(), &row.1)?,
        sha256: row.2,
        created_at: parse_datetime("content revision created_at", &row.3)?,
    })
}

pub(super) fn diff_content_object_revisions(
    storage: &Storage,
    table: DocumentTable,
    id: &str,
    from_revision: u64,
    to_revision: u64,
) -> CoreResult<ContentModuleRevisionDiff> {
    let mut connection = storage.connection()?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let from = get_object_revision::<Value>(&transaction, table, id, from_revision)?;
    let to = get_object_revision::<Value>(&transaction, table, id, to_revision)?;
    transaction.commit().map_err(storage_db_error)?;
    let mut changed_paths = BTreeSet::new();
    collect_changed_paths(Some(&from.value), Some(&to.value), "", &mut changed_paths);
    Ok(ContentModuleRevisionDiff {
        module_id: ContentModuleId::from(id),
        from_revision,
        to_revision,
        from_sha256: from.sha256,
        to_sha256: to.sha256,
        changed_paths: changed_paths.into_iter().collect(),
    })
}

pub(super) fn diff_prompt_preset_revision_documents(
    storage: &Storage,
    id: &PromptPresetId,
    from_revision: u64,
    to_revision: u64,
) -> CoreResult<PromptPresetRevisionDiff> {
    validate_identifier("prompt preset", id.as_str())?;
    if from_revision == to_revision {
        return Err(CoreError::invalid(
            "prompt preset diff requires two distinct revisions",
        ));
    }
    let mut connection = storage.connection()?;
    let transaction = connection.transaction().map_err(storage_db_error)?;
    let from = get_object_revision::<Value>(
        &transaction,
        DocumentTable::PromptPresets,
        id.as_str(),
        from_revision,
    )?;
    let to = get_object_revision::<Value>(
        &transaction,
        DocumentTable::PromptPresets,
        id.as_str(),
        to_revision,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    prompt_preset_diff_from_revisions(id, from, to)
}

pub(super) fn prompt_preset_diff_from_revisions(
    id: &PromptPresetId,
    from: ObjectRevision<Value>,
    to: ObjectRevision<Value>,
) -> CoreResult<PromptPresetRevisionDiff> {
    let mut changed_paths = BTreeSet::new();
    collect_changed_paths(Some(&from.value), Some(&to.value), "", &mut changed_paths);
    let changed_paths = changed_paths.into_iter().collect::<Vec<_>>();
    let diff_sha256 = sha256_hex(
        serde_json::to_string(&serde_json::json!({
            "schema_version": 1,
            "preset_id": id,
            "from_revision_id": from.revision_id,
            "from_revision": from.revision,
            "from_sha256": from.sha256,
            "to_revision_id": to.revision_id,
            "to_revision": to.revision,
            "to_sha256": to.sha256,
            "changed_paths": changed_paths,
        }))
        .map_err(|error| CoreError::internal(format!("cannot encode prompt preset diff: {error}")))?
        .as_bytes(),
    );
    Ok(PromptPresetRevisionDiff {
        preset_id: id.clone(),
        from_revision_id: from.revision_id,
        from_revision: from.revision,
        from_sha256: from.sha256,
        to_revision_id: to.revision_id,
        to_revision: to.revision,
        to_sha256: to.sha256,
        changed_paths,
        diff_sha256,
    })
}

struct CurrentRollbackState {
    state_version: i64,
    active_revision_id: String,
    document_json: String,
    created_at: String,
    deleted_at: Option<String>,
}

fn load_current_rollback_state(
    transaction: &Transaction<'_>,
    table: DocumentTable,
    id: &str,
) -> CoreResult<CurrentRollbackState> {
    transaction
        .query_row(
            "SELECT state.state_version, state.active_revision_id,
                    active.document_json, object.created_at, object.deleted_at
             FROM content_objects AS object
             JOIN content_object_state AS state
               ON state.object_id = object.id
             JOIN content_revisions AS active
               ON active.object_id = object.id
              AND active.id = state.active_revision_id
             WHERE object.id = ?1 AND object.object_kind = ?2",
            params![id, table.object_kind()],
            |row| {
                Ok(CurrentRollbackState {
                    state_version: row.get(0)?,
                    active_revision_id: row.get(1)?,
                    document_json: row.get(2)?,
                    created_at: row.get(3)?,
                    deleted_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found(table.object_kind()))
}

struct ContentRollbackPlan {
    id: String,
    diff_json: String,
    diff_sha256: String,
    plan_sha256: String,
    next_state_version: u64,
    applied_at: DateTime<Utc>,
    applied_at_text: String,
}

fn prepare_content_rollback_plan<T>(
    transaction: &Transaction<'_>,
    id: &str,
    expected_revision: u64,
    current: &CurrentRollbackState,
    target: &ObjectRevision<T>,
) -> CoreResult<ContentRollbackPlan>
where
    T: Serialize,
{
    let target_json = serde_json::to_string(&target.value)
        .map_err(|error| CoreError::internal(format!("cannot encode rollback target: {error}")))?;
    let diff_json = revision_diff_json(Some(&current.document_json), &target_json)?;
    let diff_sha256 = sha256_hex(diff_json.as_bytes());
    let plan_json = serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "object_id": id,
        "expected_state_version": expected_revision,
        "from_revision_id": current.active_revision_id,
        "target_revision_id": target.revision_id,
        "diff_sha256": diff_sha256,
    }))
    .map_err(|error| CoreError::internal(format!("cannot encode rollback plan: {error}")))?;
    let applied_at = Utc::now();
    let applied_at_text = applied_at.to_rfc3339();
    let plan = ContentRollbackPlan {
        id: Uuid::new_v4().to_string(),
        diff_json,
        diff_sha256,
        plan_sha256: sha256_hex(plan_json.as_bytes()),
        next_state_version: expected_revision
            .checked_add(1)
            .ok_or_else(|| CoreError::internal("content state revision overflow"))?,
        applied_at,
        applied_at_text,
    };
    insert_content_rollback_plan(transaction, id, current, target, &plan)?;
    Ok(plan)
}

fn insert_content_rollback_plan<T>(
    transaction: &Transaction<'_>,
    id: &str,
    current: &CurrentRollbackState,
    target: &ObjectRevision<T>,
    plan: &ContentRollbackPlan,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO content_rollback_plans
             (id, object_id, expected_active_revision_id, target_revision_id,
              diff_json, diff_sha256, plan_sha256, state, prepared_at,
              approved_at, applied_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'prepared', ?8, NULL, NULL)",
            params![
                plan.id,
                id,
                current.active_revision_id,
                target.revision_id,
                plan.diff_json,
                plan.diff_sha256,
                plan.plan_sha256,
                plan.applied_at_text,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn activate_content_rollback(
    transaction: &Transaction<'_>,
    table: DocumentTable,
    id: &str,
    expected_revision: u64,
    current: &CurrentRollbackState,
    target_revision_id: &str,
    plan: &ContentRollbackPlan,
) -> CoreResult<()> {
    let changed = transaction
        .execute(
            "UPDATE content_object_state
             SET active_revision_id = ?2, state_version = ?3, updated_at = ?4
             WHERE object_id = ?1
               AND active_revision_id = ?5
               AND state_version = ?6",
            params![
                id,
                target_revision_id,
                i64_revision(plan.next_state_version)?,
                plan.applied_at_text,
                current.active_revision_id,
                i64_revision(expected_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            table.object_kind(),
            id,
            Some(expected_revision),
            None,
        ));
    }
    Ok(())
}

fn record_content_rollback(
    transaction: &Transaction<'_>,
    id: &str,
    current_revision_id: &str,
    target_revision_id: &str,
    plan: &ContentRollbackPlan,
) -> CoreResult<()> {
    transaction
        .execute(
            "INSERT INTO content_revision_events
             (id, object_id, event_kind, from_revision_id, to_revision_id,
              diff_json, diff_sha256, plan_sha256, idempotency_key, created_at)
             VALUES (?1, ?2, 'rollback', ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                Uuid::new_v4().to_string(),
                id,
                current_revision_id,
                target_revision_id,
                plan.diff_json,
                plan.diff_sha256,
                plan.plan_sha256,
                Uuid::new_v4().to_string(),
                plan.applied_at_text,
            ],
        )
        .map_err(storage_db_error)?;
    transaction
        .execute(
            "UPDATE content_rollback_plans
             SET state = 'applied', approved_at = ?2, applied_at = ?2
             WHERE id = ?1 AND state = 'prepared'",
            params![plan.id, plan.applied_at_text],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(super) fn rollback_content_object<T>(
    storage: &Storage,
    table: DocumentTable,
    id: &str,
    target_revision: u64,
    expected_revision: u64,
) -> CoreResult<StoredRevision<T>>
where
    T: Serialize + DeserializeOwned,
{
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let current = load_current_rollback_state(&transaction, table, id)?;
    let actual_revision = u64_revision(current.state_version)?;
    if current.deleted_at.is_some() || actual_revision != expected_revision {
        return Err(revision_conflict(
            table.object_kind(),
            id,
            Some(expected_revision),
            Some(actual_revision),
        ));
    }
    let target = get_object_revision::<T>(&transaction, table, id, target_revision)?;
    if target.revision_id == current.active_revision_id {
        return Err(CoreError::invalid(
            "rollback target is already the active revision",
        ));
    }
    let plan =
        prepare_content_rollback_plan(&transaction, id, expected_revision, &current, &target)?;
    activate_content_rollback(
        &transaction,
        table,
        id,
        expected_revision,
        &current,
        &target.revision_id,
        &plan,
    )?;
    record_content_rollback(
        &transaction,
        id,
        &current.active_revision_id,
        &target.revision_id,
        &plan,
    )?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value: target.value,
        revision: plan.next_state_version,
        revision_id: Some(target.revision_id),
        created_at: parse_datetime("content object created_at", &current.created_at)?,
        updated_at: plan.applied_at,
        deleted_at: None,
    })
}

pub(super) fn load_exact_content_revision<T>(
    connection: &Connection,
    revision_id: &str,
    expected_kind: &str,
) -> CoreResult<ObjectRevision<T>>
where
    T: DeserializeOwned,
{
    let row = connection
        .query_row(
            "SELECT object_id, revision_no, document_json, document_sha256,
                    created_at
             FROM content_revisions
             WHERE id = ?1 AND object_kind = ?2",
            params![revision_id, expected_kind],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("exact module component revision"))?;
    if sha256_hex(row.2.as_bytes()) != row.3 {
        return Err(storage_corrupted(
            "immutable module component document hash is invalid",
        ));
    }
    Ok(ObjectRevision {
        revision_id: revision_id.to_owned(),
        object_kind: expected_kind.to_owned(),
        object_id: row.0,
        revision: u64_revision(row.1)?,
        value: decode_document(expected_kind, &row.2)?,
        sha256: row.3,
        created_at: parse_datetime("module component revision created_at", &row.4)?,
    })
}
