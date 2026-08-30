fn save_modern_direct_provider_graph(
    storage: &Storage,
    profile: &ProviderProfile,
) -> (ProviderConnection, ModelRoute, GenerationPreset) {
    let (connection, mut route, preset) =
        legacy_provider_graph(profile, Utc::now()).expect("direct provider fixture");
    route.metadata_source = ModelMetadataSource::UserOverride;
    storage
        .save_provider_connection(&connection)
        .expect("save direct provider connection");
    storage
        .save_model_route(&route)
        .expect("save direct provider route");
    storage
        .save_generation_preset(&preset)
        .expect("save direct provider preset");
    assert_eq!(
        storage
            .get_provider_profile(&profile.id)
            .expect_err("direct provider fixture must not retain a legacy profile")
            .code,
        CoreErrorCode::NotFound
    );
    (connection, route, preset)
}

#[test]
fn capability_observation_table_enforces_json_enums_and_route_foreign_keys() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .save_provider_profile(&ProviderProfile {
            id: "observed".to_owned(),
            display_name: "Observed".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "model".to_owned(),
            timeout_seconds: 30,
        })
        .expect("seed route");
    let connection = storage.connection().expect("connection");
    connection
        .execute(
            "INSERT INTO model_capability_observations
                 (id, model_route_id, capability_key, value_json, support_status,
                  source_kind, confidence, evidence_ref, observed_at, expires_at)
                 VALUES ('valid', 'observed', 'streaming', 'true', 'verified',
                         'provider_api', 'high', NULL, ?1, NULL)",
            ["2026-01-01T00:00:00Z"],
        )
        .expect("valid observation");
    for (id, route_id, capability_key, value_json) in [
        ("bad-route", "missing", "streaming", "true"),
        ("bad-capability", "observed", "arbitrary_script", "true"),
        ("bad-json", "observed", "streaming", "not-json"),
    ] {
        assert!(
            connection
                .execute(
                    "INSERT INTO model_capability_observations
                         (id, model_route_id, capability_key, value_json, support_status,
                          source_kind, confidence, evidence_ref, observed_at, expires_at)
                         VALUES (?1, ?2, ?3, ?4, 'verified', 'provider_api', 'high',
                                 NULL, ?5, NULL)",
                    params![
                        id,
                        route_id,
                        capability_key,
                        value_json,
                        "2026-01-01T00:00:00Z"
                    ],
                )
                .is_err(),
            "{id} must violate table integrity"
        );
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM model_capability_observations",
                [],
                |row| row.get::<_, u32>(0),
            )
            .expect("observation count"),
        1
    );
}

#[test]
fn capability_observation_crud_is_typed_monotonic_and_secret_free() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .save_provider_profile(&ProviderProfile {
            id: "capability-crud".to_owned(),
            display_name: "Capability CRUD".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "model".to_owned(),
            timeout_seconds: 30,
        })
        .expect("seed model route");
    let observed_at = Utc::now();
    let mut observation = CapabilityObservation {
        id: ObservationId::from("provider-api:context-window"),
        model_route_id: ModelRouteId::from("capability-crud"),
        key: CapabilityKey::ContextWindow,
        value: CapabilityValue::Integer(32_768),
        status: SupportStatus::Verified,
        source: ObservationSource::ProviderApi,
        confidence: Confidence::High,
        observed_at,
        expires_at: Some(observed_at + Duration::hours(24)),
        evidence_ref: None,
    };
    storage
        .upsert_capability_observation(&observation)
        .expect("insert observation");
    storage
        .upsert_capability_observation(&observation)
        .expect("idempotent observation");
    assert_eq!(
        storage
            .get_capability_observation(&observation.id)
            .expect("stored observation"),
        observation
    );
    assert_eq!(
        storage
            .list_capability_observations_for_key(
                &observation.model_route_id,
                CapabilityKey::ContextWindow,
            )
            .expect("observations"),
        vec![observation.clone()]
    );

    let original = observation.clone();
    observation.observed_at += Duration::minutes(1);
    observation.expires_at = Some(observation.observed_at + Duration::hours(24));
    observation.value = CapabilityValue::Integer(65_536);
    storage
        .upsert_capability_observation(&observation)
        .expect("advance provider observation");
    assert_eq!(
        storage
            .get_capability_observation(&observation.id)
            .expect("updated observation"),
        observation
    );
    assert!(
        storage
            .upsert_capability_observation(&original)
            .expect_err("older observation must not overwrite current evidence")
            .message
            .contains("backwards")
    );

    let secret_metadata = CapabilityObservation {
        id: ObservationId::from("secret-metadata"),
        model_route_id: ModelRouteId::from("capability-crud"),
        key: CapabilityKey::Reasoning,
        value: CapabilityValue::Structured(serde_json::json!({
            "dialect": "open_ai_responses",
            "api_key": "must-not-persist",
        })),
        status: SupportStatus::Documented,
        source: ObservationSource::OfficialDocumentation,
        confidence: Confidence::High,
        observed_at,
        expires_at: None,
        evidence_ref: None,
    };
    assert!(
        storage
            .upsert_capability_observation(&secret_metadata)
            .expect_err("secret-like metadata must be rejected")
            .message
            .contains("credentials")
    );

    storage
        .delete_capability_observation(&observation.id)
        .expect("delete observation");
    assert_eq!(
        storage
            .list_capability_observations(&ModelRouteId::from("capability-crud"))
            .expect("empty observations"),
        Vec::<CapabilityObservation>::new()
    );
}

