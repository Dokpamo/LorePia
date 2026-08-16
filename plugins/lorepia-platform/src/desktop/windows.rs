use std::{
    cell::RefCell,
    ffi::OsString,
    fs::File,
    os::windows::ffi::OsStrExt,
    os::windows::fs::MetadataExt,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
};

use ::windows::{
    ApplicationModel::DataTransfer::{Clipboard, StandardDataFormats},
    Security::Credentials::{PasswordCredential, PasswordVault},
    Storage::Pickers::{FileOpenPicker, FileSavePicker},
    Win32::{
        Foundation::{
            E_ABORT, E_POINTER, ERROR_CANCELLED, ERROR_NOT_FOUND, HLOCAL, RPC_E_CHANGED_MODE,
        },
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
        Storage::FileSystem::{
            FILE_DISPOSITION_INFO, FileDispositionInfo, MOVEFILE_REPLACE_EXISTING,
            MOVEFILE_WRITE_THROUGH, MoveFileExW, REPLACE_FILE_FLAGS, ReplaceFileW,
            SetFileInformationByHandle,
        },
        System::{
            DataExchange::GetClipboardSequenceNumber,
            Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
        UI::Shell::IInitializeWithWindow,
    },
    core::{Error as WindowsError, HRESULT, HSTRING, Interface, PCWSTR},
};
use tauri::{AppHandle, Manager, Runtime};
use zeroize::{Zeroize, Zeroizing};

use crate::{
    ClipboardCleanupStatus, CredentialStatus, NativeCaptureStatus, NativeCredential,
    NativeSensitiveText, PlatformError, PlatformErrorCode, PlatformResult,
    validation::{
        validate_credential_read, validate_credential_write, validate_reference,
        validate_sensitive_capture,
    },
};

pub(crate) const PRODUCTION_CREDENTIAL_RESOURCE: &str = "LorePia.ProviderCredential";
pub(crate) const DEVELOPMENT_CREDENTIAL_RESOURCE: &str = "LorePia.ProviderCredential.Development";

const WINDOWS_DPAPI_MAXIMUM_PLAINTEXT_BYTES: usize = 32 * 1024;
const WINDOWS_DPAPI_MAXIMUM_CIPHERTEXT_BYTES: usize = 64 * 1024;
const WINDOWS_DPAPI_MAXIMUM_ENTROPY_BYTES: usize = 4 * 1024;

struct LocalDpapiBlob {
    blob: CRYPT_INTEGER_BLOB,
    sensitive: bool,
}

impl LocalDpapiBlob {
    fn new(sensitive: bool) -> Self {
        Self {
            blob: CRYPT_INTEGER_BLOB::default(),
            sensitive,
        }
    }

    fn as_mut_ptr(&mut self) -> *mut CRYPT_INTEGER_BLOB {
        &raw mut self.blob
    }

    #[allow(unsafe_code)]
    fn copy_and_release(mut self, maximum_bytes: usize) -> PlatformResult<Zeroizing<Vec<u8>>> {
        let length = self.blob.cbData as usize;
        if length == 0 || length > maximum_bytes || self.blob.pbData.is_null() {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
        // SAFETY: a successful DPAPI call initialized this LocalAlloc-owned
        // buffer and `cbData` was bounded before constructing the slice.
        let copied = Zeroizing::new(unsafe {
            std::slice::from_raw_parts(self.blob.pbData.cast_const(), length).to_vec()
        });
        if !self.release() {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
        Ok(copied)
    }

    #[allow(unsafe_code)]
    fn release(&mut self) -> bool {
        if self.blob.pbData.is_null() {
            return true;
        }
        if self.sensitive && self.blob.cbData != 0 {
            // SAFETY: DPAPI returned this writable LocalAlloc-owned allocation
            // with exactly `cbData` initialized bytes. Clear plaintext before
            // returning the allocation to the process heap.
            unsafe {
                std::slice::from_raw_parts_mut(self.blob.pbData, self.blob.cbData as usize)
                    .zeroize();
            }
        }
        // SAFETY: DPAPI requires callers to release `pbData` with LocalFree.
        // A null return value means the allocation was released successfully.
        let remaining = unsafe {
            ::windows::Win32::Foundation::LocalFree(Some(HLOCAL(self.blob.pbData.cast())))
        };
        if remaining.0.is_null() {
            self.blob = CRYPT_INTEGER_BLOB::default();
            true
        } else {
            self.blob.pbData = remaining.0.cast();
            false
        }
    }
}

impl Drop for LocalDpapiBlob {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn dpapi_input_blob(bytes: &[u8], error: PlatformErrorCode) -> PlatformResult<CRYPT_INTEGER_BLOB> {
    if bytes.is_empty() {
        return Err(PlatformError::new(error));
    }
    let length = u32::try_from(bytes.len()).map_err(|_| PlatformError::new(error))?;
    Ok(CRYPT_INTEGER_BLOB {
        cbData: length,
        pbData: bytes.as_ptr().cast_mut(),
    })
}

/// Protects one bounded byte string with the current Windows user's DPAPI key.
///
/// The required entropy is caller-owned protocol context and must be supplied
/// unchanged to `unprotect_current_user_data`. Machine scope is deliberately
/// not enabled, so another local user is not granted decrypt authority.
#[allow(unsafe_code)]
pub(super) fn protect_current_user_data(
    plaintext: &[u8],
    entropy: &[u8],
) -> PlatformResult<Zeroizing<Vec<u8>>> {
    if plaintext.len() > WINDOWS_DPAPI_MAXIMUM_PLAINTEXT_BYTES
        || entropy.len() > WINDOWS_DPAPI_MAXIMUM_ENTROPY_BYTES
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let plaintext = dpapi_input_blob(plaintext, PlatformErrorCode::InvalidInput)?;
    let entropy = dpapi_input_blob(entropy, PlatformErrorCode::InvalidInput)?;
    let mut protected = LocalDpapiBlob::new(true);
    // SAFETY: the two input slices remain live for this synchronous call; all
    // reserved and prompt pointers are null, and the output guard always uses
    // LocalFree. UI_FORBIDDEN makes the primitive non-interactive. Omitting
    // LOCAL_MACHINE keeps protection scoped to the current user.
    unsafe {
        CryptProtectData(
            &raw const plaintext,
            PCWSTR::null(),
            Some(&raw const entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            protected.as_mut_ptr(),
        )
    }
    .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    protected.copy_and_release(WINDOWS_DPAPI_MAXIMUM_CIPHERTEXT_BYTES)
}

/// Unprotects one DPAPI byte string and clears the native plaintext allocation.
#[allow(unsafe_code)]
pub(super) fn unprotect_current_user_data(
    ciphertext: &[u8],
    entropy: &[u8],
    maximum_plaintext_bytes: usize,
) -> PlatformResult<Zeroizing<Vec<u8>>> {
    if ciphertext.len() > WINDOWS_DPAPI_MAXIMUM_CIPHERTEXT_BYTES
        || entropy.len() > WINDOWS_DPAPI_MAXIMUM_ENTROPY_BYTES
        || maximum_plaintext_bytes == 0
        || maximum_plaintext_bytes > WINDOWS_DPAPI_MAXIMUM_PLAINTEXT_BYTES
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let ciphertext = dpapi_input_blob(ciphertext, PlatformErrorCode::CredentialRecoveryRequired)?;
    let entropy = dpapi_input_blob(entropy, PlatformErrorCode::CredentialRecoveryRequired)?;
    let mut plaintext = LocalDpapiBlob::new(true);
    // SAFETY: the inputs remain live for the call, the optional description is
    // not requested, and `plaintext` clears then LocalFree's every output path.
    unsafe {
        CryptUnprotectData(
            &raw const ciphertext,
            None,
            Some(&raw const entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            plaintext.as_mut_ptr(),
        )
    }
    .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    plaintext.copy_and_release(maximum_plaintext_bytes)
}

/// Marks an already-opened and verified file handle for deletion on close.
#[allow(unsafe_code)]
pub(super) fn delete_verified_file(file: &std::fs::File) -> PlatformResult<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    let length = u32::try_from(std::mem::size_of_val(&disposition))
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let handle = ::windows::Win32::Foundation::HANDLE(file.as_raw_handle());
    // SAFETY: `file` remains open through this synchronous call and the input
    // pointer references a correctly sized FILE_DISPOSITION_INFO. The caller
    // opens the same verified handle with DELETE access and exclusive sharing.
    unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfo,
            (&raw const disposition).cast(),
            length,
        )
    }
    .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))
}

pub(super) fn open_verified_staging_file_for_delete(path: &Path) -> PlatformResult<std::fs::File> {
    use ::windows::Win32::{Foundation::GENERIC_READ, Storage::FileSystem::DELETE};
    use std::os::windows::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .access_mode(GENERIC_READ.0 | DELETE.0)
        .share_mode(0)
        .custom_flags(::windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    let metadata = file
        .metadata()
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    if !metadata.is_file()
        || metadata.file_attributes()
            & ::windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT.0
            != 0
    {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok(file)
}

pub(super) struct BoundCredentialOperationGuard {
    handle: ::windows::Win32::Foundation::HANDLE,
    _thread_affinity: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[allow(unsafe_code)]
pub(super) fn lock_bound_credential_operation(
    name: &str,
) -> PlatformResult<BoundCredentialOperationGuard> {
    use ::windows::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};

    let name = HSTRING::from(name);
    // SAFETY: the HSTRING owns a NUL-terminated UTF-16 buffer for the duration
    // of the call. Default security attributes inherit the creating token's
    // DACL; Global is required to serialize the same per-user LocalAppData
    // lifecycle across interactive and RDP sessions.
    let handle = unsafe { CreateMutexW(None, false, &name) }
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    // SAFETY: `handle` is a live mutex handle returned immediately above.
    let outcome = unsafe { WaitForSingleObject(handle, 30_000) };
    if outcome == WAIT_OBJECT_0 || outcome == WAIT_ABANDONED {
        return Ok(BoundCredentialOperationGuard {
            handle,
            _thread_affinity: std::marker::PhantomData,
        });
    }
    // SAFETY: the handle is live and was not acquired by this thread.
    let _ = unsafe { ::windows::Win32::Foundation::CloseHandle(handle) };
    Err(PlatformError::new(if outcome == WAIT_TIMEOUT {
        PlatformErrorCode::Busy
    } else {
        PlatformErrorCode::CredentialRecoveryRequired
    }))
}

#[allow(unsafe_code)]
impl Drop for BoundCredentialOperationGuard {
    fn drop(&mut self) {
        // SAFETY: this guard is constructed only after this thread acquired the
        // mutex; ReleaseMutex is followed by the single owning CloseHandle.
        let _ = unsafe { ReleaseMutex(self.handle) };
        let _ = unsafe { ::windows::Win32::Foundation::CloseHandle(self.handle) };
    }
}

#[allow(unsafe_code)]
pub(super) fn publish_file_no_replace(source: &Path, destination: &Path) -> PlatformResult<()> {
    let source = null_terminated_wide(source);
    let destination = null_terminated_wide(destination);
    // SAFETY: both buffers are NUL-terminated, remain alive through the
    // synchronous call, and source/destination share the locator directory.
    // Omitting REPLACE_EXISTING makes the final publication add-only.
    unsafe {
        MoveFileExW(
            PCWSTR::from_raw(source.as_ptr()),
            PCWSTR::from_raw(destination.as_ptr()),
            MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))
}

#[allow(unsafe_code)]
pub(crate) async fn pick_file<R: Runtime>(app: &AppHandle<R>) -> PlatformResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let window_app = app.clone();
    app.run_on_main_thread(move || {
        let operation = (|| {
            let window = window_app
                .get_webview_window("main")
                .ok_or_else(WindowsError::empty)?;
            let picker = FileOpenPicker::new()?;
            let initialize: IInitializeWithWindow = picker.cast()?;
            let hwnd = window.hwnd().map_err(|_| WindowsError::empty())?;
            // SAFETY: the HWND comes directly from the live Tauri main window
            // and this closure runs on Tauri's UI thread.
            unsafe {
                initialize.Initialize(hwnd)?;
            }
            picker.FileTypeFilter()?.Append(&HSTRING::from("*"))?;
            picker.PickSingleFileAsync()
        })();
        let _ = sender.send(operation);
    })
    .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;

    let operation = receiver
        .await
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;

    tokio::task::spawn_blocking(move || {
        let _apartment = WinRtApartment::enter()
            .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        let selected = match operation.get() {
            Ok(selected) => selected,
            Err(error) if is_picker_cancellation(&error) => return Ok(None),
            Err(_) => {
                return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
            }
        };
        let path = selected
            .Path()
            .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        if path.is_empty() {
            return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
        }
        Ok(Some(PathBuf::from(OsString::from(&path))))
    })
    .await
    .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?
}

#[allow(unsafe_code)]
pub(crate) async fn pick_export_destination<R: Runtime>(
    app: &AppHandle<R>,
    suggested_name: &str,
) -> PlatformResult<Option<PathBuf>> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let window_app = app.clone();
    let suggested_name = suggested_name.to_owned();
    app.run_on_main_thread(move || {
        let operation = (|| {
            let window = window_app
                .get_webview_window("main")
                .ok_or_else(WindowsError::empty)?;
            let picker = FileSavePicker::new()?;
            let initialize: IInitializeWithWindow = picker.cast()?;
            let hwnd = window.hwnd().map_err(|_| WindowsError::empty())?;
            // SAFETY: the HWND comes from the live Tauri main window and this
            // closure is scheduled on Tauri's UI thread.
            unsafe {
                initialize.Initialize(hwnd)?;
            }
            picker.SetSuggestedFileName(&HSTRING::from(&suggested_name))?;
            let extension = Path::new(&suggested_name)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| format!(".{extension}"))
                .ok_or_else(WindowsError::empty)?;
            picker.SetDefaultFileExtension(&HSTRING::from(&extension))?;

            // FileSavePicker requires at least one native file-type choice.
            // Reuse a WinRT-owned string vector rather than constructing a
            // generic filesystem or shell surface.
            let extension_vector = FileOpenPicker::new()?.FileTypeFilter()?;
            extension_vector.Append(&HSTRING::from(&extension))?;
            picker
                .FileTypeChoices()?
                .Insert(&HSTRING::from("LorePia content source"), &extension_vector)?;
            picker.PickSaveFileAsync()
        })();
        let _ = sender.send(operation);
    })
    .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;

    let operation = receiver
        .await
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    tokio::task::spawn_blocking(move || {
        let _apartment = WinRtApartment::enter()
            .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        let selected = match operation.get() {
            Ok(selected) => selected,
            Err(error) if is_picker_cancellation(&error) => return Ok(None),
            Err(_) => return Err(PlatformError::new(PlatformErrorCode::SelectionFailed)),
        };
        let path = selected
            .Path()
            .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        if path.is_empty() {
            return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
        }
        Ok(Some(PathBuf::from(OsString::from(&path))))
    })
    .await
    .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?
}

