use lorepia_shell_api::{
    ProviderCredentialAccessAuthorityContext, ProviderCredentialOperationContext,
    ProviderCredentialSlotStatusInput, ShellApi,
};
use tauri_plugin_lorepia_platform::{
    BoundCredentialObservation, CredentialAuthority, CredentialStatus, PlatformResult,
};

use crate::error::{CommandError, CommandResult};

use super::types::CredentialVault;

pub(super) async fn observe_existing_credential(
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

pub(super) async fn observe_operation(
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

pub(super) fn operation_authority(
    operation: &ProviderCredentialOperationContext,
) -> CommandResult<CredentialAuthority> {
    operation_optional_authority(operation)?.ok_or_else(CommandError::internal)
}

pub(super) fn operation_optional_authority(
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

pub(super) fn operation_predecessor_authority(
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
pub(super) async fn observe_operation_slot(
    vault: &dyn CredentialVault,
    connection_id: &str,
    authority: Option<CredentialAuthority>,
) -> (ProviderCredentialSlotStatusInput, Option<CommandError>) {
    match authority {
        Some(authority) => observe_operation(vault, connection_id, authority).await,
        None => raw_observation(vault, connection_id).await,
    }
}

pub(super) async fn delete_operation_slot(
    vault: &dyn CredentialVault,
    connection_id: &str,
    authority: Option<CredentialAuthority>,
) -> PlatformResult<()> {
    match authority {
        Some(authority) => vault.delete_bound(connection_id, authority).await,
        None => vault.delete_raw(connection_id).await,
    }
}

pub(super) fn credential_authority(
    authority: &ProviderCredentialAccessAuthorityContext,
) -> CommandResult<CredentialAuthority> {
    CredentialAuthority::new(
        authority.authority_id.clone(),
        authority.connection_binding_sha256.clone(),
    )
    .map_err(Into::into)
}
