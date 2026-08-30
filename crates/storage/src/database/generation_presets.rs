use super::{
    CoreError, CoreResult, GenerationPreset, GenerationPresetId, GenerationPromptCacheSettings,
    GenerationReasoningSettings, ModelRouteId, OptionalExtension, ParameterLiteral, ParameterSpec,
    ParameterType, ParameterValue, ParameterValueState, ProviderTemplate, Storage,
    active_retained_legacy_profile_exists, clear_provider_selections_for_preset,
    current_migrated_legacy_route_exists, not_found, params, parse_stored_datetime,
    storage_corrupted, storage_db_error, stored_catalog_error, validate_nonempty,
};

impl Storage {
    pub fn list_generation_presets(
        &self,
        model_route_id: &ModelRouteId,
    ) -> CoreResult<Vec<GenerationPreset>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, model_route_id, display_name, values_json, created_at, updated_at
                 FROM generation_presets WHERE model_route_id = ?1
                 ORDER BY display_name COLLATE NOCASE, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([model_route_id.as_str()], generation_preset_columns)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter().map(decode_generation_preset_row).collect()
    }

    pub fn get_generation_preset(&self, id: &GenerationPresetId) -> CoreResult<GenerationPreset> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, model_route_id, display_name, values_json, created_at, updated_at
                 FROM generation_presets WHERE id = ?1",
                [id.as_str()],
                generation_preset_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation preset"))?;
        decode_generation_preset_row(row)
    }

    pub fn save_generation_preset(&self, preset: &GenerationPreset) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let connection_id = transaction
            .query_row(
                "SELECT connection_id FROM provider_models WHERE id = ?1",
                [preset.model_route_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("model route"))?;
        if active_retained_legacy_profile_exists(&transaction, &connection_id)? {
            return Err(CoreError::invalid(
                "migrated legacy generation presets are managed through their retained provider profile",
            ));
        }
        validate_generation_preset(&transaction, preset)?;
        upsert_generation_preset_row(&transaction, preset)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn delete_generation_preset(&self, id: &GenerationPresetId) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let route_id = transaction
            .query_row(
                "SELECT model_route_id FROM generation_presets WHERE id = ?1",
                [id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("generation preset"))?;
        if id.as_str() == route_id && current_migrated_legacy_route_exists(&transaction, &route_id)?
        {
            return Err(CoreError::invalid(
                "the migrated legacy current preset cannot be deleted independently",
            ));
        }
        clear_provider_selections_for_preset(&transaction, id.as_str(), &route_id)?;
        transaction
            .execute(
                "DELETE FROM generation_presets WHERE id = ?1",
                [id.as_str()],
            )
            .map_err(storage_db_error)?;
        transaction.commit().map_err(storage_db_error)
    }
}
pub(super) type GenerationPresetRow = (String, String, String, String, String, String);

fn validate_generation_preset(
    transaction: &rusqlite::Transaction<'_>,
    preset: &GenerationPreset,
) -> CoreResult<()> {
    validate_generation_preset_for_schema(transaction, preset, true)
}

pub(super) fn validate_generation_preset_for_schema(
    transaction: &rusqlite::Transaction<'_>,
    preset: &GenerationPreset,
    require_active_connection: bool,
) -> CoreResult<()> {
    validate_nonempty("generation preset id", preset.id.as_str())?;
    validate_nonempty("generation preset display name", &preset.display_name)?;
    let template_query = if require_active_connection {
        "SELECT template.manifest_json
         FROM provider_models AS model
         JOIN provider_connections AS connection
           ON connection.id = model.connection_id
         JOIN provider_templates AS template
           ON template.id = connection.template_id
          AND template.version = connection.template_version
         WHERE model.id = ?1
           AND connection.archived_at IS NULL"
    } else {
        "SELECT template.manifest_json
         FROM provider_models AS model
         JOIN provider_connections AS connection
           ON connection.id = model.connection_id
         JOIN provider_templates AS template
           ON template.id = connection.template_id
          AND template.version = connection.template_version
         WHERE model.id = ?1"
    };
    let template_json = transaction
        .query_row(template_query, [preset.model_route_id.as_str()], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("model route"))?;
    let template = serde_json::from_str::<ProviderTemplate>(&template_json).map_err(|error| {
        storage_corrupted(format!("stored provider template is invalid: {error}"))
    })?;
    let mut ids = std::collections::BTreeSet::new();
    for value in &preset.values {
        if !ids.insert(value.parameter_id.as_str()) {
            return Err(CoreError::invalid(
                "generation preset parameter identifiers must be unique",
            ));
        }
        let specification = template
            .default_manifest
            .parameters
            .iter()
            .find(|specification| specification.id == value.parameter_id)
            .ok_or_else(|| {
                CoreError::invalid(format!(
                    "generation preset references unknown parameter {}",
                    value.parameter_id
                ))
            })?;
        validate_parameter_value(specification, &value.state)?;
    }
    Ok(())
}

