use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{
    Runtime,
    plugin::{PluginHandle, mobile::PluginInvokeError},
};

use crate::{
    CredentialStatus, NativeCaptureStatus, NativeCredential, NativeSensitiveText, PlatformError,
    PlatformErrorCode, PlatformResult, StagedImport,
    model::{
        MobileCaptureStatusResponse, MobileCredentialResponse, MobileCredentialStatusResponse,
        MobilePathResponse, MobilePickResponse, MobileSaveContentSourceResponse,
        MobileSensitiveCaptureResponse, NativeSavedContentSource,
    },
    validation::{
        MAXIMUM_SENSITIVE_CAPTURE_BYTES, validate_credential_read, validate_credential_write,
        validate_export_receipt_display_name, validate_export_sha256, validate_reference,
        validate_sensitive_capture, verify_content_source_for_export,
    },
};
#[cfg(target_os = "android")]
use zeroize::Zeroize;

pub(crate) struct MobilePlatform<R: Runtime> {
    handle: PluginHandle<R>,
    data_root: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceArgs<'a> {
    reference: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CredentialArgs<'a> {
    reference: &'a str,
    value: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StagedPathArgs<'a> {
    path: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SensitiveCaptureArgs {
    maximum_bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveContentSourceArgs<'a> {
    source_path: &'a str,
    suggested_name: &'a str,
    expected_size_bytes: u64,
    expected_sha256: &'a str,
}

impl<R: Runtime> MobilePlatform<R> {
    pub(crate) fn new(handle: PluginHandle<R>) -> PlatformResult<Self> {
        let response = handle
            .run_mobile_plugin::<MobilePathResponse>("dataRoot", ())
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))?;
        let data_root = PathBuf::from(response.path);
        if !data_root.is_absolute() {
            return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
        }
        Ok(Self { handle, data_root })
    }

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub(crate) async fn pick_import(&self) -> PlatformResult<Option<StagedImport>> {
        let response = self
            .handle
            .run_mobile_plugin_async::<MobilePickResponse>("pickImport", ())
            .await
            .map_err(|_| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        if !response.selected {
            return Ok(None);
        }

        let path = response
            .path
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        let display_name = response
            .display_name
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        let size_bytes = response
            .size_bytes
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        Ok(Some(StagedImport::new(path, display_name, size_bytes)))
    }

    pub(crate) async fn discard_staged_import(&self, staged: &StagedImport) -> PlatformResult<()> {
        let path = staged
            .path()
            .to_str()
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
        self.handle
            .run_mobile_plugin_async::<()>("discardStagedImport", StagedPathArgs { path })
            .await
            .map_err(|_| PlatformError::new(PlatformErrorCode::StorageUnavailable))
    }

    pub(crate) async fn save_content_source(
        &self,
        source_path: &Path,
        suggested_name: &str,
        expected_size_bytes: u64,
        expected_sha256: &str,
    ) -> PlatformResult<Option<NativeSavedContentSource>> {
        verify_content_source_for_export(
            source_path,
            &self.data_root,
            expected_size_bytes,
            expected_sha256,
            suggested_name,
        )?;
        let source_path = source_path
            .to_str()
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::InvalidInput))?;
        let response = self
            .handle
            .run_mobile_plugin_async::<MobileSaveContentSourceResponse>(
                "saveContentSource",
                SaveContentSourceArgs {
                    source_path,
                    suggested_name,
                    expected_size_bytes,
                    expected_sha256,
                },
            )
            .await
            .map_err(map_export_invoke_error)?;
        if !response.selected {
            if response.display_name.is_some()
                || response.size_bytes.is_some()
                || response.sha256.is_some()
            {
                return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
            }
            return Ok(None);
        }

        let display_name = response
            .display_name
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        let size_bytes = response
            .size_bytes
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        let sha256 = response
            .sha256
            .ok_or_else(|| PlatformError::new(PlatformErrorCode::SelectionFailed))?;
        validate_export_receipt_display_name(&display_name)?;
        validate_export_sha256(&sha256)?;
        if size_bytes != expected_size_bytes || sha256 != expected_sha256 {
            return Err(PlatformError::new(PlatformErrorCode::SelectionFailed));
        }
        Ok(Some(NativeSavedContentSource::new(
            display_name,
            size_bytes,
            sha256,
        )))
    }

    pub(crate) async fn credential_status(
        &self,
        reference: &str,
    ) -> PlatformResult<CredentialStatus> {
        validate_reference(reference)?;
        self.handle
            .run_mobile_plugin_async::<MobileCredentialStatusResponse>(
                "credentialStatus",
                ReferenceArgs { reference },
            )
            .await
            .map(|response| response.status)
            .map_err(map_credential_invoke_error)
    }

    /// Observes an authority-bound slot without allowing an iOS Keychain read
    /// to harden accessibility metadata before the durable Started cutpoint.
    pub(crate) async fn bound_credential_status(
        &self,
        reference: &str,
    ) -> PlatformResult<CredentialStatus> {
        validate_reference(reference)?;
        self.handle
            .run_mobile_plugin_async::<MobileCredentialStatusResponse>(
                "boundCredentialStatus",
                ReferenceArgs { reference },
            )
            .await
            .map(|response| response.status)
            .map_err(map_credential_invoke_error)
    }

    pub(crate) async fn read_credential(
        &self,
        reference: &str,
    ) -> PlatformResult<Option<NativeCredential>> {
        validate_reference(reference)?;
        let response = self
            .handle
            .run_mobile_plugin_async::<MobileCredentialResponse>(
                "readCredential",
                ReferenceArgs { reference },
            )
            .await
            .map_err(map_credential_invoke_error)?;
        match response.value {
            Some(value) => {
                validate_credential_read(&value)?;
                Ok(Some(NativeCredential::new(value)))
            }
            None => Ok(None),
        }
    }

    /// Reads an authority-bound slot through the strictly nonmutating iOS
    /// Keychain command. Android's credential read is already nonmutating.
    pub(crate) async fn read_bound_credential(
        &self,
        reference: &str,
    ) -> PlatformResult<Option<NativeCredential>> {
        validate_reference(reference)?;
        let response = self
            .handle
            .run_mobile_plugin_async::<MobileCredentialResponse>(
                "readBoundCredential",
                ReferenceArgs { reference },
            )
            .await
            .map_err(map_credential_invoke_error)?;
        match response.value {
            Some(value) => {
                validate_credential_read(&value)?;
                Ok(Some(NativeCredential::new(value)))
            }
            None => Ok(None),
        }
    }

    pub(crate) async fn store_credential(
        &self,
        reference: &str,
        value: NativeCredential,
    ) -> PlatformResult<()> {
        self.validate_credential_store(reference, &value)?;
        self.store_prevalidated_credential(reference, value).await
    }

    /// Complete all deterministic rejection checks before a durable
    /// credential operation is marked started.
    pub(crate) fn validate_credential_store(
        &self,
        reference: &str,
        value: &NativeCredential,
    ) -> PlatformResult<()> {
        let _ = self;
        validate_reference(reference)?;
        validate_credential_write(value.expose())
    }

    /// Enter the mobile native store with a credential proven valid by
    /// `validate_credential_store` while the durable operation was prepared.
    pub(crate) async fn store_prevalidated_credential(
        &self,
        reference: &str,
        value: NativeCredential,
    ) -> PlatformResult<()> {
        self.handle
            .run_mobile_plugin_async::<()>(
                "storeCredential",
                CredentialArgs {
                    reference,
                    value: value.expose(),
                },
            )
            .await
            .map_err(map_credential_invoke_error)
    }

    /// Enters the platform's add-only authority-bound store after Rust has
    /// completed every deterministic validation and persisted Started.
    pub(crate) async fn store_prevalidated_bound_credential(
        &self,
        reference: &str,
        value: NativeCredential,
    ) -> PlatformResult<()> {
        self.handle
            .run_mobile_plugin_async::<()>(
                "storeBoundCredential",
                CredentialArgs {
                    reference,
                    value: value.expose(),
                },
            )
            .await
            .map_err(map_credential_invoke_error)
    }

    pub(crate) async fn delete_credential(&self, reference: &str) -> PlatformResult<()> {
        validate_reference(reference)?;
        self.handle
            .run_mobile_plugin_async::<()>("deleteCredential", ReferenceArgs { reference })
            .await
            .map_err(map_credential_invoke_error)
    }

    pub(crate) async fn delete_bound_credential(&self, reference: &str) -> PlatformResult<()> {
        validate_reference(reference)?;
        #[cfg(target_os = "android")]
        let command = "deleteBoundCredential";
        #[cfg(target_os = "ios")]
        let command = "deleteCredential";
        self.handle
            .run_mobile_plugin_async::<()>(command, ReferenceArgs { reference })
            .await
            .map_err(map_credential_invoke_error)
    }

    pub(crate) async fn capture_credential_from_clipboard(
        &self,
        reference: &str,
    ) -> PlatformResult<NativeCaptureStatus> {
        validate_reference(reference)?;
        let response = self
            .handle
            .run_mobile_plugin_async::<MobileCaptureStatusResponse>(
                "captureCredential",
                ReferenceArgs { reference },
            )
            .await
            .map_err(map_credential_invoke_error)?;
        Ok(NativeCaptureStatus {
            clipboard_cleanup: response.clipboard_cleanup,
        })
    }

    pub(crate) async fn capture_sensitive_text_from_clipboard(
        &self,
        maximum_bytes: usize,
    ) -> PlatformResult<NativeSensitiveText> {
        validate_sensitive_capture("x", maximum_bytes)?;
        let maximum_bytes = u64::try_from(maximum_bytes)
            .map_err(|_| PlatformError::new(PlatformErrorCode::InvalidInput))?;
        let response = self
            .handle
            .run_mobile_plugin_async::<MobileSensitiveCaptureResponse>(
                "captureSensitiveText",
                SensitiveCaptureArgs { maximum_bytes },
            )
            .await
            .map_err(map_sensitive_capture_invoke_error)?;
        #[cfg(target_os = "ios")]
        let value = response.value;
        #[cfg(target_os = "android")]
        let value = {
            let path = PathBuf::from(response.path);
            let mut bytes = crate::staging::consume_sensitive_capture(
                &path,
                &self.data_root,
                response.size_bytes,
                maximum_bytes,
            )?;
            match String::from_utf8(std::mem::take(&mut bytes)) {
                Ok(value) => value,
                Err(error) => {
                    let mut invalid = error.into_bytes();
                    invalid.zeroize();
                    return Err(PlatformError::new(PlatformErrorCode::InvalidInput));
                }
            }
        };
        let captured = NativeSensitiveText::new(
            value,
            NativeCaptureStatus {
                clipboard_cleanup: response.clipboard_cleanup,
            },
        );
        validate_sensitive_capture(
            captured.expose(),
            usize::try_from(maximum_bytes).unwrap_or(MAXIMUM_SENSITIVE_CAPTURE_BYTES),
        )?;
        Ok(captured)
    }
}

