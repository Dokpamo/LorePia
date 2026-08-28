use std::{
    cell::RefCell,
    ffi::{OsStr, OsString},
    fs::File,
    os::windows::ffi::{OsStrExt, OsStringExt},
    os::windows::fs::MetadataExt,
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::{Path, PathBuf},
};

use ::windows::{
    ApplicationModel::DataTransfer::{Clipboard, StandardDataFormats},
    Security::Credentials::{PasswordCredential, PasswordVault},
    Storage::Pickers::FileOpenPicker,
    Win32::{
        Foundation::{
            E_ABORT, E_POINTER, ERROR_CANCELLED, ERROR_NOT_FOUND, ERROR_SHARING_VIOLATION, HLOCAL,
            RPC_E_CHANGED_MODE,
        },
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
        Storage::FileSystem::{
            FILE_DISPOSITION_INFO, FILE_ID_INFO, FileDispositionInfo, FileIdInfo, FileRenameInfo,
            GetFileInformationByHandleEx, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            SetFileInformationByHandle,
        },
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree},
            DataExchange::GetClipboardSequenceNumber,
            Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject},
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
        },
        UI::{
            Shell::{
                FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_NOTESTFILECREATE, FOS_OVERWRITEPROMPT,
                FOS_PATHMUSTEXIST, FileSaveDialog, IFileSaveDialog, IInitializeWithWindow,
                SIGDN_FILESYSPATH,
            },
            WindowsAndMessaging::{
                IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND, MB_TASKMODAL,
                MessageBoxW,
            },
        },
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

#[allow(unsafe_code)]
pub(crate) async fn confirm_credential_effect<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    informative_text: &str,
) -> PlatformResult<()> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let window_app = app.clone();
    let title = HSTRING::from(title);
    let informative_text = HSTRING::from(informative_text);
    app.run_on_main_thread(move || {
        let result = (|| {
            let window = window_app
                .get_webview_window("main")
                .ok_or_else(|| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
            if !window
                .is_focused()
                .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?
            {
                return Err(PlatformError::new(PlatformErrorCode::PermissionDenied));
            }
            let hwnd = window
                .hwnd()
                .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
            // SAFETY: the owner HWND comes from the live focused Tauri window;
            // both HSTRING buffers remain alive for this synchronous native
            // modal call. Cancel is the default button and the only accepted
            // result is an explicit click on OK.
            let response = unsafe {
                MessageBoxW(
                    Some(hwnd),
                    &informative_text,
                    &title,
                    MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2 | MB_TASKMODAL | MB_SETFOREGROUND,
                )
            };
            if response == IDOK {
                Ok(())
            } else {
                Err(PlatformError::new(PlatformErrorCode::PermissionDenied))
            }
        })();
        let _ = sender.send(result);
    })
    .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?;
    receiver
        .await
        .map_err(|_| PlatformError::new(PlatformErrorCode::PermissionDenied))?
}

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
pub(super) fn delete_verified_file(file: &std::fs::File) -> PlatformResult<()> {
    delete_file_with_error(file, PlatformErrorCode::CredentialRecoveryRequired)
}

fn delete_export_file(file: &std::fs::File) -> PlatformResult<()> {
    delete_file_with_error(file, PlatformErrorCode::StorageUnavailable)
}

