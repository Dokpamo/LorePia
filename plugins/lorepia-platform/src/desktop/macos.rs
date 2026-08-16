use std::{path::PathBuf, sync::Mutex};

use core_foundation::{
    array::CFArray,
    base::{CFType, CFTypeRef, TCFType},
    boolean::CFBoolean,
    data::CFData,
    dictionary::CFDictionary,
    string::{CFString, CFStringRef},
};
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSModalResponseOK, NSOpenPanel, NSPasteboard, NSPasteboardTypeString, NSSavePanel,
};
use objc2_foundation::NSString;
use security_framework_sys::{
    access_control::kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
    base::{errSecDuplicateItem, errSecItemNotFound, errSecSuccess},
    item::{
        kSecAttrAccount, kSecAttrService, kSecAttrSynchronizable, kSecClass,
        kSecClassGenericPassword, kSecReturnAttributes, kSecReturnData, kSecReturnPersistentRef,
        kSecUseDataProtectionKeychain, kSecValueData,
    },
    keychain_item::{SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate},
};
use tauri::{AppHandle, Runtime};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    ClipboardCleanupStatus, CredentialStatus, NativeCaptureStatus, NativeCredential,
    NativeSensitiveText, PlatformError, PlatformErrorCode, PlatformResult,
    validation::{
        validate_credential_read, validate_credential_write, validate_reference,
        validate_sensitive_capture,
    },
};

#[allow(unsafe_code)]
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    #[link_name = "kSecAttrAccessible"]
    static SEC_ATTR_ACCESSIBLE: CFStringRef;
    #[link_name = "kSecMatchItemList"]
    static SEC_MATCH_ITEM_LIST: CFStringRef;
    #[link_name = "kSecValuePersistentRef"]
    static SEC_VALUE_PERSISTENT_REF: CFStringRef;
}

static KEYCHAIN_LOCK: Mutex<()> = Mutex::new(());

pub(crate) async fn pick_file<R: Runtime>(app: &AppHandle<R>) -> PlatformResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let selected = MainThreadMarker::new().and_then(|mtm| {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);
            panel.setResolvesAliases(false);
            (panel.runModal() == NSModalResponseOK)
                .then(|| panel.URL())
                .flatten()
                .and_then(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
        });
        let _ = sender.send(selected);
    })
    .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    receiver
        .await
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))
}

pub(crate) async fn pick_export_destination<R: Runtime>(
    app: &AppHandle<R>,
    suggested_name: &str,
) -> PlatformResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let suggested_name = suggested_name.to_owned();
    app.run_on_main_thread(move || {
        let selected = MainThreadMarker::new().and_then(|mtm| {
            let panel = NSSavePanel::savePanel(mtm);
            panel.setNameFieldStringValue(&NSString::from_str(&suggested_name));
            panel.setCanCreateDirectories(true);
            panel.setAllowsOtherFileTypes(true);
            panel.setExtensionHidden(false);
            // NSSavePanel owns the explicit overwrite confirmation. A URL is
            // returned only after the user accepts that native prompt.
            (panel.runModal() == NSModalResponseOK)
                .then(|| panel.URL())
                .flatten()
                .and_then(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()))
        });
        let _ = sender.send(selected);
    })
    .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    receiver
        .await
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))
}

pub(crate) fn capture_clipboard_text(maximum_bytes: usize) -> PlatformResult<NativeSensitiveText> {
    let pasteboard = NSPasteboard::generalPasteboard();
    let change_count = pasteboard.changeCount();
    let mut value = Zeroizing::new(
        pasteboard
            .stringForType(pasteboard_string_type())
            .map(|value| value.to_string())
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?,
    );
    validate_sensitive_capture(value.as_str(), maximum_bytes)?;

    let current = pasteboard
        .stringForType(pasteboard_string_type())
        .map(|current| Zeroizing::new(current.to_string()));
    let unchanged = current
        .as_ref()
        .is_some_and(|current| current.as_str() == value.as_str())
        && pasteboard.changeCount() == change_count;
    let clipboard_cleanup = if unchanged {
        let _ = pasteboard.clearContents();
        if pasteboard.stringForType(pasteboard_string_type()).is_none() {
            ClipboardCleanupStatus::Cleared
        } else {
            ClipboardCleanupStatus::ClearFailed
        }
    } else {
        ClipboardCleanupStatus::AlreadyReplaced
    };
    Ok(NativeSensitiveText::new(
        std::mem::take(&mut *value),
        NativeCaptureStatus { clipboard_cleanup },
    ))
}

#[allow(unsafe_code)]
fn pasteboard_string_type() -> &'static objc2_app_kit::NSPasteboardType {
    // SAFETY: AppKit exports this immutable, process-lifetime pasteboard type
    // constant and the binding exposes it as a shared static reference.
    unsafe { NSPasteboardTypeString }
}

pub(crate) fn credential_status(
    service: &str,
    include_legacy: bool,
    reference: &str,
) -> PlatformResult<CredentialStatus> {
    validate_reference(reference)?;
    let _guard = lock_keychain()?;
    Ok(observe_credential_status_with(
        &mut SystemKeychain,
        service,
        reference,
        include_legacy,
    ))
}

pub(crate) fn bound_credential_status(
    service: &str,
    reference: &str,
) -> PlatformResult<CredentialStatus> {
    validate_reference(reference)?;
    let _guard = lock_keychain()?;
    bound_credential_status_with(&mut SystemKeychain, service, reference)
}

pub(crate) fn read_bound_credential(
    service: &str,
    reference: &str,
) -> PlatformResult<Option<NativeCredential>> {
    validate_reference(reference)?;
    let _guard = lock_keychain()?;
    read_bound_credential_with(&mut SystemKeychain, service, reference)
}

pub(crate) fn read_credential(
    service: &str,
    migrate_legacy: bool,
    reference: &str,
) -> PlatformResult<Option<NativeCredential>> {
    validate_reference(reference)?;
    let _guard = lock_keychain()?;
    read_current_credential(&mut SystemKeychain, service, reference, migrate_legacy)
}

pub(crate) fn validate_credential_store(
    reference: &str,
    value: &NativeCredential,
) -> PlatformResult<()> {
    validate_reference(reference)?;
    validate_credential_write(value.expose())
}

pub(crate) fn store_prevalidated_credential(
    service: &str,
    migrate_legacy: bool,
    reference: &str,
    value: NativeCredential,
) -> PlatformResult<()> {
    let _guard = lock_keychain()?;
    store_prevalidated_credential_with(
        &mut SystemKeychain,
        service,
        reference,
        &value,
        migrate_legacy,
    )
}

pub(crate) fn store_prevalidated_bound_credential(
    service: &str,
    reference: &str,
    value: NativeCredential,
) -> PlatformResult<()> {
    let _guard = lock_keychain()?;
    store_bound_credential_with(&mut SystemKeychain, service, reference, &value)
}

pub(crate) fn delete_credential(
    service: &str,
    migrate_legacy: bool,
    reference: &str,
) -> PlatformResult<()> {
    validate_reference(reference)?;
    let _guard = lock_keychain()?;
    delete_credential_with(&mut SystemKeychain, service, reference, migrate_legacy)
}

