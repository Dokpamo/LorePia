fn version_three_provider_database(root: &std::path::Path) -> Connection {
    fs::create_dir_all(root.join("db")).expect("db directory");
    let connection = Connection::open(root.join("db/lorepia.sqlite3")).expect("legacy database");
    connection
        .pragma_update(None, "foreign_keys", true)
        .expect("foreign keys");
    connection
        .execute_batch(MIGRATION_0001)
        .expect("initial schema");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?1)",
            ["2026-01-01T00:00:00Z"],
        )
        .expect("version one");
    connection
        .execute_batch(MIGRATION_0002)
        .expect("second migration");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (2, ?1)",
            ["2026-01-01T00:00:01Z"],
        )
        .expect("version two");
    connection
        .execute_batch(MIGRATION_0003)
        .expect("third migration");
    connection
        .execute(
            "INSERT INTO schema_migrations(version, applied_at) VALUES (3, ?1)",
            ["2026-01-01T00:00:02Z"],
        )
        .expect("version three");
    connection
}

fn insert_legacy_provider_profile(connection: &Connection, profile: (&str, &str, &str, &str, i64)) {
    connection
        .execute(
            "INSERT INTO provider_profiles
                 (id, display_name, base_url, model, timeout_seconds)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            params![profile.0, profile.1, profile.2, profile.3, profile.4],
        )
        .expect("legacy provider profile");
}

