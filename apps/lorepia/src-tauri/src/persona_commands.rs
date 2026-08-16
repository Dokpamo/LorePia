//! Explicit Tauri commands for local personas and conversation selection.

use lorepia_shell_api as shell;
use tauri::State;

use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

#[tauri::command]
pub fn create_persona(
    state: State<'_, AppState>,
    request: shell::CreatePersonaInput,
) -> CommandResult<shell::PersonaDto> {
    state
        .shell()?
        .create_persona(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn update_persona(
    state: State<'_, AppState>,
    request: shell::UpdatePersonaInput,
) -> CommandResult<shell::PersonaDto> {
    state
        .shell()?
        .update_persona(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_persona(
    state: State<'_, AppState>,
    request: shell::GetPersonaInput,
) -> CommandResult<shell::PersonaDto> {
    state
        .shell()?
        .get_persona(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_personas(
    state: State<'_, AppState>,
    request: shell::ListPersonasInput,
) -> CommandResult<Vec<shell::PersonaDto>> {
    state
        .shell()?
        .list_personas(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_persona_page(
    state: State<'_, AppState>,
    request: shell::ListPersonasInput,
) -> CommandResult<shell::PersonaListPageDto> {
    state
        .shell()?
        .list_persona_page(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_persona(
    state: State<'_, AppState>,
    request: shell::DeletePersonaInput,
) -> CommandResult<shell::PersonaDeletionReceiptDto> {
    state
        .shell()?
        .delete_persona(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_conversation_persona_selection(
    state: State<'_, AppState>,
    request: shell::GetConversationPersonaSelectionInput,
) -> CommandResult<shell::ConversationPersonaSelectionDto> {
    state
        .shell()?
        .get_conversation_persona_selection(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn select_conversation_persona(
    state: State<'_, AppState>,
    request: shell::SelectConversationPersonaInput,
) -> CommandResult<shell::ConversationPersonaSelectionDto> {
    state
        .shell()?
        .select_conversation_persona(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn clear_conversation_persona(
    state: State<'_, AppState>,
    request: shell::ClearConversationPersonaInput,
) -> CommandResult<shell::ConversationPersonaSelectionDto> {
    state
        .shell()?
        .clear_conversation_persona(request)
        .map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use lorepia_shell_api as shell;
    use tauri::State;

    use crate::{error::CommandResult, state::AppState};

    #[test]
    fn raw_list_and_page_list_keep_distinct_response_contracts() {
        let _: for<'a> fn(
            State<'a, AppState>,
            shell::ListPersonasInput,
        ) -> CommandResult<Vec<shell::PersonaDto>> = super::list_personas;
        let _: for<'a> fn(
            State<'a, AppState>,
            shell::ListPersonasInput,
        ) -> CommandResult<shell::PersonaListPageDto> = super::list_persona_page;
    }
}
