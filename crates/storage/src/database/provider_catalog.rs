use super::{
    ApiFamily, AuthBinding, CanonicalOrigin, ConnectionConfig, ConnectionConfigEntry,
    ConnectionConfigValue, ConnectionFieldSpec, ConnectionFieldType, ConnectionStatus, CoreError,
    CoreErrorCode, CoreResult, CredentialRedirectPolicy, CredentialRef, CredentialScope, DateTime,
    DecoderId, Digest, EndpointPath, EndpointSpec, GenerationPreset, GenerationPresetId,
    GenerationPromptCacheSettings, GenerationReasoningSettings, HttpMethod, IpAddr,
    LEGACY_BASE_URL_CONFIG_KEY, LEGACY_PROVIDER_TEMPLATE_ID, LEGACY_PROVIDER_TEMPLATE_VERSION,
    MAX_OUTPUT_TOKENS_PARAMETER_ID, ManifestDecoders, ManifestEndpoints, ModelAvailability,
    ModelMetadataSource, ModelRoute, ModelRouteConfig, ModelRouteId, OptionalExtension,
    ParameterDefaultMode, ParameterId, ParameterLiteral, ParameterSpec, ParameterType,
    ParameterValue, ParameterValueState, ProviderConnection, ProviderConnectionId,
    ProviderManifest, ProviderNetworkMode, ProviderParameterMapping, ProviderParameterTarget,
    ProviderProfile, ProviderTemplate, ProviderTemplateId, Sha256, Storage,
    TEMPERATURE_PARAMETER_ID, TemplateSource, UiParameterLevel, Url, Utc, Uuid, api_family_to_str,
    archive_provider_connection_row, is_sensitive_configuration_key, not_found, params, row_exists,
    storage_corrupted, storage_db_error, update_stored_settings, upsert_generation_preset_row,
    upsert_model_route_row, upsert_provider_connection_row, validate_nonempty,
    validate_parameter_value, validate_provider_catalog_foreign_keys,
};

