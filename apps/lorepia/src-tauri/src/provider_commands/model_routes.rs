use super::*;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSettingsRequest {
    pub settings: shell::AppSettingsDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectGenerationTargetRequest {
    pub target: Option<shell::GenerationTargetDto>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpsertModelRouteRequest {
    pub input: shell::UpsertModelRouteInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelRouteRequest {
    pub model_route_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPresetCandidateRequest {
    pub input: shell::GenerationPresetInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationPresetRequest {
    pub generation_preset_id: String,
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> CommandResult<shell::AppSettingsDto> {
    state.shell()?.get_settings().map_err(Into::into)
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    request: UpdateSettingsRequest,
) -> CommandResult<shell::AppSettingsDto> {
    state
        .shell()?
        .update_settings(request.settings)
        .map_err(Into::into)
}

#[tauri::command]
pub fn select_generation_target(
    state: State<'_, AppState>,
    request: SelectGenerationTargetRequest,
) -> CommandResult<shell::AppSettingsDto> {
    state
        .shell()?
        .select_generation_target(request.target)
        .map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_templates(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::ProviderTemplateDto>> {
    state.shell()?.list_provider_templates().map_err(Into::into)
}

#[tauri::command]
pub fn list_provider_profiles(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::ProviderProfileDto>> {
    state.shell()?.list_provider_profiles().map_err(Into::into)
}

#[tauri::command]
pub fn upsert_model_route(
    state: State<'_, AppState>,
    request: UpsertModelRouteRequest,
) -> CommandResult<shell::ModelRouteDto> {
    state
        .shell()?
        .upsert_model_route(request.input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn delete_model_route(
    state: State<'_, AppState>,
    request: ModelRouteRequest,
) -> CommandResult<()> {
    state
        .shell()?
        .delete_model_route(&request.model_route_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn upsert_generation_preset(
    state: State<'_, AppState>,
    request: GenerationPresetCandidateRequest,
) -> CommandResult<shell::GenerationPresetDto> {
    state
        .shell()?
        .upsert_generation_preset(request.input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn delete_generation_preset(
    state: State<'_, AppState>,
    request: GenerationPresetRequest,
) -> CommandResult<()> {
    state
        .shell()?
        .delete_generation_preset(&request.generation_preset_id)
        .map_err(Into::into)
}

#[tauri::command]
pub fn validate_generation_preset_candidate(
    state: State<'_, AppState>,
    request: GenerationPresetCandidateRequest,
) -> CommandResult<()> {
    state
        .shell()?
        .validate_generation_preset_candidate(request.input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn render_reasoning_control_for_preset(
    state: State<'_, AppState>,
    request: GenerationPresetCandidateRequest,
) -> CommandResult<shell::ReasoningControlDto> {
    state
        .shell()?
        .render_reasoning_control_for_preset(request.input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn render_prompt_cache_control_for_preset(
    state: State<'_, AppState>,
    request: GenerationPresetCandidateRequest,
) -> CommandResult<shell::PromptCacheControlDto> {
    state
        .shell()?
        .render_prompt_cache_control_for_preset(request.input)
        .map_err(Into::into)
}

#[tauri::command]
pub fn preview_provider_request_candidate(
    state: State<'_, AppState>,
    request: GenerationPresetCandidateRequest,
) -> CommandResult<shell::RequestPreviewDto> {
    state
        .shell()?
        .preview_provider_request_candidate(request.input)
        .map_err(Into::into)
}
