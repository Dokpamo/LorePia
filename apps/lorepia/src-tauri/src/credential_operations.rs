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

    #[tokio::test]
    async fn raw_available_slot_blocks_unowned_create_but_isolated_bound_install_succeeds() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let vault = FakeVault::new(shell.clone(), FakeItem::Raw, "unused-secret");
        ensure_slot_missing(&vault, "orphan-slot")
            .await
            .expect_err("unowned available slot must block create");
        assert!(
            shell
                .list_provider_connections()
                .expect("connections")
                .is_empty()
        );

        create_credential_connection(&shell, "capture-guard");
        capture_provider_connection_credential_with(&vault, &shell, "capture-guard")
            .await
            .expect("authority-derived bound install must not overwrite the raw logical slot");
        assert!(matches!(vault.raw_item(), FakeItem::Raw));
        assert!(matches!(vault.bound_item(), FakeItem::Bound { .. }));
        assert_eq!(vault.counts(), (2, 1, 1, 0));
    }

    #[tokio::test]
    async fn legacy_surface_never_reads_overwrites_or_deletes_a_bound_envelope() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let vault = FakeVault::new_raw(
            shell,
            FakeItem::Bound {
                authority_id: "owned-install".to_owned(),
                binding_sha256: "a".repeat(64),
                secret: "must-not-escape".to_owned(),
            },
            "must-not-capture",
        );
        let access = FakeLegacyAccess { allowed: true };

        assert_eq!(
            legacy_provider_credential_status_with(&vault, &access, "legacy-bound")
                .await
                .expect("safe status"),
            CredentialStatus::Unreadable
        );
        read_legacy_provider_credential_with(&vault, &access, "legacy-bound")
            .await
            .expect_err("bound envelope must never be returned as a legacy secret");
        capture_legacy_provider_credential_with(&vault, &access, "legacy-bound")
            .await
            .expect_err("bound envelope must never be overwritten by legacy capture");
        delete_legacy_provider_credential_with(&vault, &access, "legacy-bound")
            .await
            .expect_err("bound envelope must never be deleted outside its journal");
        assert_eq!(vault.counts(), (0, 0, 0, 0));
        assert_eq!(vault.legacy_counts(), (3, 1, 0, 0));

        let denied = FakeLegacyAccess { allowed: false };
        legacy_provider_credential_status_with(&vault, &denied, "legacy-bound")
            .await
            .expect_err("durably owned slot must be rejected before native status");
        assert_eq!(vault.legacy_counts(), (3, 1, 0, 0));
    }

    #[tokio::test]
    async fn confirmation_revision_changes_when_current_credential_authority_rotates() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "confirmation-authority-rotation");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "rotated-secret");
        let before = provider_connection_credential_effect_context(
            &shell,
            "confirmation-authority-rotation",
            NativeCredentialEffect::CaptureOrReplace,
        )
        .expect("no-credential confirmation context");

        capture_provider_connection_credential_with(
            &vault,
            &shell,
            "confirmation-authority-rotation",
        )
        .await
        .expect("rotate into a durable current authority");
        let after = provider_connection_credential_effect_context(
            &shell,
            "confirmation-authority-rotation",
            NativeCredentialEffect::CaptureOrReplace,
        )
        .expect("owned confirmation context");

        assert!(before.revision().ends_with("journal=settled"));
        assert_ne!(before.revision(), after.revision());
    }

    #[tokio::test]
    async fn delete_confirmation_binds_exact_unresolved_cleanup_state() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "confirmation-unresolved-cleanup");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "owned-secret");
        capture_provider_connection_credential_with(
            &vault,
            &shell,
            "confirmation-unresolved-cleanup",
        )
        .await
        .expect("install current authority");
        let prepared = shell
            .prepare_provider_credential_operation(
                "confirmation-unresolved-cleanup",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare explicit cleanup");
        let prepared_context = provider_connection_credential_effect_context(
            &shell,
            "confirmation-unresolved-cleanup",
            NativeCredentialEffect::Delete,
        )
        .expect("delete can confirm the exact unresolved cleanup");
        assert!(prepared_context.revision().ends_with("journal=prepared"));
        assert!(
            provider_connection_credential_effect_context(
                &shell,
                "confirmation-unresolved-cleanup",
                NativeCredentialEffect::CaptureOrReplace,
            )
            .is_err(),
            "capture cannot layer over unresolved credential work"
        );

        shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("advance exact cleanup cutpoint");
        let started_context = provider_connection_credential_effect_context(
            &shell,
            "confirmation-unresolved-cleanup",
            NativeCredentialEffect::Delete,
        )
        .expect("started cleanup remains explicitly confirmable");
        assert!(started_context.revision().ends_with("journal=started"));
        assert_ne!(prepared_context.revision(), started_context.revision());
    }

    #[tokio::test]
    async fn ordinary_credential_actions_reject_legacy_alias_but_archive_removes_it_durably() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "dual-written-legacy");
        let vault = FakeVault::new(shell.clone(), FakeItem::Raw, "must-not-capture-as-ordinary");
        let policy = FakeOrdinaryTargetPolicy {
            aliases_legacy_raw_slot: true,
        };
        let settings_before = shell
            .get_settings()
            .expect("settings before rejected actions");
        let connections_before = shell
            .list_provider_connections()
            .expect("connections before rejected actions");
        let unresolved_before = shell
            .list_unresolved_provider_credential_operations()
            .expect("journal before rejected actions");

        super::capture_provider_connection_credential_with_policy(
            &vault,
            &shell,
            &policy,
            "dual-written-legacy",
        )
        .await
        .expect_err("ordinary capture must not convert an eligible legacy raw slot");
        remove_provider_credential_with_policy(
            &vault,
            &shell,
            &policy,
            "dual-written-legacy",
            false,
        )
        .await
        .expect_err("ordinary delete must not remove an eligible legacy raw slot");
        assert_eq!(vault.counts(), (0, 0, 0, 0));
        assert_eq!(vault.legacy_counts(), (0, 0, 0, 0));
        assert!(matches!(vault.item(), FakeItem::Raw));
        assert_eq!(
            shell
                .get_settings()
                .expect("settings after rejected actions"),
            settings_before
        );
        assert_eq!(
            shell
                .list_provider_connections()
                .expect("connections after rejected actions"),
            connections_before
        );
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("journal after rejected actions"),
            unresolved_before
        );

        remove_provider_credential_with_policy(
            &vault,
            &shell,
            &policy,
            "dual-written-legacy",
            true,
        )
        .await
        .expect("connection archive durably removes the aliased raw slot and connection");
        assert_eq!(
            vault.counts(),
            (2, 0, 0, 0),
            "archive observes the raw slot before and after deletion without a bound mutation"
        );
        assert_eq!(vault.legacy_counts(), (0, 0, 0, 1));
        assert!(matches!(vault.item(), FakeItem::Missing));
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("archive journal terminalized")
                .is_empty()
        );
        assert!(
            shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "dual-written-legacy")
        );
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen archived root");
        assert!(
            reopened
                .list_provider_connections()
                .expect("reopened active connections")
                .iter()
                .all(|connection| connection.id != "dual-written-legacy")
        );
    }

    #[tokio::test]
    async fn legitimate_legacy_pending_raw_slot_remains_usable() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let vault = FakeVault::new(shell, FakeItem::Raw, "replacement-legacy-secret");
        let access = FakeLegacyAccess { allowed: true };

        assert_eq!(
            legacy_provider_credential_status_with(&vault, &access, "legacy-raw")
                .await
                .expect("legacy raw status"),
            CredentialStatus::Available
        );
        assert_eq!(
            read_legacy_provider_credential_with(&vault, &access, "legacy-raw")
                .await
                .expect("legacy raw read")
                .expect("legacy raw value")
                .into_secret_string(),
            "synthetic-legacy-raw-secret"
        );
        capture_legacy_provider_credential_with(&vault, &access, "legacy-raw")
            .await
            .expect("replace legacy raw credential");
        delete_legacy_provider_credential_with(&vault, &access, "legacy-raw")
            .await
            .expect("delete legacy raw credential");
        assert!(matches!(vault.item(), FakeItem::Missing));
        assert_eq!(vault.legacy_counts(), (4, 1, 1, 1));
    }

    #[tokio::test]
    async fn legacy_raw_capture_retains_the_full_native_credential_limit() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let access = FakeLegacyAccess { allowed: true };
        let maximum = FakeVault::new(
            shell.clone(),
            FakeItem::Missing,
            &"r".repeat(MAXIMUM_LEGACY_CREDENTIAL_BYTES),
        );
        capture_legacy_provider_credential_with(&maximum, &access, "legacy-maximum")
            .await
            .expect("the historical 16 KiB raw credential remains valid");
        assert!(matches!(maximum.item(), FakeItem::Raw));

        let oversized = FakeVault::new(
            shell,
            FakeItem::Missing,
            &"r".repeat(MAXIMUM_LEGACY_CREDENTIAL_BYTES + 1),
        );
        capture_legacy_provider_credential_with(&oversized, &access, "legacy-oversized")
            .await
            .expect_err("a raw credential above the native 16 KiB limit is rejected");
        assert!(matches!(oversized.item(), FakeItem::Missing));
        assert_eq!(oversized.legacy_counts().2, 0);
    }

    #[tokio::test]
    async fn install_is_started_before_single_store_and_journal_is_secret_free() {
        const SECRET: &str = "synthetic-fake-vault-secret-canary";
        let secret_sha256 = format!("{:x}", Sha256::digest(SECRET.as_bytes()));
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "bound-install");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, SECRET);
        capture_provider_connection_credential_with(&vault, &shell, "bound-install")
            .await
            .expect("install bound credential");
        assert_eq!(vault.counts(), (1, 1, 1, 0));
        let authority = shell
            .ensure_provider_credential_access_settled("bound-install")
            .expect("durable access authority");
        let debug = format!("{authority:?}");
        assert!(!debug.contains(SECRET));
        assert!(!debug.contains(&secret_sha256));
        drop(vault);
        drop(shell);
        assert_tree_excludes(root.path(), &[SECRET, &secret_sha256]);
    }

    #[tokio::test]
    async fn replacement_stores_successor_before_deleting_exact_predecessor_and_preserves_raw_slot()
    {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "replacement-order");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "replacement-secret");

        capture_provider_connection_credential_with(&vault, &shell, "replacement-order")
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled("replacement-order")
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        assert_eq!(vault.bound_slot_count(), 1);
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        vault.replace_raw_item(FakeItem::Raw);

        capture_provider_connection_credential_with(&vault, &shell, "replacement-order")
            .await
            .expect("replacement B stores before deleting and attesting predecessor A");
        let authority_b = shell
            .ensure_provider_credential_access_settled("replacement-order")
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
        assert_eq!(vault.bound_slot_count(), 1);
        assert!(matches!(vault.raw_item(), FakeItem::Raw));

        let events = vault.events();
        let delete_a = events
            .iter()
            .position(|event| event == &FakeVaultEvent::Delete(key_a.clone()))
            .expect("exact predecessor A delete");
        let store_b = events
            .iter()
            .position(|event| event == &FakeVaultEvent::Store(key_b.clone()))
            .expect("exact replacement B store");
        assert!(store_b < delete_a, "B must be stored before A is deleted");
    }

    #[tokio::test]
    async fn replacement_missing_successor_never_starts_predecessor_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let connection_id = "replacement-missing-successor";
        create_credential_connection(&shell, connection_id);
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");

        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        vault.replace_capture_secret("secret-b");
        vault.fail_store_before_mutation();

        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect_err("missing B publication cannot authorize deleting A");

        assert_eq!(vault.bound_slot_count(), 1);
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(
            vault
                .events()
                .iter()
                .filter(|event| event == &&FakeVaultEvent::Delete(key_a.clone()))
                .count(),
            0,
            "A deletion must remain downstream of verified B publication"
        );
        assert_eq!(
            operation_predecessor_authority(
                &shell
                    .list_unresolved_provider_credential_operations()
                    .expect("failed replacement remains journaled")[0]
            )
            .expect("predecessor authority parses")
            .as_ref()
            .map(FakeAuthorityKey::from_authority),
            Some(key_a.clone())
        );

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("startup fences missing B without touching A");
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(
            vault
                .events()
                .iter()
                .filter(|event| event == &&FakeVaultEvent::Delete(key_a.clone()))
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn replacement_predecessor_failure_keeps_a_and_b_in_durable_recoverable_state() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "replacement-prepared-drop");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "replacement-secret");

        capture_provider_connection_credential_with(&vault, &shell, "replacement-prepared-drop")
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled("replacement-prepared-drop")
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        vault.fail_delete_before_mutation();

        capture_provider_connection_credential_with(&vault, &shell, "replacement-prepared-drop")
            .await
            .expect_err("failed A cleanup leaves the verified B store journaled");

        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("replacement failure remains journaled");
        assert_eq!(unresolved.len(), 1);
        let key_b = FakeAuthorityKey::from_authority(
            &operation_authority(&unresolved[0]).expect("successor B authority"),
        );
        assert_eq!(
            operation_predecessor_authority(&unresolved[0])
                .expect("predecessor authority parses")
                .as_ref()
                .map(FakeAuthorityKey::from_authority),
            Some(key_a.clone()),
        );
        assert_eq!(vault.counts().2, 2, "B is stored before A cleanup starts");
        assert_eq!(vault.bound_slot_count(), 2);
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("startup fences the unresolved replacement");
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("replacement remains recoverable")[0]
                .status,
            "cleanup_required"
        );
        assert!(
            shell
                .ensure_provider_credential_access_settled("replacement-prepared-drop")
                .is_err(),
            "neither journaled slot is exposed as settled provider authority"
        );

        remove_provider_credential_with(&vault, &shell, "replacement-prepared-drop", false)
            .await
            .expect("explicit cleanup removes the exact journaled slots");
        assert_eq!(vault.bound_slot_count(), 0);
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("cleanup settles the replacement")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn replacement_crash_cleanup_resumes_predecessor_at_every_cutpoint() {
        for archive in [false, true] {
            for cutpoint in 0..3 {
                let root = tempdir().expect("root");
                let shell = ShellApi::open_data_root(root.path()).expect("shell");
                let connection_id = format!("replacement-crash-{archive}-{cutpoint}");
                create_credential_connection(&shell, &connection_id);
                let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
                capture_provider_connection_credential_with(&vault, &shell, &connection_id)
                    .await
                    .expect("install predecessor A");
                let authority_a = shell
                    .ensure_provider_credential_access_settled(&connection_id)
                    .expect("authority A");
                let key_a = FakeAuthorityKey {
                    authority_id: authority_a.authority_id,
                    binding_sha256: authority_a.connection_binding_sha256,
                };
                let item_a = vault.bound_item_for(&key_a).expect("restorable A envelope");

                let proposed_b = shell
                    .propose_provider_credential_install_authority(&connection_id)
                    .expect("propose replacement B");
                let prepared_b = shell
                    .prepare_provider_credential_install_operation(
                        &connection_id,
                        &proposed_b,
                        ProviderCredentialSlotStatusInput::Missing,
                    )
                    .expect("prepare replacement B");
                let started_b = shell
                    .start_provider_credential_operation(
                        &prepared_b.operation_id,
                        &prepared_b.plan_sha256,
                    )
                    .expect("start replacement B");

                if cutpoint >= 1 {
                    shell
                        .attest_provider_credential_predecessor_delete_intent(
                            &started_b.operation_id,
                            &started_b.plan_sha256,
                            ProviderCredentialSlotStatusInput::Available,
                        )
                        .expect("persist predecessor delete intent");
                }
                if cutpoint == 2 {
                    vault
                        .delete_bound(&connection_id, native_authority(&key_a))
                        .await
                        .expect("delete A before simulated crash");
                }
                let deletes_before_cleanup = vault.counts().3;

                remove_provider_credential_with(&vault, &shell, &connection_id, archive)
                    .await
                    .unwrap_or_else(|error| {
                        panic!(
                            "explicit cleanup resumes predecessor and settles replacement B; archive={archive} cutpoint={cutpoint}: {error:?}"
                        )
                    });

                assert!(vault.bound_item_for(&key_a).is_none());
                // Before any predecessor intent exists, Started may also
                // represent an attempted B publication. Explicit recovery
                // therefore repairs B and then removes A. Once predecessor
                // cleanup intent exists, only the exact A retry remains.
                let expected_cleanup_deletes = if cutpoint == 0 { 2 } else { 1 };
                assert_eq!(
                    vault.counts().3,
                    deletes_before_cleanup + expected_cleanup_deletes,
                    "cleanup must repair every possibly attempted slot and repeat predecessor deletion until durable missing evidence exists"
                );
                assert!(
                    shell
                        .list_unresolved_provider_credential_operations()
                        .expect("replacement cleanup terminalized")
                        .is_empty()
                );
                recover_provider_credential_operations_with(&vault, &shell)
                    .await
                    .expect("first post-cleanup bootstrap is idempotent");
                recover_provider_credential_operations_with(&vault, &shell)
                    .await
                    .expect("second post-cleanup bootstrap is idempotent");
                drop(vault);
                drop(shell);

                let reopened = ShellApi::open_data_root(root.path()).expect("reopen cleanup root");
                let restored = FakeVault::new(reopened.clone(), item_a, "must-not-capture");
                read_provider_connection_credential_with(&restored, &reopened, &connection_id)
                    .await
                    .expect_err("restored predecessor A must remain unauthorized");
            }
        }
    }

    struct ReplacementArchiveRestartFixture {
        root: TempDir,
        connection_id: &'static str,
        key_a: FakeAuthorityKey,
        item_a: FakeItem,
    }

    async fn prepare_replacement_archive_restart(
        deleted_b_before_crash: bool,
    ) -> ReplacementArchiveRestartFixture {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let connection_id = if deleted_b_before_crash {
            "replacement-archive-crash-after-b-delete"
        } else {
            "replacement-archive-crash-after-mark"
        };
        create_credential_connection(&shell, connection_id);
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        let item_a = vault
            .bound_item_for(&key_a)
            .expect("predecessor A physical slot");
        let proposed_b = shell
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose B");
        let prepared_b = shell
            .prepare_provider_credential_install_operation(
                connection_id,
                &proposed_b,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("prepare B");
        let started_b = shell
            .start_provider_credential_operation(&prepared_b.operation_id, &prepared_b.plan_sha256)
            .expect("start B");
        let key_b = FakeAuthorityKey {
            authority_id: started_b
                .credential_authority_id
                .clone()
                .expect("B authority id"),
            binding_sha256: started_b
                .credential_authority_binding_sha256
                .clone()
                .expect("B binding"),
        };
        let observed_b = if deleted_b_before_crash {
            vault.insert_bound_item(
                key_b.clone(),
                FakeItem::Bound {
                    authority_id: key_b.authority_id.clone(),
                    binding_sha256: key_b.binding_sha256.clone(),
                    secret: "partial-secret-b".to_owned(),
                },
            );
            ProviderCredentialSlotStatusInput::Available
        } else {
            ProviderCredentialSlotStatusInput::Missing
        };
        shell
            .mark_provider_credential_cleanup_required(
                &started_b.operation_id,
                &started_b.plan_sha256,
                observed_b,
                true,
            )
            .expect("persist cleanup archive intent before crash");
        if deleted_b_before_crash {
            vault
                .delete_bound(connection_id, native_authority(&key_b))
                .await
                .expect("delete partial B before crash");
        }
        drop(vault);
        drop(shell);
        ReplacementArchiveRestartFixture {
            root,
            connection_id,
            key_a,
            item_a,
        }
    }

    async fn assert_replacement_archive_restart(deleted_b_before_crash: bool) {
        let fixture = prepare_replacement_archive_restart(deleted_b_before_crash).await;
        let reopened = ShellApi::open_data_root(fixture.root.path()).expect("reopen crash root");
        let vault = FakeVault::new(reopened.clone(), FakeItem::Missing, "must-not-capture");
        vault.insert_bound_item(fixture.key_a.clone(), fixture.item_a);

        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("bootstrap defers archive until predecessor cleanup resumes");
        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("repeated bootstrap remains idempotently deferred");
        let unresolved = reopened
            .list_unresolved_provider_credential_operations()
            .expect("deferred cleanup remains visible");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert!(unresolved[0].cleanup_archives_connection);
        assert!(
            reopened
                .list_provider_connections()
                .expect("connection remains active before exact cleanup")
                .iter()
                .any(|connection| connection.id == fixture.connection_id)
        );
        assert!(matches!(
            vault.bound_item_for(&fixture.key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.counts().3, 0);

        remove_provider_credential_with(&vault, &reopened, fixture.connection_id, true)
            .await
            .expect("explicit archive resumes A cleanup and atomically terminalizes");
        assert!(vault.bound_item_for(&fixture.key_a).is_none());
        assert_eq!(vault.counts().3, 1);
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("cleanup terminal")
                .is_empty()
        );
        assert!(
            reopened
                .list_provider_connections()
                .expect("archived connection")
                .iter()
                .all(|connection| connection.id != fixture.connection_id)
        );
    }

    #[tokio::test]
    async fn replacement_archive_cleanup_restart_defers_until_predecessor_can_resume() {
        for deleted_b_before_crash in [false, true] {
            assert_replacement_archive_restart(deleted_b_before_crash).await;
        }
    }

    #[tokio::test]
    async fn pending_missing_superseded_slot_completes_gc_without_native_delete() {
        let (_root, shell, vault, key_a, _item_a, key_b) =
            replacement_gc_fixture("gc-pending-missing").await;
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("missing A completes without a native effect");

        assert_eq!(vault.counts().3, deletes_before);
        assert!(vault.bound_item_for(&key_a).is_none());
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("completed garbage is hidden")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn unattended_gc_never_deletes_a_sqlite_derived_available_slot() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-available").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.replace_raw_item(FakeItem::Raw);
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("unattended GC observes but never deletes superseded A");

        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.bound_slot_count(), 2);
        assert!(matches!(vault.raw_item(), FakeItem::Raw));
        assert_eq!(vault.counts().3, deletes_before);
        assert!(!vault.events().contains(&FakeVaultEvent::Delete(key_b)));
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("available target remains unresolved")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn unattended_gc_never_calls_a_delete_that_would_mutate_then_error() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-response-loss").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.fail_delete_after_mutation();
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("unattended GC never enters the native delete path");

        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("present target remains durable")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn unattended_gc_never_uses_native_delete_to_repair_durability() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-durability-recovery-required").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.require_recovery_after_delete_mutation();
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("SQLite-derived work cannot authorize a durability-repair delete");
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        let unresolved = shell
            .list_provider_credential_slot_garbage()
            .expect("durability repair remains journaled");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "pending");
        assert_eq!(vault.counts().3, deletes_before);

        vault.fail_delete_before_mutation();
        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("retries remain observe-only");
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("unresolved target remains journaled")[0]
                .status,
            "pending"
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
    }

    #[tokio::test]
    async fn unattended_gc_never_resumes_a_started_sqlite_derived_delete() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-started-before-delete").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        let deletes_before = vault.counts().3;
        let target = shell
            .list_provider_credential_slot_garbage()
            .expect("pending target")
            .pop()
            .expect("target");
        let started = shell
            .observe_provider_credential_slot_garbage(
                &target.connection_id,
                target.authority_sequence,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("durable delete cutpoint before simulated crash");
        assert_eq!(started.status, "started");

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("restart leaves legacy Started deletion unresolved");

        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("started target remains durable")[0]
                .status,
            "started"
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
    }

    #[tokio::test]
    async fn unattended_gc_never_repeats_a_started_delete_after_crash() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-delete-before-observe").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        let target = shell
            .list_provider_credential_slot_garbage()
            .expect("pending target")
            .pop()
            .expect("target");
        shell
            .observe_provider_credential_slot_garbage(
                &target.connection_id,
                target.authority_sequence,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("durable delete cutpoint");
        vault
            .delete_bound(&target.connection_id, native_authority(&key_a))
            .await
            .expect("native delete before simulated crash");
        let deletes_after_crash = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("restart cannot replay a SQLite-derived native effect");

        assert_eq!(vault.counts().3, deletes_after_crash);
        assert!(vault.bound_item_for(&key_a).is_none());
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("unattested started target remains durable")[0]
                .status,
            "started"
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
    }

    #[tokio::test]
    async fn unreadable_superseded_gc_stays_unresolved_without_native_delete() {
        let (_root, shell, vault, key_a, _item_a, key_b) =
            replacement_gc_fixture("gc-unreadable").await;
        vault.insert_bound_item(key_a.clone(), FakeItem::UnreadableSlot);

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("unreadable stale target is retained, never adopted");

        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::UnreadableSlot)
        ));
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        shell
            .ensure_provider_credential_access_settled("gc-unreadable")
            .expect("current B authority remains owned");
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("unreadable target remains durable")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn gc_observe_and_status_error_keeps_startup_and_current_authority_usable() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-observe-error").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.fail_next_bound_observation_and_status();
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("one stale backend error must not abort startup recovery");
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("stale target remains retryable")
                .len(),
            1
        );
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(
            read_provider_connection_credential_with(&vault, &shell, "gc-observe-error")
                .await
                .expect("current B remains usable")
                .credential
                .expect("current secret")
                .into_secret_string(),
            "replacement-secret"
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("later retry remains observe-only for present stale A");
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("retry remains unresolved")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn unattended_gc_never_enters_the_post_delete_retry_path() {
        let (_root, shell, vault, key_a, item_a, key_b) =
            replacement_gc_fixture("gc-post-delete-observe-error").await;
        vault.insert_bound_item(key_a.clone(), item_a);
        vault.fail_post_delete_observation_and_status();
        let deletes_before = vault.counts().3;

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("no native delete means no post-delete observation");
        assert!(matches!(
            vault.bound_item_for(&key_a),
            Some(FakeItem::Bound { .. })
        ));
        let unresolved = shell
            .list_provider_credential_slot_garbage()
            .expect("present target remains retryable");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "pending");
        assert_eq!(vault.counts().3, deletes_before);
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        read_provider_connection_credential_with(&vault, &shell, "gc-post-delete-observe-error")
            .await
            .expect("current B remains usable after stale A postflight error");

        recover_provider_credential_slot_garbage_with(&vault, &shell)
            .await
            .expect("later retry still cannot gain deletion authority");
        assert_eq!(vault.counts().3, deletes_before);
        assert_eq!(
            shell
                .list_provider_credential_slot_garbage()
                .expect("retry remains unresolved")[0]
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn more_than_twenty_replacement_and_remove_cycles_keep_bound_slots_bounded() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "bounded-slot-cycles");
        let vault = FakeVault::new(shell.clone(), FakeItem::Raw, "rotating-secret");

        capture_provider_connection_credential_with(&vault, &shell, "bounded-slot-cycles")
            .await
            .expect("initial authority install");
        for cycle in 0..21 {
            if cycle % 2 == 0 {
                capture_provider_connection_credential_with(&vault, &shell, "bounded-slot-cycles")
                    .await
                    .expect("replacement rotates through predecessor deletion");
            } else {
                remove_provider_credential_with(&vault, &shell, "bounded-slot-cycles", false)
                    .await
                    .expect("explicit remove deletes the exact current slot");
                assert_eq!(vault.bound_slot_count(), 0);
                capture_provider_connection_credential_with(&vault, &shell, "bounded-slot-cycles")
                    .await
                    .expect("install after exact removal");
            }
            assert_eq!(
                vault.bound_slot_count(),
                1,
                "cycle {cycle} must retain only the current authority-derived slot"
            );
            assert_eq!(vault.bound_keys().len(), 1);
            assert!(matches!(vault.raw_item(), FakeItem::Raw));
        }
    }

    #[tokio::test]
    async fn capture_failure_terminalizes_prepared_install_and_allows_immediate_retry() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "capture-retry");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "retry-secret");
        vault.fail_next_capture();

        capture_provider_connection_credential_with(&vault, &shell, "capture-retry")
            .await
            .expect_err("synthetic clipboard capture fails before native mutation");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("failed capture journal")
                .is_empty(),
            "capture failure must settle its Prepared operation without a restart"
        );
        capture_provider_connection_credential_with(&vault, &shell, "capture-retry")
            .await
            .expect("immediate capture retry succeeds");
    }

    #[tokio::test]
    async fn prepared_store_failure_terminalizes_install_and_allows_immediate_retry() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "prepare-store-retry");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "retry-secret");
        vault.fail_next_prepare_store();

        let error =
            capture_provider_connection_credential_with(&vault, &shell, "prepare-store-retry")
                .await
                .expect_err("synthetic native store preparation fails after durable Prepared");
        assert_eq!(error.code, "storage_unavailable");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("failed preparation journal")
                .is_empty(),
            "prepare failure must settle its exact Prepared operation as no-effect"
        );
        assert!(matches!(vault.bound_item(), FakeItem::Missing));
        assert_eq!(vault.counts().2, 0, "no native store may start");

        capture_provider_connection_credential_with(&vault, &shell, "prepare-store-retry")
            .await
            .expect("immediate capture retry succeeds");
        shell
            .ensure_provider_credential_access_settled("prepare-store-retry")
            .expect("retry grants only its fresh durable authority");
    }

    #[tokio::test]
    async fn raw_logical_slot_appearing_during_capture_is_isolated_from_bound_install() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "capture-slot-race");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "captured-secret");
        vault.create_raw_slot_after_capture();

        capture_provider_connection_credential_with(&vault, &shell, "capture-slot-race")
            .await
            .expect("the authority-derived bound slot is independent of the raw logical slot");
        assert!(matches!(vault.item(), FakeItem::Raw));
        assert!(matches!(vault.bound_item(), FakeItem::Bound { .. }));
        assert_eq!(vault.counts().2, 1, "only the derived bound slot is stored");
        shell
            .ensure_provider_credential_access_settled("capture-slot-race")
            .expect("raw logical slot is never adopted as the bound credential");
    }

    #[tokio::test]
    async fn exact_postflight_wins_over_mutate_then_error_for_store_and_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "postflight-wins");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "stored-secret");
        vault.fail_store_after_mutation();
        capture_provider_connection_credential_with(&vault, &shell, "postflight-wins")
            .await
            .expect("matching bound postflight confirms store despite response loss");
        shell
            .ensure_provider_credential_access_settled("postflight-wins")
            .expect("store postflight owns exact authority");

        vault.fail_delete_after_mutation();
        remove_provider_credential_with(&vault, &shell, "postflight-wins", false)
            .await
            .expect("missing postflight confirms delete despite response loss");
        assert!(matches!(vault.item(), FakeItem::Missing));
        shell
            .ensure_provider_credential_access_settled("postflight-wins")
            .expect_err("confirmed delete revokes credential authority");
    }

    #[tokio::test]
    async fn recovery_required_store_never_adopts_visible_credential() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "durability-unknown-store");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "stored-secret");
        vault.require_recovery_after_store_mutation();

        let error =
            capture_provider_connection_credential_with(&vault, &shell, "durability-unknown-store")
                .await
                .expect_err("visible Match cannot override explicit recovery-required");
        assert_eq!(error.code, "credential_recovery_required");
        assert!(matches!(vault.bound_item(), FakeItem::Bound { .. }));
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("durability-unknown install remains journaled");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        shell
            .ensure_provider_credential_access_settled("durability-unknown-store")
            .expect_err("visible credential with unknown durability is never adopted");

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("bootstrap keeps the explicit recovery barrier");
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("recovery barrier survives bootstrap")[0]
                .status,
            "cleanup_required"
        );
    }

    #[tokio::test]
    async fn recovery_required_delete_never_accepts_visible_missing_as_durable_success() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "durability-unknown-delete");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "stored-secret");
        capture_provider_connection_credential_with(&vault, &shell, "durability-unknown-delete")
            .await
            .expect("install credential");
        vault.require_recovery_after_delete_mutation();

        let error =
            remove_provider_credential_with(&vault, &shell, "durability-unknown-delete", false)
                .await
                .expect_err("visible Missing cannot override explicit recovery-required");
        assert_eq!(error.code, "credential_recovery_required");
        assert!(matches!(vault.bound_item(), FakeItem::Missing));
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("durability-unknown removal remains journaled");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("bootstrap keeps the explicit recovery barrier");
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("delete recovery barrier survives bootstrap")[0]
                .status,
            "cleanup_required"
        );
        assert_eq!(vault.counts().3, 1, "the uncertain delete ran once");

        vault.fail_delete_before_mutation();
        let retry_error =
            remove_provider_credential_with(&vault, &shell, "durability-unknown-delete", false)
                .await
                .expect_err("a failed exact durability retry cannot be cleared by Missing");
        assert_eq!(retry_error.code, "storage_unavailable");
        let still_blocked = shell
            .list_unresolved_provider_credential_operations()
            .expect("failed repair remains journaled");
        assert_eq!(still_blocked.len(), 1);
        assert!(still_blocked[0].operation_slot_recovery_required);

        remove_provider_credential_with(&vault, &shell, "durability-unknown-delete", false)
            .await
            .expect("explicit cleanup retries the exact durability boundary");
        assert_eq!(
            vault.counts().3,
            3,
            "Missing visibility must not skip either failed or successful native repair retries"
        );
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("successful repair settles the barrier")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn replacement_predecessor_recovery_required_preserves_b_until_exact_cleanup_retry() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let connection_id = "replacement-predecessor-durability";
        create_credential_connection(&shell, connection_id);
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("owned predecessor A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        let stores_before_b = vault.counts().2;
        vault.replace_capture_secret("secret-b");
        vault.require_recovery_after_delete_mutation();

        let error = capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect_err("uncertain predecessor delete must leave replacement journaled");
        assert_eq!(error.code, "credential_recovery_required");
        assert!(vault.bound_item_for(&key_a).is_none());
        assert_eq!(
            vault.counts().2,
            stores_before_b + 1,
            "verified B must exist before predecessor cleanup starts"
        );
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("replacement cleanup remains durable");
        assert_eq!(unresolved.len(), 1);
        let key_b = FakeAuthorityKey::from_authority(
            &operation_authority(&unresolved[0]).expect("successor B authority"),
        );
        assert!(matches!(
            vault.bound_item_for(&key_b),
            Some(FakeItem::Bound { .. })
        ));
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert!(unresolved[0].predecessor_slot_recovery_required);
        assert!(!unresolved[0].operation_slot_recovery_required);
        assert_eq!(
            unresolved[0].outcome_code.as_deref(),
            Some("native_predecessor_durability_unknown")
        );

        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("bootstrap preserves predecessor durability barrier");
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("replacement barrier survives bootstrap")[0]
                .status,
            "cleanup_required"
        );
        let deletes_before_repair = vault.counts().3;
        let predecessor_deletes_before = vault
            .events()
            .into_iter()
            .filter(|event| event == &FakeVaultEvent::Delete(key_a.clone()))
            .count();
        vault.fail_delete_before_mutation();
        let retry_error = remove_provider_credential_with(&vault, &shell, connection_id, false)
            .await
            .expect_err("failed predecessor repair cannot be cleared by Missing visibility");
        assert_eq!(retry_error.code, "storage_unavailable");
        let still_blocked = shell
            .list_unresolved_provider_credential_operations()
            .expect("failed predecessor repair remains journaled");
        assert_eq!(still_blocked.len(), 1);
        assert!(still_blocked[0].predecessor_slot_recovery_required);
        assert_eq!(vault.counts().2, stores_before_b + 1);

        remove_provider_credential_with(&vault, &shell, connection_id, false)
            .await
            .expect("explicit cleanup repeats exact predecessor delete boundary");
        assert_eq!(vault.counts().3, deletes_before_repair + 3);
        assert_eq!(
            vault
                .events()
                .into_iter()
                .filter(|event| event == &FakeVaultEvent::Delete(key_a.clone()))
                .count(),
            predecessor_deletes_before + 2,
            "explicit retry repairs predecessor A rather than unrelated B"
        );
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("replacement cleanup terminal")
                .is_empty()
        );
        assert_eq!(vault.counts().2, stores_before_b + 1);
        assert!(vault.bound_item_for(&key_b).is_none());
    }

    #[tokio::test]
    async fn archive_postflight_wins_over_native_delete_response_loss_and_reopens_settled() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "archive-postflight-wins");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "archive-secret");
        capture_provider_connection_credential_with(&vault, &shell, "archive-postflight-wins")
            .await
            .expect("install archive credential");
        vault.fail_delete_after_mutation();

        remove_provider_credential_with(&vault, &shell, "archive-postflight-wins", true)
            .await
            .expect("missing postflight atomically confirms archive despite response loss");
        assert!(matches!(vault.item(), FakeItem::Missing));
        assert!(
            shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "archive-postflight-wins")
        );
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen archived root");
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("reopened archive journal")
                .is_empty()
        );
        assert!(
            reopened
                .list_provider_connections()
                .expect("reopened active connections")
                .iter()
                .all(|connection| connection.id != "archive-postflight-wins")
        );
    }

    #[tokio::test]
    async fn uncertain_cleanup_archive_postflight_wins_over_delete_response_loss() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "uncertain-archive-response-loss");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "archive-secret");
        capture_provider_connection_credential_with(
            &vault,
            &shell,
            "uncertain-archive-response-loss",
        )
        .await
        .expect("install archive credential");
        let prepared = shell
            .prepare_provider_credential_operation(
                "uncertain-archive-response-loss",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare an ordinary removal before the archive request");
        shell
            .finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record uncertain pre-effect observation");
        vault.fail_delete_after_mutation();

        remove_provider_credential_with(&vault, &shell, "uncertain-archive-response-loss", true)
            .await
            .expect("truthful missing postflight completes the durable cleanup archive intent");
        assert!(matches!(vault.item(), FakeItem::Missing));
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("cleanup archive journal")
                .is_empty()
        );
        assert!(
            shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "uncertain-archive-response-loss")
        );
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen cleanup archive root");
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("reopened cleanup journal")
                .is_empty()
        );
        assert!(
            reopened
                .list_provider_connections()
                .expect("reopened active connections")
                .iter()
                .all(|connection| connection.id != "uncertain-archive-response-loss")
        );
    }

    #[tokio::test]
    async fn delete_that_leaves_the_slot_available_never_reports_success() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "delete-no-effect");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "retained-secret");
        capture_provider_connection_credential_with(&vault, &shell, "delete-no-effect")
            .await
            .expect("install credential");
        vault.preserve_item_on_delete();

        remove_provider_credential_with(&vault, &shell, "delete-no-effect", false)
            .await
            .expect_err("available postflight means explicit delete did not succeed");
        assert!(!matches!(vault.item(), FakeItem::Missing));
    }

    #[tokio::test]
    async fn unreadable_slot_can_be_explicitly_deleted_then_reinstalled() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "unreadable-delete");
        let vault = FakeVault::new(
            shell.clone(),
            FakeItem::UnreadableSlot,
            "replacement-secret",
        );

        remove_provider_credential_with(&vault, &shell, "unreadable-delete", false)
            .await
            .expect("explicit journaled delete may clear an unreadable native item");
        assert!(matches!(vault.item(), FakeItem::Missing));
        capture_provider_connection_credential_with(&vault, &shell, "unreadable-delete")
            .await
            .expect("cleared unreadable slot can be reinstalled");
    }

    #[tokio::test]
    async fn prior_owned_observe_error_with_unreadable_status_can_remove_or_archive() {
        for archive in [false, true] {
            let root = tempdir().expect("root");
            let shell = ShellApi::open_data_root(root.path()).expect("shell");
            let connection_id = if archive {
                "owned-observe-error-archive"
            } else {
                "owned-observe-error-remove"
            };
            create_credential_connection(&shell, connection_id);
            let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "owned-secret");
            capture_provider_connection_credential_with(&vault, &shell, connection_id)
                .await
                .expect("install prior A");
            let prior_item = vault.bound_item();
            vault.replace_item(FakeItem::UnreadableSlot);
            vault.fail_next_bound_observation();

            remove_provider_credential_with(&vault, &shell, connection_id, archive)
                .await
                .expect("status fallback journals and deletes exact unreadable A");

            assert_eq!(vault.counts().3, 1);
            assert!(matches!(vault.bound_item(), FakeItem::Missing));
            assert!(
                shell
                    .list_unresolved_provider_credential_operations()
                    .expect("cleanup terminalized")
                    .is_empty()
            );
            assert_eq!(
                shell
                    .list_provider_connections()
                    .expect("active connections")
                    .iter()
                    .any(|connection| connection.id == connection_id),
                !archive
            );
            drop(vault);
            drop(shell);

            let reopened = ShellApi::open_data_root(root.path()).expect("reopen cleanup root");
            assert!(
                reopened
                    .list_unresolved_provider_credential_operations()
                    .expect("reopened cleanup terminal")
                    .is_empty()
            );
            let restored = FakeVault::new(reopened.clone(), prior_item, "must-not-capture");
            read_provider_connection_credential_with(&restored, &reopened, connection_id)
                .await
                .expect_err("restored prior A remains unauthorized");
        }
    }

    #[tokio::test]
    async fn prior_owned_unreadable_delete_failure_is_not_reported_as_success() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "owned-unreadable-delete-failure");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "owned-secret");
        capture_provider_connection_credential_with(
            &vault,
            &shell,
            "owned-unreadable-delete-failure",
        )
        .await
        .expect("install prior A");
        vault.replace_item(FakeItem::UnreadableSlot);
        vault.fail_next_bound_observation();
        vault.fail_delete_before_mutation();

        let error = remove_provider_credential_with(
            &vault,
            &shell,
            "owned-unreadable-delete-failure",
            false,
        )
        .await
        .expect_err("failed native delete must remain visible");
        assert_eq!(error.code, "storage_unavailable");
        assert!(matches!(vault.bound_item(), FakeItem::UnreadableSlot));
        assert_eq!(vault.counts().3, 1);
        assert_eq!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("retryable cleanup intent")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn unreadable_uncertain_install_cleanup_can_remove_or_archive_without_reappearing() {
        for archive in [false, true] {
            let root = tempdir().expect("root");
            let shell = ShellApi::open_data_root(root.path()).expect("shell");
            let connection_id = if archive {
                "unreadable-install-archive"
            } else {
                "unreadable-install-remove"
            };
            create_credential_connection(&shell, connection_id);
            let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "unused-secret");
            let prepared = prepare_authority_bound_install(&shell, connection_id);
            shell
                .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
                .expect("start install before unreadable outcome");
            vault.replace_item(FakeItem::UnreadableSlot);
            let uncertain = shell
                .finish_provider_credential_operation(
                    &prepared.operation_id,
                    &prepared.plan_sha256,
                    ProviderCredentialSlotStatusInput::Unreadable,
                )
                .expect("record unreadable install outcome");
            assert_eq!(uncertain.status, "outcome_unknown");

            remove_provider_credential_with(&vault, &shell, connection_id, archive)
                .await
                .expect("explicit cleanup settles the original unreadable install");
            assert!(matches!(vault.item(), FakeItem::Missing));
            assert!(
                shell
                    .list_unresolved_provider_credential_operations()
                    .expect("settled cleanup journal")
                    .is_empty()
            );
            let remains_active = shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .any(|connection| connection.id == connection_id);
            assert_eq!(remains_active, !archive);
            if !archive {
                shell
                    .ensure_provider_credential_access_settled(connection_id)
                    .expect_err("explicit cleanup revokes any prior authority");
            }
            drop(vault);
            drop(shell);

            let reopened = ShellApi::open_data_root(root.path()).expect("reopen cleanup root");
            assert!(
                reopened
                    .list_unresolved_provider_credential_operations()
                    .expect("reopened cleanup journal")
                    .is_empty()
            );
            let remains_active = reopened
                .list_provider_connections()
                .expect("reopened active connections")
                .iter()
                .any(|connection| connection.id == connection_id);
            assert_eq!(remains_active, !archive);
        }
    }

    #[tokio::test]
    async fn stale_or_malformed_marker_is_blocked_but_explicit_delete_allows_reinstall() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "marker-reset");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault, &shell, "marker-reset")
            .await
            .expect("install A");
        vault.replace_item(FakeItem::Bound {
            authority_id: "newer-install-b".to_owned(),
            binding_sha256: "b".repeat(64),
            secret: "secret-b".to_owned(),
        });
        read_provider_connection_credential_with(&vault, &shell, "marker-reset")
            .await
            .expect_err("mismatched envelope in the exact authority slot fails closed");
        remove_provider_credential_with(&vault, &shell, "marker-reset", false)
            .await
            .expect("explicit mismatch deletion");
        capture_provider_connection_credential_with(&vault, &shell, "marker-reset")
            .await
            .expect("fresh install after explicit deletion");

        vault.replace_item(FakeItem::MalformedEnvelope);
        remove_provider_credential_with(&vault, &shell, "marker-reset", false)
            .await
            .expect("explicit malformed-envelope deletion");
        assert_eq!(vault.counts().3, 2);
    }

    #[tokio::test]
    async fn archive_first_blocks_then_forces_background_credential_read_to_fail_closed() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "archive-first-read");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "leased-secret");
        capture_provider_connection_credential_with(&vault, &shell, "archive-first-read")
            .await
            .expect("install credential");

        let operation_lock = Arc::new(tokio::sync::Mutex::new(()));
        let archive_guard = Arc::clone(&operation_lock).lock_owned().await;
        shell
            .prepare_provider_credential_operation(
                "archive-first-read",
                ProviderCredentialOperationKindInput::RemoveForArchive,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare archive before releasing operation lock");

        let read = {
            let operation_lock = Arc::clone(&operation_lock);
            let vault = vault.clone();
            let shell = shell.clone();
            tokio::spawn(async move {
                let _lease = operation_lock.lock_owned().await;
                read_provider_connection_credential_with(&vault, &shell, "archive-first-read").await
            })
        };
        tokio::task::yield_now().await;
        assert!(
            !read.is_finished(),
            "reader must wait behind archive intent"
        );
        drop(archive_guard);
        assert!(
            read.await.expect("credential reader task").is_err(),
            "the settled-access gate must reject an archive-first credential read"
        );
    }

    #[tokio::test]
    async fn restart_recovery_fences_exact_marker_without_observing_or_repeating_store() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "restart-install");
        let prepared = prepare_authority_bound_install(&shell, "restart-install");
        let started = shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("start");
        let vault = FakeVault::new(
            shell.clone(),
            FakeItem::Bound {
                authority_id: started.operation_id.clone(),
                binding_sha256: started.connection_binding_sha256.clone(),
                secret: "restart-secret".to_owned(),
            },
            "must-not-capture",
        );
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("recover exact marker");
        assert_eq!(vault.counts(), (0, 0, 0, 0));
        shell
            .ensure_provider_credential_access_settled("restart-install")
            .expect_err("bare Started visibility is never adopted after restart");
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("load startup fence");
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].operation_slot_recovery_required);
    }

    #[tokio::test]
    async fn explicit_cleanup_fences_bare_started_before_missing_slot_retry() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "same-process-started-cleanup");
        let prepared = prepare_authority_bound_install(&shell, "same-process-started-cleanup");
        shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("persist Started before simulated post-native interruption");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "must-not-capture");
        vault.fail_delete_before_mutation();

        let error =
            remove_provider_credential_with(&vault, &shell, "same-process-started-cleanup", false)
                .await
                .expect_err("failed exact repair must preserve the same-process Started fence");
        assert_eq!(error.code, "storage_unavailable");
        assert_eq!(vault.counts().3, 1);
        let blocked = shell
            .list_unresolved_provider_credential_operations()
            .expect("load explicit cleanup fence");
        assert_eq!(blocked.len(), 1);
        assert_eq!(blocked[0].status, "cleanup_required");
        assert!(blocked[0].operation_slot_recovery_required);
        shell
            .ensure_provider_credential_access_settled("same-process-started-cleanup")
            .expect_err("bare Started remains inaccessible until an exact successful retry");

        remove_provider_credential_with(&vault, &shell, "same-process-started-cleanup", false)
            .await
            .expect("successful Missing-slot retry repairs the durability boundary");
        assert_eq!(vault.counts().3, 2);
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("successful repair settles same-process Started")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn persistent_mismatch_can_be_explicitly_cleaned_and_reinstalled_after_reopen() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "restart-mismatch");
        let prepared = prepare_authority_bound_install(&shell, "restart-mismatch");
        shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("start");
        let vault = FakeVault::new(
            shell.clone(),
            FakeItem::Bound {
                authority_id: "different-install".to_owned(),
                binding_sha256: "b".repeat(64),
                secret: "unowned-secret".to_owned(),
            },
            "must-not-capture",
        );
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("first mismatch recovery");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("second mismatch recovery is a no-op");
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("persistent mismatch remains restart-visible");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert!(unresolved[0].operation_slot_recovery_required);
        shell
            .ensure_provider_credential_access_settled("restart-mismatch")
            .expect_err("persistent mismatch remains use-blocking");
        remove_provider_credential_with(&vault, &shell, "restart-mismatch", false)
            .await
            .expect("explicit delete continues the same uncertain authority");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("cleanup terminalized")
                .is_empty()
        );
        assert!(matches!(vault.item(), FakeItem::Missing));
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen after cleanup");
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("reopened cleanup is settled")
                .is_empty()
        );
        let reinstall_vault = FakeVault::new(reopened.clone(), FakeItem::Missing, "fresh-secret");
        capture_provider_connection_credential_with(
            &reinstall_vault,
            &reopened,
            "restart-mismatch",
        )
        .await
        .expect("fresh install is allowed after explicit cleanup");
    }

    #[tokio::test]
    async fn cleanup_intent_survives_restart_before_delete_without_reenabling_credential() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "cleanup-crash");
        let prepared = prepare_authority_bound_install(&shell, "cleanup-crash");
        shell
            .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
            .expect("start install");
        let vault = FakeVault::new(
            shell.clone(),
            FakeItem::Bound {
                authority_id: "different-install".to_owned(),
                binding_sha256: "b".repeat(64),
                secret: "unowned-secret".to_owned(),
            },
            "must-not-capture",
        );
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("classify mismatched envelope as uncertain");
        let marked = shell
            .list_unresolved_provider_credential_operations()
            .expect("load fenced cleanup intent")
            .into_iter()
            .find(|operation| operation.operation_id == prepared.operation_id)
            .expect("startup fence persists the exact cleanup intent before native delete");
        assert_eq!(marked.status, "cleanup_required");
        assert!(marked.operation_slot_recovery_required);

        let retained_item = vault.item();
        drop(vault);
        drop(shell);
        let reopened = ShellApi::open_data_root(root.path()).expect("restart after cleanup mark");
        let vault = FakeVault::new(reopened.clone(), retained_item, "must-not-capture");

        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("first restart preserves pending cleanup intent");
        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("second restart preserves pending cleanup intent");
        let unresolved = reopened
            .list_unresolved_provider_credential_operations()
            .expect("cleanup remains restart-visible");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert_eq!(
            vault.counts().3,
            0,
            "bootstrap must not replay native delete"
        );
        reopened
            .ensure_provider_credential_access_settled("cleanup-crash")
            .expect_err("cleanup intent must not be reclassified as an owned install");

        remove_provider_credential_with(&vault, &reopened, "cleanup-crash", false)
            .await
            .expect("explicit retry resumes and terminalizes the cleanup intent");
        assert_eq!(vault.counts().3, 1);
        assert!(
            reopened
                .list_unresolved_provider_credential_operations()
                .expect("cleanup terminalized")
                .is_empty()
        );
        assert!(matches!(vault.item(), FakeItem::Missing));
    }

    #[tokio::test]
    async fn uncertain_archive_explicit_retry_finishes_the_original_operation_once() {
        for already_cleanup_required in [false, true] {
            let root = tempdir().expect("root");
            let shell = ShellApi::open_data_root(root.path()).expect("shell");
            let connection_id = if already_cleanup_required {
                "cleanup-required-archive"
            } else {
                "outcome-unknown-archive"
            };
            create_credential_connection(&shell, connection_id);
            let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "archive-secret");
            capture_provider_connection_credential_with(&vault, &shell, connection_id)
                .await
                .expect("install credential before archive");

            let prepared = shell
                .prepare_provider_credential_operation(
                    connection_id,
                    ProviderCredentialOperationKindInput::RemoveForArchive,
                    ProviderCredentialSlotStatusInput::Available,
                )
                .expect("prepare archive removal");
            let started = shell
                .start_provider_credential_operation(&prepared.operation_id, &prepared.plan_sha256)
                .expect("start archive removal");
            let uncertain = shell
                .finish_provider_credential_operation(
                    &started.operation_id,
                    &started.plan_sha256,
                    ProviderCredentialSlotStatusInput::Unreadable,
                )
                .expect("record uncertain archive observation");
            assert_eq!(uncertain.status, "outcome_unknown");
            if already_cleanup_required {
                let cleanup = shell
                    .mark_provider_credential_cleanup_required(
                        &started.operation_id,
                        &started.plan_sha256,
                        ProviderCredentialSlotStatusInput::Available,
                        true,
                    )
                    .expect("persist cleanup intent before explicit retry");
                assert_eq!(cleanup.status, "cleanup_required");
            }

            remove_provider_credential_with(&vault, &shell, connection_id, true)
                .await
                .expect("same uncertain archive operation must complete the command");
            assert_eq!(vault.counts().3, 1, "native delete runs exactly once");
            assert!(
                shell
                    .list_unresolved_provider_credential_operations()
                    .expect("archive cleanup is terminal")
                    .is_empty()
            );
            assert!(
                shell
                    .list_provider_connections()
                    .expect("active connections")
                    .iter()
                    .all(|connection| connection.id != connection_id)
            );
            let terminal = shell
                .reconcile_provider_credential_archive(
                    &started.operation_id,
                    &started.plan_sha256,
                    ProviderCredentialSlotStatusInput::Missing,
                )
                .expect("original archive operation remains idempotently terminal");
            assert_eq!(terminal.status, "succeeded");

            drop(vault);
            drop(shell);
            let reopened = ShellApi::open_data_root(root.path()).expect("reopen archived root");
            assert!(
                reopened
                    .list_unresolved_provider_credential_operations()
                    .expect("reopened archive remains settled")
                    .is_empty()
            );
            assert!(
                reopened
                    .list_provider_connections()
                    .expect("reopened active connections")
                    .iter()
                    .all(|connection| connection.id != connection_id)
            );
        }
    }

    #[tokio::test]
    async fn unstarted_uncertain_remove_revokes_the_prior_envelope_after_explicit_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "unstarted-remove-cleanup");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "prior-secret");
        capture_provider_connection_credential_with(&vault, &shell, "unstarted-remove-cleanup")
            .await
            .expect("install prior owned envelope");
        let prior_envelope = vault.item();

        let prepared = shell
            .prepare_provider_credential_operation(
                "unstarted-remove-cleanup",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare removal without starting native effect");
        let uncertain = shell
            .finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record uncertain pre-effect observation");
        assert_eq!(uncertain.status, "outcome_unknown");
        assert!(!uncertain.native_effect_started);

        remove_provider_credential_with(&vault, &shell, "unstarted-remove-cleanup", false)
            .await
            .expect("explicit delete must durably revoke the prior authority");
        assert_eq!(vault.counts().3, 1);
        assert!(matches!(vault.item(), FakeItem::Missing));
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen after explicit delete");
        let restored = FakeVault::new(reopened.clone(), prior_envelope, "must-not-capture");
        read_provider_connection_credential_with(&restored, &reopened, "unstarted-remove-cleanup")
            .await
            .expect_err("restoring the explicitly deleted envelope must remain unauthorized");
    }

    #[tokio::test]
    async fn missing_uncertain_remove_still_revokes_the_prior_envelope() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "missing-remove-cleanup");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "prior-secret");
        capture_provider_connection_credential_with(&vault, &shell, "missing-remove-cleanup")
            .await
            .expect("install prior owned envelope");
        let prior_envelope = vault.item();
        let prepared = shell
            .prepare_provider_credential_operation(
                "missing-remove-cleanup",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare unstarted removal");
        shell
            .finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record uncertain removal");
        vault.replace_item(FakeItem::Missing);

        remove_provider_credential_with(&vault, &shell, "missing-remove-cleanup", false)
            .await
            .expect("missing explicit delete still records durable revocation");
        assert_eq!(vault.counts().3, 0, "missing cleanup has no native effect");
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("reopen after missing cleanup");
        let restored = FakeVault::new(reopened.clone(), prior_envelope, "must-not-capture");
        read_provider_connection_credential_with(&restored, &reopened, "missing-remove-cleanup")
            .await
            .expect_err("the prior envelope remains revoked after missing cleanup");
    }

    #[tokio::test]
    async fn archive_cleanup_intent_survives_restart_before_native_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "archive-cleanup-restart");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "archive-secret");
        capture_provider_connection_credential_with(&vault, &shell, "archive-cleanup-restart")
            .await
            .expect("install credential before archive cleanup");
        let prepared = shell
            .prepare_provider_credential_operation(
                "archive-cleanup-restart",
                ProviderCredentialOperationKindInput::RemoveCredential,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare an ordinary removal before archive request");
        shell
            .finish_provider_credential_operation(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record uncertain ordinary removal");
        let marked = shell
            .mark_provider_credential_cleanup_required(
                &prepared.operation_id,
                &prepared.plan_sha256,
                ProviderCredentialSlotStatusInput::Available,
                true,
            )
            .expect("persist archive disposition before native delete");
        assert!(marked.cleanup_archives_connection);
        let retained_item = vault.item();
        drop(vault);
        drop(shell);

        let reopened = ShellApi::open_data_root(root.path()).expect("restart before native delete");
        let vault = FakeVault::new(reopened.clone(), retained_item, "must-not-capture");
        recover_provider_credential_operations_with(&vault, &reopened)
            .await
            .expect("bootstrap preserves archive cleanup disposition");
        let unresolved = reopened
            .list_unresolved_provider_credential_operations()
            .expect("archive cleanup remains visible");
        assert_eq!(unresolved.len(), 1);
        assert!(unresolved[0].cleanup_archives_connection);
        assert_eq!(vault.counts().3, 0, "bootstrap never replays native delete");

        remove_provider_credential_with(&vault, &reopened, "archive-cleanup-restart", true)
            .await
            .expect("explicit retry completes the persisted archive disposition");
        assert_eq!(vault.counts().3, 1);
        assert!(
            reopened
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "archive-cleanup-restart")
        );
        drop(vault);
        drop(reopened);
        let final_reopen = ShellApi::open_data_root(root.path()).expect("reopen archived root");
        assert!(
            final_reopen
                .list_unresolved_provider_credential_operations()
                .expect("archive cleanup terminal")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn prepared_available_archive_missing_closes_without_blocking_bootstrap() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "prepared-archive-drift");
        shell
            .prepare_provider_credential_operation(
                "prepared-archive-drift",
                ProviderCredentialOperationKindInput::RemoveForArchive,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("prepare archive from available slot");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "must-not-capture");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("prepared drift is conservatively classified");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("repeated bootstrap remains idempotent");
        let unresolved = shell
            .list_unresolved_provider_credential_operations()
            .expect("list reconciled archive");
        assert!(unresolved.is_empty());
        assert!(
            shell
                .list_provider_connections()
                .expect("connection remains active")
                .iter()
                .any(|connection| connection.id == "prepared-archive-drift")
        );
        assert_eq!(vault.counts().3, 0, "recovery must not issue delete");
    }

    #[tokio::test]
    async fn unstarted_uncertain_archive_then_missing_never_blocks_later_bootstrap() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "unstarted-archive-unknown");
        shell
            .prepare_provider_credential_operation(
                "unstarted-archive-unknown",
                ProviderCredentialOperationKindInput::RemoveForArchive,
                ProviderCredentialSlotStatusInput::Unreadable,
            )
            .expect("record unreadable archive preflight");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "must-not-capture");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("later missing slot safely closes unstarted uncertainty");
        recover_provider_credential_operations_with(&vault, &shell)
            .await
            .expect("next bootstrap sees no unresolved replay");
        assert!(
            shell
                .list_unresolved_provider_credential_operations()
                .expect("uncertain archive settled")
                .is_empty()
        );
        assert!(
            shell
                .list_provider_connections()
                .expect("connection remains active after no-effect")
                .iter()
                .any(|connection| connection.id == "unstarted-archive-unknown")
        );
        assert_eq!(vault.counts().3, 0);
    }

    #[tokio::test]
    async fn missing_archive_preflight_never_issues_native_delete() {
        let root = tempdir().expect("root");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        create_credential_connection(&shell, "archive-already-missing");
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "must-not-capture");
        remove_provider_credential_with(&vault, &shell, "archive-already-missing", true)
            .await
            .expect("archive missing slot atomically as native no-effect");
        assert_eq!(vault.counts().3, 0);
        assert!(
            shell
                .list_provider_connections()
                .expect("active connections")
                .iter()
                .all(|connection| connection.id != "archive-already-missing")
        );
    }

    #[tokio::test]
    async fn restored_database_snapshot_rejects_newer_vault_marker_at_every_shared_read_boundary() {
        let root = tempdir().expect("root");
        let snapshot = tempdir().expect("snapshot");
        let shell_a = ShellApi::open_data_root(root.path()).expect("shell A");
        create_credential_connection(&shell_a, "rollback-marker");
        let vault_a = FakeVault::new(shell_a.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault_a, &shell_a, "rollback-marker")
            .await
            .expect("install A");
        let item_a = vault_a.item();
        drop(vault_a);
        drop(shell_a);
        copy_tree(root.path(), snapshot.path());

        let shell_b = ShellApi::open_data_root(root.path()).expect("shell B");
        let vault_b = FakeVault::new(shell_b.clone(), item_a, "secret-b");
        remove_provider_credential_with(&vault_b, &shell_b, "rollback-marker", false)
            .await
            .expect("remove A");
        capture_provider_connection_credential_with(&vault_b, &shell_b, "rollback-marker")
            .await
            .expect("install B");
        let item_b = vault_b.item();
        drop(vault_b);
        drop(shell_b);

        fs::remove_dir_all(root.path()).expect("remove newer temporary DB root");
        fs::create_dir(root.path()).expect("recreate rollback root");
        copy_tree(snapshot.path(), root.path());
        let restored_a = ShellApi::open_data_root(root.path()).expect("restore shell A snapshot");
        let mismatched_vault = FakeVault::new(restored_a.clone(), item_b, "unused");

        for sink in ["generation", "model_sync", "background_task"] {
            let error = super::read_provider_connection_credential_with(
                &mismatched_vault,
                &restored_a,
                "rollback-marker",
            )
            .await
            .expect_err("newer vault marker must not be released under restored DB A");
            assert_eq!(
                error.code, "credential_recovery_required",
                "{sink} sink must fail closed through the shared native read boundary"
            );
        }
    }

    #[tokio::test]
    async fn restored_started_replacement_with_available_b_does_not_break_bootstrap_or_adopt_b() {
        let root = tempdir().expect("root");
        let snapshot = tempdir().expect("snapshot");
        let shell = ShellApi::open_data_root(root.path()).expect("shell");
        let connection_id = "rollback-started-replacement";
        create_credential_connection(&shell, connection_id);
        let vault = FakeVault::new(shell.clone(), FakeItem::Missing, "secret-a");
        capture_provider_connection_credential_with(&vault, &shell, connection_id)
            .await
            .expect("install predecessor A");
        let authority_a = shell
            .ensure_provider_credential_access_settled(connection_id)
            .expect("authority A");
        let key_a = FakeAuthorityKey {
            authority_id: authority_a.authority_id,
            binding_sha256: authority_a.connection_binding_sha256,
        };
        let item_a = vault
            .bound_item_for(&key_a)
            .expect("predecessor A envelope");
        let prepared_b = prepare_authority_bound_install(&shell, connection_id);
        let started_b = shell
            .start_provider_credential_operation(&prepared_b.operation_id, &prepared_b.plan_sha256)
            .expect("start replacement B before snapshot");
        drop(vault);
        drop(shell);
        copy_tree(root.path(), snapshot.path());

        let newer_shell = ShellApi::open_data_root(root.path()).expect("newer shell");
        let newer_vault = FakeVault::new(newer_shell.clone(), FakeItem::Missing, "unused");
        newer_vault.insert_bound_item(key_a.clone(), item_a);
        newer_shell
            .attest_provider_credential_predecessor_delete_intent(
                &started_b.operation_id,
                &started_b.plan_sha256,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("record predecessor delete intent");
        newer_vault
            .delete_bound(connection_id, native_authority(&key_a))
            .await
            .expect("delete predecessor A");
        newer_shell
            .attest_provider_credential_predecessor_missing(
                &started_b.operation_id,
                &started_b.plan_sha256,
            )
            .expect("record predecessor A missing");
        let authority_b = super::operation_authority(&started_b).expect("authority B");
        let key_b = FakeAuthorityKey::from_authority(&authority_b);
        let prepared_store = newer_vault
            .prepare_bound_store(
                connection_id,
                NativeCredential::new("secret-b".to_owned()),
                &authority_b,
            )
            .expect("prepare B store");
        newer_vault
            .store_prepared(prepared_store)
            .await
            .expect("store replacement B");
        newer_shell
            .finish_provider_credential_operation(
                &started_b.operation_id,
                &started_b.plan_sha256,
                ProviderCredentialSlotStatusInput::Available,
            )
            .expect("newer database adopts B with complete evidence");
        let item_b = newer_vault
            .bound_item_for(&key_b)
            .expect("replacement B envelope survives rollback");
        drop(newer_vault);
        drop(newer_shell);

        fs::remove_dir_all(root.path()).expect("remove newer database root");
        fs::create_dir(root.path()).expect("recreate rollback root");
        copy_tree(snapshot.path(), root.path());
        let restored = ShellApi::open_data_root(root.path()).expect("restore Started snapshot");
        let restored_vault = FakeVault::new(restored.clone(), item_b, "must-not-capture");
        recover_provider_credential_operations_with(&restored_vault, &restored)
            .await
            .expect("rollback bootstrap settles B fail closed");
        recover_provider_credential_operations_with(&restored_vault, &restored)
            .await
            .expect("subsequent bootstrap is idempotent");
        let unresolved = restored
            .list_unresolved_provider_credential_operations()
            .expect("durable rollback recovery state");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].status, "cleanup_required");
        assert!(unresolved[0].operation_slot_recovery_required);
        restored
            .ensure_provider_credential_access_settled(connection_id)
            .expect_err("restored snapshot must not adopt surviving B");
        assert_eq!(restored_vault.counts().2, 0, "recovery never replays store");
    }

    fn create_credential_connection(shell: &ShellApi, id: &str) {
        let template = shell
            .list_provider_templates()
            .expect("templates")
            .into_iter()
            .find(|template| {
                template.credential_required
                    && template.default_network_mode == "public"
                    && template.default_api_origin.is_some()
            })
            .expect("credential template");
        let origin = template.default_api_origin.expect("origin");
        shell
            .create_provider_connection(CreateProviderConnectionInput {
                id: id.to_owned(),
                template_id: template.id,
                template_version: template.manifest_version,
                display_name: format!("Synthetic {id}"),
                api_origin: origin.clone(),
                api_base_path: None,
                network_mode: ProviderNetworkModeInput::Public,
                local_network_approval: None,
                values: Vec::new(),
                approved_credential_origin: Some(origin),
                timeout_seconds: 30,
            })
            .expect("create connection");
    }

    fn prepare_authority_bound_install(
        shell: &ShellApi,
        connection_id: &str,
    ) -> lorepia_shell_api::ProviderCredentialOperationContext {
        let proposed = shell
            .propose_provider_credential_install_authority(connection_id)
            .expect("propose authority-bound install");
        shell
            .prepare_provider_credential_install_operation(
                connection_id,
                &proposed,
                ProviderCredentialSlotStatusInput::Missing,
            )
            .expect("prepare authority-bound install")
    }

    fn assert_tree_excludes(root: &Path, needles: &[&str]) {
        let mut stack = vec![root.to_path_buf()];
        while let Some(path) = stack.pop() {
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                if let Ok(entries) = fs::read_dir(path) {
                    stack.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
                }
                continue;
            }
            let Ok(bytes) = fs::read(path) else {
                continue;
            };
            for needle in needles {
                assert!(
                    !bytes
                        .windows(needle.len())
                        .any(|window| window == needle.as_bytes()),
                    "data root file contains forbidden secret material"
                );
            }
        }
    }

    fn copy_tree(source: &Path, destination: &Path) {
        fs::create_dir_all(destination).expect("create snapshot directory");
        for entry in fs::read_dir(source).expect("read snapshot source") {
            let entry = entry.expect("snapshot entry");
            let source_path = entry.path();
            let destination_path = destination.join(entry.file_name());
            if entry.file_type().expect("snapshot entry type").is_dir() {
                copy_tree(&source_path, &destination_path);
            } else {
                fs::copy(&source_path, &destination_path).expect("copy snapshot file");
            }
        }
    }
}
