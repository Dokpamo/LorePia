use std::{future::Future, pin::Pin};

use lorepia_shell_api::{ProviderCredentialAccessAuthorityContext, ShellApi};
use tauri::AppHandle;
use tauri_plugin_lorepia_platform::{
    BoundCredentialObservation, CredentialAuthority, CredentialStatus, LegacyCredentialObservation,
    LorepiaPlatformExt, NativeCaptureStatus, NativeCredential, PlatformResult,
    PreparedBoundCredentialStore,
};

use crate::error::CommandResult;

pub(super) type VaultFuture<'a, T> = Pin<Box<dyn Future<Output = PlatformResult<T>> + Send + 'a>>;

pub(super) struct CapturedCredential {
    pub(super) value: NativeCredential,
    pub(super) status: NativeCaptureStatus,
}

pub(super) enum PreparedCredentialStore {
    Platform(PreparedBoundCredentialStore),
    #[cfg(test)]
    Fake {
        value: NativeCredential,
        authority: CredentialAuthority,
    },
}

impl PreparedCredentialStore {
    fn into_platform(self) -> PreparedBoundCredentialStore {
        match self {
            Self::Platform(prepared) => prepared,
            #[cfg(test)]
            Self::Fake { .. } => {
                unreachable!("platform vault received a fake prepared credential store")
            }
        }
    }

    #[cfg(test)]
    pub(super) fn into_fake(self) -> (NativeCredential, CredentialAuthority) {
        match self {
            Self::Fake { value, authority } => (value, authority),
            Self::Platform(_) => {
                unreachable!("fake vault received a platform prepared credential store")
            }
        }
    }
}

pub(super) trait CredentialVault: Send + Sync {
    fn status<'a>(&'a self, reference: &'a str) -> VaultFuture<'a, CredentialStatus>;

    fn observe<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> VaultFuture<'a, BoundCredentialObservation>;

    fn status_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> VaultFuture<'a, CredentialStatus>;

    fn capture_bound(&self) -> VaultFuture<'_, CapturedCredential>;

    fn capture_legacy(&self) -> VaultFuture<'_, CapturedCredential>;

    fn prepare_bound_store(
        &self,
        reference: &str,
        value: NativeCredential,
        authority: &CredentialAuthority,
    ) -> PlatformResult<PreparedCredentialStore>;

    fn store_prepared(&self, prepared: PreparedCredentialStore) -> VaultFuture<'_, ()>;

    fn read_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> VaultFuture<'a, Option<NativeCredential>>;

    fn observe_legacy<'a>(
        &'a self,
        reference: &'a str,
    ) -> VaultFuture<'a, LegacyCredentialObservation>;

    fn read_legacy<'a>(&'a self, reference: &'a str) -> VaultFuture<'a, Option<NativeCredential>>;

    fn store_raw<'a>(&'a self, reference: &'a str, value: NativeCredential) -> VaultFuture<'a, ()>;

    fn delete_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> VaultFuture<'a, ()>;

    fn delete_raw<'a>(&'a self, reference: &'a str) -> VaultFuture<'a, ()>;
}

pub(super) trait LegacyCredentialAccess: Send + Sync {
    fn ensure_legacy_raw_access(&self, provider_profile_id: &str) -> CommandResult<()>;
}

pub(super) trait OrdinaryCredentialTargetPolicy: Send + Sync {
    fn aliases_legacy_raw_slot(&self, connection_id: &str) -> CommandResult<bool>;
}

impl LegacyCredentialAccess for ShellApi {
    fn ensure_legacy_raw_access(&self, provider_profile_id: &str) -> CommandResult<()> {
        self.ensure_legacy_profile_raw_credential_access(provider_profile_id)
            .map_err(Into::into)
    }
}