pub(crate) fn delete_bound_credential(service: &str, reference: &str) -> PlatformResult<()> {
    validate_reference(reference)?;
    let _guard = lock_keychain()?;
    delete_bound_credential_with(&mut SystemKeychain, service, reference)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum KeychainStore {
    DataProtection,
    Legacy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum KeychainAccessibility {
    Required,
    Other(String),
    Legacy,
}

struct KeychainRecord {
    value: NativeCredential,
    accessibility: KeychainAccessibility,
}

struct BoundKeychainRecord<I> {
    record: KeychainRecord,
    identity: I,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoundDeleteOutcome {
    Deleted,
    NotCurrent,
}

trait KeychainBackend {
    type BoundIdentity;

    fn read(
        &mut self,
        service: &str,
        reference: &str,
        store: KeychainStore,
    ) -> PlatformResult<Option<KeychainRecord>>;

    fn read_bound_data_protection(
        &mut self,
        service: &str,
        reference: &str,
    ) -> PlatformResult<Option<BoundKeychainRecord<Self::BoundIdentity>>>;

    fn add_data_protection_exact(
        &mut self,
        service: &str,
        reference: &str,
        record: &KeychainRecord,
    ) -> PlatformResult<()>;

    fn delete_bound_data_protection_exact(
        &mut self,
        service: &str,
        reference: &str,
        expected: &BoundKeychainRecord<Self::BoundIdentity>,
    ) -> PlatformResult<BoundDeleteOutcome>;

    fn upsert_data_protection(
        &mut self,
        service: &str,
        reference: &str,
        record: &KeychainRecord,
    ) -> PlatformResult<()>;

    fn delete(
        &mut self,
        service: &str,
        reference: &str,
        store: KeychainStore,
    ) -> PlatformResult<()>;
}

struct SystemKeychain;

impl KeychainBackend for SystemKeychain {
    type BoundIdentity = CFData;

    fn read(
        &mut self,
        service: &str,
        reference: &str,
        store: KeychainStore,
    ) -> PlatformResult<Option<KeychainRecord>> {
        system_read_record(service, reference, store)
    }

    fn read_bound_data_protection(
        &mut self,
        service: &str,
        reference: &str,
    ) -> PlatformResult<Option<BoundKeychainRecord<Self::BoundIdentity>>> {
        system_read_bound_data_protection_snapshot(service, reference)
    }

    fn add_data_protection_exact(
        &mut self,
        service: &str,
        reference: &str,
        record: &KeychainRecord,
    ) -> PlatformResult<()> {
        system_add_data_protection_exact(service, reference, record)
    }

    fn delete_bound_data_protection_exact(
        &mut self,
        service: &str,
        reference: &str,
        expected: &BoundKeychainRecord<Self::BoundIdentity>,
    ) -> PlatformResult<BoundDeleteOutcome> {
        system_delete_bound_data_protection_exact(service, reference, expected)
    }

    fn upsert_data_protection(
        &mut self,
        service: &str,
        reference: &str,
        record: &KeychainRecord,
    ) -> PlatformResult<()> {
        system_upsert_data_protection(service, reference, record)
    }

    fn delete(
        &mut self,
        service: &str,
        reference: &str,
        store: KeychainStore,
    ) -> PlatformResult<()> {
        system_delete_record(service, reference, store)
    }
}

fn observe_credential_status_with<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    include_legacy: bool,
) -> CredentialStatus {
    match read_validated_record(backend, service, reference, KeychainStore::DataProtection) {
        Ok(Some(_)) => CredentialStatus::Available,
        Ok(None) if include_legacy => {
            match read_validated_record(backend, service, reference, KeychainStore::Legacy) {
                Ok(Some(_)) => CredentialStatus::Available,
                Ok(None) => CredentialStatus::Missing,
                Err(_) => CredentialStatus::Unreadable,
            }
        }
        Ok(None) => CredentialStatus::Missing,
        Err(_) => CredentialStatus::Unreadable,
    }
}

fn read_current_credential<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    migrate_legacy: bool,
) -> PlatformResult<Option<NativeCredential>> {
    if let Some(record) =
        read_validated_record(backend, service, reference, KeychainStore::DataProtection)?
    {
        let credential = NativeCredential::new(record.value.expose().to_owned());
        harden_data_protection_if_needed(backend, service, reference, &record, &credential)?;
        if migrate_legacy {
            backend.delete(service, reference, KeychainStore::Legacy)?;
        }
        return Ok(Some(credential));
    }
    if !migrate_legacy {
        return Ok(None);
    }

    let Some(record) = read_validated_record(backend, service, reference, KeychainStore::Legacy)?
    else {
        return Ok(None);
    };
    let credential = NativeCredential::new(record.value.expose().to_owned());
    migrate_legacy_credential(backend, service, reference, &credential)?;
    Ok(Some(credential))
}

fn read_bound_credential_with<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
) -> PlatformResult<Option<NativeCredential>> {
    let Some(snapshot) = backend.read_bound_data_protection(service, reference)? else {
        return Ok(None);
    };
    validate_bound_record(&snapshot.record)?;
    Ok(Some(NativeCredential::new(
        snapshot.record.value.expose().to_owned(),
    )))
}

fn validate_bound_record(record: &KeychainRecord) -> PlatformResult<()> {
    validate_credential_read(record.value.expose()).map_err(|_| credential_recovery_required())?;
    if record.accessibility != KeychainAccessibility::Required {
        return Err(credential_recovery_required());
    }
    Ok(())
}

fn bound_credential_status_with<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
) -> PlatformResult<CredentialStatus> {
    Ok(
        match read_bound_credential_with(backend, service, reference)? {
            Some(_) => CredentialStatus::Available,
            None => CredentialStatus::Missing,
        },
    )
}

fn read_validated_record<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    store: KeychainStore,
) -> PlatformResult<Option<KeychainRecord>> {
    let Some(record) = backend.read(service, reference, store)? else {
        return Ok(None);
    };
    validate_credential_read(record.value.expose()).map_err(|_| credential_unavailable())?;
    Ok(Some(record))
}

fn harden_data_protection_if_needed<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    previous: &KeychainRecord,
    credential: &NativeCredential,
) -> PlatformResult<()> {
    if previous.accessibility == KeychainAccessibility::Required
        && previous.value.expose() == credential.expose()
    {
        return Ok(());
    }

    // Continuity requires the frozen native policy exactly. Treat every other
    // accessibility value as drift, even if a particular value could be
    // considered stricter in isolation.
    let hardened = KeychainRecord {
        value: NativeCredential::new(credential.expose().to_owned()),
        accessibility: KeychainAccessibility::Required,
    };
    backend.upsert_data_protection(service, reference, &hardened)?;
    if let Err(error) = verify_data_protection(backend, service, reference, &hardened) {
        restore_data_protection(backend, service, reference, Some(previous))
            .map_err(|_| credential_recovery_required())?;
        return Err(error);
    }
    Ok(())
}

fn migrate_legacy_credential<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    value: &NativeCredential,
) -> PlatformResult<()> {
    validate_credential_read(value.expose()).map_err(|_| credential_unavailable())?;
    let protected = KeychainRecord {
        value: NativeCredential::new(value.expose().to_owned()),
        accessibility: KeychainAccessibility::Required,
    };
    backend.upsert_data_protection(service, reference, &protected)?;
    if let Err(error) = verify_data_protection(backend, service, reference, &protected) {
        // This path is entered only after a protected lookup returned not-found,
        // so deleting the attempted destination cannot remove an older item.
        restore_data_protection(backend, service, reference, None)
            .map_err(|_| credential_recovery_required())?;
        return Err(error);
    }

    // Once the protected copy has been verified, preserve it if the legacy
    // deletion reports failure. That avoids turning an ambiguous delete result
    // into loss of both copies.
    backend.delete(service, reference, KeychainStore::Legacy)
}

#[cfg(test)]
fn store_credential_with<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    value: &NativeCredential,
    migrate_legacy: bool,
) -> PlatformResult<()> {
    validate_credential_write(value.expose())?;
    store_prevalidated_credential_with(backend, service, reference, value, migrate_legacy)
}

fn store_prevalidated_credential_with<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    value: &NativeCredential,
    migrate_legacy: bool,
) -> PlatformResult<()> {
    let previous =
        read_validated_record(backend, service, reference, KeychainStore::DataProtection)?;
    let replacement = KeychainRecord {
        value: NativeCredential::new(value.expose().to_owned()),
        accessibility: KeychainAccessibility::Required,
    };

    backend.upsert_data_protection(service, reference, &replacement)?;
    if let Err(error) = verify_data_protection(backend, service, reference, &replacement) {
        restore_data_protection(backend, service, reference, previous.as_ref())
            .map_err(|_| credential_recovery_required())?;
        return Err(error);
    }
    if migrate_legacy && let Err(error) = backend.delete(service, reference, KeychainStore::Legacy)
    {
        // If this was a new protected item, an ambiguous legacy-delete
        // failure must not trigger deletion of the only verified copy.
        // This matches legacy migration's fail-safe behavior.
        if previous.is_some() {
            restore_data_protection(backend, service, reference, previous.as_ref())
                .map_err(|_| credential_recovery_required())?;
        }
        return Err(error);
    }
    Ok(())
}

fn store_bound_credential_with<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    value: &NativeCredential,
) -> PlatformResult<()> {
    let replacement = KeychainRecord {
        value: NativeCredential::new(value.expose().to_owned()),
        accessibility: KeychainAccessibility::Required,
    };
    backend.add_data_protection_exact(service, reference, &replacement)?;
    verify_bound_data_protection(backend, service, reference, &replacement)
        .map_err(|_| credential_recovery_required())
}