#[allow(unsafe_code)]
pub(crate) fn atomic_replace_file(source: &Path, destination: &Path) -> PlatformResult<()> {
    let source = null_terminated_wide(source);
    let destination_wide = null_terminated_wide(destination);
    let source_ptr = PCWSTR::from_raw(source.as_ptr());
    let destination_ptr = PCWSTR::from_raw(destination_wide.as_ptr());
    let result = if destination.exists() {
        // SAFETY: both UTF-16 buffers are NUL-terminated and remain alive for
        // the synchronous call. No backup path or callback pointers are used.
        unsafe {
            ReplaceFileW(
                destination_ptr,
                source_ptr,
                PCWSTR::null(),
                REPLACE_FILE_FLAGS(0),
                None,
                None,
            )
        }
    } else {
        // SAFETY: the same validated buffers remain alive. WRITE_THROUGH makes
        // the atomic move durable before success is returned.
        unsafe {
            MoveFileExW(
                source_ptr,
                destination_ptr,
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    result.map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))
}

fn null_terminated_wide(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[allow(unsafe_code)]
pub(crate) fn capture_clipboard_text(maximum_bytes: usize) -> PlatformResult<NativeSensitiveText> {
    let _apartment = WinRtApartment::enter()?;
    // SAFETY: this process-global query has no pointer arguments and is valid
    // after entering the WinRT apartment.
    let sequence = unsafe { GetClipboardSequenceNumber() };
    let mut value = Zeroizing::new(clipboard_text()?);
    validate_sensitive_capture(value.as_str(), maximum_bytes)?;

    // A value-only comparison permits an ABA replacement. Require the native
    // clipboard sequence to remain exact before clearing the captured value.
    let current = clipboard_text().map(Zeroizing::new);
    // SAFETY: this is the same argument-free process-global query as above.
    let current_sequence = unsafe { GetClipboardSequenceNumber() };
    let clipboard_cleanup = match current {
        Ok(current)
            if sequence != 0
                && current_sequence == sequence
                && current.as_str() == value.as_str() =>
        {
            if Clipboard::Clear().is_ok() && clipboard_has_text().is_ok_and(|has_text| !has_text) {
                ClipboardCleanupStatus::Cleared
            } else {
                ClipboardCleanupStatus::ClearFailed
            }
        }
        Ok(_) if sequence != 0 => ClipboardCleanupStatus::AlreadyReplaced,
        Ok(_) | Err(_) => ClipboardCleanupStatus::ClearFailed,
    };
    Ok(NativeSensitiveText::new(
        std::mem::take(&mut *value),
        NativeCaptureStatus { clipboard_cleanup },
    ))
}

fn clipboard_text() -> PlatformResult<String> {
    let content = Clipboard::GetContent()
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
    let text_format = StandardDataFormats::Text()
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
    if !content
        .Contains(&text_format)
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let value = content
        .GetTextAsync()
        .and_then(|operation| operation.get())
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
    String::try_from(&value).map_err(|_| PlatformError::new(PlatformErrorCode::InvalidInput))
}

fn clipboard_has_text() -> PlatformResult<bool> {
    let content = Clipboard::GetContent()
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
    let text_format = StandardDataFormats::Text()
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
    content
        .Contains(&text_format)
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))
}

