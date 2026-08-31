use lorepia_shell_api as shell;
use tauri::AppHandle;
use tauri_plugin_lorepia_platform::{
    BoundCredentialObservation, CredentialAuthority, CredentialStatus, NativeCaptureStatus,
    NativeCredential, PlatformResult,
};

use super::{
    CapturedDiscoveryCredential, DiscoveryCredentialVault, PlatformDiscoveryCredentialVault,
    platform_result_requires_credential_recovery, status_only_bound_observation,
};
use crate::{
    error::{CommandError, CommandResult},
    state::{AppState, DiscoveryCredentialLeaseBinding},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(in crate::provider_commands) enum CredentialInstallRecoveryAction {
    DeferToCore,
}
#[derive(Clone)]
pub(in crate::provider_commands) struct DiscoveryCredentialCommitCandidate {
    pub(in crate::provider_commands) session_id: String,
    pub(in crate::provider_commands) session_revision: u64,
    pub(in crate::provider_commands) connection_id: String,
    pub(in crate::provider_commands) commit_attempt_id: String,
    pub(in crate::provider_commands) commit_plan_sha256: String,
}

pub(in crate::provider_commands) trait DiscoveryCredentialInstallJournal:
    Send + Sync
{
    fn install_context(
        &self,
        session_id: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto>;

    fn reserve_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto>;

    fn start_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_reservation_id: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto>;

    fn attest_no_effect(
        &self,
        session_id: &str,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
    ) -> CommandResult<()>;

    #[allow(clippy::too_many_arguments)]
    fn mark_durability_unknown(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> CommandResult<()>;
}

impl DiscoveryCredentialInstallJournal for shell::ShellApi {
    fn install_context(
        &self,
        session_id: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
        self.get_provider_discovery_credential_install_context(session_id)
            .map_err(Into::into)
    }

    fn start_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_reservation_id: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
        self.start_provider_discovery_credential_install(
            session_id,
            expected_revision,
            operation_id,
            commit_attempt_id,
            commit_plan_sha256,
            native_execution_reservation_id,
        )
        .map_err(Into::into)
    }

    fn reserve_install(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
    ) -> CommandResult<shell::ProviderDiscoveryCredentialInstallContextDto> {
        self.reserve_provider_discovery_credential_install(
            session_id,
            expected_revision,
            operation_id,
            commit_attempt_id,
            commit_plan_sha256,
        )
        .map_err(Into::into)
    }

    fn attest_no_effect(
        &self,
        session_id: &str,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
    ) -> CommandResult<()> {
        self.attest_provider_discovery_credential_install_no_effect(
            session_id,
            operation_id,
            commit_attempt_id,
            commit_plan_sha256,
            native_execution_id,
        )
        .map(|_| ())
        .map_err(Into::into)
    }

    fn mark_durability_unknown(
        &self,
        session_id: &str,
        expected_revision: u64,
        operation_id: &str,
        commit_attempt_id: &str,
        commit_plan_sha256: &str,
        native_execution_id: &str,
        connection_id: &str,
        connection_binding_sha256: &str,
    ) -> CommandResult<()> {
        self.mark_provider_discovery_credential_install_durability_unknown(
            session_id,
            expected_revision,
            operation_id,
            commit_attempt_id,
            commit_plan_sha256,
            native_execution_id,
            connection_id,
            connection_binding_sha256,
        )
        .map(|_| ())
        .map_err(Into::into)
    }
}
pub(in crate::provider_commands) fn require_started_discovery_credential_install(
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    if context.operation_status == "started"
        && context.native_execution_id.is_some()
        && context.native_execution_reservation_id.as_deref()
            == context.native_execution_id.as_deref()
    {
        return Ok(());
    }
    // A matching envelope from a future database generation is still an
    // orphan until this exact rollback-visible WAL entry proves the native
    // effect was started. Never adopt it from Prepared.
    Err(CommandError::invalid_input())
}
pub(in crate::provider_commands) fn recover_provider_discovery_credential_installs(
    shell: &shell::ShellApi,
) -> CommandResult<Vec<shell::DiscoveryRecoveryResultDto>> {
    let mut recovered = Vec::new();
    let mut previous_candidate_ids = None;
    loop {
        let sessions = shell.list_provider_discovery_credential_recovery_candidates()?;
        if sessions.is_empty() {
            recovered.extend(shell.recover_provider_discovery()?);
            break;
        }
        let candidate_ids = sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        if previous_candidate_ids.as_ref() == Some(&candidate_ids) {
            return Err(CommandError::internal());
        }
        previous_candidate_ids = Some(candidate_ids);

        for session in sessions {
            // Startup always uses the recovery-only projection. It alone can
            // represent a Storage-proven sealed pre37 Started operation with
            // no physical execution ID, which must defer without vault access.
            // Normal product/confirmation paths continue to reject that shape.
            let context =
                shell.get_provider_discovery_credential_install_recovery_context(&session.id)?;
            if context.session_id != session.id
                || context.session_revision != session.revision
                || session.commit_attempt_id.as_deref() != Some(context.commit_attempt_id.as_str())
                || session.commit_plan_sha256.as_deref()
                    != Some(context.commit_plan_sha256.as_str())
                || context.commit_phase != "prepared"
                || context.connection_id != session.connection_id
            {
                return Err(CommandError::internal());
            }
            if context.operation_status == "started" {
                settle_started_discovery_credential_recovery(shell, &session, &context)?;
            } else if context.operation_status != "prepared" {
                return Err(CommandError::internal());
            }
        }

        recovered.extend(shell.recover_provider_discovery()?);
        if shell
            .list_provider_discovery_credential_recovery_candidates()?
            .is_empty()
        {
            break;
        }
    }

    Ok(recovered)
}

pub(in crate::provider_commands) fn settle_started_discovery_credential_recovery(
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    match (
        context.native_execution_reservation_id.as_deref(),
        context.native_execution_id.as_deref(),
    ) {
        (Some(reservation_id), Some(execution_id)) if reservation_id == execution_id => {
            shell.mark_provider_discovery_credential_install_durability_unknown(
                &session.id,
                context.session_revision,
                &context.operation_id,
                &context.commit_attempt_id,
                &context.commit_plan_sha256,
                execution_id,
                &context.connection_id,
                &context.connection_binding_sha256,
            )?;
            Ok(())
        }
        (None, None) => {
            // Sealed pre-37 Started rows intentionally have no physical
            // execution authority. Generic Core recovery classifies them
            // Unknown without native access.
            Ok(())
        }
        _ => Err(CommandError::internal()),
    }
}

#[cfg(test)]
pub(in crate::provider_commands) fn credential_install_recovery_action(
    _cancellation_pending: bool,
    operation_status: &str,
    _credential_status: CredentialStatus,
) -> CommandResult<CredentialInstallRecoveryAction> {
    match operation_status {
        "prepared" | "started" => Ok(CredentialInstallRecoveryAction::DeferToCore),
        _ => Err(CommandError::internal()),
    }
}
fn discovery_credential_lease_binding(
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
    expected_revision: u64,
) -> CommandResult<DiscoveryCredentialLeaseBinding> {
    if session.revision != expected_revision || !session.credential_binding_requested {
        return Err(CommandError::invalid_input());
    }
    let context = shell.get_provider_discovery_credential_lease_context(&session.id)?;
    if context.session_id != session.id || context.connection_id != session.connection_id {
        return Err(CommandError::internal());
    }
    Ok(DiscoveryCredentialLeaseBinding {
        session_id: context.session_id,
        connection_id: context.connection_id,
        credential_origin_approval_id: context.credential_origin_approval_id,
        credential_origin_grant_sha256: context.credential_origin_grant_sha256,
        connection_binding_sha256: context.connection_binding_sha256,
    })
}

pub(crate) async fn discovery_credential_status(
    app: &AppHandle,
    state: &AppState,
    shell: &shell::ShellApi,
    session_id: &str,
    expected_revision: u64,
) -> CommandResult<CredentialStatus> {
    let session = shell.get_provider_discovery(session_id)?;
    if session.revision != expected_revision || !session.credential_binding_requested {
        return Err(CommandError::invalid_input());
    }
    if session.state != "committing" {
        let binding = discovery_credential_lease_binding(shell, &session, expected_revision)?;
        return Ok(state.discovery_credential_lease_status(&binding));
    }

    let context = shell.get_provider_discovery_credential_install_context(session_id)?;
    if context.session_id != session.id
        || context.session_revision != session.revision
        || context.connection_id != session.connection_id
        || session.commit_attempt_id.as_deref() != Some(context.commit_attempt_id.as_str())
        || session.commit_plan_sha256.as_deref() != Some(context.commit_plan_sha256.as_str())
    {
        return Err(CommandError::internal());
    }
    discovery_committing_credential_status_with(&PlatformDiscoveryCredentialVault { app }, &context)
        .await
}

pub(in crate::provider_commands) async fn discovery_committing_credential_status_with(
    vault: &dyn DiscoveryCredentialVault,
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<CredentialStatus> {
    match (
        context.operation_status.as_str(),
        context.native_execution_id.as_ref(),
    ) {
        // Prepared has no physical slot to inspect. In particular, a slot
        // from a rolled-back execution must not be projected as available.
        ("prepared", None) => Ok(CredentialStatus::Missing),
        ("started", Some(_)) => Ok(status_only_bound_observation(
            vault
                .observe_bound(
                    &context.connection_id,
                    discovery_credential_authority(context)?,
                )
                .await,
        )),
        _ => Ok(CredentialStatus::Unreadable),
    }
}

pub(crate) async fn capture_discovery_credential_for_empty_bound_slot(
    app: &AppHandle,
    reference: &str,
    authority: &CredentialAuthority,
) -> CommandResult<(NativeCaptureStatus, NativeCredential)> {
    let captured = capture_discovery_credential_for_empty_bound_slot_with(
        &PlatformDiscoveryCredentialVault { app },
        reference,
        authority,
    )
    .await?;
    Ok((captured.status, captured.value))
}

pub(in crate::provider_commands) async fn capture_discovery_credential_for_empty_bound_slot_with(
    vault: &dyn DiscoveryCredentialVault,
    reference: &str,
    authority: &CredentialAuthority,
) -> CommandResult<CapturedDiscoveryCredential> {
    require_missing_discovery_bound_slot(vault, reference, authority).await?;
    let captured = vault.capture_bound().await?;
    require_missing_discovery_bound_slot(vault, reference, authority).await?;
    Ok(captured)
}

async fn require_missing_discovery_bound_slot(
    vault: &dyn DiscoveryCredentialVault,
    reference: &str,
    authority: &CredentialAuthority,
) -> CommandResult<()> {
    if vault.status_bound(reference, authority.clone()).await? == CredentialStatus::Missing {
        Ok(())
    } else {
        Err(CommandError::invalid_input())
    }
}

pub(crate) async fn capture_precommit_discovery_credential(
    app: &AppHandle,
    state: &AppState,
    shell: &shell::ShellApi,
    session_id: &str,
    expected_revision: u64,
) -> CommandResult<NativeCaptureStatus> {
    capture_precommit_discovery_credential_with(
        &PlatformDiscoveryCredentialVault { app },
        state,
        shell,
        session_id,
        expected_revision,
    )
    .await
}

pub(in crate::provider_commands) async fn capture_precommit_discovery_credential_with(
    vault: &dyn DiscoveryCredentialVault,
    state: &AppState,
    shell: &shell::ShellApi,
    session_id: &str,
    expected_revision: u64,
) -> CommandResult<NativeCaptureStatus> {
    let session = shell.get_provider_discovery(session_id)?;
    if session.state == "committing" {
        return Err(CommandError::invalid_input());
    }
    let binding = discovery_credential_lease_binding(shell, &session, expected_revision)?;
    let captured = vault.capture_bound().await?;
    state.install_discovery_credential_lease(binding, captured.value)?;
    Ok(captured.status)
}

/// Moves one exact process-local discovery credential into the existing
/// durable install WAL. The WAL is marked started before the runtime entry is
/// invalidated or the native vault is written.
pub(in crate::provider_commands) async fn promote_discovery_credential_lease(
    app: &AppHandle,
    state: &AppState,
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
) -> CommandResult<bool> {
    if session.state != "committing" || !session.credential_binding_requested {
        return Ok(false);
    }
    let candidate = DiscoveryCredentialCommitCandidate {
        session_id: session.id.clone(),
        session_revision: session.revision,
        connection_id: session.connection_id.clone(),
        commit_attempt_id: session
            .commit_attempt_id
            .clone()
            .ok_or_else(CommandError::internal)?,
        commit_plan_sha256: session
            .commit_plan_sha256
            .clone()
            .ok_or_else(CommandError::internal)?,
    };
    promote_discovery_credential_lease_with(
        &PlatformDiscoveryCredentialVault { app },
        state,
        shell,
        &candidate,
    )
    .await
}

pub(in crate::provider_commands) async fn promote_discovery_credential_lease_with(
    vault: &dyn DiscoveryCredentialVault,
    state: &AppState,
    journal: &dyn DiscoveryCredentialInstallJournal,
    candidate: &DiscoveryCredentialCommitCandidate,
) -> CommandResult<bool> {
    let context = journal.install_context(&candidate.session_id)?;
    validate_discovery_commit_context(candidate, &context)?;
    if context.operation_status == "started" {
        require_started_discovery_credential_install(&context)?;
        if state.discovery_credential_lease_matches_commit(
            &candidate.session_id,
            &context.connection_id,
            &context.connection_binding_sha256,
        )? {
            // Started owns the native side effect already. A surviving runtime
            // lease would be ambiguous and must never trigger a second store.
            state.clear_discovery_credential_lease(&candidate.session_id);
            return Err(CommandError::internal());
        }
        return Ok(false);
    }
    if context.operation_status != "prepared"
        || context.native_execution_reservation_id.is_some()
        || context.native_execution_id.is_some()
    {
        return Err(CommandError::internal());
    }
    if !state.discovery_credential_lease_matches_commit(
        &candidate.session_id,
        &context.connection_id,
        &context.connection_binding_sha256,
    )? {
        return Ok(false);
    }
    let reserved = journal.reserve_install(
        &candidate.session_id,
        candidate.session_revision,
        &context.operation_id,
        &context.commit_attempt_id,
        &context.commit_plan_sha256,
    )?;
    validate_reserved_discovery_install_context(&context, &reserved)?;
    let authority = discovery_credential_reservation_authority(&reserved)?;
    require_missing_discovery_bound_slot(vault, &reserved.connection_id, &authority).await?;
    // Recheck after the asynchronous reservation boundary and immediately
    // before consuming the process-local secret. A restored or competing
    // exact slot must remain visible to recovery rather than being adopted or
    // handed to the native store under this new execution authority.
    require_missing_discovery_bound_slot(vault, &reserved.connection_id, &authority).await?;
    let credential = state
        .take_discovery_credential_lease_for_commit(
            &candidate.session_id,
            &reserved.connection_id,
            &reserved.connection_binding_sha256,
        )?
        .ok_or_else(CommandError::internal)?;
    let reservation_id = reserved
        .native_execution_reservation_id
        .as_deref()
        .ok_or_else(CommandError::internal)?;
    let prepared_store =
        vault.prepare_bound_store(&reserved.connection_id, credential, &authority)?;
    // Everything that can fail without attempting the native write is above
    // this cutpoint. Started is the durable store-attempt marker, so invoke
    // exactly one store immediately after Core accepts this exact reservation.
    let started = journal.start_install(
        &candidate.session_id,
        candidate.session_revision,
        &reserved.operation_id,
        &reserved.commit_attempt_id,
        &reserved.commit_plan_sha256,
        reservation_id,
    )?;
    if !discovery_install_start_is_exact(&started, &reserved, reservation_id, &authority) {
        return Err(CommandError::internal());
    }
    let store_result = vault.store_prepared(prepared_store).await;
    if platform_result_requires_credential_recovery(&store_result) {
        mark_discovery_credential_store_durability_unknown(
            journal, candidate, &started, &authority,
        )?;
        return Err(store_result
            .expect_err("recovery-required result is an error")
            .into());
    }
    let observation = vault
        .observe_bound(&reserved.connection_id, authority.clone())
        .await;
    settle_discovery_credential_store_attempt(
        journal,
        candidate,
        &started,
        &authority,
        store_result,
        observation,
    )
}

fn settle_discovery_credential_store_attempt(
    journal: &dyn DiscoveryCredentialInstallJournal,
    candidate: &DiscoveryCredentialCommitCandidate,
    started: &shell::ProviderDiscoveryCredentialInstallContextDto,
    authority: &CredentialAuthority,
    store_result: PlatformResult<()>,
    observation: PlatformResult<BoundCredentialObservation>,
) -> CommandResult<bool> {
    match observation {
        Ok(BoundCredentialObservation::Match) => Ok(true),
        Ok(BoundCredentialObservation::Missing) => {
            journal.attest_no_effect(
                &candidate.session_id,
                &started.operation_id,
                &started.commit_attempt_id,
                &started.commit_plan_sha256,
                authority.authority_id(),
            )?;
            store_result?;
            Err(CommandError::internal())
        }
        Ok(
            BoundCredentialObservation::Legacy
            | BoundCredentialObservation::Mismatch
            | BoundCredentialObservation::Unreadable,
        ) => {
            store_result?;
            Err(CommandError::internal())
        }
        Err(error) => {
            store_result?;
            Err(error.into())
        }
    }
}

fn mark_discovery_credential_store_durability_unknown(
    journal: &dyn DiscoveryCredentialInstallJournal,
    candidate: &DiscoveryCredentialCommitCandidate,
    started: &shell::ProviderDiscoveryCredentialInstallContextDto,
    authority: &CredentialAuthority,
) -> CommandResult<()> {
    journal.mark_durability_unknown(
        &candidate.session_id,
        started.session_revision,
        &started.operation_id,
        &started.commit_attempt_id,
        &started.commit_plan_sha256,
        authority.authority_id(),
        &started.connection_id,
        &started.connection_binding_sha256,
    )
}

fn validate_discovery_commit_context(
    candidate: &DiscoveryCredentialCommitCandidate,
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    if context.session_id != candidate.session_id
        || context.session_revision != candidate.session_revision
        || context.connection_id != candidate.connection_id
        || context.commit_attempt_id != candidate.commit_attempt_id
        || context.commit_plan_sha256 != candidate.commit_plan_sha256
        || context.commit_phase != "prepared"
    {
        return Err(CommandError::internal());
    }
    Ok(())
}

fn validate_reserved_discovery_install_context(
    initial: &shell::ProviderDiscoveryCredentialInstallContextDto,
    reserved: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<()> {
    if reserved.operation_status != "prepared"
        || reserved.native_execution_id.is_some()
        || reserved.commit_phase != "prepared"
        || reserved.operation_id != initial.operation_id
        || reserved.commit_attempt_id != initial.commit_attempt_id
        || reserved.commit_plan_sha256 != initial.commit_plan_sha256
        || reserved.connection_id != initial.connection_id
        || reserved.connection_binding_sha256 != initial.connection_binding_sha256
    {
        return Err(CommandError::internal());
    }
    Ok(())
}

fn discovery_install_start_is_exact(
    started: &shell::ProviderDiscoveryCredentialInstallContextDto,
    reserved: &shell::ProviderDiscoveryCredentialInstallContextDto,
    reservation_id: &str,
    authority: &CredentialAuthority,
) -> bool {
    started.operation_status == "started"
        && started.commit_phase == "prepared"
        && started.operation_id == reserved.operation_id
        && started.commit_attempt_id == reserved.commit_attempt_id
        && started.commit_plan_sha256 == reserved.commit_plan_sha256
        && started.connection_id == reserved.connection_id
        && started.connection_binding_sha256 == reserved.connection_binding_sha256
        && started.native_execution_reservation_id.as_deref() == Some(reservation_id)
        && started.native_execution_id.as_deref() == Some(authority.authority_id())
}

fn discovery_action_requires_runtime_credential(
    session: &shell::ProviderDiscoverySessionDto,
    action: &shell::ContinueProviderDiscoveryActionInput,
) -> bool {
    session.credential_binding_requested
        && matches!(
            (session.state.as_str(), action),
            (
                "awaiting_credential_origin_approval",
                shell::ContinueProviderDiscoveryActionInput::ApproveCredentialOrigin { .. }
            ) | (
                "awaiting_probe_consent",
                shell::ContinueProviderDiscoveryActionInput::ApproveProbes { .. }
            ) | (
                "interrupted",
                shell::ContinueProviderDiscoveryActionInput::RestartInterrupted
            )
        )
        && (session.state != "interrupted"
            || matches!(
                session.recovery_operation.as_deref(),
                Some("list_models" | "probe_capabilities")
            ))
}

pub(in crate::provider_commands) fn credential_for_discovery_action(
    state: &AppState,
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
    expected_revision: u64,
    action: &shell::ContinueProviderDiscoveryActionInput,
) -> CommandResult<Option<shell::SecretCredential>> {
    if !discovery_action_requires_runtime_credential(session, action) {
        return Ok(None);
    }
    let binding = discovery_credential_lease_binding(shell, session, expected_revision)?;
    state
        .discovery_credential_for_request(&binding)?
        .ok_or_else(CommandError::invalid_input)
        .map(Some)
}

pub(crate) fn discovery_credential_authority(
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<CredentialAuthority> {
    if context.operation_status != "started"
        || context.native_execution_reservation_id.as_deref()
            != context.native_execution_id.as_deref()
    {
        return Err(CommandError::invalid_input());
    }
    let native_execution_id = context
        .native_execution_id
        .clone()
        .ok_or_else(CommandError::invalid_input)?;
    CredentialAuthority::new(
        native_execution_id,
        context.connection_binding_sha256.clone(),
    )
    .map_err(Into::into)
}

pub(crate) fn discovery_credential_reservation_authority(
    context: &shell::ProviderDiscoveryCredentialInstallContextDto,
) -> CommandResult<CredentialAuthority> {
    if context.operation_status != "prepared" || context.native_execution_id.is_some() {
        return Err(CommandError::invalid_input());
    }
    let reservation_id = context
        .native_execution_reservation_id
        .clone()
        .ok_or_else(CommandError::invalid_input)?;
    CredentialAuthority::new(reservation_id, context.connection_binding_sha256.clone())
        .map_err(Into::into)
}
