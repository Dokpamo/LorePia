use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::provider_commands) enum CredentialCompensationDeleteOutcome {
    Complete,
    Fail,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::provider_commands) enum CompensationObserveErrorPolicy {
    Propagate,
    Defer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::provider_commands) enum CompensationCredentialEffectPolicy {
    RequireNativeConfirmation,
    ObserveOnly,
}

#[derive(Debug)]
pub(in crate::provider_commands) enum DiscoveryCompensationDriveResult {
    Finished(shell::ProviderDiscoverySessionDto),
    NativeConfirmationRequired {
        session: shell::ProviderDiscoverySessionDto,
        context: NativeCredentialEffectContext,
    },
}
pub(in crate::provider_commands) fn discovery_compensation_credential_authority(
    context: &shell::ProviderDiscoveryCredentialAuthorityDto,
) -> CommandResult<CredentialAuthority> {
    CredentialAuthority::new(
        context.native_execution_id.clone(),
        context.connection_binding_sha256.clone(),
    )
    .map_err(Into::into)
}

pub(in crate::provider_commands) fn discovery_compensation_confirmation_context(
    session: &shell::ProviderDiscoverySessionDto,
    authority: &shell::ProviderDiscoveryCredentialAuthorityDto,
) -> CommandResult<NativeCredentialEffectContext> {
    if authority.connection_id != session.connection_id {
        return Err(CommandError::invalid_input());
    }
    let mut state_hasher = Sha256::new();
    let session_revision = session.revision.to_string();
    state_hasher.update(b"dev.lorepia.discovery-compensation-confirmation.v1\0");
    for value in [
        session.id.as_bytes(),
        session_revision.as_bytes(),
        authority.operation_id.as_bytes(),
        authority.native_execution_id.as_bytes(),
        authority.commit_attempt_id.as_bytes(),
        authority.connection_id.as_bytes(),
        authority.credential_api_origin.as_bytes(),
        authority.credential_origin_approval_id.as_bytes(),
        authority.credential_origin_grant_sha256.as_bytes(),
        authority.connection_binding_sha256.as_bytes(),
    ] {
        state_hasher.update(value);
        state_hasher.update([0]);
    }
    let state_sha256 = format!("{:x}", state_hasher.finalize());
    NativeCredentialEffectContext::new(
        NativeCredentialEffect::DiscoveryCompensation,
        session.connection_id.clone(),
        authority.credential_api_origin.clone(),
        format!(
            "compensation_state_sha256={state_sha256};session_revision={}",
            session.revision
        ),
    )
    .map_err(Into::into)
}
pub(in crate::provider_commands) async fn drive_provider_discovery_compensation_observe_only(
    app: &AppHandle,
    shell: &shell::ShellApi,
    session: shell::ProviderDiscoverySessionDto,
    allow_failed_retry: bool,
    observe_error_policy: CompensationObserveErrorPolicy,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    match drive_provider_discovery_compensation_with(
        &PlatformDiscoveryCredentialVault { app },
        shell,
        session,
        allow_failed_retry,
        CompensationCredentialEffectPolicy::ObserveOnly,
        observe_error_policy,
        None,
    )
    .await?
    {
        DiscoveryCompensationDriveResult::Finished(session) => Ok(session),
        DiscoveryCompensationDriveResult::NativeConfirmationRequired { .. } => {
            Err(CommandError::internal())
        }
    }
}

/// Runs the observation pass under the writer, presents the trusted modal
/// with no credential lock held, then reacquires and repeats every durable and
/// native precondition before consuming the one-use receipt and deleting.
pub(in crate::provider_commands) async fn drive_provider_discovery_compensation_explicit(
    app: &AppHandle,
    state: &AppState,
    shell: &shell::ShellApi,
    expected_session: shell::ProviderDiscoverySessionDto,
    allow_failed_retry: bool,
    observe_error_policy: CompensationObserveErrorPolicy,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let vault = PlatformDiscoveryCredentialVault { app };
    let initial = {
        let _operation = state.lock_provider_credential_operation().await;
        let latest = shell.get_provider_discovery(&expected_session.id)?;
        if latest != expected_session {
            return Err(CommandError::invalid_input());
        }
        drive_provider_discovery_compensation_with(
            &vault,
            shell,
            latest,
            allow_failed_retry,
            CompensationCredentialEffectPolicy::RequireNativeConfirmation,
            observe_error_policy,
            None,
        )
        .await?
    };
    let (prompted_session, prompt_context) = match initial {
        DiscoveryCompensationDriveResult::Finished(session) => return Ok(session),
        DiscoveryCompensationDriveResult::NativeConfirmationRequired { session, context } => {
            (session, context)
        }
    };

    // Deliberately outside the global writer. Cancel, focus loss, and native
    // presentation failure all return here without starting the durable step.
    let confirmation = vault.confirm_compensation(prompt_context).await?;

    let _operation = state.lock_provider_credential_operation().await;
    let latest = shell.get_provider_discovery(&prompted_session.id)?;
    if latest != prompted_session {
        return Err(CommandError::invalid_input());
    }
    match drive_provider_discovery_compensation_with(
        &vault,
        shell,
        latest,
        allow_failed_retry,
        CompensationCredentialEffectPolicy::RequireNativeConfirmation,
        observe_error_policy,
        Some(confirmation),
    )
    .await?
    {
        DiscoveryCompensationDriveResult::Finished(session) => Ok(session),
        DiscoveryCompensationDriveResult::NativeConfirmationRequired { .. } => {
            Err(CommandError::invalid_input())
        }
    }
}