pub(crate) fn credential_status(
    resource: &str,
    reference: &str,
) -> PlatformResult<CredentialStatus> {
    validate_resource(resource)?;
    super::with_validated_windows_raw_credential_reference(reference, || {
        let _apartment = WinRtApartment::enter()?;
        let vault = password_vault()?;
        match retrieve_credential(&vault, resource, reference) {
            Ok(Some(credential)) => {
                let valid = credential_value(&credential)
                    .ok()
                    .is_some_and(|value| validate_credential_read(&value).is_ok());
                Ok(if valid {
                    CredentialStatus::Available
                } else {
                    CredentialStatus::Unreadable
                })
            }
            Ok(None) => Ok(CredentialStatus::Missing),
            Err(_) => Ok(CredentialStatus::Unreadable),
        }
    })
}

pub(crate) fn read_credential(
    resource: &str,
    reference: &str,
) -> PlatformResult<Option<NativeCredential>> {
    validate_resource(resource)?;
    super::with_validated_windows_raw_credential_reference(reference, || {
        let _apartment = WinRtApartment::enter()?;
        let vault = password_vault()?;
        read_credential_from_vault(&vault, resource, reference)
    })
}

fn read_credential_from_vault(
    vault: &PasswordVault,
    resource: &str,
    reference: &str,
) -> PlatformResult<Option<NativeCredential>> {
    let Some(credential) = retrieve_credential(vault, resource, reference)? else {
        return Ok(None);
    };
    let value = credential_value(&credential)?;
    validate_credential_read(value.as_str())
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialUnavailable))?;
    Ok(Some(NativeCredential::new(value.as_str().to_owned())))
}

