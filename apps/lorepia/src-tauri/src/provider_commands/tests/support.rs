    use std::{
        collections::{BTreeMap, BTreeSet},
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        path::Path,
        sync::{Arc, Mutex, mpsc},
        thread,
        time::Duration,
    };

    use lorepia_shell_api as shell;
    use serde_json::json;
    use tauri_plugin_lorepia_platform::{
        BoundCredentialObservation, ClipboardCleanupStatus, CredentialAuthority, CredentialStatus,
        NativeCaptureStatus, NativeCredential, NativeCredentialEffect,
        NativeCredentialEffectContext, PlatformError, PlatformErrorCode, PlatformResult,
    };
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{
        CancelProviderDiscoveryRequest, CapturedDiscoveryCredential,
        CompensationCredentialEffectPolicy, CompensationObserveErrorPolicy,
        ConnectionSlotGuardFuture, CredentialCompensationDeleteOutcome,
        CredentialInstallRecoveryAction, DiscoveryCompensationConfirmation,
        DiscoveryCompensationDriveResult, DiscoveryCredentialCommitCandidate,
        DiscoveryCredentialInstallJournal, DiscoveryCredentialVault, DiscoveryVaultFuture,
        ExistingConnectionCredentialReadFuture, ExistingConnectionCredentialReader,
        MAXIMUM_PROVIDER_CURL_BYTES, NewConnectionSlotGuard,
        PollProviderDiscoveryEventsForSessionRequest, PreparedDiscoveryCredentialStore,
        ProviderDiscoverySessionRequest, begin_provider_discovery_curl_with_reader,
        begin_provider_discovery_with_reader, bounded_secret_curl,
        capture_discovery_credential_for_empty_bound_slot_with,
        capture_precommit_discovery_credential_with, continue_provider_discovery_off_runtime,
        create_provider_connection_with_slot_guard, credential_compensation_delete_outcome,
        credential_for_discovery_action, credential_install_recovery_action,
        delete_and_observe_discovery_bound_slot, discovery_committing_credential_status_with,
        discovery_compensation_confirmation_context, discovery_compensation_credential_authority,
        discovery_credential_authority, drive_provider_discovery_compensation_with,
        observe_discovery_compensation_slot, promote_discovery_credential_lease_with,
        recover_provider_discovery_credential_installs, register_active_discovery_request,
        request_provider_discovery_cancellation, require_started_discovery_credential_install,
        run_provider_discovery_assistant_turn, run_shell_discovery_off_runtime,
        settle_started_discovery_credential_recovery, start_provider_model_sync_with_reader,
        status_only_bound_observation, supply_provider_discovery_curl_evidence_off_runtime,
    };
    use crate::{
        error::{CommandError, CommandResult},
        state::{AppState, DiscoveryCredentialLeaseBinding},
    };

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct FakeDiscoveryBoundKey {
        reference: String,
        authority_id: String,
        binding_sha256: String,
    }

    impl FakeDiscoveryBoundKey {
        fn new(reference: &str, authority: &CredentialAuthority) -> Self {
            Self {
                reference: reference.to_owned(),
                authority_id: authority.authority_id().to_owned(),
                binding_sha256: authority.binding_sha256().to_owned(),
            }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum FakeDiscoveryVaultFault {
        Status,
        Observe,
        PrepareStore,
        StoreAfterEffect,
        StoreRecoveryRequiredAfterEffect,
        DeleteAfterEffect,
    }

    struct FakeDiscoveryVaultState {
        raw_slots: BTreeMap<String, CredentialStatus>,
        bound_slots: BTreeMap<FakeDiscoveryBoundKey, BoundCredentialObservation>,
        bound_slot_to_insert_on_capture: Option<FakeDiscoveryBoundKey>,
        bound_slot_to_insert_after_status: Option<FakeDiscoveryBoundKey>,
        rolled_back_bound_slot_to_restore_before_store: Option<FakeDiscoveryBoundKey>,
        captured_secret: String,
        faults: BTreeSet<FakeDiscoveryVaultFault>,
    }

    struct FakeDiscoveryVault {
        state: Mutex<FakeDiscoveryVaultState>,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeExistingConnectionCredentialReader {
        read: Mutex<Option<crate::credential_operations::ProviderConnectionCredentialRead>>,
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl FakeExistingConnectionCredentialReader {
        fn new(
            read: Option<crate::credential_operations::ProviderConnectionCredentialRead>,
        ) -> Self {
            Self {
                read: Mutex::new(read),
                calls: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl ExistingConnectionCredentialReader for FakeExistingConnectionCredentialReader {
        fn read<'a>(
            &'a self,
            _shell: &'a shell::ShellApi,
            connection_id: &'a str,
        ) -> ExistingConnectionCredentialReadFuture<'a> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake existing credential calls")
                    .push(connection_id.to_owned());
                self.read
                    .lock()
                    .expect("fake existing credential read")
                    .take()
                    .ok_or_else(CommandError::invalid_input)
            })
        }
    }

    impl FakeDiscoveryVault {
        fn new(calls: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                state: Mutex::new(FakeDiscoveryVaultState {
                    raw_slots: BTreeMap::new(),
                    bound_slots: BTreeMap::new(),
                    bound_slot_to_insert_on_capture: None,
                    bound_slot_to_insert_after_status: None,
                    rolled_back_bound_slot_to_restore_before_store: None,
                    captured_secret: "synthetic-discovery-secret".to_owned(),
                    faults: BTreeSet::new(),
                }),
                calls,
            }
        }

        fn insert_raw(&self, reference: &str) {
            self.state
                .lock()
                .expect("fake vault")
                .raw_slots
                .insert(reference.to_owned(), CredentialStatus::Available);
        }

        fn raw_status(&self, reference: &str) -> CredentialStatus {
            self.state
                .lock()
                .expect("fake vault")
                .raw_slots
                .get(reference)
                .copied()
                .unwrap_or(CredentialStatus::Missing)
        }

        fn insert_bound(&self, reference: &str, authority: &CredentialAuthority) {
            self.state.lock().expect("fake vault").bound_slots.insert(
                FakeDiscoveryBoundKey::new(reference, authority),
                BoundCredentialObservation::Match,
            );
        }

        fn insert_bound_during_capture(&self, reference: &str, authority: &CredentialAuthority) {
            self.state
                .lock()
                .expect("fake vault")
                .bound_slot_to_insert_on_capture =
                Some(FakeDiscoveryBoundKey::new(reference, authority));
        }

        fn insert_bound_after_next_status(&self, reference: &str, authority: &CredentialAuthority) {
            self.state
                .lock()
                .expect("fake vault")
                .bound_slot_to_insert_after_status =
                Some(FakeDiscoveryBoundKey::new(reference, authority));
        }

        fn restore_rolled_back_bound_slot_before_next_store(
            &self,
            reference: &str,
            prior_execution_authority: &CredentialAuthority,
        ) {
            self.state
                .lock()
                .expect("fake vault")
                .rolled_back_bound_slot_to_restore_before_store = Some(FakeDiscoveryBoundKey::new(
                reference,
                prior_execution_authority,
            ));
        }

        fn bound_status(
            &self,
            reference: &str,
            authority: &CredentialAuthority,
        ) -> CredentialStatus {
            let state = self.state.lock().expect("fake vault");
            match state
                .bound_slots
                .get(&FakeDiscoveryBoundKey::new(reference, authority))
            {
                None => CredentialStatus::Missing,
                Some(BoundCredentialObservation::Unreadable) => CredentialStatus::Unreadable,
                Some(
                    BoundCredentialObservation::Missing
                    | BoundCredentialObservation::Legacy
                    | BoundCredentialObservation::Match
                    | BoundCredentialObservation::Mismatch,
                ) => CredentialStatus::Available,
            }
        }

        fn fail_status(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::Status);
        }

        fn fail_observe(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::Observe);
        }

        fn restore_observe(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .remove(&FakeDiscoveryVaultFault::Observe);
        }

        fn fail_store_after_effect(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::StoreAfterEffect);
        }

        fn require_recovery_after_store_effect(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::StoreRecoveryRequiredAfterEffect);
        }

        fn fail_prepare_store(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::PrepareStore);
        }

        fn fail_delete_after_effect(&self) {
            self.state
                .lock()
                .expect("fake vault")
                .faults
                .insert(FakeDiscoveryVaultFault::DeleteAfterEffect);
        }
    }

    impl DiscoveryCredentialVault for FakeDiscoveryVault {
        fn status_bound<'a>(
            &'a self,
            reference: &'a str,
            authority: CredentialAuthority,
        ) -> DiscoveryVaultFuture<'a, CredentialStatus> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake calls")
                    .push("vault_bound_status");
                let mut state = self.state.lock().expect("fake vault");
                if state.faults.contains(&FakeDiscoveryVaultFault::Status) {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                let status = match state
                    .bound_slots
                    .get(&FakeDiscoveryBoundKey::new(reference, &authority))
                {
                    None => CredentialStatus::Missing,
                    Some(BoundCredentialObservation::Unreadable) => CredentialStatus::Unreadable,
                    Some(
                        BoundCredentialObservation::Missing
                        | BoundCredentialObservation::Legacy
                        | BoundCredentialObservation::Match
                        | BoundCredentialObservation::Mismatch,
                    ) => CredentialStatus::Available,
                };
                if let Some(key) = state.bound_slot_to_insert_after_status.take() {
                    state
                        .bound_slots
                        .insert(key, BoundCredentialObservation::Match);
                }
                Ok(status)
            })
        }

        fn capture_bound(&self) -> DiscoveryVaultFuture<'_, CapturedDiscoveryCredential> {
            Box::pin(async move {
                self.calls.lock().expect("fake calls").push("capture");
                let mut state = self.state.lock().expect("fake vault");
                if let Some(key) = state.bound_slot_to_insert_on_capture.take() {
                    state
                        .bound_slots
                        .insert(key, BoundCredentialObservation::Match);
                }
                Ok(CapturedDiscoveryCredential {
                    value: NativeCredential::new(state.captured_secret.clone()),
                    status: NativeCaptureStatus {
                        clipboard_cleanup: ClipboardCleanupStatus::Cleared,
                    },
                })
            })
        }

        fn prepare_bound_store(
            &self,
            reference: &str,
            value: NativeCredential,
            authority: &CredentialAuthority,
        ) -> PlatformResult<PreparedDiscoveryCredentialStore> {
            self.calls
                .lock()
                .expect("fake calls")
                .push("vault_prepare_store");
            if self
                .state
                .lock()
                .expect("fake vault")
                .faults
                .contains(&FakeDiscoveryVaultFault::PrepareStore)
            {
                return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
            }
            if value.expose() != "synthetic-discovery-secret" {
                return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
            }
            Ok(PreparedDiscoveryCredentialStore::Fake {
                reference: reference.to_owned(),
                value,
                authority: authority.clone(),
            })
        }

        fn store_prepared(
            &self,
            prepared: PreparedDiscoveryCredentialStore,
        ) -> DiscoveryVaultFuture<'_, ()> {
            Box::pin(async move {
                let (reference, value, authority) = prepared.into_fake();
                self.calls.lock().expect("fake calls").push("vault_store");
                assert_eq!(value.expose(), "synthetic-discovery-secret");
                let mut state = self.state.lock().expect("fake vault");
                if let Some(prior_execution) =
                    state.rolled_back_bound_slot_to_restore_before_store.take()
                {
                    state
                        .bound_slots
                        .insert(prior_execution, BoundCredentialObservation::Match);
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                state.bound_slots.insert(
                    FakeDiscoveryBoundKey::new(&reference, &authority),
                    BoundCredentialObservation::Match,
                );
                if state
                    .faults
                    .contains(&FakeDiscoveryVaultFault::StoreAfterEffect)
                {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                if state
                    .faults
                    .contains(&FakeDiscoveryVaultFault::StoreRecoveryRequiredAfterEffect)
                {
                    return Err(PlatformError::new(
                        PlatformErrorCode::CredentialRecoveryRequired,
                    ));
                }
                Ok(())
            })
        }

        fn observe_bound<'a>(
            &'a self,
            reference: &'a str,
            authority: CredentialAuthority,
        ) -> DiscoveryVaultFuture<'a, BoundCredentialObservation> {
            Box::pin(async move {
                self.calls.lock().expect("fake calls").push("vault_observe");
                let state = self.state.lock().expect("fake vault");
                if state.faults.contains(&FakeDiscoveryVaultFault::Observe) {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                Ok(state
                    .bound_slots
                    .get(&FakeDiscoveryBoundKey::new(reference, &authority))
                    .copied()
                    .unwrap_or(BoundCredentialObservation::Missing))
            })
        }

        fn delete_bound<'a>(
            &'a self,
            reference: &'a str,
            authority: CredentialAuthority,
        ) -> DiscoveryVaultFuture<'a, ()> {
            Box::pin(async move {
                self.calls.lock().expect("fake calls").push("vault_delete");
                let mut state = self.state.lock().expect("fake vault");
                state
                    .bound_slots
                    .remove(&FakeDiscoveryBoundKey::new(reference, &authority));
                if state
                    .faults
                    .contains(&FakeDiscoveryVaultFault::DeleteAfterEffect)
                {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                Ok(())
            })
        }

        fn confirm_compensation(
            &self,
            context: NativeCredentialEffectContext,
        ) -> DiscoveryVaultFuture<'_, DiscoveryCompensationConfirmation> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake calls")
                    .push("vault_confirm_compensation");
                Ok(DiscoveryCompensationConfirmation::Fake {
                    effect: context.effect(),
                    target_id: context.target_id().to_owned(),
                    origin: context.origin().to_owned(),
                    revision: context.revision().to_owned(),
                })
            })
        }
    }

    struct FakeDiscoveryJournal {
        context: Mutex<shell::ProviderDiscoveryCredentialInstallContextDto>,
        next_native_execution_id: Mutex<String>,
        mismatch_started_context: Mutex<bool>,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    struct FakeNewConnectionSlotGuard {
        status: CredentialStatus,
        calls: Mutex<Vec<String>>,
    }

    impl FakeNewConnectionSlotGuard {
        fn new(status: CredentialStatus) -> Self {
            Self {
                status,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl NewConnectionSlotGuard for FakeNewConnectionSlotGuard {
        fn ensure_missing<'a>(&'a self, connection_id: &'a str) -> ConnectionSlotGuardFuture<'a> {
            Box::pin(async move {
                self.calls
                    .lock()
                    .expect("fake slot calls")
                    .push(connection_id.to_owned());
                if self.status == CredentialStatus::Missing {
                    Ok(())
                } else {
                    Err(CommandError::invalid_input())
                }
            })
        }
    }

    impl FakeDiscoveryJournal {
        fn new(
            context: shell::ProviderDiscoveryCredentialInstallContextDto,
            calls: Arc<Mutex<Vec<&'static str>>>,
        ) -> Self {
            Self {
                context: Mutex::new(context),
                next_native_execution_id: Mutex::new(Uuid::new_v4().to_string()),
                mismatch_started_context: Mutex::new(false),
                calls,
            }
        }

        fn next_native_execution_id(&self) -> String {
            self.next_native_execution_id
                .lock()
                .expect("fake native execution")
                .clone()
        }

        fn mismatch_next_started_context(&self) {
            *self
                .mismatch_started_context
                .lock()
                .expect("fake started mismatch") = true;
        }
    }

    fn compensating_started_discovery_fixture(
        root: &Path,
    ) -> (
        shell::ShellApi,
        shell::ProviderDiscoverySessionDto,
        CredentialAuthority,
    ) {
        let fixture =
            shell::test_support::seed_synthetic_started_discovery_credential_install(root)
                .expect("seed exact Started discovery");
        let shell = fixture.shell;
        let started = shell
            .get_provider_discovery(&fixture.install.session_id)
            .expect("load Started session");
        let cancelled = shell
            .cancel_provider_discovery(&started.id, started.revision)
            .expect("request cancellation while commit is in flight");
        assert_eq!(cancelled.state, "committing");
        assert!(cancelled.cancellation_pending);
        shell
            .commit_provider_discovery(&cancelled.id, None)
            .expect_err("missing credential confirmation enters compensation");
        let compensating = shell
            .get_provider_discovery(&cancelled.id)
            .expect("reload compensating session");
        assert_eq!(compensating.state, "compensating");
        let authority_context = shell
            .get_provider_discovery_credential_compensation_authority(&compensating.id)
            .expect("load exact producing operation authority");
        let authority = discovery_compensation_credential_authority(&authority_context)
            .expect("validate exact compensation authority");
        (shell, compensating, authority)
    }