impl Storage {
    pub fn list_provider_profiles(&self) -> CoreResult<Vec<ProviderProfile>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT profile.id, profile.display_name, profile.base_url,
                        profile.model, profile.timeout_seconds
                 FROM provider_profiles AS profile
                 JOIN provider_connections AS connection
                   ON connection.id = profile.id
                  AND connection.archived_at IS NULL
                 ORDER BY profile.display_name COLLATE NOCASE, profile.id",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], map_provider_profile)
            .map_err(storage_db_error)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)
    }

    pub fn get_provider_profile(&self, id: &str) -> CoreResult<ProviderProfile> {
        self.connection()?
            .query_row(
                "SELECT profile.id, profile.display_name, profile.base_url,
                        profile.model, profile.timeout_seconds
                 FROM provider_profiles AS profile
                 JOIN provider_connections AS connection
                   ON connection.id = profile.id
                  AND connection.archived_at IS NULL
                 WHERE profile.id = ?1",
                [id],
                map_provider_profile,
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| {
                CoreError::new(
                    CoreErrorCode::NotFound,
                    "provider profile was not found",
                    false,
                )
            })
    }

    pub fn save_provider_profile(&self, profile: &ProviderProfile) -> CoreResult<()> {
        let (connection_value, mut route, mut preset) = legacy_provider_graph(profile, Utc::now())?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        ensure_legacy_template_exists(&transaction)?;
        upsert_provider_profile_row(&transaction, profile)?;
        upsert_provider_connection_row(&transaction, &connection_value)?;

        // A legacy profile can change its selected model, but a ModelRoute
        // identity cannot be renamed. Reuse an exact route when it exists;
        // otherwise create a deterministic sibling and preserve the old
        // route, presets, and generation/conversation references.
        route.id = legacy_model_route_id(&transaction, &route)?;
        preset.id = GenerationPresetId::from(route.id.as_str());
        preset.model_route_id = route.id.clone();
        upsert_model_route_row(&transaction, &route)?;
        if !row_exists(
            &transaction,
            "SELECT EXISTS(SELECT 1 FROM generation_presets WHERE id = ?1)",
            preset.id.as_str(),
        )? {
            upsert_generation_preset_row(&transaction, &preset)?;
        }
        update_stored_settings(&transaction, |settings| {
            if settings.selected_provider_profile_id.as_deref() == Some(profile.id.as_str()) {
                settings.selected_model_route_id = Some(route.id.clone());
                settings.selected_generation_preset_id = Some(preset.id.clone());
            }
            Ok(())
        })?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn delete_provider_profile(&self, id: &str) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        let active_profile = row_exists(
            &transaction,
            "SELECT EXISTS(
               SELECT 1
               FROM provider_profiles AS profile
               JOIN provider_connections AS connection
                 ON connection.id = profile.id
                AND connection.archived_at IS NULL
               WHERE profile.id = ?1
             )",
            id,
        )?;
        if !active_profile {
            return Err(not_found("provider profile"));
        }
        archive_provider_connection_row(&transaction, id, Utc::now())?;
        validate_provider_catalog_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    pub fn list_provider_templates(&self) -> CoreResult<Vec<ProviderTemplate>> {
        let connection = self.connection()?;
        let mut statement = connection
            .prepare(
                "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
                 FROM provider_templates
                 ORDER BY display_name COLLATE NOCASE, id, version DESC",
            )
            .map_err(storage_db_error)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?;
        rows.into_iter().map(decode_provider_template_row).collect()
    }

    pub fn get_provider_template(
        &self,
        id: &ProviderTemplateId,
        version: u32,
    ) -> CoreResult<ProviderTemplate> {
        let row = self
            .connection()?
            .query_row(
                "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
                 FROM provider_templates WHERE id = ?1 AND version = ?2",
                params![id.as_str(), version],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_db_error)?
            .ok_or_else(|| not_found("provider template"))?;
        decode_provider_template_row(row)
    }

    pub fn save_provider_template(&self, template: &ProviderTemplate) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction().map_err(storage_db_error)?;
        save_provider_template_row(&transaction, template)?;
        transaction.commit().map_err(storage_db_error)
    }
}
pub(super) fn save_provider_template_row(
    transaction: &rusqlite::Transaction<'_>,
    template: &ProviderTemplate,
) -> CoreResult<()> {
    validate_provider_template(template)?;
    let manifest_json = serde_json::to_string(template).map_err(|error| {
        CoreError::internal(format!("cannot encode provider template: {error}"))
    })?;
    let manifest_sha256 = hex::encode(Sha256::digest(manifest_json.as_bytes()));
    if let Some(existing_row) = transaction
        .query_row(
            "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
             FROM provider_templates WHERE id = ?1 AND version = ?2",
            params![template.id.as_str(), template.manifest_version],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
    {
        let existing = decode_provider_template_row(existing_row)?;
        if existing == *template {
            return Ok(());
        }
        return Err(CoreError::invalid(
            "provider template versions are immutable; save changes under a new version",
        ));
    }
    transaction
        .execute(
            "INSERT INTO provider_templates
             (id, version, display_name, source_kind, manifest_json, manifest_sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                template.id.as_str(),
                template.manifest_version,
                template.display_name,
                template_source_to_str(template.source),
                manifest_json,
                manifest_sha256,
                Utc::now().to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn validate_provider_template(template: &ProviderTemplate) -> CoreResult<()> {
    validate_nonempty("provider template id", template.id.as_str())?;
    validate_nonempty("provider template display name", &template.display_name)?;
    if template.manifest_version == 0 || template.default_manifest.schema_version == 0 {
        return Err(CoreError::invalid(
            "provider template and manifest versions must be positive",
        ));
    }
    if template.api_family != template.default_manifest.api_family {
        return Err(CoreError::invalid(
            "provider template API family must match its manifest",
        ));
    }
    let mut connection_field_keys = std::collections::BTreeSet::new();
    for field in &template.connection_fields {
        validate_nonempty("provider connection field key", &field.key)?;
        validate_nonempty("provider connection field label", &field.label_key)?;
        if !connection_field_keys.insert(field.key.as_str()) {
            return Err(CoreError::invalid(
                "provider connection field keys must be unique",
            ));
        }
        if is_sensitive_configuration_key(&field.key)
            && field.value_type != ConnectionFieldType::Credential
        {
            return Err(CoreError::invalid(
                "secret-like provider connection fields must use the credential field type",
            ));
        }
    }
    let mut parameter_ids = std::collections::BTreeSet::new();
    for specification in &template.default_manifest.parameters {
        validate_nonempty("provider parameter id", specification.id.as_str())?;
        validate_nonempty("provider parameter label", &specification.label_key)?;
        validate_nonempty(
            "provider parameter mapping field",
            &specification.provider_mapping.field_name,
        )?;
        if !parameter_ids.insert(specification.id.as_str()) {
            return Err(CoreError::invalid(
                "provider manifest parameter identifiers must be unique",
            ));
        }
        if specification
            .minimum
            .is_some_and(|value| !value.is_finite())
            || specification
                .maximum
                .is_some_and(|value| !value.is_finite())
            || specification
                .step
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || matches!(
                (specification.minimum, specification.maximum),
                (Some(minimum), Some(maximum)) if minimum > maximum
            )
        {
            return Err(CoreError::invalid(
                "provider parameter numeric constraints are invalid",
            ));
        }
        for choice in &specification.allowed_values {
            validate_nonempty("provider parameter choice label", &choice.label_key)?;
            validate_parameter_value(
                specification,
                &ParameterValueState::Explicit(choice.value.clone()),
            )?;
        }
    }
    for source in &template.default_manifest.sources {
        if let Some(hash) = source.content_sha256.as_deref()
            && (hash.len() != 64
                || hash
                    .bytes()
                    .any(|value| !value.is_ascii_hexdigit() || value.is_ascii_uppercase()))
        {
            return Err(CoreError::invalid(
                "provider manifest source hash must be lowercase SHA-256 hex",
            ));
        }
    }
    Ok(())
}

pub(super) fn legacy_provider_template() -> CoreResult<ProviderTemplate> {
    let temperature = ParameterSpec {
        id: ParameterId::from(TEMPERATURE_PARAMETER_ID),
        label_key: "provider.parameter.temperature".to_owned(),
        description_key: Some("provider.parameter.temperature.description".to_owned()),
        value_type: ParameterType::Number,
        allowed_values: Vec::new(),
        minimum: Some(0.0),
        maximum: Some(2.0),
        step: Some(0.1),
        default_mode: ParameterDefaultMode::ProviderDefault,
        visibility: None,
        conflicts: Vec::new(),
        provider_mapping: ProviderParameterMapping {
            target: ProviderParameterTarget::RequestBody,
            field_name: TEMPERATURE_PARAMETER_ID.to_owned(),
        },
        level: UiParameterLevel::Basic,
    };
    let max_output_tokens = ParameterSpec {
        id: ParameterId::from(MAX_OUTPUT_TOKENS_PARAMETER_ID),
        label_key: "provider.parameter.max_output_tokens".to_owned(),
        description_key: Some("provider.parameter.max_output_tokens.description".to_owned()),
        value_type: ParameterType::Integer,
        allowed_values: Vec::new(),
        minimum: Some(1.0),
        maximum: Some(f64::from(u32::MAX)),
        step: Some(1.0),
        default_mode: ParameterDefaultMode::ProviderDefault,
        visibility: None,
        conflicts: Vec::new(),
        provider_mapping: ProviderParameterMapping {
            target: ProviderParameterTarget::RequestBody,
            field_name: "max_tokens".to_owned(),
        },
        level: UiParameterLevel::Basic,
    };
    Ok(ProviderTemplate {
        id: ProviderTemplateId::from(LEGACY_PROVIDER_TEMPLATE_ID),
        display_name: "Custom OpenAI-compatible Chat".to_owned(),
        manifest_version: LEGACY_PROVIDER_TEMPLATE_VERSION,
        source: TemplateSource::BuiltIn,
        api_family: ApiFamily::OpenAiChatCompletions,
        connection_fields: vec![
            ConnectionFieldSpec {
                key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
                label_key: "provider.connection.api_base_url".to_owned(),
                description_key: Some("provider.connection.api_base_url.description".to_owned()),
                value_type: ConnectionFieldType::Text,
                required: true,
            },
            ConnectionFieldSpec {
                key: "api_key".to_owned(),
                label_key: "provider.connection.api_key".to_owned(),
                description_key: Some("provider.connection.api_key.description".to_owned()),
                value_type: ConnectionFieldType::Credential,
                required: false,
            },
        ],
        default_manifest: ProviderManifest {
            schema_version: 1,
            api_family: ApiFamily::OpenAiChatCompletions,
            sources: Vec::new(),
            default_api_origin: None,
            auth: AuthBinding::BearerHeader,
            endpoints: ManifestEndpoints {
                models: Some(EndpointSpec {
                    method: HttpMethod::Get,
                    path: endpoint_path("/models")?,
                }),
                generate: EndpointSpec {
                    method: HttpMethod::Post,
                    path: endpoint_path("/chat/completions")?,
                },
                embeddings: None,
            },
            decoders: ManifestDecoders {
                response: DecoderId::OpenAiJsonV1,
                streaming: Some(DecoderId::OpenAiSseV1),
            },
            parameters: vec![temperature, max_output_tokens],
        },
    })
}

fn endpoint_path(value: &str) -> CoreResult<EndpointPath> {
    EndpointPath::parse(value).map_err(|error| {
        CoreError::internal(format!("built-in provider endpoint is invalid: {error}"))
    })
}

pub(super) fn legacy_provider_graph(
    profile: &ProviderProfile,
    timestamp: DateTime<Utc>,
) -> CoreResult<(ProviderConnection, ModelRoute, GenerationPreset)> {
    validate_legacy_provider_profile(profile)?;
    let api_origin = canonical_origin_for_legacy_base_url(&profile.base_url)?;
    let id = profile.id.as_str();
    let connection = ProviderConnection {
        id: ProviderConnectionId::from(id),
        template_id: ProviderTemplateId::from(LEGACY_PROVIDER_TEMPLATE_ID),
        template_version: LEGACY_PROVIDER_TEMPLATE_VERSION,
        display_name: profile.display_name.clone(),
        api_origin: api_origin.clone(),
        config: ConnectionConfig {
            api_base_path: legacy_api_base_path(&profile.base_url)?,
            network_mode: legacy_network_mode(&profile.base_url)?,
            local_network_approval: None,
            values: vec![ConnectionConfigEntry {
                key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
                value: ConnectionConfigValue::Text(profile.base_url.clone()),
            }],
        },
        credential_ref: Some(CredentialRef(profile.id.clone())),
        credential_scope: Some(CredentialScope {
            allowed_origins: vec![api_origin],
            auth_binding: AuthBinding::BearerHeader,
            redirect_policy: CredentialRedirectPolicy::Deny,
        }),
        timeout_seconds: profile.timeout_seconds,
        status: ConnectionStatus::Untested,
        created_at: timestamp,
        updated_at: timestamp,
    };
    let route = ModelRoute {
        id: ModelRouteId::from(id),
        connection_id: ProviderConnectionId::from(id),
        api_family: ApiFamily::OpenAiChatCompletions,
        model_id: profile.model.clone(),
        display_name: Some(profile.model.clone()),
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::Legacy,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: timestamp,
        last_seen_at: None,
    };
    let preset = GenerationPreset {
        id: GenerationPresetId::from(id),
        model_route_id: ModelRouteId::from(id),
        display_name: "Default".to_owned(),
        values: vec![
            ParameterValue {
                parameter_id: ParameterId::from(TEMPERATURE_PARAMETER_ID),
                state: ParameterValueState::Explicit(ParameterLiteral::Number(1.0)),
            },
            ParameterValue {
                parameter_id: ParameterId::from(MAX_OUTPUT_TOKENS_PARAMETER_ID),
                state: ParameterValueState::Explicit(ParameterLiteral::Integer(4096)),
            },
        ],
        reasoning: GenerationReasoningSettings {
            preserve_opaque_state: false,
            ..GenerationReasoningSettings::default()
        },
        prompt_cache: GenerationPromptCacheSettings::default(),
        created_at: timestamp,
        updated_at: timestamp,
    };
    Ok((connection, route, preset))
}

fn legacy_model_route_id(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
) -> CoreResult<ModelRouteId> {
    let route_json = serde_json::to_string(&route.route_config)
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    if let Some(existing_id) = transaction
        .query_row(
            "SELECT id FROM provider_models
             WHERE connection_id = ?1
               AND api_family = ?2
               AND model_id = ?3
               AND route_json = ?4
             ORDER BY first_seen_at, id
             LIMIT 1",
            params![
                route.connection_id.as_str(),
                api_family_to_str(route.api_family),
                route.model_id,
                route_json,
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
    {
        return Ok(ModelRouteId::from(existing_id));
    }
    if !row_exists(
        transaction,
        "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
        route.id.as_str(),
    )? {
        return Ok(route.id.clone());
    }
    let identity = format!(
        "lorepia:legacy-model-route:v1\u{0}{}\u{0}{}\u{0}{}",
        route.connection_id.as_str(),
        api_family_to_str(route.api_family),
        route.model_id,
    );
    Ok(ModelRouteId::from(
        Uuid::new_v5(&Uuid::NAMESPACE_URL, identity.as_bytes()).to_string(),
    ))
}

fn validate_legacy_provider_profile(profile: &ProviderProfile) -> CoreResult<()> {
    validate_nonempty("provider profile id", &profile.id)?;
    validate_nonempty("provider display name", &profile.display_name)?;
    validate_nonempty("provider model", &profile.model)?;
    if !(1..=600).contains(&profile.timeout_seconds) {
        return Err(CoreError::invalid(
            "provider timeout must be from 1 to 600 seconds",
        ));
    }
    canonical_origin_for_legacy_base_url(&profile.base_url)?;
    Ok(())
}

pub(super) fn canonical_origin_for_legacy_base_url(base_url: &str) -> CoreResult<CanonicalOrigin> {
    if base_url.trim() != base_url || base_url.is_empty() {
        return Err(CoreError::invalid(
            "provider base URL must be non-empty and contain no surrounding whitespace",
        ));
    }
    let url = Url::parse(base_url)
        .map_err(|error| CoreError::invalid(format!("invalid provider base URL: {error}")))?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(CoreError::invalid(
            "provider base URL must not contain embedded credentials",
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(CoreError::invalid(
            "provider base URL must not contain a query or fragment",
        ));
    }
    match url.scheme() {
        "https" => {}
        "http" if url.host_str().is_some_and(is_loopback_host) => {}
        "http" => {
            return Err(CoreError::invalid(
                "unencrypted HTTP is allowed only for loopback provider URLs",
            ));
        }
        _ => {
            return Err(CoreError::invalid(
                "provider base URL must use HTTPS or loopback HTTP",
            ));
        }
    }
    CanonicalOrigin::parse(&url.origin().ascii_serialization())
        .map_err(|error| CoreError::invalid(format!("invalid provider API origin: {error}")))
}

pub(super) fn legacy_api_base_path(base_url: &str) -> CoreResult<Option<EndpointPath>> {
    let url = Url::parse(base_url)
        .map_err(|error| CoreError::invalid(format!("invalid provider base URL: {error}")))?;
    if url.path() == "/" {
        Ok(None)
    } else {
        EndpointPath::parse(url.path())
            .map(Some)
            .map_err(|error| CoreError::invalid(format!("invalid provider API base path: {error}")))
    }
}

pub(super) fn legacy_network_mode(base_url: &str) -> CoreResult<ProviderNetworkMode> {
    let url = Url::parse(base_url)
        .map_err(|error| CoreError::invalid(format!("invalid provider base URL: {error}")))?;
    Ok(if url.host_str().is_some_and(is_loopback_host) {
        ProviderNetworkMode::LocalLoopback
    } else {
        ProviderNetworkMode::Public
    })
}

pub(super) fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn upsert_provider_profile_row(
    transaction: &rusqlite::Transaction<'_>,
    profile: &ProviderProfile,
) -> CoreResult<()> {
    validate_legacy_provider_profile(profile)?;
    let existing_base_url = transaction
        .query_row(
            "SELECT base_url FROM provider_profiles WHERE id = ?1",
            [profile.id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    if existing_base_url
        .as_deref()
        .is_some_and(|base_url| base_url != profile.base_url)
    {
        return Err(CoreError::invalid(
            "an existing provider profile cannot change its API endpoint; \
             create a new connection instead",
        ));
    }
    transaction
        .execute(
            "INSERT INTO provider_profiles
             (id, display_name, base_url, model, timeout_seconds)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               model = excluded.model,
               timeout_seconds = excluded.timeout_seconds",
            params![
                profile.id,
                profile.display_name,
                profile.base_url,
                profile.model,
                profile.timeout_seconds
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn ensure_legacy_template_exists(transaction: &rusqlite::Transaction<'_>) -> CoreResult<()> {
    let row = transaction
        .query_row(
            "SELECT id, version, display_name, source_kind, manifest_json, manifest_sha256
             FROM provider_templates WHERE id = ?1 AND version = ?2",
            params![
                LEGACY_PROVIDER_TEMPLATE_ID,
                LEGACY_PROVIDER_TEMPLATE_VERSION
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("built-in legacy provider template is missing"))?;
    let stored = decode_provider_template_row(row)?;
    let expected = legacy_provider_template()?;
    if stored != expected {
        return Err(storage_corrupted(
            "built-in legacy provider template does not match the supported definition",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(super) fn active_retained_legacy_profile_exists(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<bool> {
    row_exists(
        transaction,
        "SELECT EXISTS(
           SELECT 1
           FROM provider_profiles AS profile
           JOIN provider_connections AS connection
             ON connection.id = profile.id
            AND connection.archived_at IS NULL
           WHERE profile.id = ?1
         )",
        connection_id,
    )
}

pub(super) fn current_migrated_legacy_route_exists(
    transaction: &rusqlite::Transaction<'_>,
    route_id: &str,
) -> CoreResult<bool> {
    let route_json = serde_json::to_string(&ModelRouteConfig::default())
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    transaction
        .query_row(
            "SELECT EXISTS(
               SELECT 1
               FROM provider_models AS model
               JOIN provider_profiles AS profile
                 ON profile.id = model.connection_id
                AND profile.model = model.model_id
               JOIN provider_connections AS connection
                 ON connection.id = profile.id
                AND connection.archived_at IS NULL
               WHERE model.id = ?1
                 AND model.api_family = 'openai_chat_completions'
                 AND model.route_json = ?2
             )",
            params![route_id, route_json],
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_db_error)
}

fn map_provider_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProviderProfile> {
    let timeout_seconds = row.get::<_, i64>(4)?;
    let timeout_seconds = u32::try_from(timeout_seconds).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    Ok(ProviderProfile {
        id: row.get(0)?,
        display_name: row.get(1)?,
        base_url: row.get(2)?,
        model: row.get(3)?,
        timeout_seconds,
    })
}

pub(super) fn decode_provider_template_row(
    row: (String, i64, String, String, String, String),
) -> CoreResult<ProviderTemplate> {
    let (id, version, display_name, source, manifest_json, manifest_sha256) = row;
    let version = u32::try_from(version)
        .map_err(|_| storage_corrupted("stored provider template version is invalid"))?;
    let actual_sha256 = hex::encode(Sha256::digest(manifest_json.as_bytes()));
    if actual_sha256 != manifest_sha256 {
        return Err(storage_corrupted(
            "stored provider template manifest hash does not match its content",
        ));
    }
    let template = serde_json::from_str::<ProviderTemplate>(&manifest_json).map_err(|error| {
        storage_corrupted(format!("stored provider template is invalid: {error}"))
    })?;
    let source = str_to_template_source(&source)?;
    if template.id.as_str() != id
        || template.manifest_version != version
        || template.display_name != display_name
        || template.source != source
        || template.api_family != template.default_manifest.api_family
    {
        return Err(storage_corrupted(
            "stored provider template columns do not match its typed manifest",
        ));
    }
    Ok(template)
}

const fn template_source_to_str(source: TemplateSource) -> &'static str {
    match source {
        TemplateSource::BuiltIn => "built_in",
        TemplateSource::SignedCatalog => "signed_catalog",
        TemplateSource::UserDiscovered => "user_discovered",
    }
}

fn str_to_template_source(value: &str) -> CoreResult<TemplateSource> {
    match value {
        "built_in" => Ok(TemplateSource::BuiltIn),
        "signed_catalog" => Ok(TemplateSource::SignedCatalog),
        "user_discovered" => Ok(TemplateSource::UserDiscovered),
        _ => Err(storage_corrupted(format!(
            "stored provider template source is invalid: {value}"
        ))),
    }
}