fn read_bound_credential_from_vault(
    vault: &PasswordVault,
    resource: &str,
    claim: &super::WindowsBoundCredentialClaim,
) -> PlatformResult<Option<NativeCredential>> {
    super::validate_windows_bound_credential_claim(claim)?;
    if claim.file_record_sha256.is_some() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    read_credential_from_vault(vault, resource, &claim.username)?
        .map(|value| super::decode_windows_bound_credential_value(&claim.generation, value))
        .transpose()
}

fn retrieve_bound_credential_from_vault(
    vault: &PasswordVault,
    resource: &str,
    claim: &super::WindowsBoundCredentialClaim,
) -> PlatformResult<Option<PasswordCredential>> {
    super::validate_windows_bound_credential_claim(claim)?;
    if claim.file_record_sha256.is_some() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    let Some(credential) = retrieve_credential(vault, resource, &claim.username)? else {
        return Ok(None);
    };
    let value = credential_value(&credential)?;
    super::decode_windows_bound_credential_value(
        &claim.generation,
        NativeCredential::new(value.as_str().to_owned()),
    )?;
    Ok(Some(credential))
}

pub(crate) fn validate_credential_store(
    resource: &str,
    reference: &str,
    value: &NativeCredential,
) -> PlatformResult<()> {
    validate_resource(resource)?;
    validate_reference(reference)?;
    validate_credential_write(value.expose())?;
    super::validate_windows_bound_credential_value_size(value)
}

