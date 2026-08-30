use chrono::Utc;
use lorepia_domain::{
    AppSettings, BoundedJson, CoreError, CoreErrorCode, CoreResult, CredentialRef,
    GenerationPreset, ModelRoute, ProviderConnection, ProviderProfile,
};
use rusqlite::{OptionalExtension, params};

use super::{
    LEGACY_PROVIDER_TEMPLATE_ID, LEGACY_PROVIDER_TEMPLATE_VERSION, api_family_to_str,
    connection_status_to_str, count, encode_generation_preset_values, legacy_provider_graph,
    legacy_provider_template, model_availability_to_str, normalize_settings_for_schema,
    query_count, save_provider_template_row, storage_corrupted, storage_db_error,
    validate_generation_preset_for_schema, validate_model_route_for_schema,
    validate_provider_catalog_foreign_keys, validate_provider_connection,
};

pub(super) fn migrate_legacy_provider_catalog(
    transaction: &rusqlite::Transaction<'_>,
) -> CoreResult<()> {
    for table in [
        "provider_templates",
        "provider_connections",
        "provider_models",
        "model_capability_observations",
        "generation_presets",
        "provider_discovery_sessions",
        "provider_discovery_evidence",
    ] {
        if count(transaction, table)? != 0 {
            return Err(storage_corrupted(format!(
                "provider catalog migration found pre-existing rows in {table}"
            )));
        }
    }

    insert_legacy_provider_template(transaction)?;
    let profiles = {
        let mut statement = transaction
            .prepare(
                "SELECT id, display_name, base_url, model, timeout_seconds
                 FROM provider_profiles ORDER BY id",
            )
            .map_err(storage_db_error)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })
            .map_err(storage_db_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage_db_error)?
    };
    let migrated_at = Utc::now();
    for (id, display_name, base_url, model, timeout_seconds) in profiles {
        let timeout_seconds = u32::try_from(timeout_seconds).map_err(|_| {
            storage_corrupted("legacy provider timeout is outside the supported range")
        })?;
        let profile = ProviderProfile {
            id,
            display_name,
            base_url,
            model,
            timeout_seconds,
        };
        let (connection, route, preset) =
            legacy_provider_graph(&profile, migrated_at).map_err(provider_migration_error)?;
        insert_provider_connection_during_v4_migration(transaction, &connection)
            .map_err(provider_migration_error)?;
        insert_model_route_during_v4_migration(transaction, &route)
            .map_err(provider_migration_error)?;
        insert_generation_preset_during_v4_migration(transaction, &preset)
            .map_err(provider_migration_error)?;
    }

    if let Some(settings_json) = transaction
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
    {
        let settings = serde_json::from_str::<AppSettings>(&settings_json).map_err(|error| {
            storage_corrupted(format!(
                "provider catalog migration found invalid settings: {error}"
            ))
        })?;
        let settings = normalize_settings_during_v4_migration(transaction, &settings)
            .map_err(provider_migration_error)?;
        let settings_json = serde_json::to_string(&settings).map_err(|error| {
            CoreError::internal(format!("cannot encode migrated settings: {error}"))
        })?;
        transaction
            .execute(
                "UPDATE app_settings SET value_json = ?1 WHERE key = 'application'",
                [settings_json],
            )
            .map_err(storage_db_error)?;
    }
    Ok(())
}

