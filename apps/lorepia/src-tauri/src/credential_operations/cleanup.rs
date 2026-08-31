use lorepia_shell_api::{
    ProviderCredentialOperationContext, ProviderCredentialOperationKindInput,
    ProviderCredentialSlotStatusInput, ShellApi,
};
use tauri_plugin_lorepia_platform::{PlatformErrorCode, PlatformResult};

use crate::error::{CommandError, CommandResult};

use super::{
    authority::{
        delete_operation_slot, observe_operation, observe_operation_slot,
        operation_optional_authority, operation_predecessor_authority,
    },
    types::CredentialVault,
};

pub(super) async fn remove_replacement_predecessor(
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
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum UncertainCredentialCleanup {
    NotApplicable,
    CredentialRemoved,
    ConnectionArchived,
}

#[derive(Clone, Copy)]
pub(super) enum CredentialDurabilityBarrier {
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

pub(super) async fn cleanup_uncertain_credential_for_explicit_delete(
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
pub(super) fn platform_result_requires_credential_recovery(result: &PlatformResult<()>) -> bool {
    matches!(
        result,
        Err(error) if error.code() == PlatformErrorCode::CredentialRecoveryRequired
    )
}

pub(super) fn persist_explicit_credential_recovery_barrier(
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