pub(super) fn validate_parameter_value(
    specification: &ParameterSpec,
    state: &ParameterValueState,
) -> CoreResult<()> {
    let ParameterValueState::Explicit(literal) = state else {
        return Ok(());
    };
    let type_matches = matches!(
        (specification.value_type, literal),
        (ParameterType::Boolean, ParameterLiteral::Boolean(_))
            | (ParameterType::Integer, ParameterLiteral::Integer(_))
            | (ParameterType::Number, ParameterLiteral::Number(_))
            | (ParameterType::String, ParameterLiteral::String(_))
            | (ParameterType::Enum, ParameterLiteral::Enum(_))
            | (ParameterType::StringList, ParameterLiteral::StringList(_))
            | (ParameterType::JsonSchema, ParameterLiteral::JsonSchema(_))
            | (
                ParameterType::StopSequenceList,
                ParameterLiteral::StopSequenceList(_)
            )
            | (ParameterType::ToolPolicy, ParameterLiteral::ToolPolicy(_))
    );
    if !type_matches {
        return Err(CoreError::invalid(format!(
            "generation preset parameter {} has the wrong value type",
            specification.id
        )));
    }
    let numeric_value = match literal {
        ParameterLiteral::Integer(value) => {
            Some(value.to_string().parse::<f64>().map_err(|error| {
                CoreError::internal(format!(
                    "cannot validate generation preset integer value: {error}"
                ))
            })?)
        }
        ParameterLiteral::Number(value) if value.is_finite() => Some(*value),
        ParameterLiteral::Number(_) => {
            return Err(CoreError::invalid(
                "generation preset numeric values must be finite",
            ));
        }
        _ => None,
    };
    if let Some(value) = numeric_value
        && (specification.minimum.is_some_and(|minimum| value < minimum)
            || specification.maximum.is_some_and(|maximum| value > maximum))
    {
        return Err(CoreError::invalid(format!(
            "generation preset parameter {} is outside its allowed range",
            specification.id
        )));
    }
    if !specification.allowed_values.is_empty()
        && !specification
            .allowed_values
            .iter()
            .any(|choice| choice.value == *literal)
    {
        return Err(CoreError::invalid(format!(
            "generation preset parameter {} is not an allowed value",
            specification.id
        )));
    }
    Ok(())
}