#[allow(clippy::too_many_lines)] // Keeps the observe-confirm-reobserve-delete state machine linear.
pub(in crate::provider_commands) async fn drive_provider_discovery_compensation_with(
    vault: &dyn DiscoveryCredentialVault,
    shell: &shell::ShellApi,
    session: shell::ProviderDiscoverySessionDto,
    allow_failed_retry: bool,
    effect_policy: CompensationCredentialEffectPolicy,
    observe_error_policy: CompensationObserveErrorPolicy,
    confirmation: Option<DiscoveryCompensationConfirmation>,
) -> CommandResult<DiscoveryCompensationDriveResult> {
    if session.state != "compensating" {
        return Ok(DiscoveryCompensationDriveResult::Finished(session));
    }
    let attempt_id = session
        .commit_attempt_id
        .as_deref()
        .ok_or_else(CommandError::internal)?;
    let steps = shell.list_provider_discovery_compensation_steps(attempt_id)?;
    let mut credential_steps = steps
        .iter()
        .filter(|step| step.kind == "remove_credential_slot");
    let credential_step = credential_steps.next();
    if credential_steps.next().is_some()
        || credential_step.is_some_and(|step| step.commit_attempt_id != attempt_id)
    {
        return Err(CommandError::internal());
    }
    // The DTO deliberately withholds the native slot target. Core revalidates
    // this exact step ID against the session's immutable commit plan before it
    // lets the backend claim the step; only then may the backend use the
    // session-bound connection ID as the opaque native credential reference.
    let Some(step) = credential_step else {
        if session.credential_binding_requested {
            return Err(CommandError::internal());
        }
        return shell
            .continue_provider_discovery_compensation(&session.id)
            .map(DiscoveryCompensationDriveResult::Finished)
            .map_err(Into::into);
    };

    match step.status.as_str() {
        "completed" => {
            return shell
                .continue_provider_discovery_compensation(&session.id)
                .map(DiscoveryCompensationDriveResult::Finished)
                .map_err(Into::into);
        }
        "pending" if step.attempt_count == 0 => {}
        "failed" if allow_failed_retry => {}
        "pending" | "in_progress" | "failed" | "outcome_unknown" => {
            return Ok(DiscoveryCompensationDriveResult::Finished(session));
        }
        _ => return Err(CommandError::internal()),
    }

    let authority_context =
        shell.get_provider_discovery_credential_compensation_authority(&session.id)?;
    if authority_context.operation_id.is_empty()
        || authority_context.commit_attempt_id != attempt_id
        || authority_context.connection_id != session.connection_id
    {
        return Err(CommandError::internal());
    }
    let authority = discovery_compensation_credential_authority(&authority_context)?;
    let Some(preflight) = observe_discovery_compensation_slot(
        vault,
        &session.connection_id,
        &authority,
        observe_error_policy,
    )
    .await?
    else {
        // A status/read backend outage is not evidence that this exact slot
        // is absent. Leave the pending step untouched so startup can publish
        // and a later recovery pass can retry it.
        return Ok(DiscoveryCompensationDriveResult::Finished(session));
    };
    if preflight == BoundCredentialObservation::Match {
        if effect_policy == CompensationCredentialEffectPolicy::ObserveOnly {
            // Startup may observe and publish non-effect progress, but a
            // database-derived target never gains unattended delete authority.
            return Ok(DiscoveryCompensationDriveResult::Finished(session));
        }
        let Some(confirmation) = confirmation else {
            let context =
                discovery_compensation_confirmation_context(&session, &authority_context)?;
            return Ok(
                DiscoveryCompensationDriveResult::NativeConfirmationRequired { session, context },
            );
        };
        let context = discovery_compensation_confirmation_context(&session, &authority_context)?;
        confirmation.consume_exact(&context)?;
    }
    let started = shell.start_provider_discovery_credential_compensation(&session.id, &step.id)?;
    if started.id != step.id
        || started.commit_attempt_id != attempt_id
        || started.kind != "remove_credential_slot"
        || started.status != "in_progress"
    {
        return Err(CommandError::internal());
    }

    let result = match preflight {
        BoundCredentialObservation::Missing => {
            complete_provider_discovery_credential_compensation(shell, &session, &step.id)
        }
        BoundCredentialObservation::Legacy
        | BoundCredentialObservation::Mismatch
        | BoundCredentialObservation::Unreadable => shell
            .mark_provider_discovery_credential_compensation_unknown(&session.id, &step.id)
            .map_err(Into::into),
        BoundCredentialObservation::Match => {
            let (delete_result, postflight) =
                delete_and_observe_discovery_bound_slot(vault, &session.connection_id, &authority)
                    .await;
            match credential_compensation_delete_outcome(&delete_result, &postflight) {
                CredentialCompensationDeleteOutcome::Complete => {
                    complete_provider_discovery_credential_compensation(shell, &session, &step.id)
                }
                CredentialCompensationDeleteOutcome::Fail => shell
                    .fail_provider_discovery_credential_compensation(
                        &session.id,
                        &step.id,
                        credential_compensation_failure(
                            "credential_compensation_delete_failed",
                            "provider.discovery.credential_compensation_delete_failed",
                        ),
                    )
                    .map_err(Into::into),
                CredentialCompensationDeleteOutcome::Unknown => shell
                    .mark_provider_discovery_credential_compensation_unknown(&session.id, &step.id)
                    .map_err(Into::into),
            }
        }
    }?;
    Ok(DiscoveryCompensationDriveResult::Finished(result))
}