fn map_credential_invoke_error(error: PluginInvokeError) -> PlatformError {
    let recovery_required = matches!(
        error,
        PluginInvokeError::InvokeRejected(ref response)
            if matches!(
                response.code.as_deref(),
                Some("credential_recovery_required" | "credential_restore_failed")
            )
    );
    PlatformError::new(if recovery_required {
        PlatformErrorCode::CredentialRecoveryRequired
    } else {
        PlatformErrorCode::CredentialUnavailable
    })
}

fn map_sensitive_capture_invoke_error(error: PluginInvokeError) -> PlatformError {
    let code = match error {
        PluginInvokeError::InvokeRejected(response) => match response.code.as_deref() {
            Some("permission_denied") => PlatformErrorCode::PermissionDenied,
            Some("invalid_input" | "empty_clipboard" | "capture_too_large") => {
                PlatformErrorCode::InvalidInput
            }
            Some("storage_unavailable") => PlatformErrorCode::StorageUnavailable,
            _ => PlatformErrorCode::Internal,
        },
        _ => PlatformErrorCode::Internal,
    };
    PlatformError::new(code)
}

fn map_export_invoke_error(error: PluginInvokeError) -> PlatformError {
    let code = match error {
        PluginInvokeError::InvokeRejected(response) => match response.code.as_deref() {
            Some("busy") => PlatformErrorCode::Busy,
            Some("invalid_input") => PlatformErrorCode::InvalidInput,
            Some("storage_unavailable") => PlatformErrorCode::StorageUnavailable,
            Some("selection_failed") => PlatformErrorCode::SelectionFailed,
            _ => PlatformErrorCode::Internal,
        },
        _ => PlatformErrorCode::Internal,
    };
    PlatformError::new(code)
}
