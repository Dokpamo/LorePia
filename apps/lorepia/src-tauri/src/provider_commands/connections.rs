use lorepia_shell_api as shell;
use serde::Deserialize;
use tauri::{AppHandle, State};
use tauri_plugin_lorepia_platform::{LorepiaPlatformExt, NativeCredentialEffect};

use super::credentials::{NewConnectionSlotGuard, PlatformNewConnectionSlotGuard, find_connection};
use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProviderConnectionRequest {
    pub input: shell::CreateProviderConnectionInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProviderConnectionRequest {
    pub input: shell::UpdateProviderConnectionInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConnectionRequest {
    pub connection_id: String,
}

#[tauri::command]
pub fn list_provider_connections(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::ProviderConnectionDto>> {
    state
        .shell()?
        .list_provider_connections()
        .map_err(Into::into)
}

#[tauri::command]
pub async fn create_provider_connection(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CreateProviderConnectionRequest,
) -> CommandResult<shell::ProviderConnectionDto> {
    let shell = state.shell()?;
    let _operation = state.lock_provider_credential_operation().await;
    let CreateProviderConnectionRequest { input } = request;
    create_provider_connection_with_slot_guard(
        &shell,
        input,
        &PlatformNewConnectionSlotGuard { app: &app },
    )
    .await
}

pub(in crate::provider_commands) async fn create_provider_connection_with_slot_guard(
    shell: &shell::ShellApi,
    input: shell::CreateProviderConnectionInput,
    slot_guard: &dyn NewConnectionSlotGuard,
) -> CommandResult<shell::ProviderConnectionDto> {
    let connection_id = input.id.clone();
    if shell
        .list_provider_connections()?
        .iter()
        .any(|connection| connection.id == connection_id)
    {
        return Err(CommandError::invalid_input());
    }
    let template = shell
        .list_provider_templates()?
        .into_iter()
        .find(|template| {
            template.id == input.template_id && template.manifest_version == input.template_version
        })
        .ok_or_else(CommandError::invalid_input)?;
    let credential_binding_expected =
        template.credential_required || input.approved_credential_origin.is_some();
    if credential_binding_expected {
        slot_guard.ensure_missing(&connection_id).await?;
    }

    let connection = shell.create_provider_connection(input)?;
    if connection.id != connection_id {
        return Err(CommandError::internal());
    }
    if connection.credential_binding_required != credential_binding_expected {
        return Err(CommandError::internal());
    }
    Ok(connection)
}

#[tauri::command]
pub fn upsert_provider_connection(
    state: State<'_, AppState>,
    request: UpdateProviderConnectionRequest,
) -> CommandResult<shell::ProviderConnectionDto> {
    state
        .shell()?
        .upsert_provider_connection(request.input)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn delete_provider_connection(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ProviderConnectionRequest,
) -> CommandResult<()> {
    let shell = state.shell()?;
    let connection = find_connection(&shell, &request.connection_id)?;
    let legacy_raw =
        shell.provider_connection_uses_legacy_raw_credential(&request.connection_id)?;
    let confirmation = if connection.credential_binding_required {
        let confirmation = if legacy_raw {
            crate::commands::confirm_legacy_credential_effect(
                &app,
                &shell,
                &request.connection_id,
                NativeCredentialEffect::Archive,
            )
            .await?
        } else {
            let context =
                crate::credential_operations::provider_connection_credential_effect_context(
                    &shell,
                    &request.connection_id,
                    NativeCredentialEffect::Archive,
                )?;
            let confirmation = app
                .lorepia_platform()
                .confirm_credential_effect(context)
                .await?;
            let latest =
                crate::credential_operations::provider_connection_credential_effect_context(
                    &shell,
                    &request.connection_id,
                    NativeCredentialEffect::Archive,
                )?;
            if confirmation.context() != &latest {
                return Err(CommandError::invalid_input());
            }
            confirmation
        };
        if find_connection(&shell, &request.connection_id)? != connection
            || shell.provider_connection_uses_legacy_raw_credential(&request.connection_id)?
                != legacy_raw
        {
            return Err(CommandError::invalid_input());
        }
        Some(confirmation)
    } else {
        None
    };
    // Never let a renderer-triggered native modal hold either global
    // credential lock. Reacquire only after the one-use approval exists.
    let _legacy_archive_operation = if legacy_raw {
        Some(state.lock_legacy_provider_credential_archive().await)
    } else {
        None
    };
    let _operation = if legacy_raw {
        None
    } else {
        Some(state.lock_provider_credential_operation().await)
    };
    if find_connection(&shell, &request.connection_id)? != connection
        || shell.provider_connection_uses_legacy_raw_credential(&request.connection_id)?
            != legacy_raw
    {
        return Err(CommandError::invalid_input());
    }
    crate::credential_operations::archive_provider_connection(
        &app,
        &shell,
        &request.connection_id,
        connection.credential_binding_required,
        legacy_raw,
        confirmation,
    )
    .await
}
