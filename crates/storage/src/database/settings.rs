use super::{
    AppSettings, Connection, CoreError, CoreErrorCode, CoreResult, DiscoveryPreviousSelection,
    GenerationPresetId, LocalUserId, ModelRouteConfig, ModelRouteId, OptionalExtension, Storage,
    TransactionBehavior, Uuid, i64_to_u64, not_found, params, row_exists, storage_corrupted,
    storage_db_error,
};

impl Storage {
    pub fn load_settings(&self) -> CoreResult<AppSettings> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let settings = load_or_create_settings_in_transaction(&transaction)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(settings)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> CoreResult<()> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let stored = load_or_create_settings_in_transaction(&transaction)?;
        let mut requested = settings.clone();
        // The local identity is repository-owned. Shell/API callers may update
        // preferences but cannot replace the durable user identity.
        requested.local_user_id = stored.local_user_id;
        let settings = normalize_settings_for_write(&transaction, &requested)?;
        write_application_settings(&transaction, &settings)?;
        // `save_settings` is a user-facing settings intent. Advancing even for
        // an identical provider selection records an explicit None after a
        // discovery-owned clear and closes that otherwise invisible ABA.
        advance_provider_selection_revision(&transaction)?;
        transaction.commit().map_err(storage_db_error)
    }

    /// Atomically persists one explicit generation-target selection intent.
    ///
    /// Other application preferences are loaded and retained inside the same
    /// transaction. The provider-selection revision advances even when all
    /// three requested values are already `None`, because that no-op value is
    /// still newer user intent than an earlier discovery compensation clear.
    pub fn save_generation_target_selection(
        &self,
        selected_provider_profile_id: Option<String>,
        selected_model_route_id: Option<ModelRouteId>,
        selected_generation_preset_id: Option<GenerationPresetId>,
    ) -> CoreResult<AppSettings> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_db_error)?;
        let mut settings = load_or_create_settings_in_transaction(&transaction)?;
        settings.selected_provider_profile_id = selected_provider_profile_id;
        settings.selected_model_route_id = selected_model_route_id;
        settings.selected_generation_preset_id = selected_generation_preset_id;
        let settings = normalize_settings_for_write(&transaction, &settings)?;
        write_application_settings(&transaction, &settings)?;
        advance_provider_selection_revision(&transaction)?;
        transaction.commit().map_err(storage_db_error)?;
        Ok(settings)
    }
}
fn normalize_settings_for_write(
    transaction: &rusqlite::Transaction<'_>,
    settings: &AppSettings,
) -> CoreResult<AppSettings> {
    normalize_settings_for_schema(transaction, settings, true)
}

