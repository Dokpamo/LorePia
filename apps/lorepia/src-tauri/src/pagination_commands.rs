//! Strict, bounded creator and memory page commands.
use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};
use lorepia_shell_api::{
    CreatorDocumentsPageDto, ListCreatorDocumentsPageInput, ListMemoryRecordsPageInput,
    MemoryRecordsPageDto,
};
use tauri::State;

#[tauri::command]
pub fn list_creator_documents_page(
    state: State<'_, AppState>,
    request: ListCreatorDocumentsPageInput,
) -> CommandResult<CreatorDocumentsPageDto> {
    state
        .shell()?
        .list_creator_documents_page(request)
        .map_err(CommandError::from)
}
#[tauri::command]
pub fn list_memory_records_page(
    state: State<'_, AppState>,
    request: ListMemoryRecordsPageInput,
) -> CommandResult<MemoryRecordsPageDto> {
    state
        .shell()?
        .list_memory_records_page(request)
        .map_err(CommandError::from)
}