fn verify_bound_data_protection<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    expected: &KeychainRecord,
) -> PlatformResult<()> {
    let actual = backend.read_bound_data_protection(service, reference)?;
    if actual.as_ref().is_some_and(|actual| {
        actual.record.value.expose() == expected.value.expose()
            && actual.record.accessibility == expected.accessibility
    }) {
        Ok(())
    } else {
        Err(credential_recovery_required())
    }
}

fn delete_credential_with<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    migrate_legacy: bool,
) -> PlatformResult<()> {
    let previous =
        read_validated_record(backend, service, reference, KeychainStore::DataProtection)?;
    let result = (|| {
        backend.delete(service, reference, KeychainStore::DataProtection)?;
        verify_absent(backend, service, reference, KeychainStore::DataProtection)?;
        if migrate_legacy {
            backend.delete(service, reference, KeychainStore::Legacy)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        restore_data_protection(backend, service, reference, previous.as_ref())
            .map_err(|_| credential_recovery_required())?;
        return Err(error);
    }
    Ok(())
}

fn delete_bound_credential_with<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
) -> PlatformResult<()> {
    let Some(expected) = backend.read_bound_data_protection(service, reference)? else {
        // A slot which was missing at this cutpoint is already deleted. In
        // particular, never issue a broad delete which could erase an item
        // installed immediately after this observation.
        return Ok(());
    };
    validate_bound_record(&expected.record)?;
    match backend.delete_bound_data_protection_exact(service, reference, &expected) {
        Ok(BoundDeleteOutcome::Deleted) => {}
        Ok(BoundDeleteOutcome::NotCurrent) | Err(_) => {
            return Err(credential_recovery_required());
        }
    }

    // Observe only. A new add-only writer may legitimately win after the
    // persistent-reference delete. Restoring, deleting, or updating here
    // would mutate that unsnapshotted winner, so every occupied or unreadable
    // result requires durable reconciliation instead.
    match backend.read_bound_data_protection(service, reference) {
        Ok(None) => Ok(()),
        Ok(Some(_)) | Err(_) => Err(credential_recovery_required()),
    }
}

fn verify_data_protection<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    expected: &KeychainRecord,
) -> PlatformResult<()> {
    let actual = read_validated_record(backend, service, reference, KeychainStore::DataProtection)?;
    if actual.as_ref().is_some_and(|actual| {
        actual.value.expose() == expected.value.expose()
            && actual.accessibility == expected.accessibility
    }) {
        Ok(())
    } else {
        Err(credential_unavailable())
    }
}

fn restore_data_protection<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    previous: Option<&KeychainRecord>,
) -> PlatformResult<()> {
    if let Some(previous) = previous {
        backend.upsert_data_protection(service, reference, previous)?;
        verify_data_protection(backend, service, reference, previous)?;
    } else {
        backend.delete(service, reference, KeychainStore::DataProtection)?;
        verify_absent(backend, service, reference, KeychainStore::DataProtection)?;
    }
    Ok(())
}

fn verify_absent<B: KeychainBackend>(
    backend: &mut B,
    service: &str,
    reference: &str,
    store: KeychainStore,
) -> PlatformResult<()> {
    if backend.read(service, reference, store)?.is_none() {
        Ok(())
    } else {
        Err(credential_unavailable())
    }
}

fn system_read_record(
    service: &str,
    reference: &str,
    store: KeychainStore,
) -> PlatformResult<Option<KeychainRecord>> {
    let mut query_pairs = keychain_identity_pairs(service, reference, store);
    query_pairs.push((
        security_constant(unsafe_security_constant(SecurityConstant::ReturnData)),
        CFBoolean::true_value().into_CFType(),
    ));
    if store == KeychainStore::DataProtection {
        query_pairs.push((
            security_constant(unsafe_security_constant(SecurityConstant::ReturnAttributes)),
            CFBoolean::true_value().into_CFType(),
        ));
    }

    let Some(result) = copy_matching(&query_pairs)? else {
        return Ok(None);
    };
    match store {
        KeychainStore::DataProtection => decode_data_protection_record(result).map(Some),
        KeychainStore::Legacy => {
            let data = result
                .downcast_into::<CFData>()
                .ok_or_else(credential_unavailable)?;
            Ok(Some(KeychainRecord {
                value: native_credential_from_data(&data)?,
                accessibility: KeychainAccessibility::Legacy,
            }))
        }
    }
}

fn system_read_bound_data_protection_snapshot(
    service: &str,
    reference: &str,
) -> PlatformResult<Option<BoundKeychainRecord<CFData>>> {
    let mut query_pairs =
        keychain_identity_pairs(service, reference, KeychainStore::DataProtection);
    query_pairs.push((
        security_constant(unsafe_security_constant(SecurityConstant::ReturnData)),
        CFBoolean::true_value().into_CFType(),
    ));
    query_pairs.push((
        security_constant(unsafe_security_constant(SecurityConstant::ReturnAttributes)),
        CFBoolean::true_value().into_CFType(),
    ));
    query_pairs.push((
        security_constant(unsafe_security_constant(
            SecurityConstant::ReturnPersistentRef,
        )),
        CFBoolean::true_value().into_CFType(),
    ));
    let Some(result) = copy_matching(&query_pairs)? else {
        return Ok(None);
    };
    decode_bound_data_protection_snapshot(result)
        .map(Some)
        .map_err(|_| credential_recovery_required())
}

#[allow(unsafe_code)]
fn system_delete_bound_data_protection_exact(
    service: &str,
    reference: &str,
    expected: &BoundKeychainRecord<CFData>,
) -> PlatformResult<BoundDeleteOutcome> {
    let Some(actual) =
        system_read_bound_data_protection_by_identity(service, reference, &expected.identity)?
    else {
        return Ok(BoundDeleteOutcome::NotCurrent);
    };
    if !keychain_records_match(&actual, &expected.record) {
        return Ok(BoundDeleteOutcome::NotCurrent);
    }

    // Bound credential publication is add-only. The persistent reference
    // therefore names the exact item observed above: a concurrent delete/add
    // winner receives another reference and cannot be matched by this delete.
    let query_pairs =
        bound_persistent_reference_query_pairs(service, reference, &expected.identity);
    let query = CFDictionary::from_CFType_pairs(&query_pairs);
    // SAFETY: `query` and its retained one-item persistent-reference array
    // remain alive for the call. SecItemDelete does not retain either value.
    let status = unsafe { SecItemDelete(query.as_concrete_TypeRef()) };
    if status == errSecSuccess {
        Ok(BoundDeleteOutcome::Deleted)
    } else if status == errSecItemNotFound {
        Ok(BoundDeleteOutcome::NotCurrent)
    } else {
        Err(credential_unavailable())
    }
}

fn system_read_bound_data_protection_by_identity(
    service: &str,
    reference: &str,
    identity: &CFData,
) -> PlatformResult<Option<KeychainRecord>> {
    let mut query_pairs = bound_persistent_reference_query_pairs(service, reference, identity);
    query_pairs.push((
        security_constant(unsafe_security_constant(SecurityConstant::ReturnData)),
        CFBoolean::true_value().into_CFType(),
    ));
    query_pairs.push((
        security_constant(unsafe_security_constant(SecurityConstant::ReturnAttributes)),
        CFBoolean::true_value().into_CFType(),
    ));
    let Some(result) = copy_matching(&query_pairs)? else {
        return Ok(None);
    };
    decode_data_protection_record(result)
        .map(Some)
        .map_err(|_| credential_recovery_required())
}

fn bound_persistent_reference_query_pairs(
    service: &str,
    reference: &str,
    identity: &CFData,
) -> Vec<(CFString, CFType)> {
    let mut pairs = keychain_identity_pairs(service, reference, KeychainStore::DataProtection);
    let item_list = CFArray::from_CFTypes(std::slice::from_ref(identity));
    pairs.push((
        security_constant(unsafe_security_constant(SecurityConstant::MatchItemList)),
        item_list.into_CFType(),
    ));
    pairs
}

fn keychain_records_match(left: &KeychainRecord, right: &KeychainRecord) -> bool {
    left.value.expose() == right.value.expose() && left.accessibility == right.accessibility
}