#[allow(unsafe_code)]
fn delete_file_with_error(
    file: &std::fs::File,
    error_code: PlatformErrorCode,
) -> PlatformResult<()> {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    let length = u32::try_from(std::mem::size_of_val(&disposition))
        .map_err(|_| PlatformError::new(error_code))?;
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
    .map_err(|_| PlatformError::new(error_code))
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
        let selection = (|| {
            let window = window_app
                .get_webview_window("main")
                .ok_or_else(WindowsError::empty)?;
            let hwnd = window.hwnd().map_err(|_| WindowsError::empty())?;
            let extension = Path::new(&suggested_name)
                .extension()
                .and_then(|extension| extension.to_str())
                .ok_or_else(WindowsError::empty)?;
            let suggested_name = OsStr::new(&suggested_name)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            let extension = OsStr::new(extension)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();

            // SAFETY: Tao initializes the Windows UI thread as an STA. The
            // dialog is owned by the live Tauri window, all PCWSTR buffers stay
            // alive for the synchronous calls, and IFileSaveDialog returns a
            // path-only shell item without creating or truncating that path.
            unsafe {
                let dialog: IFileSaveDialog =
                    CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?;
                let options = dialog.GetOptions()?
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_OVERWRITEPROMPT
                    | FOS_NOCHANGEDIR
                    | FOS_NOTESTFILECREATE;
                dialog.SetOptions(options)?;
                dialog.SetFileName(PCWSTR::from_raw(suggested_name.as_ptr()))?;
                dialog.SetDefaultExtension(PCWSTR::from_raw(extension.as_ptr()))?;
                if let Err(error) = dialog.Show(Some(hwnd)) {
                    if is_picker_cancellation(&error) {
                        return Ok(None);
                    }
                    return Err(error);
                }
                let selected = dialog.GetResult()?;
                let display_name = selected.GetDisplayName(SIGDN_FILESYSPATH)?;
                if display_name.is_null() {
                    return Err(WindowsError::empty());
                }
                let path = OsString::from_wide(display_name.as_wide());
                CoTaskMemFree(Some(display_name.as_ptr().cast()));
                if path.is_empty() {
                    return Err(WindowsError::empty());
                }
                Ok(Some(PathBuf::from(path)))
            }
        })();
        let _ = sender.send(selection);
    })
    .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;

    receiver
        .await
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct WindowsFileIdentity {
    volume_serial_number: u64,
    file_id: [u8; 16],
}

/// Keeps the exact canonical destination directory and every canonical
/// ancestor open without write or delete sharing. Windows has no stable
/// `openat` equivalent in the Win32 API used here; retaining this handle chain
/// prevents an attacker from turning a component into a junction or replacing
/// it before the same-directory temporary file is promoted.
pub(crate) struct PinnedExportDirectory {
    path: PathBuf,
    identity: WindowsFileIdentity,
    parent: RefCell<File>,
    _guards: Vec<File>,
}

pub(crate) fn pin_export_directory(
    parent: &Path,
    data_root: &Path,
) -> PlatformResult<PinnedExportDirectory> {
    reject_windows_network_export_path(parent)?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    reject_windows_network_export_path(&canonical_parent)?;
    let canonical_data_root = std::fs::canonicalize(data_root)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if windows_path_is_within(&canonical_parent, &canonical_data_root) {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }

    let (mut guards, identity) =
        pin_canonical_directory_chain(&canonical_parent, PlatformErrorCode::SelectionFailed)?;
    let parent_handle = guards
        .pop()
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    let (data_root_guards, _) =
        pin_canonical_directory_chain(&canonical_data_root, PlatformErrorCode::StorageUnavailable)?;
    guards.extend(data_root_guards);

    let stable_parent = std::fs::canonicalize(parent)
        .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
    let stable_data_root = std::fs::canonicalize(data_root)
        .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
    if !windows_paths_equal(&stable_parent, &canonical_parent)
        || !windows_paths_equal(&stable_data_root, &canonical_data_root)
        || windows_path_is_within(&stable_parent, &stable_data_root)
    {
        return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
    }

    let pinned = PinnedExportDirectory {
        path: canonical_parent,
        identity,
        parent: RefCell::new(parent_handle),
        _guards: guards,
    };
    pinned.verify_identity()?;
    Ok(pinned)
}

impl PinnedExportDirectory {
    pub(crate) fn verify_identity(&self) -> PlatformResult<()> {
        let parent = self.parent.borrow();
        let handle_metadata = parent
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        let metadata = std::fs::symlink_metadata(&self.path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if !handle_metadata.is_dir()
            || !metadata.is_dir()
            || metadata.file_attributes()
                & ::windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT.0
                != 0
            || windows_file_identity(&parent, PlatformErrorCode::StorageUnavailable)?
                != self.identity
            || windows_path_identity(&self.path, PlatformErrorCode::StorageUnavailable)?
                != self.identity
        {
            return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
        }
        Ok(())
    }

    pub(crate) fn create_partial(&self, name: &OsStr) -> PlatformResult<File> {
        use ::windows::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE},
            Storage::FileSystem::DELETE,
        };
        use std::os::windows::fs::OpenOptionsExt;

