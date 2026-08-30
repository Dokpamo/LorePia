fn raw_legacy_provider_identity_rows(storage: &Storage, id: &str) -> (String, String) {
    storage
        .connection()
        .expect("database")
        .query_row(
            "SELECT
                   hex(CAST(json_array(
                     profile.id, profile.display_name, profile.base_url,
                     profile.model, profile.timeout_seconds
                   ) AS BLOB)),
                   hex(CAST(json_array(
                     connection.id, connection.template_id, connection.template_version,
                     connection.display_name, connection.api_origin, connection.config_json,
                     connection.credential_ref, connection.credential_scope_json,
                     connection.timeout_seconds, connection.status,
                     connection.created_at, connection.updated_at, connection.archived_at
                   ) AS BLOB))
                 FROM provider_profiles AS profile
                 JOIN provider_connections AS connection ON connection.id = profile.id
                 WHERE profile.id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("raw legacy provider rows")
}

#[test]
fn provider_connection_catalog_state_compare_and_swap_rejects_stale_review() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let mut template = legacy_provider_template().expect("template");
    template.id = ProviderTemplateId::from("signed-catalog-cas-template");
    template.manifest_version = 7;
    template.source = TemplateSource::SignedCatalog;

    let profile = ProviderProfile {
        id: "catalog-cas-connection".to_owned(),
        display_name: "Catalog CAS".to_owned(),
        base_url: "https://api.example.test/v1".to_owned(),
        model: "model".to_owned(),
        timeout_seconds: 30,
    };
    let (mut connection, _, _) = legacy_provider_graph(&profile, Utc::now()).expect("connection");
    connection.template_id = template.id.clone();
    connection.template_version = template.manifest_version;
    storage
        .insert_provider_connection_for_catalog_state(&connection, &template, 0)
        .expect("save against reviewed state");

    let mut duplicate = connection.clone();
    duplicate.display_name = "Retargeted duplicate".to_owned();
    let duplicate_error = storage
        .insert_provider_connection_for_catalog_state(&duplicate, &template, 0)
        .expect_err("catalog create must not overwrite an occupied connection ID");
    assert_eq!(duplicate_error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_provider_connection(&connection.id)
            .expect("original catalog connection"),
        connection
    );

    storage
        .connection()
        .expect("database")
        .execute(
            "UPDATE provider_catalog_state
                 SET state_version = 1, updated_at = ?1
                 WHERE singleton = 1",
            [Utc::now().to_rfc3339()],
        )
        .expect("advance catalog state");
    connection.id = ProviderConnectionId::from("catalog-cas-stale");
    connection.credential_ref = Some(CredentialRef("catalog-cas-stale".to_owned()));
    let error = storage
        .insert_provider_connection_for_catalog_state(&connection, &template, 0)
        .expect_err("stale catalog review must fail");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        storage.get_provider_connection(&connection.id).is_err(),
        "stale connection must not be inserted"
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn provider_profile_writes_dual_write_the_catalog_atomically() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let original = ProviderProfile {
        id: "dual".to_owned(),
        display_name: "Original".to_owned(),
        base_url: "https://api.example.test/v1".to_owned(),
        model: "model-one".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&original)
        .expect("save original");
    assert_eq!(
        storage
            .get_provider_connection(&ProviderConnectionId::from("dual"))
            .expect("connection")
            .display_name,
        "Original"
    );
    assert_eq!(
        storage
            .get_model_route(&ModelRouteId::from("dual"))
            .expect("route")
            .model_id,
        "model-one"
    );
    assert_eq!(
        storage
            .get_generation_preset(&GenerationPresetId::from("dual"))
            .expect("preset")
            .values
            .len(),
        2
    );

    let raw_identity_before = raw_legacy_provider_identity_rows(&storage, "dual");
    let endpoint_mutation = ProviderProfile {
        base_url: "https://api.example.test/openai/v2".to_owned(),
        ..original.clone()
    };
    let error = storage
        .save_provider_profile(&endpoint_mutation)
        .expect_err("stable legacy connection ID must not retarget its endpoint");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        error.message.contains("create a new connection"),
        "endpoint mutation error must direct callers to a new identity"
    );
    assert_eq!(
        storage
            .get_provider_profile("dual")
            .expect("profile after rejected endpoint mutation"),
        original
    );
    assert_eq!(
        storage
            .list_model_routes(&ProviderConnectionId::from("dual"))
            .expect("routes after rejected endpoint mutation")
            .len(),
        1
    );
    assert_eq!(
        raw_legacy_provider_identity_rows(&storage, "dual"),
        raw_identity_before,
        "the rejected endpoint mutation must leave both identity rows byte-exact"
    );

    drop(storage);
    let storage = Storage::open(root.path()).expect("reopen after rejected endpoint mutation");
    assert_eq!(
        raw_legacy_provider_identity_rows(&storage, "dual"),
        raw_identity_before,
        "the exact identity rows must remain unchanged after rollback and reopen"
    );
    assert_eq!(
        storage
            .get_provider_profile("dual")
            .expect("profile after rejected endpoint mutation and reopen"),
        original
    );

    storage
        .connection()
        .expect("connection")
        .execute_batch(
            "CREATE TEMP TRIGGER reject_connection_update
                 BEFORE UPDATE ON provider_connections
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic catalog failure');
                 END;",
        )
        .expect("failure trigger");

    let updated = ProviderProfile {
        display_name: "Updated".to_owned(),
        model: "model-two".to_owned(),
        timeout_seconds: 60,
        ..original.clone()
    };
    let error = storage
        .save_provider_profile(&updated)
        .expect_err("dual write must roll back");
    assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
    assert_eq!(
        storage
            .get_provider_profile("dual")
            .expect("legacy profile after rollback"),
        original
    );
    assert_eq!(
        storage
            .get_model_route(&ModelRouteId::from("dual"))
            .expect("route after rollback")
            .model_id,
        "model-one"
    );
    storage
        .connection()
        .expect("connection")
        .execute_batch("DROP TRIGGER reject_connection_update;")
        .expect("drop failure trigger");

    storage
        .save_provider_profile(&updated)
        .expect("save updated profile");
    let connection = storage
        .get_provider_connection(&ProviderConnectionId::from("dual"))
        .expect("updated connection");
    assert_eq!(connection.display_name, "Updated");
    assert_eq!(
        connection
            .config
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/v1")
    );
    assert_eq!(
        storage
            .get_model_route(&ModelRouteId::from("dual"))
            .expect("preserved original route")
            .model_id,
        "model-one"
    );
    assert_eq!(
        storage
            .get_generation_preset(&GenerationPresetId::from("dual"))
            .expect("preserved original preset")
            .values
            .len(),
        2
    );
    let updated_route = storage
        .list_model_routes(&ProviderConnectionId::from("dual"))
        .expect("updated model routes")
        .into_iter()
        .find(|route| route.model_id == "model-two")
        .expect("new stable route for updated model");
    assert_ne!(updated_route.id.as_str(), "dual");
    let updated_preset_id = GenerationPresetId::from(updated_route.id.as_str());
    assert_eq!(
        storage
            .get_generation_preset(&updated_preset_id)
            .expect("new model preset")
            .model_route_id,
        updated_route.id.clone()
    );

    storage
        .save_settings(&AppSettings {
            local_user_id: LocalUserId::default(),
            preserve_partial_generations: true,
            selected_provider_profile_id: Some("dual".to_owned()),
            selected_model_route_id: None,
            selected_generation_preset_id: None,
        })
        .expect("dual-write selection");
    let settings = storage.load_settings().expect("settings");
    assert_eq!(
        settings
            .selected_model_route_id
            .as_ref()
            .map(ModelRouteId::as_str),
        Some(updated_route.id.as_str())
    );
    assert_eq!(
        settings
            .selected_generation_preset_id
            .as_ref()
            .map(GenerationPresetId::as_str),
        Some(updated_preset_id.as_str())
    );

    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen stable legacy routes");
    assert_eq!(
        reopened
            .get_model_route(&ModelRouteId::from("dual"))
            .expect("original route after reopen")
            .model_id,
        "model-one"
    );
    assert_eq!(
        reopened
            .get_model_route(&updated_route.id)
            .expect("updated route after reopen")
            .model_id,
        "model-two"
    );
    reopened
        .save_provider_profile(&updated)
        .expect("idempotently reuse updated route");
    assert_eq!(
        reopened
            .list_model_routes(&ProviderConnectionId::from("dual"))
            .expect("routes after idempotent update")
            .len(),
        2
    );
}

