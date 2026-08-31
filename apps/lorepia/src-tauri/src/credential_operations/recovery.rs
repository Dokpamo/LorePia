use lorepia_shell_api::{
    ProviderCredentialOperationKindInput, ProviderCredentialSlotStatusInput, ShellApi,
};
use tauri::AppHandle;
use tauri_plugin_lorepia_platform::{CredentialAuthority, CredentialStatus};

use crate::error::{CommandError, CommandResult};

use super::{
    authority::{
        credential_authority, observe_operation, observe_operation_slot,
        operation_optional_authority,
    },
    types::{CredentialVault, PlatformCredentialVault},
};

pub(crate) async fn recover_provider_credential_operations(
    app: &AppHandle,
    shell: &ShellApi,
) -> CommandResult<()> {
    let vault = PlatformCredentialVault { app };
    recover_provider_credential_operations_with(&vault, shell).await?;
    recover_provider_credential_slot_garbage_with(&vault, shell).await
}
pub(super) async fn recover_provider_credential_operations_with(
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
pub(super) async fn recover_provider_credential_slot_garbage_with(
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