        self.verify_identity()?;
        let path = self.path.join(name);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(GENERIC_READ.0 | GENERIC_WRITE.0 | DELETE.0)
            .share_mode(0)
            .custom_flags(
                (::windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT
                    | ::windows::Win32::Storage::FileSystem::FILE_FLAG_WRITE_THROUGH)
                    .0,
            )
            .open(&path)
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        let metadata = file
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if !metadata.is_file()
            || metadata.file_attributes()
                & ::windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT.0
                != 0
        {
            return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
        }
        self.verify_identity()?;
        Ok(file)
    }

    pub(crate) fn atomic_replace(
        &self,
        source: &File,
        source_name: &OsStr,
        destination_name: &OsStr,
    ) -> PlatformResult<()> {
        self.verify_identity()
            .map_err(|error| windows_export_test_error("verify parent before rename", error))?;
        self.verify_child(source, source_name)
            .map_err(|error| windows_export_test_error("verify partial before rename", error))?;
        self.replace_parent_share_mode(promotion_export_directory_share_mode())
            .map_err(|error| windows_export_test_error("enter promotion share mode", error))?;
        let rename_result =
            rename_open_file_in_pinned_directory(source, &self.path, destination_name);
        let restore_result = self.replace_parent_share_mode(pinned_export_directory_share_mode());
        if let Err(error) = restore_result {
            let _ = delete_export_file(source);
            return Err(windows_export_test_error(
                "restore pinned parent after rename",
                error,
            ));
        }
        rename_result.map_err(|error| windows_export_test_error("rename open file", error))?;
        source
            .sync_all()
            .map_err(|error| windows_export_test_error("flush renamed file", error))?;
        self.verify_identity()
            .map_err(|error| windows_export_test_error("verify parent after rename", error))?;

        // The promoted file remains intentionally exclusive. Reopening it for
        // path metadata can conflict with that handle, so validate its type on
        // the handle and bind the destination path separately by file ID.
        let destination_metadata = source
            .metadata()
            .map_err(|error| windows_export_test_error("read renamed handle metadata", error))?;
        let source_identity = windows_file_identity(source, PlatformErrorCode::StorageUnavailable)
            .map_err(|error| windows_export_test_error("read renamed handle identity", error))?;
        let destination_identity = windows_path_identity(
            &self.path.join(destination_name),
            PlatformErrorCode::StorageUnavailable,
        )
        .map_err(|error| windows_export_test_error("open destination identity", error))?;
        if !destination_metadata.is_file()
            || destination_metadata.file_attributes()
                & ::windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT.0
                != 0
            || source_identity != destination_identity
        {
            #[cfg(test)]
            eprintln!("Windows export promotion failed final handle/type identity validation");
            return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
        }
        Ok(())
    }

    fn verify_child(&self, source: &File, source_name: &OsStr) -> PlatformResult<()> {
        validate_export_file_name(source_name)?;
        let metadata = source
            .metadata()
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        if !metadata.is_file()
            || metadata.file_attributes()
                & ::windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT.0
                != 0
            || windows_file_identity(source, PlatformErrorCode::StorageUnavailable)?
                != windows_path_identity(
                    &self.path.join(source_name),
                    PlatformErrorCode::StorageUnavailable,
                )?
        {
            return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
        }
        Ok(())
    }

    fn replace_parent_share_mode(&self, share_mode: u32) -> PlatformResult<()> {
        let (replacement, identity) = open_verified_export_directory(
            &self.path,
            PlatformErrorCode::StorageUnavailable,
            share_mode,
        )?;
        if identity != self.identity {
            return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
        }
        drop(self.parent.replace(replacement));
        Ok(())
    }

    pub(crate) fn remove_partial(&self, name: &OsStr) {
        if self.verify_identity().is_ok() {
            let _ = std::fs::remove_file(self.path.join(name));
        }
    }
}

