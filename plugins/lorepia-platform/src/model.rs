use std::{
    fmt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
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

/// Exact backend-owned credential effect shown by the native confirmation UI.
///
/// This value never crosses the app's renderer IPC boundary. The platform
/// plugin owns the presentation and returns a non-cloneable confirmation which
/// the Rust backend consumes immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCredentialEffect {
    CaptureOrReplace,
    Delete,
    Archive,
    DiscoveryCompensation,
}

impl NativeCredentialEffect {
    #[cfg(mobile)]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::CaptureOrReplace => "capture_or_replace",
            Self::Delete => "delete",
            Self::Archive => "archive",
            Self::DiscoveryCompensation => "discovery_compensation",
        }
    }
}

/// Non-secret, exact context for one native credential-effect prompt.
///
/// Construction rejects controls and unbounded text so a compromised database
/// cannot shape or visually truncate the trusted native prompt. This type has
/// no serialization implementation; mobile serialization is confined to the
/// private Rust-to-native plugin bridge.
#[derive(Debug, PartialEq, Eq)]
pub struct NativeCredentialEffectContext {
    effect: NativeCredentialEffect,
    target_id: String,
    origin: String,
    revision: String,
}

impl NativeCredentialEffectContext {
    pub fn new(
        effect: NativeCredentialEffect,
        target_id: String,
        origin: String,
        revision: String,
    ) -> crate::PlatformResult<Self> {
        validate_confirmation_text(&target_id, 256)?;
        validate_confirmation_text(&origin, 2_048)?;
        validate_confirmation_text(&revision, 256)?;
        Ok(Self {
            effect,
            target_id,
            origin,
            revision,
        })
    }

    pub const fn effect(&self) -> NativeCredentialEffect {
        self.effect
    }

    pub fn target_id(&self) -> &str {
        &self.target_id
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }
}

fn validate_confirmation_text(value: &str, maximum_bytes: usize) -> crate::PlatformResult<()> {
    if value.is_empty()
        || value.chars().all(|character| character == ' ')
        || value.len() > maximum_bytes
        || value
            .chars()
            .any(|character| is_confirmation_spoofing_code_point(character as u32))
    {
        return Err(crate::PlatformError::new(
            crate::PlatformErrorCode::InvalidInput,
        ));
    }
    Ok(())
}

/// Platform-neutral scalar policy for trusted native prompt fields.
///
/// Only the ordinary ASCII space remains available for layout inside a field.
/// The list deliberately covers C0/C1 controls, every Unicode whitespace
/// scalar, bidi shaping, zero-width/default-ignorable format characters,
/// variation selectors, and tag characters. Android and iOS mirror these
/// exact numeric ranges before constructing their native dialogs.
const fn is_confirmation_spoofing_code_point(code_point: u32) -> bool {
    matches!(
        code_point,
        0x0000..=0x001f
            | 0x007f..=0x00a0
            | 0x00ad
            | 0x034f
            | 0x0600..=0x0605
            | 0x061c
            | 0x06dd
            | 0x070f
            | 0x0890..=0x0891
            | 0x08e2
            | 0x115f..=0x1160
            | 0x1680
            | 0x17b4..=0x17b5
            | 0x180b..=0x180f
            | 0x2000..=0x200f
            | 0x2028..=0x202f
            | 0x205f..=0x206f
            | 0x3000
            | 0x3164
            | 0xfe00..=0xfe0f
            | 0xfeff
            | 0xffa0
            | 0xfff0..=0xfffb
            | 0x110bd
            | 0x110cd
            | 0x13430..=0x13455
            | 0x1bca0..=0x1bca3
            | 0x1d173..=0x1d17a
            | 0xe0000..=0xe0fff
    )
}

/// One native modal approval. It cannot be cloned, serialized, or constructed
/// outside this crate and is consumed by the app backend before the effect.
pub struct NativeCredentialEffectConfirmation {
    context: NativeCredentialEffectContext,
    expires_at: Instant,
}

impl NativeCredentialEffectConfirmation {
    const VALIDITY: Duration = Duration::from_secs(30);

    pub(crate) fn new(context: NativeCredentialEffectContext) -> Self {
        let issued_at = Instant::now();
        let expires_at = issued_at.checked_add(Self::VALIDITY).unwrap_or(issued_at);
        Self {
            context,
            expires_at,
        }
    }

    #[cfg(test)]
    pub(crate) const fn new_with_deadline_for_test(
        context: NativeCredentialEffectContext,
        expires_at: Instant,
    ) -> Self {
        Self {
            context,
            expires_at,
        }
    }

    pub fn context(&self) -> &NativeCredentialEffectContext {
        &self.context
    }

    /// Consumes this one-use receipt only when every backend-derived prompt
    /// field still matches the state immediately preceding the native effect.
    pub fn consume_exact(
        self,
        expected_effect: NativeCredentialEffect,
        expected_target_id: &str,
        expected_origin: &str,
        expected_revision: &str,
    ) -> crate::PlatformResult<()> {
        if Instant::now() >= self.expires_at {
            return Err(crate::PlatformError::new(
                crate::PlatformErrorCode::PermissionDenied,
            ));
        }
        if self.context.effect != expected_effect
            || self.context.target_id != expected_target_id
            || self.context.origin != expected_origin
            || self.context.revision != expected_revision
        {
            return Err(crate::PlatformError::new(
                crate::PlatformErrorCode::InvalidInput,
            ));
        }
        Ok(())
    }
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
#[serde(rename_all = "camelCase")]
#[cfg(mobile)]
pub(crate) struct MobileCredentialEffectConfirmationResponse {
    pub approved: bool,
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