#[allow(unsafe_code)]
fn system_add_data_protection_exact(
    service: &str,
    reference: &str,
    record: &KeychainRecord,
) -> PlatformResult<()> {
    if record.accessibility != KeychainAccessibility::Required {
        return Err(credential_recovery_required());
    }
    let mut item_pairs = keychain_identity_pairs(service, reference, KeychainStore::DataProtection);
    item_pairs.extend(credential_attribute_pairs(record)?);
    let item = CFDictionary::from_CFType_pairs(&item_pairs);
    // SAFETY: `item` is a complete generic-password item dictionary and
    // remains alive for the call. A result object is not requested.
    let status = unsafe { SecItemAdd(item.as_concrete_TypeRef(), std::ptr::null_mut()) };
    if status == errSecSuccess {
        Ok(())
    } else if status == errSecDuplicateItem {
        Err(credential_recovery_required())
    } else {
        Err(credential_unavailable())
    }
}

#[allow(unsafe_code)]
fn system_upsert_data_protection(
    service: &str,
    reference: &str,
    record: &KeychainRecord,
) -> PlatformResult<()> {
    if record.accessibility == KeychainAccessibility::Legacy {
        return Err(credential_unavailable());
    }

    let identity_pairs = keychain_identity_pairs(service, reference, KeychainStore::DataProtection);
    let query = CFDictionary::from_CFType_pairs(&identity_pairs);
    let attribute_pairs = credential_attribute_pairs(record)?;
    let attributes = CFDictionary::from_CFType_pairs(&attribute_pairs);

    // SAFETY: both dictionaries remain alive for the duration of the call and
    // contain only Security.framework keys with Core Foundation values of the
    // required types. SecItemUpdate does not retain the dictionary pointers.
    let update_status = unsafe {
        SecItemUpdate(
            query.as_concrete_TypeRef(),
            attributes.as_concrete_TypeRef(),
        )
    };
    if update_status == errSecSuccess {
        return Ok(());
    }
    if update_status != errSecItemNotFound {
        return Err(credential_unavailable());
    }

    let mut item_pairs = identity_pairs;
    item_pairs.extend(credential_attribute_pairs(record)?);
    let item = CFDictionary::from_CFType_pairs(&item_pairs);
    // SAFETY: `item` is a complete generic-password item dictionary and
    // remains alive for the call. A result object is not requested.
    let add_status = unsafe { SecItemAdd(item.as_concrete_TypeRef(), std::ptr::null_mut()) };
    if add_status == errSecSuccess {
        Ok(())
    } else {
        // This deliberately includes `errSecDuplicateItem`: a writer outside
        // this process won the add race, and overwriting it would make snapshot
        // rollback unsafe.
        Err(credential_unavailable())
    }
}

#[allow(unsafe_code)]
fn system_delete_record(
    service: &str,
    reference: &str,
    store: KeychainStore,
) -> PlatformResult<()> {
    let query_pairs = keychain_identity_pairs(service, reference, store);
    let query = CFDictionary::from_CFType_pairs(&query_pairs);
    // SAFETY: `query` remains alive for the call and contains a bounded
    // generic-password identity. SecItemDelete does not retain the pointer.
    let status = unsafe { SecItemDelete(query.as_concrete_TypeRef()) };
    if status == errSecSuccess || status == errSecItemNotFound {
        Ok(())
    } else {
        Err(credential_unavailable())
    }
}

fn keychain_identity_pairs(
    service: &str,
    reference: &str,
    store: KeychainStore,
) -> Vec<(CFString, CFType)> {
    let mut pairs = vec![
        (
            security_constant(unsafe_security_constant(SecurityConstant::Class)),
            security_constant(unsafe_security_constant(
                SecurityConstant::GenericPasswordClass,
            ))
            .into_CFType(),
        ),
        (
            security_constant(unsafe_security_constant(SecurityConstant::Service)),
            CFString::new(service).into_CFType(),
        ),
        (
            security_constant(unsafe_security_constant(SecurityConstant::Account)),
            CFString::new(reference).into_CFType(),
        ),
    ];
    pairs.push((
        security_constant(unsafe_security_constant(
            SecurityConstant::UseDataProtectionKeychain,
        )),
        match store {
            KeychainStore::DataProtection => CFBoolean::true_value(),
            KeychainStore::Legacy => CFBoolean::false_value(),
        }
        .into_CFType(),
    ));
    if store == KeychainStore::DataProtection {
        pairs.push((
            security_constant(unsafe_security_constant(SecurityConstant::Synchronizable)),
            CFBoolean::false_value().into_CFType(),
        ));
    }
    pairs
}

fn credential_attribute_pairs(record: &KeychainRecord) -> PlatformResult<Vec<(CFString, CFType)>> {
    let accessibility = match &record.accessibility {
        KeychainAccessibility::Required => required_accessibility(),
        KeychainAccessibility::Other(value) if !value.is_empty() => CFString::new(value),
        KeychainAccessibility::Other(_) | KeychainAccessibility::Legacy => {
            return Err(credential_unavailable());
        }
    };
    Ok(vec![
        (
            security_constant(unsafe_security_constant(SecurityConstant::ValueData)),
            CFData::from_buffer(record.value.expose().as_bytes()).into_CFType(),
        ),
        (accessible_attribute_key(), accessibility.into_CFType()),
    ])
}

#[allow(unsafe_code)]
fn copy_matching(pairs: &[(CFString, CFType)]) -> PlatformResult<Option<CFType>> {
    let query = CFDictionary::from_CFType_pairs(pairs);
    let mut result: CFTypeRef = std::ptr::null();
    // SAFETY: `query` remains alive for the call, `result` is a valid out
    // pointer, and a non-null successful result follows Core Foundation's
    // create rule and is immediately wrapped below.
    let status = unsafe { SecItemCopyMatching(query.as_concrete_TypeRef(), &raw mut result) };
    let returned = if result.is_null() {
        None
    } else {
        // SAFETY: SecItemCopyMatching returned this non-null object under the
        // create rule. CFType now owns the single corresponding release.
        Some(unsafe { CFType::wrap_under_create_rule(result) })
    };
    if status == errSecSuccess {
        returned.map(Some).ok_or_else(credential_unavailable)
    } else if status == errSecItemNotFound {
        Ok(None)
    } else {
        Err(credential_unavailable())
    }
}

fn decode_data_protection_record(result: CFType) -> PlatformResult<KeychainRecord> {
    let dictionary = result
        .downcast_into::<CFDictionary>()
        .ok_or_else(credential_unavailable)?;
    let data = dictionary_value(
        &dictionary,
        unsafe_security_constant(SecurityConstant::ValueData),
    )?
    .downcast_into::<CFData>()
    .ok_or_else(credential_unavailable)?;
    let accessibility = dictionary_value(
        &dictionary,
        unsafe_security_constant(SecurityConstant::Accessible),
    )?
    .downcast_into::<CFString>()
    .ok_or_else(credential_unavailable)?;
    let accessibility = if accessibility == required_accessibility() {
        KeychainAccessibility::Required
    } else {
        KeychainAccessibility::Other(accessibility.to_string())
    };
    Ok(KeychainRecord {
        value: native_credential_from_data(&data)?,
        accessibility,
    })
}

fn decode_bound_data_protection_snapshot(
    result: CFType,
) -> PlatformResult<BoundKeychainRecord<CFData>> {
    let dictionary = result
        .downcast_into::<CFDictionary>()
        .ok_or_else(credential_unavailable)?;
    let data = dictionary_value(
        &dictionary,
        unsafe_security_constant(SecurityConstant::ValueData),
    )?
    .downcast_into::<CFData>()
    .ok_or_else(credential_unavailable)?;
    let accessibility = dictionary_value(
        &dictionary,
        unsafe_security_constant(SecurityConstant::Accessible),
    )?
    .downcast_into::<CFString>()
    .ok_or_else(credential_unavailable)?;
    let accessibility = if accessibility == required_accessibility() {
        KeychainAccessibility::Required
    } else {
        KeychainAccessibility::Other(accessibility.to_string())
    };
    let persistent_reference = dictionary_value(
        &dictionary,
        unsafe_security_constant(SecurityConstant::ValuePersistentRef),
    )?
    .downcast_into::<CFData>()
    .ok_or_else(credential_unavailable)?;
    Ok(BoundKeychainRecord {
        record: KeychainRecord {
            value: native_credential_from_data(&data)?,
            accessibility,
        },
        identity: persistent_reference,
    })
}

#[allow(unsafe_code)]
fn dictionary_value(dictionary: &CFDictionary, key: CFStringRef) -> PlatformResult<CFType> {
    let value = dictionary
        .find(key.cast::<std::ffi::c_void>())
        .ok_or_else(credential_unavailable)?;
    // SAFETY: `value` is a non-null object owned by `dictionary`. Wrapping
    // under the get rule retains it before the borrowed dictionary is dropped.
    Ok(unsafe { CFType::wrap_under_get_rule(*value) })
}