#[test]
#[allow(clippy::too_many_lines)]
fn v3_provider_catalog_migrates_to_v11_with_profiles_selection_and_base_paths() {
    let root = tempdir().expect("temp root");
    let connection = version_three_provider_database(root.path());
    insert_legacy_provider_profile(
        &connection,
        (
            "remote",
            "Remote",
            "https://api.example.test/openai/v1",
            "remote-model",
            45,
        ),
    );
    insert_legacy_provider_profile(
        &connection,
        (
            "local",
            "Local",
            "http://127.0.0.1:11434/v1",
            "local-model",
            30,
        ),
    );
    connection
        .execute(
            "INSERT INTO app_settings(key, value_json)
                 VALUES ('application', ?1)",
            [r#"{"preserve_partial_generations":false,"selected_provider_profile_id":"remote"}"#],
        )
        .expect("legacy settings");
    drop(connection);

    let storage = Storage::open(root.path()).expect("migrate provider catalog");
    assert_eq!(
        storage
            .schema_version()
            .expect("read durable schema version"),
        SCHEMA_VERSION
    );
    let templates = storage.list_provider_templates().expect("templates");
    assert_eq!(
        templates,
        vec![legacy_provider_template().expect("template")]
    );
    let stored_template = storage
        .get_provider_template(
            &ProviderTemplateId::from(LEGACY_PROVIDER_TEMPLATE_ID),
            LEGACY_PROVIDER_TEMPLATE_VERSION,
        )
        .expect("built-in template");
    assert_eq!(stored_template.source, TemplateSource::BuiltIn);

    let connections = storage.list_provider_connections().expect("connections");
    assert_eq!(connections.len(), 2);
    let remote = storage
        .get_provider_connection(&ProviderConnectionId::from("remote"))
        .expect("remote connection");
    assert_eq!(remote.api_origin.as_str(), "https://api.example.test");
    assert_eq!(remote.config.network_mode, ProviderNetworkMode::Public);
    assert_eq!(
        remote
            .config
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/openai/v1")
    );
    assert_eq!(
        remote.config.values,
        vec![ConnectionConfigEntry {
            key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
            value: ConnectionConfigValue::Text("https://api.example.test/openai/v1".to_owned()),
        }]
    );
    assert_eq!(
        remote.credential_ref.as_ref().map(CredentialRef::as_str),
        Some("remote")
    );
    assert_eq!(
        remote
            .credential_scope
            .as_ref()
            .expect("credential scope")
            .allowed_origins,
        vec![CanonicalOrigin::parse("https://api.example.test").expect("origin")]
    );
    assert_eq!(
        storage
            .get_provider_connection(&ProviderConnectionId::from("local"))
            .expect("local connection")
            .config
            .network_mode,
        ProviderNetworkMode::LocalLoopback
    );

    let routes = storage
        .list_model_routes(&ProviderConnectionId::from("remote"))
        .expect("routes");
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].id.as_str(), "remote");
    assert_eq!(routes[0].model_id, "remote-model");
    assert_eq!(routes[0].status, ModelAvailability::Available);
    let presets = storage
        .list_generation_presets(&ModelRouteId::from("remote"))
        .expect("presets");
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].id.as_str(), "remote");
    assert_eq!(
        presets[0].values,
        vec![
            ParameterValue {
                parameter_id: ParameterId::from(TEMPERATURE_PARAMETER_ID),
                state: ParameterValueState::Explicit(ParameterLiteral::Number(1.0)),
            },
            ParameterValue {
                parameter_id: ParameterId::from(MAX_OUTPUT_TOKENS_PARAMETER_ID),
                state: ParameterValueState::Explicit(ParameterLiteral::Integer(4096)),
            },
        ]
    );
    let settings = storage.load_settings().expect("migrated settings");
    assert_eq!(
        settings.selected_provider_profile_id.as_deref(),
        Some("remote")
    );
    assert_eq!(
        settings
            .selected_model_route_id
            .as_ref()
            .map(ModelRouteId::as_str),
        Some("remote")
    );
    assert_eq!(
        settings
            .selected_generation_preset_id
            .as_ref()
            .map(GenerationPresetId::as_str),
        Some("remote")
    );
    {
        let connection = storage.connection().expect("connection");
        assert_eq!(
            count(&connection, "provider_profiles").expect("profiles"),
            2
        );
        let (manifest_json, manifest_sha256) = connection
            .query_row(
                "SELECT manifest_json, manifest_sha256
                     FROM provider_templates WHERE id = ?1 AND version = ?2",
                params![
                    LEGACY_PROVIDER_TEMPLATE_ID,
                    LEGACY_PROVIDER_TEMPLATE_VERSION
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("manifest and hash");
        assert_eq!(
            manifest_sha256,
            hex::encode(Sha256::digest(manifest_json.as_bytes()))
        );
        validate_provider_catalog_foreign_keys(&connection).expect("foreign keys");
    }
    let before_reopen = (
        storage.list_provider_connections().expect("connections"),
        storage
            .list_model_routes(&ProviderConnectionId::from("remote"))
            .expect("routes"),
        storage
            .list_generation_presets(&ModelRouteId::from("remote"))
            .expect("presets"),
        storage.load_settings().expect("settings"),
    );
    drop(storage);

    let reopened = Storage::open(root.path()).expect("reopen migrated catalog");
    assert_eq!(
        (
            reopened
                .list_provider_connections()
                .expect("reopened connections"),
            reopened
                .list_model_routes(&ProviderConnectionId::from("remote"))
                .expect("reopened routes"),
            reopened
                .list_generation_presets(&ModelRouteId::from("remote"))
                .expect("reopened presets"),
            reopened.load_settings().expect("reopened settings"),
        ),
        before_reopen
    );
}

#[test]
fn empty_provider_catalog_migration_seeds_only_the_builtin_template() {
    let root = tempdir().expect("temp root");
    drop(version_three_provider_database(root.path()));

    let storage = Storage::open(root.path()).expect("migrate empty database");
    assert_eq!(
        storage.list_provider_templates().expect("templates").len(),
        1
    );
    assert!(
        storage
            .list_provider_connections()
            .expect("connections")
            .is_empty()
    );
    let settings = storage.load_settings().expect("settings");
    assert!(!settings.local_user_id.as_str().is_empty());
    assert!(settings.preserve_partial_generations);
    assert!(settings.selected_provider_profile_id.is_none());
    assert!(settings.selected_model_route_id.is_none());
    assert!(settings.selected_generation_preset_id.is_none());
    let local_user_id = settings.local_user_id;
    drop(storage);
    assert_eq!(
        Storage::open(root.path())
            .expect("reopen migrated empty database")
            .load_settings()
            .expect("reopened settings")
            .local_user_id,
        local_user_id,
        "first-load local identity must be persisted instead of regenerated"
    );
}

#[test]
fn provider_template_versions_are_hashed_idempotent_and_immutable() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let mut template = legacy_provider_template().expect("template");
    template.id = ProviderTemplateId::from("user-template");
    template.display_name = "User template".to_owned();
    template.source = TemplateSource::UserDiscovered;

    storage
        .save_provider_template(&template)
        .expect("save template");
    storage
        .save_provider_template(&template)
        .expect("idempotent save");
    assert_eq!(
        storage
            .get_provider_template(&template.id, template.manifest_version)
            .expect("roundtrip template"),
        template
    );
    assert_eq!(
        storage
            .connection()
            .expect("connection")
            .query_row(
                "SELECT COUNT(*) FROM provider_templates
                     WHERE id = 'user-template' AND version = 1",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("template count"),
        1
    );

    let mut conflicting = template.clone();
    conflicting.display_name = "Conflicting payload".to_owned();
    let error = storage
        .save_provider_template(&conflicting)
        .expect_err("same version must be immutable");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_provider_template(&template.id, template.manifest_version)
            .expect("unchanged template"),
        template
    );

    let mut next_version = conflicting;
    next_version.manifest_version = 2;
    storage
        .save_provider_template(&next_version)
        .expect("save next version");
    assert_eq!(
        storage
            .get_provider_template(&next_version.id, 2)
            .expect("next version"),
        next_version
    );
    let connection = storage.connection().expect("connection");
    let (manifest_json, manifest_sha256) = connection
        .query_row(
            "SELECT manifest_json, manifest_sha256
                 FROM provider_templates
                 WHERE id = 'user-template' AND version = 2",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .expect("stored template");
    assert_eq!(
        manifest_sha256,
        hex::encode(Sha256::digest(manifest_json.as_bytes()))
    );
}

#[test]
fn invalid_legacy_provider_catalog_data_rolls_back_schema_four() {
    for fixture in ["remote-http", "invalid-timeout", "dangling-selection"] {
        let root = tempdir().expect("temp root");
        let connection = version_three_provider_database(root.path());
        match fixture {
            "remote-http" => insert_legacy_provider_profile(
                &connection,
                (
                    "invalid",
                    "Invalid",
                    "http://api.example.test/v1",
                    "model",
                    30,
                ),
            ),
            "invalid-timeout" => insert_legacy_provider_profile(
                &connection,
                (
                    "invalid",
                    "Invalid",
                    "https://api.example.test/v1",
                    "model",
                    0,
                ),
            ),
            "dangling-selection" => {
                connection
                        .execute(
                            "INSERT INTO app_settings(key, value_json)
                             VALUES ('application', ?1)",
                            [
                                r#"{"preserve_partial_generations":true,"selected_provider_profile_id":"missing"}"#,
                            ],
                        )
                        .expect("dangling settings");
            }
            _ => unreachable!(),
        }
        drop(connection);

        let Err(error) = Storage::open(root.path()) else {
            panic!("{fixture} must fail migration");
        };
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted, "{fixture}");
        let connection =
            Connection::open(root.path().join("db/lorepia.sqlite3")).expect("database");
        assert_eq!(
            connection
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get::<_, u32>(0)
                })
                .expect("schema version"),
            3,
            "{fixture}"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'table' AND name = 'provider_templates'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("provider template table"),
            0,
            "{fixture}"
        );
    }
}