pub(in crate::provider_commands) fn credential_compensation_delete_outcome(
    delete_result: &PlatformResult<()>,
    postflight: &PlatformResult<BoundCredentialObservation>,
) -> CredentialCompensationDeleteOutcome {
    if platform_result_requires_credential_recovery(delete_result) {
        return CredentialCompensationDeleteOutcome::Unknown;
    }
    match (delete_result, postflight) {
        (_, Ok(BoundCredentialObservation::Missing)) => {
            CredentialCompensationDeleteOutcome::Complete
        }
        (Err(_), Ok(BoundCredentialObservation::Match)) => {
            CredentialCompensationDeleteOutcome::Fail
        }
        _ => CredentialCompensationDeleteOutcome::Unknown,
    }
}

pub(super) fn platform_result_requires_credential_recovery(result: &PlatformResult<()>) -> bool {
    matches!(
        result,
        Err(error) if error.code() == PlatformErrorCode::CredentialRecoveryRequired
    )
}

pub(in crate::provider_commands) async fn observe_discovery_compensation_slot(
    vault: &dyn DiscoveryCredentialVault,
    reference: &str,
    authority: &CredentialAuthority,
    error_policy: CompensationObserveErrorPolicy,
) -> CommandResult<Option<BoundCredentialObservation>> {
    match vault.observe_bound(reference, authority.clone()).await {
        Ok(observation) => Ok(Some(observation)),
        Err(_) if error_policy == CompensationObserveErrorPolicy::Defer => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(in crate::provider_commands) async fn delete_and_observe_discovery_bound_slot(
    vault: &dyn DiscoveryCredentialVault,
    reference: &str,
    authority: &CredentialAuthority,
) -> (
    PlatformResult<()>,
    PlatformResult<BoundCredentialObservation>,
) {
    let delete_result = vault.delete_bound(reference, authority.clone()).await;
    let postflight = vault.observe_bound(reference, authority.clone()).await;
    (delete_result, postflight)
}

fn complete_provider_discovery_credential_compensation(
    shell: &shell::ShellApi,
    session: &shell::ProviderDiscoverySessionDto,
    step_id: &str,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    match shell.complete_provider_discovery_credential_compensation(&session.id, step_id) {
        Ok(session) => Ok(session),
        Err(_) => shell
            .fail_provider_discovery_credential_compensation(
                &session.id,
                step_id,
                credential_compensation_failure(
                    "credential_compensation_record_failed",
                    "provider.discovery.credential_compensation_record_failed",
                ),
            )
            .map_err(Into::into),
    }
}

fn credential_compensation_failure(code: &str, message_key: &str) -> shell::DiscoveryFailureDto {
    shell::DiscoveryFailureDto {
        code: code.to_owned(),
        message_key: message_key.to_owned(),
        recoverable: true,
    }
}