#[test]
fn discovery_compensation_restores_a_retained_legacy_profile_on_its_sibling_route() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let original = ProviderProfile {
        id: "legacy-discovery-compensation".to_owned(),
        display_name: "Legacy discovery compensation".to_owned(),
        base_url: "https://api.example.test/v1".to_owned(),
        model: "model-one".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&original)
        .expect("save original legacy profile");
    storage
        .save_provider_profile(&ProviderProfile {
            model: "model-two".to_owned(),
            ..original.clone()
        })
        .expect("move legacy profile to a sibling route");
    storage
        .save_settings(&AppSettings {
            selected_provider_profile_id: Some(original.id.clone()),
            ..AppSettings::default()
        })
        .expect("select retained legacy profile");

    let selected = storage
        .load_settings()
        .expect("normalized legacy selection");
    let selected_route = selected
        .selected_model_route_id
        .clone()
        .expect("selected sibling route");
    let selected_preset = selected
        .selected_generation_preset_id
        .clone()
        .expect("selected sibling preset");
    assert_ne!(selected_route.as_str(), original.id);
    let previous = storage
        .current_discovery_previous_selection()
        .expect("capture discovery previous selection");
    assert_eq!(
        previous,
        DiscoveryPreviousSelection::RouteAndPreset {
            selected_provider_profile_id: Some(original.id.clone()),
            model_route_id: selected_route.clone(),
            generation_preset_id: selected_preset.clone(),
        }
    );

    let expected_selection_revision = {
        let mut connection = storage.connection().expect("graph-clear connection");
        let transaction = connection.transaction().expect("graph-clear transaction");
        let revision = clear_provider_selections_for_discovery_compensation(
            &transaction,
            original.id.as_str(),
        )
        .expect("clear graph-owned legacy selection")
        .expect("legacy graph clear changed the selected target");
        transaction.commit().expect("commit graph-owned clear");
        revision
    };
    {
        let mut connection = storage.connection().expect("compensation connection");
        let transaction = connection.transaction().expect("compensation transaction");
        restore_discovery_provider_selection(
            &transaction,
            &previous,
            Some(expected_selection_revision),
        )
        .expect("restore exact legacy selection");
        transaction.commit().expect("commit compensation");
    }

    let restored = storage.load_settings().expect("restored settings");
    assert_eq!(
        restored.selected_provider_profile_id.as_deref(),
        Some(original.id.as_str())
    );
    assert_eq!(restored.selected_model_route_id, Some(selected_route));
    assert_eq!(
        restored.selected_generation_preset_id,
        Some(selected_preset)
    );
}