fn windows_export_test_error(stage: &'static str, error: impl std::fmt::Debug) -> PlatformError {
    #[cfg(test)]
    eprintln!("Windows export promotion failed at {stage}: {error:?}");
    #[cfg(not(test))]
    let _ = (stage, error);
    PlatformError::new(PlatformErrorCode::StorageUnavailable)
}
fn pin_canonical_directory_chain(
    path: &Path,
    error_code: PlatformErrorCode,
) -> PlatformResult<(Vec<File>, WindowsFileIdentity)> {
    let mut ancestors = path
        .ancestors()
        .filter(|ancestor| !ancestor.as_os_str().is_empty())
        .collect::<Vec<_>>();
    ancestors.reverse();
    let mut guards = Vec::with_capacity(ancestors.len());
    let mut leaf_identity = None;
    for ancestor in ancestors {
        let (file, handle_identity) = open_verified_export_directory(
            ancestor,
            error_code,
            pinned_export_directory_share_mode(),
        )?;
        leaf_identity = Some(handle_identity);
        guards.push(file);
    }
    let identity = leaf_identity.ok_or_else(|| PlatformError::new(error_code))?;
    Ok((guards, identity))
}

fn open_verified_export_directory(
    path: &Path,
    error_code: PlatformErrorCode,
    share_mode: u32,
) -> PlatformResult<(File, WindowsFileIdentity)> {
    use std::os::windows::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(share_mode)
        .custom_flags(
            (::windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS
                | ::windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT)
                .0,
        )
        .open(path)
        .map_err(|_| PlatformError::new(error_code))?;
    let handle_metadata = file
        .metadata()
        .map_err(|_| PlatformError::new(error_code))?;
    let path_metadata =
        std::fs::symlink_metadata(path).map_err(|_| PlatformError::new(error_code))?;
    let reparse = ::windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT.0;
    if !handle_metadata.is_dir()
        || !path_metadata.is_dir()
        || handle_metadata.file_attributes() & reparse != 0
        || path_metadata.file_attributes() & reparse != 0
    {
        return Err(PlatformError::new(error_code));
    }
    let identity = windows_file_identity(&file, error_code)?;
    if identity != windows_path_identity(path, error_code)? {
        return Err(PlatformError::new(error_code));
    }
    Ok((file, identity))
}

const fn pinned_export_directory_share_mode() -> u32 {
    // Denying WRITE prevents another process from opening the retained
    // directory with the authority needed to turn it into a junction between
    // identity verification and the path-based child create. Denying DELETE
    // continues to prevent rename/replacement of every retained component.
    ::windows::Win32::Storage::FileSystem::FILE_SHARE_READ.0
}

const fn promotion_export_directory_share_mode() -> u32 {
    use ::windows::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0
}

fn reject_windows_network_export_path(path: &Path) -> PlatformResult<()> {
    if windows_export_path_is_network_with(path, |path| {
        windows_export_drive_type(path).unwrap_or(4)
    }) {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
}

fn windows_export_path_is_network_with(path: &Path, drive_type: impl FnOnce(&Path) -> u32) -> bool {
    use std::path::{Component, Prefix};

    if !path.is_absolute() {
        return true;
    }
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return true;
    };
    match prefix.kind() {
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) => drive_type(path) == 4,
        Prefix::UNC(_, _)
        | Prefix::VerbatimUNC(_, _)
        | Prefix::DeviceNS(_)
        | Prefix::Verbatim(_) => true,
    }
}

