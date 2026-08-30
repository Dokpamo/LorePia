fn approved_lan_connection(id: &str) -> (ProviderTemplate, ProviderConnection) {
    let profile = ProviderProfile {
        id: id.to_owned(),
        display_name: "Approved LAN".to_owned(),
        base_url: "https://api.example.test/v1".to_owned(),
        model: "model".to_owned(),
        timeout_seconds: 30,
    };
    let (mut connection, _, _) =
        legacy_provider_graph(&profile, Utc::now()).expect("LAN connection fixture");
    let mut template = legacy_provider_template().expect("LAN template fixture");
    template.id = ProviderTemplateId::from("approved-lan-test-template");
    template.display_name = "Approved LAN test template".to_owned();
    template.source = TemplateSource::UserDiscovered;
    connection.template_id = template.id.clone();
    let api_origin = CanonicalOrigin::parse("https://192.168.10.20:11434").expect("LAN origin");
    connection.api_origin = api_origin.clone();
    connection.config.network_mode = ProviderNetworkMode::ApprovedLocalNetwork;
    connection.config.local_network_approval = Some(ProviderLocalNetworkApproval {
        origin: api_origin.clone(),
        addresses: vec!["192.168.10.20".parse().expect("LAN address")],
    });
    connection
        .credential_scope
        .as_mut()
        .expect("legacy credential scope")
        .allowed_origins = vec![api_origin];
    connection.config.values = vec![ConnectionConfigEntry {
        key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
        value: ConnectionConfigValue::Text("https://192.168.10.20:11434/v1".to_owned()),
    }];
    (template, connection)
}

#[test]
fn storage_owner_lock_child_probe() {
    let Some(root) = std::env::var_os(STORAGE_LOCK_PROBE_ROOT_ENV) else {
        return;
    };
    let Err(error) = Storage::open(PathBuf::from(root)) else {
        panic!("a second process unexpectedly acquired the data root");
    };
    assert_eq!(error.code, CoreErrorCode::StorageUnavailable);
    assert_eq!(
        error.message,
        "data root is already owned by another LorePia process"
    );
}

