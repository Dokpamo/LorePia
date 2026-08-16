//! Explicit Tauri commands for hash-bound content-module lifecycle review.

use lorepia_shell_api as shell;
use tauri::State;

use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

#[tauri::command]
pub fn list_content_module_lifecycle_candidates(
    state: State<'_, AppState>,
    request: shell::ListContentModuleLifecycleCandidatesInput,
) -> CommandResult<shell::ContentModuleLifecycleCandidatesDto> {
    execute_list_content_module_lifecycle_candidates(&state.shell()?, request)
}

pub(crate) fn execute_list_content_module_lifecycle_candidates(
    shell_api: &shell::ShellApi,
    request: shell::ListContentModuleLifecycleCandidatesInput,
) -> CommandResult<shell::ContentModuleLifecycleCandidatesDto> {
    shell_api
        .list_content_module_lifecycle_candidates(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_content_module_lifecycle_bindings(
    state: State<'_, AppState>,
    request: shell::ListContentModuleLifecycleBindingsInput,
) -> CommandResult<shell::ContentModuleLifecycleBindingsDto> {
    state
        .shell()?
        .list_content_module_lifecycle_bindings(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn review_content_module_activation(
    state: State<'_, AppState>,
    request: shell::ReviewContentModuleActivationInput,
) -> CommandResult<shell::ContentModuleActivationReviewDto> {
    state
        .shell()?
        .review_content_module_activation(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn resolve_content_module_activation(
    state: State<'_, AppState>,
    request: shell::ResolveContentModuleActivationInput,
) -> CommandResult<shell::ContentModuleActivationPlanDto> {
    state
        .shell()?
        .resolve_content_module_activation(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn activate_content_module(
    state: State<'_, AppState>,
    request: shell::ActivateContentModuleInput,
) -> CommandResult<shell::ContentModuleActivationReceiptDto> {
    state
        .shell()?
        .activate_content_module(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn review_content_module_deactivation(
    state: State<'_, AppState>,
    request: shell::ReviewContentModuleDeactivationInput,
) -> CommandResult<shell::ContentModuleDeactivationReviewDto> {
    state
        .shell()?
        .review_content_module_deactivation(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn deactivate_content_module(
    state: State<'_, AppState>,
    request: shell::DeactivateContentModuleInput,
) -> CommandResult<shell::ContentModuleDeactivationReceiptDto> {
    state
        .shell()?
        .deactivate_content_module(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn review_content_module_rollback(
    state: State<'_, AppState>,
    request: shell::ReviewContentModuleRollbackInput,
) -> CommandResult<shell::ContentModuleRollbackReviewDto> {
    state
        .shell()?
        .review_content_module_rollback(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn resolve_content_module_rollback(
    state: State<'_, AppState>,
    request: shell::ResolveContentModuleRollbackInput,
) -> CommandResult<shell::ContentModuleRollbackPlanDto> {
    state
        .shell()?
        .resolve_content_module_rollback(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn apply_content_module_rollback(
    state: State<'_, AppState>,
    request: shell::ApplyContentModuleRollbackInput,
) -> CommandResult<shell::ContentModuleActivationReceiptDto> {
    state
        .shell()?
        .apply_content_module_rollback(request)
        .map_err(CommandError::from)
}
