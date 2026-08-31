//! Native-host coordination for ordinary provider credential effects.
//!
//! The webview never sees a credential, vault reference, authority marker, or
//! journal plan. Every native mutation is preceded by Core's durable cutpoint
//! and recovery only observes state; it never repeats a store/delete effect.

use std::{future::Future, pin::Pin};

use lorepia_shell_api::{
    ProviderCredentialAccessAuthorityContext, ProviderCredentialOperationContext,
    ProviderCredentialOperationKindInput, ProviderCredentialSlotStatusInput, ShellApi,
};
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tauri_plugin_lorepia_platform::{
    BoundCredentialObservation, CredentialAuthority, CredentialStatus, LegacyCredentialObservation,
    LorepiaPlatformExt, NativeCaptureStatus, NativeCredential, NativeCredentialEffect,
    NativeCredentialEffectConfirmation, NativeCredentialEffectContext, PlatformError,
    PlatformErrorCode, PlatformResult, PreparedBoundCredentialStore,
};

use crate::error::{CommandError, CommandResult};

type VaultFuture<'a, T> = Pin<Box<dyn Future<Output = PlatformResult<T>> + Send + 'a>>;

struct CapturedCredential {
    value: NativeCredential,
    status: NativeCaptureStatus,
}

enum PreparedCredentialStore {
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
    fn into_fake(self) -> (NativeCredential, CredentialAuthority) {
        match self {
            Self::Fake { value, authority } => (value, authority),
            Self::Platform(_) => {
                unreachable!("fake vault received a platform prepared credential store")
            }
        }
    }
}