pub(crate) fn upsert_generation_preset_row(
    transaction: &rusqlite::Transaction<'_>,
    preset: &GenerationPreset,
) -> CoreResult<()> {
    validate_generation_preset(transaction, preset)?;
    if preset.updated_at < preset.created_at {
        return Err(CoreError::invalid(
            "generation preset updated_at must not precede created_at",
        ));
    }
    let existing_route = transaction
        .query_row(
            "SELECT model_route_id FROM generation_presets WHERE id = ?1",
            [preset.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if existing_route
        .as_deref()
        .is_some_and(|id| id != preset.model_route_id.as_str())
    {
        return Err(CoreError::invalid(
            "an existing generation preset cannot change its model route",
        ));
    }
    let values_json = encode_generation_preset_values(preset)?;
    transaction
        .execute(
            "INSERT INTO generation_presets
             (id, model_route_id, display_name, values_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               values_json = excluded.values_json,
               updated_at = excluded.updated_at",
            params![
                preset.id.as_str(),
                preset.model_route_id.as_str(),
                preset.display_name,
                values_json,
                preset.created_at.to_rfc3339(),
                preset.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(super) fn encode_generation_preset_values(preset: &GenerationPreset) -> CoreResult<String> {
    serde_json::to_string(&serde_json::json!({
        "schema_version": 1,
        "values": &preset.values,
        "reasoning": &preset.reasoning,
        "prompt_cache": &preset.prompt_cache,
    }))
    .map_err(|error| {
        CoreError::internal(format!("cannot encode generation preset values: {error}"))
    })
}

pub(super) fn generation_preset_columns(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<GenerationPresetRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

pub(super) fn decode_generation_preset_row(
    row: GenerationPresetRow,
) -> CoreResult<GenerationPreset> {
    let (id, model_route_id, display_name, values_json, created_at, updated_at) = row;
    validate_nonempty("stored generation preset display name", &display_name)
        .map_err(stored_catalog_error)?;
    let (values, reasoning, prompt_cache) = decode_generation_preset_values(&values_json)?;
    let mut parameter_ids = std::collections::BTreeSet::new();
    if values
        .iter()
        .any(|value| !parameter_ids.insert(value.parameter_id.as_str()))
    {
        return Err(storage_corrupted(
            "stored generation preset contains duplicate parameter identifiers",
        ));
    }
    let created_at = parse_stored_datetime(&created_at, "generation preset created_at")?;
    let updated_at = parse_stored_datetime(&updated_at, "generation preset updated_at")?;
    if updated_at < created_at {
        return Err(storage_corrupted(
            "stored generation preset timestamps are inconsistent",
        ));
    }
    Ok(GenerationPreset {
        id: GenerationPresetId::from(id),
        model_route_id: ModelRouteId::from(model_route_id),
        display_name,
        values,
        reasoning,
        prompt_cache,
        created_at,
        updated_at,
    })
}

fn decode_generation_preset_values(
    values_json: &str,
) -> CoreResult<(
    Vec<ParameterValue>,
    GenerationReasoningSettings,
    GenerationPromptCacheSettings,
)> {
    let value = serde_json::from_str::<serde_json::Value>(values_json).map_err(|error| {
        storage_corrupted(format!(
            "stored generation preset values are invalid: {error}"
        ))
    })?;
    if value.is_array() {
        let values = serde_json::from_value(value).map_err(|error| {
            storage_corrupted(format!(
                "stored legacy generation preset values are invalid: {error}"
            ))
        })?;
        return Ok((
            values,
            GenerationReasoningSettings::default(),
            GenerationPromptCacheSettings::default(),
        ));
    }
    let object = value.as_object().ok_or_else(|| {
        storage_corrupted("stored generation preset values must be an object or legacy array")
    })?;
    let expected_keys = ["schema_version", "values", "reasoning", "prompt_cache"];
    if object.len() != expected_keys.len()
        || expected_keys.iter().any(|key| !object.contains_key(*key))
        || object
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            != Some(1)
    {
        return Err(storage_corrupted(
            "stored generation preset values use an unsupported schema",
        ));
    }
    let values = serde_json::from_value(object["values"].clone()).map_err(|error| {
        storage_corrupted(format!(
            "stored generation preset parameter values are invalid: {error}"
        ))
    })?;
    let reasoning = serde_json::from_value(object["reasoning"].clone()).map_err(|error| {
        storage_corrupted(format!(
            "stored generation preset reasoning settings are invalid: {error}"
        ))
    })?;
    let prompt_cache = serde_json::from_value(object["prompt_cache"].clone()).map_err(|error| {
        storage_corrupted(format!(
            "stored generation preset prompt-cache settings are invalid: {error}"
        ))
    })?;
    Ok((values, reasoning, prompt_cache))
}
