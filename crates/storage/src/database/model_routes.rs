use super::{
    ApiFamily, BoundedJson, CapabilityObservation, Connection, CoreError, CoreResult, DateTime,
    GenerationPreset, LEGACY_PROVIDER_TEMPLATE_ID, LEGACY_PROVIDER_TEMPLATE_VERSION,
    ModelAvailability, ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId,
    ModelSyncJobId, OptionalExtension, ProviderConnection, ProviderConnectionId, ProviderTemplate,
    Storage, Utc, active_retained_legacy_profile_exists, clear_provider_selections_for_route,
    current_migrated_legacy_route_exists, decode_provider_connection_row, not_found, params,
    parse_stored_datetime, provider_connection_columns, row_exists, storage_corrupted,
    storage_db_error, stored_catalog_error, upsert_capability_observation_row,
    upsert_generation_preset_row, upsert_provider_connection_row, validate_nonempty,
    validate_provider_api_snapshot_observations_for_routes, validate_provider_catalog_foreign_keys,
    validate_route_config,
};

impl Storage {
    pub fn list_model_routes(
        &self,
        connection_id: &ProviderConnectionId,
    ) -> CoreResult<Vec<ModelRoute>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, connection_id, api_family, model_id, display_name, route_json,
                        availability, raw_metadata_json, miss_count, metadata_source_kind,
                        metadata_observed_at, last_reconciled_sync_job_id,
                        metadata_sync_job_id, first_seen_at, last_seen_at
                 FROM provider_models WHERE connection_id = ?1
                 ORDER BY model_id COLLATE NOCASE, id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([connection_id.as_str()], model_route_columns)
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter().map(decode_model_route_row).collect()
    }

    pub fn get_model_route(&self, id: &ModelRouteId) -> CoreResult<ModelRoute> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, connection_id, api_family, model_id, display_name, route_json,
                        availability, raw_metadata_json, miss_count, metadata_source_kind,
                        metadata_observed_at, last_reconciled_sync_job_id,
                        metadata_sync_job_id, first_seen_at, last_seen_at
                 FROM provider_models WHERE id = ?1",
                [id.as_str()],
                model_route_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("model route"))?;
        decode_model_route_row(row)
    }

    pub fn save_model_route(&self, route: &ModelRoute) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        if active_retained_legacy_profile_exists(&transaction, route.connection_id.as_str())? {
            return Err(CoreError::invalid(
                "migrated legacy model routes are managed through their retained provider profile",
            ));
        }
        validate_model_route(&transaction, route)?;
        upsert_model_route_row(&transaction, route)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn reconcile_model_routes(
        &self,
        connection_id: &ProviderConnectionId,
        listed_routes: &[ModelRoute],
        observed_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        if !row_exists(
            &transaction,
            "SELECT EXISTS(
               SELECT 1 FROM provider_connections
               WHERE id = ?1 AND archived_at IS NULL
             )",
            connection_id.as_str(),
        )? {
            return Err(not_found("provider connection"));
        }
        let existing_routes = load_model_routes_for_reconciliation(&transaction, connection_id)?;
        if existing_routes.iter().any(|route| {
            route
                .last_seen_at
                .is_some_and(|last_seen_at| last_seen_at > observed_at)
        }) {
            return Err(CoreError::invalid(
                "model reconciliation observation is older than stored model data",
            ));
        }
        let mut listed_ids = std::collections::BTreeSet::new();
        for route in listed_routes {
            if route.connection_id.as_str() != connection_id.as_str() {
                return Err(CoreError::invalid(
                    "every reconciled model route must belong to the requested connection",
                ));
            }
            if !listed_ids.insert(route.id.as_str()) {
                return Err(CoreError::invalid(
                    "reconciled model route identifiers must be unique",
                ));
            }
            let mut seen = route.clone();
            // This legacy wrapper represents a successful list response and
            // therefore normalizes every returned route to available. The
            // durable model-sync path preserves richer reviewed availability.
            seen.status = ModelAvailability::Available;
            seen.miss_count = 0;
            seen.last_seen_at = Some(observed_at);
            seen.first_seen_at = existing_routes
                .iter()
                .find(|existing| existing.id == route.id)
                .map_or(observed_at, |existing| existing.first_seen_at);
            upsert_model_route_row(&transaction, &seen)?;
        }
        for existing in existing_routes {
            if !listed_ids.contains(existing.id.as_str()) {
                transaction
                    .execute(
                        "UPDATE provider_models
                         SET miss_count = MIN(miss_count + 1, 4294967295),
                             availability = CASE
                               WHEN availability IN (
                                 'documented_only', 'access_denied', 'deprecated', 'retired'
                               ) THEN availability
                               ELSE 'missing_temporarily'
                             END
                         WHERE id = ?1 AND connection_id = ?2",
                        params![existing.id.as_str(), connection_id.as_str()],
                    )
                    .map_err(storage_db_error)?;
            }
        }
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Publishes an entire model refresh as one transaction so routes, initial
    /// presets, and connection status can never be observed half-applied.
    #[allow(
        clippy::too_many_lines,
        reason = "the refresh graph and its authoritative observation snapshot share one transaction"
    )]
    pub fn commit_model_refresh(
        &self,
        expected_connection: &ProviderConnection,
        refreshed_connection: &ProviderConnection,
        listed_routes: &[ModelRoute],
        new_presets: &[GenerationPreset],
        capability_observations: &[CapabilityObservation],
        observed_at: DateTime<Utc>,
    ) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        if expected_connection.id != refreshed_connection.id {
            return Err(CoreError::invalid(
                "model refresh connection identity cannot change",
            ));
        }
        let stored_connection = transaction
            .query_row(
                "SELECT id, template_id, template_version, display_name, api_origin,
                        config_json, credential_ref, credential_scope_json, timeout_seconds,
                        status, created_at, updated_at
                 FROM provider_connections
                 WHERE id = ?1 AND archived_at IS NULL",
                [expected_connection.id.as_str()],
                provider_connection_columns,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("provider connection"))
            .and_then(decode_provider_connection_row)?;
        if stored_connection != *expected_connection {
            return Err(CoreError::invalid(
                "provider connection changed while its model list was refreshing",
            ));
        }
        let existing_routes =
            load_model_routes_for_reconciliation(&transaction, &refreshed_connection.id)?;
        if existing_routes.iter().any(|route| {
            route
                .last_seen_at
                .is_some_and(|last_seen_at| last_seen_at > observed_at)
        }) {
            return Err(CoreError::invalid(
                "model reconciliation observation is older than stored model data",
            ));
        }

        let mut listed_ids = std::collections::BTreeSet::new();
        for route in listed_routes {
            if route.connection_id != refreshed_connection.id {
                return Err(CoreError::invalid(
                    "every reconciled model route must belong to the requested connection",
                ));
            }
            if !listed_ids.insert(route.id.as_str()) {
                return Err(CoreError::invalid(
                    "reconciled model route identifiers must be unique",
                ));
            }
            let mut seen = route.clone();
            seen.miss_count = 0;
            seen.last_seen_at = Some(observed_at);
            seen.first_seen_at = existing_routes
                .iter()
                .find(|existing| existing.id == route.id)
                .map_or(observed_at, |existing| existing.first_seen_at);
            upsert_model_route_row(&transaction, &seen)?;
        }
        validate_provider_api_snapshot_observations_for_routes(
            capability_observations,
            &listed_ids,
            observed_at,
        )?;
        for existing in existing_routes {
            if !listed_ids.contains(existing.id.as_str()) {
                transaction
                    .execute(
                        "UPDATE provider_models
                         SET miss_count = MIN(miss_count + 1, 4294967295),
                             availability = CASE
                               WHEN availability IN (
                                 'documented_only', 'access_denied', 'deprecated', 'retired'
                               ) THEN availability
                               ELSE 'missing_temporarily'
                             END
                         WHERE id = ?1 AND connection_id = ?2",
                        params![existing.id.as_str(), refreshed_connection.id.as_str()],
                    )
                    .map_err(storage_db_error)?;
            }
        }
        for preset in new_presets {
            upsert_generation_preset_row(&transaction, preset)?;
        }
        for listed_id in &listed_ids {
            transaction
                .execute(
                    "DELETE FROM model_capability_observations
                     WHERE model_route_id = ?1 AND source_kind = 'provider_api'",
                    [*listed_id],
                )
                .map_err(storage_db_error)?;
        }
        for observation in capability_observations {
            if !listed_ids.contains(observation.model_route_id.as_str()) {
                return Err(CoreError::invalid(
                    "model refresh capability observations must belong to a listed route",
                ));
            }
            upsert_capability_observation_row(&transaction, observation)?;
        }
        upsert_provider_connection_row(&transaction, refreshed_connection)?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn delete_model_route(&self, id: &ModelRouteId) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let connection_id = transaction
            .query_row(
                "SELECT connection_id FROM provider_models WHERE id = ?1",
                [id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("model route"))?;
        if current_migrated_legacy_route_exists(&transaction, id.as_str())? {
            return Err(CoreError::invalid(
                "delete the migrated legacy provider connection instead of its current model route",
            ));
        }
        clear_provider_selections_for_route(&transaction, id.as_str(), &connection_id)?;
        transaction
            .execute(
                "DELETE FROM generation_presets WHERE model_route_id = ?1",
                [id.as_str()],
            )
            .map_err(storage_db_error)?;
        transaction
            .execute("DELETE FROM provider_models WHERE id = ?1", [id.as_str()])
            .map_err(storage_db_error)?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }
}
type ModelRouteRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);

