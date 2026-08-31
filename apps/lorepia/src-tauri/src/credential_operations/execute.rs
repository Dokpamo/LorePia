use lorepia_shell_api::{
    ProviderCredentialOperationContext, ProviderCredentialOperationKindInput,
    ProviderCredentialSlotStatusInput, ShellApi,
};
use tauri::AppHandle;
#[cfg(test)]
use tauri_plugin_lorepia_platform::CredentialStatus;
use tauri_plugin_lorepia_platform::{
    LegacyCredentialObservation, NativeCaptureStatus, NativeCredential, NativeCredentialEffect,
    NativeCredentialEffectConfirmation, PlatformError, PlatformErrorCode,
};

use crate::error::{CommandError, CommandResult};

use super::{
    authority::{
        credential_authority, delete_operation_slot, observe_existing_credential,
        observe_operation, observe_operation_slot, operation_authority,
        operation_optional_authority,
    },
    cleanup::{
        CredentialDurabilityBarrier, UncertainCredentialCleanup,
        cleanup_uncertain_credential_for_explicit_delete,
        persist_explicit_credential_recovery_barrier, platform_result_requires_credential_recovery,
        remove_replacement_predecessor,
    },
    prepare::{
        consume_connection_confirmation, consume_legacy_confirmation, ensure_bound_slot_missing,
        ensure_ordinary_connection_does_not_alias_legacy_raw_slot,
    },
    recovery::recover_provider_credential_slot_garbage_with,
    types::{
        CredentialVault, LegacyCredentialAccess, OrdinaryCredentialTargetPolicy,
        PlatformCredentialVault, ProviderConnectionCredentialRead,
    },
};

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
pub(crate) async fn read_provider_connection_credential(
    app: &AppHandle,
    shell: &ShellApi,
    connection_id: &str,
) -> CommandResult<ProviderConnectionCredentialRead> {
    read_provider_connection_credential_with(&PlatformCredentialVault { app }, shell, connection_id)
        .await
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
pub(super) async fn legacy_provider_credential_status_with(
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

pub(super) async fn capture_legacy_provider_credential_with(
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

pub(super) async fn delete_legacy_provider_credential_with(
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

pub(super) async fn read_legacy_provider_credential_with(
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

pub(super) async fn read_provider_connection_credential_with(
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
pub(super) async fn capture_provider_connection_credential_with(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
) -> CommandResult<NativeCaptureStatus> {
    capture_provider_connection_credential_with_policy(vault, shell, shell, connection_id).await
}

pub(super) async fn capture_provider_connection_credential_with_policy(
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

pub(super) async fn remove_provider_credential_with(
    vault: &dyn CredentialVault,
    shell: &ShellApi,
    connection_id: &str,
    archive: bool,
) -> CommandResult<()> {
    remove_provider_credential_with_policy(vault, shell, shell, connection_id, archive).await
}

pub(super) async fn remove_provider_credential_with_policy(
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