#[allow(clippy::too_many_lines)]
pub(super) fn normalize_settings_for_schema(
    transaction: &rusqlite::Transaction<'_>,
    settings: &AppSettings,
    require_active_connection: bool,
) -> CoreResult<AppSettings> {
    let mut normalized = settings.clone();
    let profile_exists_query = if require_active_connection {
        "SELECT EXISTS(
           SELECT 1
           FROM provider_profiles AS profile
           JOIN provider_connections AS connection
             ON connection.id = profile.id
            AND connection.archived_at IS NULL
           WHERE profile.id = ?1
         )"
    } else {
        "SELECT EXISTS(
           SELECT 1
           FROM provider_profiles AS profile
           JOIN provider_connections AS connection
             ON connection.id = profile.id
           WHERE profile.id = ?1
         )"
    };
    let route_exists_query = if require_active_connection {
        "SELECT EXISTS(
           SELECT 1
           FROM provider_models AS model
           JOIN provider_connections AS connection
             ON connection.id = model.connection_id
            AND connection.archived_at IS NULL
           WHERE model.id = ?1
         )"
    } else {
        "SELECT EXISTS(
           SELECT 1
           FROM provider_models AS model
           JOIN provider_connections AS connection
             ON connection.id = model.connection_id
           WHERE model.id = ?1
         )"
    };
    if let Some(profile_id) = normalized.selected_provider_profile_id.as_deref() {
        if !row_exists(transaction, profile_exists_query, profile_id)? {
            return Err(not_found("provider profile"));
        }
        let current_route_id = legacy_profile_current_route_id_for_schema(
            transaction,
            profile_id,
            require_active_connection,
        )?;
        match normalized.selected_model_route_id.as_ref() {
            Some(route_id) if route_id != &current_route_id => {
                return Err(CoreError::invalid(
                    "legacy provider and model route selections must identify the same migrated provider",
                ));
            }
            None => {
                normalized.selected_model_route_id = Some(current_route_id.clone());
            }
            Some(_) => {}
        }
        let current_preset_id = GenerationPresetId::from(current_route_id.as_str());
        match normalized.selected_generation_preset_id.as_ref() {
            Some(preset_id) if preset_id != &current_preset_id => {
                return Err(CoreError::invalid(
                    "legacy provider and generation preset selections must identify the same migrated provider",
                ));
            }
            None => {
                normalized.selected_generation_preset_id = Some(current_preset_id);
            }
            Some(_) => {}
        }
    }

    match (
        normalized.selected_model_route_id.as_ref(),
        normalized.selected_generation_preset_id.as_ref(),
    ) {
        (None, None) => {}
        (Some(route_id), Some(preset_id)) => {
            let route_exists = row_exists(transaction, route_exists_query, route_id.as_str())?;
            if !route_exists {
                return Err(not_found("model route"));
            }
            let preset_route_id = transaction
                .query_row(
                    "SELECT model_route_id FROM generation_presets WHERE id = ?1",
                    [preset_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage_db_error)?
                .ok_or_else(|| not_found("generation preset"))?;
            if preset_route_id != route_id.as_str() {
                return Err(CoreError::invalid(
                    "selected generation preset does not belong to the selected model route",
                ));
            }
            if normalized.selected_provider_profile_id.is_none()
                && route_id.as_str() == preset_id.as_str()
                && row_exists(transaction, profile_exists_query, route_id.as_str())?
            {
                normalized.selected_provider_profile_id = Some(route_id.as_str().to_owned());
            }
        }
        _ => {
            return Err(CoreError::invalid(
                "model route and generation preset selections must be set or cleared together",
            ));
        }
    }
    Ok(normalized)
}

pub(super) fn legacy_profile_current_route_id_for_schema(
    connection: &Connection,
    profile_id: &str,
    require_active_connection: bool,
) -> CoreResult<ModelRouteId> {
    let route_json = serde_json::to_string(&ModelRouteConfig::default())
        .map_err(|error| CoreError::internal(format!("cannot encode model route: {error}")))?;
    let route_query = if require_active_connection {
        "SELECT model.id
         FROM provider_profiles AS profile
         JOIN provider_connections AS connection
           ON connection.id = profile.id
          AND connection.archived_at IS NULL
         JOIN provider_models AS model
           ON model.connection_id = profile.id
          AND model.api_family = 'openai_chat_completions'
          AND model.model_id = profile.model
          AND model.route_json = ?2
         WHERE profile.id = ?1
         ORDER BY model.first_seen_at, model.id
         LIMIT 1"
    } else {
        "SELECT model.id
         FROM provider_profiles AS profile
         JOIN provider_connections AS connection
           ON connection.id = profile.id
         JOIN provider_models AS model
           ON model.connection_id = profile.id
          AND model.api_family = 'openai_chat_completions'
          AND model.model_id = profile.model
          AND model.route_json = ?2
         WHERE profile.id = ?1
         ORDER BY model.first_seen_at, model.id
         LIMIT 1"
    };
    connection
        .query_row(route_query, params![profile_id, route_json], |row| {
            row.get::<_, String>(0)
        })
        .optional()
        .map_err(storage_db_error)?
        .map(ModelRouteId::from)
        .ok_or_else(|| {
            storage_corrupted("legacy provider profile has no route for its current model")
        })
}

pub(super) fn clear_provider_selections_for_route(
    transaction: &rusqlite::Transaction<'_>,
    route_id: &str,
    connection_id: &str,
) -> CoreResult<()> {
    update_stored_settings(transaction, |settings| {
        if settings
            .selected_model_route_id
            .as_ref()
            .is_some_and(|selected| selected.as_str() == route_id)
        {
            if settings.selected_provider_profile_id.as_deref() == Some(connection_id) {
                settings.selected_provider_profile_id = None;
            }
            settings.selected_model_route_id = None;
            settings.selected_generation_preset_id = None;
        }
        Ok(())
    })
}

pub(super) fn clear_provider_selections_for_preset(
    transaction: &rusqlite::Transaction<'_>,
    preset_id: &str,
    route_id: &str,
) -> CoreResult<()> {
    update_stored_settings(transaction, |settings| {
        if settings
            .selected_generation_preset_id
            .as_ref()
            .is_some_and(|selected| selected.as_str() == preset_id)
        {
            if settings.selected_provider_profile_id.as_deref() == Some(route_id) {
                settings.selected_provider_profile_id = None;
            }
            settings.selected_model_route_id = None;
            settings.selected_generation_preset_id = None;
        }
        Ok(())
    })
}

