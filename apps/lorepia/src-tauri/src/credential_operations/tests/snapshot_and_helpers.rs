
    #[tokio::test]
    async fn missing_archive_preflight_never_issues_native_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "archive-already-missing");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "must-not-capture");
        remove_provider_credential_with(&vault, &shell, "archive-already-missing", true)
            .await
            .expect("archive missing slot atomically as native no-effect");
        assert_eq!(vault.counts().3, 0);
        assert!(
            shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "archive-already-missing")
        );
    }

    #[tokio::test]
    async fn restored_database_snapshot_rejects_newer_vault_marker_at_every_shared_read_boundary() {
        let root = tempdir().expect("root");
        let snapshot = tempdir().expect("snapshot");
        let shell_a = ShellApi::open_data_root(root.path()).expect("shell A");
        create_credential_connection(&shell_a, "rollback-marker");
        let vault_a = FakeVault::new(shell_a.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault_a, &shell_a, "rollback-marker")
            .await
            .expect("install A");
        let item_a = vault_a.item();
        drop(vault_a);
        drop(shell_a);
        copy_tree(root.path(), snapshot.path());

        let shell_b = ShellApi::open_data_root(root.path()).expect("shell B");
        let vault_b = FakeVault::new(shell_b.clone(), item_a, "secret-b");
        remove_provider_credential_with(&vault_b, &shell_b, "rollback-marker", false)
            .await
            .expect("remove A");
        capture_provider_connection_credential_with(&vault_b, &shell_b, "rollback-marker")
            .await
            .expect("install B");
        let item_b = vault_b.item();
        drop(vault_b);
        drop(shell_b);

        fs::remove_dir_all(root.path()).expect("remove newer temporary DB root");
        fs::create_dir(root.path()).expect("recreate rollback root");
        copy_tree(snapshot.path(), root.path());
        let restored_a = ShellApi::open_data_root(root.path()).expect("restore shell A snapshot");
        let mismatched_vault = FakeVault::new(restored_a.clone(), item_b, "unused");

        for sink in ["generation", "model_sync", "background_task"] {
            let error = super::read_provider_connection_credential_with(
                &mismatched_vault,
                &restored_a,
                "rollback-marker",
            )
            .await
            .expect_err("newer vault marker must not be released under restored DB A");
            assert_eq!(
                error.code, "credential_recovery_required",
                "{sink} sink must fail closed through the shared native read boundary"
            );
        }
    }

    #[tokio::test]
    async fn restored_started_replacement_with_available_b_does_not_break_bootstrap_or_adopt_b() {
        let root = tempdir().expect("root");
        let snapshot = tempdir().expect("snapshot");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let connection_id = "rollback-started-replacement";
        create_credential_connection(&shell, connection_id);
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        let item_a = vault
            .bound_item_for(&key_a)
            .expect("predecessor A envelope");
        let prepared_b = prepare_authority_bound_install(&shell, connection_id);
        let started_b = shell
            .start_provider_credential_operation(&prepared_b.operation_id, &prepared_b.plan_sha256)
            .expect("start replacement B before snapshot");
        drop(vault);
        drop(shell);
        copy_tree(root.path(), snapshot.path());

        let newer_shell = ShellApi::open_data_root(root.path()).expect("newer shell");
        let newer_vault = FakeVault::new(newer_shell.clone(), FakeItem::Missing, "unused");
        newer_vault.insert_bound_item(key_a.clone(), item_a);
        newer_shell
            .attest_provider_credential_predecessor_delete_intent(
                &started_b.operation_id,
                &started_b.plan_sha256,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("record predecessor delete intent");
        newer_vault
            .delete_bound(connection_id, native_authority(&key_a))
            .await
            .expect("delete predecessor A");
        newer_shell
            .attest_provider_credential_predecessor_missing(
                &started_b.operation_id,
                &started_b.plan_sha256,
            )
            .expect("record predecessor A missing");
        let authority_b = super::operation_authority(&started_b).expect("authority B");
        let key_b = FakeAuthorityKey::from_authority(&authority_b);
        let prepared_store = newer_vault
            .prepare_bound_store(
                connection_id,
                NativeCredential::new("secret-b".to_owned()),
                &authority_b,
            )
            .expect("prepare B store");
        newer_vault
            .store_prepared(prepared_store)
            .await
            .expect("store replacement B");
        newer_shell
            .finish_provider_credential_operation(
                &started_b.operation_id,
                &started_b.plan_sha256,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("newer database adopts B with complete evidence");
        let item_b = newer_vault
            .bound_item_for(&key_b)
            .expect("replacement B envelope survives rollback");
        drop(newer_vault);
        drop(newer_shell);

        fs::remove_dir_all(root.path()).expect("remove newer database root");
        fs::create_dir(root.path()).expect("recreate rollback root");
        copy_tree(snapshot.path(), root.path());
        let restored = ShellApi::open_data_root(root.path()).expect("restore Started snapshot");
        let restored_vault = FakeVault::new(restored.clone(), item_b, "must-not-capture");
        recover_provider_credential_operations_with(&restored_vault, &restored)
            .await
            .expect("rollback bootstrap settles B fail closed");
        recover_provider_credential_operations_with(&restored_vault, &restored)
            .await
            .expect("subsequent bootstrap is idempotent");
        let unresolved = restored
            .list_unresolved_provider_credential_operations()
            .expect("durable rollback recovery state");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert!(unresolved[0].operation_slot_recovery_required);
        restored
            .ensure_provider_credential_access_settled(connection_id)
            .expect_err("restored snapshot must not adopt surviving B");
        assert_eq!(restored_vault.counts().2, 0, "recovery never replays store");
    }

    fn create_credential_connection(shell: &ShellApi, id: &str) {
        let template = shell
            .list_provider_templates()
            .expect("templates")
            .into_iter()
            .find(|template| {
                template.credential_required
                    && template.default_network_mode == "public"
                    && template.default_api_origin.is_some()
            })
            .expect("credential template");
        let origin = template.default_api_origin.expect("origin");
        shell
            .create_provider_connection(CreateProviderConnectionInput {
                id: id.to_owned(),
                template_id: template.id,
                template_version: template.manifest_version,
                display_name: format!("Synthetic {id}"),
                api_origin: origin.clone(),
                api_base_path: None,
                network_mode: ProviderNetworkModeInput::Public,
                local_network_approval: None,
                values: Vec::new(),
                approved_credential_origin: Some(origin),
                timeout_seconds: 30,
            })
            .expect("create connection");
    }

    fn prepare_authority_bound_install(
        shell: &ShellApi,
        connection_id: &str,
    ) -> lorepia_shell_api::ProviderCredentialOperationContext {
        let proposed = shell
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose authority-bound install");
        shell
            .prepare_provider_credential_install_operation(
                connection_id,
                &proposed,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("prepare authority-bound install")
    }

    fn assert_tree_excludes(root: &Path, needles: &[&str]) {
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                if let Ok(entries) = fs::read_dir(path) {
                    stack.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
                }
                continue;
            }
            let Ok(bytes) = fs::read(path) else {
                continue;
            };
            for needle in needles {
                assert!(
                    !bytes
                        .windows(needle.len())
                        .any(|window| window == needle.as_bytes()),
                    "data root file contains forbidden secret material"
                );
            }
        }
    }

    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).expect("create snapshot directory");
        for entry in fs::read_dir(source).expect("read snapshot source") {
            let entry = entry.expect("snapshot entry");
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if entry.file_type().expect("snapshot entry type").is_dir() {
                copy_tree(&source_path, &destination_path);
            } else {
                fs::copy(&source_path, &destination_path).expect("copy snapshot file");
            }
        }
    }