pub(crate) fn store_prevalidated_credential(
    resource: &str,
    reference: &str,
    value: NativeCredential,
) -> PlatformResult<()> {
    super::with_validated_windows_raw_credential_reference(reference, || {
        let _apartment = WinRtApartment::enter()?;
        let vault = password_vault()?;
        let replacement = new_credential(resource, reference, value.expose())?;

        let previous = retrieve_credential(&vault, resource, reference)?;
        let previous_value = previous.as_ref().map(credential_value).transpose()?;
        if let Some(previous) = previous.as_ref() {
            vault.Remove(previous).map_err(credential_error)?;
        }

        let replacement_verified = vault.Add(&replacement).is_ok()
            && retrieve_credential(&vault, resource, reference)
                .ok()
                .flatten()
                .and_then(|credential| credential_value(&credential).ok())
                .is_some_and(|stored| stored.as_str() == value.expose());
        if replacement_verified {
            return Ok(());
        }

        let removed_attempt = match retrieve_credential(&vault, resource, reference) {
            Ok(Some(attempted)) => vault.Remove(&attempted).is_ok(),
            Ok(None) => true,
            Err(_) => false,
        };
        if !removed_attempt
            || !restore_credential_value(&vault, resource, reference, previous_value.as_ref())
        {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
        Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable))
    })
}

