use lorepia_shell_api as shell;
use serde::Deserialize;
use tauri::State;

use super::model_routes::ModelRouteRequest;
use crate::{error::CommandResult, state::AppState};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveCapabilityRequest {
    pub model_route_id: String,
    pub key: shell::CapabilityKeyInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertCapabilityOverrideRequest {
    pub input: shell::UpsertCapabilityOverrideInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteCapabilityOverrideRequest {
    pub model_route_id: String,
    pub observation_id: String,
}

#[tauri::command]
pub fn list_capability_observations(
    state: State<'_, AppState>,
    request: ModelRouteRequest,
) -> CommandResult<Vec<shell::CapabilityObservationDto>> {
    state
        .shell()?
        .list_capability_observations(&request.model_route_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn effective_capability(
    state: State<'_, AppState>,
    request: EffectiveCapabilityRequest,
) -> CommandResult<Option<shell::EffectiveCapabilityDto>> {
    state
        .shell()?
        .effective_capability(&request.model_route_id, request.key)
        .map_err(Into::into)
}

#[tauri::command]
pub fn effective_parameter_specs(
    state: State<'_, AppState>,
    request: ModelRouteRequest,
) -> CommandResult<Vec<shell::ParameterSpecDto>> {
    state
        .shell()?
        .effective_parameter_specs(&request.model_route_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn upsert_user_capability_override(
    state: State<'_, AppState>,
    request: UpsertCapabilityOverrideRequest,
) -> CommandResult<shell::CapabilityObservationDto> {
    state
        .shell()?
        .upsert_user_capability_override(request.input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn delete_user_capability_override(
    state: State<'_, AppState>,
    request: DeleteCapabilityOverrideRequest,
) -> CommandResult<()> {
    state
        .shell()?
        .delete_user_capability_override(&request.model_route_id, &request.observation_id)
        .map_err(Into::into)
}
