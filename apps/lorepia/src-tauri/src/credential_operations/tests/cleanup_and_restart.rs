
    #[tokio::test]
    async fn delete_that_leaves_the_slot_available_never_reports_success() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "delete-no-effect");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "retained-secret");
        capture_provider_connection_credential_with(&vault, &shell, "delete-no-effect")
            .await
            .expect("install credential");
        vault.preserve_item_on_delete();

        remove_provider_credential_with(&vault, &shell, "delete-no-effect", false)
            .await
            .expect_err("available postflight means explicit delete did not succeed");
        assert!(!matches!(vault.item(), FakeItem::Missing));
    }

    #[tokio::test]
    async fn unreadable_slot_can_be_explicitly_deleted_then_reinstalled() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "unreadable-delete");
        let vault = FakeVault::new(
            shell.clone(),
            FakeItem::UnreadableSlot,
            "replacement-secret",
        );

        remove_provider_credential_with(&vault, &shell, "unreadable-delete", false)
            .await
            .expect("explicit journaled delete may clear an unreadable native item");
        assert!(matches!(vault.item(), FakeItem::Missing));
        capture_provider_connection_credential_with(&vault, &shell, "unreadable-delete")
            .await
            .expect("cleared unreadable slot can be reinstalled");
    }

    #[tokio::test]
    async fn prior_owned_observe_error_with_unreadable_status_can_remove_or_archive() {
        for archive in [false, true] {
            let root = tempdir().expect("root");
            let shell = ShellApi::open_data_root(root.path()).expect("shell");
            let connection_id = if archive {
                "owned-observe-error-archive"
            } else {
                "owned-observe-error-remove"
            };
            create_credential_connection(&shell, connection_id);
            let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "owned-secret");
            capture_provider_connection_credential_with(&vault, &shell, connection_id)
                .await
                .expect("install prior A");
            let prior_item = vault.bound_item();
            vault.replace_item(FakeItem::UnreadableSlot);
            vault.fail_next_bound_observation();

            remove_provider_credential_with(&vault, &shell, connection_id, archive)
                .await
                .expect("status fallback journals and deletes exact unreadable A");

            assert_eq!(vault.counts().3, 1);
            assert!(matches!(vault.bound_item(), FakeItem::Missing));
            assert!(
                shell
                    .list_unresolved_provider_credential_operations()
                    .expect("cleanup terminalized")
                    .is_empty()
            );
            assert_eq!(
                shell
                    .list_provider_connections()
                    .expect("active connections")
                    .iter()
                    .any(|connection| connection.id == connection_id),
                !archive
            );
            drop(vault);
            drop(shell);

            let reopened = ShellApi::open_data_root(root.path()).expect("reopen cleanup root");
            assert!(
                reopened
                    .list_unresolved_provider_credential_operations()
                    .expect("reopened cleanup terminal")
                    .is_empty()
            );
            let restored = FakeVault::new(reopened.clone(), prior_item, "must-not-capture");
            read_provider_connection_credential_with(&restored, &reopened, connection_id)
                .await
                .expect_err("restored prior A remains unauthorized");
        }
    }

    #[tokio::test]
    async fn prior_owned_unreadable_delete_failure_is_not_reported_as_success() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "owned-unreadable-delete-failure");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "owned-secret");
        capture_provider_connection_credential_with(
            &vault,
            &shell,
            "owned-unreadable-delete-failure",
        )
        .await
        .expect("install prior A");
        vault.replace_item(FakeItem::UnreadableSlot);
        vault.fail_next_bound_observation();
        vault.fail_delete_before_mutation();

        let error = remove_provider_credential_with(
            &vault,
            &shell,
            "owned-unreadable-delete-failure",
            false,
        )
        .await
        .expect_err("failed native delete must remain visible");
        assert_eq!(error.code, "storage_unavailable");
        assert!(matches!(vault.bound_item(), FakeItem::UnreadableSlot));
        assert_eq!(vault.counts().3, 1);
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("retryable cleanup intent")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn unreadable_uncertain_install_cleanup_can_remove_or_archive_without_reappearing() {
        for archive in [false, true] {
            let root = tempdir().expect("root");
            let shell = ShellApi::open_data_root(root.path()).expect("shell");
            let connection_id = if archive {
                "unreadable-install-archive"
            } else {
                "unreadable-install-remove"
            };
            create_credential_connection(&shell, connection_id);
            let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "unused-secret");
            let prepared = prepare_authority_bound_install(&shell, connection_id);
            shell
                .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
                .expect("start install before unreadable outcome");
            vault.replace_item(FakeItem::UnreadableSlot);
            let uncertain = shell
                .finish_provider_credential_operation(
                    &prepared.operation_id,
                    &prepared.plan_sha256,
                    ProviderCredentialSlotStatusInput::Unreadable,
                )
                .expect("record unreadable install outcome");
            assert_eq!(uncertain.status, "outcome_unknown");

            remove_provider_credential_with(&vault, &shell, connection_id, archive)
                .await
                .expect("explicit cleanup settles the original unreadable install");
            assert!(matches!(vault.item(), FakeItem::Missing));
            assert!(
                shell
                    .list_unresolved_provider_credential_operations()
                    .expect("settled cleanup journal")
                    .is_empty()
            );
            let remains_active = shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .any(|connection| connection.id == connection_id);
            assert_eq!(remains_active, !archive);
            if !archive {
                shell
                    .ensure_provider_credential_access_settled(connection_id)
                    .expect_err("explicit cleanup revokes any prior authority");
            }
            drop(vault);
            drop(shell);

            let reopened = ShellApi::open_data_root(root.path()).expect("reopen cleanup root");
            assert!(
                reopened
                    .list_unresolved_provider_credential_operations()
                    .expect("reopened cleanup journal")
                    .is_empty()
            );
            let remains_active = reopened
                .list_provider_connections()
                .expect("reopened active connections")
                .iter()
                .any(|connection| connection.id == connection_id);
            assert_eq!(remains_active, !archive);
        }
    }

    #[tokio::test]
    async fn stale_or_malformed_marker_is_blocked_but_explicit_delete_allows_reinstall() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "marker-reset");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault, &shell, "marker-reset")
            .await
            .expect("install A");
        vault.replace_item(FakeItem::Bound {
            authority_id: "newer-install-b".to_owned(),
            binding_sha256: "b".repeat(64),
            secret: "secret-b".to_owned(),
        });
        read_provider_connection_credential_with(&vault, &shell, "marker-reset")
            .await
            .expect_err("mismatched envelope in the exact authority slot fails closed");
        remove_provider_credential_with(&vault, &shell, "marker-reset", false)
            .await
            .expect("explicit mismatch deletion");
        capture_provider_connection_credential_with(&vault, &shell, "marker-reset")
            .await
            .expect("fresh install after explicit deletion");

        vault.replace_item(FakeItem::MalformedEnvelope);
        remove_provider_credential_with(&vault, &shell, "marker-reset", false)
            .await
            .expect("explicit malformed-envelope deletion");
        assert_eq!(vault.counts().3, 2);
    }

    #[tokio::test]
    async fn archive_first_blocks_then_forces_background_credential_read_to_fail_closed() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "archive-first-read");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "leased-secret");
        capture_provider_connection_credential_with(&vault, &shell, "archive-first-read")
            .await
            .expect("install credential");

        let operation_lock = Arc::new(tokio::sync::Mutex::new(()));
        let archive_guard = Arc::clone(&operation_lock).lock_owned().await;
        shell
            .prepare_provider_credential_operation(
                "archive-first-read",
                ProviderCredentialOperationKindInput::RemoveForArchive,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare archive before releasing operation lock");

        let read = {
            let operation_lock = Arc::clone(&operation_lock);
            let vault = vault.clone();
            let shell = shell.clone();
            tokio::spawn(async move {
                let _lease = operation_lock.lock_owned().await;
                read_provider_connection_credential_with(&vault, &shell, "archive-first-read").await
            })
        };
        tokio::task::yield_now().await;
        assert!(
            !read.is_finished(),
            "reader must wait behind archive intent"
        );
        drop(archive_guard);
        assert!(
            read.await.expect("credential reader task").is_err(),
            "the settled-access gate must reject an archive-first credential read"
        );
    }

    #[tokio::test]
    async fn restart_recovery_fences_exact_marker_without_observing_or_repeating_store() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "restart-install");
        let prepared = prepare_authority_bound_install(&shell, "restart-install");
        let started = shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("start");
        let vault = FakeVault::new(
            shell.clone(),
            FakeItem::Bound {
                authority_id: started.operation_id.clone(),
                binding_sha256: started.connection_binding_sha256.clone(),
                secret: "restart-secret".to_owned(),
            },
            "must-not-capture",
        );
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("recover exact marker");
        assert_eq!(vault.counts(), (0, 0, 0, 0));
        shell
            .ensure_provider_credential_access_settled("restart-install")
            .expect_err("bare Started visibility is never adopted after restart");
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("load startup fence");
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].operation_slot_recovery_required);
    }

    #[tokio::test]
    async fn explicit_cleanup_fences_bare_started_before_missing_slot_retry() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "same-process-started-cleanup");
        let prepared = prepare_authority_bound_install(&shell, "same-process-started-cleanup");
        shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("persist Started before simulated post-native interruption");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "must-not-capture");
        vault.fail_delete_before_mutation();

        let error =
            remove_provider_credential_with(&vault, &shell, "same-process-started-cleanup", false)
                .await
                .expect_err("failed exact repair must preserve the same-process Started fence");
        assert_eq!(error.code, "storage_unavailable");
        assert_eq!(vault.counts().3, 1);
        let blocked = shell
            .list_unresolved_provider_credential_operations()
            .expect("load explicit cleanup fence");
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].status, "cleanup_required");
        assert!(blocked[0].operation_slot_recovery_required);
        shell
            .ensure_provider_credential_access_settled("same-process-started-cleanup")
            .expect_err("bare Started remains inaccessible until an exact successful retry");

        remove_provider_credential_with(&vault, &shell, "same-process-started-cleanup", false)
            .await
            .expect("successful Missing-slot retry repairs the durability boundary");
        assert_eq!(vault.counts().3, 2);
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("successful repair settles same-process Started")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn persistent_mismatch_can_be_explicitly_cleaned_and_reinstalled_after_reopen() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "restart-mismatch");
        let prepared = prepare_authority_bound_install(&shell, "restart-mismatch");
        shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("start");
        let vault = FakeVault::new(
            shell.clone(),
            FakeItem::Bound {
                authority_id: "different-install".to_owned(),
                binding_sha256: "b".repeat(64),
                secret: "unowned-secret".to_owned(),
            },
            "must-not-capture",
        );
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("first mismatch recovery");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("second mismatch recovery is a no-op");
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("persistent mismatch remains restart-visible");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert!(unresolved[0].operation_slot_recovery_required);
        shell
            .ensure_provider_credential_access_settled("restart-mismatch")
            .expect_err("persistent mismatch remains use-blocking");
        remove_provider_credential_with(&vault, &shell, "restart-mismatch", false)
            .await
            .expect("explicit delete continues the same uncertain authority");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("cleanup terminalized")
                .is_empty()
        );
        assert!(matches!(vault.item(), FakeItem::Missing));
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen after cleanup");
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("reopened cleanup is settled")
                .is_empty()
        );
        let reinstall_vault = FakeVault::new(reopened.clone(), FakeItem::Missing, "fresh-secret");
        capture_provider_connection_credential_with(
            &reinstall_vault,
            &reopened,
            "restart-mismatch",
        )
        .await
        .expect("fresh install is allowed after explicit cleanup");
    }

    #[tokio::test]
    async fn cleanup_intent_survives_restart_before_delete_without_reenabling_credential() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "cleanup-crash");
        let prepared = prepare_authority_bound_install(&shell, "cleanup-crash");
        shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("start install");
        let vault = FakeVault::new(
            shell.clone(),
            FakeItem::Bound {
                authority_id: "different-install".to_owned(),
                binding_sha256: "b".repeat(64),
                secret: "unowned-secret".to_owned(),
            },
            "must-not-capture",
        );
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("classify mismatched envelope as uncertain");
        let marked = shell
            .list_unresolved_provider_credential_operations()
            .expect("load fenced cleanup intent")
            .into_iter()
            .find(|operation| operation.operation_id == prepared.operation_id)
            .expect("startup fence persists the exact cleanup intent before native delete");
        assert_eq!(marked.status, "cleanup_required");
        assert!(marked.operation_slot_recovery_required);

        let retained_item = vault.item();
        drop(vault);
        drop(shell);
        let reopened = ShellApi::open_data_root(root.path()).expect("restart after cleanup mark");
        let vault = FakeVault::new(reopened.clone(), retained_item, "must-not-capture");

        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("first restart preserves pending cleanup intent");
        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("second restart preserves pending cleanup intent");
        let unresolved = reopened
            .list_unresolved_provider_credential_operations()
            .expect("cleanup remains restart-visible");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert_eq!(
            vault.counts().3,
            0,
            "bootstrap must not replay native delete"
        );
        reopened
            .ensure_provider_credential_access_settled("cleanup-crash")
            .expect_err("cleanup intent must not be reclassified as an owned install");

        remove_provider_credential_with(&vault, &reopened, "cleanup-crash", false)
            .await
            .expect("explicit retry resumes and terminalizes the cleanup intent");
        assert_eq!(vault.counts().3, 1);
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("cleanup terminalized")
                .is_empty()
        );
        assert!(matches!(vault.item(), FakeItem::Missing));
    }

    #[tokio::test]
    async fn uncertain_archive_explicit_retry_finishes_the_original_operation_once() {
        for already_cleanup_required in [false, true] {
            let root = tempdir().expect("root");
            let shell = ShellApi::open_data_root(root.path()).expect("shell");
            let connection_id = if already_cleanup_required {
                "cleanup-required-archive"
            } else {
                "outcome-unknown-archive"
            };
            create_credential_connection(&shell, connection_id);
            let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "archive-secret");
            capture_provider_connection_credential_with(&vault, &shell, connection_id)
                .await
                .expect("install credential before archive");

            let prepared = shell
                .prepare_provider_credential_operation(
                    connection_id,
                    ProviderCredentialOperationKindInput::RemoveForArchive,
                    ProviderCredentialSlotStatusInput::Available,
                )
                .expect("prepare archive removal");
            let started = shell
                .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
                .expect("start archive removal");
            let uncertain = shell
                .finish_provider_credential_operation(
                    &started.operation_id,
                    &started.plan_sha256,
                    ProviderCredentialSlotStatusInput::Unreadable,
                )
                .expect("record uncertain archive observation");
            assert_eq!(uncertain.status, "outcome_unknown");
            if already_cleanup_required {
                let cleanup = shell
                    .mark_provider_credential_cleanup_required(
                        &started.operation_id,
                        &started.plan_sha256,
                        ProviderCredentialSlotStatusInput::Available,
                        true,
                    )
                    .expect("persist cleanup intent before explicit retry");
                assert_eq!(cleanup.status, "cleanup_required");
            }

            remove_provider_credential_with(&vault, &shell, connection_id, true)
                .await
                .expect("same uncertain archive operation must complete the command");
            assert_eq!(vault.counts().3, 1, "native delete runs exactly once");
            assert!(
                shell
                    .list_unresolved_provider_credential_operations()
                    .expect("archive cleanup is terminal")
                    .is_empty()
            );
            assert!(
                shell
                    .list_provider_connections()
                    .expect("active connections")
                    .iter()
                    .all(|connection| connection.id != connection_id)
            );
            let terminal = shell
                .reconcile_provider_credential_archive(
                    &started.operation_id,
                    &started.plan_sha256,
                    ProviderCredentialSlotStatusInput::Missing,
                )
                .expect("original archive operation remains idempotently terminal");
            assert_eq!(terminal.status, "succeeded");

            drop(vault);
            drop(shell);
            let reopened = ShellApi::open_data_root(root.path()).expect("reopen archived root");
            assert!(
                reopened
                    .list_unresolved_provider_credential_operations()
                    .expect("reopened archive remains settled")
                    .is_empty()
            );
            assert!(
                reopened
                    .list_provider_connections()
                    .expect("reopened active connections")
                    .iter()
                    .all(|connection| connection.id != connection_id)
            );
        }
    }

    #[tokio::test]
    async fn unstarted_uncertain_remove_revokes_the_prior_envelope_after_explicit_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "unstarted-remove-cleanup");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "prior-secret");
        capture_provider_connection_credential_with(&vault, &shell, "unstarted-remove-cleanup")
            .await
            .expect("install prior owned envelope");
        let prior_envelope = vault.item();

        let prepared = shell
            .prepare_provider_credential_operation(
                "unstarted-remove-cleanup",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare removal without starting native effect");
        let uncertain = shell
            .finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record uncertain pre-effect observation");
        assert_eq!(uncertain.status, "outcome_unknown");
        assert!(!uncertain.native_effect_started);

        remove_provider_credential_with(&vault, &shell, "unstarted-remove-cleanup", false)
            .await
            .expect("explicit delete must durably revoke the prior authority");
        assert_eq!(vault.counts().3, 1);
        assert!(matches!(vault.item(), FakeItem::Missing));
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen after explicit delete");
        let restored = FakeVault::new(reopened.clone(), prior_envelope, "must-not-capture");
        read_provider_connection_credential_with(&restored, &reopened, "unstarted-remove-cleanup")
            .await
            .expect_err("restoring the explicitly deleted envelope must remain unauthorized");
    }

    #[tokio::test]
    async fn missing_uncertain_remove_still_revokes_the_prior_envelope() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "missing-remove-cleanup");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "prior-secret");
        capture_provider_connection_credential_with(&vault, &shell, "missing-remove-cleanup")
            .await
            .expect("install prior owned envelope");
        let prior_envelope = vault.item();
        let prepared = shell
            .prepare_provider_credential_operation(
                "missing-remove-cleanup",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare unstarted removal");
        shell
            .finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record uncertain removal");
        vault.replace_item(FakeItem::Missing);

        remove_provider_credential_with(&vault, &shell, "missing-remove-cleanup", false)
            .await
            .expect("missing explicit delete still records durable revocation");
        assert_eq!(vault.counts().3, 0, "missing cleanup has no native effect");
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen after missing cleanup");
        let restored = FakeVault::new(reopened.clone(), prior_envelope, "must-not-capture");
        read_provider_connection_credential_with(&restored, &reopened, "missing-remove-cleanup")
            .await
            .expect_err("the prior envelope remains revoked after missing cleanup");
    }

    #[tokio::test]
    async fn archive_cleanup_intent_survives_restart_before_native_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "archive-cleanup-restart");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "archive-secret");
        capture_provider_connection_credential_with(&vault, &shell, "archive-cleanup-restart")
            .await
            .expect("install credential before archive cleanup");
        let prepared = shell
            .prepare_provider_credential_operation(
                "archive-cleanup-restart",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare an ordinary removal before archive request");
        shell
            .finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record uncertain ordinary removal");
        let marked = shell
            .mark_provider_credential_cleanup_required(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Available,
                true,
            )
            .expect("persist archive disposition before native delete");
        assert!(marked.cleanup_archives_connection);
        let retained_item = vault.item();
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("restart before native delete");
        let vault = FakeVault::new(reopened.clone(), retained_item, "must-not-capture");
        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("bootstrap preserves archive cleanup disposition");
        let unresolved = reopened
            .list_unresolved_provider_credential_operations()
            .expect("archive cleanup remains visible");
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].cleanup_archives_connection);
        assert_eq!(vault.counts().3, 0, "bootstrap never replays native delete");

        remove_provider_credential_with(&vault, &reopened, "archive-cleanup-restart", true)
            .await
            .expect("explicit retry completes the persisted archive disposition");
        assert_eq!(vault.counts().3, 1);
        assert!(
            reopened
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "archive-cleanup-restart")
        );
        drop(vault);
        drop(reopened);
        let final_reopen = ShellApi::open_data_root(root.path()).expect("reopen archived root");
        assert!(
            final_reopen
                .list_unresolved_provider_credential_operations()
                .expect("archive cleanup terminal")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn prepared_available_archive_missing_closes_without_blocking_bootstrap() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "prepared-archive-drift");
        shell
            .prepare_provider_credential_operation(
                "prepared-archive-drift",
                ProviderCredentialOperationKindInput::RemoveForArchive,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare archive from available slot");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "must-not-capture");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("prepared drift is conservatively classified");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("repeated bootstrap remains idempotent");
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("list reconciled archive");
        assert!(unresolved.is_empty());
        assert!(
            shell
                .list_provider_connections()
                .expect("connection remains active")
                .iter()
                .any(|connection| connection.id == "prepared-archive-drift")
        );
        assert_eq!(vault.counts().3, 0, "recovery must not issue delete");
    }

    #[tokio::test]
    async fn unstarted_uncertain_archive_then_missing_never_blocks_later_bootstrap() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "unstarted-archive-unknown");
        shell
            .prepare_provider_credential_operation(
                "unstarted-archive-unknown",
                ProviderCredentialOperationKindInput::RemoveForArchive,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record unreadable archive preflight");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "must-not-capture");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("later missing slot safely closes unstarted uncertainty");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("next bootstrap sees no unresolved replay");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("uncertain archive settled")
                .is_empty()
        );
        assert!(
            shell
                .list_provider_connections()
                .expect("connection remains active after no-effect")
                .iter()
                .any(|connection| connection.id == "unstarted-archive-unknown")
        );
        assert_eq!(vault.counts().3, 0);
    }