fn native_credential_from_data(data: &CFData) -> PlatformResult<NativeCredential> {
    if usize::try_from(data.len())
        .ok()
        .is_none_or(|length| length > crate::validation::MAXIMUM_CREDENTIAL_READ_BYTES)
    {
        return Err(credential_unavailable());
    }
    match String::from_utf8(data.bytes().to_vec()) {
        Ok(value) => Ok(NativeCredential::new(value)),
        Err(error) => {
            let mut bytes = error.into_bytes();
            bytes.zeroize();
            Err(credential_unavailable())
        }
    }
}

fn required_accessibility() -> CFString {
    security_constant(unsafe_security_constant(
        SecurityConstant::AccessibleWhenUnlockedThisDeviceOnly,
    ))
}

fn accessible_attribute_key() -> CFString {
    security_constant(unsafe_security_constant(SecurityConstant::Accessible))
}

#[derive(Clone, Copy)]
enum SecurityConstant {
    Class,
    GenericPasswordClass,
    Service,
    Account,
    Synchronizable,
    UseDataProtectionKeychain,
    ReturnData,
    ReturnAttributes,
    ReturnPersistentRef,
    MatchItemList,
    ValueData,
    ValuePersistentRef,
    Accessible,
    AccessibleWhenUnlockedThisDeviceOnly,
}

#[allow(unsafe_code)]
fn unsafe_security_constant(constant: SecurityConstant) -> CFStringRef {
    // SAFETY: these are immutable, process-lifetime CFString constants
    // exported by Security.framework. The wrapper created by callers retains
    // the selected value under Core Foundation's get rule.
    unsafe {
        match constant {
            SecurityConstant::Class => kSecClass,
            SecurityConstant::GenericPasswordClass => kSecClassGenericPassword,
            SecurityConstant::Service => kSecAttrService,
            SecurityConstant::Account => kSecAttrAccount,
            SecurityConstant::Synchronizable => kSecAttrSynchronizable,
            SecurityConstant::UseDataProtectionKeychain => kSecUseDataProtectionKeychain,
            SecurityConstant::ReturnData => kSecReturnData,
            SecurityConstant::ReturnAttributes => kSecReturnAttributes,
            SecurityConstant::ReturnPersistentRef => kSecReturnPersistentRef,
            SecurityConstant::MatchItemList => SEC_MATCH_ITEM_LIST,
            SecurityConstant::ValueData => kSecValueData,
            SecurityConstant::ValuePersistentRef => SEC_VALUE_PERSISTENT_REF,
            SecurityConstant::Accessible => SEC_ATTR_ACCESSIBLE,
            SecurityConstant::AccessibleWhenUnlockedThisDeviceOnly => {
                kSecAttrAccessibleWhenUnlockedThisDeviceOnly
            }
        }
    }
}

#[allow(unsafe_code)]
fn security_constant(value: CFStringRef) -> CFString {
    // SAFETY: callers provide only non-null, immutable process-lifetime
    // Security.framework CFString constants.
    unsafe { CFString::wrap_under_get_rule(value) }
}

fn lock_keychain() -> PlatformResult<std::sync::MutexGuard<'static, ()>> {
    KEYCHAIN_LOCK.lock().map_err(|_| credential_unavailable())
}

fn credential_unavailable() -> PlatformError {
    PlatformError::new(PlatformErrorCode::CredentialUnavailable)
}