impl OrdinaryCredentialTargetPolicy for ShellApi {
    fn aliases_legacy_raw_slot(&self, connection_id: &str) -> CommandResult<bool> {
        self.provider_connection_uses_legacy_raw_credential(connection_id)
            .map_err(Into::into)
    }
}

pub(super) struct PlatformCredentialVault<'a> {
    pub(super) app: &'a AppHandle,
}

impl CredentialVault for PlatformCredentialVault<'_> {
    fn status<'a>(&'a self, reference: &'a str) -> VaultFuture<'a, CredentialStatus> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .credential_status(reference)
                .await
        })
    }

    fn observe<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> VaultFuture<'a, BoundCredentialObservation> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .observe_bound_credential(reference, &authority)
                .await
        })
    }

    fn status_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> VaultFuture<'a, CredentialStatus> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .bound_credential_status(reference, &authority)
                .await
        })
    }

    fn capture_bound(&self) -> VaultFuture<'_, CapturedCredential> {
        Box::pin(async move {
            let captured = self
                .app
                .lorepia_platform()
                .capture_credential_text_from_clipboard()
                .await?;
            Ok(CapturedCredential {
                status: captured.status(),
                value: NativeCredential::new(captured.into_secret_string()),
            })
        })
    }

    fn capture_legacy(&self) -> VaultFuture<'_, CapturedCredential> {
        Box::pin(async move {
            let captured = self
                .app
                .lorepia_platform()
                .capture_legacy_credential_text_from_clipboard()
                .await?;
            Ok(CapturedCredential {
                status: captured.status(),
                value: NativeCredential::new(captured.into_secret_string()),
            })
        })
    }

    fn prepare_bound_store(
        &self,
        reference: &str,
        value: NativeCredential,
        authority: &CredentialAuthority,
    ) -> PlatformResult<PreparedCredentialStore> {
        self.app
            .lorepia_platform()
            .prepare_bound_credential_store(reference, value, authority)
            .map(PreparedCredentialStore::Platform)
    }

    fn store_prepared(&self, prepared: PreparedCredentialStore) -> VaultFuture<'_, ()> {
        let prepared = prepared.into_platform();
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .store_prepared_bound_credential(prepared)
                .await
        })
    }

    fn delete_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> VaultFuture<'a, ()> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .delete_bound_credential(reference, &authority)
                .await
        })
    }

    fn delete_raw<'a>(&'a self, reference: &'a str) -> VaultFuture<'a, ()> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .delete_credential(reference)
                .await
        })
    }

    fn read_bound<'a>(
        &'a self,
        reference: &'a str,
        authority: CredentialAuthority,
    ) -> VaultFuture<'a, Option<NativeCredential>> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .read_bound_credential(reference, &authority)
                .await
        })
    }

    fn observe_legacy<'a>(
        &'a self,
        reference: &'a str,
    ) -> VaultFuture<'a, LegacyCredentialObservation> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .observe_legacy_credential(reference)
                .await
        })
    }

    fn read_legacy<'a>(&'a self, reference: &'a str) -> VaultFuture<'a, Option<NativeCredential>> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .read_legacy_credential(reference)
                .await
        })
    }

    fn store_raw<'a>(&'a self, reference: &'a str, value: NativeCredential) -> VaultFuture<'a, ()> {
        Box::pin(async move {
            self.app
                .lorepia_platform()
                .store_credential(reference, value)
                .await
        })
    }
}
/// Rust-only result of one authority-bound native vault read.
///
/// The authority and credential are captured as one indivisible carrier so
/// later durable admission can reject a read whose ownership epoch changed.
pub(crate) struct ProviderConnectionCredentialRead {
    pub(crate) credential: Option<NativeCredential>,
    pub(crate) access_authority: ProviderCredentialAccessAuthorityContext,
}

impl std::fmt::Debug for ProviderConnectionCredentialRead {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderConnectionCredentialRead")
            .field("credential_present", &self.credential.is_some())
            .field("access_authority", &self.access_authority)
            .finish()
    }
}