#[test]
fn provider_catalog_integrity_checks_detect_row_and_foreign_key_mismatches() {
    let mut connection = Connection::open_in_memory().expect("database");
    connection
        .execute_batch(MIGRATION_0001)
        .expect("initial schema");
    connection
        .execute_batch(MIGRATION_0004)
        .expect("provider schema");
    {
        let transaction = connection.transaction().expect("transaction");
        insert_legacy_provider_template(&transaction).expect("template");
        transaction
            .execute(
                "INSERT INTO provider_profiles
                     (id, display_name, base_url, model, timeout_seconds)
                     VALUES ('unmigrated', 'Unmigrated', 'https://api.example.test/v1',
                             'model', 30)",
                [],
            )
            .expect("legacy profile");
        let error =
            validate_provider_catalog_migration(&transaction).expect_err("row mismatch must fail");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    }

    connection
        .pragma_update(None, "foreign_keys", false)
        .expect("disable enforcement for corruption fixture");
    connection
        .execute(
            "INSERT INTO provider_models
                 (id, connection_id, api_family, model_id, display_name, route_json,
                  availability, raw_metadata_json, first_seen_at, last_seen_at)
                 VALUES ('orphan', 'missing', 'openai_chat_completions', 'model',
                         NULL, '{}', 'available', NULL, ?1, NULL)",
            ["2026-01-01T00:00:00Z"],
        )
        .expect("orphan fixture");
    let error = validate_provider_catalog_foreign_keys(&connection)
        .expect_err("foreign key mismatch must fail");
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}