pub(crate) fn store_prevalidated_bound_credential(
    data_root: &Path,
    resource: &str,
    reference: &str,
    value: NativeCredential,
) -> PlatformResult<()> {
    super::cleanup_windows_bound_credential_staging(data_root, resource);
    let _apartment = WinRtApartment::enter()?;
    let vault = password_vault()?;
    let seed = super::new_windows_bound_credential_file_seed();
    let (claim, record) = super::prepare_windows_bound_credential_file_with(
        data_root,
        resource,
        reference,
        &seed,
        &value,
        protect_current_user_data,
    )?;

    super::store_prevalidated_windows_bound_credential_claim_with(
        data_root,
        reference,
        &claim,
        &value,
        |claimed| {
            debug_assert_eq!(claimed, &claim);
            super::publish_windows_bound_credential_file(data_root, reference, claimed, &record)
        },
        |claimed| {
            super::read_windows_bound_credential_file_value_with(
                data_root,
                resource,
                reference,
                claimed,
                false,
                unprotect_current_user_data,
            )
            .map(|stored| stored.map(|(_, value)| value))
        },
        || read_credential_from_vault(&vault, resource, reference),
    )
}

pub(crate) fn bound_credential_status(
    data_root: &Path,
    resource: &str,
    reference: &str,
) -> PlatformResult<CredentialStatus> {
    Ok(
        if read_bound_credential(data_root, resource, reference)?.is_some() {
            CredentialStatus::Available
        } else {
            CredentialStatus::Missing
        },
    )
}

pub(crate) fn read_bound_credential(
    data_root: &Path,
    resource: &str,
    reference: &str,
) -> PlatformResult<Option<NativeCredential>> {
    validate_resource(resource)?;
    validate_reference(reference)?;
    super::cleanup_windows_bound_credential_staging(data_root, resource);
    let _apartment = WinRtApartment::enter()?;
    let vault = password_vault()?;
    super::read_windows_bound_credential_claim_with(
        data_root,
        reference,
        |claimed| {
            if claimed.file_record_sha256.is_some() {
                super::read_windows_bound_credential_file_value_with(
                    data_root,
                    resource,
                    reference,
                    claimed,
                    false,
                    unprotect_current_user_data,
                )
                .map(|stored| stored.map(|(_, value)| value))
            } else {
                read_bound_credential_from_vault(&vault, resource, claimed)
            }
        },
        || read_credential_from_vault(&vault, resource, reference),
    )
}

pub(crate) fn delete_credential(resource: &str, reference: &str) -> PlatformResult<()> {
    validate_resource(resource)?;
    super::with_validated_windows_raw_credential_reference(reference, || {
        let _apartment = WinRtApartment::enter()?;
        let vault = password_vault()?;
        delete_credential_from_vault(&vault, resource, reference)
    })
}

pub(crate) fn delete_bound_credential(
    data_root: &Path,
    resource: &str,
    reference: &str,
) -> PlatformResult<()> {
    enum StoredBoundCredential {
        PasswordVault(PasswordCredential),
        File(RefCell<Option<File>>),
    }

    validate_resource(resource)?;
    validate_reference(reference)?;
    super::cleanup_windows_bound_credential_staging(data_root, resource);
    let _apartment = WinRtApartment::enter()?;
    let vault = password_vault()?;
    super::delete_windows_bound_credential_claim_with(
        data_root,
        reference,
        |claimed| {
            if claimed.file_record_sha256.is_some() {
                super::read_windows_bound_credential_file_value_with(
                    data_root,
                    resource,
                    reference,
                    claimed,
                    true,
                    unprotect_current_user_data,
                )
                .map(|stored| {
                    stored.map(|(file, value)| {
                        drop(value);
                        StoredBoundCredential::File(RefCell::new(Some(file)))
                    })
                })
            } else {
                retrieve_bound_credential_from_vault(&vault, resource, claimed)
                    .map(|stored| stored.map(StoredBoundCredential::PasswordVault))
            }
        },
        |claimed, credential| match (claimed.file_record_sha256.as_ref(), credential) {
            (Some(_), StoredBoundCredential::File(file)) => {
                let file = file.borrow_mut().take().ok_or_else(|| {
                    PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired)
                })?;
                delete_verified_file(&file)?;
                drop(file);
                Ok(())
            }
            (None, StoredBoundCredential::PasswordVault(credential)) => {
                vault.Remove(credential).map_err(credential_error)
            }
            _ => Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            )),
        },
        || {
            retrieve_credential(&vault, resource, reference)
                .map(|stored| stored.map(StoredBoundCredential::PasswordVault))
        },
        |credential| match credential {
            StoredBoundCredential::PasswordVault(credential) => {
                vault.Remove(credential).map_err(credential_error)
            }
            StoredBoundCredential::File(_) => Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            )),
        },
    )
}