pub(super) fn update_stored_settings(
    transaction: &rusqlite::Transaction<'_>,
    update: impl FnOnce(&mut AppSettings) -> CoreResult<()>,
) -> CoreResult<()> {
    if update_stored_settings_without_selection_revision(transaction, update)? {
        advance_provider_selection_revision(transaction)?;
    }
    Ok(())
}

/// Applies an internal settings mutation and returns whether the provider
/// selection tuple changed. Callers must either advance the ordinary revision
/// or bind the resulting revision to discovery compensation before commit.
pub(super) fn update_stored_settings_without_selection_revision(
    transaction: &rusqlite::Transaction<'_>,
    update: impl FnOnce(&mut AppSettings) -> CoreResult<()>,
) -> CoreResult<bool> {
    let settings_json = transaction
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    let Some(settings_json) = settings_json else {
        return Ok(false);
    };
    let mut settings = serde_json::from_str::<AppSettings>(&settings_json)
        .map_err(|error| storage_corrupted(format!("stored settings are invalid: {error}")))?;
    let original = settings.clone();
    update(&mut settings)?;
    if settings == original {
        return Ok(false);
    }
    let selection_changed = provider_selection_changed(&original, &settings);
    write_application_settings(transaction, &settings)?;
    Ok(selection_changed)
}

fn provider_selection_changed(left: &AppSettings, right: &AppSettings) -> bool {
    left.selected_provider_profile_id != right.selected_provider_profile_id
        || left.selected_model_route_id != right.selected_model_route_id
        || left.selected_generation_preset_id != right.selected_generation_preset_id
}

fn write_application_settings(
    transaction: &rusqlite::Transaction<'_>,
    settings: &AppSettings,
) -> CoreResult<()> {
    let settings_json = serde_json::to_string(settings)
        .map_err(|error| CoreError::internal(format!("cannot encode settings: {error}")))?;
    transaction
        .execute(
            "INSERT INTO app_settings (key, value_json) VALUES ('application', ?1)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
            [settings_json],
        )
        .map_err(storage_db_error)?;
    Ok(())
}

pub(super) fn load_provider_selection_revision(connection: &Connection) -> CoreResult<u64> {
    let revision = connection
        .query_row(
            "SELECT revision
             FROM provider_selection_state
             WHERE singleton_key = 'application'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("provider selection revision is missing"))?;
    i64_to_u64("provider selection revision", revision)
}

pub(super) fn advance_provider_selection_revision(
    transaction: &rusqlite::Transaction<'_>,
) -> CoreResult<u64> {
    let changed = transaction
        .execute(
            "UPDATE provider_selection_state
             SET revision = revision + 1
             WHERE singleton_key = 'application'
               AND revision < 9223372036854775807",
            [],
        )
        .map_err(storage_db_error)?;
    if changed != 1 {
        return Err(storage_corrupted(
            "provider selection revision is missing or exhausted",
        ));
    }
    load_provider_selection_revision(transaction)
}

