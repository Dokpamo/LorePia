use lorepia_shell_api as shell;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tauri_plugin_lorepia_platform::{
    BoundCredentialObservation, LorepiaPlatformExt, NativeCaptureStatus,
};

use super::credentials::{
    CompensationObserveErrorPolicy, ExistingConnectionCredentialReader,
    PlatformExistingConnectionCredentialReader,
    credential_authority_for_existing_connection_with_reader, credential_for_discovery_action,
    discovery_credential_authority, drive_provider_discovery_compensation_explicit,
    drive_provider_discovery_compensation_observe_only, promote_discovery_credential_lease,
    recover_provider_discovery_credential_installs, require_started_discovery_credential_install,
};
use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

mod runtime;

pub(in crate::provider_commands) use runtime::register_active_discovery_request;
use runtime::{ActiveDiscoveryRequestRegistration, signal_active_discovery_request_cancellation};

pub(in crate::provider_commands) const MAXIMUM_PROVIDER_CURL_BYTES: usize = 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginProviderDiscoveryRequest {
    pub input: shell::BeginProviderDiscoveryInput,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginProviderDiscoveryCurlRequest {
    pub input: shell::BeginProviderDiscoveryCurlInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LimitRequest {
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollProviderDiscoveryEventsForSessionRequest {
    pub session_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoverySessionRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordProviderDiscoveryAssistantFailureRequest {
    pub session_id: String,
    pub kind: shell::DiscoveryAssistantFailureKindInput,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterruptProviderDiscoveryAssistantRequest {
    pub session_id: String,
    pub outcome: shell::DiscoveryAssistantInterruptionOutcomeInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinueProviderDiscoveryRequest {
    pub input: shell::ContinueProviderDiscoveryInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupplyProviderDiscoveryDocumentEvidenceRequest {
    pub session_id: String,
    pub expected_revision: u64,
    pub document_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SupplyProviderDiscoveryCurlEvidenceRequest {
    pub session_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelProviderDiscoveryRequest {
    pub session_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitProviderDiscoveryRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CapturedProviderDiscoveryDto {
    pub session: shell::ProviderDiscoverySessionDto,
    pub capture: NativeCaptureStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderDiscoveryEventRequest {
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryCompensationStepsRequest {
    pub commit_attempt_id: String,
}

#[tauri::command]
pub async fn begin_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
    request: BeginProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    begin_provider_discovery_with_reader(
        &shell,
        request.input,
        &PlatformExistingConnectionCredentialReader { app: &app },
    )
    .await
}

#[tauri::command]
pub async fn begin_provider_discovery_curl(
    app: AppHandle,
    state: State<'_, AppState>,
    request: BeginProviderDiscoveryCurlRequest,
) -> CommandResult<CapturedProviderDiscoveryDto> {
    state.ensure_ready()?;
    let captured = app
        .lorepia_platform()
        .capture_sensitive_text_from_clipboard(MAXIMUM_PROVIDER_CURL_BYTES)
        .await?;
    let capture = captured.status();
    let curl = bounded_secret_curl(captured.into_secret_string())?;
    let shell = state.shell()?;
    let session = begin_provider_discovery_curl_with_reader(
        &shell,
        request.input,
        curl,
        &PlatformExistingConnectionCredentialReader { app: &app },
    )
    .await?;
    Ok(CapturedProviderDiscoveryDto { session, capture })
}

#[tauri::command]
pub fn list_provider_discoveries(
    state: State<'_, AppState>,
    request: LimitRequest,
) -> CommandResult<Vec<shell::ProviderDiscoverySessionDto>> {
    state
        .shell()?
        .list_provider_discoveries(request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .get_provider_discovery(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_discovery_candidates(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Vec<shell::DiscoveryCandidateDto>> {
    state
        .shell()?
        .list_provider_discovery_candidates(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_discovery_evidence(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Vec<shell::DiscoveryEvidenceDto>> {
    state
        .shell()?
        .list_provider_discovery_evidence(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_discovery_approvals(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Vec<shell::DiscoveryApprovalRecordDto>> {
    state
        .shell()?
        .list_provider_discovery_approvals(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery_review(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Option<shell::DiscoveryReviewDto>> {
    state
        .shell()?
        .get_provider_discovery_review(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery_approval_proposal(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Option<shell::ProviderDiscoveryApprovalProposalDto>> {
    state
        .shell()?
        .get_provider_discovery_approval_proposal(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery_review_proposal(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Option<shell::ProviderDiscoveryReviewProposalDto>> {
    state
        .shell()?
        .get_provider_discovery_review_proposal(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn get_provider_discovery_assistant_resume_boundary(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<Option<shell::DiscoveryAssistantResumeBoundaryDto>> {
    state
        .shell()?
        .get_provider_discovery_assistant_resume_boundary(&request.session_id)
        .map_err(Into::into)
}

/// Remote setup-assistant execution stays unavailable until Rust can price and
/// tokenize the exact prepared provider request. Deliberately accepting neither
/// application state nor a platform handle makes credential access and provider
/// construction impossible on this command path.
#[tauri::command]
pub fn run_provider_discovery_assistant_turn(
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::DiscoveryAssistantHostActionDto> {
    let _ = request;
    Err(CommandError::assistant_pricing_unavailable())
}

#[tauri::command]
pub fn resume_provider_discovery_assistant_core_host_action(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .resume_provider_discovery_assistant_core_host_action(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn approve_provider_discovery_assistant_retry(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .approve_provider_discovery_assistant_retry(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn request_provider_discovery_assistant_revision(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .request_provider_discovery_assistant_revision(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn accept_provider_discovery_assistant_draft(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .accept_provider_discovery_assistant_draft(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn record_provider_discovery_assistant_failure(
    state: State<'_, AppState>,
    request: RecordProviderDiscoveryAssistantFailureRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .record_provider_discovery_assistant_failure(
            &request.session_id,
            request.kind,
            request.retryable,
        )
        .map_err(Into::into)
}

#[tauri::command]
pub fn interrupt_provider_discovery_assistant(
    state: State<'_, AppState>,
    request: InterruptProviderDiscoveryAssistantRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .interrupt_provider_discovery_assistant(&request.session_id, request.outcome)
        .map_err(Into::into)
}

#[tauri::command]
pub fn restart_provider_discovery_assistant_after_interruption(
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .restart_provider_discovery_assistant_after_interruption(&request.session_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn continue_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ContinueProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    let input = request.input;
    // Keep the repository-wide provider credential read lease for the entire
    // dispatch, not merely while copying the secret. A native replacement or
    // removal therefore cannot race an authenticated discovery request.
    let dispatch_lease = state.lease_provider_credential_operation().await;
    let session = shell.get_provider_discovery(&input.session_id)?;
    let credential = credential_for_discovery_action(
        &state,
        &shell,
        &session,
        input.expected_revision,
        &input.action,
    )?;
    let mut next = if let Some(credential) = credential {
        let (registration, cancelled) = register_active_discovery_request(&input.session_id)?;
        continue_provider_discovery_with_credential_dispatch_off_runtime(
            &shell,
            input,
            credential,
            shell::TaskCredentialLease::new(dispatch_lease),
            cancelled,
            registration,
        )
        .await?
    } else {
        drop(dispatch_lease);
        continue_provider_discovery_off_runtime(&shell, input, None).await?
    };
    if next.state == "committing" {
        let _operation = state.lock_provider_credential_operation().await;
        let latest = shell.get_provider_discovery(&next.id)?;
        if latest.state == "committing"
            && !latest.cancellation_pending
            && latest.revision == next.revision
        {
            let _ = promote_discovery_credential_lease(&app, &state, &shell, &latest).await?;
        } else {
            next = latest;
        }
    }
    if next.cancellation_pending
        || matches!(
            next.state.as_str(),
            "ready" | "failed" | "cancelled" | "compensating"
        )
    {
        state.clear_discovery_credential_lease(&next.id);
    }
    Ok(next)
}

#[tauri::command]
pub fn supply_provider_discovery_document_evidence(
    state: State<'_, AppState>,
    request: SupplyProviderDiscoveryDocumentEvidenceRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    state
        .shell()?
        .supply_provider_discovery_document_evidence(
            &request.session_id,
            request.expected_revision,
            &request.document_url,
        )
        .map_err(Into::into)
}

#[tauri::command]
pub async fn supply_provider_discovery_curl_evidence(
    app: AppHandle,
    state: State<'_, AppState>,
    request: SupplyProviderDiscoveryCurlEvidenceRequest,
) -> CommandResult<CapturedProviderDiscoveryDto> {
    state.ensure_ready()?;
    let captured = app
        .lorepia_platform()
        .capture_sensitive_text_from_clipboard(MAXIMUM_PROVIDER_CURL_BYTES)
        .await?;
    let capture = captured.status();
    let curl = bounded_secret_curl(captured.into_secret_string())?;
    let shell = state.shell()?;
    let session = supply_provider_discovery_curl_evidence_off_runtime(
        &shell,
        request.session_id,
        request.expected_revision,
        curl,
    )
    .await?;
    Ok(CapturedProviderDiscoveryDto { session, capture })
}

#[tauri::command]
pub async fn cancel_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CancelProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    let cancelled = request_provider_discovery_cancellation(&state, &shell, &request)?;
    if cancelled.state != "committing" || !cancelled.cancellation_pending {
        return Ok(cancelled);
    }

    // Only credential compensation remains under the global writer. The
    // durable cancellation request above stays prompt even while another task
    // is resolving or reading provider model pages.
    let latest = {
        let _operation = state.lock_provider_credential_operation().await;
        let latest = shell.get_provider_discovery(&request.session_id)?;
        if latest.state != "committing" || !latest.cancellation_pending {
            return Ok(latest);
        }

        // A discovery credential install marks the atomic operation started before
        // the native vault write. Route cancellation back through Core's existing
        // commit-cancellation transition so its durable compensation recipe owns
        // removal of the slot; never delete an unjournaled reference here.
        let _ = shell.commit_provider_discovery(&request.session_id, None);
        shell.get_provider_discovery(&request.session_id)?
    };
    if latest.state == "compensating" {
        return drive_provider_discovery_compensation_explicit(
            &app,
            &state,
            &shell,
            latest,
            false,
            CompensationObserveErrorPolicy::Propagate,
        )
        .await;
    }
    Ok(latest)
}

pub(in crate::provider_commands) fn request_provider_discovery_cancellation(
    state: &AppState,
    shell: &shell::ShellApi,
    request: &CancelProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let current = shell.get_provider_discovery(&request.session_id)?;
    if current.revision != request.expected_revision {
        return shell
            .cancel_provider_discovery(&request.session_id, request.expected_revision)
            .map_err(Into::into);
    }
    // Revoke the backend-owned dispatch token before the durable cancellation
    // transition can be accepted. The active registration remains owned by
    // the blocking worker even if the invoking async task is dropped.
    signal_active_discovery_request_cancellation(&request.session_id)?;
    let cancelled =
        shell.cancel_provider_discovery(&request.session_id, request.expected_revision)?;
    state.clear_discovery_credential_lease(&request.session_id);
    Ok(cancelled)
}

#[tauri::command]
pub async fn commit_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CommitProviderDiscoveryRequest,
) -> CommandResult<shell::ProviderConnectionDto> {
    let shell = state.shell()?;
    let CommitProviderDiscoveryRequest { session_id } = request;
    let (error, latest) = {
        let _operation = state.lock_provider_credential_operation().await;
        let session = shell.get_provider_discovery(&session_id)?;
        let _ = promote_discovery_credential_lease(&app, &state, &shell, &session).await?;
        let credential_confirmation = if session.credential_binding_requested {
            let context = shell.get_provider_discovery_credential_install_context(&session_id)?;
            require_started_discovery_credential_install(&context)?;
            let observation = app
                .lorepia_platform()
                .observe_bound_credential(
                    &context.connection_id,
                    &discovery_credential_authority(&context)?,
                )
                .await?;
            if observation != BoundCredentialObservation::Match {
                return Err(CommandError::invalid_input());
            }
            Some(shell::ProviderDiscoveryCredentialCommitConfirmationDto::try_from(&context)?)
        } else {
            None
        };

        match shell.commit_provider_discovery(&session_id, credential_confirmation.as_ref()) {
            Ok(connection) => {
                state.clear_discovery_credential_lease(&session_id);
                return Ok(connection);
            }
            Err(error) => (error, shell.get_provider_discovery(&session_id).ok()),
        }
    };
    match latest {
        Some(latest) if latest.state == "compensating" => {
            state.clear_discovery_credential_lease(&session_id);
            drive_provider_discovery_compensation_explicit(
                &app,
                &state,
                &shell,
                latest,
                false,
                CompensationObserveErrorPolicy::Propagate,
            )
            .await?;
        }
        Some(latest) if latest.state == "ready" => {
            state.clear_discovery_credential_lease(&session_id);
        }
        Some(latest) if latest.state == "unknown_outcome" => {}
        _ => {}
    }
    Err(error.into())
}

#[tauri::command]
pub fn poll_provider_discovery_events(
    state: State<'_, AppState>,
    request: LimitRequest,
) -> CommandResult<Vec<shell::DiscoveryOutboxEventDto>> {
    state
        .shell()?
        .poll_provider_discovery_events(request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn poll_provider_discovery_events_for_session(
    state: State<'_, AppState>,
    request: PollProviderDiscoveryEventsForSessionRequest,
) -> CommandResult<Vec<shell::DiscoveryOutboxEventDto>> {
    state
        .shell()?
        .poll_provider_discovery_events_for_session(&request.session_id, request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn ack_provider_discovery_event(
    state: State<'_, AppState>,
    request: ProviderDiscoveryEventRequest,
) -> CommandResult<bool> {
    state
        .shell()?
        .ack_provider_discovery_event(&request.event_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn recover_provider_discovery(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::DiscoveryRecoveryResultDto>> {
    recover_provider_discovery_backend(&app, &state).await
}

pub(crate) async fn recover_provider_discovery_backend(
    app: &AppHandle,
    state: &AppState,
) -> CommandResult<Vec<shell::DiscoveryRecoveryResultDto>> {
    let _operation = state.lock_provider_credential_operation().await;
    let shell = state.shell()?;
    recover_provider_discovery_with_shell(app, &shell).await
}

pub(crate) async fn recover_provider_discovery_with_shell(
    app: &AppHandle,
    shell: &shell::ShellApi,
) -> CommandResult<Vec<shell::DiscoveryRecoveryResultDto>> {
    let recovered = recover_provider_discovery_credential_installs(shell)?;

    for session in shell.list_unfinished_provider_discovery_recovery_candidates()? {
        if session.state == "compensating" {
            drive_provider_discovery_compensation_observe_only(
                app,
                shell,
                session,
                false,
                CompensationObserveErrorPolicy::Defer,
            )
            .await?;
        }
    }
    Ok(recovered)
}

#[tauri::command]
pub fn list_provider_discovery_compensation_steps(
    state: State<'_, AppState>,
    request: DiscoveryCompensationStepsRequest,
) -> CommandResult<Vec<shell::DiscoveryCompensationRecordDto>> {
    state
        .shell()?
        .list_provider_discovery_compensation_steps(&request.commit_attempt_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn continue_provider_discovery_compensation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    let session = {
        let _operation = state.lock_provider_credential_operation().await;
        shell
            .continue_provider_discovery_compensation(&request.session_id)
            .map_err(CommandError::from)?
    };
    drive_provider_discovery_compensation_explicit(
        &app,
        &state,
        &shell,
        session,
        false,
        CompensationObserveErrorPolicy::Propagate,
    )
    .await
}

#[tauri::command]
pub async fn resume_provider_discovery_compensation(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ProviderDiscoverySessionRequest,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = state.shell()?;
    let session = {
        let _operation = state.lock_provider_credential_operation().await;
        shell
            .resume_provider_discovery_compensation(&request.session_id)
            .map_err(CommandError::from)?
    };
    drive_provider_discovery_compensation_explicit(
        &app,
        &state,
        &shell,
        session,
        true,
        CompensationObserveErrorPolicy::Propagate,
    )
    .await
}

pub(in crate::provider_commands) async fn run_shell_discovery_off_runtime<T, F>(
    operation: F,
) -> CommandResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> shell::ShellResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(operation)
        .await
        .map_err(|_| CommandError::internal())?
        .map_err(CommandError::from)
}

pub(in crate::provider_commands) async fn begin_provider_discovery_with_reader<
    R: ExistingConnectionCredentialReader + ?Sized,
>(
    shell: &shell::ShellApi,
    input: shell::BeginProviderDiscoveryInput,
    reader: &R,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let access_authority = credential_authority_for_existing_connection_with_reader(
        reader,
        shell,
        &input.connection_id,
    )
    .await?;
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || {
        shell.begin_provider_discovery_with_credential_authority(input, access_authority)
    })
    .await
}

pub(in crate::provider_commands) async fn begin_provider_discovery_curl_with_reader<
    R: ExistingConnectionCredentialReader + ?Sized,
>(
    shell: &shell::ShellApi,
    input: shell::BeginProviderDiscoveryCurlInput,
    curl: shell::SecretProviderCurl,
    reader: &R,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let access_authority = credential_authority_for_existing_connection_with_reader(
        reader,
        shell,
        &input.connection_id,
    )
    .await?;
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || {
        shell.begin_provider_discovery_curl_with_credential_authority(input, curl, access_authority)
    })
    .await
}

pub(in crate::provider_commands) async fn continue_provider_discovery_off_runtime(
    shell: &shell::ShellApi,
    input: shell::ContinueProviderDiscoveryInput,
    credential: Option<shell::SecretCredential>,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || shell.continue_provider_discovery(input, credential))
        .await
}

async fn continue_provider_discovery_with_credential_dispatch_off_runtime(
    shell: &shell::ShellApi,
    input: shell::ContinueProviderDiscoveryInput,
    credential: shell::SecretCredential,
    dispatch_lease: shell::TaskCredentialLease,
    cancelled: tokio::sync::watch::Receiver<bool>,
    registration: ActiveDiscoveryRequestRegistration,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || {
        let _registration = registration;
        shell.continue_provider_discovery_with_credential_dispatch(
            input,
            credential,
            dispatch_lease,
            cancelled,
        )
    })
    .await
}

pub(in crate::provider_commands) async fn supply_provider_discovery_curl_evidence_off_runtime(
    shell: &shell::ShellApi,
    session_id: String,
    expected_revision: u64,
    curl: shell::SecretProviderCurl,
) -> CommandResult<shell::ProviderDiscoverySessionDto> {
    let shell = shell.clone();
    run_shell_discovery_off_runtime(move || {
        shell.supply_provider_discovery_curl_evidence(&session_id, expected_revision, curl)
    })
    .await
}

pub(in crate::provider_commands) fn bounded_secret_curl(
    value: String,
) -> CommandResult<shell::SecretProviderCurl> {
    if value.len() > MAXIMUM_PROVIDER_CURL_BYTES || value.trim().is_empty() {
        return Err(CommandError::invalid_input());
    }
    Ok(shell::SecretProviderCurl::new(value))
}