fn validate_model_route(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
) -> CoreResult<()> {
    validate_model_route_for_schema(transaction, route, true)
}

pub(super) fn validate_model_route_for_schema(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
    require_active_connection: bool,
) -> CoreResult<()> {
    validate_nonempty("model route id", route.id.as_str())?;
    validate_nonempty("provider connection id", route.connection_id.as_str())?;
    validate_nonempty("model id", &route.model_id)?;
    if route
        .display_name
        .as_deref()
        .is_some_and(|display_name| display_name.trim().is_empty())
    {
        return Err(CoreError::invalid(
            "model route display name must not be empty",
        ));
    }
    validate_route_config(&route.route_config)?;
    let template_query = if require_active_connection {
        "SELECT template.manifest_json
         FROM provider_connections AS connection
         JOIN provider_templates AS template
           ON template.id = connection.template_id
          AND template.version = connection.template_version
         WHERE connection.id = ?1
           AND connection.archived_at IS NULL"
    } else {
        "SELECT template.manifest_json
         FROM provider_connections AS connection
         JOIN provider_templates AS template
           ON template.id = connection.template_id
          AND template.version = connection.template_version
         WHERE connection.id = ?1"
    };
    let template_json = transaction
        .query_row(template_query, [route.connection_id.as_str()], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| not_found("provider connection"))?;
    let template = serde_json::from_str::<ProviderTemplate>(&template_json).map_err(|error| {
        storage_corrupted(format!("stored provider template is invalid: {error}"))
    })?;
    if template.api_family != route.api_family
        || template.default_manifest.api_family != route.api_family
    {
        return Err(CoreError::invalid(
            "model route API family does not match its provider template",
        ));
    }
    if template.id.as_str() == LEGACY_PROVIDER_TEMPLATE_ID
        && template.manifest_version == LEGACY_PROVIDER_TEMPLATE_VERSION
        && route.id.as_str() == route.connection_id.as_str()
        && let Some(legacy_model) = transaction
            .query_row(
                "SELECT model FROM provider_profiles WHERE id = ?1",
                [route.connection_id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_db_error)?
        && legacy_model != route.model_id
    {
        return Err(CoreError::invalid(
            "legacy model route must match its provider profile model",
        ));
    }
    Ok(())
}

pub(crate) fn upsert_model_route_row(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
) -> CoreResult<()> {
    validate_model_route(transaction, route)?;
    if route
        .last_seen_at
        .is_some_and(|last_seen_at| last_seen_at < route.first_seen_at)
    {
        return Err(CoreError::invalid(
            "model route last_seen_at must not precede first_seen_at",
        ));
    }
    let route_json = serde_json::to_string(&route.route_config)
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    let existing_identity = transaction
        .query_row(
            "SELECT connection_id, api_family, model_id, route_json
             FROM provider_models WHERE id = ?1",
            [route.id.as_str()],
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
        .map_err(storage_db_error)?;
    if let Some((connection_id, api_family, model_id, stored_route_json)) = existing_identity
        && (connection_id != route.connection_id.as_str()
            || api_family != api_family_to_str(route.api_family)
            || model_id != route.model_id
            || stored_route_json != route_json)
    {
        return Err(CoreError::invalid(
            "an existing model route cannot change its stable identity",
        ));
    }
    let raw_metadata_json = route.raw_metadata.as_ref().map(BoundedJson::as_str);
    transaction
        .execute(
            "INSERT INTO provider_models
             (id, connection_id, api_family, model_id, display_name, route_json,
              availability, raw_metadata_json, miss_count, metadata_source_kind,
              metadata_observed_at, last_reconciled_sync_job_id, metadata_sync_job_id,
              first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               availability = excluded.availability,
               raw_metadata_json = excluded.raw_metadata_json,
               miss_count = excluded.miss_count,
               metadata_source_kind = excluded.metadata_source_kind,
               metadata_observed_at = excluded.metadata_observed_at,
               last_reconciled_sync_job_id = excluded.last_reconciled_sync_job_id,
               metadata_sync_job_id = excluded.metadata_sync_job_id,
               first_seen_at = MIN(provider_models.first_seen_at, excluded.first_seen_at),
               last_seen_at = excluded.last_seen_at",
            params![
                route.id.as_str(),
                route.connection_id.as_str(),
                api_family_to_str(route.api_family),
                route.model_id,
                route.display_name,
                route_json,
                model_availability_to_str(route.status),
                raw_metadata_json,
                route.miss_count,
                model_metadata_source_to_str(route.metadata_source),
                route.metadata_observed_at.map(|value| value.to_rfc3339()),
                route
                    .last_reconciled_sync_job_id
                    .as_ref()
                    .map(ModelSyncJobId::as_str),
                route
                    .metadata_sync_job_id
                    .as_ref()
                    .map(ModelSyncJobId::as_str),
                route.first_seen_at.to_rfc3339(),
                route.last_seen_at.map(|value| value.to_rfc3339()),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn model_route_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<ModelRouteRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
        row.get(12)?,
        row.get(13)?,
        row.get(14)?,
    ))
}

pub(crate) fn load_model_routes_for_reconciliation(
    transaction: &Connection,
    connection_id: &ProviderConnectionId,
) -> CoreResult<Vec<ModelRoute>> {
    let mut statement = transaction
        .prepare(
            "SELECT id, connection_id, api_family, model_id, display_name, route_json,
                    availability, raw_metadata_json, miss_count, metadata_source_kind,
                    metadata_observed_at, last_reconciled_sync_job_id,
                    metadata_sync_job_id, first_seen_at, last_seen_at
             FROM provider_models WHERE connection_id = ?1
             ORDER BY id",
        )
        .map_err(storage_db_error)?;
    let rows = statement
        .query_map([connection_id.as_str()], model_route_columns)
        .map_err(storage_db_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage_db_error)?;
    rows.into_iter().map(decode_model_route_row).collect()
}

fn decode_model_route_row(row: ModelRouteRow) -> CoreResult<ModelRoute> {
    let (
        id,
        connection_id,
        api_family,
        model_id,
        display_name,
        route_json,
        availability,
        raw_metadata_json,
        miss_count,
        metadata_source_kind,
        metadata_observed_at,
        last_reconciled_sync_job_id,
        metadata_sync_job_id,
        first_seen_at,
        last_seen_at,
    ) = row;
    validate_nonempty("stored model id", &model_id).map_err(stored_catalog_error)?;
    if display_name
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(storage_corrupted(
            "stored model route display name is empty",
        ));
    }
    let route_config = serde_json::from_str::<ModelRouteConfig>(&route_json).map_err(|error| {
        storage_corrupted(format!("stored model route config is invalid: {error}"))
    })?;
    let raw_metadata = raw_metadata_json
        .map(lorepia_domain::BoundedJson::parse)
        .transpose()
        .map_err(|error| {
            storage_corrupted(format!("stored model route metadata is invalid: {error}"))
        })?;
    let miss_count = u32::try_from(miss_count)
        .map_err(|_| storage_corrupted("stored model route miss count is invalid"))?;
    let metadata_source = str_to_model_metadata_source(&metadata_source_kind)?;
    let metadata_observed_at = metadata_observed_at
        .map(|value| parse_stored_datetime(&value, "model route metadata_observed_at"))
        .transpose()?;
    validate_route_config(&route_config).map_err(stored_catalog_error)?;
    let first_seen_at = parse_stored_datetime(&first_seen_at, "model route first_seen_at")?;
    let last_seen_at = last_seen_at
        .map(|value| parse_stored_datetime(&value, "model route last_seen_at"))
        .transpose()?;
    if last_seen_at.is_some_and(|value| value < first_seen_at) {
        return Err(storage_corrupted(
            "stored model route timestamps are inconsistent",
        ));
    }
    Ok(ModelRoute {
        id: ModelRouteId::from(id),
        connection_id: ProviderConnectionId::from(connection_id),
        api_family: str_to_api_family(&api_family)?,
        model_id,
        display_name,
        route_config,
        status: str_to_model_availability(&availability)?,
        miss_count,
        raw_metadata,
        metadata_source,
        metadata_observed_at,
        last_reconciled_sync_job_id: last_reconciled_sync_job_id.map(ModelSyncJobId::from),
        metadata_sync_job_id: metadata_sync_job_id.map(ModelSyncJobId::from),
        first_seen_at,
        last_seen_at,
    })
}

pub(super) const fn api_family_to_str(family: ApiFamily) -> &'static str {
    match family {
        ApiFamily::OpenAiResponses => "openai_responses",
        ApiFamily::OpenAiChatCompletions => "openai_chat_completions",
        ApiFamily::AnthropicMessages => "anthropic_messages",
        ApiFamily::GeminiGenerateContent => "gemini_generate_content",
        ApiFamily::OllamaNative => "ollama_native",
    }
}

pub(super) fn str_to_api_family(value: &str) -> CoreResult<ApiFamily> {
    match value {
        "openai_responses" => Ok(ApiFamily::OpenAiResponses),
        "openai_chat_completions" => Ok(ApiFamily::OpenAiChatCompletions),
        "anthropic_messages" => Ok(ApiFamily::AnthropicMessages),
        "gemini_generate_content" => Ok(ApiFamily::GeminiGenerateContent),
        "ollama_native" => Ok(ApiFamily::OllamaNative),
        _ => Err(storage_corrupted(format!(
            "stored provider API family is invalid: {value}"
        ))),
    }
}

pub(super) const fn model_availability_to_str(status: ModelAvailability) -> &'static str {
    match status {
        ModelAvailability::Available => "available",
        ModelAvailability::MissingTemporarily => "missing_temporarily",
        ModelAvailability::DocumentedOnly => "documented_only",
        ModelAvailability::AccessDenied => "access_denied",
        ModelAvailability::Deprecated => "deprecated",
        ModelAvailability::Retired => "retired",
        ModelAvailability::Unknown => "unknown",
    }
}

