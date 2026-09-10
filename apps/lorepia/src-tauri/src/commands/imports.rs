use lorepia_shell_api::{ImportCommitResultDto, ImportInspectionDto, StagedImportFile};
use tauri::{AppHandle, State};
use tauri_plugin_lorepia_platform::{
    LorepiaPlatformExt, MAXIMUM_STANDARD_IMPORT_BYTES, MAXIMUM_USER_APPROVED_IMPORT_BYTES,
};
use uuid::Uuid;

use crate::{
    contract::{
        DiscardImportRequest, ImportResourcePolicyDto, ImportTicketDto, InspectionRequest,
        PickImportRequest, TicketRequest,
    },
    error::{CommandError, CommandResult},
    state::AppState,
};

#[tauri::command]
pub async fn pick_import(
    app: AppHandle,
    state: State<'_, AppState>,
    request: PickImportRequest,
) -> CommandResult<Option<ImportTicketDto>> {
    state.ensure_ready()?;
    let maximum_bytes = match request.resource_policy {
        ImportResourcePolicyDto::Standard => MAXIMUM_STANDARD_IMPORT_BYTES,
        ImportResourcePolicyDto::UserApprovedLarge => MAXIMUM_USER_APPROVED_IMPORT_BYTES,
    };
    let Some(staged) = app
        .lorepia_platform()
        .pick_import_with_limit(maximum_bytes)
        .await?
    else {
        return Ok(None);
    };
    let ticket_id = Uuid::new_v4().to_string();
    let response = ImportTicketDto {
        ticket_id: ticket_id.clone(),
        display_name: staged.display_name().to_owned(),
        size_bytes: staged.size_bytes(),
    };
    state.insert_import_ticket(ticket_id, staged)?;
    Ok(Some(response))
}

#[tauri::command]
pub async fn inspect_import(
    app: AppHandle,
    state: State<'_, AppState>,
    request: TicketRequest,
) -> CommandResult<ImportInspectionDto> {
    let staged = state.take_import_ticket(&request.ticket_id)?;
    let shell = state.shell()?;
    let staged_file = StagedImportFile::new(staged.path());
    let inspection = match staged.maximum_bytes() {
        MAXIMUM_STANDARD_IMPORT_BYTES => shell.inspect_import(&staged_file),
        MAXIMUM_USER_APPROVED_IMPORT_BYTES => {
            shell.inspect_import_user_approved_large(&staged_file)
        }
        _ => return Err(CommandError::invalid_input()),
    }
    .map_err(CommandError::from);
    let cleanup = app
        .lorepia_platform()
        .discard_staged_import(&staged)
        .await
        .map_err(CommandError::from);

    match (inspection, cleanup) {
        (Ok(inspection), Ok(())) => Ok(inspection),
        (Ok(inspection), Err(cleanup_error)) => {
            let _ = shell.discard_import(&inspection.inspection_id);
            Err(cleanup_error)
        }
        (Err(error), _) => Err(error),
    }
}

#[tauri::command]
pub fn commit_import(
    state: State<'_, AppState>,
    request: InspectionRequest,
) -> CommandResult<ImportCommitResultDto> {
    state
        .shell()?
        .commit_compatible_import(&request.inspection_id)
        .map_err(Into::into)
}

#[tauri::command]
pub async fn discard_import(
    app: AppHandle,
    state: State<'_, AppState>,
    request: DiscardImportRequest,
) -> CommandResult<()> {
    match request {
        DiscardImportRequest::Inspection { inspection_id } => state
            .shell()?
            .discard_import(&inspection_id)
            .map_err(Into::into),
        DiscardImportRequest::Ticket { ticket_id } => {
            let reservation = state.reserve_import_ticket(&ticket_id)?;
            match app
                .lorepia_platform()
                .discard_staged_import(reservation.value())
                .await
            {
                Ok(()) => reservation.complete(),
                Err(error) => Err(error.into()),
            }
        }
    }
}