#[test]
fn discovery_graph_clear_cannot_overwrite_a_later_explicit_clear() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let previous_profile = ProviderProfile {
        id: "selection-before-discovery".to_owned(),
        display_name: "Selection before discovery".to_owned(),
        base_url: "https://previous.example.test/v1".to_owned(),
        model: "previous-model".to_owned(),
        timeout_seconds: 30,
    };
    let discovered_profile = ProviderProfile {
        id: "selection-owned-by-discovery".to_owned(),
        display_name: "Selection owned by discovery".to_owned(),
        base_url: "https://discovered.example.test/v1".to_owned(),
        model: "discovered-model".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&previous_profile)
        .expect("save previous provider graph");
    storage
        .save_provider_profile(&discovered_profile)
        .expect("save discovered provider graph");
    storage
        .save_settings(&AppSettings {
            selected_provider_profile_id: Some(previous_profile.id.clone()),
            ..AppSettings::default()
        })
        .expect("select provider that predates discovery");
    let previous_selection = storage
        .current_discovery_previous_selection()
        .expect("capture pre-discovery selection");
    storage
        .save_settings(&AppSettings {
            selected_provider_profile_id: Some(discovered_profile.id.clone()),
            ..AppSettings::default()
        })
        .expect("select graph later owned by discovery");

    let graph_clear_committed = Arc::new(Barrier::new(2));
    let explicit_clear_committed = Arc::new(Barrier::new(2));
    let discovered_profile_id = discovered_profile.id.clone();
    let storage_ref = &storage;
    thread::scope(|scope| {
        let graph_thread_clear = Arc::clone(&graph_clear_committed);
        let graph_thread_explicit = Arc::clone(&explicit_clear_committed);
        let previous_selection = previous_selection.clone();
        scope.spawn(move || {
            let expected_selection_revision = {
                let mut connection = storage_ref.connection().expect("graph-clear connection");
                let transaction = connection.transaction().expect("graph-clear transaction");
                let revision = clear_provider_selections_for_discovery_compensation(
                    &transaction,
                    discovered_profile_id.as_str(),
                )
                .expect("clear selection owned by removed discovery graph")
                .expect("discovery graph clear changed the selected target");
                transaction.commit().expect("commit graph-owned clear");
                revision
            };
            graph_thread_clear.wait();
            graph_thread_explicit.wait();
            let mut connection = storage_ref
                .connection()
                .expect("selection-restore connection");
            let transaction = connection
                .transaction()
                .expect("selection-restore transaction");
            restore_discovery_provider_selection(
                &transaction,
                &previous_selection,
                Some(expected_selection_revision),
            )
            .expect("finish discovery selection compensation");
            transaction.commit().expect("commit selection restoration");
        });

        let user_thread_clear = Arc::clone(&graph_clear_committed);
        let user_thread_explicit = Arc::clone(&explicit_clear_committed);
        scope.spawn(move || {
            user_thread_clear.wait();
            storage_ref
                .save_settings(&AppSettings::default())
                .expect("persist explicit clear after graph removal");
            user_thread_explicit.wait();
        });
    });

    let final_settings = storage.load_settings().expect("load final settings");
    assert!(
        final_settings.selected_provider_profile_id.is_none()
            && final_settings.selected_model_route_id.is_none()
            && final_settings.selected_generation_preset_id.is_none(),
        "the latest explicit clear must win over stale discovery compensation"
    );
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the four serialized delete/profile-switch orders keep their graph assertions adjacent"
)]
fn migrated_legacy_graph_crud_is_atomic_across_profile_switch_orders() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let seed_sibling = |id: &str| {
        let model_a = ProviderProfile {
            id: id.to_owned(),
            display_name: format!("{id} A"),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "model-a".to_owned(),
            timeout_seconds: 30,
        };
        let model_b = ProviderProfile {
            display_name: format!("{id} B"),
            model: "model-b".to_owned(),
            ..model_a.clone()
        };
        storage
            .save_provider_profile(&model_a)
            .expect("seed legacy model A");
        storage
            .save_provider_profile(&model_b)
            .expect("create legacy model B sibling");
        let route_b = storage
            .list_model_routes(&ProviderConnectionId::from(id))
            .expect("legacy sibling routes")
            .into_iter()
            .find(|route| route.model_id == "model-b")
            .expect("legacy model B sibling");
        let preset_b = GenerationPresetId::from(route_b.id.as_str());
        storage
            .save_provider_profile(&model_a)
            .expect("make model A current again");
        (model_b, route_b, preset_b)
    };

    let (route_delete_first_profile, route_delete_first, route_delete_first_preset) =
        seed_sibling("legacy-route-delete-first");
    storage
        .delete_model_route(&route_delete_first.id)
        .expect("an old sibling route may be deleted before the profile switches to it");
    assert_eq!(
        storage
            .get_model_route(&route_delete_first.id)
            .expect_err("old sibling route was deleted")
            .code,
        CoreErrorCode::NotFound
    );
    storage
        .save_provider_profile(&route_delete_first_profile)
        .expect("profile switch recreates its missing current route atomically");
    storage
        .get_generation_preset(&route_delete_first_preset)
        .expect("profile switch recreates its default preset");

    let (route_switch_first_profile, route_switch_first, route_switch_first_preset) =
        seed_sibling("legacy-route-switch-first");
    storage
        .save_provider_profile(&route_switch_first_profile)
        .expect("switch profile before route delete");
    let route_error = storage
        .delete_model_route(&route_switch_first.id)
        .expect_err("the transaction revalidates the newly-current sibling route");
    assert_eq!(route_error.code, CoreErrorCode::InvalidInput);
    storage
        .get_model_route(&route_switch_first.id)
        .expect("current sibling route remains");
    storage
        .get_generation_preset(&route_switch_first_preset)
        .expect("current sibling preset remains");

    let (preset_delete_first_profile, preset_delete_first_route, preset_delete_first) =
        seed_sibling("legacy-preset-delete-first");
    storage
        .delete_generation_preset(&preset_delete_first)
        .expect("an old sibling preset may be deleted before the profile switches to it");
    storage
        .save_provider_profile(&preset_delete_first_profile)
        .expect("profile switch recreates its missing default preset atomically");
    storage
        .get_generation_preset(&preset_delete_first)
        .expect("current default preset was recreated");
    storage
        .get_model_route(&preset_delete_first_route.id)
        .expect("preset recreation preserves its route");

    let (preset_switch_first_profile, preset_switch_first_route, preset_switch_first) =
        seed_sibling("legacy-preset-switch-first");
    storage
        .save_provider_profile(&preset_switch_first_profile)
        .expect("switch profile before preset delete");
    let preset_error = storage
        .delete_generation_preset(&preset_switch_first)
        .expect_err("the transaction revalidates the newly-current sibling preset");
    assert_eq!(preset_error.code, CoreErrorCode::InvalidInput);
    storage
        .get_generation_preset(&preset_switch_first)
        .expect("current sibling preset remains");

    let mut route_update = preset_switch_first_route.clone();
    route_update.display_name = Some("ordinary mutation".to_owned());
    assert_eq!(
        storage
            .save_model_route(&route_update)
            .expect_err("ordinary route save revalidates the retained profile transaction")
            .code,
        CoreErrorCode::InvalidInput
    );
    let mut extra_preset = storage
        .get_generation_preset(&preset_switch_first)
        .expect("current preset for ordinary save rejection");
    extra_preset.id = GenerationPresetId::from("ordinary-legacy-alias-preset");
    assert_eq!(
        storage
            .save_generation_preset(&extra_preset)
            .expect_err("ordinary preset save revalidates the retained profile transaction")
            .code,
        CoreErrorCode::InvalidInput
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn provider_catalog_crud_roundtrips_and_rejects_secret_or_dangling_data() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let (mut connection, _, _) = save_modern_direct_provider_graph(
        &storage,
        &ProviderProfile {
            id: "catalog".to_owned(),
            display_name: "Catalog".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "default-model".to_owned(),
            timeout_seconds: 30,
        },
    );
    let mut duplicate_connection = connection.clone();
    duplicate_connection.display_name = "Duplicate overwrite".to_owned();
    let duplicate_error = storage
        .insert_provider_connection(&duplicate_connection)
        .expect_err("create must not overwrite an occupied connection ID");
    assert_eq!(duplicate_error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_provider_connection(&ProviderConnectionId::from("catalog"))
            .expect("connection after rejected duplicate"),
        connection
    );
    let mut retargeted_connection = connection.clone();
    retargeted_connection.config.api_base_path =
        Some(EndpointPath::parse("/v2").expect("retargeted base path"));
    retargeted_connection.config.values = vec![ConnectionConfigEntry {
        key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
        value: ConnectionConfigValue::Text("https://api.example.test/v2".to_owned()),
    }];
    let retarget_error = storage
        .save_provider_connection(&retargeted_connection)
        .expect_err("stable connection ID must not change endpoint config");
    assert_eq!(retarget_error.code, CoreErrorCode::InvalidInput);

    let mut rebound_connection = connection.clone();
    rebound_connection.credential_ref = Some(CredentialRef("other-vault-entry".to_owned()));
    let rebound_error = storage
        .save_provider_connection(&rebound_connection)
        .expect_err("stable connection ID must not change credential binding");
    assert_eq!(rebound_error.code, CoreErrorCode::InvalidInput);
    assert_eq!(
        storage
            .get_provider_connection(&ProviderConnectionId::from("catalog"))
            .expect("connection after rejected identity mutations"),
        connection
    );

    connection.status = ConnectionStatus::Connected;
    connection.updated_at = Utc::now();
    storage
        .save_provider_connection(&connection)
        .expect("update connection");
    assert_eq!(
        storage
            .get_provider_connection(&ProviderConnectionId::from("catalog"))
            .expect("roundtrip connection"),
        connection
    );
    let mut unsafe_connection = connection.clone();
    unsafe_connection.config.values.push(ConnectionConfigEntry {
        key: "api_key".to_owned(),
        value: ConnectionConfigValue::Text("must-not-be-persisted".to_owned()),
    });
    let error = storage
        .save_provider_connection(&unsafe_connection)
        .expect_err("secret-like config must be rejected");
    assert_eq!(error.code, CoreErrorCode::InvalidInput);
    assert!(
        storage
            .connection()
            .expect("connection")
            .query_row(
                "SELECT CAST(config_json AS TEXT) NOT LIKE '%must-not-be-persisted%'
                     FROM provider_connections WHERE id = 'catalog'",
                [],
                |row| row.get::<_, bool>(0),
            )
            .expect("secret absence")
    );

    let now = Utc::now();
    let route = ModelRoute {
        id: ModelRouteId::from("extra-route"),
        connection_id: ProviderConnectionId::from("catalog"),
        api_family: ApiFamily::OpenAiChatCompletions,
        model_id: "extra-model".to_owned(),
        display_name: Some("Extra model".to_owned()),
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::Legacy,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: now,
        last_seen_at: Some(now),
    };
    storage.save_model_route(&route).expect("save route");
    assert_eq!(
        storage
            .get_model_route(&ModelRouteId::from("extra-route"))
            .expect("roundtrip route"),
        route
    );
    let preset = GenerationPreset {
        id: GenerationPresetId::from("extra-preset"),
        model_route_id: route.id.clone(),
        display_name: "Creative".to_owned(),
        values: vec![ParameterValue {
            parameter_id: ParameterId::from(TEMPERATURE_PARAMETER_ID),
            state: ParameterValueState::Explicit(ParameterLiteral::Number(1.5)),
        }],
        reasoning: GenerationReasoningSettings::default(),
        prompt_cache: GenerationPromptCacheSettings::default(),
        created_at: now,
        updated_at: now,
    };
    storage
        .save_generation_preset(&preset)
        .expect("save preset");
    assert_eq!(
        storage
            .get_generation_preset(&GenerationPresetId::from("extra-preset"))
            .expect("roundtrip preset"),
        preset
    );
    assert_eq!(
        storage
            .list_generation_presets(&ModelRouteId::from("extra-route"))
            .expect("listed presets"),
        vec![preset.clone()]
    );
    storage
        .save_settings(&AppSettings {
            local_user_id: LocalUserId::default(),
            preserve_partial_generations: true,
            selected_provider_profile_id: None,
            selected_model_route_id: Some(route.id.clone()),
            selected_generation_preset_id: Some(preset.id.clone()),
        })
        .expect("select route and preset");
    storage
        .delete_generation_preset(&preset.id)
        .expect("delete preset");
    let settings = storage.load_settings().expect("settings after delete");
    assert!(settings.selected_model_route_id.is_none());
    assert!(settings.selected_generation_preset_id.is_none());
    storage.delete_model_route(&route.id).expect("delete route");

    let dangling = ModelRoute {
        id: ModelRouteId::from("dangling"),
        connection_id: ProviderConnectionId::from("missing"),
        ..route
    };
    let error = storage
        .save_model_route(&dangling)
        .expect_err("dangling connection must be rejected");
    assert_eq!(error.code, CoreErrorCode::NotFound);
}