pub(crate) fn clear_provider_selections_for_connection(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<()> {
    if clear_provider_selections_for_connection_without_revision(transaction, connection_id)? {
        advance_provider_selection_revision(transaction)?;
    }
    Ok(())
}

/// Clears a selection owned by the graph currently being compensated and
/// returns the exact durable revision produced by that internal clear.
///
/// `None` means graph removal did not change the selection, so a later restore
/// has no authority to treat an already-clear value as its own effect.
pub(crate) fn clear_provider_selections_for_discovery_compensation(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<Option<u64>> {
    if clear_provider_selections_for_connection_without_revision(transaction, connection_id)? {
        return advance_provider_selection_revision(transaction).map(Some);
    }
    Ok(None)
}

fn clear_provider_selections_for_connection_without_revision(
    transaction: &rusqlite::Transaction<'_>,
    connection_id: &str,
) -> CoreResult<bool> {
    update_stored_settings_without_selection_revision(transaction, |settings| {
        let selected_route_belongs =
            if let Some(route_id) = settings.selected_model_route_id.as_ref() {
                transaction
                    .query_row(
                        "SELECT EXISTS(
                           SELECT 1 FROM provider_models
                           WHERE id = ?1 AND connection_id = ?2
                         )",
                        params![route_id.as_str(), connection_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(storage_db_error)?
            } else {
                false
            };
        if settings.selected_provider_profile_id.as_deref() == Some(connection_id)
            || selected_route_belongs
        {
            settings.selected_provider_profile_id = None;
            settings.selected_model_route_id = None;
            settings.selected_generation_preset_id = None;
        }
        Ok(())
    })
}

pub(crate) fn load_discovery_previous_selection(
    connection: &Connection,
) -> CoreResult<DiscoveryPreviousSelection> {
    let settings_json = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    let settings = settings_json.map_or_else(
        || Ok(AppSettings::default()),
        |json| {
            serde_json::from_str::<AppSettings>(&json)
                .map_err(|error| storage_corrupted(format!("stored settings are invalid: {error}")))
        },
    )?;
    let (route_id, preset_id, selected_provider_profile_id) = match (
        settings.selected_model_route_id,
        settings.selected_generation_preset_id,
        settings.selected_provider_profile_id,
    ) {
        (Some(route_id), Some(preset_id), profile_id) => (route_id, preset_id, profile_id),
        (None, None, Some(profile_id)) => (
            ModelRouteId::from(profile_id.clone()),
            GenerationPresetId::from(profile_id.clone()),
            Some(profile_id),
        ),
        (None, None, None) => return Ok(DiscoveryPreviousSelection::None),
        _ => {
            return Err(storage_corrupted(
                "stored provider route and preset selection are incomplete",
            ));
        }
    };
    let preset_route_id = connection
        .query_row(
            "SELECT model_route_id
             FROM generation_presets
             WHERE id = ?1",
            [preset_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .ok_or_else(|| storage_corrupted("selected generation preset is missing"))?;
    if preset_route_id != route_id.as_str()
        || !row_exists(
            connection,
            "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
            route_id.as_str(),
        )?
    {
        return Err(storage_corrupted(
            "stored provider route and preset selection do not match",
        ));
    }
    if let Some(profile_id) = selected_provider_profile_id.as_deref()
        && legacy_profile_current_route_id_for_schema(connection, profile_id, false)? != route_id
    {
        return Err(storage_corrupted(
            "stored legacy provider profile does not own its selected route",
        ));
    }
    Ok(DiscoveryPreviousSelection::RouteAndPreset {
        selected_provider_profile_id,
        model_route_id: route_id,
        generation_preset_id: preset_id,
    })
}

pub(crate) fn restore_discovery_provider_selection(
    transaction: &rusqlite::Transaction<'_>,
    previous_selection: &DiscoveryPreviousSelection,
    expected_selection_revision: Option<u64>,
) -> CoreResult<()> {
    let Some(expected_selection_revision) = expected_selection_revision else {
        return Ok(());
    };
    if load_provider_selection_revision(transaction)? != expected_selection_revision {
        // A later user or CRUD selection intent wins. Compensation still
        // completes because preserving that newer intent is the safe outcome.
        return Ok(());
    }
    if !matches!(previous_selection, DiscoveryPreviousSelection::None)
        && !row_exists(
            transaction,
            "SELECT EXISTS(
                 SELECT 1 FROM app_settings WHERE key = ?1
             )",
            "application",
        )?
    {
        return Err(CoreError::invalid(
            "previous discovery selection cannot be restored because settings are missing",
        ));
    }
    update_stored_settings_without_selection_revision(transaction, |settings| {
        let selection_is_clear = settings.selected_provider_profile_id.is_none()
            && settings.selected_model_route_id.is_none()
            && settings.selected_generation_preset_id.is_none();
        if !selection_is_clear {
            return Err(storage_corrupted(
                "discovery selection restore authority points to a non-clear selection",
            ));
        }
        match previous_selection {
            DiscoveryPreviousSelection::None => {}
            DiscoveryPreviousSelection::RouteAndPreset {
                selected_provider_profile_id,
                model_route_id,
                generation_preset_id,
            } => {
                let route_exists = row_exists(
                    transaction,
                    "SELECT EXISTS(SELECT 1 FROM provider_models WHERE id = ?1)",
                    model_route_id.as_str(),
                )?;
                if !route_exists {
                    return Err(CoreError::invalid(
                        "previous discovery model route no longer exists",
                    ));
                }
                let preset_matches = transaction
                    .query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM generation_presets
                             WHERE id = ?1 AND model_route_id = ?2
                         )",
                        params![generation_preset_id.as_str(), model_route_id.as_str(),],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(storage_db_error)?;
                if !preset_matches {
                    return Err(CoreError::invalid(
                        "previous discovery generation preset no longer matches its route",
                    ));
                }
                let legacy_profile_id = if let Some(profile_id) = selected_provider_profile_id {
                    let current_route_id =
                        legacy_profile_current_route_id_for_schema(transaction, profile_id, false)?;
                    if current_route_id != *model_route_id {
                        return Err(CoreError::invalid(
                            "previous discovery legacy profile no longer owns its selected route",
                        ));
                    }
                    Some(profile_id.clone())
                } else {
                    (model_route_id.as_str() == generation_preset_id.as_str()
                        && row_exists(
                            transaction,
                            "SELECT EXISTS(SELECT 1 FROM provider_profiles WHERE id = ?1)",
                            model_route_id.as_str(),
                        )?)
                    .then(|| model_route_id.as_str().to_owned())
                };
                let already_restored = settings.selected_provider_profile_id == legacy_profile_id
                    && settings.selected_model_route_id.as_ref() == Some(model_route_id)
                    && settings.selected_generation_preset_id.as_ref()
                        == Some(generation_preset_id);
                if already_restored {
                    return Ok(());
                }
                settings.selected_provider_profile_id = legacy_profile_id;
                settings.selected_model_route_id = Some(model_route_id.clone());
                settings.selected_generation_preset_id = Some(generation_preset_id.clone());
            }
        }
        Ok(())
    })?;
    // Consume the CAS authority even when the previous value was already None.
    // This makes a replay observe a revision mismatch instead of reusing the
    // graph-removal decision.
    advance_provider_selection_revision(transaction)?;
    Ok(())
}

pub(super) fn load_recovery_settings(connection: &Connection) -> CoreResult<AppSettings> {
    let settings = connection
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?
        .map_or_else(
            || Ok(AppSettings::default()),
            |value| {
                serde_json::from_str::<AppSettings>(&value).map_err(|error| {
                    CoreError::new(
                        CoreErrorCode::StorageCorrupted,
                        format!("stored settings are invalid: {error}"),
                        false,
                    )
                })
            },
        )?;
    validate_local_user_id(&settings.local_user_id)?;
    Ok(settings)
}

pub(super) fn ensure_stable_local_user_settings(connection: &mut Connection) -> CoreResult<()> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage_db_error)?;
    let _ = load_or_create_settings_in_transaction(&transaction)?;
    transaction.commit().map_err(storage_db_error)
}

fn load_or_create_settings_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
) -> CoreResult<AppSettings> {
    let stored_json = transaction
        .query_row(
            "SELECT value_json FROM app_settings WHERE key = 'application'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage_db_error)?;
    let (settings, requires_rewrite) = match stored_json {
        None => (AppSettings::default(), true),
        Some(json) => {
            let raw = serde_json::from_str::<serde_json::Value>(&json).map_err(|error| {
                storage_corrupted(format!("stored settings are invalid JSON: {error}"))
            })?;
            let object = raw.as_object().ok_or_else(|| {
                storage_corrupted("stored application settings must be a JSON object")
            })?;
            let requires_rewrite = !object.contains_key("local_user_id");
            let settings = serde_json::from_value::<AppSettings>(raw).map_err(|error| {
                storage_corrupted(format!("stored settings are invalid: {error}"))
            })?;
            (settings, requires_rewrite)
        }
    };
    validate_local_user_id(&settings.local_user_id)?;
    if requires_rewrite {
        let json = serde_json::to_string(&settings)
            .map_err(|error| CoreError::internal(format!("cannot encode settings: {error}")))?;
        transaction
            .execute(
                "INSERT INTO app_settings (key, value_json)
                 VALUES ('application', ?1)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                [json],
            )
            .map_err(storage_db_error)?;
    }
    Ok(settings)
}

fn validate_local_user_id(local_user_id: &LocalUserId) -> CoreResult<()> {
    let parsed = Uuid::parse_str(local_user_id.as_str())
        .map_err(|_| storage_corrupted("stored local user id is not a canonical UUID"))?;
    if parsed.get_version_num() != 4
        || parsed.hyphenated().to_string() != local_user_id.as_str().to_ascii_lowercase()
    {
        return Err(storage_corrupted(
            "stored local user id must be a canonical UUID v4",
        ));
    }
    Ok(())
}
