use lorepia_shell_api::{ProviderCredentialAccessAuthorityContext, ShellApi};
use sha2::{Digest, Sha256};
use tauri::AppHandle;
use tauri_plugin_lorepia_platform::{
    CredentialStatus, LorepiaPlatformExt, NativeCredentialEffect,
    NativeCredentialEffectConfirmation, NativeCredentialEffectContext,
};

use crate::error::{CommandError, CommandResult};

use super::{
    authority::credential_authority,
    types::{CredentialVault, OrdinaryCredentialTargetPolicy, PlatformCredentialVault},
};

pub(crate) async fn ensure_new_connection_slot_missing(
    app: &AppHandle,
    connection_id: &str,
) -> CommandResult<()> {
    ensure_slot_missing(&PlatformCredentialVault { app }, connection_id).await
}
pub(super) fn consume_connection_confirmation(
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

pub(super) async fn consume_legacy_confirmation(
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
pub(super) async fn ensure_slot_missing(
    vault: &dyn CredentialVault,
    connection_id: &str,
) -> CommandResult<()> {
    if vault.status(connection_id).await? != CredentialStatus::Missing {
        return Err(CommandError::invalid_input());
    }
    Ok(())
}
pub(super) fn ensure_ordinary_connection_does_not_alias_legacy_raw_slot(
    target_policy: &dyn OrdinaryCredentialTargetPolicy,
    connection_id: &str,
) -> CommandResult<()> {
    if target_policy.aliases_legacy_raw_slot(connection_id)? {
        return Err(CommandError::invalid_input());
    }
    Ok(())
}
pub(super) async fn ensure_bound_slot_missing(
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
