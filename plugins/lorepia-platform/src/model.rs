use std::{
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardCleanupStatus {
    Cleared,
    AlreadyReplaced,
    ClearFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeCaptureStatus {
    pub clipboard_cleanup: ClipboardCleanupStatus,
}

/// A safe receipt for one completed native content-source export.
///
/// The selected host path and source bytes are deliberately absent. The
/// high-level Tauri command may project these fields after it has compared
/// them with the Rust-owned source descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSavedContentSource {
    display_name: String,
    size_bytes: u64,
    sha256: String,
}

impl NativeSavedContentSource {
    #[cfg(any(mobile, target_os = "macos", windows, test))]
    pub(crate) fn new(display_name: String, size_bytes: u64, sha256: String) -> Self {
        Self {
            display_name,
            size_bytes,
            sha256,
        }
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn sha256(&self) -> &str {
        &self.sha256
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialStatus {
    Missing,
    Available,
    Unreadable,
}

/// Non-secret authority expected inside one atomic native credential item.
///
/// This type is Rust-only and deliberately has no serialization surface.
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialAuthority {
    authority_id: String,
    binding_sha256: String,
}

impl CredentialAuthority {
    pub fn new(authority_id: String, binding_sha256: String) -> crate::PlatformResult<Self> {
        let valid_authority = !authority_id.trim().is_empty()
            && authority_id.len() <= 256
            && !authority_id.chars().any(char::is_control);
        let valid_binding = binding_sha256.len() == 64
            && binding_sha256
                .as_bytes()
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
        if !valid_authority || !valid_binding {
            return Err(crate::PlatformError::new(
                crate::PlatformErrorCode::InvalidInput,
            ));
        }
        Ok(Self {
            authority_id,
            binding_sha256,
        })
    }

    pub fn authority_id(&self) -> &str {
        &self.authority_id
    }

    pub fn binding_sha256(&self) -> &str {
        &self.binding_sha256
    }
}

impl fmt::Debug for CredentialAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialAuthority")
            .field("authority_id", &self.authority_id)
            .field("binding_sha256", &self.binding_sha256)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoundCredentialObservation {
    Missing,
    Legacy,
    Match,
    Mismatch,
    Unreadable,
}

/// Rust-only classification used by the isolated legacy-profile path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyCredentialObservation {
    Missing,
    Raw,
    Bound,
    Unreadable,
}

/// A fully validated, authority-bound native credential write.
///
/// Construction is restricted to [`crate::LorepiaPlatform`] so every
/// deterministic operation which can reject the write is completed before a
/// durable operation moves from prepared to started. The encoded credential
/// remains Rust-owned and zeroizes its allocation on every drop/error path.
///
/// This type deliberately implements neither `Serialize`, `Clone`, nor
/// `Debug`, and its fields have no public accessors.
pub struct PreparedBoundCredentialStore {
    physical_reference: String,
    encoded_credential: NativeCredential,
}

impl PreparedBoundCredentialStore {
    pub(crate) fn new(physical_reference: String, encoded_credential: NativeCredential) -> Self {
        Self {
            physical_reference,
            encoded_credential,
        }
    }

    pub(crate) fn physical_reference(&self) -> &str {
        &self.physical_reference
    }

    pub(crate) fn encoded_credential(&self) -> &NativeCredential {
        &self.encoded_credential
    }

    pub(crate) fn into_parts(self) -> (String, NativeCredential) {
        (self.physical_reference, self.encoded_credential)
    }
}

/// A native credential that can only move farther into Rust.
///
/// It deliberately implements neither `Serialize` nor `Clone`.
pub struct NativeCredential(Zeroizing<String>);

impl NativeCredential {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub(crate) fn from_zeroizing(value: Zeroizing<String>) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    /// Transfers the credential allocation to the next Rust-only owner.
    ///
    /// The source allocation is left empty before `Drop`, avoiding a second
    /// plaintext allocation at the platform/application boundary.
    pub fn into_secret_string(mut self) -> String {
        std::mem::take(&mut *self.0)
    }
}

impl fmt::Debug for NativeCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("NativeCredential([REDACTED])")
    }
}

/// Sensitive text captured by a native surface and consumed only by Rust.
///
/// It deliberately implements neither `Serialize` nor `Clone`. The capture
/// status is safe to project to the webview after the text has been consumed.
pub struct NativeSensitiveText {
    value: Zeroizing<String>,
    status: NativeCaptureStatus,
}

impl NativeSensitiveText {
    #[cfg(any(mobile, target_os = "macos", windows, test))]
    pub(crate) fn new(value: String, status: NativeCaptureStatus) -> Self {
        Self {
            value: Zeroizing::new(value),
            status,
        }
    }

    #[cfg(mobile)]
    pub(crate) fn expose(&self) -> &str {
        self.value.as_str()
    }

    pub const fn status(&self) -> NativeCaptureStatus {
        self.status
    }

    /// Transfers the captured allocation to the next Rust-only owner.
    ///
    /// `NativeSensitiveText` intentionally has no `Debug`, `Clone`, or
    /// serialization implementation.
    pub fn into_secret_string(mut self) -> String {
        std::mem::take(&mut *self.value)
    }
}

/// An app-owned bounded copy. Its host path is never serializable.
pub struct StagedImport {
    path: PathBuf,
    display_name: String,
    size_bytes: u64,
}

impl StagedImport {
    #[cfg(any(
        target_os = "android",
        target_os = "ios",
        target_os = "macos",
        windows,
        test
    ))]
    pub(crate) fn new(path: PathBuf, display_name: String, size_bytes: u64) -> Self {
        Self {
            path,
            display_name,
            size_bytes,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
}

impl fmt::Debug for StagedImport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StagedImport")
            .field("path", &"[REDACTED]")
            .field("display_name", &"[REDACTED]")
            .field("size_bytes", &self.size_bytes)
            .finish()
    }
}

impl Drop for StagedImport {
    fn drop(&mut self) {
        // `StagedImport::new` is crate-private and is only constructed from an
        // app-owned bounded copy. This is the terminal fallback when a window
        // closes before the explicit discard command runs.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(mobile)]
pub(crate) struct MobilePathResponse {
    pub path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(mobile)]
pub(crate) struct MobileCredentialResponse {
    pub value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(mobile)]
pub(crate) struct MobileCredentialStatusResponse {
    pub status: CredentialStatus,
}

#[derive(Debug, Deserialize)]
#[cfg(mobile)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MobileCaptureStatusResponse {
    pub clipboard_cleanup: ClipboardCleanupStatus,
}

#[derive(Deserialize)]
#[cfg(mobile)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MobileSensitiveCaptureResponse {
    #[cfg(target_os = "ios")]
    pub value: String,
    #[cfg(target_os = "android")]
    pub path: String,
    #[cfg(target_os = "android")]
    pub size_bytes: u64,
    pub clipboard_cleanup: ClipboardCleanupStatus,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(mobile)]
pub(crate) struct MobilePickResponse {
    pub selected: bool,
    pub path: Option<String>,
    pub display_name: Option<String>,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg(mobile)]
pub(crate) struct MobileSaveContentSourceResponse {
    pub selected: bool,
    pub display_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
}
