//! Prompt-preset binding contracts and compare-and-swap persistence.

use super::{
    ConversationId, CoreError, CoreResult, DateTime, Deserialize, GenerationPresetId,
    GenerationReasoningEffort, MAX_BLOCK_TEXT_CHARS, MAX_NAME_CHARS, ModuleScope,
    OptionalExtension, PromptPresetId, RawStoredDocument, Serialize, Storage, StoredRevision,
    TemplateSlot, Transaction, TransactionBehavior, Utc, VariableMap, decode_stored_document,
    encode_document, enum_wire, i64_revision, not_found, params, parse_datetime, revision_conflict,
    storage_db_error, u64_revision, validate_identifier,
};

/// Durable prompt-preset selection at one product scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptPresetBinding {
    pub id: String,
    pub prompt_preset_id: PromptPresetId,
    pub scope: ModuleScope,
    pub target_id: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<ConversationId>,
    #[serde(default)]
    pub pinned_revision_id: Option<String>,
    #[serde(default)]
    pub priority: i32,
    pub enabled: bool,
    #[serde(default)]
    pub response_length: PromptResponseLength,
    #[serde(default = "default_binding_creativity")]
    pub creativity: u8,
    #[serde(default)]
    pub reasoning_effort: Option<GenerationReasoningEffort>,
    #[serde(default = "default_enabled")]
    pub memory_enabled: bool,
    #[serde(default = "default_enabled")]
    pub knowledge_enabled: bool,
    #[serde(default)]
    pub variable_overrides: VariableMap,
    #[serde(default)]
    pub generation_preset_override_id: Option<GenerationPresetId>,
    /// Optional room-owned display name used when no exact persona is selected.
    /// Empty legacy values are omitted so existing canonical binding bytes and
    /// fingerprints remain stable after a decode/re-encode cycle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name_override: Option<String>,
    /// Bounded room-scoped author instruction materialized only by an
    /// `AuthorNote` prompt block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author_note: Option<String>,
    /// Bounded room-scoped participant and speaking context materialized only
    /// by a `GroupContext` prompt block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_context: Option<String>,
    /// Named, bounded room-owned template values. `block_content` remains a
    /// resolver-reserved slot and can never be persisted here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_slots: Vec<TemplateSlot>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl PromptPresetBinding {
    /// Returns the exact canonical JSON digest used by binding persistence.
    ///
    /// The digest is safe source evidence: it identifies the complete local
    /// binding document without copying prompt text into generation metadata.
    pub fn canonical_document_sha256(&self) -> CoreResult<String> {
        validate_prompt_binding_context(self)?;
        encode_document("prompt preset binding", self).map(|(_, sha256)| sha256)
    }
}

const MAX_PROMPT_BINDING_TEMPLATE_SLOTS: usize = 128;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptResponseLength {
    Short,
    #[default]
    Balanced,
    Long,
}

const fn default_binding_creativity() -> u8 {
    50
}

const fn default_enabled() -> bool {
    true
}

struct PromptBindingTargets<'a> {
    scope_kind: &'static str,
    persona_id: Option<&'a str>,
    character_id: Option<&'a str>,
    conversation_id: Option<&'a str>,
    branch_id: Option<&'a str>,
}

#[cfg(test)]
pub(super) struct PromptBindingTargetsForTest<'a> {
    pub(super) scope_kind: &'static str,
    pub(super) persona_id: Option<&'a str>,
    pub(super) character_id: Option<&'a str>,
    pub(super) conversation_id: Option<&'a str>,
    pub(super) branch_id: Option<&'a str>,
}

#[cfg(test)]
pub(super) fn prompt_binding_targets_for_test(
    binding: &PromptPresetBinding,
) -> CoreResult<PromptBindingTargetsForTest<'_>> {
    let targets = prompt_binding_targets(binding)?;
    Ok(PromptBindingTargetsForTest {
        scope_kind: targets.scope_kind,
        persona_id: targets.persona_id,
        character_id: targets.character_id,
        conversation_id: targets.conversation_id,
        branch_id: targets.branch_id,
    })
}