/// Migration 0004 runs before the tombstone and local-network approval
/// migrations, so it must not use the current-schema connection upsert.
fn insert_provider_connection_during_v4_migration(
    transaction: &rusqlite::Transaction<'_>,
    connection: &ProviderConnection,
) -> CoreResult<()> {
    validate_provider_connection(transaction, connection)?;
    if connection.updated_at < connection.created_at {
        return Err(CoreError::invalid(
            "provider connection updated_at must not precede created_at",
        ));
    }
    let config_json = serde_json::to_string(&connection.config).map_err(|error| {
        CoreError::internal(format!("cannot encode provider connection config: {error}"))
    })?;
    let credential_scope_json = connection
        .credential_scope
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| CoreError::internal(format!("cannot encode credential scope: {error}")))?;
    transaction
        .execute(
            "INSERT INTO provider_connections
             (id, template_id, template_version, display_name, api_origin, config_json,
              credential_ref, credential_scope_json, timeout_seconds, status,
              created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                connection.id.as_str(),
                connection.template_id.as_str(),
                connection.template_version,
                connection.display_name,
                connection.api_origin.as_str(),
                config_json,
                connection
                    .credential_ref
                    .as_ref()
                    .map(CredentialRef::as_str),
                credential_scope_json,
                connection.timeout_seconds,
                connection_status_to_str(connection.status),
                connection.created_at.to_rfc3339(),
                connection.updated_at.to_rfc3339(),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

/// Migration 0004 runs before the durable model-sync columns are introduced
/// by migration 0009, so it must write only the v4 route shape.
fn insert_model_route_during_v4_migration(
    transaction: &rusqlite::Transaction<'_>,
    route: &ModelRoute,
) -> CoreResult<()> {
    validate_model_route_for_schema(transaction, route, false)?;
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
    transaction
        .execute(
            "INSERT INTO provider_models
             (id, connection_id, api_family, model_id, display_name, route_json,
              availability, raw_metadata_json, first_seen_at, last_seen_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                route.id.as_str(),
                route.connection_id.as_str(),
                api_family_to_str(route.api_family),
                route.model_id,
                route.display_name,
                route_json,
                model_availability_to_str(route.status),
                route.raw_metadata.as_ref().map(BoundedJson::as_str),
                route.first_seen_at.to_rfc3339(),
                route.last_seen_at.map(|value| value.to_rfc3339()),
            ],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

fn insert_generation_preset_during_v4_migration(
    transaction: &rusqlite::Transaction<'_>,
    preset: &GenerationPreset,
) -> CoreResult<()> {
    validate_generation_preset_for_schema(transaction, preset, false)?;
    if preset.updated_at < preset.created_at {
        return Err(CoreError::invalid(
            "generation preset updated_at must not precede created_at",
        ));
    }
    let values_json = encode_generation_preset_values(preset)?;
    transaction
        .execute(
            "INSERT INTO generation_presets
             (id, model_route_id, display_name, values_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
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

fn provider_migration_error(error: CoreError) -> CoreError {
    if error.code == CoreErrorCode::StorageCorrupted {
        error
    } else {
        storage_corrupted(format!(
            "provider catalog migration rejected legacy data: {}",
            error.message
        ))
    }
}

pub(super) fn validate_provider_catalog_migration(
    transaction: &rusqlite::Transaction<'_>,
) -> CoreResult<()> {
    let legacy_count = count(transaction, "provider_profiles")?;
    let connection_count = query_count(
        transaction,
        "SELECT COUNT(*) FROM provider_connections
         WHERE template_id = ?1 AND template_version = ?2",
        params![
            LEGACY_PROVIDER_TEMPLATE_ID,
            LEGACY_PROVIDER_TEMPLATE_VERSION
        ],
    )?;
    let route_count = query_count(
        transaction,
        "SELECT COUNT(*)
         FROM provider_models AS model
         JOIN provider_connections AS connection
           ON connection.id = model.connection_id
         WHERE connection.template_id = ?1
           AND connection.template_version = ?2",
        params![
            LEGACY_PROVIDER_TEMPLATE_ID,
            LEGACY_PROVIDER_TEMPLATE_VERSION
        ],
    )?;
    let preset_count = query_count(
        transaction,
        "SELECT COUNT(*)
         FROM generation_presets AS preset
         JOIN provider_models AS model ON model.id = preset.model_route_id
         JOIN provider_connections AS connection
           ON connection.id = model.connection_id
         WHERE connection.template_id = ?1
           AND connection.template_version = ?2",
        params![
            LEGACY_PROVIDER_TEMPLATE_ID,
            LEGACY_PROVIDER_TEMPLATE_VERSION
        ],
    )?;
    if connection_count != legacy_count
        || route_count != legacy_count
        || preset_count != legacy_count
    {
        return Err(storage_corrupted(format!(
            "provider catalog migration row-count mismatch: legacy={legacy_count}, \
             connections={connection_count}, routes={route_count}, presets={preset_count}"
        )));
    }
    let mismatched_ids = query_count(
        transaction,
        "SELECT COUNT(*)
         FROM provider_profiles AS legacy
         LEFT JOIN provider_connections AS connection
           ON connection.id = legacy.id
         LEFT JOIN provider_models AS model
           ON model.id = legacy.id AND model.connection_id = connection.id
         LEFT JOIN generation_presets AS preset
           ON preset.id = legacy.id AND preset.model_route_id = model.id
         WHERE connection.id IS NULL OR model.id IS NULL OR preset.id IS NULL",
        [],
    )?;
    if mismatched_ids != 0 {
        return Err(storage_corrupted(
            "provider catalog migration did not preserve legacy stable identifiers",
        ));
    }
    validate_provider_catalog_foreign_keys(transaction)
}

pub(super) fn insert_legacy_provider_template(
    transaction: &rusqlite::Transaction<'_>,
) -> CoreResult<()> {
    let template = legacy_provider_template()?;
    save_provider_template_row(transaction, &template)
}

fn normalize_settings_during_v4_migration(
    transaction: &rusqlite::Transaction<'_>,
    settings: &AppSettings,
) -> CoreResult<AppSettings> {
    normalize_settings_for_schema(transaction, settings, false)
}