fn delete_credential_from_vault(
    vault: &PasswordVault,
    resource: &str,
    reference: &str,
) -> PlatformResult<()> {
    let Some(previous) = retrieve_credential(vault, resource, reference)? else {
        return Ok(());
    };
    let backup = credential_value(&previous).ok();

    if vault.Remove(&previous).is_ok() {
        return Ok(());
    }

    match retrieve_credential(vault, resource, reference) {
        Ok(Some(_)) => {}
        Ok(None) if restore_credential_value(vault, resource, reference, backup.as_ref()) => {}
        Ok(None) | Err(_) => {
            return Err(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ));
        }
    }
    Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable))
}

fn restore_credential_value(
    vault: &PasswordVault,
    resource: &str,
    reference: &str,
    previous: Option<&Zeroizing<String>>,
) -> bool {
    if let Some(previous) = previous {
        let Ok(previous_item) = new_credential(resource, reference, previous.as_str()) else {
            return false;
        };
        if vault.Add(&previous_item).is_err() {
            return false;
        }
        return retrieve_credential(vault, resource, reference)
            .ok()
            .flatten()
            .and_then(|credential| credential_value(&credential).ok())
            .is_some_and(|stored| stored.as_str() == previous.as_str());
    }
    matches!(retrieve_credential(vault, resource, reference), Ok(None))
}

fn retrieve_credential(
    vault: &PasswordVault,
    resource: &str,
    reference: &str,
) -> PlatformResult<Option<PasswordCredential>> {
    let resource = HSTRING::from(resource);
    let reference = HSTRING::from(reference);
    match vault.Retrieve(&resource, &reference) {
        Ok(credential) => Ok(Some(credential)),
        Err(error) if is_missing_credential(&error) => Ok(None),
        Err(_) => Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable)),
    }
}

fn credential_value(credential: &PasswordCredential) -> PlatformResult<Zeroizing<String>> {
    credential.RetrievePassword().map_err(credential_error)?;
    let password = credential.Password().map_err(credential_error)?;
    let password = String::try_from(&password)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialUnavailable))?;
    Ok(Zeroizing::new(password))
}

fn new_credential(
    resource: &str,
    reference: &str,
    value: &str,
) -> PlatformResult<PasswordCredential> {
    PasswordCredential::CreatePasswordCredential(
        &HSTRING::from(resource),
        &HSTRING::from(reference),
        &HSTRING::from(value),
    )
    .map_err(credential_error)
}

fn password_vault() -> PlatformResult<PasswordVault> {
    PasswordVault::new().map_err(credential_error)
}

fn validate_resource(resource: &str) -> PlatformResult<()> {
    if matches!(
        resource,
        PRODUCTION_CREDENTIAL_RESOURCE | DEVELOPMENT_CREDENTIAL_RESOURCE
    ) {
        Ok(())
    } else {
        Err(PlatformError::new(PlatformErrorCode::InvalidInput))
    }
}

fn is_missing_credential(error: &WindowsError) -> bool {
    error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0)
}

fn is_picker_cancellation(error: &WindowsError) -> bool {
    let code = error.code();
    code == E_ABORT || code == E_POINTER || code == HRESULT::from_win32(ERROR_CANCELLED.0)
}

fn credential_error<E>(_error: E) -> PlatformError {
    PlatformError::new(PlatformErrorCode::CredentialUnavailable)
}

struct WinRtApartment {
    uninitialize: bool,
}

impl WinRtApartment {
    #[allow(unsafe_code)]
    fn enter() -> PlatformResult<Self> {
        // SAFETY: every successful `RoInitialize` on this thread is paired
        // with `RoUninitialize` by this guard. A pre-existing STA is also a
        // valid apartment for PasswordVault and must not be uninitialized here.
        match unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            Ok(()) => Ok(Self { uninitialize: true }),
            Err(error) if error.code() == RPC_E_CHANGED_MODE => Ok(Self {
                uninitialize: false,
            }),
            Err(_) => Err(PlatformError::new(PlatformErrorCode::CredentialUnavailable)),
        }
    }
}

