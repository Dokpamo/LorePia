//! Explicit Tauri commands for the UI-safe orchestration contract.
//!
//! The webview can submit only typed declarative documents. Core remains the
//! sole owner of validation, optimistic concurrency, storage, and execution.

use lorepia_shell_api as shell;
use serde::Deserialize;
use tauri::{AppHandle, State, ipc::Channel};

use crate::{
    error::{CommandError, CommandResult},
    state::AppState,
};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidatePromptPresetRequest {
    pub value: shell::CreatorPromptPresetDocumentDto,
}

#[tauri::command]
pub fn validate_prompt_preset(
    state: State<'_, AppState>,
    request: ValidatePromptPresetRequest,
) -> CommandResult<()> {
    state
        .shell()?
        .validate_editable_prompt_preset(request.value)
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn resolve_prompt_preview(
    app: AppHandle,
    state: State<'_, AppState>,
    request: shell::ResolvePromptPreviewInput,
) -> CommandResult<shell::ExpertPromptPreviewDto> {
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    state
        .shell()?
        .resolve_prompt_preview_async(
            request,
            &crate::state::PlatformTaskCredentialReader {
                app,
                shell: state.shell()?,
                inherited_dispatch_lease: None,
            },
            cancelled,
        )
        .await
        .map_err(CommandError::from)
}

#[tauri::command]
pub async fn send_reviewed_prompt(
    app: AppHandle,
    state: State<'_, AppState>,
    input: shell::ReviewedPromptSendInput,
    stream_id: String,
    on_event: Channel<shell::ChatStreamItem>,
) -> CommandResult<shell::GenerationStartedDto> {
    let registration = state.register_chat_stream(&stream_id)?;
    let shell_api = state.shell()?;
    let selection = shell::GenerationSelectionInput::Target {
        target: input.generation_target.clone(),
    };
    let dispatch_lease = crate::commands::generation_dispatch_lease(&state, &selection).await;
    let credential = crate::commands::credential_for_selection(
        &app,
        &state,
        &shell_api,
        &selection,
        dispatch_lease.clone(),
    )
    .await?;
    let (_cancel, cancelled) = tokio::sync::watch::channel(false);
    let started = shell_api
        .send_reviewed_prompt_async(
            input,
            credential,
            &crate::state::PlatformTaskCredentialReader {
                app,
                shell: shell_api.clone(),
                inherited_dispatch_lease: dispatch_lease,
            },
            cancelled,
        )
        .await?;
    let (response, stream) = started.into_parts();
    crate::channels::forward_chat_stream(stream, on_event, registration);
    Ok(response)
}

#[tauri::command]
pub fn explain_prompt_plan(
    state: State<'_, AppState>,
    request: shell::ExplainPromptPlanInput,
) -> CommandResult<shell::PromptResolutionTraceDto> {
    state
        .shell()?
        .explain_prompt_plan(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_orchestration_workspace(
    state: State<'_, AppState>,
    request: shell::GetOrchestrationWorkspaceInput,
) -> CommandResult<shell::OrchestrationWorkspaceSnapshotDto> {
    state
        .shell()?
        .get_orchestration_workspace(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn save_room_orchestration_config(
    state: State<'_, AppState>,
    request: shell::SaveRoomOrchestrationConfigInput,
) -> CommandResult<shell::SaveRoomOrchestrationConfigResultDto> {
    state
        .shell()?
        .save_room_orchestration_config(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn upsert_prompt_preset(
    state: State<'_, AppState>,
    request: shell::UpsertPromptPresetInput,
) -> CommandResult<shell::RevisionedDto<shell::PromptPresetSummaryDto>> {
    state
        .shell()?
        .upsert_prompt_preset_summary(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_prompt_preset(
    state: State<'_, AppState>,
    request: shell::GetPromptPresetInput,
) -> CommandResult<shell::RevisionedDto<shell::PromptPresetSummaryDto>> {
    state
        .shell()?
        .get_prompt_preset_summary(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_editable_prompt_preset(
    state: State<'_, AppState>,
    request: shell::GetPromptPresetInput,
) -> CommandResult<shell::RevisionedDto<shell::CreatorPromptPresetDocumentDto>> {
    state
        .shell()?
        .get_editable_prompt_preset(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_prompt_presets(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::RevisionedDto<shell::PromptPresetSummaryDto>>> {
    state
        .shell()?
        .list_prompt_preset_summaries()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_prompt_preset_revisions(
    state: State<'_, AppState>,
    request: shell::ListPromptPresetRevisionsInput,
) -> CommandResult<shell::PromptPresetRevisionListDto> {
    state
        .shell()?
        .list_prompt_preset_revisions(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn diff_prompt_preset_revisions(
    state: State<'_, AppState>,
    request: shell::DiffPromptPresetRevisionsInput,
) -> CommandResult<shell::PromptPresetRevisionDiffDto> {
    state
        .shell()?
        .diff_prompt_preset_revisions(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn review_prompt_preset_rollback(
    state: State<'_, AppState>,
    request: shell::ReviewPromptPresetRollbackInput,
) -> CommandResult<shell::PromptPresetRollbackReviewDto> {
    state
        .shell()?
        .review_prompt_preset_rollback(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn apply_prompt_preset_rollback(
    state: State<'_, AppState>,
    request: shell::ApplyPromptPresetRollbackInput,
) -> CommandResult<shell::PromptPresetRollbackReceiptDto> {
    state
        .shell()?
        .apply_prompt_preset_rollback(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn reorder_prompt_blocks(
    state: State<'_, AppState>,
    request: shell::ReorderPromptBlocksInput,
) -> CommandResult<shell::ReorderPromptBlocksResultDto> {
    state
        .shell()?
        .reorder_prompt_blocks(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_prompt_preset(
    state: State<'_, AppState>,
    request: shell::DeletePromptPresetInput,
) -> CommandResult<shell::RevisionedDto<shell::PromptPresetSummaryDto>> {
    state
        .shell()?
        .delete_prompt_preset_summary(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn upsert_task_profile(
    state: State<'_, AppState>,
    request: shell::UpsertTaskProfileInput,
) -> CommandResult<shell::RevisionedDto<shell::TaskProfileDto>> {
    state
        .shell()?
        .upsert_task_profile(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_task_profile(
    state: State<'_, AppState>,
    request: shell::GetTaskProfileInput,
) -> CommandResult<shell::RevisionedDto<shell::TaskProfileDto>> {
    state
        .shell()?
        .get_task_profile(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_task_profiles(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::RevisionedDto<shell::TaskProfileDto>>> {
    state
        .shell()?
        .list_task_profiles()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_task_profile(
    state: State<'_, AppState>,
    request: shell::DeleteTaskProfileInput,
) -> CommandResult<shell::RevisionedDto<shell::TaskProfileDto>> {
    state
        .shell()?
        .delete_task_profile(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn upsert_memory_profile(
    state: State<'_, AppState>,
    request: shell::UpsertMemoryProfileInput,
) -> CommandResult<shell::RevisionedDto<shell::MemoryProfileDto>> {
    state
        .shell()?
        .upsert_memory_profile(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_memory_profile(
    state: State<'_, AppState>,
    request: shell::GetMemoryProfileInput,
) -> CommandResult<shell::RevisionedDto<shell::MemoryProfileDto>> {
    state
        .shell()?
        .get_memory_profile(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_memory_profiles(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::RevisionedDto<shell::MemoryProfileDto>>> {
    state
        .shell()?
        .list_memory_profiles()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_memory_profile(
    state: State<'_, AppState>,
    request: shell::DeleteMemoryProfileInput,
) -> CommandResult<shell::RevisionedDto<shell::MemoryProfileDto>> {
    state
        .shell()?
        .delete_memory_profile(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_memory_record(
    state: State<'_, AppState>,
    request: shell::GetMemoryRecordInput,
) -> CommandResult<shell::MemoryRecordProjectionDto> {
    execute_get_memory_record(&state.shell()?, request)
}

pub(crate) fn execute_get_memory_record(
    shell_api: &shell::ShellApi,
    request: shell::GetMemoryRecordInput,
) -> CommandResult<shell::MemoryRecordProjectionDto> {
    shell_api
        .get_memory_record_projection(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn patch_memory_record(
    state: State<'_, AppState>,
    request: shell::PatchMemoryRecordInput,
) -> CommandResult<shell::MemoryRecordProjectionDto> {
    execute_patch_memory_record(&state.shell()?, request)
}

pub(crate) fn execute_patch_memory_record(
    shell_api: &shell::ShellApi,
    request: shell::PatchMemoryRecordInput,
) -> CommandResult<shell::MemoryRecordProjectionDto> {
    shell_api
        .patch_memory_record(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn set_memory_record_exclusion(
    state: State<'_, AppState>,
    request: shell::SetMemoryRecordExclusionInput,
) -> CommandResult<shell::MemoryRecordProjectionDto> {
    execute_set_memory_record_exclusion(&state.shell()?, request)
}

pub(crate) fn execute_set_memory_record_exclusion(
    shell_api: &shell::ShellApi,
    request: shell::SetMemoryRecordExclusionInput,
) -> CommandResult<shell::MemoryRecordProjectionDto> {
    shell_api
        .set_memory_record_exclusion(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_memory_record(
    state: State<'_, AppState>,
    request: shell::DeleteMemoryRecordInput,
) -> CommandResult<()> {
    execute_delete_memory_record(&state.shell()?, request)
}

pub(crate) fn execute_delete_memory_record(
    shell_api: &shell::ShellApi,
    request: shell::DeleteMemoryRecordInput,
) -> CommandResult<()> {
    shell_api
        .delete_memory_record(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn upsert_knowledge_book(
    state: State<'_, AppState>,
    request: shell::UpsertKnowledgeBookInput,
) -> CommandResult<shell::RevisionedDto<shell::KnowledgeBookDto>> {
    state
        .shell()?
        .upsert_knowledge_book(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_knowledge_book(
    state: State<'_, AppState>,
    request: shell::GetKnowledgeBookInput,
) -> CommandResult<shell::RevisionedDto<shell::KnowledgeBookDto>> {
    state
        .shell()?
        .get_knowledge_book(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_knowledge_books(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::RevisionedDto<shell::KnowledgeBookDto>>> {
    state
        .shell()?
        .list_knowledge_books()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_knowledge_book(
    state: State<'_, AppState>,
    request: shell::DeleteKnowledgeBookInput,
) -> CommandResult<shell::RevisionedDto<shell::KnowledgeBookDto>> {
    state
        .shell()?
        .delete_knowledge_book(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn upsert_transform_set(
    state: State<'_, AppState>,
    request: shell::UpsertTransformSetInput,
) -> CommandResult<shell::RevisionedDto<shell::TransformSetDto>> {
    state
        .shell()?
        .upsert_transform_set(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_transform_set(
    state: State<'_, AppState>,
    request: shell::GetTransformSetInput,
) -> CommandResult<shell::RevisionedDto<shell::TransformSetDto>> {
    state
        .shell()?
        .get_transform_set(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_transform_sets(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::RevisionedDto<shell::TransformSetDto>>> {
    state
        .shell()?
        .list_transform_sets()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_transform_set(
    state: State<'_, AppState>,
    request: shell::DeleteTransformSetInput,
) -> CommandResult<shell::RevisionedDto<shell::TransformSetDto>> {
    state
        .shell()?
        .delete_transform_set(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn upsert_interaction_rule_set(
    state: State<'_, AppState>,
    request: shell::UpsertInteractionRuleSetInput,
) -> CommandResult<shell::RevisionedDto<shell::InteractionRuleSetDto>> {
    state
        .shell()?
        .upsert_interaction_rule_set(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_interaction_rule_set(
    state: State<'_, AppState>,
    request: shell::GetInteractionRuleSetInput,
) -> CommandResult<shell::RevisionedDto<shell::InteractionRuleSetDto>> {
    state
        .shell()?
        .get_interaction_rule_set(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_interaction_rule_sets(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::RevisionedDto<shell::InteractionRuleSetDto>>> {
    state
        .shell()?
        .list_interaction_rule_sets()
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_interaction_rule_set(
    state: State<'_, AppState>,
    request: shell::DeleteInteractionRuleSetInput,
) -> CommandResult<shell::RevisionedDto<shell::InteractionRuleSetDto>> {
    state
        .shell()?
        .delete_interaction_rule_set(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_interaction_effects(
    state: State<'_, AppState>,
) -> CommandResult<Vec<crate::contract::InteractionEffectEventDto>> {
    state.list_interaction_effects()
}

#[tauri::command]
pub fn list_interaction_proposals(
    state: State<'_, AppState>,
    request: shell::ListInteractionProposalsInput,
) -> CommandResult<Vec<shell::InteractionProposalListItemDto>> {
    state
        .shell()?
        .list_interaction_proposals(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_generation_attempt_proposals(
    state: State<'_, AppState>,
    request: shell::ListGenerationAttemptProposalsInput,
) -> CommandResult<Vec<shell::GenerationAttemptProposalListItemDto>> {
    state
        .shell()?
        .list_generation_attempt_proposals(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_retryable_generation_attempts(
    state: State<'_, AppState>,
    request: shell::ListRetryableGenerationAttemptsInput,
) -> CommandResult<Vec<shell::RetryableGenerationAttemptDto>> {
    state
        .shell()?
        .list_retryable_generation_attempts(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn expire_interaction_proposals(
    state: State<'_, AppState>,
    request: shell::ExpireInteractionProposalsInput,
) -> CommandResult<shell::InteractionProposalExpiryReceiptDto> {
    state
        .shell()?
        .expire_interaction_proposals(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn expire_generation_attempt_proposals(
    state: State<'_, AppState>,
    request: shell::ExpireGenerationAttemptProposalsInput,
) -> CommandResult<shell::GenerationAttemptProposalExpiryReceiptDto> {
    state
        .shell()?
        .expire_generation_attempt_proposals(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_interaction_effect_history(
    state: State<'_, AppState>,
    request: shell::ListInteractionEffectHistoryInput,
) -> CommandResult<shell::InteractionEffectHistoryPageDto> {
    state
        .shell()?
        .list_interaction_effect_history(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_reopen_interaction_effects(
    state: State<'_, AppState>,
    request: shell::ListRecentReopenInteractionEffectsInput,
) -> CommandResult<shell::InteractionReopenSnapshotDto> {
    state
        .shell()?
        .list_reopen_interaction_effects(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn submit_interaction_choice(
    state: State<'_, AppState>,
    request: shell::SubmitInteractionChoiceInput,
) -> CommandResult<shell::InteractionChoiceSelectionReceiptDto> {
    state
        .shell()?
        .submit_interaction_choice(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn acknowledge_interaction_effect(
    state: State<'_, AppState>,
    request: crate::contract::InteractionEffectDeliveryRequest,
) -> CommandResult<()> {
    state.acknowledge_interaction_effect(&request.delivery_id)
}

#[tauri::command]
pub fn retry_interaction_effect(
    state: State<'_, AppState>,
    request: crate::contract::InteractionEffectDeliveryRequest,
) -> CommandResult<()> {
    state.retry_interaction_effect(&request.delivery_id)
}

#[tauri::command]
pub fn decide_interaction_proposal(
    state: State<'_, AppState>,
    request: shell::DecideInteractionProposalInput,
) -> CommandResult<shell::InteractionProposalDecisionReceiptDto> {
    state
        .shell()?
        .decide_interaction_proposal(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn decide_generation_attempt_proposal(
    state: State<'_, AppState>,
    request: shell::DecideGenerationAttemptProposalInput,
) -> CommandResult<shell::GenerationAttemptProposalDecisionReceiptDto> {
    state
        .shell()?
        .decide_generation_attempt_proposal(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn upsert_content_module(
    state: State<'_, AppState>,
    request: shell::UpsertContentModuleInput,
) -> CommandResult<shell::RevisionedDto<shell::ContentModuleDto>> {
    state
        .shell()?
        .upsert_content_module(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn get_content_module(
    state: State<'_, AppState>,
    request: shell::GetContentModuleInput,
) -> CommandResult<shell::RevisionedDto<shell::ContentModuleDto>> {
    state
        .shell()?
        .get_content_module(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_content_modules(
    state: State<'_, AppState>,
) -> CommandResult<Vec<shell::RevisionedDto<shell::ContentModuleDto>>> {
    execute_list_content_modules(&state.shell()?)
}

pub(crate) fn execute_list_content_modules(
    shell_api: &shell::ShellApi,
) -> CommandResult<Vec<shell::RevisionedDto<shell::ContentModuleDto>>> {
    shell_api.list_content_modules().map_err(CommandError::from)
}

#[tauri::command]
pub fn delete_content_module(
    state: State<'_, AppState>,
    request: shell::DeleteContentModuleInput,
) -> CommandResult<shell::RevisionedDto<shell::ContentModuleDto>> {
    state
        .shell()?
        .delete_content_module(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_prompt_preset_bindings(
    state: State<'_, AppState>,
    request: shell::ListPromptPresetBindingsInput,
) -> CommandResult<Vec<shell::RevisionedDto<shell::PromptPresetBindingDto>>> {
    state
        .shell()?
        .list_prompt_preset_bindings(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_memory_records(
    state: State<'_, AppState>,
    request: shell::ListMemoryRecordsInput,
) -> CommandResult<shell::MemoryRecordListDto> {
    execute_list_memory_records(&state.shell()?, request)
}

pub(crate) fn execute_list_memory_records(
    shell_api: &shell::ShellApi,
    request: shell::ListMemoryRecordsInput,
) -> CommandResult<shell::MemoryRecordListDto> {
    shell_api
        .list_memory_records(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_interrupted_memory_jobs(
    state: State<'_, AppState>,
    request: shell::ListInterruptedMemoryJobsInput,
) -> CommandResult<Vec<shell::InterruptedMemoryJobDto>> {
    state
        .shell()?
        .list_interrupted_memory_jobs(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn retry_interrupted_memory_job(
    state: State<'_, AppState>,
    request: shell::RetryInterruptedMemoryJobInput,
) -> CommandResult<shell::MemoryJobRetryReceiptDto> {
    execute_retry_interrupted_memory_job(&state.shell()?, request)
}

pub(crate) fn execute_retry_interrupted_memory_job(
    shell_api: &shell::ShellApi,
    request: shell::RetryInterruptedMemoryJobInput,
) -> CommandResult<shell::MemoryJobRetryReceiptDto> {
    shell_api
        .retry_interrupted_memory_job(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_retryable_memory_query_embeddings(
    state: State<'_, AppState>,
    request: shell::ListRetryableMemoryQueryEmbeddingsInput,
) -> CommandResult<Vec<shell::MemoryQueryEmbeddingRetryCandidateDto>> {
    state
        .shell()?
        .list_retryable_memory_query_embeddings(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn retry_memory_query_embedding(
    state: State<'_, AppState>,
    request: shell::RetryMemoryQueryEmbeddingInput,
) -> CommandResult<shell::MemoryQueryEmbeddingRetryCandidateDto> {
    state
        .shell()?
        .retry_memory_query_embedding(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn simulate_knowledge_activation(
    state: State<'_, AppState>,
    request: shell::SimulateKnowledgeInput,
) -> CommandResult<shell::KnowledgeSimulationDto> {
    state
        .shell()?
        .simulate_knowledge(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn preview_transform_rule(
    state: State<'_, AppState>,
    request: shell::PreviewTransformRuleInput,
) -> CommandResult<shell::TransformRulePreviewDto> {
    state
        .shell()?
        .preview_transform_rule(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_content_module_bindings(
    state: State<'_, AppState>,
    request: shell::ListContentModuleBindingsInput,
) -> CommandResult<Vec<shell::RevisionedDto<shell::ModuleBindingDto>>> {
    state
        .shell()?
        .list_content_module_bindings(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn list_content_module_revisions(
    state: State<'_, AppState>,
    request: shell::ListContentModuleRevisionsInput,
) -> CommandResult<shell::ContentModuleRevisionListDto> {
    state
        .shell()?
        .list_content_module_revisions(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn diff_content_module_revisions(
    state: State<'_, AppState>,
    request: shell::DiffContentModuleRevisionsInput,
) -> CommandResult<shell::ContentModuleRevisionDiffDto> {
    state
        .shell()?
        .diff_content_module_revisions(request)
        .map_err(CommandError::from)
}

#[tauri::command]
pub fn evaluate_content_module_share(
    state: State<'_, AppState>,
    request: shell::EvaluateContentModuleShareInput,
) -> CommandResult<shell::ContentShareGateDto> {
    state
        .shell()?
        .evaluate_content_module_share(request)
        .map_err(CommandError::from)
}

#[cfg(test)]
mod tests {
    use lorepia_shell_api as shell;
    use tempfile::{TempDir, tempdir};

    use super::{
        ValidatePromptPresetRequest, execute_delete_memory_record, execute_get_memory_record,
        execute_list_memory_records, execute_patch_memory_record,
        execute_retry_interrupted_memory_job, execute_set_memory_record_exclusion,
    };

    #[test]
    fn prompt_validation_request_rejects_unknown_fields_before_core() {
        let error = serde_json::from_value::<ValidatePromptPresetRequest>(serde_json::json!({
            "value": null,
            "execute": "forbidden"
        }))
        .expect_err("unknown fields must be rejected");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn interrupted_memory_job_retry_handler_preserves_explicit_acknowledgement_gate() {
        let root = tempdir().expect("temporary Tauri command data root");
        let shell_api =
            shell::test_support::open_data_root_after_drop(root.path()).expect("open Shell");

        let error = execute_retry_interrupted_memory_job(
            &shell_api,
            shell::RetryInterruptedMemoryJobInput {
                conversation_id: "conversation:synthetic".to_owned(),
                branch_id: "branch:synthetic".to_owned(),
                memory_job_id: "synthetic.interrupted-memory-job".to_owned(),
                expected_revision: 2,
                acknowledge_unknown_outcome: false,
            },
        )
        .expect_err("Tauri handler must retain the explicit unknown-outcome gate");

        assert_eq!(error.code, "permission_denied");
        assert!(error.recoverable);
    }

    struct MemoryCommandFixture {
        root: TempDir,
        conversation: String,
        branch: String,
        record: String,
    }

    fn memory_command_fixture() -> MemoryCommandFixture {
        let root = tempdir().expect("temporary Tauri command data root");
        let fixture = shell::test_support::seed_synthetic_memory_record_fixture(root.path())
            .expect("seed bounded Shell memory fixture");
        MemoryCommandFixture {
            root,
            conversation: fixture.conversation_id,
            branch: fixture.branch_id,
            record: fixture.memory_record_id,
        }
    }

    fn assert_handler_memory_owner_mismatch(
        shell_api: &shell::ShellApi,
        fixture: &MemoryCommandFixture,
        conversation_id: &str,
        branch_id: &str,
        expected_revision: u64,
        mismatch: &str,
    ) {
        let get = execute_get_memory_record(
            shell_api,
            shell::GetMemoryRecordInput {
                conversation_id: conversation_id.to_owned(),
                branch_id: branch_id.to_owned(),
                memory_record_id: fixture.record.clone(),
            },
        )
        .unwrap_err();
        assert_eq!(get.code, "not_found", "{mismatch} get");

        let patch = execute_patch_memory_record(
            shell_api,
            shell::PatchMemoryRecordInput {
                conversation_id: conversation_id.to_owned(),
                branch_id: branch_id.to_owned(),
                memory_record_id: fixture.record.clone(),
                patch: shell::MemoryRecordPatchDto {
                    title: Some("foreign handler overwrite".to_owned()),
                    ..shell::MemoryRecordPatchDto::default()
                },
                expected_revision,
            },
        )
        .unwrap_err();
        assert_eq!(patch.code, "not_found", "{mismatch} patch");

        let exclusion = execute_set_memory_record_exclusion(
            shell_api,
            shell::SetMemoryRecordExclusionInput {
                conversation_id: conversation_id.to_owned(),
                branch_id: branch_id.to_owned(),
                memory_record_id: fixture.record.clone(),
                scope: shell::MemoryRecordExclusionScopeDto::Conversation,
                excluded: true,
                expected_revision,
            },
        )
        .unwrap_err();
        assert_eq!(exclusion.code, "not_found", "{mismatch} exclusion");

        let delete = execute_delete_memory_record(
            shell_api,
            shell::DeleteMemoryRecordInput {
                conversation_id: conversation_id.to_owned(),
                branch_id: branch_id.to_owned(),
                memory_record_id: fixture.record.clone(),
                expected_revision,
            },
        )
        .unwrap_err();
        assert_eq!(delete.code, "not_found", "{mismatch} delete");
    }

    fn assert_stale_memory_patch_is_rejected(
        shell_api: &shell::ShellApi,
        fixture: &MemoryCommandFixture,
        stale_revision: u64,
        expected: &shell::MemoryRecordProjectionDto,
    ) {
        let stale = execute_patch_memory_record(
            shell_api,
            shell::PatchMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                patch: shell::MemoryRecordPatchDto {
                    title: Some("Rejected stale overwrite".to_owned()),
                    ..shell::MemoryRecordPatchDto::default()
                },
                expected_revision: stale_revision,
            },
        )
        .expect_err("stale handler mutation must fail");
        assert_eq!(stale.code, "invalid_input");
        assert!(stale.recoverable);
        let after_stale = execute_get_memory_record(
            shell_api,
            shell::GetMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
            },
        )
        .expect("read after stale rejection");
        assert_eq!(&after_stale, expected);
    }

    fn mutate_memory_through_handlers(
        shell_api: &shell::ShellApi,
        fixture: &MemoryCommandFixture,
    ) -> shell::MemoryRecordProjectionDto {
        let initial = execute_get_memory_record(
            shell_api,
            shell::GetMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
            },
        )
        .expect("handler get");
        assert_eq!(initial.revision, 1);
        let edited = execute_patch_memory_record(
            shell_api,
            shell::PatchMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                patch: shell::MemoryRecordPatchDto {
                    title: Some("Handler-edited memory".to_owned()),
                    summary: Some("Handler-edited exact summary".to_owned()),
                    importance: Some(91),
                    keywords: Some(vec!["handler".to_owned(), "exact-cas".to_owned()]),
                    pinned: None,
                },
                expected_revision: initial.revision,
            },
        )
        .expect("handler edit");
        assert_eq!(edited.revision, 2);
        assert_eq!(edited.title, "Handler-edited memory");
        assert_eq!(edited.summary, "Handler-edited exact summary");
        assert_eq!(edited.importance, 91);
        assert_eq!(edited.keywords, ["handler", "exact-cas"]);
        assert!(!edited.pinned);
        let pinned = execute_patch_memory_record(
            shell_api,
            shell::PatchMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                patch: shell::MemoryRecordPatchDto {
                    pinned: Some(true),
                    ..shell::MemoryRecordPatchDto::default()
                },
                expected_revision: edited.revision,
            },
        )
        .expect("handler pin");
        assert_eq!(pinned.revision, 3);
        assert!(pinned.pinned);
        let conversation_excluded = execute_set_memory_record_exclusion(
            shell_api,
            shell::SetMemoryRecordExclusionInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                scope: shell::MemoryRecordExclusionScopeDto::Conversation,
                excluded: true,
                expected_revision: pinned.revision,
            },
        )
        .expect("handler conversation exclusion");
        assert_eq!(conversation_excluded.revision, 4);
        let character_excluded = execute_set_memory_record_exclusion(
            shell_api,
            shell::SetMemoryRecordExclusionInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                scope: shell::MemoryRecordExclusionScopeDto::Character,
                excluded: true,
                expected_revision: conversation_excluded.revision,
            },
        )
        .expect("handler character exclusion");
        assert_eq!(character_excluded.revision, 5);
        assert!(character_excluded.excluded_from_conversation);
        assert!(character_excluded.excluded_from_character);
        assert_stale_memory_patch_is_rejected(
            shell_api,
            fixture,
            initial.revision,
            &character_excluded,
        );
        character_excluded
    }

    #[test]
    fn memory_command_handlers_execute_exact_cas_and_restart_delete_vertical() {
        let fixture = memory_command_fixture();
        let shell_api = shell::test_support::open_data_root_after_drop(fixture.root.path())
            .expect("open command fixture Shell");
        let final_projection = mutate_memory_through_handlers(&shell_api, &fixture);
        let listed = execute_list_memory_records(
            &shell_api,
            shell::ListMemoryRecordsInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                include_invalidated: false,
            },
        )
        .expect("handler list after mutations");
        assert_eq!(
            listed.records.as_slice(),
            std::slice::from_ref(&final_projection)
        );
        drop(shell_api);

        let reopened = shell::test_support::open_data_root_after_drop(fixture.root.path())
            .expect("reopen command fixture Shell");
        let readback = execute_get_memory_record(
            &reopened,
            shell::GetMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
            },
        )
        .expect("handler restart readback");
        assert_eq!(readback, final_projection);
        execute_delete_memory_record(
            &reopened,
            shell::DeleteMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
                expected_revision: final_projection.revision,
            },
        )
        .expect("handler exact delete");
        drop(reopened);

        let deleted = shell::test_support::open_data_root_after_drop(fixture.root.path())
            .expect("reopen deleted command fixture Shell");
        let error = execute_get_memory_record(
            &deleted,
            shell::GetMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
            },
        )
        .expect_err("deleted record must stay absent");
        assert_eq!(error.code, "not_found");
        let listed = execute_list_memory_records(
            &deleted,
            shell::ListMemoryRecordsInput {
                conversation_id: fixture.conversation,
                branch_id: fixture.branch,
                include_invalidated: true,
            },
        )
        .expect("handler list after delete and reopen");
        assert!(listed.records.is_empty());
    }

    #[test]
    fn memory_command_handlers_reject_each_partial_owner_mismatch() {
        let fixture = memory_command_fixture();
        let shell_api = shell::test_support::open_data_root_after_drop(fixture.root.path())
            .expect("open command fixture Shell");
        let initial = execute_get_memory_record(
            &shell_api,
            shell::GetMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
            },
        )
        .expect("initial owner-bound read");

        for (conversation_id, branch_id, mismatch) in [
            (
                "synthetic.foreign.conversation",
                fixture.branch.as_str(),
                "conversation",
            ),
            (
                fixture.conversation.as_str(),
                "synthetic.foreign.branch",
                "branch",
            ),
        ] {
            assert_handler_memory_owner_mismatch(
                &shell_api,
                &fixture,
                conversation_id,
                branch_id,
                initial.revision,
                mismatch,
            );
        }

        let unchanged = execute_get_memory_record(
            &shell_api,
            shell::GetMemoryRecordInput {
                conversation_id: fixture.conversation.clone(),
                branch_id: fixture.branch.clone(),
                memory_record_id: fixture.record.clone(),
            },
        )
        .expect("read after rejected owner mismatches");
        assert_eq!(unchanged, initial);
    }
}