#[allow(unsafe_code)]
fn windows_export_drive_type(path: &Path) -> PlatformResult<u32> {
    use ::windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};

    let path = null_terminated_wide(path);
    let volume_root_length = path
        .len()
        .checked_add(1)
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let _ = u32::try_from(volume_root_length)
        .map_err(|_| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let mut volume_root = vec![0_u16; volume_root_length];
    // SAFETY: both UTF-16 buffers are NUL-terminated/writable for their full
    // declared lengths and remain alive through these synchronous calls.
    unsafe {
        GetVolumePathNameW(PCWSTR::from_raw(path.as_ptr()), &mut volume_root)
            .map_err(|_| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    }
    let Some(terminator) = volume_root.iter().position(|code_unit| *code_unit == 0) else {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    };
    if terminator == 0 {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let drive_type = unsafe { GetDriveTypeW(PCWSTR::from_raw(volume_root.as_ptr())) };
    // DRIVE_UNKNOWN and DRIVE_NO_ROOT_DIR cannot establish the local-volume
    // contract required by handle-relative promotion, so fail closed.
    if drive_type <= 1 {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(drive_type)
}

#[allow(unsafe_code)]
fn windows_path_identity(
    path: &Path,
    error_code: PlatformErrorCode,
) -> PlatformResult<WindowsFileIdentity> {
    use ::windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let path = null_terminated_wide(path);
    // SAFETY: the path buffer is NUL-terminated and remains live for the call.
    // Zero desired access is sufficient for handle metadata, while full share
    // flags let this verifier reopen the already pinned no-write/no-delete
    // entry without granting another process mutation authority.
    let handle = unsafe {
        CreateFileW(
            PCWSTR::from_raw(path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            None,
        )
    }
    .map_err(|error| {
        #[cfg(test)]
        eprintln!("Windows path identity open failed: {error:?}");
        #[cfg(not(test))]
        let _ = error;
        PlatformError::new(error_code)
    })?;
    // SAFETY: CreateFileW returned one owned handle and File assumes exactly
    // that ownership, closing it once at the end of this function.
    let file = unsafe { File::from_raw_handle(handle.0) };
    windows_file_identity(&file, error_code)
}

#[allow(unsafe_code)]
fn windows_file_identity(
    file: &File,
    error_code: PlatformErrorCode,
) -> PlatformResult<WindowsFileIdentity> {
    let mut information = std::mem::MaybeUninit::<FILE_ID_INFO>::zeroed();
    let information_bytes = u32::try_from(std::mem::size_of::<FILE_ID_INFO>())
        .map_err(|_| PlatformError::new(error_code))?;
    // SAFETY: information points to initialized writable storage of the exact
    // Win32 structure and the borrowed File keeps its handle valid for the
    // synchronous metadata query.
    unsafe {
        GetFileInformationByHandleEx(
            ::windows::Win32::Foundation::HANDLE(file.as_raw_handle()),
            FileIdInfo,
            information.as_mut_ptr().cast(),
            information_bytes,
        )
    }
    .map_err(|error| {
        #[cfg(test)]
        eprintln!("Windows handle identity query failed: {error:?}");
        #[cfg(not(test))]
        let _ = error;
        PlatformError::new(error_code)
    })?;
    // SAFETY: a successful FileIdInfo query initializes the complete structure.
    let information = unsafe { information.assume_init() };
    Ok(windows_file_identity_from_information(information))
}

const fn windows_file_identity_from_information(information: FILE_ID_INFO) -> WindowsFileIdentity {
    WindowsFileIdentity {
        volume_serial_number: information.VolumeSerialNumber,
        file_id: information.FileId.Identifier,
    }
}

fn windows_paths_equal(left: &Path, right: &Path) -> bool {
    windows_path_key(left) == windows_path_key(right)
}

fn windows_path_is_within(path: &Path, root: &Path) -> bool {
    let path = windows_path_key(path);
    let root = windows_path_key(root);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

fn windows_path_key(path: &Path) -> String {
    path.as_os_str()
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

#[repr(C)]
struct RawFileRenameInfo {
    replace_if_exists_or_flags: u32,
    root_directory: ::windows::Win32::Foundation::HANDLE,
    file_name_length: u32,
    file_name: [u16; 1],
}

const fn file_rename_info_buffer_bytes(file_name_bytes: usize) -> Option<usize> {
    std::mem::offset_of!(RawFileRenameInfo, file_name).checked_add(file_name_bytes)
}

#[cfg(test)]
const _: () = {
    let file_name_bytes = std::mem::size_of::<u16>();
    let Some(actual) = file_rename_info_buffer_bytes(file_name_bytes) else {
        panic!("one UTF-16 code unit must fit in a FILE_RENAME_INFO buffer");
    };
    let Some(required) =
        std::mem::offset_of!(RawFileRenameInfo, file_name).checked_add(file_name_bytes)
    else {
        panic!("one UTF-16 code unit must fit in a FILE_RENAME_INFO buffer");
    };
    assert!(actual == required);
};

const _: () = {
    assert!(
        std::mem::offset_of!(RawFileRenameInfo, root_directory) == std::mem::size_of::<usize>()
    );
    assert!(
        std::mem::offset_of!(RawFileRenameInfo, file_name_length)
            == std::mem::size_of::<usize>() * 2
    );
    assert!(
        std::mem::offset_of!(RawFileRenameInfo, file_name)
            == std::mem::size_of::<usize>() * 2 + std::mem::size_of::<u32>()
    );
};

#[allow(unsafe_code)]
fn rename_open_file_in_pinned_directory(
    source: &File,
    parent: &Path,
    destination_name: &OsStr,
) -> PlatformResult<()> {
    const REPLACE_IF_EXISTS: u32 = 0x0000_0001;
    const MAXIMUM_SHARING_RETRIES: usize = 200;

    validate_export_file_name(destination_name)?;
    let destination_path = parent.join(destination_name);
    if !destination_path.is_absolute() {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let destination = destination_path
        .as_os_str()
        .encode_wide()
        .collect::<Vec<_>>();
    if destination.is_empty() || destination.contains(&0) {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    let file_name_bytes = destination
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let buffer_bytes = file_rename_info_buffer_bytes(file_name_bytes)
        .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let buffer_length = u32::try_from(buffer_bytes)
        .map_err(|_| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let file_name_length = u32::try_from(file_name_bytes)
        .map_err(|_| PlatformError::new(PlatformErrorCode::InvalidInput))?;
    let word_bytes = std::mem::size_of::<usize>();
    let word_count = buffer_bytes.div_ceil(word_bytes);
    let mut buffer = vec![0_usize; word_count];
    let info = buffer.as_mut_ptr().cast::<RawFileRenameInfo>();

    // SAFETY: `buffer` is pointer-aligned and large enough for the fixed
    // FILE_RENAME_INFO header plus the exact canonical destination. The source
    // was opened with DELETE authority, while its exclusive handle and pinned
    // ancestor chain keep the parent stable during the brief promotion mode.
    unsafe {
        info.write(RawFileRenameInfo {
            replace_if_exists_or_flags: REPLACE_IF_EXISTS,
            root_directory: ::windows::Win32::Foundation::HANDLE::default(),
            file_name_length,
            file_name: [0],
        });
        std::ptr::copy_nonoverlapping(
            destination.as_ptr(),
            (&raw mut (*info).file_name).cast::<u16>(),
            destination.len(),
        );
    }

    // Antivirus and indexers can briefly open an existing destination without
    // delete sharing. Retry that one transient result only, with a hard bound.
    for attempt in 0..=MAXIMUM_SHARING_RETRIES {
        // SAFETY: the source handle and initialized buffer stay live and
        // unchanged for every synchronous retry.
        let result = unsafe {
            SetFileInformationByHandle(
                ::windows::Win32::Foundation::HANDLE(source.as_raw_handle()),
                FileRenameInfo,
                info.cast(),
                buffer_length,
            )
        };
        match result {
            Ok(()) => return Ok(()),
            Err(error)
                if attempt < MAXIMUM_SHARING_RETRIES
                    && error.code() == HRESULT::from_win32(ERROR_SHARING_VIOLATION.0) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => {
                return Err(windows_export_test_error(
                    "SetFileInformationByHandle rename",
                    error,
                ));
            }
        }
    }
    unreachable!("bounded rename loop always returns")
}

fn validate_export_file_name(name: &OsStr) -> PlatformResult<()> {
    let mut components = Path::new(name).components();
    if !matches!(
        components.next(),
        Some(std::path::Component::Normal(component)) if component == name
    ) || components.next().is_some()
    {
        return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
    }
    Ok(())
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
        RawFileRenameInfo, WINDOWS_DPAPI_MAXIMUM_CIPHERTEXT_BYTES,
        WINDOWS_DPAPI_MAXIMUM_ENTROPY_BYTES, WINDOWS_DPAPI_MAXIMUM_PLAINTEXT_BYTES,
        delete_verified_file, file_rename_info_buffer_bytes, pin_export_directory,
        pinned_export_directory_share_mode, protect_current_user_data, unprotect_current_user_data,
        windows_export_path_is_network_with, windows_file_identity_from_information,
    };
    use crate::PlatformErrorCode;
    use std::{
        ffi::OsStr,
        fs::OpenOptions,
        io::{Read, Write},
        os::windows::fs::OpenOptionsExt,
    };
    use zeroize::Zeroizing;

    #[test]
    fn windows_file_rename_buffer_includes_complete_header_and_name() {
        let file_name_bytes = "export.charx".encode_utf16().count() * std::mem::size_of::<u16>();
        assert_eq!(
            file_rename_info_buffer_bytes(file_name_bytes),
            std::mem::offset_of!(RawFileRenameInfo, file_name).checked_add(file_name_bytes),
        );
        assert_eq!(file_rename_info_buffer_bytes(usize::MAX), None);
    }

    #[test]
    fn windows_export_picker_is_path_only_before_destination_validation() {
        let source = include_str!("windows.rs");
        let create_before_validation_api = ["PickSave", "FileAsync"].concat();
        let path_only_api = ["IFileSave", "Dialog"].concat();
        let no_test_create_option = ["FOS_NOTEST", "FILECREATE"].concat();

        assert!(!source.contains(&create_before_validation_api));
        assert!(source.contains(&path_only_api));
        assert!(source.contains(&no_test_create_option));
    }

    #[test]
    fn windows_file_identity_preserves_refs_volume_and_full_file_id() {
        let file_id = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let identity = windows_file_identity_from_information(
            ::windows::Win32::Storage::FileSystem::FILE_ID_INFO {
                VolumeSerialNumber: 0xfedc_ba98_7654_3210,
                FileId: ::windows::Win32::Storage::FileSystem::FILE_ID_128 {
                    Identifier: file_id,
                },
            },
        );

        assert_eq!(identity.volume_serial_number, 0xfedc_ba98_7654_3210);
        assert_eq!(identity.file_id, file_id);
    }

    #[test]
    fn windows_export_network_policy_rejects_unc_and_mapped_drives() {
        let remote = |_path: &std::path::Path| 4_u32;
        let local = |_path: &std::path::Path| 3_u32;

        assert!(windows_export_path_is_network_with(
            std::path::Path::new(r"\\server\share\exports"),
            local,
        ));
        assert!(windows_export_path_is_network_with(
            std::path::Path::new(r"\\?\UNC\server\share\exports"),
            local,
        ));
        assert!(windows_export_path_is_network_with(
            std::path::Path::new(r"Z:\exports"),
            remote,
        ));
        assert!(windows_export_path_is_network_with(
            std::path::Path::new(r"\\?\Z:\exports"),
            remote,
        ));
        assert!(!windows_export_path_is_network_with(
            std::path::Path::new(r"C:\exports"),
            local,
        ));
    }

    #[test]
    fn pinned_export_directories_deny_write_and_delete_sharing() {
        let share_mode = pinned_export_directory_share_mode();
        assert_eq!(
            share_mode,
            ::windows::Win32::Storage::FileSystem::FILE_SHARE_READ.0
        );
        assert_eq!(
            share_mode & ::windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE.0,
            0
        );
        assert_eq!(
            share_mode & ::windows::Win32::Storage::FileSystem::FILE_SHARE_DELETE.0,
            0
        );
    }

    #[test]
    fn pinned_export_directory_still_creates_and_promotes_child_file() {
        let root = tempfile::tempdir().expect("temporary directory");
        let data_root = root.path().join("data");
        let export_root = root.path().join("exports");
        std::fs::create_dir(&data_root).expect("data root");
        std::fs::create_dir(&export_root).expect("export root");
        let pinned = pin_export_directory(&export_root, &data_root).expect("pinned export root");
        let partial_name = OsStr::new(".lorepia-export-test.partial");
        let destination_name = OsStr::new("export.charx");

        let mut partial = pinned.create_partial(partial_name).expect("partial file");
        partial
            .write_all(b"verified export")
            .expect("write partial");
        partial.sync_all().expect("sync partial");
        pinned
            .atomic_replace(&partial, partial_name, destination_name)
            .expect("promote partial");
        drop(partial);

        assert_eq!(
            std::fs::read(export_root.join(destination_name)).expect("promoted export"),
            b"verified export"
        );
        assert!(!export_root.join(partial_name).exists());
    }

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
