    #[tokio::test]
    async fn replacement_archive_cleanup_restart_defers_until_predecessor_can_resume() {
        for deleted_b_before_crash in [false, true] {
            assert_replacement_archive_restart(deleted_b_before_crash).await;
        }
    }

    #[tokio::test]
    async fn pending_missing_superseded_slot_completes_gc_without_native_delete() {
        let (_root, shell, vault, key_a, _item_a, key_b) =
            replacement_gc_fixture("gc-pending-missing").await;
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("missing A completes without a native effect");

        assert_eq!(vault.counts().3, deletes_before);
        assert!(vault.bound_item_for(&key_a).is_none());
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("completed garbage is hidden")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn unattended_gc_never_deletes_a_sqlite_derived_available_slot() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-available").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.replace_raw_item(FakeItem::Raw);
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("unattended GC observes but never deletes superseded A");

        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.bound_slot_count(), 2);
        assert!(matches!(vault.raw_item(), FakeItem::Raw));
        assert_eq!(vault.counts().3, deletes_before);
        assert!(!vault.events().contains(&FakeVaultEvent::Delete(key_b)));
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("available target remains unresolved")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn unattended_gc_never_calls_a_delete_that_would_mutate_then_error() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-response-loss").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.fail_delete_after_mutation();
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("unattended GC never enters the native delete path");

        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("present target remains durable")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn unattended_gc_never_uses_native_delete_to_repair_durability() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-durability-recovery-required").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.require_recovery_after_delete_mutation();
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("SQLite-derived work cannot authorize a durability-repair delete");
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        let unresolved = shell
            .list_provider_credential_slot_garbage()
            .expect("durability repair remains journaled");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "pending");
        assert_eq!(vault.counts().3, deletes_before);

        vault.fail_delete_before_mutation();
        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("retries remain observe-only");
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("unresolved target remains journaled")[0]
                .status,
            "pending"
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
    }

    #[tokio::test]
    async fn unattended_gc_never_resumes_a_started_sqlite_derived_delete() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-started-before-delete").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        let deletes_before = vault.counts().3;
        let target = shell
            .list_provider_credential_slot_garbage()
            .expect("pending target")
            .pop()
            .expect("target");
        let started = shell
            .observe_provider_credential_slot_garbage(
                &target.connection_id,
                target.authority_sequence,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("durable delete cutpoint before simulated crash");
        assert_eq!(started.status, "started");

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("restart leaves legacy Started deletion unresolved");

        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("started target remains durable")[0]
                .status,
            "started"
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
    }

    #[tokio::test]
    async fn unattended_gc_never_repeats_a_started_delete_after_crash() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-delete-before-observe").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        let target = shell
            .list_provider_credential_slot_garbage()
            .expect("pending target")
            .pop()
            .expect("target");
        shell
            .observe_provider_credential_slot_garbage(
                &target.connection_id,
                target.authority_sequence,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("durable delete cutpoint");
        vault
            .delete_bound(&target.connection_id, native_authority(&key_a))
            .await
            .expect("native delete before simulated crash");
        let deletes_after_crash = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("restart cannot replay a SQLite-derived native effect");

        assert_eq!(vault.counts().3, deletes_after_crash);
        assert!(vault.bound_item_for(&key_a).is_none());
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("unattested started target remains durable")[0]
                .status,
            "started"
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
    }

    #[tokio::test]
    async fn unreadable_superseded_gc_stays_unresolved_without_native_delete() {
        let (_root, shell, vault, key_a, _item_a, key_b) =
            replacement_gc_fixture("gc-unreadable").await;
        vault.insert_bound_item(key_a.clone(), FakeItem::UnreadableSlot);

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("unreadable stale target is retained, never adopted");

        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::UnreadableSlot)
        ));
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        shell
            .ensure_provider_credential_access_settled("gc-unreadable")
            .expect("current B authority remains owned");
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("unreadable target remains durable")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn gc_observe_and_status_error_keeps_startup_and_current_authority_usable() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-observe-error").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.fail_next_bound_observation_and_status();
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("one stale backend error must not abort startup recovery");
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("stale target remains retryable")
                .len(),
            1
        );
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(
            read_provider_connection_credential_with(&vault, &shell, "gc-observe-error")
                .await
                .expect("current B remains usable")
                .credential
                .expect("current secret")
                .into_secret_string(),
            "replacement-secret"
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("later retry remains observe-only for present stale A");
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("retry remains unresolved")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn unattended_gc_never_enters_the_post_delete_retry_path() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-post-delete-observe-error").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.fail_post_delete_observation_and_status();
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("no native delete means no post-delete observation");
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        let unresolved = shell
            .list_provider_credential_slot_garbage()
            .expect("present target remains retryable");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "pending");
        assert_eq!(vault.counts().3, deletes_before);
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        read_provider_connection_credential_with(&vault, &shell, "gc-post-delete-observe-error")
            .await
            .expect("current B remains usable after stale A postflight error");

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("later retry still cannot gain deletion authority");
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("retry remains unresolved")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn more_than_twenty_replacement_and_remove_cycles_keep_bound_slots_bounded() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "bounded-slot-cycles");
        let vault = FakeVault::new(shell.clone(), FakeItem::Raw, "rotating-secret");

        capture_provider_connection_credential_with(&vault, &shell, "bounded-slot-cycles")
            .await
            .expect("initial authority install");
        for cycle in 0..21 {
            if cycle % 2 == 0 {
                capture_provider_connection_credential_with(&vault, &shell, "bounded-slot-cycles")
                    .await
                    .expect("replacement rotates through predecessor deletion");
            } else {
                remove_provider_credential_with(&vault, &shell, "bounded-slot-cycles", false)
                    .await
                    .expect("explicit remove deletes the exact current slot");
                assert_eq!(vault.bound_slot_count(), 0);
                capture_provider_connection_credential_with(&vault, &shell, "bounded-slot-cycles")
                    .await
                    .expect("install after exact removal");
            }
            assert_eq!(
                vault.bound_slot_count(),
                1,
                "cycle {cycle} must retain only the current authority-derived slot"
            );
            assert_eq!(vault.bound_keys().len(), 1);
            assert!(matches!(vault.raw_item(), FakeItem::Raw));
        }
    }

    #[tokio::test]
    async fn capture_failure_terminalizes_prepared_install_and_allows_immediate_retry() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "capture-retry");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "retry-secret");
        vault.fail_next_capture();

        capture_provider_connection_credential_with(&vault, &shell, "capture-retry")
            .await
            .expect_err("synthetic clipboard capture fails before native mutation");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("failed capture journal")
                .is_empty(),
            "capture failure must settle its Prepared operation without a restart"
        );
        capture_provider_connection_credential_with(&vault, &shell, "capture-retry")
            .await
            .expect("immediate capture retry succeeds");
    }

    #[tokio::test]
    async fn prepared_store_failure_terminalizes_install_and_allows_immediate_retry() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "prepare-store-retry");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "retry-secret");
        vault.fail_next_prepare_store();

        let error =
            capture_provider_connection_credential_with(&vault, &shell, "prepare-store-retry")
                .await
                .expect_err("synthetic native store preparation fails after durable Prepared");
        assert_eq!(error.code, "storage_unavailable");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("failed preparation journal")
                .is_empty(),
            "prepare failure must settle its exact Prepared operation as no-effect"
        );
        assert!(matches!(vault.bound_item(), FakeItem::Missing));
        assert_eq!(vault.counts().2, 0, "no native store may start");

        capture_provider_connection_credential_with(&vault, &shell, "prepare-store-retry")
            .await
            .expect("immediate capture retry succeeds");
        shell
            .ensure_provider_credential_access_settled("prepare-store-retry")
            .expect("retry grants only its fresh durable authority");
    }

    #[tokio::test]
    async fn raw_logical_slot_appearing_during_capture_is_isolated_from_bound_install() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "capture-slot-race");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "captured-secret");
        vault.create_raw_slot_after_capture();

        capture_provider_connection_credential_with(&vault, &shell, "capture-slot-race")
            .await
            .expect("the authority-derived bound slot is independent of the raw logical slot");
        assert!(matches!(vault.item(), FakeItem::Raw));
        assert!(matches!(vault.bound_item(), FakeItem::Bound { .. }));
        assert_eq!(vault.counts().2, 1, "only the derived bound slot is stored");
        shell
            .ensure_provider_credential_access_settled("capture-slot-race")
            .expect("raw logical slot is never adopted as the bound credential");
    }

    #[tokio::test]
    async fn exact_postflight_wins_over_mutate_then_error_for_store_and_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "postflight-wins");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "stored-secret");
        vault.fail_store_after_mutation();
        capture_provider_connection_credential_with(&vault, &shell, "postflight-wins")
            .await
            .expect("matching bound postflight confirms store despite response loss");
        shell
            .ensure_provider_credential_access_settled("postflight-wins")
            .expect("store postflight owns exact authority");

        vault.fail_delete_after_mutation();
        remove_provider_credential_with(&vault, &shell, "postflight-wins", false)
            .await
            .expect("missing postflight confirms delete despite response loss");
        assert!(matches!(vault.item(), FakeItem::Missing));
        shell
            .ensure_provider_credential_access_settled("postflight-wins")
            .expect_err("confirmed delete revokes credential authority");
    }

    #[tokio::test]
    async fn recovery_required_store_never_adopts_visible_credential() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "durability-unknown-store");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "stored-secret");
        vault.require_recovery_after_store_mutation();

        let error =
            capture_provider_connection_credential_with(&vault, &shell, "durability-unknown-store")
                .await
                .expect_err("visible Match cannot override explicit recovery-required");
        assert_eq!(error.code, "credential_recovery_required");
        assert!(matches!(vault.bound_item(), FakeItem::Bound { .. }));
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("durability-unknown install remains journaled");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        shell
            .ensure_provider_credential_access_settled("durability-unknown-store")
            .expect_err("visible credential with unknown durability is never adopted");

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("bootstrap keeps the explicit recovery barrier");
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("recovery barrier survives bootstrap")[0]
                .status,
            "cleanup_required"
        );
    }

    #[tokio::test]
    async fn recovery_required_delete_never_accepts_visible_missing_as_durable_success() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "durability-unknown-delete");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "stored-secret");
        capture_provider_connection_credential_with(&vault, &shell, "durability-unknown-delete")
            .await
            .expect("install credential");
        vault.require_recovery_after_delete_mutation();

        let error =
            remove_provider_credential_with(&vault, &shell, "durability-unknown-delete", false)
                .await
                .expect_err("visible Missing cannot override explicit recovery-required");
        assert_eq!(error.code, "credential_recovery_required");
        assert!(matches!(vault.bound_item(), FakeItem::Missing));
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("durability-unknown removal remains journaled");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("bootstrap keeps the explicit recovery barrier");
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("delete recovery barrier survives bootstrap")[0]
                .status,
            "cleanup_required"
        );
        assert_eq!(vault.counts().3, 1, "the uncertain delete ran once");

        vault.fail_delete_before_mutation();
        let retry_error =
            remove_provider_credential_with(&vault, &shell, "durability-unknown-delete", false)
                .await
                .expect_err("a failed exact durability retry cannot be cleared by Missing");
        assert_eq!(retry_error.code, "storage_unavailable");
        let still_blocked = shell
            .list_unresolved_provider_credential_operations()
            .expect("failed repair remains journaled");
        assert_eq!(still_blocked.len(), 1);
        assert!(still_blocked[0].operation_slot_recovery_required);

        remove_provider_credential_with(&vault, &shell, "durability-unknown-delete", false)
            .await
            .expect("explicit cleanup retries the exact durability boundary");
        assert_eq!(
            vault.counts().3,
            3,
            "Missing visibility must not skip either failed or successful native repair retries"
        );
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("successful repair settles the barrier")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn replacement_predecessor_recovery_required_preserves_b_until_exact_cleanup_retry() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let connection_id = "replacement-predecessor-durability";
        create_credential_connection(&shell, connection_id);
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("owned predecessor A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        let stores_before_b = vault.counts().2;
        vault.replace_capture_secret("secret-b");
        vault.require_recovery_after_delete_mutation();

        let error = capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect_err("uncertain predecessor delete must leave replacement journaled");
        assert_eq!(error.code, "credential_recovery_required");
        assert!(vault.bound_item_for(&key_a).is_none());
        assert_eq!(
            vault.counts().2,
            stores_before_b + 1,
            "verified B must exist before predecessor cleanup starts"
        );
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("replacement cleanup remains durable");
        assert_eq!(unresolved.len(), 1);
        let key_b = FakeAuthorityKey::from_authority(
            &operation_authority(&unresolved[0]).expect("successor B authority"),
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert!(unresolved[0].predecessor_slot_recovery_required);
        assert!(!unresolved[0].operation_slot_recovery_required);
        assert_eq!(
            unresolved[0].outcome_code.as_deref(),
            Some("native_predecessor_durability_unknown")
        );

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("bootstrap preserves predecessor durability barrier");
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("replacement barrier survives bootstrap")[0]
                .status,
            "cleanup_required"
        );
        let deletes_before_repair = vault.counts().3;
        let predecessor_deletes_before = vault
            .events()
            .into_iter()
            .filter(|event| event == &FakeVaultEvent::Delete(key_a.clone()))
            .count();
        vault.fail_delete_before_mutation();
        let retry_error = remove_provider_credential_with(&vault, &shell, connection_id, false)
            .await
            .expect_err("failed predecessor repair cannot be cleared by Missing visibility");
        assert_eq!(retry_error.code, "storage_unavailable");
        let still_blocked = shell
            .list_unresolved_provider_credential_operations()
            .expect("failed predecessor repair remains journaled");
        assert_eq!(still_blocked.len(), 1);
        assert!(still_blocked[0].predecessor_slot_recovery_required);
        assert_eq!(vault.counts().2, stores_before_b + 1);

        remove_provider_credential_with(&vault, &shell, connection_id, false)
            .await
            .expect("explicit cleanup repeats exact predecessor delete boundary");
        assert_eq!(vault.counts().3, deletes_before_repair + 3);
        assert_eq!(
            vault
                .events()
                .into_iter()
                .filter(|event| event == &FakeVaultEvent::Delete(key_a.clone()))
                .count(),
            predecessor_deletes_before + 2,
            "explicit retry repairs predecessor A rather than unrelated B"
        );
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("replacement cleanup terminal")
                .is_empty()
        );
        assert_eq!(vault.counts().2, stores_before_b + 1);
        assert!(vault.bound_item_for(&key_b).is_none());
    }

    #[tokio::test]
    async fn archive_postflight_wins_over_native_delete_response_loss_and_reopens_settled() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "archive-postflight-wins");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "archive-secret");
        capture_provider_connection_credential_with(&vault, &shell, "archive-postflight-wins")
            .await
            .expect("install archive credential");
        vault.fail_delete_after_mutation();

        remove_provider_credential_with(&vault, &shell, "archive-postflight-wins", true)
            .await
            .expect("missing postflight atomically confirms archive despite response loss");
        assert!(matches!(vault.item(), FakeItem::Missing));
        assert!(
            shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "archive-postflight-wins")
        );
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen archived root");
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("reopened archive journal")
                .is_empty()
        );
        assert!(
            reopened
                .list_provider_connections()
                .expect("reopened active connections")
                .iter()
                .all(|connection| connection.id != "archive-postflight-wins")
        );
    }

    #[tokio::test]
    async fn uncertain_cleanup_archive_postflight_wins_over_delete_response_loss() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "uncertain-archive-response-loss");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "archive-secret");
        capture_provider_connection_credential_with(
            &vault,
            &shell,
            "uncertain-archive-response-loss",
        )
        .await
        .expect("install archive credential");
        let prepared = shell
            .prepare_provider_credential_operation(
                "uncertain-archive-response-loss",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare an ordinary removal before the archive request");
        shell
            .finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record uncertain pre-effect observation");
        vault.fail_delete_after_mutation();

        remove_provider_credential_with(&vault, &shell, "uncertain-archive-response-loss", true)
            .await
            .expect("truthful missing postflight completes the durable cleanup archive intent");
        assert!(matches!(vault.item(), FakeItem::Missing));
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("cleanup archive journal")
                .is_empty()
        );
        assert!(
            shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "uncertain-archive-response-loss")
        );
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen cleanup archive root");
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("reopened cleanup journal")
                .is_empty()
        );
        assert!(
            reopened
                .list_provider_connections()
                .expect("reopened active connections")
                .iter()
                .all(|connection| connection.id != "uncertain-archive-response-loss")
        );
    }
