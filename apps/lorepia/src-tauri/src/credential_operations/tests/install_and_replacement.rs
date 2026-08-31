    #[tokio::test]
    async fn raw_available_slot_blocks_unowned_create_but_isolated_bound_install_succeeds() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let vault = FakeVault::new(shell.clone(), FakeItem::Raw, "unused-secret");
        ensure_slot_missing(&vault, "orphan-slot")
            .await
            .expect_err("unowned available slot must block create");
        assert!(
            shell
                .list_provider_connections()
                .expect("connections")
                .is_empty()
        );

        create_credential_connection(&shell, "capture-guard");
        capture_provider_connection_credential_with(&vault, &shell, "capture-guard")
            .await
            .expect("authority-derived bound install must not overwrite the raw logical slot");
        assert!(matches!(vault.raw_item(), FakeItem::Raw));
        assert!(matches!(vault.bound_item(), FakeItem::Bound { .. }));
        assert_eq!(vault.counts(), (2, 1, 1, 0));
    }

    #[tokio::test]
    async fn legacy_surface_never_reads_overwrites_or_deletes_a_bound_envelope() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let vault = FakeVault::new_raw(
            shell,
            FakeItem::Bound {
                authority_id: "owned-install".to_owned(),
                binding_sha256: "a".repeat(64),
                secret: "must-not-escape".to_owned(),
            },
            "must-not-capture",
        );
        let access = FakeLegacyAccess { allowed: true };

        assert_eq!(
            legacy_provider_credential_status_with(&vault, &access, "legacy-bound")
                .await
                .expect("safe status"),
            CredentialStatus::Unreadable
        );
        read_legacy_provider_credential_with(&vault, &access, "legacy-bound")
            .await
            .expect_err("bound envelope must never be returned as a legacy secret");
        capture_legacy_provider_credential_with(&vault, &access, "legacy-bound")
            .await
            .expect_err("bound envelope must never be overwritten by legacy capture");
        delete_legacy_provider_credential_with(&vault, &access, "legacy-bound")
            .await
            .expect_err("bound envelope must never be deleted outside its journal");
        assert_eq!(vault.counts(), (0, 0, 0, 0));
        assert_eq!(vault.legacy_counts(), (3, 1, 0, 0));

        let denied = FakeLegacyAccess { allowed: false };
        legacy_provider_credential_status_with(&vault, &denied, "legacy-bound")
            .await
            .expect_err("durably owned slot must be rejected before native status");
        assert_eq!(vault.legacy_counts(), (3, 1, 0, 0));
    }

    #[tokio::test]
    async fn confirmation_revision_changes_when_current_credential_authority_rotates() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "confirmation-authority-rotation");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "rotated-secret");
        let before = provider_connection_credential_effect_context(
            &shell,
            "confirmation-authority-rotation",
            NativeCredentialEffect::CaptureOrReplace,
        )
        .expect("no-credential confirmation context");

        capture_provider_connection_credential_with(
            &vault,
            &shell,
            "confirmation-authority-rotation",
        )
        .await
        .expect("rotate into a durable current authority");
        let after = provider_connection_credential_effect_context(
            &shell,
            "confirmation-authority-rotation",
            NativeCredentialEffect::CaptureOrReplace,
        )
        .expect("owned confirmation context");

        assert!(before.revision().ends_with("journal=settled"));
        assert_ne!(before.revision(), after.revision());
    }

    #[tokio::test]
    async fn delete_confirmation_binds_exact_unresolved_cleanup_state() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "confirmation-unresolved-cleanup");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "owned-secret");
        capture_provider_connection_credential_with(
            &vault,
            &shell,
            "confirmation-unresolved-cleanup",
        )
        .await
        .expect("install current authority");
        let prepared = shell
            .prepare_provider_credential_operation(
                "confirmation-unresolved-cleanup",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare explicit cleanup");
        let prepared_context = provider_connection_credential_effect_context(
            &shell,
            "confirmation-unresolved-cleanup",
            NativeCredentialEffect::Delete,
        )
        .expect("delete can confirm the exact unresolved cleanup");
        assert!(prepared_context.revision().ends_with("journal=prepared"));
        assert!(
            provider_connection_credential_effect_context(
                &shell,
                "confirmation-unresolved-cleanup",
                NativeCredentialEffect::CaptureOrReplace,
            )
            .is_err(),
            "capture cannot layer over unresolved credential work"
        );

        shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("advance exact cleanup cutpoint");
        let started_context = provider_connection_credential_effect_context(
            &shell,
            "confirmation-unresolved-cleanup",
            NativeCredentialEffect::Delete,
        )
        .expect("started cleanup remains explicitly confirmable");
        assert!(started_context.revision().ends_with("journal=started"));
        assert_ne!(prepared_context.revision(), started_context.revision());
    }

    #[tokio::test]
    async fn ordinary_credential_actions_reject_legacy_alias_but_archive_removes_it_durably() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "dual-written-legacy");
        let vault = FakeVault::new(shell.clone(), FakeItem::Raw, "must-not-capture-as-ordinary");
        let policy = FakeOrdinaryTargetPolicy {
            aliases_legacy_raw_slot: true,
        };
        let settings_before = shell
            .get_settings()
            .expect("settings before rejected actions");
        let connections_before = shell
            .list_provider_connections()
            .expect("connections before rejected actions");
        let unresolved_before = shell
            .list_unresolved_provider_credential_operations()
            .expect("journal before rejected actions");

        super::capture_provider_connection_credential_with_policy(
            &vault,
            &shell,
            &policy,
            "dual-written-legacy",
        )
        .await
        .expect_err("ordinary capture must not convert an eligible legacy raw slot");
        remove_provider_credential_with_policy(
            &vault,
            &shell,
            &policy,
            "dual-written-legacy",
            false,
        )
        .await
        .expect_err("ordinary delete must not remove an eligible legacy raw slot");
        assert_eq!(vault.counts(), (0, 0, 0, 0));
        assert_eq!(vault.legacy_counts(), (0, 0, 0, 0));
        assert!(matches!(vault.item(), FakeItem::Raw));
        assert_eq!(
            shell
                .get_settings()
                .expect("settings after rejected actions"),
            settings_before
        );
        assert_eq!(
            shell
                .list_provider_connections()
                .expect("connections after rejected actions"),
            connections_before
        );
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("journal after rejected actions"),
            unresolved_before
        );

        remove_provider_credential_with_policy(
            &vault,
            &shell,
            &policy,
            "dual-written-legacy",
            true,
        )
        .await
        .expect("connection archive durably removes the aliased raw slot and connection");
        assert_eq!(
            vault.counts(),
            (2, 0, 0, 0),
            "archive observes the raw slot before and after deletion without a bound mutation"
        );
        assert_eq!(vault.legacy_counts(), (0, 0, 0, 1));
        assert!(matches!(vault.item(), FakeItem::Missing));
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("archive journal terminalized")
                .is_empty()
        );
        assert!(
            shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "dual-written-legacy")
        );
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen archived root");
        assert!(
            reopened
                .list_provider_connections()
                .expect("reopened active connections")
                .iter()
                .all(|connection| connection.id != "dual-written-legacy")
        );
    }

    #[tokio::test]
    async fn legitimate_legacy_pending_raw_slot_remains_usable() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let vault = FakeVault::new(shell, FakeItem::Raw, "replacement-legacy-secret");
        let access = FakeLegacyAccess { allowed: true };

        assert_eq!(
            legacy_provider_credential_status_with(&vault, &access, "legacy-raw")
                .await
                .expect("legacy raw status"),
            CredentialStatus::Available
        );
        assert_eq!(
            read_legacy_provider_credential_with(&vault, &access, "legacy-raw")
                .await
                .expect("legacy raw read")
                .expect("legacy raw value")
                .into_secret_string(),
            "synthetic-legacy-raw-secret"
        );
        capture_legacy_provider_credential_with(&vault, &access, "legacy-raw")
            .await
            .expect("replace legacy raw credential");
        delete_legacy_provider_credential_with(&vault, &access, "legacy-raw")
            .await
            .expect("delete legacy raw credential");
        assert!(matches!(vault.item(), FakeItem::Missing));
        assert_eq!(vault.legacy_counts(), (4, 1, 1, 1));
    }

    #[tokio::test]
    async fn legacy_raw_capture_retains_the_full_native_credential_limit() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let access = FakeLegacyAccess { allowed: true };
        let maximum = FakeVault::new(
            shell.clone(),
            FakeItem::Missing,
            &"r".repeat(MAXIMUM_LEGACY_CREDENTIAL_BYTES),
        );
        capture_legacy_provider_credential_with(&maximum, &access, "legacy-maximum")
            .await
            .expect("the historical 16 KiB raw credential remains valid");
        assert!(matches!(maximum.item(), FakeItem::Raw));

        let oversized = FakeVault::new(
            shell,
            FakeItem::Missing,
            &"r".repeat(MAXIMUM_LEGACY_CREDENTIAL_BYTES + 1),
        );
        capture_legacy_provider_credential_with(&oversized, &access, "legacy-oversized")
            .await
            .expect_err("a raw credential above the native 16 KiB limit is rejected");
        assert!(matches!(oversized.item(), FakeItem::Missing));
        assert_eq!(oversized.legacy_counts().2, 0);
    }

    #[tokio::test]
    async fn install_is_started_before_single_store_and_journal_is_secret_free() {
        const SECRET: &str = "synthetic-fake-vault-secret-canary";
        let secret_sha256 = format!("{:x}", Sha256::digest(SECRET.as_bytes()));
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "bound-install");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, SECRET);
        capture_provider_connection_credential_with(&vault, &shell, "bound-install")
            .await
            .expect("install bound credential");
        assert_eq!(vault.counts(), (1, 1, 1, 0));
        let authority = shell
            .ensure_provider_credential_access_settled("bound-install")
            .expect("durable access authority");
        let debug = format!("{authority:?}");
        assert!(!debug.contains(SECRET));
        assert!(!debug.contains(&secret_sha256));
        drop(vault);
        drop(shell);
        assert_tree_excludes(root.path(), &[SECRET, &secret_sha256]);
    }

    #[tokio::test]
    async fn replacement_stores_successor_before_deleting_exact_predecessor_and_preserves_raw_slot()
    {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "replacement-order");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "replacement-secret");

        capture_provider_connection_credential_with(&vault, &shell, "replacement-order")
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled("replacement-order")
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        assert_eq!(vault.bound_slot_count(), 1);
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        vault.replace_raw_item(FakeItem::Raw);

        capture_provider_connection_credential_with(&vault, &shell, "replacement-order")
            .await
            .expect("replacement B stores before deleting and attesting predecessor A");
        let authority_b = shell
            .ensure_provider_credential_access_settled("replacement-order")
            .expect("authority B");
        let key_b = FakeAuthorityKey {
            authority_id: authority_b.authority_id,
            binding_sha256: authority_b.connection_binding_sha256,
        };
        assert_ne!(key_a, key_b);
        assert!(vault.bound_item_for(&key_a).is_none());
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.bound_slot_count(), 1);
        assert!(matches!(vault.raw_item(), FakeItem::Raw));

        let events = vault.events();
        let delete_a = events
            .iter()
            .position(|event| event == &FakeVaultEvent::Delete(key_a.clone()))
            .expect("exact predecessor A delete");
        let store_b = events
            .iter()
            .position(|event| event == &FakeVaultEvent::Store(key_b.clone()))
            .expect("exact replacement B store");
        assert!(store_b < delete_a, "B must be stored before A is deleted");
    }

    #[tokio::test]
    async fn replacement_missing_successor_never_starts_predecessor_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let connection_id = "replacement-missing-successor";
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
        vault.replace_capture_secret("secret-b");
        vault.fail_store_before_mutation();

        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect_err("missing B publication cannot authorize deleting A");

        assert_eq!(vault.bound_slot_count(), 1);
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(
            vault
                .events()
                .iter()
                .filter(|event| event == &&FakeVaultEvent::Delete(key_a.clone()))
                .count(),
            0,
            "A deletion must remain downstream of verified B publication"
        );
        assert_eq!(
            operation_predecessor_authority(
                &shell
                    .list_unresolved_provider_credential_operations()
                    .expect("failed replacement remains journaled")[0]
            )
            .expect("predecessor authority parses")
            .as_ref()
            .map(FakeAuthorityKey::from_authority),
            Some(key_a.clone())
        );

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("startup fences missing B without touching A");
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(
            vault
                .events()
                .iter()
                .filter(|event| event == &&FakeVaultEvent::Delete(key_a.clone()))
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn replacement_predecessor_failure_keeps_a_and_b_in_durable_recoverable_state() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "replacement-prepared-drop");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "replacement-secret");

        capture_provider_connection_credential_with(&vault, &shell, "replacement-prepared-drop")
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled("replacement-prepared-drop")
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        vault.fail_delete_before_mutation();

        capture_provider_connection_credential_with(&vault, &shell, "replacement-prepared-drop")
            .await
            .expect_err("failed A cleanup leaves the verified B store journaled");

        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("replacement failure remains journaled");
        assert_eq!(unresolved.len(), 1);
        let key_b = FakeAuthorityKey::from_authority(
            &operation_authority(&unresolved[0]).expect("successor B authority"),
        );
        assert_eq!(
            operation_predecessor_authority(&unresolved[0])
                .expect("predecessor authority parses")
                .as_ref()
                .map(FakeAuthorityKey::from_authority),
            Some(key_a.clone()),
        );
        assert_eq!(vault.counts().2, 2, "B is stored before A cleanup starts");
        assert_eq!(vault.bound_slot_count(), 2);
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("startup fences the unresolved replacement");
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("replacement remains recoverable")[0]
                .status,
            "cleanup_required"
        );
        assert!(
            shell
                .ensure_provider_credential_access_settled("replacement-prepared-drop")
                .is_err(),
            "neither journaled slot is exposed as settled provider authority"
        );

        remove_provider_credential_with(&vault, &shell, "replacement-prepared-drop", false)
            .await
            .expect("explicit cleanup removes the exact journaled slots");
        assert_eq!(vault.bound_slot_count(), 0);
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("cleanup settles the replacement")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn replacement_crash_cleanup_resumes_predecessor_at_every_cutpoint() {
        for archive in [false, true] {
            for cutpoint in 0..3 {
                let root = tempdir().expect("root");
                let shell = ShellApi::open_data_root(root.path()).expect("shell");
                let connection_id = format!("replacement-crash-{archive}-{cutpoint}");
                create_credential_connection(&shell, &connection_id);
                let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
                capture_provider_connection_credential_with(&vault, &shell, &connection_id)
                    .await
                    .expect("install predecessor A");
                let authority_a = shell
                    .ensure_provider_credential_access_settled(&connection_id)
                    .expect("authority A");
                let key_a = FakeAuthorityKey {
                    authority_id: authority_a.authority_id,
                    binding_sha256: authority_a.connection_binding_sha256,
                };
                let item_a = vault.bound_item_for(&key_a).expect("restorable A envelope");

                let proposed_b = shell
                    .propose_provider_credential_install_authority(&connection_id)
                    .expect("propose replacement B");
                let prepared_b = shell
                    .prepare_provider_credential_install_operation(
                        &connection_id,
                        &proposed_b,
                        ProviderCredentialSlotStatusInput::Missing,
                    )
                    .expect("prepare replacement B");
                let started_b = shell
                    .start_provider_credential_operation(
                        &prepared_b.operation_id,
                        &prepared_b.plan_sha256,
                    )
                    .expect("start replacement B");

                if cutpoint >= 1 {
                    shell
                        .attest_provider_credential_predecessor_delete_intent(
                            &started_b.operation_id,
                            &started_b.plan_sha256,
                            ProviderCredentialSlotStatusInput::Available,
                        )
                        .expect("persist predecessor delete intent");
                }
                if cutpoint == 2 {
                    vault
                        .delete_bound(&connection_id, native_authority(&key_a))
                        .await
                        .expect("delete A before simulated crash");
                }
                let deletes_before_cleanup = vault.counts().3;

                remove_provider_credential_with(&vault, &shell, &connection_id, archive)
                    .await
                    .unwrap_or_else(|error| {
                        panic!(
                            "explicit cleanup resumes predecessor and settles replacement B; archive={archive} cutpoint={cutpoint}: {error:?}"
                        )
                    });

                assert!(vault.bound_item_for(&key_a).is_none());
                // Before any predecessor intent exists, Started may also
                // represent an attempted B publication. Explicit recovery
                // therefore repairs B and then removes A. Once predecessor
                // cleanup intent exists, only the exact A retry remains.
                let expected_cleanup_deletes = if cutpoint == 0 { 2 } else { 1 };
                assert_eq!(
                    vault.counts().3,
                    deletes_before_cleanup + expected_cleanup_deletes,
                    "cleanup must repair every possibly attempted slot and repeat predecessor deletion until durable missing evidence exists"
                );
                assert!(
                    shell
                        .list_unresolved_provider_credential_operations()
                        .expect("replacement cleanup terminalized")
                        .is_empty()
                );
                recover_provider_credential_operations_with(&vault, &shell)
                    .await
                    .expect("first post-cleanup bootstrap is idempotent");
                recover_provider_credential_operations_with(&vault, &shell)
                    .await
                    .expect("second post-cleanup bootstrap is idempotent");
                drop(vault);
                drop(shell);

                let reopened = ShellApi::open_data_root(root.path()).expect("reopen cleanup root");
                let restored = FakeVault::new(reopened.clone(), item_a, "must-not-capture");
                read_provider_connection_credential_with(&restored, &reopened, &connection_id)
                    .await
                    .expect_err("restored predecessor A must remain unauthorized");
            }
        }
    }

    struct ReplacementArchiveRestartFixture {
        root: TempDir,
        connection_id: &'static str,
        key_a: FakeAuthorityKey,
        item_a: FakeItem,
    }

    async fn prepare_replacement_archive_restart(
        deleted_b_before_crash: bool,
    ) -> ReplacementArchiveRestartFixture {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let connection_id = if deleted_b_before_crash {
            "replacement-archive-crash-after-b-delete"
        } else {
            "replacement-archive-crash-after-mark"
        };
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
            .expect("predecessor A physical slot");
        let proposed_b = shell
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose B");
        let prepared_b = shell
            .prepare_provider_credential_install_operation(
                connection_id,
                &proposed_b,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("prepare B");
        let started_b = shell
            .start_provider_credential_operation(&prepared_b.operation_id, &prepared_b.plan_sha256)
            .expect("start B");
        let key_b = FakeAuthorityKey {
            authority_id: started_b
                .credential_authority_id
                .clone()
                .expect("B authority id"),
            binding_sha256: started_b
                .credential_authority_binding_sha256
                .clone()
                .expect("B binding"),
        };
        let observed_b = if deleted_b_before_crash {
            vault.insert_bound_item(
                key_b.clone(),
                FakeItem::Bound {
                    authority_id: key_b.authority_id.clone(),
                    binding_sha256: key_b.binding_sha256.clone(),
                    secret: "partial-secret-b".to_owned(),
                },
            );
            ProviderCredentialSlotStatusInput::Available
        } else {
            ProviderCredentialSlotStatusInput::Missing
        };
        shell
            .mark_provider_credential_cleanup_required(
                &started_b.operation_id,
                &started_b.plan_sha256,
                observed_b,
                true,
            )
            .expect("persist cleanup archive intent before crash");
        if deleted_b_before_crash {
            vault
                .delete_bound(connection_id, native_authority(&key_b))
                .await
                .expect("delete partial B before crash");
        }
        drop(vault);
        drop(shell);
        ReplacementArchiveRestartFixture {
            root,
            connection_id,
            key_a,
            item_a,
        }
    }

    async fn assert_replacement_archive_restart(deleted_b_before_crash: bool) {
        let fixture = prepare_replacement_archive_restart(deleted_b_before_crash).await;
        let reopened = ShellApi::open_data_root(fixture.root.path()).expect("reopen crash root");
        let vault = FakeVault::new(reopened.clone(), FakeItem::Missing, "must-not-capture");
        vault.insert_bound_item(fixture.key_a.clone(), fixture.item_a);

        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("bootstrap defers archive until predecessor cleanup resumes");
        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("repeated bootstrap remains idempotently deferred");
        let unresolved = reopened
            .list_unresolved_provider_credential_operations()
            .expect("deferred cleanup remains visible");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert!(unresolved[0].cleanup_archives_connection);
        assert!(
            reopened
                .list_provider_connections()
                .expect("connection remains active before exact cleanup")
                .iter()
                .any(|connection| connection.id == fixture.connection_id)
        );
        assert!(matches!(
            vault.bound_item_for(&fixture.key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.counts().3, 0);

        remove_provider_credential_with(&vault, &reopened, fixture.connection_id, true)
            .await
            .expect("explicit archive resumes A cleanup and atomically terminalizes");
        assert!(vault.bound_item_for(&fixture.key_a).is_none());
        assert_eq!(vault.counts().3, 1);
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("cleanup terminal")
                .is_empty()
        );
        assert!(
            reopened
                .list_provider_connections()
                .expect("archived connection")
                .iter()
                .all(|connection| connection.id != fixture.connection_id)
        );
    }