impl Drop for WinRtApartment {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        if self.uninitialize {
            // SAFETY: paired with the successful `RoInitialize` call made by
            // `WinRtApartment::enter` on this same synchronous call stack.
            unsafe {
                RoUninitialize();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WINDOWS_DPAPI_MAXIMUM_CIPHERTEXT_BYTES, WINDOWS_DPAPI_MAXIMUM_ENTROPY_BYTES,
        WINDOWS_DPAPI_MAXIMUM_PLAINTEXT_BYTES, delete_verified_file, protect_current_user_data,
        unprotect_current_user_data,
    };
    use crate::PlatformErrorCode;
    use std::{fs::OpenOptions, io::Read, os::windows::fs::OpenOptionsExt};
    use zeroize::Zeroizing;

    #[test]
    fn windows_dpapi_current_user_round_trip_rejects_wrong_entropy() {
        let plaintext = Zeroizing::new(b"synthetic-windows-dpapi-credential".to_vec());
        let entropy = b"dev.lorepia.windows-dpapi.test-context.v1\0correct";
        let wrong_entropy = b"dev.lorepia.windows-dpapi.test-context.v1\0wrong";

        let ciphertext = protect_current_user_data(plaintext.as_slice(), entropy)
            .unwrap_or_else(|_| panic!("current-user DPAPI protect must succeed"));
        assert_ne!(ciphertext.as_slice(), plaintext.as_slice());
        assert!(
            !ciphertext
                .windows(plaintext.len())
                .any(|window| window == plaintext.as_slice())
        );

        let recovered =
            unprotect_current_user_data(ciphertext.as_slice(), entropy, plaintext.len())
                .unwrap_or_else(|_| panic!("matching entropy must unprotect"));
        assert_eq!(recovered.as_slice(), plaintext.as_slice());

        let Err(error) =
            unprotect_current_user_data(ciphertext.as_slice(), wrong_entropy, plaintext.len())
        else {
            panic!("wrong entropy must fail closed");
        };
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
    }

    #[test]
    fn windows_dpapi_tampered_ciphertext_fails_closed() {
        let plaintext = Zeroizing::new(b"synthetic-windows-dpapi-tamper-target".to_vec());
        let entropy = b"dev.lorepia.windows-dpapi.test-context.v1\0tamper";
        let ciphertext = protect_current_user_data(plaintext.as_slice(), entropy)
            .unwrap_or_else(|_| panic!("current-user DPAPI protect must succeed"));
        let mut tampered = Zeroizing::new(ciphertext.to_vec());
        let index = tampered.len() / 2;
        tampered[index] ^= 0x01;

        let Err(error) = unprotect_current_user_data(tampered.as_slice(), entropy, plaintext.len())
        else {
            panic!("tampered DPAPI ciphertext must fail closed");
        };
        assert_eq!(error.code(), PlatformErrorCode::CredentialRecoveryRequired);
    }

    #[test]
    fn windows_dpapi_rejects_inputs_beyond_fixed_caps() {
        let overlong_plaintext =
            Zeroizing::new(vec![b'x'; WINDOWS_DPAPI_MAXIMUM_PLAINTEXT_BYTES + 1]);
        let overlong_entropy = vec![b'e'; WINDOWS_DPAPI_MAXIMUM_ENTROPY_BYTES + 1];
        let overlong_ciphertext =
            Zeroizing::new(vec![b'c'; WINDOWS_DPAPI_MAXIMUM_CIPHERTEXT_BYTES + 1]);

        assert!(protect_current_user_data(overlong_plaintext.as_slice(), b"entropy").is_err());
        assert!(protect_current_user_data(b"plaintext", &overlong_entropy).is_err());
        assert!(
            unprotect_current_user_data(
                overlong_ciphertext.as_slice(),
                b"entropy",
                WINDOWS_DPAPI_MAXIMUM_PLAINTEXT_BYTES,
            )
            .is_err()
        );
    }

    #[test]
    fn windows_verified_handle_delete_removes_that_open_file() {
        use ::windows::Win32::{
            Foundation::GENERIC_READ,
            Storage::FileSystem::{DELETE, FILE_FLAG_OPEN_REPARSE_POINT},
        };

        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("verified-record");
        std::fs::write(&path, b"verified-before-delete").expect("write fixture");
        let mut file = OpenOptions::new()
            .read(true)
            .access_mode(GENERIC_READ.0 | DELETE.0)
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(&path)
            .expect("open fixture with delete authority");
        let mut actual = Vec::new();
        file.read_to_end(&mut actual).expect("read exact fixture");
        assert_eq!(actual, b"verified-before-delete");

        delete_verified_file(&file).expect("mark the verified handle for deletion");
        drop(file);
        assert!(!path.exists());
    }
}