#[test]
#[allow(clippy::too_many_lines)]
fn model_route_reconciliation_preserves_missing_rows_presets_and_rolls_back_atomically() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let (_, default_route, _) = save_modern_direct_provider_graph(
        &storage,
        &ProviderProfile {
            id: "sync".to_owned(),
            display_name: "Sync".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "default-model".to_owned(),
            timeout_seconds: 30,
        },
    );
    let connection_id = ProviderConnectionId::from("sync");
    let first_seen_at = Utc::now() - chrono::Duration::hours(1);
    let old_route = ModelRoute {
        id: ModelRouteId::from("old-route"),
        connection_id: connection_id.clone(),
        api_family: ApiFamily::OpenAiChatCompletions,
        model_id: "old-model".to_owned(),
        display_name: Some("Old model".to_owned()),
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Available,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::Legacy,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at,
        last_seen_at: Some(first_seen_at),
    };
    storage
        .save_model_route(&old_route)
        .expect("save old route");
    let old_preset = GenerationPreset {
        id: GenerationPresetId::from("old-preset"),
        model_route_id: old_route.id.clone(),
        display_name: "Old preset".to_owned(),
        values: Vec::new(),
        reasoning: GenerationReasoningSettings::default(),
        prompt_cache: GenerationPromptCacheSettings::default(),
        created_at: first_seen_at,
        updated_at: first_seen_at,
    };
    storage
        .save_generation_preset(&old_preset)
        .expect("save old preset");

    let observed_at = Utc::now();
    let new_route = ModelRoute {
        id: ModelRouteId::from("new-route"),
        connection_id: connection_id.clone(),
        api_family: ApiFamily::OpenAiChatCompletions,
        model_id: "new-model".to_owned(),
        display_name: Some("New model".to_owned()),
        route_config: ModelRouteConfig::default(),
        status: ModelAvailability::Unknown,
        miss_count: 0,
        raw_metadata: None,
        metadata_source: ModelMetadataSource::Legacy,
        metadata_observed_at: None,
        last_reconciled_sync_job_id: None,
        metadata_sync_job_id: None,
        first_seen_at: first_seen_at - chrono::Duration::days(1),
        last_seen_at: None,
    };
    storage
        .reconcile_model_routes(
            &connection_id,
            &[default_route, new_route.clone()],
            observed_at,
        )
        .expect("reconcile models");
    let reconciled_default = storage
        .get_model_route(&ModelRouteId::from("sync"))
        .expect("reconciled default");
    assert_eq!(reconciled_default.status, ModelAvailability::Available);
    assert_eq!(reconciled_default.last_seen_at, Some(observed_at));
    let missing = storage
        .get_model_route(&old_route.id)
        .expect("missing route retained");
    assert_eq!(missing.status, ModelAvailability::MissingTemporarily);
    assert_eq!(missing.first_seen_at, first_seen_at);
    assert_eq!(
        storage
            .get_generation_preset(&old_preset.id)
            .expect("preset retained"),
        old_preset
    );
    let inserted = storage.get_model_route(&new_route.id).expect("new route");
    assert_eq!(inserted.status, ModelAvailability::Available);
    assert_eq!(inserted.first_seen_at, observed_at);
    assert_eq!(inserted.last_seen_at, Some(observed_at));

    let before_rollback = storage
        .list_model_routes(&connection_id)
        .expect("routes before rollback");
    storage
        .connection()
        .expect("connection")
        .execute_batch(
            "CREATE TEMP TRIGGER reject_missing_route_update
                 BEFORE UPDATE ON provider_models
                 WHEN OLD.id = 'old-route'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic reconciliation failure');
                 END;",
        )
        .expect("rollback trigger");
    let next_observation = observed_at + chrono::Duration::minutes(1);
    let listed = before_rollback
        .iter()
        .filter(|route| route.id.as_str() != "old-route")
        .cloned()
        .collect::<Vec<_>>();
    let error = storage
        .reconcile_model_routes(&connection_id, &listed, next_observation)
        .expect_err("reconciliation must roll back");
    assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
    assert_eq!(
        storage
            .list_model_routes(&connection_id)
            .expect("routes after rollback"),
        before_rollback
    );
    assert_eq!(
        storage
            .get_generation_preset(&old_preset.id)
            .expect("preset after rollback"),
        old_preset
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn provider_profile_archive_and_selection_clear_are_atomic() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let profile = ProviderProfile {
        id: "selected".to_owned(),
        display_name: "Selected".to_owned(),
        base_url: "http://127.0.0.1:11434/v1".to_owned(),
        model: "synthetic".to_owned(),
        timeout_seconds: 30,
    };
    storage
        .save_provider_profile(&profile)
        .expect("save provider");
    storage
        .save_settings(&AppSettings {
            preserve_partial_generations: true,
            selected_provider_profile_id: Some(profile.id.clone()),
            ..AppSettings::default()
        })
        .expect("select provider");
    storage
        .connection()
        .expect("connection")
        .execute_batch(
            "CREATE TEMP TRIGGER reject_provider_archive
                 BEFORE UPDATE OF archived_at ON provider_connections
                 WHEN OLD.id = 'selected'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic provider archive failure');
                 END;",
        )
        .expect("install synthetic failure");

    let error = storage
        .delete_provider_profile(&profile.id)
        .expect_err("archive trigger must abort");
    assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
    assert_eq!(
        storage
            .load_settings()
            .expect("settings after rollback")
            .selected_provider_profile_id
            .as_deref(),
        Some(profile.id.as_str())
    );
    assert_eq!(
        storage
            .get_provider_profile(&profile.id)
            .expect("provider after rollback"),
        profile
    );
    assert!(
        storage
            .get_provider_connection(&ProviderConnectionId::from(profile.id.as_str()))
            .is_ok()
    );
    assert!(
        storage
            .get_generation_preset(&GenerationPresetId::from(profile.id.as_str()))
            .is_ok()
    );

    storage
        .connection()
        .expect("connection")
        .execute_batch("DROP TRIGGER reject_provider_archive;")
        .expect("remove synthetic failure");
    storage
        .delete_provider_profile(&profile.id)
        .expect("delete provider");
    assert!(
        storage
            .list_provider_profiles()
            .expect("providers")
            .is_empty()
    );
    assert_eq!(
        storage
            .load_settings()
            .expect("settings after delete")
            .selected_provider_profile_id,
        None
    );
    assert_eq!(
        storage
            .get_provider_connection(&ProviderConnectionId::from(profile.id.as_str()))
            .expect_err("archived connection must be hidden")
            .code,
        CoreErrorCode::NotFound
    );
    assert!(
        storage
            .get_model_route(&ModelRouteId::from(profile.id.as_str()))
            .is_ok(),
        "archiving must preserve route provenance"
    );
    assert!(
        storage
            .get_generation_preset(&GenerationPresetId::from(profile.id.as_str()))
            .is_ok(),
        "archiving must preserve preset provenance"
    );
    let (archived_at, profile_rows) = storage
        .connection()
        .expect("connection")
        .query_row(
            "SELECT connection.archived_at,
                        (SELECT COUNT(*) FROM provider_profiles WHERE id = ?1)
                 FROM provider_connections AS connection
                 WHERE connection.id = ?1",
            [profile.id.as_str()],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, u32>(1)?)),
        )
        .expect("tombstoned provider rows");
    assert!(archived_at.is_some());
    assert_eq!(profile_rows, 1);
    let reuse = storage
        .save_provider_profile(&profile)
        .expect_err("archived provider id must not be reused");
    assert_eq!(reuse.code, CoreErrorCode::InvalidInput);
}