fn credential_recovery_required() -> PlatformError {
    PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        BoundDeleteOutcome, BoundKeychainRecord, CredentialStatus, KeychainAccessibility,
        KeychainBackend, KeychainRecord, KeychainStore, NativeCredential, PlatformError,
        PlatformErrorCode, PlatformResult, bound_credential_status_with,
        delete_bound_credential_with, delete_credential_with, migrate_legacy_credential,
        observe_credential_status_with, read_bound_credential_with, read_current_credential,
        store_bound_credential_with, store_credential_with,
    };

    const SERVICE: &str = "dev.lorepia.provider-credentials";
    const REFERENCE: &str = "connection-synthetic";

    struct StoredRecord {
        value: String,
        accessibility: KeychainAccessibility,
        identity: u64,
    }

    #[derive(Default)]
    struct FakeKeychain {
        values: HashMap<KeychainStore, StoredRecord>,
        operations: Vec<&'static str>,
        successful_mutations: usize,
        upsert_count: usize,
        corrupt_upsert_calls: Vec<usize>,
        wrong_accessibility_upsert_calls: Vec<usize>,
        fail_upsert_calls: Vec<usize>,
        fail_legacy_delete: bool,
        fail_protected_delete: bool,
        insert_legacy_on_protected_upsert: Option<String>,
        insert_legacy_on_protected_delete: Option<String>,
        replace_protected_after_exact_add: Option<String>,
        insert_protected_after_bound_read: Option<StoredRecord>,
        replace_protected_before_bound_delete: Option<StoredRecord>,
        insert_protected_after_bound_delete: Option<StoredRecord>,
    }

    impl KeychainBackend for FakeKeychain {
        type BoundIdentity = u64;

        fn read(
            &mut self,
            _service: &str,
            _reference: &str,
            store: KeychainStore,
        ) -> PlatformResult<Option<KeychainRecord>> {
            self.operations.push(match store {
                KeychainStore::DataProtection => "read_protected",
                KeychainStore::Legacy => "read_legacy",
            });
            let result = self.values.get(&store).map(|record| KeychainRecord {
                value: NativeCredential::new(record.value.clone()),
                accessibility: record.accessibility.clone(),
            });
            Ok(result)
        }

        fn read_bound_data_protection(
            &mut self,
            _service: &str,
            _reference: &str,
        ) -> PlatformResult<Option<BoundKeychainRecord<Self::BoundIdentity>>> {
            self.operations.push("read_protected");
            let result = self
                .values
                .get(&KeychainStore::DataProtection)
                .map(|record| BoundKeychainRecord {
                    record: KeychainRecord {
                        value: NativeCredential::new(record.value.clone()),
                        accessibility: record.accessibility.clone(),
                    },
                    identity: record.identity,
                });
            if let Some(winner) = self.insert_protected_after_bound_read.take() {
                self.values.insert(KeychainStore::DataProtection, winner);
            }
            Ok(result)
        }

        fn add_data_protection_exact(
            &mut self,
            _service: &str,
            _reference: &str,
            record: &KeychainRecord,
        ) -> PlatformResult<()> {
            self.operations.push("add_protected");
            if self.values.contains_key(&KeychainStore::DataProtection) {
                return Err(PlatformError::new(
                    PlatformErrorCode::CredentialRecoveryRequired,
                ));
            }
            self.values.insert(
                KeychainStore::DataProtection,
                StoredRecord {
                    value: record.value.expose().to_owned(),
                    accessibility: record.accessibility.clone(),
                    identity: self.successful_mutations as u64 + 1,
                },
            );
            self.successful_mutations += 1;
            if let Some(value) = self.insert_legacy_on_protected_upsert.take() {
                self.values.insert(
                    KeychainStore::Legacy,
                    StoredRecord {
                        value,
                        accessibility: KeychainAccessibility::Legacy,
                        identity: 1,
                    },
                );
            }
            if let Some(value) = self.replace_protected_after_exact_add.take() {
                self.values.insert(
                    KeychainStore::DataProtection,
                    StoredRecord {
                        value,
                        accessibility: KeychainAccessibility::Required,
                        identity: self.successful_mutations as u64 + 1,
                    },
                );
            }
            Ok(())
        }

        fn delete_bound_data_protection_exact(
            &mut self,
            _service: &str,
            _reference: &str,
            expected: &BoundKeychainRecord<Self::BoundIdentity>,
        ) -> PlatformResult<BoundDeleteOutcome> {
            self.operations.push("delete_bound_protected_exact");
            if self.fail_protected_delete {
                return Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable));
            }
            if let Some(winner) = self.replace_protected_before_bound_delete.take() {
                self.values.insert(KeychainStore::DataProtection, winner);
            }
            let is_current =
                self.values
                    .get(&KeychainStore::DataProtection)
                    .is_some_and(|actual| {
                        actual.identity == expected.identity
                            && actual.value == expected.record.value.expose()
                            && actual.accessibility == expected.record.accessibility
                    });
            if !is_current {
                return Ok(BoundDeleteOutcome::NotCurrent);
            }
            self.values.remove(&KeychainStore::DataProtection);
            self.successful_mutations += 1;
            if let Some(winner) = self.insert_protected_after_bound_delete.take() {
                self.values.insert(KeychainStore::DataProtection, winner);
            }
            if let Some(value) = self.insert_legacy_on_protected_delete.take() {
                self.values.insert(
                    KeychainStore::Legacy,
                    StoredRecord {
                        value,
                        accessibility: KeychainAccessibility::Legacy,
                        identity: 1,
                    },
                );
            }
            Ok(BoundDeleteOutcome::Deleted)
        }

        fn upsert_data_protection(
            &mut self,
            _service: &str,
            _reference: &str,
            record: &KeychainRecord,
        ) -> PlatformResult<()> {
            self.operations.push("upsert_protected");
            self.upsert_count += 1;
            if self.fail_upsert_calls.contains(&self.upsert_count) {
                return Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable));
            }
            let value = if self.corrupt_upsert_calls.contains(&self.upsert_count) {
                "corrupted-after-upsert".to_owned()
            } else {
                record.value.expose().to_owned()
            };
            let accessibility = if self
                .wrong_accessibility_upsert_calls
                .contains(&self.upsert_count)
            {
                KeychainAccessibility::Other("unexpected-policy".to_owned())
            } else {
                record.accessibility.clone()
            };
            let identity = self
                .values
                .get(&KeychainStore::DataProtection)
                .map_or(self.successful_mutations as u64 + 1, |record| {
                    record.identity
                });
            self.values.insert(
                KeychainStore::DataProtection,
                StoredRecord {
                    value,
                    accessibility,
                    identity,
                },
            );
            self.successful_mutations += 1;
            if let Some(value) = self.insert_legacy_on_protected_upsert.take() {
                self.values.insert(
                    KeychainStore::Legacy,
                    StoredRecord {
                        value,
                        accessibility: KeychainAccessibility::Legacy,
                        identity: 1,
                    },
                );
            }
            Ok(())
        }

        fn delete(
            &mut self,
            _service: &str,
            _reference: &str,
            store: KeychainStore,
        ) -> PlatformResult<()> {
            self.operations.push(match store {
                KeychainStore::DataProtection => "delete_protected",
                KeychainStore::Legacy => "delete_legacy",
            });
            if store == KeychainStore::DataProtection && self.fail_protected_delete {
                return Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable));
            }
            if store == KeychainStore::Legacy && self.fail_legacy_delete {
                return Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable));
            }
            if store == KeychainStore::DataProtection
                && let Some(winner) = self.replace_protected_before_bound_delete.take()
            {
                self.values.insert(KeychainStore::DataProtection, winner);
            }
            self.values.remove(&store);
            self.successful_mutations += 1;
            if store == KeychainStore::DataProtection
                && let Some(winner) = self.insert_protected_after_bound_delete.take()
            {
                self.values.insert(KeychainStore::DataProtection, winner);
            }
            if store == KeychainStore::DataProtection
                && let Some(value) = self.insert_legacy_on_protected_delete.take()
            {
                self.values.insert(
                    KeychainStore::Legacy,
                    StoredRecord {
                        value,
                        accessibility: KeychainAccessibility::Legacy,
                        identity: 1,
                    },
                );
            }
            Ok(())
        }
    }

    fn stored(value: &str, accessibility: KeychainAccessibility) -> StoredRecord {
        StoredRecord {
            value: value.to_owned(),
            accessibility,
            identity: 1,
        }
    }

    fn stored_with_identity(
        value: &str,
        accessibility: KeychainAccessibility,
        identity: u64,
    ) -> StoredRecord {
        StoredRecord {
            value: value.to_owned(),
            accessibility,
            identity,
        }
    }

    fn assert_stored(
        backend: &FakeKeychain,
        store: KeychainStore,
        value: &str,
        accessibility: &KeychainAccessibility,
    ) {
        let record = backend.values.get(&store).expect("stored record");
        assert_eq!(record.value, value);
        assert_eq!(&record.accessibility, accessibility);
    }

    fn assert_stored_identity(backend: &FakeKeychain, store: KeychainStore, identity: u64) {
        assert_eq!(
            backend.values.get(&store).expect("stored record").identity,
            identity
        );
    }

    fn native_mutation_count(backend: &FakeKeychain) -> usize {
        backend.successful_mutations
    }

    fn legacy_operation_count(backend: &FakeKeychain) -> usize {
        backend
            .operations
            .iter()
            .filter(|operation| operation.ends_with("_legacy"))
            .count()
    }

    #[test]
    fn ordinary_status_observation_does_not_harden_or_delete_parallel_legacy_item() {
        let prior_accessibility = KeychainAccessibility::Other("synthetic-prior-policy".to_owned());
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("ordinary-raw", prior_accessibility.clone()),
        );
        backend.values.insert(
            KeychainStore::Legacy,
            stored("parallel-legacy", KeychainAccessibility::Legacy),
        );

        let status = observe_credential_status_with(&mut backend, SERVICE, REFERENCE, true);

        assert_eq!(status, CredentialStatus::Available);
        assert_eq!(backend.operations, ["read_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "ordinary-raw",
            &prior_accessibility,
        );
        assert_stored(
            &backend,
            KeychainStore::Legacy,
            "parallel-legacy",
            &KeychainAccessibility::Legacy,
        );
    }

    #[test]
    fn ordinary_status_observation_does_not_migrate_legacy_only_item() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::Legacy,
            stored("legacy-raw", KeychainAccessibility::Legacy),
        );

        let status = observe_credential_status_with(&mut backend, SERVICE, REFERENCE, true);

        assert_eq!(status, CredentialStatus::Available);
        assert_eq!(backend.operations, ["read_protected", "read_legacy"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::Legacy,
            "legacy-raw",
            &KeychainAccessibility::Legacy,
        );
        assert!(!backend.values.contains_key(&KeychainStore::DataProtection));
    }

    #[test]
    fn isolated_status_observation_does_not_query_legacy_namespace() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::Legacy,
            stored("legacy-raw", KeychainAccessibility::Legacy),
        );

        let status = observe_credential_status_with(&mut backend, SERVICE, REFERENCE, false);

        assert_eq!(status, CredentialStatus::Missing);
        assert_eq!(backend.operations, ["read_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_eq!(legacy_operation_count(&backend), 0);
    }

    #[test]
    fn bound_store_never_touches_legacy_item_created_during_native_write() {
        let mut backend = FakeKeychain {
            insert_legacy_on_protected_upsert: Some("racing-legacy".to_owned()),
            ..FakeKeychain::default()
        };
        let replacement = NativeCredential::new("bound-envelope".to_owned());

        store_bound_credential_with(&mut backend, SERVICE, REFERENCE, &replacement)
            .expect("bound store");

        assert_eq!(backend.operations, ["add_protected", "read_protected"]);
        assert_eq!(native_mutation_count(&backend), 1);
        assert_eq!(legacy_operation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "bound-envelope",
            &KeychainAccessibility::Required,
        );
        assert_stored(
            &backend,
            KeychainStore::Legacy,
            "racing-legacy",
            &KeychainAccessibility::Legacy,
        );
    }

    #[test]
    fn bound_install_duplicate_race_never_updates_or_deletes_winning_item() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("racing-winner", KeychainAccessibility::Required),
        );
        let attempted = NativeCredential::new("attempted-bound-envelope".to_owned());

        let error = store_bound_credential_with(&mut backend, SERVICE, REFERENCE, &attempted)
            .expect_err("duplicate bound install must fail closed");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(backend.operations, ["add_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "racing-winner",
            &KeychainAccessibility::Required,
        );
    }

    #[test]
    fn bound_install_post_add_race_never_rolls_back_over_new_winner() {
        let mut backend = FakeKeychain {
            replace_protected_after_exact_add: Some("post-add-race-winner".to_owned()),
            ..FakeKeychain::default()
        };
        let attempted = NativeCredential::new("attempted-bound-envelope".to_owned());

        let error = store_bound_credential_with(&mut backend, SERVICE, REFERENCE, &attempted)
            .expect_err("post-add replacement must fail closed");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(backend.operations, ["add_protected", "read_protected"]);
        assert_eq!(native_mutation_count(&backend), 1);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "post-add-race-winner",
            &KeychainAccessibility::Required,
        );
    }

    #[test]
    fn bound_delete_never_touches_legacy_item_created_during_native_delete() {
        let mut backend = FakeKeychain {
            insert_legacy_on_protected_delete: Some("racing-legacy".to_owned()),
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("bound-envelope", KeychainAccessibility::Required),
        );

        delete_bound_credential_with(&mut backend, SERVICE, REFERENCE).expect("bound delete");

        assert_eq!(
            backend.operations,
            [
                "read_protected",
                "delete_bound_protected_exact",
                "read_protected"
            ]
        );
        assert_eq!(native_mutation_count(&backend), 1);
        assert_eq!(legacy_operation_count(&backend), 0);
        assert!(!backend.values.contains_key(&KeychainStore::DataProtection));
        assert_stored(
            &backend,
            KeychainStore::Legacy,
            "racing-legacy",
            &KeychainAccessibility::Legacy,
        );
    }

    #[test]
    fn bound_delete_missing_snapshot_never_deletes_a_late_winner() {
        let mut backend = FakeKeychain {
            insert_protected_after_bound_read: Some(stored_with_identity(
                "late-race-winner",
                KeychainAccessibility::Required,
                101,
            )),
            ..FakeKeychain::default()
        };

        delete_bound_credential_with(&mut backend, SERVICE, REFERENCE)
            .expect("an initially missing bound slot is already deleted");

        assert_eq!(backend.operations, ["read_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "late-race-winner",
            &KeychainAccessibility::Required,
        );
        assert_stored_identity(&backend, KeychainStore::DataProtection, 101);
    }

    #[test]
    fn bound_delete_never_deletes_a_winner_replaced_before_exact_delete() {
        let mut backend = FakeKeychain {
            replace_protected_before_bound_delete: Some(stored_with_identity(
                "authority-owned-envelope",
                KeychainAccessibility::Required,
                202,
            )),
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("authority-owned-envelope", KeychainAccessibility::Required),
        );

        let error = delete_bound_credential_with(&mut backend, SERVICE, REFERENCE)
            .expect_err("a changed bound item must fail closed");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(
            backend.operations,
            ["read_protected", "delete_bound_protected_exact"]
        );
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "authority-owned-envelope",
            &KeychainAccessibility::Required,
        );
        assert_stored_identity(&backend, KeychainStore::DataProtection, 202);
    }

    #[test]
    fn bound_delete_never_restores_over_a_post_delete_winner() {
        let mut backend = FakeKeychain {
            insert_protected_after_bound_delete: Some(stored_with_identity(
                "authority-owned-envelope",
                KeychainAccessibility::Required,
                303,
            )),
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("authority-owned-envelope", KeychainAccessibility::Required),
        );

        let error = delete_bound_credential_with(&mut backend, SERVICE, REFERENCE)
            .expect_err("a post-delete winner must require durable reconciliation");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(
            backend.operations,
            [
                "read_protected",
                "delete_bound_protected_exact",
                "read_protected"
            ]
        );
        assert_eq!(native_mutation_count(&backend), 1);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "authority-owned-envelope",
            &KeychainAccessibility::Required,
        );
        assert_stored_identity(&backend, KeychainStore::DataProtection, 303);
    }

    #[test]
    fn bound_delete_rejects_non_owned_accessibility_without_mutation() {
        let prior_accessibility = KeychainAccessibility::Other("unexpected-policy".to_owned());
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("unowned-envelope", prior_accessibility.clone()),
        );

        let error = delete_bound_credential_with(&mut backend, SERVICE, REFERENCE)
            .expect_err("policy drift cannot be deleted as an authority-owned item");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(backend.operations, ["read_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "unowned-envelope",
            &prior_accessibility,
        );
    }

    #[test]
    fn migration_verifies_protected_copy_before_deleting_legacy() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::Legacy,
            stored(" synthetic-secret\n", KeychainAccessibility::Legacy),
        );
        let value = NativeCredential::new(" synthetic-secret\n".to_owned());

        migrate_legacy_credential(&mut backend, SERVICE, REFERENCE, &value).expect("migration");

        assert_eq!(
            backend.operations,
            ["upsert_protected", "read_protected", "delete_legacy"]
        );
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            " synthetic-secret\n",
            &KeychainAccessibility::Required,
        );
        assert!(!backend.values.contains_key(&KeychainStore::Legacy));
    }

    #[test]
    fn retained_legacy_read_still_migrates_and_returns_verified_item() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::Legacy,
            stored("retained-legacy-value", KeychainAccessibility::Legacy),
        );

        let value = read_current_credential(&mut backend, SERVICE, REFERENCE, true)
            .expect("retained legacy read")
            .expect("migrated credential");

        assert_eq!(value.expose(), "retained-legacy-value");
        assert_eq!(
            backend.operations,
            [
                "read_protected",
                "read_legacy",
                "upsert_protected",
                "read_protected",
                "delete_legacy"
            ]
        );
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "retained-legacy-value",
            &KeychainAccessibility::Required,
        );
        assert!(!backend.values.contains_key(&KeychainStore::Legacy));
    }

    #[test]
    fn legacy_delete_failure_keeps_both_verified_copies() {
        let mut backend = FakeKeychain {
            fail_legacy_delete: true,
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::Legacy,
            stored("synthetic-secret", KeychainAccessibility::Legacy),
        );
        let value = NativeCredential::new("synthetic-secret".to_owned());

        assert!(migrate_legacy_credential(&mut backend, SERVICE, REFERENCE, &value).is_err());
        assert_stored(
            &backend,
            KeychainStore::Legacy,
            "synthetic-secret",
            &KeychainAccessibility::Legacy,
        );
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "synthetic-secret",
            &KeychainAccessibility::Required,
        );
        assert_eq!(
            backend.operations,
            ["upsert_protected", "read_protected", "delete_legacy"]
        );
    }

    #[test]
    fn protected_upsert_failure_never_deletes_or_cleans_legacy() {
        let mut backend = FakeKeychain {
            fail_upsert_calls: vec![1],
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::Legacy,
            stored("synthetic-secret", KeychainAccessibility::Legacy),
        );
        let value = NativeCredential::new("synthetic-secret".to_owned());

        assert!(migrate_legacy_credential(&mut backend, SERVICE, REFERENCE, &value).is_err());
        assert!(backend.values.contains_key(&KeychainStore::Legacy));
        assert_eq!(backend.operations, ["upsert_protected"]);
    }

    #[test]
    fn failed_new_install_verification_deletes_and_verifies_attempted_destination() {
        let mut backend = FakeKeychain {
            corrupt_upsert_calls: vec![1],
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::Legacy,
            stored("synthetic-secret", KeychainAccessibility::Legacy),
        );
        let value = NativeCredential::new("synthetic-secret".to_owned());

        assert!(migrate_legacy_credential(&mut backend, SERVICE, REFERENCE, &value).is_err());
        assert!(backend.values.contains_key(&KeychainStore::Legacy));
        assert!(!backend.values.contains_key(&KeychainStore::DataProtection));
        assert_eq!(
            backend.operations,
            [
                "upsert_protected",
                "read_protected",
                "delete_protected",
                "read_protected"
            ]
        );
    }

    #[test]
    fn replacement_uses_atomic_upsert_without_predelete() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("previous", KeychainAccessibility::Required),
        );
        let replacement = NativeCredential::new(" \nreplacement\t".to_owned());

        store_credential_with(&mut backend, SERVICE, REFERENCE, &replacement, false)
            .expect("replacement");

        assert_eq!(
            backend.operations,
            ["read_protected", "upsert_protected", "read_protected"]
        );
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            " \nreplacement\t",
            &KeychainAccessibility::Required,
        );

        let mut invalid_backend = FakeKeychain::default();
        let blank = NativeCredential::new(" \n\t".to_owned());
        let error = store_credential_with(&mut invalid_backend, SERVICE, REFERENCE, &blank, false)
            .expect_err("blank input");
        assert_eq!(error.code(), PlatformErrorCode::InvalidInput);
        assert!(invalid_backend.operations.is_empty());
    }

    #[test]
    fn read_preserves_opaque_value_without_rewriting_exact_accessibility() {
        let mut backend = FakeKeychain::default();
        let padded = format!(
            "\u{3000}{}synthetic-secret \r",
            " ".repeat(crate::validation::MAXIMUM_CREDENTIAL_WRITE_BYTES)
        );
        backend.values.insert(
            KeychainStore::DataProtection,
            stored(&padded, KeychainAccessibility::Required),
        );

        let value = read_current_credential(&mut backend, SERVICE, REFERENCE, false)
            .expect("read")
            .expect("credential");

        assert_eq!(value.expose(), padded);
        assert_eq!(backend.operations, ["read_protected"]);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            &padded,
            &KeychainAccessibility::Required,
        );
    }

    #[test]
    fn bound_observation_rejects_accessibility_drift_without_mutating_keychain() {
        let drifted_accessibility =
            KeychainAccessibility::Other("synthetic-weaker-policy".to_owned());
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("synthetic-envelope", drifted_accessibility.clone()),
        );

        let Err(error) = read_bound_credential_with(&mut backend, SERVICE, REFERENCE) else {
            panic!("accessibility drift must fail closed");
        };

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(backend.operations, ["read_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "synthetic-envelope",
            &drifted_accessibility,
        );
    }

    #[test]
    fn bound_observation_maps_malformed_native_value_to_recovery_required() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("", KeychainAccessibility::Required),
        );

        let error = read_bound_credential_with(&mut backend, SERVICE, REFERENCE)
            .expect_err("malformed bound value must fail closed");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
        assert_eq!(backend.operations, ["read_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
    }

    #[test]
    fn bound_status_propagates_recovery_required_for_permanent_native_drift() {
        for (value, accessibility) in [
            (
                "bound-envelope",
                KeychainAccessibility::Other("synthetic-weaker-policy".to_owned()),
            ),
            ("", KeychainAccessibility::Required),
        ] {
            let mut backend = FakeKeychain::default();
            backend
                .values
                .insert(KeychainStore::DataProtection, stored(value, accessibility));

            let error = bound_credential_status_with(&mut backend, SERVICE, REFERENCE)
                .expect_err("permanent bound drift must require recovery");

            assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
            assert_eq!(backend.operations, ["read_protected"]);
            assert_eq!(native_mutation_count(&backend), 0);
        }
    }

    #[test]
    fn bound_observation_ignores_legacy_only_item_without_mutating_keychain() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::Legacy,
            stored("synthetic-envelope", KeychainAccessibility::Legacy),
        );

        let value = read_bound_credential_with(&mut backend, SERVICE, REFERENCE)
            .expect("bound observation");

        assert!(value.is_none());
        assert_eq!(backend.operations, ["read_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::Legacy,
            "synthetic-envelope",
            &KeychainAccessibility::Legacy,
        );
        assert!(!backend.values.contains_key(&KeychainStore::DataProtection));
    }

    #[test]
    fn bound_consuming_read_preserves_parallel_legacy_item_without_mutating_keychain() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("current-bound-envelope", KeychainAccessibility::Required),
        );
        backend.values.insert(
            KeychainStore::Legacy,
            stored("retained-legacy-value", KeychainAccessibility::Legacy),
        );

        let value = read_bound_credential_with(&mut backend, SERVICE, REFERENCE)
            .expect("bound consuming read")
            .expect("protected credential");

        assert_eq!(value.expose(), "current-bound-envelope");
        assert_eq!(backend.operations, ["read_protected"]);
        assert_eq!(native_mutation_count(&backend), 0);
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "current-bound-envelope",
            &KeychainAccessibility::Required,
        );
        assert_stored(
            &backend,
            KeychainStore::Legacy,
            "retained-legacy-value",
            &KeychainAccessibility::Legacy,
        );
    }

    #[test]
    fn oversized_legacy_value_is_rejected_before_migration() {
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::Legacy,
            stored(
                &"s".repeat(crate::validation::MAXIMUM_CREDENTIAL_READ_BYTES + 1),
                KeychainAccessibility::Legacy,
            ),
        );

        assert!(read_current_credential(&mut backend, SERVICE, REFERENCE, true).is_err());
        assert_eq!(backend.operations, ["read_protected", "read_legacy"]);
        assert!(!backend.values.contains_key(&KeychainStore::DataProtection));
        assert!(backend.values.contains_key(&KeychainStore::Legacy));
    }

    #[test]
    fn new_store_legacy_delete_failure_keeps_both_verified_copies() {
        let mut backend = FakeKeychain {
            fail_legacy_delete: true,
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::Legacy,
            stored("legacy", KeychainAccessibility::Legacy),
        );
        let replacement = NativeCredential::new("replacement".to_owned());

        assert!(
            store_credential_with(&mut backend, SERVICE, REFERENCE, &replacement, true).is_err()
        );
        assert_stored(
            &backend,
            KeychainStore::Legacy,
            "legacy",
            &KeychainAccessibility::Legacy,
        );
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "replacement",
            &KeychainAccessibility::Required,
        );
        assert_eq!(
            backend.operations,
            [
                "read_protected",
                "upsert_protected",
                "read_protected",
                "delete_legacy"
            ]
        );
    }

    #[test]
    fn read_hardens_wrong_accessibility_before_returning() {
        let previous_accessibility =
            KeychainAccessibility::Other("synthetic-weaker-policy".to_owned());
        let mut backend = FakeKeychain::default();
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("synthetic-secret", previous_accessibility),
        );

        let value = read_current_credential(&mut backend, SERVICE, REFERENCE, false)
            .expect("read")
            .expect("credential");

        assert_eq!(value.expose(), "synthetic-secret");
        assert_eq!(
            backend.operations,
            ["read_protected", "upsert_protected", "read_protected"]
        );
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "synthetic-secret",
            &KeychainAccessibility::Required,
        );
    }

    #[test]
    fn failed_hardening_restores_value_and_exact_previous_accessibility() {
        let previous_accessibility =
            KeychainAccessibility::Other("synthetic-weaker-policy".to_owned());
        let mut backend = FakeKeychain {
            corrupt_upsert_calls: vec![1],
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("previous", previous_accessibility.clone()),
        );

        assert!(read_current_credential(&mut backend, SERVICE, REFERENCE, false).is_err());
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "previous",
            &previous_accessibility,
        );
        assert_eq!(
            backend.operations,
            [
                "read_protected",
                "upsert_protected",
                "read_protected",
                "upsert_protected",
                "read_protected"
            ]
        );
    }

    #[test]
    fn replacement_verification_failure_restores_previous_record_exactly() {
        let previous_accessibility =
            KeychainAccessibility::Other("synthetic-prior-policy".to_owned());
        let mut backend = FakeKeychain {
            corrupt_upsert_calls: vec![1],
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("previous", previous_accessibility.clone()),
        );
        let replacement = NativeCredential::new("replacement".to_owned());

        assert!(
            store_credential_with(&mut backend, SERVICE, REFERENCE, &replacement, false).is_err()
        );
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "previous",
            &previous_accessibility,
        );
    }

    #[test]
    fn wrong_replacement_accessibility_restores_previous_record_exactly() {
        let previous_accessibility =
            KeychainAccessibility::Other("synthetic-prior-policy".to_owned());
        let mut backend = FakeKeychain {
            wrong_accessibility_upsert_calls: vec![1],
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("previous", previous_accessibility.clone()),
        );
        let replacement = NativeCredential::new("replacement".to_owned());

        assert!(
            store_credential_with(&mut backend, SERVICE, REFERENCE, &replacement, false).is_err()
        );
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "previous",
            &previous_accessibility,
        );
    }

    #[test]
    fn failed_restore_is_reported_as_recovery_required() {
        let mut backend = FakeKeychain {
            corrupt_upsert_calls: vec![1],
            fail_upsert_calls: vec![2],
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("previous", KeychainAccessibility::Required),
        );
        let replacement = NativeCredential::new("replacement".to_owned());

        let error = store_credential_with(&mut backend, SERVICE, REFERENCE, &replacement, false)
            .expect_err("recovery required");

        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
    }

    #[test]
    fn delete_legacy_failure_restores_previous_protected_record() {
        let previous_accessibility =
            KeychainAccessibility::Other("synthetic-prior-policy".to_owned());
        let mut backend = FakeKeychain {
            fail_legacy_delete: true,
            ..FakeKeychain::default()
        };
        backend.values.insert(
            KeychainStore::DataProtection,
            stored("previous", previous_accessibility.clone()),
        );
        backend.values.insert(
            KeychainStore::Legacy,
            stored("legacy", KeychainAccessibility::Legacy),
        );

        assert!(delete_credential_with(&mut backend, SERVICE, REFERENCE, true).is_err());
        assert_stored(
            &backend,
            KeychainStore::DataProtection,
            "previous",
            &previous_accessibility,
        );
    }
}
