//! Prompt-preset relational projection and immutable dependency capture.

use super::{
    BTreeMap, BTreeSet, ContentModuleId, CoreError, CoreResult, GenerationPresetId,
    OptionalExtension, OverflowPolicy, PlacementZone, PromptBlockKind, PromptPreset, RoleHint,
    Transaction, Utc, content_revision_number, encode_document, enum_wire, i64_revision, params,
    source_kind_str, storage_corrupted, storage_db_error, usize_to_i64, validate_identifier,
    variable_storage_key, variable_value_type,
};

fn write_prompt_preset_header(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    let (provenance_json, _) =
        encode_document("prompt preset provenance", &preset.metadata.provenance)?;
    let state_version = expected_revision.map_or(1, |value| value.saturating_add(1));
    let now = Utc::now().to_rfc3339();
    transaction
        .execute(
            "INSERT INTO prompt_presets
             (id, name, schema_version, revision, default_generation_preset_id,
              document_json, provenance_json, source_kind, source_hash,
              created_at, updated_at, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10, NULL)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name,
                 schema_version = excluded.schema_version,
                 revision = excluded.revision,
                 default_generation_preset_id =
                     excluded.default_generation_preset_id,
                 document_json = excluded.document_json,
                 provenance_json = excluded.provenance_json,
                 source_kind = excluded.source_kind,
                 source_hash = excluded.source_hash,
                 updated_at = excluded.updated_at",
            params![
                preset.id.as_str(),
                preset.name,
                preset.schema_version,
                i64_revision(state_version)?,
                preset
                    .default_generation_preset_id
                    .as_ref()
                    .map(GenerationPresetId::as_str),
                document_json,
                provenance_json,
                source_kind_str(&preset.metadata.provenance.source_kind),
                preset.metadata.provenance.source_hash,
                now,
            ],
        )
        .map_err(storage_db_error)?;
    let revision_no = content_revision_number(transaction, revision_id)?;
    let metadata_json = serde_json::to_string(&preset.metadata)
        .map_err(|error| CoreError::invalid(format!("cannot encode preset metadata: {error}")))?;
    transaction
        .execute(
            "INSERT INTO prompt_preset_revisions
             (revision_id, prompt_preset_id, revision_no, name,
              default_generation_preset_id, metadata_json, document_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                revision_id,
                preset.id.as_str(),
                i64_revision(revision_no)?,
                preset.name,
                preset
                    .default_generation_preset_id
                    .as_ref()
                    .map(GenerationPresetId::as_str),
                metadata_json,
                document_json,
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn prompt_preset_variables(
    preset: &PromptPreset,
) -> CoreResult<BTreeMap<lorepia_domain::VariableRef, lorepia_domain::VariableValue>> {
    let mut variables = BTreeMap::new();
    for binding in &preset.default_values.values {
        variables.insert(binding.variable.clone(), binding.value.clone());
    }
    for control in &preset.controls {
        if let (Some(variable), Some(value)) = (&control.variable, &control.default_value)
            && variables
                .insert(variable.clone(), value.clone())
                .is_some_and(|existing| existing != *value)
        {
            return Err(CoreError::invalid(format!(
                "prompt control {} conflicts with the preset default value",
                control.id.as_str()
            )));
        }
    }
    Ok(variables)
}

fn write_prompt_preset_variables(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
    variables: &BTreeMap<lorepia_domain::VariableRef, lorepia_domain::VariableValue>,
) -> CoreResult<()> {
    for (variable, value) in variables {
        let variable_key = variable_storage_key(variable);
        let sensitive = preset
            .controls
            .iter()
            .any(|control| control.variable.as_ref() == Some(variable) && control.sensitive);
        let value_json = serde_json::to_string(value).map_err(|error| {
            CoreError::invalid(format!("cannot encode prompt variable value: {error}"))
        })?;
        let payload_json = serde_json::to_string(&serde_json::json!({
            "variable": variable,
            "default_value": value,
        }))
        .map_err(|error| CoreError::invalid(format!("cannot encode prompt variable: {error}")))?;
        transaction
            .execute(
                "INSERT INTO prompt_variables
                 (owner_revision_id, variable_key, value_type, scope, namespace,
                  default_value_json, sensitive, payload_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    revision_id,
                    variable_key,
                    variable_value_type(value),
                    enum_wire(&variable.scope)?,
                    variable.namespace.as_ref().map(ContentModuleId::as_str),
                    value_json,
                    sensitive,
                    payload_json,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_prompt_preset_controls(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
) -> CoreResult<()> {
    for (ordinal, control) in preset.controls.iter().enumerate() {
        let (control_json, _) = encode_document("prompt control", control)?;
        let variable_key = control.variable.as_ref().map(variable_storage_key);
        let options_json = serde_json::to_string(&control.options).map_err(|error| {
            CoreError::invalid(format!("cannot encode prompt control options: {error}"))
        })?;
        let visibility_json = control
            .visible_when
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::invalid(format!("cannot encode control visibility: {error}"))
            })?;
        transaction
            .execute(
                "INSERT INTO prompt_controls
                 (owner_revision_id, control_id, ordinal, kind, variable_key,
                  label, description, options_json, minimum, maximum, step,
                  visibility_condition_json, regenerate_required, document_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                         ?12, ?13, ?14)",
                params![
                    revision_id,
                    control.id.as_str(),
                    usize_to_i64(ordinal, "prompt control ordinal")?,
                    enum_wire(&control.kind)?,
                    variable_key,
                    control.label,
                    control.description,
                    options_json,
                    control.minimum,
                    control.maximum,
                    control.step,
                    visibility_json,
                    control.requires_regeneration,
                    control_json,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_prompt_preset_blocks(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
) -> CoreResult<()> {
    for (ordinal, block) in preset.blocks.iter().enumerate() {
        let (block_json, _) = encode_document("prompt block", block)?;
        let template_json = block
            .template
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::invalid(format!("cannot encode block template: {error}"))
            })?;
        let condition_json = block
            .condition
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::invalid(format!("cannot encode block condition: {error}"))
            })?;
        let source_json = serde_json::to_string(&block.source)
            .map_err(|error| CoreError::invalid(format!("cannot encode block source: {error}")))?;
        let history_json = block
            .history_selector
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| {
                CoreError::invalid(format!("cannot encode history selector: {error}"))
            })?;
        let provenance_json = serde_json::to_string(&block.provenance).map_err(|error| {
            CoreError::invalid(format!("cannot encode block provenance: {error}"))
        })?;
        transaction
            .execute(
                "INSERT INTO prompt_blocks
                 (owner_revision_id, block_id, ordinal, name, kind, enabled,
                  authority, role_hint, template_json, condition_json,
                  source_json, placement_zone, history_selector_json,
                  token_priority, min_tokens, max_tokens, reserve_tokens,
                  overflow_policy, merge_policy, provenance_json, document_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
                         ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
                params![
                    revision_id,
                    block.id.as_str(),
                    usize_to_i64(ordinal, "prompt block ordinal")?,
                    block.name,
                    enum_wire(&block.kind)?,
                    block.enabled,
                    enum_wire(&block.authority)?,
                    enum_wire(&block.role_hint)?,
                    template_json,
                    condition_json,
                    source_json,
                    enum_wire(&block.placement_zone)?,
                    history_json,
                    block.token_policy.priority,
                    block.token_policy.min_tokens,
                    block.token_policy.max_tokens,
                    block.token_policy.reserve_tokens,
                    enum_wire(&block.overflow_policy)?,
                    enum_wire(&block.merge_policy)?,
                    provenance_json,
                    block_json,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_prompt_preset_cache_boundaries(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
) -> CoreResult<()> {
    for (ordinal, boundary) in preset.cache_boundaries.iter().enumerate() {
        let (role_filter, exact_role) = match boundary.role_filter {
            lorepia_domain::CacheRoleFilter::All => ("all", None),
            lorepia_domain::CacheRoleFilter::SystemLike => ("system_like", None),
            lorepia_domain::CacheRoleFilter::ExactRole { role } => {
                if role == RoleHint::ProviderDefault {
                    return Err(CoreError::invalid(
                        "an exact-role cache boundary cannot target provider_default",
                    ));
                }
                ("exact_role", Some(enum_wire(&role)?))
            }
        };
        let (boundary_json, _) = encode_document("prompt cache boundary", boundary)?;
        transaction
            .execute(
                "INSERT INTO prompt_cache_boundaries
                 (owner_revision_id, id, after_block_id, ordinal, role_filter,
                  exact_role, ttl, ttl_seconds, mode, document_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9)",
                params![
                    revision_id,
                    boundary.id.as_str(),
                    boundary.after_block_id.as_str(),
                    usize_to_i64(ordinal, "cache boundary ordinal")?,
                    role_filter,
                    exact_role,
                    enum_wire(&boundary.ttl)?,
                    enum_wire(&boundary.mode)?,
                    boundary_json,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_prompt_preset_knowledge_books(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
) -> CoreResult<()> {
    let mut knowledge_book_ids = BTreeSet::new();
    for (ordinal, book_id) in preset.knowledge_book_ids.iter().enumerate() {
        if !knowledge_book_ids.insert(book_id.as_str()) {
            return Err(CoreError::invalid(
                "prompt preset knowledge book ids must be unique",
            ));
        }
        let book_revision_id = active_projection_revision_id(
            transaction,
            book_id.as_str(),
            "knowledge_book",
            "knowledge_books",
            "knowledge_book_revisions",
            "knowledge_book_id",
        )?;
        transaction
            .execute(
                "INSERT INTO prompt_preset_knowledge_books
                 (prompt_preset_revision_id, ordinal,
                  knowledge_book_revision_id, enabled, config_json)
                 VALUES (?1, ?2, ?3, 1, '{}')",
                params![
                    revision_id,
                    usize_to_i64(ordinal, "prompt preset knowledge ordinal")?,
                    book_revision_id,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_prompt_preset_transform_sets(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
) -> CoreResult<()> {
    let mut transform_set_ids = BTreeSet::new();
    for (ordinal, transform_set_id) in preset.transform_set_ids.iter().enumerate() {
        if !transform_set_ids.insert(transform_set_id.as_str()) {
            return Err(CoreError::invalid(
                "prompt preset transform set ids must be unique",
            ));
        }
        let transform_revision_id = active_projection_revision_id(
            transaction,
            transform_set_id.as_str(),
            "transform_set",
            "transform_sets",
            "transform_set_revisions",
            "transform_set_id",
        )?;
        transaction
            .execute(
                "INSERT INTO prompt_preset_transform_sets
                 (prompt_preset_revision_id, ordinal,
                  transform_set_revision_id, enabled, config_json)
                 VALUES (?1, ?2, ?3, 1, '{}')",
                params![
                    revision_id,
                    usize_to_i64(ordinal, "prompt preset transform ordinal")?,
                    transform_revision_id,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_prompt_preset_memory_profile(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
) -> CoreResult<()> {
    if let Some(memory_profile_id) = &preset.memory_profile_id {
        let memory_revision_id = active_projection_revision_id(
            transaction,
            memory_profile_id.as_str(),
            "memory_profile",
            "memory_profiles",
            "memory_profile_revisions",
            "memory_profile_id",
        )?;
        transaction
            .execute(
                "INSERT INTO prompt_preset_memory_profiles
                 (prompt_preset_revision_id, memory_profile_revision_id,
                  enabled, config_json)
                 VALUES (?1, ?2, 1, '{}')",
                params![revision_id, memory_revision_id],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

fn write_prompt_preset_modules(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
) -> CoreResult<()> {
    let mut module_ids = BTreeSet::new();
    for (ordinal, module_id) in preset.module_ids.iter().enumerate() {
        if !module_ids.insert(module_id.as_str()) {
            return Err(CoreError::invalid(
                "prompt preset module ids must be unique",
            ));
        }
        let resolved = transaction
            .query_row(
                "SELECT state.active_revision_id, revision.source_hash
                 FROM content_objects AS object
                 JOIN content_object_state AS state
                   ON state.object_id = object.id
                 JOIN content_modules AS module
                   ON module.id = object.id
                  AND module.deleted_at IS NULL
                 JOIN content_module_revisions AS revision
                   ON revision.module_id = object.id
                  AND revision.revision_id = state.active_revision_id
                 WHERE object.id = ?1
                   AND object.object_kind = 'content_module'
                   AND object.deleted_at IS NULL",
                [module_id.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "prompt preset references missing or deleted content module {}",
                    module_id.as_str()
                ))
            })?;
        lorepia_domain::Sha256Digest::parse(resolved.1.clone()).map_err(|error| {
            storage_corrupted(format!(
                "active content module has invalid source hash: {error}"
            ))
        })?;
        transaction
            .execute(
                "INSERT INTO prompt_preset_modules
                 (prompt_preset_revision_id, ordinal, module_id,
                  module_revision_id, source_sha256, enabled, config_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, '{}')",
                params![
                    revision_id,
                    usize_to_i64(ordinal, "prompt preset module ordinal")?,
                    module_id.as_str(),
                    resolved.0,
                    resolved.1,
                ],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

pub(super) fn write_prompt_preset_projection(
    transaction: &Transaction<'_>,
    revision_id: &str,
    preset: &PromptPreset,
    document_json: &str,
    expected_revision: Option<u64>,
) -> CoreResult<()> {
    validate_prompt_preset_storage_shape(preset)?;
    write_prompt_preset_header(
        transaction,
        revision_id,
        preset,
        document_json,
        expected_revision,
    )?;
    let variables = prompt_preset_variables(preset)?;
    write_prompt_preset_variables(transaction, revision_id, preset, &variables)?;
    write_prompt_preset_controls(transaction, revision_id, preset)?;
    write_prompt_preset_blocks(transaction, revision_id, preset)?;
    write_prompt_preset_cache_boundaries(transaction, revision_id, preset)?;
    write_prompt_preset_knowledge_books(transaction, revision_id, preset)?;
    write_prompt_preset_transform_sets(transaction, revision_id, preset)?;
    write_prompt_preset_memory_profile(transaction, revision_id, preset)?;
    write_prompt_preset_modules(transaction, revision_id, preset)
}

pub(super) fn validate_prompt_preset_storage_shape(preset: &PromptPreset) -> CoreResult<()> {
    validate_identifier("prompt preset", preset.id.as_str())?;
    if preset.name.trim().is_empty() || preset.schema_version == 0 {
        return Err(CoreError::invalid(
            "prompt preset name and schema version are required",
        ));
    }
    let mut block_ids = BTreeSet::new();
    let mut latest_user_count = 0_usize;
    let mut application_policy_count = 0_usize;
    for block in &preset.blocks {
        if !block_ids.insert(block.id.as_str()) {
            return Err(CoreError::invalid("prompt block ids must be unique"));
        }
        if block.kind == PromptBlockKind::LatestUserTurn {
            latest_user_count += 1;
        }
        if block.placement_zone == PlacementZone::ApplicationPolicy {
            application_policy_count += 1;
            if block.role_hint != RoleHint::System
                || block.authority != lorepia_domain::InstructionAuthority::Application
                || block.overflow_policy != OverflowPolicy::Reject
            {
                return Err(CoreError::invalid(
                    "application policy blocks must be application-owned, system-role, and non-droppable",
                ));
            }
        }
    }
    if latest_user_count != 1 || application_policy_count == 0 {
        return Err(CoreError::invalid(
            "prompt preset requires exactly one latest-user block and an application policy",
        ));
    }
    let block_ids = block_ids.into_iter().collect::<BTreeSet<_>>();
    if preset
        .cache_boundaries
        .iter()
        .any(|boundary| !block_ids.contains(boundary.after_block_id.as_str()))
    {
        return Err(CoreError::invalid(
            "prompt cache boundary references an unknown block",
        ));
    }
    let mut module_ids = BTreeSet::new();
    if preset
        .module_ids
        .iter()
        .any(|module_id| !module_ids.insert(module_id.as_str()))
    {
        return Err(CoreError::invalid(
            "prompt preset module ids must be unique",
        ));
    }
    let mut knowledge_book_ids = BTreeSet::new();
    if preset
        .knowledge_book_ids
        .iter()
        .any(|id| !knowledge_book_ids.insert(id.as_str()))
    {
        return Err(CoreError::invalid(
            "prompt preset knowledge book ids must be unique",
        ));
    }
    let mut transform_set_ids = BTreeSet::new();
    if preset
        .transform_set_ids
        .iter()
        .any(|id| !transform_set_ids.insert(id.as_str()))
    {
        return Err(CoreError::invalid(
            "prompt preset transform set ids must be unique",
        ));
    }
    Ok(())
}

fn active_projection_revision_id(
    transaction: &Transaction<'_>,
    object_id: &str,
    object_kind: &str,
    current_table: &str,
    revision_table: &str,
    revision_object_column: &str,
) -> CoreResult<String> {
    let (current_table, revision_table, revision_object_column) =
        match (current_table, revision_table, revision_object_column) {
            ("knowledge_books", "knowledge_book_revisions", "knowledge_book_id") => (
                "knowledge_books",
                "knowledge_book_revisions",
                "knowledge_book_id",
            ),
            ("transform_sets", "transform_set_revisions", "transform_set_id") => (
                "transform_sets",
                "transform_set_revisions",
                "transform_set_id",
            ),
            ("memory_profiles", "memory_profile_revisions", "memory_profile_id") => (
                "memory_profiles",
                "memory_profile_revisions",
                "memory_profile_id",
            ),
            _ => {
                return Err(CoreError::internal(
                    "unsupported prompt preset dependency kind",
                ));
            }
        };
    let sql = format!(
        "SELECT state.active_revision_id
         FROM content_objects AS object
         JOIN content_object_state AS state
           ON state.object_id = object.id
         JOIN {current_table} AS current
           ON current.id = object.id
          AND current.deleted_at IS NULL
         JOIN {revision_table} AS revision
           ON revision.{revision_object_column} = object.id
          AND revision.revision_id = state.active_revision_id
         WHERE object.id = ?1
           AND object.object_kind = ?2
           AND object.deleted_at IS NULL"
    );
    transaction
        .query_row(&sql, params![object_id, object_kind], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| {
            CoreError::invalid(format!(
                "prompt preset references missing or deleted {object_kind} {object_id}"
            ))
        })
}