fn str_to_model_availability(value: &str) -> CoreResult<ModelAvailability> {
    match value {
        "available" => Ok(ModelAvailability::Available),
        "missing_temporarily" => Ok(ModelAvailability::MissingTemporarily),
        "documented_only" => Ok(ModelAvailability::DocumentedOnly),
        "access_denied" => Ok(ModelAvailability::AccessDenied),
        "deprecated" => Ok(ModelAvailability::Deprecated),
        "retired" => Ok(ModelAvailability::Retired),
        "unknown" => Ok(ModelAvailability::Unknown),
        _ => Err(storage_corrupted(format!(
            "stored model availability is invalid: {value}"
        ))),
    }
}

const fn model_metadata_source_to_str(source: ModelMetadataSource) -> &'static str {
    match source {
        ModelMetadataSource::Legacy => "legacy",
        ModelMetadataSource::ProviderApi => "provider_api",
        ModelMetadataSource::OfficialDocumentation => "official_documentation",
        ModelMetadataSource::SignedCatalog => "signed_catalog",
        ModelMetadataSource::CapabilityProbe => "capability_probe",
        ModelMetadataSource::UserOverride => "user_override",
    }
}

fn str_to_model_metadata_source(value: &str) -> CoreResult<ModelMetadataSource> {
    match value {
        "legacy" => Ok(ModelMetadataSource::Legacy),
        "provider_api" => Ok(ModelMetadataSource::ProviderApi),
        "official_documentation" => Ok(ModelMetadataSource::OfficialDocumentation),
        "signed_catalog" => Ok(ModelMetadataSource::SignedCatalog),
        "capability_probe" => Ok(ModelMetadataSource::CapabilityProbe),
        "user_override" => Ok(ModelMetadataSource::UserOverride),
        _ => Err(storage_corrupted(format!(
            "stored model metadata source is invalid: {value}"
        ))),
    }
}