trait CredentialVault: Send + Sync {
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

trait LegacyCredentialAccess: Send + Sync {
    fn ensure_legacy_raw_access(&self, provider_profile_id: &str) -> CommandResult<()>;
}

trait OrdinaryCredentialTargetPolicy: Send + Sync {
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

struct PlatformCredentialVault<'a> {
    app: &'a AppHandle,
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

pub(crate) async fn ensure_new_connection_slot_missing(
    app: &AppHandle,
    connection_id: &str,
) -> CommandResult<()> {
    ensure_slot_missing(&PlatformCredentialVault { app }, connection_id).await
}

pub(crate) async fn capture_provider_connection_credential(
    app: &AppHandle,
    shell: &ShellApi,
    connection_id: &str,
    confirmation: NativeCredentialEffectConfirmation,
) -> CommandResult<NativeCaptureStatus> {
    consume_connection_confirmation(
        shell,
        confirmation,
        NativeCredentialEffect::CaptureOrReplace,
        connection_id,
    )?;
    let vault = PlatformCredentialVault { app };
    let status = capture_provider_connection_credential_with(&vault, shell, connection_id).await?;
    recover_provider_credential_slot_garbage_with(&vault, shell).await?;
    Ok(status)
}

pub(crate) async fn delete_provider_connection_credential(
    app: &AppHandle,
    shell: &ShellApi,
    connection_id: &str,
    confirmation: NativeCredentialEffectConfirmation,
) -> CommandResult<()> {
    consume_connection_confirmation(
        shell,
        confirmation,
        NativeCredentialEffect::Delete,
        connection_id,
    )?;
    let vault = PlatformCredentialVault { app };
    remove_provider_credential_with(&vault, shell, connection_id, false).await?;
    recover_provider_credential_slot_garbage_with(&vault, shell).await
}

pub(crate) async fn archive_provider_connection(
    app: &AppHandle,
    shell: &ShellApi,
    connection_id: &str,
    credential_binding_required: bool,
    legacy_raw: bool,
    confirmation: Option<NativeCredentialEffectConfirmation>,
) -> CommandResult<()> {
    if !credential_binding_required {
        if legacy_raw || confirmation.is_some() {
            return Err(CommandError::invalid_input());
        }
        return shell
            .delete_provider_connection(connection_id)
            .map_err(Into::into);
    }
    let confirmation = confirmation.ok_or_else(CommandError::invalid_input)?;
    if legacy_raw {
        consume_legacy_confirmation(
            app,
            shell,
            confirmation,
            NativeCredentialEffect::Archive,
            connection_id,
        )
        .await?;
    } else {
        consume_connection_confirmation(
            shell,
            confirmation,
            NativeCredentialEffect::Archive,
            connection_id,
        )?;
    }
    let vault = PlatformCredentialVault { app };
    remove_provider_credential_with(&vault, shell, connection_id, true).await?;
    recover_provider_credential_slot_garbage_with(&vault, shell).await
}

pub(crate) async fn recover_provider_credential_operations(
    app: &AppHandle,
    shell: &ShellApi,
) -> CommandResult<()> {
    let vault = PlatformCredentialVault { app };
    recover_provider_credential_operations_with(&vault, shell).await?;
    recover_provider_credential_slot_garbage_with(&vault, shell).await
}

pub(crate) async fn read_provider_connection_credential(
    app: &AppHandle,
    shell: &ShellApi,
    connection_id: &str,
) -> CommandResult<ProviderConnectionCredentialRead> {
    read_provider_connection_credential_with(&PlatformCredentialVault { app }, shell, connection_id)
        .await
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

pub(crate) async fn capture_legacy_provider_credential(
    app: &AppHandle,
    shell: &ShellApi,
    provider_profile_id: &str,
    confirmation: NativeCredentialEffectConfirmation,
) -> CommandResult<NativeCaptureStatus> {
    consume_legacy_confirmation(
        app,
        shell,
        confirmation,
        NativeCredentialEffect::CaptureOrReplace,
        provider_profile_id,
    )
    .await?;
    capture_legacy_provider_credential_with(
        &PlatformCredentialVault { app },
        shell,
        provider_profile_id,
    )
    .await
}

pub(crate) async fn delete_legacy_provider_credential(
    app: &AppHandle,
    shell: &ShellApi,
    provider_profile_id: &str,
    confirmation: NativeCredentialEffectConfirmation,
) -> CommandResult<()> {
    consume_legacy_confirmation(
        app,
        shell,
        confirmation,
        NativeCredentialEffect::Delete,
        provider_profile_id,
    )
    .await?;
    delete_legacy_provider_credential_with(
        &PlatformCredentialVault { app },
        shell,
        provider_profile_id,
    )
    .await
}

fn consume_connection_confirmation(
    shell: &ShellApi,
    confirmation: NativeCredentialEffectConfirmation,
    expected_effect: NativeCredentialEffect,
    connection_id: &str,
) -> CommandResult<()> {
    let context =
        provider_connection_credential_effect_context(shell, connection_id, expected_effect)?;
    confirmation.consume_exact(
        context.effect(),
        context.target_id(),
        context.origin(),
        context.revision(),
    )?;
    Ok(())
}

pub(crate) fn provider_connection_credential_effect_context(
    shell: &ShellApi,
    connection_id: &str,
    effect: NativeCredentialEffect,
) -> CommandResult<NativeCredentialEffectContext> {
    let connection = shell
        .list_provider_connections()?
        .into_iter()
        .find(|candidate| candidate.id == connection_id)
        .ok_or_else(CommandError::invalid_input)?;
    if !connection.credential_binding_required {
        return Err(CommandError::invalid_input());
    }
    let unresolved = shell
        .list_unresolved_provider_credential_operations()?
        .into_iter()
        .filter(|operation| operation.connection_id == connection_id)
        .collect::<Vec<_>>();
    if unresolved.len() > 1
        || (effect == NativeCredentialEffect::CaptureOrReplace && !unresolved.is_empty())
    {
        return Err(CommandError::invalid_input());
    }
    let authority = shell.provider_credential_recovery_authority(connection_id)?;
    let mut state_hasher = Sha256::new();
    state_hasher.update(b"dev.lorepia.credential-confirmation-state.v1\0");
    state_hasher.update(format!("{connection:?}").as_bytes());
    state_hasher.update([0]);
    state_hasher.update(format!("{authority:?}").as_bytes());
    state_hasher.update([0]);
    state_hasher.update(format!("{unresolved:?}").as_bytes());
    let state_sha256 = format!("{:x}", state_hasher.finalize());
    let journal = unresolved
        .first()
        .map_or("settled", |operation| operation.status.as_str());
    NativeCredentialEffectContext::new(
        effect,
        connection.id,
        connection.api_origin,
        format!("credential_state_sha256={state_sha256};journal={journal}"),
    )
    .map_err(Into::into)
}

async fn consume_legacy_confirmation(
    app: &AppHandle,
    shell: &ShellApi,
    confirmation: NativeCredentialEffectConfirmation,
    expected_effect: NativeCredentialEffect,
    provider_profile_id: &str,
) -> CommandResult<()> {
    let profile = shell
        .list_provider_profiles()?
        .into_iter()
        .find(|candidate| candidate.id == provider_profile_id)
        .ok_or_else(CommandError::invalid_input)?;
    shell.ensure_legacy_profile_credential_mutation_settled(provider_profile_id)?;
    let revision = app
        .lorepia_platform()
        .legacy_credential_confirmation_revision(provider_profile_id)
        .await?;
    confirmation.consume_exact(expected_effect, &profile.id, &profile.base_url, &revision)?;
    Ok(())
}

pub(crate) async fn read_legacy_provider_credential(
    app: &AppHandle,
    shell: &ShellApi,
    provider_profile_id: &str,
) -> CommandResult<Option<NativeCredential>> {
    read_legacy_provider_credential_with(
        &PlatformCredentialVault { app },
        shell,
        provider_profile_id,
    )
    .await
}

#[cfg(test)]
async fn legacy_provider_credential_status_with(
    vault: &dyn CredentialVault,
    access: &dyn LegacyCredentialAccess,
    provider_profile_id: &str,
) -> CommandResult<CredentialStatus> {
    access.ensure_legacy_raw_access(provider_profile_id)?;
    match vault.observe_legacy(provider_profile_id).await? {
        LegacyCredentialObservation::Missing => Ok(CredentialStatus::Missing),
        LegacyCredentialObservation::Raw => Ok(CredentialStatus::Available),
        LegacyCredentialObservation::Bound | LegacyCredentialObservation::Unreadable => {
            Ok(CredentialStatus::Unreadable)
        }
    }
}

async fn capture_legacy_provider_credential_with(
    vault: &dyn CredentialVault,
    access: &dyn LegacyCredentialAccess,
    provider_profile_id: &str,
) -> CommandResult<NativeCaptureStatus> {
    access.ensure_legacy_raw_access(provider_profile_id)?;
    if !matches!(
        vault.observe_legacy(provider_profile_id).await?,
        LegacyCredentialObservation::Missing | LegacyCredentialObservation::Raw
    ) {
        return Err(CommandError::invalid_input());
    }
    let captured = vault.capture_legacy().await?;
    if !matches!(
        vault.observe_legacy(provider_profile_id).await?,
        LegacyCredentialObservation::Missing | LegacyCredentialObservation::Raw
    ) {
        return Err(CommandError::invalid_input());
    }
    vault.store_raw(provider_profile_id, captured.value).await?;
    Ok(captured.status)
}

async fn delete_legacy_provider_credential_with(
    vault: &dyn CredentialVault,
    access: &dyn LegacyCredentialAccess,
    provider_profile_id: &str,
) -> CommandResult<()> {
    access.ensure_legacy_raw_access(provider_profile_id)?;
    match vault.observe_legacy(provider_profile_id).await? {
        LegacyCredentialObservation::Missing => Ok(()),
        LegacyCredentialObservation::Raw => vault
            .delete_raw(provider_profile_id)
            .await
            .map_err(Into::into),
        LegacyCredentialObservation::Bound | LegacyCredentialObservation::Unreadable => {
            Err(CommandError::invalid_input())
        }
    }
}

async fn read_legacy_provider_credential_with(
    vault: &dyn CredentialVault,
    access: &dyn LegacyCredentialAccess,
    provider_profile_id: &str,
) -> CommandResult<Option<NativeCredential>> {
    access.ensure_legacy_raw_access(provider_profile_id)?;
    vault
        .read_legacy(provider_profile_id)
        .await
        .map_err(Into::into)
}

async fn read_provider_connection_credential_with(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
) -> CommandResult<ProviderConnectionCredentialRead> {
    let access_authority = shell.ensure_provider_credential_access_settled(connection_id)?;
    let native_authority = credential_authority(&access_authority)?;
    let credential = vault
        .read_bound(connection_id, native_authority)
        .await?
        .ok_or_else(|| {
            CommandError::from(PlatformError::new(
                PlatformErrorCode::CredentialRecoveryRequired,
            ))
        })?;
    Ok(ProviderConnectionCredentialRead {
        credential: Some(credential),
        access_authority,
    })
}

async fn ensure_slot_missing(
    vault: &dyn CredentialVault,
    connection_id: &str,
) -> CommandResult<()> {
    if vault.status(connection_id).await? != CredentialStatus::Missing {
        return Err(CommandError::invalid_input());
    }
    Ok(())
}

async fn capture_provider_connection_credential_with(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
) -> CommandResult<NativeCaptureStatus> {
    capture_provider_connection_credential_with_policy(vault, shell, shell, connection_id).await
}

async fn capture_provider_connection_credential_with_policy(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    target_policy: &dyn OrdinaryCredentialTargetPolicy,
    connection_id: &str,
) -> CommandResult<NativeCaptureStatus> {
    ensure_ordinary_connection_does_not_alias_legacy_raw_slot(target_policy, connection_id)?;
    let proposed = shell.propose_provider_credential_install_authority(connection_id)?;
    ensure_bound_slot_missing(vault, connection_id, &proposed).await?;
    let captured = match vault.capture_bound().await {
        Ok(captured) => captured,
        Err(error) => return Err(error.into()),
    };
    let (observed_before_start, observation_error) =
        observe_operation(vault, connection_id, credential_authority(&proposed)?).await;
    if observed_before_start != ProviderCredentialSlotStatusInput::Missing {
        if let Some(error) = observation_error {
            return Err(error);
        }
        return Err(CommandError::invalid_input());
    }
    let prepared = shell.prepare_provider_credential_install_operation(
        connection_id,
        &proposed,
        ProviderCredentialSlotStatusInput::Missing,
    )?;
    let authority = operation_authority(&prepared)?;
    let prepared_store = match vault.prepare_bound_store(connection_id, captured.value, &authority)
    {
        Ok(prepared_store) => prepared_store,
        Err(error) => {
            settle_pre_native_install_failure(vault, shell, connection_id, &prepared).await?;
            return Err(error.into());
        }
    };
    let started = match shell
        .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
    {
        Ok(started) => started,
        Err(error) => {
            settle_pre_native_install_failure(vault, shell, connection_id, &prepared).await?;
            return Err(error.into());
        }
    };
    if started.status != "started" || started.operation_id != prepared.operation_id {
        return Err(CommandError::internal());
    }
    if operation_authority(&started)? != authority {
        return Err(CommandError::internal());
    }
    let store_result = vault.store_prepared(prepared_store).await;
    if platform_result_requires_credential_recovery(&store_result) {
        persist_explicit_credential_recovery_barrier(
            shell,
            &started,
            false,
            CredentialDurabilityBarrier::OperationSlot,
        )?;
        return Err(store_result
            .expect_err("recovery-required result is an error")
            .into());
    }
    let (observed, observation_error) = observe_operation(vault, connection_id, authority).await;
    if observed == ProviderCredentialSlotStatusInput::Available {
        // B is now the verified recovery copy. If predecessor cleanup fails,
        // leave both exact authorities attached to the durable Started
        // operation. Startup fences it and explicit cleanup can safely remove
        // the operation slot; an ad-hoc rollback here could erase the only
        // surviving copy when the predecessor delete outcome is uncertain.
        remove_replacement_predecessor(vault, shell, connection_id, &started).await?;
    }
    let completed = shell.finish_provider_credential_operation(
        &started.operation_id,
        &started.plan_sha256,
        observed,
    )?;
    if let Some(error) = observation_error {
        return Err(error);
    }
    if completed.status == "succeeded" {
        return Ok(captured.status);
    }
    if let Err(error) = store_result {
        return Err(error.into());
    }
    Err(CommandError::invalid_input())
}

async fn remove_replacement_predecessor(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
    operation: &ProviderCredentialOperationContext,
) -> CommandResult<()> {
    if operation.operation_kind != ProviderCredentialOperationKindInput::Install {
        return Ok(());
    }
    let Some(authority) = operation_predecessor_authority(operation)? else {
        return Ok(());
    };
    let requires_durability_repair = operation.predecessor_slot_recovery_required;
    let (observed, _) = observe_operation(vault, connection_id, authority.clone()).await;
    shell.attest_provider_credential_predecessor_delete_intent(
        &operation.operation_id,
        &operation.plan_sha256,
        observed,
    )?;
    let delete_result =
        if observed == ProviderCredentialSlotStatusInput::Missing && !requires_durability_repair {
            None
        } else {
            Some(vault.delete_bound(connection_id, authority.clone()).await)
        };
    if let Some(Err(error)) = delete_result.as_ref()
        && error.code() == PlatformErrorCode::CredentialRecoveryRequired
    {
        persist_explicit_credential_recovery_barrier(
            shell,
            operation,
            operation.cleanup_archives_connection,
            CredentialDurabilityBarrier::PredecessorSlot,
        )?;
        return Err(error.clone().into());
    }
    if requires_durability_repair && let Some(Err(error)) = delete_result.as_ref() {
        // An already-Missing slot is not proof that this exact retry reached
        // the platform durability boundary. Keep the predecessor barrier until
        // an idempotent delete itself succeeds.
        return Err(error.clone().into());
    }
    let (postflight, observation_error) = observe_operation(vault, connection_id, authority).await;
    if postflight == ProviderCredentialSlotStatusInput::Missing {
        shell.attest_provider_credential_predecessor_missing(
            &operation.operation_id,
            &operation.plan_sha256,
        )?;
        if requires_durability_repair {
            shell.attest_provider_credential_predecessor_durability_repaired(
                &operation.operation_id,
                &operation.plan_sha256,
            )?;
        }
        return Ok(());
    }
    if let Some(error) = observation_error {
        return Err(error);
    }
    if let Some(Err(error)) = delete_result {
        return Err(error.into());
    }
    Err(CommandError::invalid_input())
}

async fn settle_pre_native_install_failure(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
    prepared: &ProviderCredentialOperationContext,
) -> CommandResult<()> {
    let (observed, observation_error) =
        observe_operation(vault, connection_id, operation_authority(prepared)?).await;
    let completed = shell.finish_provider_credential_operation(
        &prepared.operation_id,
        &prepared.plan_sha256,
        observed,
    )?;
    if let Some(error) = observation_error {
        return Err(error);
    }
    (completed.status == "no_effect")
        .then_some(())
        .ok_or_else(CommandError::invalid_input)
}

async fn remove_provider_credential_with(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
    archive: bool,
) -> CommandResult<()> {
    remove_provider_credential_with_policy(vault, shell, shell, connection_id, archive).await
}

async fn remove_provider_credential_with_policy(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    target_policy: &dyn OrdinaryCredentialTargetPolicy,
    connection_id: &str,
    archive: bool,
) -> CommandResult<()> {
    if !archive {
        ensure_ordinary_connection_does_not_alias_legacy_raw_slot(target_policy, connection_id)?;
    }
    match cleanup_uncertain_credential_for_explicit_delete(vault, shell, connection_id, archive)
        .await?
    {
        UncertainCredentialCleanup::ConnectionArchived => return Ok(()),
        UncertainCredentialCleanup::CredentialRemoved if !archive => return Ok(()),
        UncertainCredentialCleanup::NotApplicable
        | UncertainCredentialCleanup::CredentialRemoved => {}
    }
    let (preflight, _) = observe_existing_credential(vault, shell, connection_id).await?;
    let kind = if archive {
        ProviderCredentialOperationKindInput::RemoveForArchive
    } else {
        ProviderCredentialOperationKindInput::RemoveCredential
    };
    let prepared = shell.prepare_provider_credential_operation(connection_id, kind, preflight)?;
    if preflight == ProviderCredentialSlotStatusInput::Unreadable {
        return match cleanup_uncertain_credential_for_explicit_delete(
            vault,
            shell,
            connection_id,
            archive,
        )
        .await?
        {
            UncertainCredentialCleanup::CredentialRemoved
            | UncertainCredentialCleanup::ConnectionArchived => Ok(()),
            UncertainCredentialCleanup::NotApplicable => Err(CommandError::internal()),
        };
    }
    if preflight == ProviderCredentialSlotStatusInput::Missing {
        let completed = if archive {
            shell.finish_provider_credential_archive(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Missing,
            )?
        } else {
            shell.finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Missing,
            )?
        };
        return (completed.status == "no_effect")
            .then_some(())
            .ok_or_else(CommandError::internal);
    }
    run_started_provider_credential_removal(
        vault,
        shell,
        connection_id,
        archive,
        preflight,
        &prepared,
    )
    .await
}

async fn run_started_provider_credential_removal(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
    archive: bool,
    preflight: ProviderCredentialSlotStatusInput,
    prepared: &ProviderCredentialOperationContext,
) -> CommandResult<()> {
    let started =
        shell.start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)?;
    let slot_authority = operation_optional_authority(&started)?;
    let delete_result = if matches!(
        preflight,
        ProviderCredentialSlotStatusInput::Available
            | ProviderCredentialSlotStatusInput::Unreadable
    ) {
        Some(delete_operation_slot(vault, connection_id, slot_authority.clone()).await)
    } else {
        None
    };
    if let Some(Err(error)) = delete_result.as_ref()
        && error.code() == PlatformErrorCode::CredentialRecoveryRequired
    {
        persist_explicit_credential_recovery_barrier(
            shell,
            &started,
            archive,
            CredentialDurabilityBarrier::OperationSlot,
        )?;
        return Err(error.clone().into());
    }
    let (observed, observation_error) =
        observe_operation_slot(vault, connection_id, slot_authority).await;
    let completed = if archive && observed == ProviderCredentialSlotStatusInput::Missing {
        shell.finish_provider_credential_archive(
            &started.operation_id,
            &started.plan_sha256,
            observed,
        )?
    } else {
        shell.finish_provider_credential_operation(
            &started.operation_id,
            &started.plan_sha256,
            observed,
        )?
    };
    if let Some(error) = observation_error {
        return Err(error);
    }
    if observed == ProviderCredentialSlotStatusInput::Missing && completed.status == "succeeded" {
        return Ok(());
    }
    if let Some(Err(error)) = delete_result {
        return Err(error.into());
    }
    Err(CommandError::invalid_input())
}

fn ensure_ordinary_connection_does_not_alias_legacy_raw_slot(
    target_policy: &dyn OrdinaryCredentialTargetPolicy,
    connection_id: &str,
) -> CommandResult<()> {
    if target_policy.aliases_legacy_raw_slot(connection_id)? {
        return Err(CommandError::invalid_input());
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UncertainCredentialCleanup {
    NotApplicable,
    CredentialRemoved,
    ConnectionArchived,
}

#[derive(Clone, Copy)]
enum CredentialDurabilityBarrier {
    OperationSlot,
    PredecessorSlot,
}

fn unresolved_explicit_cleanup_operation(
    shell: &ShellApi,
    connection_id: &str,
) -> CommandResult<Option<ProviderCredentialOperationContext>> {
    Ok(shell
        .list_unresolved_provider_credential_operations()?
        .into_iter()
        .find(|operation| operation.connection_id == connection_id)
        .filter(|operation| {
            matches!(
                operation.status.as_str(),
                "started" | "outcome_unknown" | "cleanup_required"
            )
        }))
}

fn persist_explicit_cleanup_intent(
    shell: &ShellApi,
    operation: &ProviderCredentialOperationContext,
    observed: ProviderCredentialSlotStatusInput,
    archive: bool,
) -> CommandResult<ProviderCredentialOperationContext> {
    if operation.operation_slot_recovery_required {
        shell
            .mark_provider_credential_durability_recovery_required(
                &operation.operation_id,
                &operation.plan_sha256,
                archive,
            )
            .map_err(Into::into)
    } else if operation.predecessor_slot_recovery_required {
        shell
            .mark_provider_credential_predecessor_durability_recovery_required(
                &operation.operation_id,
                &operation.plan_sha256,
                archive,
            )
            .map_err(Into::into)
    } else {
        shell
            .mark_provider_credential_cleanup_required(
                &operation.operation_id,
                &operation.plan_sha256,
                observed,
                archive,
            )
            .map_err(Into::into)
    }
}

async fn cleanup_uncertain_credential_for_explicit_delete(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
    archive: bool,
) -> CommandResult<UncertainCredentialCleanup> {
    let Some(mut operation) = unresolved_explicit_cleanup_operation(shell, connection_id)? else {
        return Ok(UncertainCredentialCleanup::NotApplicable);
    };
    if operation.status == "started" {
        // Explicit cleanup can race the same post-native hard-crash cutpoint as
        // bootstrap. Fence intent-only Started state before observing a slot so
        // visibility can never terminalize an unrecorded durability outcome.
        operation = shell.fence_started_provider_credential_operation_for_recovery(
            &operation.operation_id,
            &operation.plan_sha256,
        )?;
    }
    let slot_authority = operation_optional_authority(&operation)?;
    let (observed, _) = observe_operation_slot(vault, connection_id, slot_authority.clone()).await;
    let requires_operation_durability_repair = operation.operation_slot_recovery_required;
    let requires_predecessor_durability_repair = operation.predecessor_slot_recovery_required;
    operation = persist_explicit_cleanup_intent(shell, &operation, observed, archive)?;
    if requires_predecessor_durability_repair {
        remove_replacement_predecessor(vault, shell, connection_id, &operation).await?;
        operation = shell
            .list_unresolved_provider_credential_operations()?
            .into_iter()
            .find(|candidate| candidate.operation_id == operation.operation_id)
            .ok_or_else(CommandError::internal)?;
    }
    if requires_operation_durability_repair
        || matches!(
            observed,
            ProviderCredentialSlotStatusInput::Available
                | ProviderCredentialSlotStatusInput::Unreadable
        )
    {
        let delete_result =
            delete_operation_slot(vault, connection_id, slot_authority.clone()).await;
        if platform_result_requires_credential_recovery(&delete_result) {
            persist_explicit_credential_recovery_barrier(
                shell,
                &operation,
                operation.cleanup_archives_connection || archive,
                CredentialDurabilityBarrier::OperationSlot,
            )?;
            return Err(delete_result
                .expect_err("recovery-required result is an error")
                .into());
        }
        if requires_operation_durability_repair && let Err(error) = delete_result.as_ref() {
            // Visibility cannot discharge a repair barrier unless this exact
            // native retry reported that it completed its durability work.
            return Err(error.clone().into());
        }
        let (postflight, error) =
            observe_operation_slot(vault, connection_id, slot_authority).await;
        if let Some(error) = error {
            return Err(error);
        }
        if postflight != ProviderCredentialSlotStatusInput::Missing {
            if let Err(error) = delete_result {
                return Err(error.into());
            }
            return Err(CommandError::invalid_input());
        }
        if requires_operation_durability_repair {
            operation = shell.attest_provider_credential_durability_repaired(
                &operation.operation_id,
                &operation.plan_sha256,
            )?;
        }
    }
    remove_replacement_predecessor(vault, shell, connection_id, &operation).await?;
    let archives_connection = operation.cleanup_archives_connection
        || (operation.operation_kind == ProviderCredentialOperationKindInput::RemoveForArchive
            && (operation.native_effect_started
                || operation.preflight_status == ProviderCredentialSlotStatusInput::Missing));
    let completed = if archives_connection {
        shell.reconcile_provider_credential_archive(
            &operation.operation_id,
            &operation.plan_sha256,
            ProviderCredentialSlotStatusInput::Missing,
        )?
    } else {
        shell.reconcile_provider_credential_operation(
            &operation.operation_id,
            &operation.plan_sha256,
            ProviderCredentialSlotStatusInput::Missing,
        )?
    };
    if matches!(completed.status.as_str(), "succeeded" | "no_effect") {
        Ok(if archives_connection {
            UncertainCredentialCleanup::ConnectionArchived
        } else {
            UncertainCredentialCleanup::CredentialRemoved
        })
    } else {
        Err(CommandError::invalid_input())
    }
}

async fn recover_provider_credential_operations_with(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
) -> CommandResult<()> {
    for mut operation in shell.list_unresolved_provider_credential_operations()? {
        if operation.status == "started" {
            operation = shell.fence_started_provider_credential_operation_for_recovery(
                &operation.operation_id,
                &operation.plan_sha256,
            )?;
        }
        if matches!(
            operation.status.as_str(),
            "outcome_unknown" | "cleanup_required"
        ) && (operation.operation_slot_recovery_required
            || operation.predecessor_slot_recovery_required)
        {
            // `CredentialRecoveryRequired` means the platform observed a
            // durability failure after a possible mutation. Visibility after
            // restart cannot discharge that explicit barrier; only an
            // explicit cleanup action may attempt the exact native slot again.
            continue;
        }
        let (observed, error) = observe_operation_slot(
            vault,
            &operation.connection_id,
            operation_optional_authority(&operation)?,
        )
        .await;
        let _ = error;
        if operation.operation_kind == ProviderCredentialOperationKindInput::Install
            && operation.predecessor_authority_id.is_some()
            && operation.status == "cleanup_required"
        {
            // Cleanup may still need the user-authorized predecessor delete.
            // Startup recovery never replays that native effect and therefore
            // leaves the exact cleanup disposition durable for explicit resume.
            continue;
        }
        if observed == ProviderCredentialSlotStatusInput::Missing
            && (operation.cleanup_archives_connection
                || (operation.operation_kind
                    == ProviderCredentialOperationKindInput::RemoveForArchive
                    && (operation.native_effect_started
                        || operation.preflight_status
                            == ProviderCredentialSlotStatusInput::Missing)))
        {
            shell.reconcile_provider_credential_archive(
                &operation.operation_id,
                &operation.plan_sha256,
                observed,
            )?;
        } else {
            shell.reconcile_provider_credential_operation(
                &operation.operation_id,
                &operation.plan_sha256,
                observed,
            )?;
        }
    }
    Ok(())
}

fn platform_result_requires_credential_recovery(result: &PlatformResult<()>) -> bool {
    matches!(
        result,
        Err(error) if error.code() == PlatformErrorCode::CredentialRecoveryRequired
    )
}

fn persist_explicit_credential_recovery_barrier(
    shell: &ShellApi,
    operation: &ProviderCredentialOperationContext,
    archive_connection: bool,
    barrier: CredentialDurabilityBarrier,
) -> CommandResult<()> {
    let blocked = match barrier {
        CredentialDurabilityBarrier::OperationSlot => shell
            .mark_provider_credential_durability_recovery_required(
                &operation.operation_id,
                &operation.plan_sha256,
                archive_connection,
            )?,
        CredentialDurabilityBarrier::PredecessorSlot => shell
            .mark_provider_credential_predecessor_durability_recovery_required(
                &operation.operation_id,
                &operation.plan_sha256,
                archive_connection,
            )?,
    };
    let target_is_blocked = match barrier {
        CredentialDurabilityBarrier::OperationSlot => blocked.operation_slot_recovery_required,
        CredentialDurabilityBarrier::PredecessorSlot => blocked.predecessor_slot_recovery_required,
    };
    if blocked.status != "cleanup_required" || !target_is_blocked {
        return Err(CommandError::internal());
    }
    Ok(())
}

async fn recover_provider_credential_slot_garbage_with(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
) -> CommandResult<()> {
    for target in shell.list_provider_credential_slot_garbage()? {
        // This journal is derived entirely from SQLite. Even a self-consistent
        // forged ownership history is therefore not authority to mutate an OS
        // credential store. Unattended recovery may prove that a never-started
        // target is already absent, but every present/unreadable target and
        // every legacy Started target stays durably unresolved until a future
        // native-user-confirmed cleanup flow owns the exact delete.
        if target.status == "started" {
            continue;
        }
        if target.status != "pending" {
            return Err(CommandError::internal());
        }
        let authority = credential_authority(&target.authority)?;
        let (observed, observation_error) =
            observe_gc_slot(vault, &target.connection_id, authority).await;
        if observation_error.is_some() || observed != ProviderCredentialSlotStatusInput::Missing {
            continue;
        }
        let completed = shell.observe_provider_credential_slot_garbage(
            &target.connection_id,
            target.authority_sequence,
            ProviderCredentialSlotStatusInput::Missing,
        )?;
        if completed.status != "completed" {
            return Err(CommandError::internal());
        }
    }
    Ok(())
}

async fn observe_gc_slot(
    vault: &dyn CredentialVault,
    connection_id: &str,
    authority: CredentialAuthority,
) -> (ProviderCredentialSlotStatusInput, Option<CommandError>) {
    let (observed, error) = observe_operation(vault, connection_id, authority.clone()).await;
    if error.is_none() {
        return (observed, None);
    }
    match vault.status_bound(connection_id, authority).await {
        Ok(CredentialStatus::Missing) => (ProviderCredentialSlotStatusInput::Missing, None),
        Ok(CredentialStatus::Available) => (ProviderCredentialSlotStatusInput::Available, None),
        Ok(CredentialStatus::Unreadable) => (ProviderCredentialSlotStatusInput::Unreadable, None),
        Err(error) => (
            ProviderCredentialSlotStatusInput::Unreadable,
            Some(error.into()),
        ),
    }
}

async fn observe_existing_credential(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
) -> CommandResult<(ProviderCredentialSlotStatusInput, Option<CommandError>)> {
    let Some(authority) = shell.provider_credential_recovery_authority(connection_id)? else {
        return Ok(raw_observation(vault, connection_id).await);
    };
    match vault
        .observe(connection_id, credential_authority(&authority)?)
        .await
    {
        Ok(BoundCredentialObservation::Missing) => {
            Ok((ProviderCredentialSlotStatusInput::Missing, None))
        }
        Ok(
            BoundCredentialObservation::Match
            | BoundCredentialObservation::Legacy
            | BoundCredentialObservation::Mismatch
            | BoundCredentialObservation::Unreadable,
        ) => Ok((ProviderCredentialSlotStatusInput::Available, None)),
        Err(_) => match vault
            .status_bound(connection_id, credential_authority(&authority)?)
            .await
        {
            Ok(CredentialStatus::Missing) => Ok((ProviderCredentialSlotStatusInput::Missing, None)),
            Ok(CredentialStatus::Available) => {
                Ok((ProviderCredentialSlotStatusInput::Available, None))
            }
            Ok(CredentialStatus::Unreadable) => {
                Ok((ProviderCredentialSlotStatusInput::Unreadable, None))
            }
            Err(error) => Ok((
                ProviderCredentialSlotStatusInput::Unreadable,
                Some(error.into()),
            )),
        },
    }
}

async fn observe_operation(
    vault: &dyn CredentialVault,
    connection_id: &str,
    authority: CredentialAuthority,
) -> (ProviderCredentialSlotStatusInput, Option<CommandError>) {
    match vault.observe(connection_id, authority).await {
        Ok(BoundCredentialObservation::Missing) => {
            (ProviderCredentialSlotStatusInput::Missing, None)
        }
        Ok(BoundCredentialObservation::Match) => {
            (ProviderCredentialSlotStatusInput::Available, None)
        }
        Ok(
            BoundCredentialObservation::Legacy
            | BoundCredentialObservation::Mismatch
            | BoundCredentialObservation::Unreadable,
        ) => (ProviderCredentialSlotStatusInput::Unreadable, None),
        Err(error) => (
            ProviderCredentialSlotStatusInput::Unreadable,
            Some(error.into()),
        ),
    }
}

async fn raw_observation(
    vault: &dyn CredentialVault,
    connection_id: &str,
) -> (ProviderCredentialSlotStatusInput, Option<CommandError>) {
    match vault.status(connection_id).await {
        Ok(CredentialStatus::Missing) => (ProviderCredentialSlotStatusInput::Missing, None),
        Ok(CredentialStatus::Available) => (ProviderCredentialSlotStatusInput::Available, None),
        Ok(CredentialStatus::Unreadable) => (ProviderCredentialSlotStatusInput::Unreadable, None),
        Err(error) => (
            ProviderCredentialSlotStatusInput::Unreadable,
            Some(error.into()),
        ),
    }
}

fn operation_authority(
    operation: &ProviderCredentialOperationContext,
) -> CommandResult<CredentialAuthority> {
    operation_optional_authority(operation)?.ok_or_else(CommandError::internal)
}

fn operation_optional_authority(
    operation: &ProviderCredentialOperationContext,
) -> CommandResult<Option<CredentialAuthority>> {
    match (
        operation.credential_authority_id.as_ref(),
        operation.credential_authority_binding_sha256.as_ref(),
    ) {
        (Some(authority_id), Some(binding_sha256)) => {
            CredentialAuthority::new(authority_id.clone(), binding_sha256.clone())
                .map(Some)
                .map_err(Into::into)
        }
        (None, None) => Ok(None),
        _ => Err(CommandError::internal()),
    }
}

fn operation_predecessor_authority(
    operation: &ProviderCredentialOperationContext,
) -> CommandResult<Option<CredentialAuthority>> {
    match (
        operation.predecessor_authority_id.as_ref(),
        operation.predecessor_authority_binding_sha256.as_ref(),
    ) {
        (Some(authority_id), Some(binding_sha256)) => {
            CredentialAuthority::new(authority_id.clone(), binding_sha256.clone())
                .map(Some)
                .map_err(Into::into)
        }
        (None, None) => Ok(None),
        _ => Err(CommandError::internal()),
    }
}

async fn ensure_bound_slot_missing(
    vault: &dyn CredentialVault,
    connection_id: &str,
    authority: &ProviderCredentialAccessAuthorityContext,
) -> CommandResult<()> {
    if vault
        .status_bound(connection_id, credential_authority(authority)?)
        .await?
        != CredentialStatus::Missing
    {
        return Err(CommandError::invalid_input());
    }
    Ok(())
}

async fn observe_operation_slot(
    vault: &dyn CredentialVault,
    connection_id: &str,
    authority: Option<CredentialAuthority>,
) -> (ProviderCredentialSlotStatusInput, Option<CommandError>) {
    match authority {
        Some(authority) => observe_operation(vault, connection_id, authority).await,
        None => raw_observation(vault, connection_id).await,
    }
}

async fn delete_operation_slot(
    vault: &dyn CredentialVault,
    connection_id: &str,
    authority: Option<CredentialAuthority>,
) -> PlatformResult<()> {
    match authority {
        Some(authority) => vault.delete_bound(connection_id, authority).await,
        None => vault.delete_raw(connection_id).await,
    }
}

fn credential_authority(
    authority: &ProviderCredentialAccessAuthorityContext,
) -> CommandResult<CredentialAuthority> {
    CredentialAuthority::new(
        authority.authority_id.clone(),
        authority.connection_binding_sha256.clone(),
    )
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    include!("credential_operations/tests/support.rs");
    include!("credential_operations/tests/install_and_replacement.rs");
    include!("credential_operations/tests/garbage_and_failure.rs");
    include!("credential_operations/tests/cleanup_and_restart.rs");
    include!("credential_operations/tests/snapshot_and_helpers.rs");
}
