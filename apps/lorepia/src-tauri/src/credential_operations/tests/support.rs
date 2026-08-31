    use std::{
        collections::BTreeMap,
        fs,
        path::Path,
        sync::{Arc, Mutex},
    };

    use lorepia_shell_api::{
        CreateProviderConnectionInput, ProviderCredentialOperationKindInput,
        ProviderCredentialSlotStatusInput, ProviderNetworkModeInput, ShellApi,
    };
    use sha2::{Digest, Sha256};
    use tauri_plugin_lorepia_platform::{
        BoundCredentialObservation, ClipboardCleanupStatus, CredentialAuthority, CredentialStatus,
        LegacyCredentialObservation, MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES,
        MAXIMUM_LEGACY_CREDENTIAL_BYTES, NativeCaptureStatus, NativeCredential, PlatformError,
        PlatformErrorCode, PlatformResult,
    };
    use tempfile::{TempDir, tempdir};

    use super::{
        CapturedCredential, CommandError, CommandResult, CredentialVault, LegacyCredentialAccess,
        OrdinaryCredentialTargetPolicy, PreparedCredentialStore, VaultFuture,
        capture_legacy_provider_credential_with, capture_provider_connection_credential_with,
        delete_legacy_provider_credential_with, ensure_slot_missing,
        legacy_provider_credential_status_with, operation_authority,
        operation_predecessor_authority, provider_connection_credential_effect_context,
        read_legacy_provider_credential_with, read_provider_connection_credential_with,
        recover_provider_credential_operations_with, recover_provider_credential_slot_garbage_with,
        remove_provider_credential_with, remove_provider_credential_with_policy,
    };
    use tauri_plugin_lorepia_platform::NativeCredentialEffect;

    #[derive(Clone)]
    struct FakeVault {
        state: Arc<Mutex<FakeVaultState>>,
        shell: ShellApi,
    }

    struct FakeVaultState {
        raw_item: FakeItem,
        bound_items: BTreeMap<FakeAuthorityKey, FakeItem>,
        active_bound_key: Option<FakeAuthorityKey>,
        capture_secret: String,
        status_calls: usize,
        capture_calls: usize,
        store_calls: usize,
        delete_calls: usize,
        legacy_observe_calls: usize,
        legacy_read_calls: usize,
        raw_store_calls: usize,
        raw_delete_calls: usize,
        events: Vec<FakeVaultEvent>,
        faults: Vec<FakeVaultFault>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum FakeVaultEvent {
        Observe(FakeAuthorityKey),
        Status(FakeAuthorityKey),
        Store(FakeAuthorityKey),
        Delete(FakeAuthorityKey),
    }

    #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct FakeAuthorityKey {
        authority_id: String,
        binding_sha256: String,
    }

    impl FakeAuthorityKey {
        fn from_authority(authority: &CredentialAuthority) -> Self {
            Self {
                authority_id: authority.authority_id().to_owned(),
                binding_sha256: authority.binding_sha256().to_owned(),
            }
        }

        fn from_bound_item(item: &FakeItem) -> Option<Self> {
            let FakeItem::Bound {
                authority_id,
                binding_sha256,
                ..
            } = item
            else {
                return None;
            };
            Some(Self {
                authority_id: authority_id.clone(),
                binding_sha256: binding_sha256.clone(),
            })
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum FakeVaultFault {
        CaptureOnce,
        PrepareStoreOnce,
        CreateRawSlotAfterCapture,
        StoreBeforeMutation,
        StoreAfterMutation,
        StoreRecoveryRequiredAfterMutation,
        DeleteBeforeMutation,
        DeleteAfterMutation,
        DeleteRecoveryRequiredAfterMutation,
        PreserveDelete,
        ObserveBoundOnce,
        StatusBoundOnce,
        ObserveAndStatusAfterDelete,
    }

    #[derive(Clone)]
    enum FakeItem {
        Missing,
        Raw,
        UnreadableSlot,
        MalformedEnvelope,
        Bound {
            authority_id: String,
            binding_sha256: String,
            secret: String,
        },
    }

    impl FakeVault {
        fn new(shell: ShellApi, item: FakeItem, capture_secret: &str) -> Self {
            let mut raw_item = FakeItem::Missing;
            let mut bound_items = BTreeMap::new();
            let mut active_bound_key = None;
            match item {
                FakeItem::Bound { .. } => {
                    let key = unresolved_fake_authority_key(&shell)
                        .or_else(|| FakeAuthorityKey::from_bound_item(&item))
                        .expect("bound fake item has an authority key");
                    bound_items.insert(key.clone(), item);
                    active_bound_key = Some(key);
                }
                other => raw_item = other,
            }
            Self {
                state: Arc::new(Mutex::new(FakeVaultState {
                    raw_item,
                    bound_items,
                    active_bound_key,
                    capture_secret: capture_secret.to_owned(),
                    status_calls: 0,
                    capture_calls: 0,
                    store_calls: 0,
                    delete_calls: 0,
                    legacy_observe_calls: 0,
                    legacy_read_calls: 0,
                    raw_store_calls: 0,
                    raw_delete_calls: 0,
                    events: Vec::new(),
                    faults: Vec::new(),
                })),
                shell,
            }
        }

        fn new_raw(shell: ShellApi, item: FakeItem, capture_secret: &str) -> Self {
            let vault = Self::new(shell, FakeItem::Missing, capture_secret);
            vault.state.lock().expect("fake vault").raw_item = item;
            vault
        }

        fn replace_item(&self, item: FakeItem) {
            let operation_key = unresolved_fake_authority_key(&self.shell);
            let mut state = self.state.lock().expect("fake vault");
            let bound_key = operation_key
                .or_else(|| state.active_bound_key.clone())
                .or_else(|| FakeAuthorityKey::from_bound_item(&item));
            if let Some(key) = bound_key {
                if matches!(item, FakeItem::Missing) {
                    state.bound_items.remove(&key);
                } else {
                    state.bound_items.insert(key.clone(), item);
                }
                state.active_bound_key = Some(key);
            } else {
                state.raw_item = item;
            }
        }

        fn replace_capture_secret(&self, secret: &str) {
            self.state.lock().expect("fake vault").capture_secret = secret.to_owned();
        }

        fn replace_raw_item(&self, item: FakeItem) {
            self.state.lock().expect("fake vault").raw_item = item;
        }

        fn fail_next_capture(&self) {
            self.inject_fault(FakeVaultFault::CaptureOnce);
        }

        fn fail_next_prepare_store(&self) {
            self.inject_fault(FakeVaultFault::PrepareStoreOnce);
        }

        fn create_raw_slot_after_capture(&self) {
            self.inject_fault(FakeVaultFault::CreateRawSlotAfterCapture);
        }

        fn fail_store_after_mutation(&self) {
            self.inject_fault(FakeVaultFault::StoreAfterMutation);
        }

        fn fail_store_before_mutation(&self) {
            self.inject_fault(FakeVaultFault::StoreBeforeMutation);
        }

        fn require_recovery_after_store_mutation(&self) {
            self.inject_fault(FakeVaultFault::StoreRecoveryRequiredAfterMutation);
        }

        fn fail_delete_after_mutation(&self) {
            self.inject_fault(FakeVaultFault::DeleteAfterMutation);
        }

        fn require_recovery_after_delete_mutation(&self) {
            self.inject_fault(FakeVaultFault::DeleteRecoveryRequiredAfterMutation);
        }

        fn fail_delete_before_mutation(&self) {
            self.inject_fault(FakeVaultFault::DeleteBeforeMutation);
        }

        fn preserve_item_on_delete(&self) {
            self.inject_fault(FakeVaultFault::PreserveDelete);
        }

        fn fail_next_bound_observation_and_status(&self) {
            self.inject_fault(FakeVaultFault::ObserveBoundOnce);
            self.inject_fault(FakeVaultFault::StatusBoundOnce);
        }

        fn fail_next_bound_observation(&self) {
            self.inject_fault(FakeVaultFault::ObserveBoundOnce);
        }

        fn fail_post_delete_observation_and_status(&self) {
            self.inject_fault(FakeVaultFault::ObserveAndStatusAfterDelete);
        }

        fn inject_fault(&self, fault: FakeVaultFault) {
            self.state.lock().expect("fake vault").faults.push(fault);
        }

        fn counts(&self) -> (usize, usize, usize, usize) {
            let state = self.state.lock().expect("fake vault");
            (
                state.status_calls,
                state.capture_calls,
                state.store_calls,
                state.delete_calls,
            )
        }

        fn item(&self) -> FakeItem {
            let state = self.state.lock().expect("fake vault");
            if !matches!(state.raw_item, FakeItem::Missing) {
                return state.raw_item.clone();
            }
            state
                .active_bound_key
                .as_ref()
                .and_then(|key| state.bound_items.get(key))
                .cloned()
                .unwrap_or(FakeItem::Missing)
        }

        fn bound_item(&self) -> FakeItem {
            let state = self.state.lock().expect("fake vault");
            state
                .active_bound_key
                .as_ref()
                .and_then(|key| state.bound_items.get(key))
                .cloned()
                .unwrap_or(FakeItem::Missing)
        }

        fn bound_slot_count(&self) -> usize {
            self.state.lock().expect("fake vault").bound_items.len()
        }

        fn bound_keys(&self) -> Vec<FakeAuthorityKey> {
            self.state
                .lock()
                .expect("fake vault")
                .bound_items
                .keys()
                .cloned()
                .collect()
        }

        fn bound_item_for(&self, key: &FakeAuthorityKey) -> Option<FakeItem> {
            self.state
                .lock()
                .expect("fake vault")
                .bound_items
                .get(key)
                .cloned()
        }

        fn insert_bound_item(&self, key: FakeAuthorityKey, item: FakeItem) {
            let mut state = self.state.lock().expect("fake vault");
            state.bound_items.insert(key, item);
        }

        fn raw_item(&self) -> FakeItem {
            self.state.lock().expect("fake vault").raw_item.clone()
        }

        fn events(&self) -> Vec<FakeVaultEvent> {
            self.state.lock().expect("fake vault").events.clone()
        }

        fn legacy_counts(&self) -> (usize, usize, usize, usize) {
            let state = self.state.lock().expect("fake vault");
            (
                state.legacy_observe_calls,
                state.legacy_read_calls,
                state.raw_store_calls,
                state.raw_delete_calls,
            )
        }
    }

    fn unresolved_fake_authority_key(shell: &ShellApi) -> Option<FakeAuthorityKey> {
        shell
            .list_unresolved_provider_credential_operations()
            .ok()?
            .into_iter()
            .find_map(|operation| {
                Some(FakeAuthorityKey {
                    authority_id: operation.credential_authority_id?,
                    binding_sha256: operation.credential_authority_binding_sha256?,
                })
            })
    }

    struct FakeLegacyAccess {
        allowed: bool,
    }

    struct FakeOrdinaryTargetPolicy {
        aliases_legacy_raw_slot: bool,
    }

    impl OrdinaryCredentialTargetPolicy for FakeOrdinaryTargetPolicy {
        fn aliases_legacy_raw_slot(&self, _connection_id: &str) -> CommandResult<bool> {
            Ok(self.aliases_legacy_raw_slot)
        }
    }

    impl LegacyCredentialAccess for FakeLegacyAccess {
        fn ensure_legacy_raw_access(&self, _provider_profile_id: &str) -> CommandResult<()> {
            self.allowed
                .then_some(())
                .ok_or_else(CommandError::invalid_input)
        }
    }

    impl CredentialVault for FakeVault {
        fn status<'a>(&'a self, _reference: &'a str) -> VaultFuture<'a, CredentialStatus> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                state.status_calls += 1;
                Ok(match state.raw_item {
                    FakeItem::Missing => CredentialStatus::Missing,
                    FakeItem::Raw | FakeItem::MalformedEnvelope | FakeItem::Bound { .. } => {
                        CredentialStatus::Available
                    }
                    FakeItem::UnreadableSlot => CredentialStatus::Unreadable,
                })
            })
        }

        fn observe<'a>(
            &'a self,
            _reference: &'a str,
            authority: CredentialAuthority,
        ) -> VaultFuture<'a, BoundCredentialObservation> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                let key = FakeAuthorityKey::from_authority(&authority);
                state.events.push(FakeVaultEvent::Observe(key.clone()));
                if take_fake_vault_fault(&mut state, FakeVaultFault::ObserveBoundOnce) {
                    return Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable));
                }
                Ok(match state.bound_items.get(&key) {
                    None | Some(FakeItem::Missing) => BoundCredentialObservation::Missing,
                    Some(FakeItem::Raw) => BoundCredentialObservation::Legacy,
                    Some(FakeItem::UnreadableSlot | FakeItem::MalformedEnvelope) => {
                        BoundCredentialObservation::Unreadable
                    }
                    Some(FakeItem::Bound {
                        authority_id,
                        binding_sha256,
                        ..
                    }) if authority_id == authority.authority_id()
                        && binding_sha256 == authority.binding_sha256() =>
                    {
                        BoundCredentialObservation::Match
                    }
                    Some(FakeItem::Bound { .. }) => BoundCredentialObservation::Mismatch,
                })
            })
        }

        fn status_bound<'a>(
            &'a self,
            _reference: &'a str,
            authority: CredentialAuthority,
        ) -> VaultFuture<'a, CredentialStatus> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                state.status_calls += 1;
                let key = FakeAuthorityKey::from_authority(&authority);
                state.events.push(FakeVaultEvent::Status(key.clone()));
                if take_fake_vault_fault(&mut state, FakeVaultFault::StatusBoundOnce) {
                    return Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable));
                }
                Ok(match state.bound_items.get(&key) {
                    None | Some(FakeItem::Missing) => CredentialStatus::Missing,
                    Some(FakeItem::Raw | FakeItem::MalformedEnvelope | FakeItem::Bound { .. }) => {
                        CredentialStatus::Available
                    }
                    Some(FakeItem::UnreadableSlot) => CredentialStatus::Unreadable,
                })
            })
        }

        fn capture_bound(&self) -> VaultFuture<'_, CapturedCredential> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                state.capture_calls += 1;
                if take_fake_vault_fault(&mut state, FakeVaultFault::CaptureOnce) {
                    return Err(tauri_plugin_lorepia_platform::PlatformError::new(
                        tauri_plugin_lorepia_platform::PlatformErrorCode::CredentialUnavailable,
                    ));
                }
                if state.capture_secret.len() > MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES {
                    return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
                }
                if take_fake_vault_fault(&mut state, FakeVaultFault::CreateRawSlotAfterCapture) {
                    state.raw_item = FakeItem::Raw;
                }
                Ok(CapturedCredential {
                    value: NativeCredential::new(state.capture_secret.clone()),
                    status: NativeCaptureStatus {
                        clipboard_cleanup: ClipboardCleanupStatus::Cleared,
                    },
                })
            })
        }

        fn capture_legacy(&self) -> VaultFuture<'_, CapturedCredential> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                state.capture_calls += 1;
                if take_fake_vault_fault(&mut state, FakeVaultFault::CaptureOnce) {
                    return Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable));
                }
                if state.capture_secret.len() > MAXIMUM_LEGACY_CREDENTIAL_BYTES {
                    return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
                }
                Ok(CapturedCredential {
                    value: NativeCredential::new(state.capture_secret.clone()),
                    status: NativeCaptureStatus {
                        clipboard_cleanup: ClipboardCleanupStatus::Cleared,
                    },
                })
            })
        }

        fn prepare_bound_store(
            &self,
            _reference: &str,
            value: NativeCredential,
            authority: &CredentialAuthority,
        ) -> PlatformResult<PreparedCredentialStore> {
            let mut state = self.state.lock().expect("fake vault");
            if take_fake_vault_fault(&mut state, FakeVaultFault::PrepareStoreOnce) {
                return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
            }
            Ok(PreparedCredentialStore::Fake {
                value,
                authority: authority.clone(),
            })
        }

        fn store_prepared(&self, prepared: PreparedCredentialStore) -> VaultFuture<'_, ()> {
            Box::pin(async move {
                let (value, authority) = prepared.into_fake();
                let operation = self
                    .shell
                    .list_unresolved_provider_credential_operations()
                    .expect("read durable store cutpoint")
                    .into_iter()
                    .find(|operation| operation.operation_id == authority.authority_id())
                    .expect("exact install operation exists before store");
                assert_eq!(operation.status, "started");
                assert_eq!(
                    operation.connection_binding_sha256,
                    authority.binding_sha256()
                );
                let mut state = self.state.lock().expect("fake vault");
                state.store_calls += 1;
                let key = FakeAuthorityKey::from_authority(&authority);
                state.events.push(FakeVaultEvent::Store(key.clone()));
                if take_fake_vault_fault(&mut state, FakeVaultFault::StoreBeforeMutation) {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                let item = FakeItem::Bound {
                    authority_id: authority.authority_id().to_owned(),
                    binding_sha256: authority.binding_sha256().to_owned(),
                    secret: value.into_secret_string(),
                };
                state.bound_items.insert(key.clone(), item);
                state.active_bound_key = Some(key);
                if take_fake_vault_fault(&mut state, FakeVaultFault::StoreAfterMutation) {
                    return Err(tauri_plugin_lorepia_platform::PlatformError::new(
                        tauri_plugin_lorepia_platform::PlatformErrorCode::StorageUnavailable,
                    ));
                }
                if take_fake_vault_fault(
                    &mut state,
                    FakeVaultFault::StoreRecoveryRequiredAfterMutation,
                ) {
                    return Err(PlatformError::new(
                        PlatformErrorCode::CredentialRecoveryRequired,
                    ));
                }
                Ok(())
            })
        }

        fn delete_bound<'a>(
            &'a self,
            _reference: &'a str,
            authority: CredentialAuthority,
        ) -> VaultFuture<'a, ()> {
            Box::pin(async move {
                let unresolved = self
                    .shell
                    .list_unresolved_provider_credential_operations()
                    .expect("read durable delete cutpoint");
                let garbage = self
                    .shell
                    .list_provider_credential_slot_garbage()
                    .expect("read durable garbage-collection cutpoint");
                assert!(
                    unresolved.iter().any(|operation| matches!(
                        operation.status.as_str(),
                        "started" | "cleanup_required"
                    )) || garbage.iter().any(|target| {
                        target.status == "started"
                            && target.authority.authority_id == authority.authority_id()
                            && target.authority.connection_binding_sha256
                                == authority.binding_sha256()
                    }),
                    "operation or slot-GC journal must be Started before native delete"
                );
                let mut state = self.state.lock().expect("fake vault");
                state.delete_calls += 1;
                let key = FakeAuthorityKey::from_authority(&authority);
                state.events.push(FakeVaultEvent::Delete(key.clone()));
                if take_fake_vault_fault(&mut state, FakeVaultFault::DeleteBeforeMutation) {
                    return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
                }
                if !take_fake_vault_fault(&mut state, FakeVaultFault::PreserveDelete) {
                    state.bound_items.remove(&key);
                    state.active_bound_key = Some(key);
                }
                if take_fake_vault_fault(&mut state, FakeVaultFault::ObserveAndStatusAfterDelete) {
                    state.faults.push(FakeVaultFault::ObserveBoundOnce);
                    state.faults.push(FakeVaultFault::StatusBoundOnce);
                }
                if take_fake_vault_fault(&mut state, FakeVaultFault::DeleteAfterMutation) {
                    return Err(tauri_plugin_lorepia_platform::PlatformError::new(
                        tauri_plugin_lorepia_platform::PlatformErrorCode::StorageUnavailable,
                    ));
                }
                if take_fake_vault_fault(
                    &mut state,
                    FakeVaultFault::DeleteRecoveryRequiredAfterMutation,
                ) {
                    return Err(PlatformError::new(
                        PlatformErrorCode::CredentialRecoveryRequired,
                    ));
                }
                Ok(())
            })
        }

        fn delete_raw<'a>(&'a self, _reference: &'a str) -> VaultFuture<'a, ()> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                state.raw_delete_calls += 1;
                state.raw_item = FakeItem::Missing;
                Ok(())
            })
        }

        fn read_bound<'a>(
            &'a self,
            _reference: &'a str,
            authority: CredentialAuthority,
        ) -> VaultFuture<'a, Option<NativeCredential>> {
            Box::pin(async move {
                let state = self.state.lock().expect("fake vault");
                let key = FakeAuthorityKey::from_authority(&authority);
                match state.bound_items.get(&key) {
                    None | Some(FakeItem::Missing) => Ok(None),
                    Some(FakeItem::Bound {
                        authority_id,
                        binding_sha256,
                        secret,
                    }) if authority_id == authority.authority_id()
                        && binding_sha256 == authority.binding_sha256() =>
                    {
                        Ok(Some(NativeCredential::new(secret.clone())))
                    }
                    Some(
                        FakeItem::Raw
                        | FakeItem::UnreadableSlot
                        | FakeItem::MalformedEnvelope
                        | FakeItem::Bound { .. },
                    ) => Err(
                        tauri_plugin_lorepia_platform::PlatformError::new(
                            tauri_plugin_lorepia_platform::PlatformErrorCode::CredentialRecoveryRequired,
                        ),
                    ),
                }
            })
        }

        fn observe_legacy<'a>(
            &'a self,
            _reference: &'a str,
        ) -> VaultFuture<'a, LegacyCredentialObservation> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                state.legacy_observe_calls += 1;
                Ok(match state.raw_item {
                    FakeItem::Missing => LegacyCredentialObservation::Missing,
                    FakeItem::Raw => LegacyCredentialObservation::Raw,
                    FakeItem::UnreadableSlot | FakeItem::MalformedEnvelope => {
                        LegacyCredentialObservation::Unreadable
                    }
                    FakeItem::Bound { .. } => LegacyCredentialObservation::Bound,
                })
            })
        }

        fn read_legacy<'a>(
            &'a self,
            _reference: &'a str,
        ) -> VaultFuture<'a, Option<NativeCredential>> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                state.legacy_read_calls += 1;
                match state.raw_item {
                    FakeItem::Missing => Ok(None),
                    FakeItem::Raw => Ok(Some(NativeCredential::new(
                        "synthetic-legacy-raw-secret".to_owned(),
                    ))),
                    FakeItem::Bound { .. }
                    | FakeItem::UnreadableSlot
                    | FakeItem::MalformedEnvelope => Err(
                        tauri_plugin_lorepia_platform::PlatformError::new(
                            tauri_plugin_lorepia_platform::PlatformErrorCode::CredentialRecoveryRequired,
                        ),
                    ),
                }
            })
        }

        fn store_raw<'a>(
            &'a self,
            _reference: &'a str,
            value: NativeCredential,
        ) -> VaultFuture<'a, ()> {
            Box::pin(async move {
                let mut state = self.state.lock().expect("fake vault");
                state.raw_store_calls += 1;
                let _ = value.into_secret_string();
                state.raw_item = FakeItem::Raw;
                Ok(())
            })
        }
    }

    fn take_fake_vault_fault(state: &mut FakeVaultState, fault: FakeVaultFault) -> bool {
        let Some(index) = state
            .faults
            .iter()
            .position(|candidate| *candidate == fault)
        else {
            return false;
        };
        state.faults.swap_remove(index);
        true
    }

    async fn replacement_gc_fixture(
        connection_id: &str,
    ) -> (
        TempDir,
        ShellApi,
        FakeVault,
        FakeAuthorityKey,
        FakeItem,
        FakeAuthorityKey,
    ) {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, connection_id);
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "replacement-secret");
        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect("install authority A");
        let authority_a = shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        let item_a = vault
            .bound_item_for(&key_a)
            .expect("physical authority A slot");
        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect("replace A with authority B");
        let authority_b = shell
            .ensure_provider_credential_access_settled(connection_id)
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
        let garbage = shell
            .list_provider_credential_slot_garbage()
            .expect("superseded A garbage journal");
        assert_eq!(garbage.len(), 1);
        assert_eq!(garbage[0].status, "pending");
        assert_eq!(garbage[0].authority.authority_id, key_a.authority_id);
        assert_eq!(
            garbage[0].authority.connection_binding_sha256,
            key_a.binding_sha256
        );
        (root, shell, vault, key_a, item_a, key_b)
    }

    fn native_authority(key: &FakeAuthorityKey) -> CredentialAuthority {
        CredentialAuthority::new(key.authority_id.clone(), key.binding_sha256.clone())
            .expect("native fake authority")
    }

