use super::*;

const MAXIMUM_SIGNED_CATALOG_BYTES: u64 = 4 * 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogImportTicketDto {
    pub ticket_id: String,
    pub plan: shell::ProviderCatalogImportPlanDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogTicketRequest {
    pub ticket_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogHistoryRequest {
    pub limit: u32,
    pub before_revision: Option<u64>,
    pub before_state_version: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCatalogDiffRequest {
    pub from_revision: u64,
    pub to_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareProviderCatalogRollbackRequest {
    pub target_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivateProviderCatalogRollbackRequest {
    pub plan: shell::ProviderCatalogRollbackPlanDto,
}

#[tauri::command]
pub async fn pick_provider_catalog_import(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Option<ProviderCatalogImportTicketDto>> {
    let shell = state.shell()?;
    let Some(bytes) = app
        .lorepia_platform()
        .pick_bounded_file(MAXIMUM_SIGNED_CATALOG_BYTES)
        .await?
    else {
        return Ok(None);
    };
    let envelope = shell::SignedCatalogEnvelope::new(bytes);
    let plan = shell.prepare_signed_provider_catalog_import(&envelope)?;
    let ticket_id = Uuid::new_v4().to_string();
    let response = ProviderCatalogImportTicketDto {
        ticket_id: ticket_id.clone(),
        plan: plan.clone(),
    };
    state.insert_catalog_ticket(ticket_id, CatalogImportTicket { plan, envelope })?;
    Ok(Some(response))
}

#[tauri::command]
pub fn activate_provider_catalog_import(
    state: State<'_, AppState>,
    request: ProviderCatalogTicketRequest,
) -> CommandResult<shell::ProviderCatalogImportResultDto> {
    let shell = state.shell()?;
    state.activate_catalog_ticket(&shell, &request.ticket_id)
}

#[tauri::command]
pub fn discard_provider_catalog_import(
    state: State<'_, AppState>,
    request: ProviderCatalogTicketRequest,
) -> CommandResult<()> {
    state.discard_catalog_ticket(&request.ticket_id)
}

#[tauri::command]
pub fn provider_catalog_status(
    state: State<'_, AppState>,
) -> CommandResult<shell::ProviderCatalogStatusDto> {
    state.shell()?.provider_catalog_status().map_err(Into::into)
}

#[tauri::command]
pub fn provider_catalog_history(
    state: State<'_, AppState>,
    request: ProviderCatalogHistoryRequest,
) -> CommandResult<shell::ProviderCatalogHistoryDto> {
    state
        .shell()?
        .provider_catalog_history(
            request.limit,
            request.before_revision,
            request.before_state_version,
        )
        .map_err(Into::into)
}

#[tauri::command]
pub fn diff_provider_catalog_revisions(
    state: State<'_, AppState>,
    request: ProviderCatalogDiffRequest,
) -> CommandResult<shell::ProviderCatalogDiffDto> {
    state
        .shell()?
        .diff_provider_catalog_revisions(request.from_revision, request.to_revision)
        .map_err(Into::into)
}

#[tauri::command]
pub fn prepare_provider_catalog_rollback(
    state: State<'_, AppState>,
    request: PrepareProviderCatalogRollbackRequest,
) -> CommandResult<shell::ProviderCatalogRollbackPlanDto> {
    state
        .shell()?
        .prepare_provider_catalog_rollback(request.target_revision)
        .map_err(Into::into)
}

#[tauri::command]
pub fn activate_provider_catalog_rollback(
    state: State<'_, AppState>,
    request: ActivateProviderCatalogRollbackRequest,
) -> CommandResult<shell::ProviderCatalogRollbackResultDto> {
    state
        .shell()?
        .activate_provider_catalog_rollback(request.plan)
        .map_err(Into::into)
}
