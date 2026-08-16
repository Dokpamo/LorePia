use std::{
    fmt,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
};

use zeroize::Zeroize;

/// Request-scoped credential material supplied by a native platform service.
///
/// This type intentionally implements neither `Serialize` nor `Clone`.
pub struct SecretCredential(String);

impl SecretCredential {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn into_core_value(mut self) -> String {
        std::mem::take(&mut self.0)
    }

    pub(crate) fn expose_to_core(&self) -> &str {
        &self.0
    }
}

/// Opaque process-local lease for one auxiliary provider dispatch.
///
/// This type intentionally implements neither serialization nor debug traits.
pub struct TaskCredentialLease(Arc<dyn Send + Sync>);

impl Clone for TaskCredentialLease {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl TaskCredentialLease {
    pub fn new(value: impl Send + Sync + 'static) -> Self {
        Self(Arc::new(value))
    }

    pub(crate) fn into_inner(self) -> Arc<dyn Send + Sync> {
        self.0
    }
}

/// Result of one native-vault lookup for an auxiliary task.
///
/// This enum intentionally has no serialization, clone, display, or debug
/// surface. `Unreadable` carries no backend diagnostic.
pub enum TaskCredentialRead {
    Available(SecretCredential),
    AvailableWithLease {
        credential: SecretCredential,
        lease: TaskCredentialLease,
    },
    Missing,
    MissingWithLease(TaskCredentialLease),
    Unreadable,
}

/// Rust-only native credential hook used by Core's background task runner.
///
/// The connection identifier is selected by Core. No credential, endpoint, or
/// request body is accepted from a Tauri command or the webview.
pub trait TaskCredentialReader: Send + Sync {
    fn credential_for<'a>(
        &'a self,
        connection_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = TaskCredentialRead> + Send + 'a>>;
}

/// Credential context for one generation selection.
///
/// Target credentials retain the Rust-selected provider connection identity
/// across native vault awaits. Legacy profile credentials remain explicitly
/// unbound for compatibility with the frozen native contract.
pub struct GenerationCredential(GenerationCredentialKind);

pub(crate) enum GenerationCredentialKind {
    Legacy {
        credential: Option<SecretCredential>,
        admission_lease: Option<lorepia_core::GenerationCredentialAdmissionLease>,
    },
    Connection {
        connection_id: String,
        credential: Option<SecretCredential>,
        access_authority: Option<crate::ProviderCredentialAccessAuthorityContext>,
        dispatch_lease: Option<TaskCredentialLease>,
    },
}

impl GenerationCredential {
    pub fn legacy(credential: Option<SecretCredential>) -> Self {
        Self(GenerationCredentialKind::Legacy {
            credential,
            admission_lease: None,
        })
    }

    pub fn legacy_with_admission_lease(
        credential: Option<SecretCredential>,
        admission_lease: impl Send + Sync + 'static,
    ) -> Self {
        Self(GenerationCredentialKind::Legacy {
            credential,
            admission_lease: Some(lorepia_core::GenerationCredentialAdmissionLease::new(
                admission_lease,
            )),
        })
    }

    pub fn connection(
        connection_id: impl Into<String>,
        credential: Option<SecretCredential>,
    ) -> Self {
        Self(GenerationCredentialKind::Connection {
            connection_id: connection_id.into(),
            credential,
            access_authority: None,
            dispatch_lease: None,
        })
    }

    pub fn connection_with_access_authority(
        connection_id: impl Into<String>,
        credential: Option<SecretCredential>,
        access_authority: crate::ProviderCredentialAccessAuthorityContext,
    ) -> Self {
        Self(GenerationCredentialKind::Connection {
            connection_id: connection_id.into(),
            credential,
            access_authority: Some(access_authority),
            dispatch_lease: None,
        })
    }

    pub fn connection_with_access_authority_and_dispatch_lease(
        connection_id: impl Into<String>,
        credential: Option<SecretCredential>,
        access_authority: crate::ProviderCredentialAccessAuthorityContext,
        dispatch_lease: TaskCredentialLease,
    ) -> Self {
        Self(GenerationCredentialKind::Connection {
            connection_id: connection_id.into(),
            credential,
            access_authority: Some(access_authority),
            dispatch_lease: Some(dispatch_lease),
        })
    }