#[test]
fn approved_lan_connection_persists_exact_grant_and_reopens() {
    let root = tempdir().expect("temp root");
    let (template, connection) = approved_lan_connection("approved-lan-persisted");
    {
        let storage = Storage::open(root.path()).expect("open storage");
        storage
            .save_provider_template(&template)
            .expect("save LAN template");
        storage
            .save_provider_connection(&connection)
            .expect("save approved LAN connection");
        assert_eq!(
            storage
                .get_provider_connection(&connection.id)
                .expect("read approved LAN connection"),
            connection
        );
        let mirror = storage
            .connection()
            .expect("database")
            .query_row(
                "SELECT origin, addresses_json
                     FROM provider_connection_local_network_approvals
                     WHERE connection_id = ?1",
                [connection.id.as_str()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .expect("LAN approval mirror");
        assert_eq!(mirror.0, "https://192.168.10.20:11434");
        assert_eq!(mirror.1, r#"["192.168.10.20"]"#);
    }
    let reopened = Storage::open(root.path()).expect("reopen storage");
    assert_eq!(
        reopened
            .get_provider_connection(&connection.id)
            .expect("reopened approved LAN connection"),
        connection
    );
    reopened
        .connection()
        .expect("database")
        .execute(
            "DELETE FROM provider_connection_local_network_approvals
                 WHERE connection_id = ?1",
            [connection.id.as_str()],
        )
        .expect("simulate missing approval mirror");
    drop(reopened);
    let Err(error) = Storage::open(root.path()) else {
        panic!("missing LAN mirror must fail closed");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
}

#[test]
fn provider_connection_storage_rejects_cleartext_credential_lan_grants() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let (template, mut connection) = approved_lan_connection("cleartext-credential-lan");
    let cleartext_origin =
        CanonicalOrigin::parse("http://192.168.10.20:11434").expect("cleartext LAN origin");
    connection.api_origin = cleartext_origin.clone();
    connection
        .config
        .local_network_approval
        .as_mut()
        .expect("LAN approval")
        .origin = cleartext_origin.clone();
    connection
        .credential_scope
        .as_mut()
        .expect("credential scope")
        .allowed_origins = vec![cleartext_origin];
    connection.config.values = vec![ConnectionConfigEntry {
        key: LEGACY_BASE_URL_CONFIG_KEY.to_owned(),
        value: ConnectionConfigValue::Text("http://192.168.10.20:11434/v1".to_owned()),
    }];
    storage
        .save_provider_template(&template)
        .expect("save LAN template");

    let error = storage
        .save_provider_connection(&connection)
        .expect_err("credential-bearing LAN grants require authenticated transport");
    assert_eq!(error.code, CoreErrorCode::PermissionDenied);
}

#[test]
#[allow(clippy::too_many_lines)]
fn schema_ten_invalid_lan_grant_rolls_back_eleven_and_reopens_after_repair() {
    let root = tempdir().expect("temp root");
    let (template, connection) = approved_lan_connection("invalid-lan-v10");
    {
        fs::create_dir_all(root.path().join("db")).expect("create schema-ten database root");
        let mut database = Connection::open(root.path().join("db/lorepia.sqlite3"))
            .expect("open schema-ten database");
        database
            .pragma_update(None, "foreign_keys", true)
            .expect("enable schema-ten foreign keys");
        for (version, migration) in [
            (1, MIGRATION_0001),
            (2, MIGRATION_0002),
            (3, MIGRATION_0003),
            (4, MIGRATION_0004),
            (5, MIGRATION_0005),
            (6, MIGRATION_0006),
            (7, MIGRATION_0007),
            (8, MIGRATION_0008),
            (9, MIGRATION_0009),
            (10, MIGRATION_0010),
        ] {
            database
                .execute_batch(migration)
                .expect("apply historical migration through schema ten");
            database
                .execute(
                    "INSERT INTO schema_migrations(version, applied_at)
                         VALUES (?1, '2026-07-31T00:00:00Z')",
                    [version],
                )
                .expect("record historical migration through schema ten");
        }
        let mut invalid_config = connection.config.clone();
        invalid_config
            .local_network_approval
            .as_mut()
            .expect("LAN approval")
            .addresses = vec!["8.8.8.8".parse().expect("public address")];
        let invalid_config_json =
            serde_json::to_string(&invalid_config).expect("encode invalid v10 config");
        let transaction = database.transaction().expect("schema-ten seed transaction");
        save_provider_template_row(&transaction, &template).expect("save schema-ten template");
        transaction
            .execute(
                "INSERT INTO provider_connections
                     (id, template_id, template_version, display_name, api_origin,
                      config_json, credential_ref, credential_scope_json, timeout_seconds,
                      status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                params![
                    connection.id.as_str(),
                    connection.template_id.as_str(),
                    connection.template_version,
                    connection.display_name,
                    connection.api_origin.as_str(),
                    invalid_config_json,
                    connection
                        .credential_ref
                        .as_ref()
                        .map(CredentialRef::as_str),
                    connection
                        .credential_scope
                        .as_ref()
                        .map(serde_json::to_string)
                        .transpose()
                        .expect("encode schema-ten credential scope"),
                    connection.timeout_seconds,
                    connection_status_to_str(connection.status),
                    connection.created_at.to_rfc3339(),
                    connection.updated_at.to_rfc3339(),
                ],
            )
            .expect("seed semantically invalid schema-ten LAN grant");
        transaction.commit().expect("commit schema-ten fixture");
    }

    let Err(error) = Storage::open(root.path()) else {
        panic!("invalid schema-ten LAN grant must fail migration");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

    {
        let database = Connection::open(root.path().join("db/lorepia.sqlite3")).expect("database");
        assert_eq!(
            database
                .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| {
                    row.get::<_, u32>(0)
                })
                .expect("schema version"),
            10
        );
        assert_eq!(
            database
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'table'
                           AND name = 'provider_connection_local_network_approvals'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("schema-eleven table count"),
            0
        );
        assert_eq!(
            database
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                         WHERE type = 'trigger'
                           AND name = 'provider_connection_local_network_approval_guard'",
                    [],
                    |row| row.get::<_, u32>(0),
                )
                .expect("schema-eleven trigger count"),
            0
        );
        database
            .execute(
                "UPDATE provider_connections
                     SET config_json = ?2
                     WHERE id = ?1",
                params![
                    connection.id.as_str(),
                    serde_json::to_string(&connection.config).expect("encode repaired v10 config")
                ],
            )
            .expect("repair schema-ten LAN grant");
    }

    let reopened = Storage::open(root.path()).expect("migrate repaired schema-ten storage");
    assert_eq!(
        reopened
            .schema_version()
            .expect("read durable schema version"),
        SCHEMA_VERSION
    );
    assert_eq!(
        reopened
            .get_provider_connection(&connection.id)
            .expect("reopened LAN connection"),
        connection
    );
    assert_eq!(
        reopened
            .connection()
            .expect("database")
            .query_row(
                "SELECT addresses_json
                     FROM provider_connection_local_network_approvals
                     WHERE connection_id = ?1",
                [connection.id.as_str()],
                |row| row.get::<_, String>(0),
            )
            .expect("recreated approval mirror"),
        r#"["192.168.10.20"]"#
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn provider_connection_storage_rejects_noncanonical_network_grants() {
    let root = tempdir().expect("temp root");
    let storage = Storage::open(root.path()).expect("open storage");
    let (template, valid) = approved_lan_connection("approved-lan-invalid");
    storage
        .save_provider_template(&template)
        .expect("save LAN template");

    let mut mismatch = valid.clone();
    mismatch
        .config
        .local_network_approval
        .as_mut()
        .expect("approval")
        .origin = CanonicalOrigin::parse("http://192.168.10.21:11434").expect("other LAN origin");
    assert_eq!(
        storage
            .save_provider_connection(&mismatch)
            .expect_err("origin mismatch")
            .code,
        CoreErrorCode::InvalidInput
    );

    let mut empty = valid.clone();
    empty
        .config
        .local_network_approval
        .as_mut()
        .expect("approval")
        .addresses
        .clear();
    assert!(
        storage
            .save_provider_connection(&empty)
            .expect_err("empty address approval")
            .message
            .contains("1 to 16")
    );

    let mut oversized = valid.clone();
    oversized
        .config
        .local_network_approval
        .as_mut()
        .expect("approval")
        .addresses = (1..=17)
        .map(|last| IpAddr::V4(Ipv4Addr::new(10, 0, 0, last)))
        .collect();
    assert!(
        storage
            .save_provider_connection(&oversized)
            .expect_err("oversized address approval")
            .message
            .contains("1 to 16")
    );

    let mut unsorted = valid.clone();
    unsorted
        .config
        .local_network_approval
        .as_mut()
        .expect("approval")
        .addresses = vec![
        "192.168.10.21".parse().expect("LAN address"),
        "192.168.10.20".parse().expect("LAN address"),
    ];
    assert!(
        storage
            .save_provider_connection(&unsorted)
            .expect_err("unsorted address approval")
            .message
            .contains("sorted")
    );

    let mut public_address = valid.clone();
    public_address
        .config
        .local_network_approval
        .as_mut()
        .expect("approval")
        .addresses = vec!["8.8.8.8".parse().expect("public address")];
    assert!(
        storage
            .save_provider_connection(&public_address)
            .expect_err("public address approval")
            .message
            .contains("RFC1918")
    );

    let mut loopback_with_grant = valid;
    let loopback_origin =
        CanonicalOrigin::parse("http://127.0.0.1:11434").expect("loopback origin");
    loopback_with_grant.api_origin = loopback_origin.clone();
    loopback_with_grant.config.network_mode = ProviderNetworkMode::LocalLoopback;
    loopback_with_grant
        .config
        .local_network_approval
        .as_mut()
        .expect("approval")
        .origin = loopback_origin.clone();
    loopback_with_grant
        .credential_scope
        .as_mut()
        .expect("credential scope")
        .allowed_origins = vec![loopback_origin];
    assert!(
        storage
            .save_provider_connection(&loopback_with_grant)
            .expect_err("loopback mode with LAN approval")
            .message
            .contains("only valid")
    );
}

#[test]
fn data_root_owner_lock_blocks_recovery_in_a_second_process_until_drop() {
    let root = tempdir().expect("temp root");
    let owner = Storage::open(root.path()).expect("open owner");
    let active_staging = root.path().join("staging/active-import.partial");
    fs::write(&active_staging, b"owned by the first process").expect("active staging");

    let output = Command::new(std::env::current_exe().expect("current test executable"))
        .arg(STORAGE_LOCK_PROBE_TEST_NAME)
        .arg("--exact")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env(STORAGE_LOCK_PROBE_ROOT_ENV, root.path())
        .output()
        .expect("run second-process probe");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "second-process probe failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let expected_success = format!("test {STORAGE_LOCK_PROBE_TEST_NAME} ... ok");
    assert!(
        stdout.lines().any(|line| line == expected_success),
        "second-process probe did not execute the exact child test\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        active_staging.exists(),
        "a rejected second process must not run staging recovery"
    );

    drop(owner);
    let reopened = Storage::open(root.path()).expect("reopen after owner drop");
    assert!(
        !active_staging.exists(),
        "the next owner must run normal staging recovery"
    );
    drop(reopened);
}

#[cfg(unix)]
#[test]
fn data_root_and_owner_lock_must_not_be_symbolic_links() {
    use std::os::unix::fs::symlink;

    let parent = tempdir().expect("temp parent");
    let real_root = parent.path().join("real");
    let linked_root = parent.path().join("linked");
    fs::create_dir(&real_root).expect("real root");
    symlink(&real_root, &linked_root).expect("data root symlink");
    let Err(error) = Storage::open(&linked_root) else {
        panic!("symbolic-link data root must be rejected");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);

    let root = tempdir().expect("temp root");
    let outside = parent.path().join("outside-lock");
    fs::write(&outside, b"not a LorePia lock").expect("outside lock");
    symlink(&outside, root.path().join(".lorepia-owner.lock")).expect("owner lock symlink");
    let Err(error) = Storage::open(root.path()) else {
        panic!("symbolic-link owner lock must be rejected");
    };
    assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
    assert_eq!(error.message, "data root owner lock is not a regular file");
}