#[test]
fn capability_observation_batch_is_atomic() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    storage
        .save_provider_profile(&ProviderProfile {
            id: "capability-batch".to_owned(),
            display_name: "Capability Batch".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "model".to_owned(),
            timeout_seconds: 30,
        })
        .expect("seed model route");
    let observed_at = Utc::now();
    let valid = CapabilityObservation {
        id: ObservationId::from("valid-batch-observation"),
        model_route_id: ModelRouteId::from("capability-batch"),
        key: CapabilityKey::Streaming,
        value: CapabilityValue::Boolean(true),
        status: SupportStatus::Verified,
        source: ObservationSource::CapabilityProbe,
        confidence: Confidence::High,
        observed_at,
        expires_at: Some(observed_at + Duration::hours(1)),
        evidence_ref: None,
    };
    let mut invalid = valid.clone();
    invalid.id = ObservationId::from("invalid-batch-observation");
    invalid.model_route_id = ModelRouteId::from("missing-route");
    storage
        .upsert_capability_observations(&[valid, invalid])
        .expect_err("invalid batch must roll back");
    assert_eq!(
        storage
            .list_capability_observations(&ModelRouteId::from("capability-batch"))
            .expect("rolled back observations"),
        Vec::<CapabilityObservation>::new()
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn model_refresh_replaces_only_listed_provider_api_snapshots_atomically_and_reopens() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let (_, main_route, _) = save_modern_direct_provider_graph(
        &storage,
        &ProviderProfile {
            id: "observation-refresh".to_owned(),
            display_name: "Observation refresh".to_owned(),
            base_url: "https://api.example.test/v1".to_owned(),
            model: "main-model".to_owned(),
            timeout_seconds: 30,
        },
    );
    let connection_id = ProviderConnectionId::from("observation-refresh");
    let main_route_id = ModelRouteId::from("observation-refresh");
    let mut omitted_route = main_route.clone();
    omitted_route.id = ModelRouteId::from("observation-refresh-omitted");
    omitted_route.model_id = "omitted-model".to_owned();
    omitted_route.display_name = Some("Omitted model".to_owned());
    storage
        .save_model_route(&omitted_route)
        .expect("seed route omitted by the second refresh");

    let seed_time = Utc::now();
    let preserved_observation =
        |id: &str, key: CapabilityKey, source: ObservationSource| CapabilityObservation {
            id: ObservationId::from(id),
            model_route_id: main_route_id.clone(),
            key,
            value: CapabilityValue::Boolean(true),
            status: SupportStatus::Verified,
            source,
            confidence: Confidence::High,
            observed_at: seed_time,
            expires_at: None,
            evidence_ref: None,
        };
    let signed = preserved_observation(
        "refresh:signed:reasoning",
        CapabilityKey::Reasoning,
        ObservationSource::SignedLorepiaCatalog,
    );
    let probe = preserved_observation(
        "refresh:probe:tool-calling",
        CapabilityKey::ToolCalling,
        ObservationSource::CapabilityProbe,
    );
    let user = preserved_observation(
        "refresh:user:seed",
        CapabilityKey::Seed,
        ObservationSource::UserOverride,
    );
    let legacy_prompt_caching = CapabilityObservation {
        id: ObservationId::from("refresh:provider-api:legacy-prompt-caching"),
        model_route_id: main_route_id.clone(),
        key: CapabilityKey::PromptCaching,
        value: CapabilityValue::Boolean(true),
        status: SupportStatus::Verified,
        source: ObservationSource::ProviderApi,
        confidence: Confidence::High,
        observed_at: seed_time,
        expires_at: None,
        evidence_ref: None,
    };
    storage
        .upsert_capability_observations(&[
            signed.clone(),
            probe.clone(),
            user.clone(),
            legacy_prompt_caching.clone(),
        ])
        .expect("seed observations from preserved sources");

    let first_observed_at = seed_time + Duration::minutes(1);
    let provider_observation =
        |id: &str, route_id: &ModelRouteId, key: CapabilityKey, value: CapabilityValue| {
            CapabilityObservation {
                id: ObservationId::from(id),
                model_route_id: route_id.clone(),
                key,
                value,
                status: SupportStatus::Verified,
                source: ObservationSource::ProviderApi,
                confidence: Confidence::High,
                observed_at: first_observed_at,
                expires_at: Some(first_observed_at + Duration::hours(24)),
                evidence_ref: None,
            }
        };
    let context_window = provider_observation(
        "refresh:provider-api:context-window",
        &main_route_id,
        CapabilityKey::ContextWindow,
        CapabilityValue::Integer(128_000),
    );
    let max_output = provider_observation(
        "refresh:provider-api:max-output",
        &main_route_id,
        CapabilityKey::MaxOutputTokens,
        CapabilityValue::Integer(8_192),
    );
    let unsupported_parallel_tools = CapabilityObservation {
        id: ObservationId::from("refresh:provider-api:parallel-tools"),
        model_route_id: main_route_id.clone(),
        key: CapabilityKey::ParallelToolCalling,
        value: CapabilityValue::Boolean(false),
        status: SupportStatus::Unsupported,
        source: ObservationSource::ProviderApi,
        confidence: Confidence::High,
        observed_at: first_observed_at,
        expires_at: Some(first_observed_at + Duration::hours(24)),
        evidence_ref: None,
    };
    let omitted_route_context = provider_observation(
        "refresh:provider-api:omitted-route-context",
        &omitted_route.id,
        CapabilityKey::ContextWindow,
        CapabilityValue::Integer(32_768),
    );
    let expected_first = storage
        .get_provider_connection(&connection_id)
        .expect("connection before first refresh");
    let mut refreshed_first = expected_first.clone();
    refreshed_first.status = ConnectionStatus::Connected;
    refreshed_first.updated_at = first_observed_at;
    storage
        .commit_model_refresh(
            &expected_first,
            &refreshed_first,
            &[main_route.clone(), omitted_route.clone()],
            &[],
            &[
                context_window.clone(),
                max_output.clone(),
                unsupported_parallel_tools.clone(),
                omitted_route_context.clone(),
            ],
            first_observed_at,
        )
        .expect("commit first provider API snapshot");

    let after_first = storage
        .list_capability_observations(&main_route_id)
        .expect("main observations after first refresh");
    for expected in [
        &context_window,
        &max_output,
        &unsupported_parallel_tools,
        &signed,
        &probe,
        &user,
    ] {
        assert!(after_first.contains(expected));
    }
    assert!(!after_first.contains(&legacy_prompt_caching));

    let expected_second = storage
        .get_provider_connection(&connection_id)
        .expect("connection before second refresh");
    let mut refreshed_second = expected_second.clone();
    let second_observed_at = first_observed_at + Duration::minutes(1);
    refreshed_second.updated_at = second_observed_at;
    let listed_main = storage
        .get_model_route(&main_route_id)
        .expect("listed route before second refresh");
    let invalid_unlisted_observation = CapabilityObservation {
        id: ObservationId::from("refresh:provider-api:unlisted"),
        model_route_id: omitted_route.id.clone(),
        key: CapabilityKey::MaxOutputTokens,
        value: CapabilityValue::Integer(1_024),
        status: SupportStatus::Verified,
        source: ObservationSource::ProviderApi,
        confidence: Confidence::High,
        observed_at: second_observed_at,
        expires_at: Some(second_observed_at + Duration::hours(24)),
        evidence_ref: None,
    };
    storage
        .commit_model_refresh(
            &expected_second,
            &refreshed_second,
            std::slice::from_ref(&listed_main),
            &[],
            &[invalid_unlisted_observation],
            second_observed_at,
        )
        .expect_err("unlisted observation must roll back snapshot deletion");
    let after_rollback = storage
        .list_capability_observations(&main_route_id)
        .expect("main observations after rollback");
    for expected in [&context_window, &max_output, &unsupported_parallel_tools] {
        assert!(
            after_rollback.contains(expected),
            "provider API observation must survive a rolled-back refresh"
        );
    }
    assert_eq!(
        storage
            .get_provider_connection(&connection_id)
            .expect("connection after rollback"),
        expected_second
    );
    let foreign_source_observation = CapabilityObservation {
        id: ObservationId::from("refresh:signed:foreign-source"),
        model_route_id: main_route_id.clone(),
        key: CapabilityKey::ToolCalling,
        value: CapabilityValue::Boolean(true),
        status: SupportStatus::Verified,
        source: ObservationSource::SignedLorepiaCatalog,
        confidence: Confidence::High,
        observed_at: second_observed_at,
        expires_at: Some(second_observed_at + Duration::hours(24)),
        evidence_ref: None,
    };
    storage
        .commit_model_refresh(
            &expected_second,
            &refreshed_second,
            std::slice::from_ref(&listed_main),
            &[],
            &[foreign_source_observation],
            second_observed_at,
        )
        .expect_err("direct provider API snapshot must reject a foreign source");
    let after_foreign_source = storage
        .list_capability_observations(&main_route_id)
        .expect("observations after foreign-source rejection");
    for expected in [&context_window, &max_output, &unsupported_parallel_tools] {
        assert!(after_foreign_source.contains(expected));
    }
    assert_eq!(
        storage
            .get_provider_connection(&connection_id)
            .expect("connection after foreign-source rejection"),
        expected_second
    );

    storage
        .connection()
        .expect("database")
        .execute_batch(
            "CREATE TEMP TRIGGER reject_direct_snapshot_publish
                 BEFORE UPDATE ON provider_connections
                 WHEN OLD.id = 'observation-refresh'
                 BEGIN
                   SELECT RAISE(ABORT, 'synthetic direct refresh publish failure');
                 END;",
        )
        .expect("install rollback trigger");
    storage
        .commit_model_refresh(
            &expected_second,
            &refreshed_second,
            std::slice::from_ref(&listed_main),
            &[],
            &[],
            second_observed_at,
        )
        .expect_err("post-delete direct refresh failure must roll back the snapshot");
    let after_publish_rollback = storage
        .list_capability_observations(&main_route_id)
        .expect("observations after publish rollback");
    for expected in [&context_window, &max_output, &unsupported_parallel_tools] {
        assert!(
            after_publish_rollback.contains(expected),
            "provider API observation must survive a post-delete rollback"
        );
    }
    storage
        .connection()
        .expect("database")
        .execute_batch("DROP TRIGGER reject_direct_snapshot_publish;")
        .expect("remove rollback trigger");

    storage
        .commit_model_refresh(
            &expected_second,
            &refreshed_second,
            std::slice::from_ref(&listed_main),
            &[],
            &[],
            second_observed_at,
        )
        .expect("commit provider API snapshot with omitted numeric limits");
    let after_second = storage
        .list_capability_observations(&main_route_id)
        .expect("main observations after second refresh");
    assert!(after_second.contains(&signed));
    assert!(after_second.contains(&probe));
    assert!(after_second.contains(&user));
    assert!(
        after_second
            .iter()
            .all(|observation| observation.source != ObservationSource::ProviderApi),
        "the listed route must not retain omitted provider API observations"
    );
    assert_eq!(
        storage
            .list_capability_observations(&omitted_route.id)
            .expect("omitted route observations"),
        vec![omitted_route_context.clone()]
    );

    drop(storage);
    let reopened = Storage::open(root.path()).expect("reopen refreshed catalog");
    let reopened_main = reopened
        .list_capability_observations(&main_route_id)
        .expect("main observations after reopen");
    assert!(reopened_main.contains(&signed));
    assert!(reopened_main.contains(&probe));
    assert!(reopened_main.contains(&user));
    assert!(
        reopened_main
            .iter()
            .all(|observation| observation.source != ObservationSource::ProviderApi)
    );
    assert_eq!(
        reopened
            .list_capability_observations(&omitted_route.id)
            .expect("omitted route observations after reopen"),
        vec![omitted_route_context]
    );
}
