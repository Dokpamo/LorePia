use lorepia_shell_api as shell;
use serde::Deserialize;
use tauri::{AppHandle, State};

use super::credentials::{
    ExistingConnectionCredentialReader, PlatformExistingConnectionCredentialReader,
    credential_for_connection_with_reader,
};
use crate::{error::CommandResult, state::AppState};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartProviderModelSyncRequest {
    pub connection_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSyncJobRequest {
    pub job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListProviderModelSyncsRequest {
    pub connection_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApproveProviderModelSyncRequest {
    pub job_id: String,
    pub review_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PollProviderModelSyncEventsRequest {
    pub job_id: String,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckProviderModelSyncEventRequest {
    pub job_id: String,
    pub sequence: u64,
}

#[tauri::command]
pub async fn start_provider_model_sync(
    app: AppHandle,
    state: State<'_, AppState>,
    request: StartProviderModelSyncRequest,
) -> CommandResult<shell::ModelSyncStartedDto> {
    let shell = state.shell()?;
    let dispatch_lease = state.lease_provider_credential_operation().await;
    start_provider_model_sync_with_reader(
        &shell,
        &request.connection_id,
        &PlatformExistingConnectionCredentialReader { app: &app },
        Some(shell::TaskCredentialLease::new(dispatch_lease)),
    )
    .await
}

#[tauri::command]
pub fn get_provider_model_sync(
    state: State<'_, AppState>,
    request: ModelSyncJobRequest,
) -> CommandResult<shell::ModelSyncJobDto> {
    state
        .shell()?
        .get_provider_model_sync(&request.job_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_model_syncs(
    state: State<'_, AppState>,
    request: ListProviderModelSyncsRequest,
) -> CommandResult<Vec<shell::ModelSyncJobDto>> {
    state
        .shell()?
        .list_provider_model_syncs(&request.connection_id, request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn approve_provider_model_sync(
    state: State<'_, AppState>,
    request: ApproveProviderModelSyncRequest,
) -> CommandResult<shell::ModelSyncJobDto> {
    state
        .shell()?
        .approve_provider_model_sync(&request.job_id, &request.review_sha256)
        .map_err(Into::into)
}

#[tauri::command]
pub fn cancel_provider_model_sync(
    state: State<'_, AppState>,
    request: ModelSyncJobRequest,
) -> CommandResult<shell::ModelSyncJobDto> {
    state
        .shell()?
        .cancel_provider_model_sync(&request.job_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn poll_provider_model_sync_events(
    state: State<'_, AppState>,
    request: PollProviderModelSyncEventsRequest,
) -> CommandResult<Vec<shell::ModelSyncEventDto>> {
    state
        .shell()?
        .poll_provider_model_sync_events(&request.job_id, request.limit)
        .map_err(Into::into)
}

#[tauri::command]
pub fn ack_provider_model_sync_event(
    state: State<'_, AppState>,
    request: AckProviderModelSyncEventRequest,
) -> CommandResult<bool> {
    state
        .shell()?
        .ack_provider_model_sync_event(&request.job_id, request.sequence)
        .map_err(Into::into)
}

pub(in crate::provider_commands) async fn start_provider_model_sync_with_reader<
    R: ExistingConnectionCredentialReader + ?Sized,
>(
    shell: &shell::ShellApi,
    connection_id: &str,
    reader: &R,
    dispatch_lease: Option<shell::TaskCredentialLease>,
) -> CommandResult<shell::ModelSyncStartedDto> {
    let (credential, access_authority) =
        credential_for_connection_with_reader(reader, shell, connection_id).await?;
    match dispatch_lease {
        Some(dispatch_lease) => shell
            .start_provider_model_sync_with_credential_authority_and_dispatch_lease(
                connection_id,
                credential,
                access_authority,
                dispatch_lease,
            ),
        None => shell.start_provider_model_sync_with_credential_authority(
            connection_id,
            credential,
            access_authority,
        ),
    }
    .map_err(Into::into)
}