    pub fn connection_with_dispatch_lease(
        connection_id: impl Into<String>,
        credential: Option<SecretCredential>,
        dispatch_lease: TaskCredentialLease,
    ) -> Self {
        Self(GenerationCredentialKind::Connection {
            connection_id: connection_id.into(),
            credential,
            access_authority: None,
            dispatch_lease: Some(dispatch_lease),
        })
    }

    pub(crate) fn into_kind(self) -> GenerationCredentialKind {
        self.0
    }
}

impl fmt::Debug for GenerationCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GenerationCredential([REDACTED])")
    }
}

/// Raw, potentially credential-bearing pasted cURL input.
///
/// Core parses this without executing a shell command. Like credentials, this
/// value cannot be cloned, serialized, displayed, or logged.
pub struct SecretProviderCurl(String);

impl SecretProviderCurl {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub(crate) fn into_core_value(mut self) -> lorepia_core::SecretCurlInput {
        lorepia_core::SecretCurlInput::new(std::mem::take(&mut self.0))
    }
}

impl fmt::Debug for SecretProviderCurl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretProviderCurl([REDACTED])")
    }
}

impl Drop for SecretProviderCurl {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Exact signed provider-catalog bytes retained by the native host between
/// review and activation.
///
/// The verified review DTO is serializable; the raw selected file is not.
pub struct SignedCatalogEnvelope(Vec<u8>);

impl SignedCatalogEnvelope {
    pub fn new(value: impl Into<Vec<u8>>) -> Self {
        Self(value.into())
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SignedCatalogEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SignedCatalogEnvelope([REDACTED])")
    }
}

impl Drop for SignedCatalogEnvelope {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretCredential([REDACTED])")
    }
}

impl Drop for SecretCredential {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// Platform-owned, bounded transport copy selected by a native file picker.
///
/// The host path can cross the Rust call boundary but cannot be serialized to
/// the frontend. `Core::inspect_import` immediately snapshots it into Core's
/// private staging area.
pub struct StagedImportFile(PathBuf);

impl StagedImportFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }
}

impl fmt::Debug for StagedImportFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("StagedImportFile([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use std::marker::PhantomData;

    use super::{
        GenerationCredential, SecretCredential, SecretProviderCurl, SignedCatalogEnvelope,
        StagedImportFile, TaskCredentialLease, TaskCredentialRead,
    };

    trait AmbiguousIfSerialize<Marker> {
        fn marker() {}
    }

    struct SerializeCheck<T: ?Sized>(PhantomData<T>);

    impl<T: ?Sized> AmbiguousIfSerialize<()> for SerializeCheck<T> {}
    impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<u8> for SerializeCheck<T> {}

    #[test]
    fn sensitive_boundary_debug_output_is_redacted() {
        let secret = "sk-shell-sensitive-canary";
        let path = "/Users/synthetic/private/card.json";

        assert!(!format!("{:?}", SecretCredential::new(secret)).contains(secret));
        assert!(
            !format!(
                "{:?}",
                GenerationCredential::connection(
                    "synthetic-connection",
                    Some(SecretCredential::new(secret))
                )
            )
            .contains(secret)
        );
        assert!(!format!("{:?}", SecretProviderCurl::new(secret)).contains(secret));
        assert!(
            !format!(
                "{:?}",
                SignedCatalogEnvelope::new(secret.as_bytes().to_vec())
            )
            .contains(secret)
        );
        assert!(!format!("{:?}", StagedImportFile::new(path)).contains(path));
    }

    #[test]
    fn sensitive_boundary_types_do_not_implement_serialize() {
        let _ = <SerializeCheck<SecretCredential> as AmbiguousIfSerialize<_>>::marker;
        let _ = <SerializeCheck<GenerationCredential> as AmbiguousIfSerialize<_>>::marker;
        let _ = <SerializeCheck<SecretProviderCurl> as AmbiguousIfSerialize<_>>::marker;
        let _ = <SerializeCheck<SignedCatalogEnvelope> as AmbiguousIfSerialize<_>>::marker;
        let _ = <SerializeCheck<StagedImportFile> as AmbiguousIfSerialize<_>>::marker;
        let _ = <SerializeCheck<TaskCredentialLease> as AmbiguousIfSerialize<_>>::marker;
        let _ = <SerializeCheck<TaskCredentialRead> as AmbiguousIfSerialize<_>>::marker;
    }
}
