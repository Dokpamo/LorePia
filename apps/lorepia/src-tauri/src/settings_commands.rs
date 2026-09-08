use tauri::State;

use crate::{error::CommandResult, state::AppState};

#[tauri::command]
pub fn get_storage_overview(
    state: State<'_, AppState>,
) -> CommandResult<lorepia_shell_api::StorageOverviewDto> {
    state.shell()?.get_storage_overview().map_err(Into::into)
}