fn prompt_binding_targets(binding: &PromptPresetBinding) -> CoreResult<PromptBindingTargets<'_>> {
    let target = binding.target_id.as_deref();
    match binding.scope {
        ModuleScope::App if target.is_none() && binding.conversation_id.is_none() => {
            Ok(PromptBindingTargets {
                scope_kind: "app",
                persona_id: None,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::User if target.is_none() && binding.conversation_id.is_none() => {
            Ok(PromptBindingTargets {
                scope_kind: "user",
                persona_id: None,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Persona if target.is_some() && binding.conversation_id.is_none() => {
            Ok(PromptBindingTargets {
                scope_kind: "persona",
                persona_id: target,
                character_id: None,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Character if target.is_some() && binding.conversation_id.is_none() => {
            Ok(PromptBindingTargets {
                scope_kind: "character",
                persona_id: None,
                character_id: target,
                conversation_id: None,
                branch_id: None,
            })
        }
        ModuleScope::Conversation if target.is_some() && binding.conversation_id.is_none() => {
            Ok(PromptBindingTargets {
                scope_kind: "conversation",
                persona_id: None,
                character_id: None,
                conversation_id: target,
                branch_id: None,
            })
        }
        ModuleScope::Branch if target.is_some() && binding.conversation_id.as_ref().is_some() => {
            Ok(PromptBindingTargets {
                scope_kind: "branch",
                persona_id: None,
                character_id: None,
                conversation_id: binding
                    .conversation_id
                    .as_ref()
                    .map(|conversation_id| conversation_id.0.as_str()),
                branch_id: target,
            })
        }
        _ => Err(CoreError::invalid(
            "prompt preset binding scope and target are inconsistent",
        )),
    }
}

struct PromptBindingRevision {
    revision: u64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

fn next_prompt_binding_revision(
    transaction: &Transaction<'_>,
    binding_id: &str,
    expected_revision: Option<u64>,
) -> CoreResult<PromptBindingRevision> {
    let current = transaction
        .query_row(
            "SELECT revision, created_at, deleted_at
             FROM prompt_preset_bindings WHERE id = ?1",
            [binding_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?;
    let updated_at = Utc::now();
    let (revision, created_at) = match (expected_revision, current) {
        (None, None) => (1, updated_at),
        (None, Some((actual, _, _))) => {
            return Err(revision_conflict(
                "prompt preset binding",
                binding_id,
                None,
                Some(u64_revision(actual)?),
            ));
        }
        (Some(expected), Some((actual, created_at, deleted_at))) => {
            let actual = u64_revision(actual)?;
            if actual != expected || deleted_at.is_some() {
                return Err(revision_conflict(
                    "prompt preset binding",
                    binding_id,
                    Some(expected),
                    Some(actual),
                ));
            }
            (
                expected
                    .checked_add(1)
                    .ok_or_else(|| CoreError::internal("binding revision overflow"))?,
                parse_datetime("binding created_at", &created_at)?,
            )
        }
        (Some(expected), None) => {
            return Err(revision_conflict(
                "prompt preset binding",
                binding_id,
                Some(expected),
                None,
            ));
        }
    };
    Ok(PromptBindingRevision {
        revision,
        created_at,
        updated_at,
    })
}

const UPSERT_PROMPT_BINDING_SQL: &str = "INSERT INTO prompt_preset_bindings
     (id, prompt_preset_id, resolution_mode, pinned_revision_id,
      scope_kind, persona_id, character_id, conversation_id, branch_id,
      generation_preset_override_id, response_length, creativity,
      reasoning_effort, memory_enabled, knowledge_enabled,
      variable_overrides_json, priority, enabled, revision,
      document_json, created_at, updated_at, deleted_at)
     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
             ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
             NULL)
     ON CONFLICT(id) DO UPDATE SET
         prompt_preset_id = excluded.prompt_preset_id,
         resolution_mode = excluded.resolution_mode,
         pinned_revision_id = excluded.pinned_revision_id,
         scope_kind = excluded.scope_kind,
         persona_id = excluded.persona_id,
         character_id = excluded.character_id,
         conversation_id = excluded.conversation_id,
         branch_id = excluded.branch_id,
         generation_preset_override_id = excluded.generation_preset_override_id,
         response_length = excluded.response_length,
         creativity = excluded.creativity,
         reasoning_effort = excluded.reasoning_effort,
         memory_enabled = excluded.memory_enabled,
         knowledge_enabled = excluded.knowledge_enabled,
         variable_overrides_json = excluded.variable_overrides_json,
         priority = excluded.priority,
         enabled = excluded.enabled,
         revision = excluded.revision,
         document_json = excluded.document_json,
         updated_at = excluded.updated_at
     WHERE prompt_preset_bindings.revision = ?23
       AND prompt_preset_bindings.deleted_at IS NULL";

struct PromptBindingWrite<'a> {
    value: &'a PromptPresetBinding,
    targets: PromptBindingTargets<'a>,
    revision: &'a PromptBindingRevision,
    document_json: &'a str,
    variable_overrides_json: &'a str,
    expected_revision: Option<u64>,
}

fn write_prompt_binding(
    transaction: &Transaction<'_>,
    write: &PromptBindingWrite<'_>,
) -> CoreResult<()> {
    let expected_sql = write
        .expected_revision
        .map(i64_revision)
        .transpose()?
        .unwrap_or_default();
    let value = write.value;
    let changed = transaction
        .execute(
            UPSERT_PROMPT_BINDING_SQL,
            params![
                value.id,
                value.prompt_preset_id.as_str(),
                if value.pinned_revision_id.is_some() {
                    "pinned"
                } else {
                    "active"
                },
                value.pinned_revision_id,
                write.targets.scope_kind,
                write.targets.persona_id,
                write.targets.character_id,
                write.targets.conversation_id,
                write.targets.branch_id,
                value
                    .generation_preset_override_id
                    .as_ref()
                    .map(GenerationPresetId::as_str),
                enum_wire(&value.response_length)?,
                value.creativity,
                value
                    .reasoning_effort
                    .as_ref()
                    .map(enum_wire)
                    .transpose()?
                    .unwrap_or_else(|| "provider_default".to_owned()),
                value.memory_enabled,
                value.knowledge_enabled,
                write.variable_overrides_json,
                value.priority,
                value.enabled,
                i64_revision(write.revision.revision)?,
                write.document_json,
                write.revision.created_at.to_rfc3339(),
                write.revision.updated_at.to_rfc3339(),
                expected_sql,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "prompt preset binding",
            &value.id,
            write.expected_revision,
            None,
        ));
    }
    Ok(())
}

fn save_prompt_binding(
    storage: &Storage,
    binding: &PromptPresetBinding,
    expected_revision: Option<u64>,
) -> CoreResult<StoredRevision<PromptPresetBinding>> {
    validate_identifier("prompt preset binding", &binding.id)?;
    validate_prompt_binding_context(binding)?;
    if binding.creativity > 100 {
        return Err(CoreError::invalid(
            "prompt binding creativity must be between 0 and 100",
        ));
    }
    let targets = prompt_binding_targets(binding)?;
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let revision = next_prompt_binding_revision(&transaction, &binding.id, expected_revision)?;
    let mut value = binding.clone();
    value.created_at = revision.created_at;
    value.updated_at = revision.updated_at;
    let (document_json, _) = encode_document("prompt preset binding", &value)?;
    let variable_overrides_json = serde_json::to_string(&value.variable_overrides)
        .map_err(|error| CoreError::invalid(format!("cannot encode binding variables: {error}")))?;
    write_prompt_binding(
        &transaction,
        &PromptBindingWrite {
            value: &value,
            targets,
            revision: &revision,
            document_json: &document_json,
            variable_overrides_json: &variable_overrides_json,
            expected_revision,
        },
    )?;
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value,
        revision: revision.revision,
        revision_id: None,
        created_at: revision.created_at,
        updated_at: revision.updated_at,
        deleted_at: None,
    })
}

fn validate_prompt_binding_context(binding: &PromptPresetBinding) -> CoreResult<()> {
    validate_prompt_binding_optional_text(
        "prompt binding user name",
        binding.user_name_override.as_deref(),
        MAX_NAME_CHARS,
        true,
    )?;
    validate_prompt_binding_optional_text(
        "prompt binding author note",
        binding.author_note.as_deref(),
        MAX_BLOCK_TEXT_CHARS,
        false,
    )?;
    validate_prompt_binding_optional_text(
        "prompt binding group context",
        binding.group_context.as_deref(),
        MAX_BLOCK_TEXT_CHARS,
        false,
    )?;
    if binding.template_slots.len() > MAX_PROMPT_BINDING_TEMPLATE_SLOTS {
        return Err(CoreError::invalid(format!(
            "prompt binding must contain at most {MAX_PROMPT_BINDING_TEMPLATE_SLOTS} template slots"
        )));
    }
    let mut names = Vec::with_capacity(binding.template_slots.len());
    for slot in &binding.template_slots {
        validate_prompt_binding_slot(slot)?;
        names.push(slot.name.as_str());
    }
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(CoreError::invalid(
            "prompt binding template slot names must be unique",
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn validate_prompt_binding_context_for_test(
    binding: &PromptPresetBinding,
) -> CoreResult<()> {
    validate_prompt_binding_context(binding)
}

fn validate_prompt_binding_optional_text(
    label: &str,
    value: Option<&str>,
    maximum_chars: usize,
    require_trimmed: bool,
) -> CoreResult<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let chars = value.chars().count();
    if chars == 0
        || chars > maximum_chars
        || value.trim().is_empty()
        || value.contains('\0')
        || (require_trimmed && value.trim() != value)
    {
        return Err(CoreError::invalid(format!(
            "{label} is empty, oversized, invalidly padded, or contains NUL"
        )));
    }
    Ok(())
}

fn validate_prompt_binding_slot(slot: &TemplateSlot) -> CoreResult<()> {
    let name_chars = slot.name.chars().count();
    if name_chars == 0
        || name_chars > MAX_NAME_CHARS
        || slot.name.trim() != slot.name
        || slot.name.chars().any(char::is_control)
        || slot.name == "block_content"
    {
        return Err(CoreError::invalid(
            "prompt binding template slot name is invalid or reserved",
        ));
    }
    if slot.value.chars().count() > MAX_BLOCK_TEXT_CHARS || slot.value.contains('\0') {
        return Err(CoreError::invalid(
            "prompt binding template slot value is oversized or contains NUL",
        ));
    }
    Ok(())
}

fn list_prompt_bindings(
    storage: &Storage,
    scope: ModuleScope,
    target_id: Option<&str>,
) -> CoreResult<Vec<StoredRevision<PromptPresetBinding>>> {
    let (scope_kind, target_clause) = match scope {
        ModuleScope::App if target_id.is_none() => ("app", "1 = 1"),
        ModuleScope::User if target_id.is_none() => ("user", "1 = 1"),
        ModuleScope::Persona if target_id.is_some() => ("persona", "persona_id = ?2"),
        ModuleScope::Character if target_id.is_some() => ("character", "character_id = ?2"),
        ModuleScope::Conversation if target_id.is_some() => {
            ("conversation", "conversation_id = ?2")
        }
        ModuleScope::Branch if target_id.is_some() => ("branch", "branch_id = ?2"),
        _ => {
            return Err(CoreError::invalid(
                "prompt binding list scope requires a compatible target",
            ));
        }
    };
    let sql = format!(
        "SELECT document_json, revision, created_at, updated_at, deleted_at
         FROM prompt_preset_bindings
         WHERE scope_kind = ?1 AND {target_clause} AND deleted_at IS NULL
         ORDER BY priority DESC, id"
    );
    let connection = storage.connection()?;
    let mut statement = connection.prepare(&sql).map_err(storage_db_error)?;
    let rows = if let Some(target_id) = target_id {
        statement
            .query_map(params![scope_kind, target_id], prompt_binding_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    } else {
        statement
            .query_map([scope_kind], prompt_binding_row)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    rows.into_iter()
        .map(|row| decode_stored_document("prompt preset binding", row))
        .collect()
}

fn prompt_binding_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawStoredDocument> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, i64>(1)?,
        None,
        row.get::<_, String>(2)?,
        row.get::<_, String>(3)?,
        row.get::<_, Option<String>>(4)?,
    ))
}

fn soft_delete_prompt_binding(
    storage: &Storage,
    id: &str,
    expected_revision: u64,
) -> CoreResult<StoredRevision<PromptPresetBinding>> {
    let mut connection = storage.connection()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let row = transaction
        .query_row(
            "SELECT document_json, revision, created_at, updated_at, deleted_at
             FROM prompt_preset_bindings WHERE id = ?1",
            [id],
            prompt_binding_row,
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("prompt preset binding"))?;
    let current = decode_stored_document::<PromptPresetBinding>("prompt preset binding", row)?;
    if current.deleted_at.is_some() || current.revision != expected_revision {
        return Err(revision_conflict(
            "prompt preset binding",
            id,
            Some(expected_revision),
            Some(current.revision),
        ));
    }
    let next_revision = expected_revision
        .checked_add(1)
        .ok_or_else(|| CoreError::internal("binding revision overflow"))?;
    let now = Utc::now();
    let changed = transaction
        .execute(
            "UPDATE prompt_preset_bindings
             SET revision = ?2, updated_at = ?3, deleted_at = ?3
             WHERE id = ?1 AND revision = ?4 AND deleted_at IS NULL",
            params![
                id,
                i64_revision(next_revision)?,
                now.to_rfc3339(),
                i64_revision(expected_revision)?,
            ],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(revision_conflict(
            "prompt preset binding",
            id,
            Some(expected_revision),
            None,
        ));
    }
    transaction.commit().map_err(storage_db_error)?;
    Ok(StoredRevision {
        value: current.value,
        revision: next_revision,
        revision_id: None,
        created_at: current.created_at,
        updated_at: now,
        deleted_at: Some(now),
    })
}

impl Storage {
    pub fn save_prompt_preset_binding(
        &self,
        binding: &PromptPresetBinding,
        expected_revision: Option<u64>,
    ) -> CoreResult<StoredRevision<PromptPresetBinding>> {
        save_prompt_binding(self, binding, expected_revision)
    }

    pub fn list_prompt_preset_bindings(
        &self,
        scope: ModuleScope,
        target_id: Option<&str>,
    ) -> CoreResult<Vec<StoredRevision<PromptPresetBinding>>> {
        list_prompt_bindings(self, scope, target_id)
    }

    pub fn soft_delete_prompt_preset_binding(
        &self,
        id: &str,
        expected_revision: u64,
    ) -> CoreResult<StoredRevision<PromptPresetBinding>> {
        soft_delete_prompt_binding(self, id, expected_revision)
    }
}
